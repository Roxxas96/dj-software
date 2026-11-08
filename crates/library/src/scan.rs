//! Scanning library folders: finding audio files, reading their tags and fingerprints,
//! and keeping track ids (and so cues) attached to files that moved or were retagged.
//!
//! Files are only read. Tracks are never deleted: a file that disappeared is flagged
//! `missing`, and relinked when its audio turns up again under another path.

use std::collections::HashSet;
use std::fs;
use std::path::{MAIN_SEPARATOR, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::UNIX_EPOCH;

use rusqlite::{Connection, OptionalExtension};
use walkdir::WalkDir;

use crate::tags::{self, TrackTags};
use crate::{Library, LibraryError, WriteOp, Writer, fingerprint};

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
    /// Of `updated`, the files whose audio itself changed (different fingerprint). Their
    /// stale analysis was deleted (a locked grid is kept); cues are kept.
    pub audio_changed: usize,
    /// Tracks whose file was found under a new path (same id, cues kept).
    pub relinked: usize,
    /// Tracks under the root newly flagged missing.
    pub missing: usize,
    /// Files with a supported extension that could not be scanned (unreadable or
    /// undecodable), and walk errors (unreadable directories, symlink loops), with the
    /// reason.
    pub skipped: Vec<(PathBuf, String)>,
    /// Files without a supported audio extension (covers, playlists, notes...), counted
    /// but not listed.
    pub ignored: usize,
}

#[derive(Debug, Clone, Copy)]
enum Change {
    Added,
    Updated,
    AudioChanged,
    Relinked,
}

/// Scans `root` (following symlinks, with loop detection) and brings the library in
/// line with it, on the calling thread, writing through the library's writer:
///
/// - a known path with unchanged size and mtime is skipped;
/// - a known path whose size or mtime changed gets its tags and fingerprint re-read
///   (same id) and its analysis marked out of date (analyzer version 0), since the
///   fingerprint covers only the first seconds; if its audio changed, its now stale
///   analysis is deleted (a locked grid is kept);
/// - a new path whose fingerprint matches a track whose file no longer exists is relinked
///   to it (path updated, `missing` cleared, same id), provided the durations agree
///   within max(1 s, 1%) and the match is unambiguous (see `relink_candidate`). Two
///   existing files with the same audio are never merged, and a track whose library
///   folder is itself gone (say, on an unplugged drive) is never relinked. A track under
///   no registered folder is only relinked when it was under `root`;
/// - any other new file is inserted.
///
/// Paths are stored canonical, so a file reached twice through symlinks is one track.
/// Afterwards, tracks under `root` whose file no longer exists are flagged missing.
/// Problems with audio files or directories are reported in [`ScanReport::skipped`]
/// and do not stop the scan; other files are only counted in [`ScanReport::ignored`].
/// Hidden entries (names starting with `.`, such as macOS `._*` resource forks) are
/// skipped silently.
///
/// # Errors
///
/// Fails if `root` cannot be resolved, or if the library cannot be read or written.
pub fn scan_folder(
    lib: &Library,
    root: &Path,
    progress: &mut dyn FnMut(ScanProgress),
) -> Result<ScanReport, LibraryError> {
    let reader = lib.reader()?;
    scan_folder_with(
        &lib.writer(),
        reader.connection(),
        root,
        None,
        None,
        None,
        progress,
    )
}

/// Adds the audio files directly in `folder` (not those in its subfolders) as tracks, the
/// way [`scan_folder`] does, without registering the folder: it is not watched, and no
/// track is flagged missing. Meant for the files of a folder being browsed.
///
/// # Errors
///
/// Fails if `folder` cannot be resolved, or if the library cannot be read or written.
pub fn add_folder_files(
    writer: &Writer,
    conn: &Connection,
    folder: &Path,
) -> Result<ScanReport, LibraryError> {
    scan_folder_with(writer, conn, folder, None, None, Some(1), &mut |_| {})
}

/// [`scan_folder`] on a writer handle and a read connection, for threads that do not
/// hold the [`Library`] itself. `relink_root`, when given, is the folder that moved files
/// may come from (a watched root holding the scanned subfolder); missing tracks are still
/// only flagged under `root`. When `stop` becomes true the scan ends early (changes
/// already found are written, nothing is flagged missing). With `max_depth` set, only
/// that many levels below `root` are walked (1: its own files), and nothing is flagged
/// missing, since the files of the levels left out were not looked at.
pub(crate) fn scan_folder_with(
    writer: &Writer,
    conn: &Connection,
    root: &Path,
    relink_root: Option<&Path>,
    stop: Option<&AtomicBool>,
    max_depth: Option<usize>,
    progress: &mut dyn FnMut(ScanProgress),
) -> Result<ScanReport, LibraryError> {
    let root = normalize_path(root)?;
    let mut scan = Scan::new(writer, conn, dir_prefix(&root)?)?;
    if let Some(relink_root) = relink_root {
        scan.relink_prefix = dir_prefix(relink_root)?;
    }
    let mut scanned = 0;
    let walk = WalkDir::new(&root)
        .max_depth(max_depth.unwrap_or(usize::MAX))
        .follow_links(true)
        .sort_by_file_name()
        .into_iter()
        .filter_entry(|e| e.depth() == 0 || !is_hidden(e.file_name()));
    for entry in walk {
        if stop.is_some_and(|s| s.load(Ordering::Relaxed)) {
            scan.flush()?;
            return Ok(scan.report);
        }
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
            scan.report.ignored += 1;
            continue;
        }
        let canonical = match normalize_path(path) {
            Ok(canonical) => canonical,
            Err(err) => {
                scan.report.skipped.push((path.to_owned(), err.to_string()));
                continue;
            }
        };
        let Some(text) = canonical.to_str() else {
            scan.report
                .skipped
                .push((path.to_owned(), "path is not valid UTF-8".into()));
            continue;
        };
        // The same file reached again through a symlink.
        if !scan.seen.insert(text.to_owned()) {
            continue;
        }
        scanned += 1;
        progress(ScanProgress {
            scanned,
            path: canonical.clone(),
        });
        let result = entry
            .metadata()
            .map_err(|e| FileError::Skip(e.to_string()))
            .and_then(|meta| scan.file(&canonical, text, &meta));
        match result {
            Ok(()) => {}
            Err(FileError::Skip(reason)) => scan.report.skipped.push((canonical, reason)),
            Err(FileError::Library(err)) => return Err(err),
        }
        if scan.pending.len() >= BATCH_SIZE {
            scan.flush()?;
        }
    }
    scan.flush()?;
    if max_depth.is_none() {
        scan.flag_missing()?;
    }
    Ok(scan.report)
}

/// Brings the library in line with one path under `root`, as [`scan_folder`] does for a
/// whole folder, using the same per-file logic. `path` need not be canonical, nor exist:
///
/// - an audio file is added, updated or relinked (a renamed file keeps its track id);
/// - a path that is gone flags the track at it, and every track under it if it was a
///   folder, as missing (nothing is deleted);
/// - a folder is scanned whole when `dirs` is true, else left alone;
/// - hidden entries and other files are ignored.
pub(crate) fn scan_path(
    writer: &Writer,
    conn: &Connection,
    root: &Path,
    path: &Path,
    dirs: bool,
    stop: Option<&AtomicBool>,
) -> Result<ScanReport, LibraryError> {
    let root = normalize_path(root)?;
    let path = resolve_path(path)?;
    let hidden = path
        .strip_prefix(&root)
        .is_ok_and(|rel| rel.components().any(|c| is_hidden(c.as_os_str())));
    if hidden {
        return Ok(ScanReport::default());
    }
    match fs::metadata(&path) {
        Ok(meta) if meta.is_dir() => {
            if dirs {
                scan_folder_with(writer, conn, &path, Some(&root), stop, None, &mut |_| {})
            } else {
                Ok(ScanReport::default())
            }
        }
        Ok(meta) => {
            let mut scan = Scan::new(writer, conn, dir_prefix(&root)?)?;
            if !is_supported(&path) {
                scan.report.ignored += 1;
                return Ok(scan.report);
            }
            let Some(text) = path.to_str() else {
                scan.report
                    .skipped
                    .push((path.clone(), "path is not valid UTF-8".into()));
                return Ok(scan.report);
            };
            match scan.file(&path, text, &meta) {
                Ok(()) => {}
                Err(FileError::Skip(reason)) => scan.report.skipped.push((path.clone(), reason)),
                Err(FileError::Library(err)) => return Err(err),
            }
            scan.flush()?;
            Ok(scan.report)
        }
        Err(_) if is_gone(&path) => {
            let mut scan = Scan::new(writer, conn, dir_prefix(&path)?)?;
            if let Some(text) = path.to_str() {
                scan.flag_exact(text)?;
                scan.flag_missing()?;
            }
            Ok(scan.report)
        }
        Err(err) => Ok(ScanReport {
            skipped: vec![(path, err.to_string())],
            ..ScanReport::default()
        }),
    }
}

/// [`normalize_path`] for a path that may be gone: the nearest existing ancestor is
/// normalized and the rest appended.
pub(crate) fn resolve_path(path: &Path) -> std::io::Result<PathBuf> {
    match normalize_path(path) {
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            match (path.parent(), path.file_name()) {
                (Some(parent), Some(name)) => Ok(resolve_path(parent)?.join(name)),
                _ => Err(err),
            }
        }
        other => other,
    }
}

/// Adds `path` (resolved to its canonical form) to the library folders. Only the
/// `folders` table changes; scan it with [`scan_folder`].
///
/// # Errors
///
/// Fails if `path` cannot be resolved, is not valid UTF-8, or the write fails.
pub fn add_folder(lib: &Library, path: &Path) -> Result<(), LibraryError> {
    lib.writer()
        .submit_wait(WriteOp::AddFolder(normalized_text(path)?))?;
    Ok(())
}

impl crate::Reader {
    /// The registered library folders, sorted by path.
    ///
    /// # Errors
    ///
    /// Any database error.
    pub fn folders(&self) -> Result<Vec<PathBuf>, LibraryError> {
        let mut stmt = self
            .connection()
            .prepare_cached("SELECT path FROM folders ORDER BY path")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        rows.map(|r| r.map(PathBuf::from).map_err(LibraryError::from))
            .collect()
    }
}

/// Removes `path` (as given, and in canonical form if it still resolves) from the library
/// folders. Its tracks, and their cues, are kept.
///
/// # Errors
///
/// Fails if `path` is not valid UTF-8 or the write fails.
pub fn remove_folder(lib: &Library, path: &Path) -> Result<(), LibraryError> {
    let mut ops = vec![WriteOp::RemoveFolder(utf8(path)?.to_owned())];
    if let Ok(canonical) = normalized_text(path) {
        ops.push(WriteOp::RemoveFolder(canonical));
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

/// The stored state of a track found by path.
struct Known {
    id: i64,
    size: Option<i64>,
    mtime: Option<i64>,
    missing: bool,
    fingerprint: Vec<u8>,
}

struct Scan<'a> {
    writer: &'a Writer,
    conn: &'a Connection,
    /// The canonical root followed by a separator.
    root_prefix: String,
    /// Where tracks not under any registered folder may be relinked from: the root, or
    /// the watched folder around it.
    relink_prefix: String,
    /// Registered library folders, with whether each exists once checked.
    folders: Vec<(String, Option<bool>)>,
    /// Canonical paths of every audio file reached by this scan.
    seen: HashSet<String>,
    /// Changes not yet written, with the file they are for.
    pending: Vec<(PathBuf, WriteOp, Change)>,
    /// Tracks updated or relinked by this scan, which no other file may relink.
    touched: HashSet<i64>,
    report: ScanReport,
}

impl<'a> Scan<'a> {
    fn new(
        writer: &'a Writer,
        conn: &'a Connection,
        root_prefix: String,
    ) -> Result<Self, rusqlite::Error> {
        let folders = {
            let mut stmt = conn.prepare("SELECT path FROM folders")?;
            let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
            rows.map(|r| r.map(|f| (f, None)))
                .collect::<Result<_, _>>()?
        };
        Ok(Self {
            writer,
            conn,
            relink_prefix: root_prefix.clone(),
            root_prefix,
            folders,
            pending: Vec::new(),
            touched: HashSet::new(),
            seen: HashSet::new(),
            report: ScanReport::default(),
        })
    }

    /// Flags the track stored at exactly `path`, if its file is gone.
    fn flag_exact(&mut self, path: &str) -> Result<(), LibraryError> {
        let id: Option<i64> = self
            .conn
            .prepare_cached("SELECT id FROM tracks WHERE path = ?1 AND missing = 0")?
            .query_row([path], |r| r.get(0))
            .optional()?;
        if let Some(id) = id {
            self.writer.submit_wait(WriteOp::MarkMissing(vec![id]))?;
            self.report.missing += 1;
        }
        Ok(())
    }
}

impl Scan<'_> {
    /// Handles one audio file at canonical `path` (`text` is the same path).
    fn file(&mut self, path: &Path, text: &str, meta: &fs::Metadata) -> Result<(), FileError> {
        let size = i64::try_from(meta.len()).unwrap_or(i64::MAX);
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |d| i64::try_from(d.as_nanos()).unwrap_or(i64::MAX));

        let known: Option<Known> = self
            .conn
            .prepare_cached(
                "SELECT id, size, mtime, missing, fingerprint FROM tracks WHERE path = ?1",
            )?
            .query_row([text], |r| {
                Ok(Known {
                    id: r.get(0)?,
                    size: r.get(1)?,
                    mtime: r.get(2)?,
                    missing: r.get(3)?,
                    fingerprint: r.get(4)?,
                })
            })
            .optional()?;
        if let Some(k) = &known
            && !k.missing
            && k.size == Some(size)
            && k.mtime == Some(mtime)
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
        let (op, change) = if let Some(Known {
            id,
            fingerprint: old_print,
            size: old_size,
            mtime: old_mtime,
            ..
        }) = known
        {
            self.touched.insert(id);
            let update = WriteOp::UpdateTrack { id, file };
            if old_print == print && old_size == Some(size) && old_mtime == Some(mtime) {
                // A missing file back unchanged.
                (update, Change::Updated)
            } else if old_print == print {
                // The file changed but its first seconds did not: a retag, or a file
                // scanned while still being copied. The analysis may cover only part of
                // the audio, so it is redone (a locked grid and cues are kept).
                (
                    WriteOp::Batch(vec![update, WriteOp::ResetAnalysis(id)]),
                    Change::Updated,
                )
            } else {
                // Same file, different audio: the analysis no longer matches it.
                (
                    WriteOp::Batch(vec![update, WriteOp::ClearAnalysis(id)]),
                    Change::AudioChanged,
                )
            }
        } else if let Some(id) = self.relink_candidate(&file)? {
            self.touched.insert(id);
            (WriteOp::UpdateTrack { id, file }, Change::Relinked)
        } else {
            (WriteOp::InsertTrack(file), Change::Added)
        };
        self.pending.push((path.to_owned(), op, change));
        Ok(())
    }

    /// The track `file` (a new path) is a moved copy of, if exactly one fits: same
    /// fingerprint, file no longer there, library folder present, not already claimed by
    /// this scan, and the same duration within max(1 s, 1%) when both durations are known
    /// (the fingerprint covers only the first seconds). Among several, the one with the
    /// same file name wins; when that still leaves more than one, or none of several, the
    /// match is ambiguous and `None` is returned: the file becomes a new track and the
    /// old ones stay missing with their cues, rather than risk swapping them.
    fn relink_candidate(&mut self, file: &TrackFile) -> Result<Option<i64>, rusqlite::Error> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, path, duration_frames, sample_rate FROM tracks \
             WHERE fingerprint = ?1 ORDER BY id",
        )?;
        let mut rows = stmt.query([&file.fingerprint[..]])?;
        let mut candidates = Vec::new();
        while let Some(row) = rows.next()? {
            let id: i64 = row.get(0)?;
            let path: String = row.get(1)?;
            let duration: Option<i64> = row.get(2)?;
            let rate: Option<i64> = row.get(3)?;
            if !self.touched.contains(&id) && is_gone(Path::new(&path)) {
                candidates.push((id, path, duration, rate));
            }
        }
        drop(rows);
        drop(stmt);
        let new_duration = file
            .tags
            .duration_frames
            .and_then(|f| i64::try_from(f).ok());
        let new_rate = file.tags.sample_rate.map(i64::from);
        let mut fits = Vec::new();
        for (id, path, duration, rate) in candidates {
            if same_duration(new_duration, duration, new_rate.or(rate))
                && self.folder_is_present(&path)
            {
                fits.push((id, path));
            }
        }
        if let [(id, _)] = fits.as_slice() {
            return Ok(Some(*id));
        }
        let name = Path::new(&file.path).file_name();
        let mut same_name = fits
            .iter()
            .filter(|(_, path)| Path::new(path).file_name() == name);
        match (same_name.next(), same_name.next()) {
            (Some((id, _)), None) => Ok(Some(*id)),
            _ => Ok(None),
        }
    }

    /// Whether the library folder holding `path` exists: the longest registered folder
    /// containing it, else the scanned root if `path` is under it. A path under neither
    /// has no known folder and counts as absent.
    fn folder_is_present(&mut self, path: &str) -> bool {
        let best = self
            .folders
            .iter_mut()
            .filter(|(folder, _)| is_under(path, folder))
            .max_by_key(|(folder, _)| folder.len());
        match best {
            Some((folder, present)) => {
                *present.get_or_insert_with(|| matches!(Path::new(folder).try_exists(), Ok(true)))
            }
            None => path.starts_with(&self.relink_prefix),
        }
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
        let writer = self.writer;
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
            Change::AudioChanged => {
                self.report.updated += 1;
                self.report.audio_changed += 1;
            }
            Change::Relinked => self.report.relinked += 1,
        }
    }

    /// Flags tracks under the root (not merely sharing its prefix) whose file is gone.
    /// Files seen during the walk are present; only the others are checked on disk.
    fn flag_missing(&mut self) -> Result<(), LibraryError> {
        let prefix = self.root_prefix.clone();
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
                if !self.seen.contains(&path) && is_gone(Path::new(&path)) {
                    gone.push(row.get::<_, i64>(0)?);
                }
            }
        }
        let writer = self.writer;
        for ids in gone.chunks(MISSING_BATCH_SIZE) {
            writer.submit_wait(WriteOp::MarkMissing(ids.to_vec()))?;
            self.report.missing += ids.len();
        }
        Ok(())
    }
}

/// Whether two durations in frames (at `rate` Hz, 44.1 kHz if unknown) are within
/// max(1 s, 1%) of each other; true when either is unknown.
fn same_duration(a: Option<i64>, b: Option<i64>, rate: Option<i64>) -> bool {
    let (Some(a), Some(b)) = (a, b) else {
        return true;
    };
    let second = rate.filter(|&r| r > 0).unwrap_or(44_100);
    let tolerance = second.max(a.max(b) / 100);
    a.abs_diff(b) <= tolerance.unsigned_abs()
}

/// True only when `path` is known not to exist. When that cannot be checked (say, a
/// directory without permission) the file is assumed to still be there.
fn is_gone(path: &Path) -> bool {
    matches!(path.try_exists(), Ok(false))
}

/// The one way paths are canonicalized for the library: absolute, symlinks resolved,
/// and on Windows without the `\\?\` verbatim prefix where a plain path works (via
/// `dunce`), so stored paths match paths that arrive by other routes.
pub(crate) fn normalize_path(path: &Path) -> std::io::Result<PathBuf> {
    dunce::canonicalize(path)
}

/// [`normalize_path`] as the UTF-8 text stored in the database (library folders).
fn normalized_text(path: &Path) -> Result<String, LibraryError> {
    let path = normalize_path(path)?;
    Ok(utf8(&path)?.to_owned())
}

/// `dir` as a string ending with a separator, for prefix matching.
fn dir_prefix(dir: &Path) -> Result<String, LibraryError> {
    let mut prefix = utf8(dir)?.to_owned();
    if !prefix.ends_with(MAIN_SEPARATOR) {
        prefix.push(MAIN_SEPARATOR);
    }
    Ok(prefix)
}

/// Whether `path` lies inside directory `dir` (both canonical strings).
fn is_under(path: &str, dir: &str) -> bool {
    path.strip_prefix(dir)
        .is_some_and(|rest| dir.ends_with(MAIN_SEPARATOR) || rest.starts_with(MAIN_SEPARATOR))
}

/// A `.`-prefixed name, skipped by scans and the browser.
pub(crate) fn is_hidden(name: &std::ffi::OsStr) -> bool {
    name.as_encoded_bytes().first() == Some(&b'.')
}

/// Whether `path` has one of the [`EXTENSIONS`].
pub(crate) fn is_supported(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| EXTENSIONS.iter().any(|x| x.eq_ignore_ascii_case(e)))
}

fn utf8(path: &Path) -> Result<&str, LibraryError> {
    path.to_str()
        .ok_or_else(|| LibraryError::NonUtf8Path(path.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_path_is_absolute_without_verbatim_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let normalized = normalize_path(dir.path()).unwrap();
        assert!(normalized.is_absolute(), "{}", normalized.display());
        #[cfg(windows)]
        assert!(
            !normalized.to_string_lossy().starts_with(r"\\?\"),
            "{}",
            normalized.display()
        );
        #[cfg(unix)]
        assert_eq!(normalized, fs::canonicalize(dir.path()).unwrap());
    }

    fn fixture() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../decode/tests/fixtures/sine440.wav")
    }

    /// Renames a tracked file and applies the remove and the create as `scan_path`
    /// calls in either order: same track id, cue intact, not missing.
    fn rename_in_order(remove_first: bool, registered: bool) {
        let dir = tempfile::tempdir().unwrap();
        let lib = Library::open(&dir.path().join("db").join("library.sqlite3")).unwrap();
        let root = dir.path().join("music");
        fs::create_dir_all(&root).unwrap();
        let root = normalize_path(&root).unwrap();
        if registered {
            add_folder(&lib, &root).unwrap();
        }
        let old = root.join("old.wav");
        fs::copy(fixture(), &old).unwrap();
        scan_folder(&lib, &root, &mut |_| {}).unwrap();
        let reader = lib.reader().unwrap();
        let conn = reader.connection();
        let id: i64 = conn
            .query_row("SELECT id FROM tracks", [], |r| r.get(0))
            .unwrap();
        lib.writer()
            .submit_wait(WriteOp::Raw(vec![crate::RawStatement {
                sql: "INSERT INTO cues(track_id, cue_frames) VALUES (?1, 1234.0)".into(),
                params: vec![rusqlite::types::Value::Integer(id)],
            }]))
            .unwrap();

        let new = root.join("sub").join("new.wav");
        fs::create_dir_all(new.parent().unwrap()).unwrap();
        fs::rename(&old, &new).unwrap();
        let writer = lib.writer();
        let remove = || scan_path(&writer, conn, &root, &old, false, None).unwrap();
        let create = || scan_path(&writer, conn, &root, &new, false, None).unwrap();
        if remove_first {
            remove();
            create();
        } else {
            create();
            remove();
        }
        let rows: Vec<(i64, String, bool)> = conn
            .prepare("SELECT id, path, missing FROM tracks")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(rows, [(id, new.to_str().unwrap().to_owned(), false)]);
        let cues: i64 = conn
            .query_row("SELECT COUNT(*) FROM cues WHERE track_id = ?1", [id], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(cues, 1);
    }

    #[test]
    fn rename_remove_then_create_relinks() {
        rename_in_order(true, false);
    }

    #[test]
    fn rename_create_then_remove_relinks() {
        rename_in_order(false, false);
    }

    #[test]
    fn rename_remove_then_create_relinks_with_registered_folder() {
        rename_in_order(true, true);
    }

    #[test]
    fn rename_create_then_remove_relinks_with_registered_folder() {
        rename_in_order(false, true);
    }
}
