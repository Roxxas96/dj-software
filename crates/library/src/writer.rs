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
        WriteOp::Raw(statements) => run_raw(tx, statements),
        #[cfg(feature = "test-hooks")]
        WriteOp::PanicAfter { statements, after } => {
            let head = statements.get(..*after).unwrap_or(statements);
            run_raw(tx, head)?;
            panic!("test hook: writer panicked inside a transaction");
        }
    }
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
