//! Startup backups: before the library is opened for writing, the existing database is
//! copied with SQLite's online backup API to
//! `<dir>/backups/library-<UTC yyyymmdd-hhmmss>.sqlite3`, and only the newest
//! [`KEEP`] backups are kept.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, MAIN_DB, OpenFlags};

use crate::LibraryError;

/// How many backups survive pruning.
pub(crate) const KEEP: usize = 10;

const PREFIX: &str = "library-";
const SUFFIX: &str = ".sqlite3";

/// Backs up `db` (which must exist) into `<db dir>/backups/`, then prunes old backups.
/// Returns the new backup's path.
pub(crate) fn backup_and_prune(db: &Path) -> Result<PathBuf, LibraryError> {
    let dir = db
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("backups");
    std::fs::create_dir_all(&dir)?;
    let stamp = utc_stamp(SystemTime::now());
    let target = unique_name(&dir, &stamp);

    // Read-write without create: a read-only open of a WAL database can fail when the
    // shared-memory file is absent, and nothing here writes to the source.
    let src = Connection::open_with_flags(
        db,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(LibraryError::Backup)?;
    // Copy to a temporary name first so an interrupted backup never looks like a real one.
    let partial = target.with_extension("sqlite3.partial");
    let _ = std::fs::remove_file(&partial);
    src.backup(MAIN_DB, &partial, None::<fn(rusqlite::backup::Progress)>)
        .map_err(LibraryError::Backup)?;
    drop(src);
    std::fs::rename(&partial, &target)?;

    prune(&dir, KEEP)?;
    Ok(target)
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

/// Deletes all but the newest `keep` backups in `dir`. Files that are not backups are
/// left alone.
fn prune(dir: &Path, keep: usize) -> Result<(), LibraryError> {
    let mut found: Vec<((String, u32), PathBuf)> = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        if let Some(key) = entry.file_name().to_str().and_then(backup_key) {
            found.push((key, entry.path()));
        }
    }
    found.sort();
    let excess = found.len().saturating_sub(keep);
    for (_, path) in found.into_iter().take(excess) {
        std::fs::remove_file(path)?;
    }
    Ok(())
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
        prune(dir.path(), 2).unwrap();
        let mut left: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        left.sort();
        assert_eq!(
            left,
            [
                "keep-me.txt",
                "library-20260101-000000-2.sqlite3",
                "library-20260102-000000.sqlite3"
            ]
        );
    }
}
