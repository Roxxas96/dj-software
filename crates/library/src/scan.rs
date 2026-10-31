//! Scanning library folders: finding audio files, reading their tags and fingerprints,
//! and keeping track ids (and so cues) attached to files that moved or were retagged.
//!
//! Files are only read. Tracks are never deleted: a file that disappeared is flagged
//! `missing`, and relinked when its audio turns up again under another path.

use std::collections::HashSet;
use std::fs;
use std::path::{MAIN_SEPARATOR, Path, PathBuf};
use std::time::UNIX_EPOCH;

use rusqlite::{Connection, OptionalExtension};
use walkdir::WalkDir;

use crate::tags::{self, TrackTags};
use crate::{Library, LibraryError, WriteOp, fingerprint};

/// File extensions the scanner picks up, compared case-insensitively.
pub const EXTENSIONS: &[&str] = &["mp3", "wav", "aif", "aiff", "flac", "m4a", "aac", "ogg"];

/// How many file changes go into one write transaction. Each file's change is still
/// atomic: a batch is committed whole, and if it fails its changes are retried one by one.
const BATCH_SIZE: usize = 200;

/// How many tracks one [`WriteOp::MarkMissing`] flags.
const MISSING_BATCH_SIZE: usize = 500;

/// Everything stored about a file when it is added, updated or relinked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackFile {
    pub path: String,
    pub fingerprint: [u8; 32],
    /// Size in bytes.
    pub size: i64,
    /// Modification time in nanoseconds since the Unix epoch (0 if unknown).
    pub mtime: i64,
    pub tags: TrackTags,
}

/// Reported before each audio file is processed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanProgress {
    /// Audio files reached so far, this one included.
    pub scanned: usize,
    /// The file about to be processed.
    pub path: PathBuf,
}

/// What a scan changed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScanReport {
    /// New tracks.
    pub added: usize,
    /// Known files whose size or mtime changed (tags and fingerprint re-read, same id).
    pub updated: usize,
    /// Tracks whose file was found under a new path (same id, cues kept).
    pub relinked: usize,
    /// Tracks under the root newly flagged missing.
    pub missing: usize,
    /// Paths that could not be scanned, with the reason: unreadable or undecodable
    /// files, unsupported file types, unreadable directories, symlink loops.
    pub skipped: Vec<(PathBuf, String)>,
}

#[derive(Debug, Clone, Copy)]
enum Change {
    Added,
    Updated,
    Relinked,
}

/// Scans `root` (following symlinks, with loop detection) and brings the library in
/// line with it, on the calling thread, writing through the library's writer:
///
/// - a known path with unchanged size and mtime is skipped;
/// - a known path whose size or mtime changed gets its tags and fingerprint re-read
///   (same id);
/// - a new path whose fingerprint matches a track whose file no longer exists is relinked
///   to it (path updated, `missing` cleared, same id). Two existing files with the same
///   audio are never merged;
/// - any other new file is inserted.
///
/// Afterwards, tracks under `root` whose file no longer exists are flagged missing.
/// Problems with single files or directories are reported in [`ScanReport::skipped`]
/// and do not stop the scan. Hidden entries (names starting with `.`, such as macOS
/// `._*` resource forks) are ignored.
///
/// # Errors
///
/// Fails if `root` cannot be resolved, or if the library cannot be read or written.
pub fn scan_folder(
    lib: &Library,
    root: &Path,
    progress: &mut dyn FnMut(ScanProgress),
) -> Result<ScanReport, LibraryError> {
    let root = fs::canonicalize(root)?;
    let reader = lib.reader()?;
    let mut scan = Scan {
        lib,
        conn: reader.connection(),
        pending: Vec::new(),
        touched: HashSet::new(),
        report: ScanReport::default(),
    };
    let mut scanned = 0;
    let walk = WalkDir::new(&root)
        .follow_links(true)
        .sort_by_file_name()
        .into_iter()
        .filter_entry(|e| e.depth() == 0 || !is_hidden(e.file_name()));
    for entry in walk {
        let entry = match entry {
            Ok(entry) => entry,
            Err(err) => {
                let path = err.path().unwrap_or(&root).to_owned();
                scan.report.skipped.push((path, err.to_string()));
                continue;
            }
        };
        if entry.file_type().is_dir() {
            continue;
        }
        let path = entry.path();
        if !is_supported(path) {
            scan.report
                .skipped
                .push((path.to_owned(), "not a supported audio file".into()));
            continue;
        }
        scanned += 1;
        progress(ScanProgress {
            scanned,
            path: path.to_owned(),
        });
        match scan.file(path) {
            Ok(()) => {}
            Err(FileError::Skip(reason)) => scan.report.skipped.push((path.to_owned(), reason)),
            Err(FileError::Library(err)) => return Err(err),
        }
        if scan.pending.len() >= BATCH_SIZE {
            scan.flush()?;
        }
    }
    scan.flush()?;
    scan.flag_missing(&root)?;
    Ok(scan.report)
}

/// Adds `path` (resolved to its canonical form) to the library folders. Only the
/// `folders` table changes; scan it with [`scan_folder`].
///
/// # Errors
///
/// Fails if `path` cannot be resolved, is not valid UTF-8, or the write fails.
pub fn add_folder(lib: &Library, path: &Path) -> Result<(), LibraryError> {
    let path = fs::canonicalize(path)?;
    lib.writer()
        .submit_wait(WriteOp::AddFolder(utf8(&path)?.to_owned()))?;
    Ok(())
}

/// Removes `path` (as given, and in canonical form if it still resolves) from the library
/// folders. Its tracks, and their cues, are kept.
///
/// # Errors
///
/// Fails if `path` is not valid UTF-8 or the write fails.
pub fn remove_folder(lib: &Library, path: &Path) -> Result<(), LibraryError> {
    let mut ops = vec![WriteOp::RemoveFolder(utf8(path)?.to_owned())];
    if let Ok(canonical) = fs::canonicalize(path) {
        ops.push(WriteOp::RemoveFolder(utf8(&canonical)?.to_owned()));
    }
    lib.writer().submit_wait(WriteOp::Batch(ops))?;
    Ok(())
}

enum FileError {
    /// This file is reported in [`ScanReport::skipped`] and the scan goes on.
    Skip(String),
    /// The library itself failed; the scan stops.
    Library(LibraryError),
}

impl From<rusqlite::Error> for FileError {
    fn from(err: rusqlite::Error) -> Self {
        Self::Library(err.into())
    }
}

struct Scan<'a> {
    lib: &'a Library,
    conn: &'a Connection,
    /// Changes not yet written, with the file they are for.
    pending: Vec<(PathBuf, WriteOp, Change)>,
    /// Tracks updated or relinked by this scan, which no other file may relink.
    touched: HashSet<i64>,
    report: ScanReport,
}

impl Scan<'_> {
    fn file(&mut self, path: &Path) -> Result<(), FileError> {
        let text = path
            .to_str()
            .ok_or_else(|| FileError::Skip("path is not valid UTF-8".into()))?;
        let meta = fs::metadata(path).map_err(|e| FileError::Skip(e.to_string()))?;
        let size = i64::try_from(meta.len()).unwrap_or(i64::MAX);
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |d| i64::try_from(d.as_nanos()).unwrap_or(i64::MAX));

        let known: Option<(i64, Option<i64>, Option<i64>, bool)> = self
            .conn
            .query_row(
                "SELECT id, size, mtime, missing FROM tracks WHERE path = ?1",
                [text],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?;
        if let Some((_, Some(s), Some(m), false)) = known
            && s == size
            && m == mtime
        {
            return Ok(());
        }

        let print = fingerprint::of_file(path).map_err(|e| FileError::Skip(e.to_string()))?;
        let file = Box::new(TrackFile {
            path: text.to_owned(),
            fingerprint: print,
            size,
            mtime,
            tags: tags::read(path),
        });
        let (op, change) = if let Some((id, ..)) = known {
            self.touched.insert(id);
            (WriteOp::UpdateTrack { id, file }, Change::Updated)
        } else if let Some(id) = self.relink_candidate(&print)? {
            self.touched.insert(id);
            (WriteOp::UpdateTrack { id, file }, Change::Relinked)
        } else {
            (WriteOp::InsertTrack(file), Change::Added)
        };
        self.pending.push((path.to_owned(), op, change));
        Ok(())
    }

    /// A track with this fingerprint whose file no longer exists and that this scan has
    /// not already claimed; missing-flagged tracks first, then the oldest.
    fn relink_candidate(&self, print: &[u8; 32]) -> Result<Option<i64>, rusqlite::Error> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, path FROM tracks WHERE fingerprint = ?1 ORDER BY missing DESC, id",
        )?;
        let mut rows = stmt.query([&print[..]])?;
        while let Some(row) = rows.next()? {
            let id: i64 = row.get(0)?;
            let path: String = row.get(1)?;
            if !self.touched.contains(&id) && is_gone(Path::new(&path)) {
                return Ok(Some(id));
            }
        }
        Ok(None)
    }

    /// Writes the pending changes in one transaction. If that fails, retries each change
    /// on its own so one bad file cannot hold back the others; changes that still fail
    /// are reported as skipped.
    fn flush(&mut self) -> Result<(), LibraryError> {
        if self.pending.is_empty() {
            return Ok(());
        }
        let pending = std::mem::take(&mut self.pending);
        let batch = WriteOp::Batch(pending.iter().map(|(_, op, _)| op.clone()).collect());
        let writer = self.lib.writer();
        match writer.submit_wait(batch) {
            Ok(_) => {
                for (_, _, change) in &pending {
                    self.count(*change);
                }
                return Ok(());
            }
            Err(LibraryError::WriterGone) => return Err(LibraryError::WriterGone),
            Err(_) => {}
        }
        for (path, op, change) in pending {
            match writer.submit_wait(op) {
                Ok(_) => self.count(change),
                Err(LibraryError::WriterGone) => return Err(LibraryError::WriterGone),
                Err(err) => self.report.skipped.push((path, err.to_string())),
            }
        }
        Ok(())
    }

    fn count(&mut self, change: Change) {
        match change {
            Change::Added => self.report.added += 1,
            Change::Updated => self.report.updated += 1,
            Change::Relinked => self.report.relinked += 1,
        }
    }

    /// Flags tracks under `root` (not merely sharing its prefix) whose file is gone.
    fn flag_missing(&mut self, root: &Path) -> Result<(), LibraryError> {
        let mut prefix = utf8(root)?.to_owned();
        if !prefix.ends_with(MAIN_SEPARATOR) {
            prefix.push(MAIN_SEPARATOR);
        }
        // Every path starting with `prefix` sorts in [prefix, upper): the separator's
        // successor replaces it. This range uses the `path` index.
        let mut upper = prefix.clone();
        upper.pop();
        upper.push(char::from_u32(u32::from(MAIN_SEPARATOR) + 1).unwrap_or(char::MAX));

        let mut gone = Vec::new();
        {
            let mut stmt = self.conn.prepare(
                "SELECT id, path FROM tracks WHERE missing = 0 AND path >= ?1 AND path < ?2",
            )?;
            let mut rows = stmt.query([&prefix, &upper])?;
            while let Some(row) = rows.next()? {
                let path: String = row.get(1)?;
                if is_gone(Path::new(&path)) {
                    gone.push(row.get::<_, i64>(0)?);
                }
            }
        }
        let writer = self.lib.writer();
        for ids in gone.chunks(MISSING_BATCH_SIZE) {
            writer.submit_wait(WriteOp::MarkMissing(ids.to_vec()))?;
            self.report.missing += ids.len();
        }
        Ok(())
    }
}

/// True only when `path` is known not to exist. When that cannot be checked (say, a
/// directory without permission) the file is assumed to still be there.
fn is_gone(path: &Path) -> bool {
    matches!(path.try_exists(), Ok(false))
}

fn is_hidden(name: &std::ffi::OsStr) -> bool {
    name.as_encoded_bytes().first() == Some(&b'.')
}

fn is_supported(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| EXTENSIONS.iter().any(|x| x.eq_ignore_ascii_case(e)))
}

fn utf8(path: &Path) -> Result<&str, LibraryError> {
    path.to_str()
        .ok_or_else(|| LibraryError::NonUtf8Path(path.to_owned()))
}
