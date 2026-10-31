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
use rusqlite::{Connection, Transaction, TransactionBehavior, params_from_iter};

use crate::LibraryError;
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
    /// Deletes a track's analysis, which no longer matches its audio. Cues are kept.
    ClearAnalysis(i64),
    /// Flags these tracks' files as missing. Tracks are never deleted by a scan.
    MarkMissing(Vec<i64>),
    /// Adds a library folder (no-op if already there). Only the `folders` table changes.
    AddFolder(String),
    /// Removes a library folder. Only the `folders` table changes: its tracks stay.
    RemoveFolder(String),
    /// Creates a playlist at the end of the playlist list; reports its id as
    /// [`WriteOutcome::Created`].
    CreatePlaylist { name: String },
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

/// What a committed op produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum WriteOutcome {
    /// The op was committed and has nothing to report.
    Done,
    /// A [`WriteOp::CreatePlaylist`] was committed; this is the new playlist's id.
    Created { id: i64 },
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
            tx.execute("DELETE FROM analysis WHERE track_id = ?1", [id])?;
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
        | WriteOp::RenamePlaylist { .. }
        | WriteOp::DeletePlaylist(_)
        | WriteOp::MovePlaylist { .. }
        | WriteOp::AddToPlaylist { .. }
        | WriteOp::RemoveFromPlaylist { .. }
        | WriteOp::MoveInPlaylist { .. } => crate::playlists::apply(tx, op),
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

/// Seconds since the Unix epoch (0 if the clock is before it).
fn unix_now() -> i64 {
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
