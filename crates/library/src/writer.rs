//! The single writer thread.
//!
//! Every change to the library is a [`WriteOp`] sent over a bounded channel to one thread
//! that owns the only read-write connection. Each op runs in its own transaction: it is
//! committed whole or rolled back whole, including when applying it panics. Ops are
//! applied in the order they were queued. Failures of ops queued without waiting are kept
//! for the app to report and retry.

use std::collections::VecDeque;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, RwLock};
use std::thread::JoinHandle;

use rusqlite::types::Value;
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params_from_iter};

use crate::LibraryError;
use crate::cues;
use crate::grid::{self, Beatgrid, GridEdit};
use crate::scan::TrackFile;

/// Capacity of the writer's queue. [`Writer::submit`] blocks while it is full rather than
/// dropping a change.
pub(crate) const QUEUE_CAPACITY: usize = 1024;

/// One SQL statement with positional parameters, for [`WriteOp::Raw`].
#[derive(Debug, Clone, PartialEq)]
pub struct RawStatement {
    pub sql: String,
    pub params: Vec<Value>,
}

/// A change to the library, applied by the writer thread in one transaction.
///
/// Plain data, no closures, so ops can be logged, queued and replayed.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum WriteOp {
    /// Sets a setting, or deletes it when `value` is `None`.
    SetSetting { key: String, value: Option<String> },
    /// Adds a track for a newly found file.
    InsertTrack(Box<TrackFile>),
    /// Rewrites track `id` from `file` (path, fingerprint, size, mtime and tags) and clears
    /// its `missing` flag, keeping its id and so its cues. Used both when a known file
    /// changed and to relink a moved file.
    UpdateTrack { id: i64, file: Box<TrackFile> },
    /// Deletes a track's analysis, which no longer matches its audio. A locked grid is
    /// kept: only the waveform and key are cleared and the analyzer version set to 0, so
    /// the track is analyzed again around the user's grid. Cues are kept.
    ClearAnalysis(i64),
    /// Marks a track's analysis as out of date (analyzer version 0) so it is analyzed
    /// again, keeping what is stored until then. Used when a file changed without its
    /// fingerprint changing (say, a file scanned while still being copied).
    ResetAnalysis(i64),
    /// Stores a track's analysis (waveform, beatgrid, key and analyzer version together),
    /// replacing any previous one. When the stored grid is locked, its `bpm` and
    /// `first_beat_frames` are kept and the ones given here are ignored. Fails with
    /// [`LibraryError::TrackChanged`] when the track's fingerprint is no longer
    /// `fingerprint` (its audio changed while it was being analyzed) and with
    /// [`LibraryError::NoSuchTrack`] when the track is gone, storing nothing. When the
    /// fingerprint matches but the file's size or mtime is no longer
    /// [`StoreAnalysis::file`] (it changed after its audio was read, say a copy that
    /// completed), the analysis is stored with analyzer version 0, to be redone.
    StoreAnalysis(Box<StoreAnalysis>),
    /// Applies a manual edit to track `track`'s beatgrid, reading the current grid in the
    /// same transaction (see [`crate::GridEdit`] for what each edit does). Creates the
    /// analysis row if the track has none. Fails with [`LibraryError::Grid`] when the edit
    /// is refused and [`LibraryError::NoSuchTrack`] when the track is gone, changing
    /// nothing. Reports the resulting grid as [`WriteOutcome::Grid`]. A [`GridEdit::Tap`]
    /// here only creates the row: tap tempo keeps its taps in the [`crate::Library`], so
    /// use [`crate::Library::edit_grid`] for taps. `sample_rate` is the decoded track's
    /// rate when known; it is preferred over the rate read from the tags.
    EditGrid {
        track: i64,
        edit: GridEdit,
        playhead_frames: Option<f64>,
        sample_rate: Option<u32>,
    },
    /// Stores `frames` (at the track's native sample rate) as track `track`'s cue point,
    /// replacing any previous one. Fails with [`LibraryError::InvalidCue`] when `frames` is
    /// not finite or is negative and with [`LibraryError::NoSuchTrack`] when the track is
    /// gone, changing nothing.
    SetCue { track: i64, frames: f64 },
    /// Stores `frames` as hotcue `slot` (1 to 8) of track `track`, keeping the color and name
    /// of an existing hotcue, and stores `loop_frames` as its loop length (`None` clears it).
    /// Fails with [`LibraryError::InvalidHotcueSlot`] for a slot outside 1-8, with
    /// [`LibraryError::InvalidCue`] when `frames` is not finite or is negative, with
    /// [`LibraryError::InvalidLoop`] when `loop_frames` is not finite and above zero and with
    /// [`LibraryError::NoSuchTrack`] when the track is gone, changing nothing.
    SetHotcue {
        track: i64,
        slot: u8,
        frames: f64,
        loop_frames: Option<f64>,
    },
    /// Replaces hotcue `slot` (1 to 8) of track `track` with `point` whole: position, loop
    /// length, color and name. Fails like [`WriteOp::SetHotcue`].
    PutHotcue {
        track: i64,
        slot: u8,
        point: crate::HotcuePoint,
    },
    /// Sets the name and colour (`0xRRGGBB`) of hotcue `slot` (1 to 8) of track `track`,
    /// keeping its position and loop length; `None` clears either. Fails with
    /// [`LibraryError::NoSuchHotcue`] when the slot is empty, and like
    /// [`WriteOp::SetHotcue`] for a bad slot or an unknown track, changing nothing.
    SetHotcueLabel {
        track: i64,
        slot: u8,
        name: Option<String>,
        color: Option<u32>,
    },
    /// Removes hotcue `slot` (1 to 8) of track `track`; an empty slot is fine. Fails like
    /// [`WriteOp::SetHotcue`] for a bad slot or an unknown track, changing nothing.
    DeleteHotcue { track: i64, slot: u8 },
    /// Records that track `id` was loaded into a deck now (`tracks.last_loaded_at`), which
    /// keeps its stems cached. Fails with [`LibraryError::NoSuchTrack`] for an unknown track.
    TrackLoaded(i64),
    /// Records (or replaces) track `track`'s stems cache entry. `prepared` is kept once set,
    /// on the track too (`tracks.stems_prepared`), so an entry removed as damaged or stale
    /// is prepared again when redone. Used by [`crate::StemCache`].
    PutStems(Box<StemsRow>),
    /// Marks track `id` and its stems cache entry, if any, as prepared ahead
    /// (`library stems`).
    MarkStemsPrepared(i64),
    /// Removes track `id`'s stems cache entry row (the cache removes its files).
    RemoveStems(i64),
    /// Flags these tracks' files as missing. Tracks are never deleted by a scan.
    MarkMissing(Vec<i64>),
    /// Adds a library folder (no-op if already there). Only the `folders` table changes.
    AddFolder(String),
    /// Removes a library folder. Only the `folders` table changes: its tracks stay.
    RemoveFolder(String),
    /// Creates a playlist at the end of the playlist list; reports its id as
    /// [`WriteOutcome::Created`].
    CreatePlaylist { name: String },
    /// Creates a playlist at the end of the playlist list holding `tracks` in that order;
    /// reports its id as [`WriteOutcome::Created`]. Fails with
    /// [`LibraryError::NoSuchTrack`] for an unknown track, creating nothing.
    CreatePlaylistWith { name: String, tracks: Vec<i64> },
    /// Renames a playlist.
    RenamePlaylist { id: i64, name: String },
    /// Deletes a playlist and its membership rows; tracks are never touched.
    DeletePlaylist(i64),
    /// Moves a playlist to `to_position` in the playlist list (clamped to the end).
    MovePlaylist { id: i64, to_position: usize },
    /// Inserts `track` at position `at` of `playlist` (appended when `None` or past the
    /// end). The same track may be added any number of times.
    AddToPlaylist {
        playlist: i64,
        track: i64,
        at: Option<usize>,
    },
    /// Removes the entry at `position` (the track stays in the library).
    RemoveFromPlaylist { playlist: i64, position: usize },
    /// Moves the entry at `from` so it ends up at position `to` (clamped to the end).
    MoveInPlaylist {
        playlist: i64,
        from: usize,
        to: usize,
    },
    /// Checks that each `(position, track)` of `entries` holds in `playlist` (positions
    /// from 0), changing nothing; fails with [`LibraryError::PlaylistChanged`] otherwise.
    /// Put first in a [`WriteOp::Batch`], it keeps an edit made from an older view of the
    /// playlist from touching other entries.
    ExpectPlaylistEntries {
        playlist: i64,
        entries: Vec<(usize, i64)>,
    },
    /// Applies several ops in one transaction: all of them or none.
    Batch(Vec<WriteOp>),
    /// Runs raw statements in one transaction. Meant for tests and maintenance; regular
    /// code uses the typed ops.
    #[doc(hidden)]
    Raw(Vec<RawStatement>),
    /// Test hook: runs the first `after` statements, then panics inside the transaction.
    #[cfg(feature = "test-hooks")]
    #[doc(hidden)]
    PanicAfter {
        statements: Vec<RawStatement>,
        after: usize,
    },
}

/// The payload of [`WriteOp::PutStems`]: one stems cache entry.
#[derive(Clone, PartialEq, Eq)]
pub struct StemsRow {
    pub track_id: i64,
    /// The track's fingerprint the stems were separated from.
    pub fingerprint: Vec<u8>,
    /// The entry's folder, relative to the stems folder.
    pub path: String,
    /// The entry's files' total size.
    pub bytes: u64,
    pub prepared: bool,
}

impl std::fmt::Debug for StemsRow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StemsRow")
            .field("track_id", &self.track_id)
            .field("path", &self.path)
            .field("bytes", &self.bytes)
            .field("prepared", &self.prepared)
            .finish_non_exhaustive()
    }
}

/// A track file's size and modification time as stored by the last scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileStamp {
    pub size: Option<i64>,
    pub mtime: Option<i64>,
}

/// The payload of [`WriteOp::StoreAnalysis`].
#[derive(Clone, PartialEq)]
pub struct StoreAnalysis {
    pub track_id: i64,
    /// The track's fingerprint when its audio was read.
    pub fingerprint: Vec<u8>,
    /// The track's size and mtime when its audio was read; `None` skips that check.
    pub file: Option<FileStamp>,
    pub analyzer_version: u32,
    pub bpm: Option<f64>,
    pub first_beat_frames: Option<f64>,
    /// [`analysis::Key::index`].
    pub key: Option<u8>,
    /// [`analysis::Waveform::to_bytes`].
    pub waveform: Vec<u8>,
}

impl std::fmt::Debug for StoreAnalysis {
    /// Shows the blobs' lengths rather than their bytes, keeping logged failures short.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StoreAnalysis")
            .field("track_id", &self.track_id)
            .field("fingerprint_len", &self.fingerprint.len())
            .field("file", &self.file)
            .field("analyzer_version", &self.analyzer_version)
            .field("bpm", &self.bpm)
            .field("first_beat_frames", &self.first_beat_frames)
            .field("key", &self.key)
            .field("waveform_len", &self.waveform.len())
            .finish()
    }
}

/// What a committed op produced.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub enum WriteOutcome {
    /// The op was committed and has nothing to report.
    Done,
    /// A [`WriteOp::CreatePlaylist`] or [`WriteOp::CreatePlaylistWith`] was committed; this
    /// is the new playlist's id.
    Created { id: i64 },
    /// A [`WriteOp::EditGrid`] was committed (or changed nothing); this is the track's
    /// grid afterwards, read in the same transaction.
    Grid(Beatgrid),
    /// A [`WriteOp::Raw`] was committed.
    Raw {
        rows_changed: usize,
        last_insert_rowid: i64,
    },
}

/// How many unreported failures [`Writer::take_failed_writes`] holds; beyond this the
/// oldest are dropped (the counter still counts them).
pub(crate) const FAILED_CAPACITY: usize = 1024;

/// An op queued with [`Writer::submit`] that could not be applied. It was rolled back;
/// the app should tell the user and may resubmit `op`.
#[derive(Debug)]
pub struct FailedWrite {
    pub op: WriteOp,
    pub error: LibraryError,
}

type Reply = mpsc::Sender<Result<WriteOutcome, LibraryError>>;

enum Message {
    Op { op: WriteOp, reply: Option<Reply> },
    Shutdown,
}

impl std::fmt::Debug for Message {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Op { op, .. } => f.debug_tuple("Op").field(op).finish(),
            Self::Shutdown => f.write_str("Shutdown"),
        }
    }
}

/// State shared by every handle and the writer thread.
#[derive(Debug, Default)]
struct Shared {
    /// Set once shutdown begins. Senders hold the read lock while sending, so once the
    /// flag is set no op can land in the queue behind `Shutdown` and be lost.
    closed: RwLock<bool>,
    failed_count: AtomicU64,
    failed: Mutex<VecDeque<FailedWrite>>,
}

/// A cloneable handle to the writer thread.
#[derive(Debug, Clone)]
pub struct Writer {
    tx: SyncSender<Message>,
    shared: Arc<Shared>,
}

impl Writer {
    /// Queues `op` and returns as soon as it is queued (blocking only while the queue is
    /// full, never dropping it). If applying it later fails, it is rolled back and
    /// reported through [`Writer::take_failed_writes`] and
    /// [`Writer::failed_write_count`]; use [`Writer::submit_wait`] to get the outcome
    /// directly.
    ///
    /// # Errors
    ///
    /// [`LibraryError::WriterGone`] once the library has been closed.
    pub fn submit(&self, op: WriteOp) -> Result<(), LibraryError> {
        self.send(Message::Op { op, reply: None })
    }

    /// Queues `op` and waits until it has been committed (or rolled back on error).
    ///
    /// # Errors
    ///
    /// The op's own error (it was rolled back), or [`LibraryError::WriterGone`] once the
    /// library has been closed.
    pub fn submit_wait(&self, op: WriteOp) -> Result<WriteOutcome, LibraryError> {
        let (reply, result) = mpsc::channel();
        self.send(Message::Op {
            op,
            reply: Some(reply),
        })?;
        result.recv().map_err(|_| LibraryError::WriterGone)?
    }

    /// Takes the ops queued with [`Writer::submit`] that failed since the last call,
    /// oldest first (at most the latest 1024).
    #[must_use]
    pub fn take_failed_writes(&self) -> Vec<FailedWrite> {
        lock(&self.shared.failed).drain(..).collect()
    }

    /// How many ops queued with [`Writer::submit`] have failed since the library opened.
    #[must_use]
    pub fn failed_write_count(&self) -> u64 {
        self.shared.failed_count.load(Ordering::Relaxed)
    }

    fn send(&self, message: Message) -> Result<(), LibraryError> {
        let closed = self
            .shared
            .closed
            .read()
            .unwrap_or_else(PoisonError::into_inner);
        if *closed {
            return Err(LibraryError::WriterGone);
        }
        self.tx.send(message).map_err(|_| LibraryError::WriterGone)
    }

    /// Closes the queue: every op already queued is applied, later sends get
    /// [`LibraryError::WriterGone`], then the thread stops.
    pub(crate) fn shutdown(&self) {
        *self
            .shared
            .closed
            .write()
            .unwrap_or_else(PoisonError::into_inner) = true;
        let _ = self.tx.send(Message::Shutdown);
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Starts the writer thread on `conn`, which must already be migrated.
pub(crate) fn spawn(conn: Connection) -> Result<(Writer, JoinHandle<()>), LibraryError> {
    let (tx, rx) = mpsc::sync_channel(QUEUE_CAPACITY);
    let shared = Arc::new(Shared::default());
    let thread_shared = Arc::clone(&shared);
    let handle = std::thread::Builder::new()
        .name("library-writer".into())
        .spawn(move || run(conn, &rx, &thread_shared))?;
    Ok((Writer { tx, shared }, handle))
}

fn run(mut conn: Connection, rx: &Receiver<Message>, shared: &Shared) {
    while let Ok(Message::Op { op, reply }) = rx.recv() {
        let result = apply_guarded(&mut conn, &op);
        match (reply, result) {
            // The waiter may have gone away; the outcome stands regardless.
            (Some(reply), result) => {
                let _ = reply.send(result);
            }
            (None, Ok(_)) => {}
            (None, Err(error)) => {
                shared.failed_count.fetch_add(1, Ordering::Relaxed);
                let mut failed = lock(&shared.failed);
                if failed.len() >= FAILED_CAPACITY {
                    failed.pop_front();
                }
                failed.push_back(FailedWrite { op, error });
            }
        }
    }
}

/// Applies `op` in a transaction, turning a panic into [`LibraryError::WriterPanicked`]
/// with the transaction rolled back, so one bad op cannot wedge the library.
fn apply_guarded(conn: &mut Connection, op: &WriteOp) -> Result<WriteOutcome, LibraryError> {
    match catch_unwind(AssertUnwindSafe(|| apply_in_transaction(conn, op))) {
        Ok(result) => result,
        Err(_) => {
            // Dropping the transaction during unwinding rolls it back; make sure.
            if !conn.is_autocommit() {
                let _ = conn.execute_batch("ROLLBACK");
            }
            Err(LibraryError::WriterPanicked)
        }
    }
}

fn apply_in_transaction(conn: &mut Connection, op: &WriteOp) -> Result<WriteOutcome, LibraryError> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let outcome = apply(&tx, op)?;
    tx.commit()?;
    Ok(outcome)
}

fn apply(tx: &Transaction<'_>, op: &WriteOp) -> Result<WriteOutcome, LibraryError> {
    match op {
        WriteOp::SetSetting {
            key,
            value: Some(v),
        } => {
            tx.execute(
                "INSERT INTO settings(key, value) VALUES (?1, ?2) \
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                (key, v),
            )?;
            Ok(WriteOutcome::Done)
        }
        WriteOp::SetSetting { key, value: None } => {
            tx.execute("DELETE FROM settings WHERE key = ?1", [key])?;
            Ok(WriteOutcome::Done)
        }
        WriteOp::InsertTrack(file) => {
            tx.execute(
                "INSERT INTO tracks(path, fingerprint, size, mtime, missing, title, artist, \
                 album, genre, duration_frames, sample_rate, added_at) \
                 VALUES (?1, ?2, ?3, ?4, 0, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                rusqlite::params![
                    file.path,
                    file.fingerprint,
                    file.size,
                    file.mtime,
                    file.tags.title,
                    file.tags.artist,
                    file.tags.album,
                    file.tags.genre,
                    file.tags
                        .duration_frames
                        .and_then(|f| i64::try_from(f).ok()),
                    file.tags.sample_rate,
                    unix_now(),
                ],
            )?;
            Ok(WriteOutcome::Done)
        }
        WriteOp::UpdateTrack { id, file } => {
            let changed = tx.execute(
                "UPDATE tracks SET path = ?2, fingerprint = ?3, size = ?4, mtime = ?5, \
                 missing = 0, title = ?6, artist = ?7, album = ?8, genre = ?9, \
                 duration_frames = ?10, sample_rate = ?11 WHERE id = ?1",
                rusqlite::params![
                    id,
                    file.path,
                    file.fingerprint,
                    file.size,
                    file.mtime,
                    file.tags.title,
                    file.tags.artist,
                    file.tags.album,
                    file.tags.genre,
                    file.tags
                        .duration_frames
                        .and_then(|f| i64::try_from(f).ok()),
                    file.tags.sample_rate,
                ],
            )?;
            if changed == 0 {
                return Err(LibraryError::Sqlite(rusqlite::Error::QueryReturnedNoRows));
            }
            Ok(WriteOutcome::Done)
        }
        WriteOp::ClearAnalysis(id) => {
            tx.execute(
                "UPDATE analysis SET analyzer_version = 0, waveform = NULL, key = NULL, \
                 analyzed_at = NULL WHERE track_id = ?1 AND grid_locked",
                [id],
            )?;
            tx.execute(
                "DELETE FROM analysis WHERE track_id = ?1 AND NOT grid_locked",
                [id],
            )?;
            Ok(WriteOutcome::Done)
        }
        WriteOp::ResetAnalysis(id) => {
            tx.execute(
                "UPDATE analysis SET analyzer_version = 0 WHERE track_id = ?1",
                [id],
            )?;
            Ok(WriteOutcome::Done)
        }
        WriteOp::StoreAnalysis(a) => {
            store_analysis(tx, a)?;
            Ok(WriteOutcome::Done)
        }
        WriteOp::EditGrid {
            track,
            edit,
            playhead_frames,
            sample_rate,
        } => grid::apply(tx, *track, *edit, *playhead_frames, *sample_rate),
        WriteOp::SetCue { track, frames } => {
            cues::set(tx, *track, *frames)?;
            Ok(WriteOutcome::Done)
        }
        WriteOp::SetHotcue {
            track,
            slot,
            frames,
            loop_frames,
        } => {
            cues::set_hotcue(tx, *track, *slot, *frames, *loop_frames)?;
            Ok(WriteOutcome::Done)
        }
        WriteOp::PutHotcue { track, slot, point } => {
            cues::put_hotcue(tx, *track, *slot, point)?;
            Ok(WriteOutcome::Done)
        }
        WriteOp::SetHotcueLabel {
            track,
            slot,
            name,
            color,
        } => {
            cues::set_hotcue_label(tx, *track, *slot, name.as_deref(), *color)?;
            Ok(WriteOutcome::Done)
        }
        WriteOp::DeleteHotcue { track, slot } => {
            cues::delete_hotcue(tx, *track, *slot)?;
            Ok(WriteOutcome::Done)
        }
        WriteOp::TrackLoaded(id) => {
            let changed = tx.execute(
                "UPDATE tracks SET last_loaded_at = ?2 WHERE id = ?1",
                (id, unix_now()),
            )?;
            if changed == 0 {
                return Err(LibraryError::NoSuchTrack(*id));
            }
            Ok(WriteOutcome::Done)
        }
        WriteOp::PutStems(row) => {
            if row.prepared {
                tx.execute(
                    "UPDATE tracks SET stems_prepared = 1 WHERE id = ?1",
                    [row.track_id],
                )?;
            }
            // A track once prepared stays so, even after its entry was removed as damaged.
            tx.execute(
                "INSERT INTO stems(track_id, fingerprint, path, bytes, prepared, created_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5 OR coalesce( \
                     (SELECT stems_prepared FROM tracks WHERE id = ?1), 0), ?6) \
                 ON CONFLICT(track_id) DO UPDATE SET \
                     fingerprint = excluded.fingerprint, path = excluded.path, \
                     bytes = excluded.bytes, prepared = prepared OR excluded.prepared, \
                     created_at = excluded.created_at",
                rusqlite::params![
                    row.track_id,
                    row.fingerprint,
                    row.path,
                    i64::try_from(row.bytes).unwrap_or(i64::MAX),
                    row.prepared,
                    unix_now(),
                ],
            )?;
            Ok(WriteOutcome::Done)
        }
        WriteOp::MarkStemsPrepared(id) => {
            tx.execute("UPDATE tracks SET stems_prepared = 1 WHERE id = ?1", [id])?;
            tx.execute("UPDATE stems SET prepared = 1 WHERE track_id = ?1", [id])?;
            Ok(WriteOutcome::Done)
        }
        WriteOp::RemoveStems(id) => {
            tx.execute("DELETE FROM stems WHERE track_id = ?1", [id])?;
            Ok(WriteOutcome::Done)
        }
        WriteOp::MarkMissing(ids) => {
            let mut stmt = tx.prepare_cached("UPDATE tracks SET missing = 1 WHERE id = ?1")?;
            for id in ids {
                stmt.execute([id])?;
            }
            Ok(WriteOutcome::Done)
        }
        WriteOp::AddFolder(path) => {
            tx.execute(
                "INSERT INTO folders(path) VALUES (?1) ON CONFLICT(path) DO NOTHING",
                [path],
            )?;
            Ok(WriteOutcome::Done)
        }
        WriteOp::RemoveFolder(path) => {
            tx.execute("DELETE FROM folders WHERE path = ?1", [path])?;
            Ok(WriteOutcome::Done)
        }
        WriteOp::CreatePlaylist { .. }
        | WriteOp::CreatePlaylistWith { .. }
        | WriteOp::RenamePlaylist { .. }
        | WriteOp::DeletePlaylist(_)
        | WriteOp::MovePlaylist { .. }
        | WriteOp::AddToPlaylist { .. }
        | WriteOp::RemoveFromPlaylist { .. }
        | WriteOp::MoveInPlaylist { .. }
        | WriteOp::ExpectPlaylistEntries { .. } => crate::playlists::apply(tx, op),
        WriteOp::Batch(ops) => {
            for op in ops {
                apply(tx, op)?;
            }
            Ok(WriteOutcome::Done)
        }
        WriteOp::Raw(statements) => run_raw(tx, statements),
        #[cfg(feature = "test-hooks")]
        WriteOp::PanicAfter { statements, after } => {
            let head = statements.get(..*after).unwrap_or(statements);
            run_raw(tx, head)?;
            panic!("test hook: writer panicked inside a transaction");
        }
    }
}

fn store_analysis(tx: &Transaction<'_>, a: &StoreAnalysis) -> Result<(), LibraryError> {
    let stored: Option<(Vec<u8>, FileStamp)> = tx
        .query_row(
            "SELECT fingerprint, size, mtime FROM tracks WHERE id = ?1",
            [a.track_id],
            |r| {
                Ok((
                    r.get(0)?,
                    FileStamp {
                        size: r.get(1)?,
                        mtime: r.get(2)?,
                    },
                ))
            },
        )
        .optional()?;
    let version = match stored {
        None => return Err(LibraryError::NoSuchTrack(a.track_id)),
        Some((f, _)) if f != a.fingerprint => {
            return Err(LibraryError::TrackChanged(a.track_id));
        }
        // Same first seconds, but the file changed since it was read: keep this
        // analysis for now and redo it.
        Some((_, stamp)) if a.file.is_some_and(|read| read != stamp) => 0,
        Some(_) => a.analyzer_version,
    };
    tx.execute(
        "INSERT INTO analysis(track_id, analyzer_version, bpm, first_beat_frames, \
                              grid_locked, key, waveform, analyzed_at) \
         VALUES (?1, ?2, ?3, ?4, 0, ?5, ?6, ?7) \
         ON CONFLICT(track_id) DO UPDATE SET \
             analyzer_version = excluded.analyzer_version, \
             bpm = CASE WHEN grid_locked THEN bpm ELSE excluded.bpm END, \
             first_beat_frames = CASE WHEN grid_locked THEN first_beat_frames \
                                      ELSE excluded.first_beat_frames END, \
             key = excluded.key, \
             waveform = excluded.waveform, \
             analyzed_at = excluded.analyzed_at",
        rusqlite::params![
            a.track_id,
            version,
            a.bpm,
            a.first_beat_frames,
            a.key,
            a.waveform,
            unix_now(),
        ],
    )?;
    Ok(())
}

/// Seconds since the Unix epoch (0 if the clock is before it).
pub(crate) fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

fn run_raw(
    tx: &Transaction<'_>,
    statements: &[RawStatement],
) -> Result<WriteOutcome, LibraryError> {
    let mut rows_changed = 0;
    for s in statements {
        rows_changed += tx.execute(&s.sql, params_from_iter(&s.params))?;
    }
    Ok(WriteOutcome::Raw {
        rows_changed,
        last_insert_rowid: tx.last_insert_rowid(),
    })
}
