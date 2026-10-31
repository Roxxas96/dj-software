//! Startup backups: before the library is opened for writing, the existing database is
//! copied with SQLite's online backup API to
//! `<dir>/backups/library-<UTC yyyymmdd-hhmmss>.sqlite3`, and only the newest
//! [`KEEP`] backups are kept.
//!
//! A backup is always taken when a migration is pending. Otherwise it is skipped when a
//! backup less than [`MIN_INTERVAL`] old already exists, so frequent short-lived opens
//! (CLI commands) cannot rotate the real backups away.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rusqlite::backup::{Backup, StepResult};
use rusqlite::{Connection, MAIN_DB, OpenFlags};

use crate::LibraryError;

/// How many backups survive pruning.
pub(crate) const KEEP: usize = 10;

/// Without a pending migration, no new backup is taken if one is younger than this.
pub(crate) const MIN_INTERVAL: Duration = Duration::from_secs(10 * 60);

/// How long the copy waits for a writer elsewhere to release its lock before giving up.
const LOCK_WAIT: Duration = Duration::from_secs(10);

/// `.partial` files older than this are leftovers of an interrupted backup.
const STALE_PARTIAL: Duration = Duration::from_secs(10 * 60);

const PREFIX: &str = "library-";
const SUFFIX: &str = ".sqlite3";
const PARTIAL_SUFFIX: &str = ".sqlite3.partial";

/// Backs up `db` (which must exist) into `<db dir>/backups/` when the policy calls for it
/// (see the module docs), then prunes old backups. `latest` is the schema version this
/// build migrates to. Returns the new backup's path, or `None` when it was skipped.
pub(crate) fn backup_if_needed(db: &Path, latest: usize) -> Result<Option<PathBuf>, LibraryError> {
    let dir = db
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("backups");
    std::fs::create_dir_all(&dir)?;

    // Read-write without create: a read-only open of a WAL database can fail when the
    // shared-memory file is absent, and nothing here writes to the source.
    let src = Connection::open_with_flags(
        db,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(LibraryError::Backup)?;
    src.busy_timeout(LOCK_WAIT).map_err(LibraryError::Backup)?;
    let version: i64 = src
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(LibraryError::Backup)?;
    let migration_pending = usize::try_from(version).is_ok_and(|v| v < latest);
    if !migration_pending && has_recent_backup(&dir, SystemTime::now()) {
        return Ok(None);
    }

    let target = unique_name(&dir, &utc_stamp(SystemTime::now()));
    // Copy to a temporary name first so an interrupted backup never looks like a real one.
    let partial = target.with_extension("sqlite3.partial");
    let _ = std::fs::remove_file(&partial);
    copy(&src, &partial).map_err(LibraryError::Backup)?;
    drop(src);
    std::fs::rename(&partial, &target)?;

    prune(&dir, KEEP, &target);
    Ok(Some(target))
}

/// Copies the whole database in one step (a consistent snapshot), retrying while another
/// connection holds a conflicting lock, for up to [`LOCK_WAIT`].
fn copy(src: &Connection, dst_path: &Path) -> rusqlite::Result<()> {
    let mut dst = Connection::open(dst_path)?;
    let backup = Backup::new_with_names(src, MAIN_DB, &mut dst, MAIN_DB)?;
    let deadline = Instant::now() + LOCK_WAIT;
    loop {
        match backup.step(-1)? {
            StepResult::Done => return Ok(()),
            StepResult::More => {}
            // Busy, Locked (and any future transient result): wait for the lock holder.
            _ if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            _ => {
                return Err(rusqlite::Error::SqliteFailure(
                    rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_BUSY),
                    Some("the library stayed locked during the backup".into()),
                ));
            }
        }
    }
}

/// Whether `dir` holds a backup written less than [`MIN_INTERVAL`] before `now`. A
/// modification time in the future (clock skew) does not count as recent.
fn has_recent_backup(dir: &Path, now: SystemTime) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    entries.flatten().any(|entry| {
        entry.file_name().to_str().and_then(backup_key).is_some()
            && entry
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|modified| now.duration_since(modified).ok())
                .is_some_and(|age| age < MIN_INTERVAL)
    })
}

/// `library-<stamp>.sqlite3`, or `library-<stamp>-<n>.sqlite3` when that name is taken
/// (several opens within one second).
fn unique_name(dir: &Path, stamp: &str) -> PathBuf {
    let plain = dir.join(format!("{PREFIX}{stamp}{SUFFIX}"));
    if !plain.exists() {
        return plain;
    }
    (1u32..)
        .map(|n| dir.join(format!("{PREFIX}{stamp}-{n}{SUFFIX}")))
        .find(|p| !p.exists())
        .expect("a free backup name exists")
}

/// Sort key of a backup file name: `(stamp, counter)`, or `None` for unrelated files.
fn backup_key(name: &str) -> Option<(String, u32)> {
    let rest = name.strip_prefix(PREFIX)?.strip_suffix(SUFFIX)?;
    let (stamp, counter) = match rest.get(15..) {
        Some("") => (rest, 0),
        Some(tail) => (&rest[..15], tail.strip_prefix('-')?.parse().ok()?),
        None => return None,
    };
    let valid = stamp.len() == 15
        && stamp.as_bytes()[8] == b'-'
        && stamp
            .bytes()
            .enumerate()
            .all(|(i, b)| i == 8 || b.is_ascii_digit());
    valid.then(|| (stamp.to_owned(), counter))
}

/// Best effort: keeps `newest` (the backup just taken, whatever its timestamp says) plus
/// the newest `keep - 1` other backups, deletes the rest, and removes stale `.partial`
/// files. Unrelated files are left alone and per-file errors are ignored: the new backup
/// is already safely in place.
fn prune(dir: &Path, keep: usize, newest: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let now = SystemTime::now();
    let mut found: Vec<((String, u32), PathBuf)> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if is_stale_partial(&name, &entry, now) {
            let _ = std::fs::remove_file(&path);
        } else if path != newest
            && let Some(key) = backup_key(&name)
        {
            found.push((key, path));
        }
    }
    found.sort();
    let excess = found.len().saturating_sub(keep.saturating_sub(1));
    for (_, path) in found.into_iter().take(excess) {
        let _ = std::fs::remove_file(path);
    }
}

fn is_stale_partial(name: &str, entry: &std::fs::DirEntry, now: SystemTime) -> bool {
    name.starts_with(PREFIX)
        && name.ends_with(PARTIAL_SUFFIX)
        && entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|age| age >= STALE_PARTIAL)
}

/// `yyyymmdd-hhmmss` in UTC.
fn utc_stamp(now: SystemTime) -> String {
    let secs = now.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let days = i64::try_from(secs / 86_400).unwrap_or(0);
    let rem = secs % 86_400;
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}{m:02}{d:02}-{:02}{:02}{:02}",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// Gregorian date of a day count since 1970-01-01 (Howard Hinnant's algorithm).
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn stamps_are_utc() {
        assert_eq!(utc_stamp(UNIX_EPOCH), "19700101-000000");
        // 2026-10-31T13:45:07Z
        let t = UNIX_EPOCH + Duration::from_secs(1_793_454_307);
        assert_eq!(utc_stamp(t), "20261031-134507");
        // 2024-02-29T23:59:59Z (leap day)
        let t = UNIX_EPOCH + Duration::from_secs(1_709_251_199);
        assert_eq!(utc_stamp(t), "20240229-235959");
    }

    #[test]
    fn keys_order_counters_after_the_plain_name() {
        let plain = backup_key("library-20261031-134507.sqlite3").unwrap();
        let one = backup_key("library-20261031-134507-1.sqlite3").unwrap();
        let ten = backup_key("library-20261031-134507-10.sqlite3").unwrap();
        let next = backup_key("library-20261031-134508.sqlite3").unwrap();
        assert!(plain < one && one < ten && ten < next);
        for junk in [
            "library-20261031-134507.sqlite3.partial",
            "library-2026103-134507.sqlite3",
            "library-20261031x134507.sqlite3",
            "library-20261031-134507-.sqlite3",
            "notes.txt",
        ] {
            assert_eq!(backup_key(junk), None, "{junk}");
        }
    }

    fn names_in(dir: &Path) -> Vec<String> {
        let mut left: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        left.sort();
        left
    }

    fn age(path: &Path, by: Duration) {
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(SystemTime::now() - by)
            .unwrap();
    }

    #[test]
    fn prune_keeps_newest_and_ignores_other_files() {
        let dir = tempfile::tempdir().unwrap();
        let names = [
            "library-20260101-000000.sqlite3",
            "library-20260101-000000-1.sqlite3",
            "library-20260101-000000-2.sqlite3",
            "library-20260102-000000.sqlite3",
            "keep-me.txt",
        ];
        for n in names {
            std::fs::write(dir.path().join(n), b"").unwrap();
        }
        let newest = dir.path().join("library-20260102-000000.sqlite3");
        prune(dir.path(), 2, &newest);
        assert_eq!(
            names_in(dir.path()),
            [
                "keep-me.txt",
                "library-20260101-000000-2.sqlite3",
                "library-20260102-000000.sqlite3"
            ]
        );
    }

    #[test]
    fn prune_never_deletes_the_backup_just_taken() {
        // The clock went backwards: the new backup has the oldest timestamp of all.
        let dir = tempfile::tempdir().unwrap();
        for day in 10..=21 {
            std::fs::write(
                dir.path()
                    .join(format!("library-209901{day}-000000.sqlite3")),
                b"",
            )
            .unwrap();
        }
        let newest = dir.path().join("library-20200101-000000.sqlite3");
        std::fs::write(&newest, b"").unwrap();
        prune(dir.path(), 10, &newest);
        let left = names_in(dir.path());
        assert_eq!(left.len(), 10, "{left:?}");
        assert!(newest.exists());
        // The other nine kept are the newest of the rest.
        assert_eq!(left[1], "library-20990113-000000.sqlite3");
    }

    #[test]
    fn prune_removes_stale_partials_only() {
        let dir = tempfile::tempdir().unwrap();
        let stale = dir.path().join("library-20260101-000000.sqlite3.partial");
        let fresh = dir.path().join("library-20260102-000000.sqlite3.partial");
        let other = dir.path().join("other.partial");
        for p in [&stale, &fresh, &other] {
            std::fs::write(p, b"").unwrap();
        }
        age(&stale, Duration::from_secs(3600));
        age(&other, Duration::from_secs(3600));
        prune(dir.path(), 10, &dir.path().join("none"));
        assert!(!stale.exists());
        assert!(fresh.exists());
        assert!(other.exists());
    }

    #[test]
    fn recent_backup_detection() {
        let dir = tempfile::tempdir().unwrap();
        let now = SystemTime::now();
        assert!(!has_recent_backup(dir.path(), now));
        let b = dir.path().join("library-20260101-000000.sqlite3");
        std::fs::write(&b, b"").unwrap();
        assert!(has_recent_backup(dir.path(), now + Duration::from_secs(1)));
        age(&b, MIN_INTERVAL + Duration::from_secs(1));
        assert!(!has_recent_backup(dir.path(), SystemTime::now()));
        // A backup from the future (clock skew) is not "recent".
        age(&b, Duration::ZERO);
        assert!(!has_recent_backup(
            dir.path(),
            now - Duration::from_secs(3600)
        ));
        // Unrelated files do not count.
        let junk = dir.path().join("notes.txt");
        std::fs::write(&junk, b"").unwrap();
        age(&b, MIN_INTERVAL * 2);
        assert!(!has_recent_backup(dir.path(), SystemTime::now()));
    }
}
