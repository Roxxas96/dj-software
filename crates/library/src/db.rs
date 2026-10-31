//! Opening the library: backup, connection setup, migrations, writer thread.

use std::path::{Path, PathBuf};
use std::thread::JoinHandle;
use std::time::Duration;

use rusqlite::{Connection, OpenFlags, TransactionBehavior};

use crate::grid::Taps;
use crate::writer::{self, Writer};
use crate::{LibraryError, backup, schema};

/// How long a connection waits on a lock held by another connection before giving up.
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// An open library. Dropping it applies every op queued so far, then stops the writer
/// thread; [`Writer`] handles that outlive it get [`LibraryError::WriterGone`].
#[derive(Debug)]
pub struct Library {
    path: PathBuf,
    writer: Writer,
    thread: Option<JoinHandle<()>>,
    /// Recent tap-tempo taps per track, for [`Library::edit_grid`].
    pub(crate) taps: Taps,
}

/// A read-only connection for one thread. Each statement sees everything committed
/// before it started.
#[derive(Debug)]
pub struct Reader {
    conn: Connection,
}

impl Reader {
    /// The underlying read-only connection.
    #[must_use]
    pub fn connection(&self) -> &Connection {
        &self.conn
    }
}

impl Library {
    /// Opens (creating if needed) the library at `path`, in this order: create the parent
    /// directories, back up the existing file (always when a migration is pending,
    /// otherwise unless a backup under 10 minutes old exists; the newest 10 are kept),
    /// open it in WAL mode
    /// with foreign keys on, migrate it to the latest schema in one transaction, and start
    /// the writer thread.
    ///
    /// # Errors
    ///
    /// Fails if the backup cannot be made (the library is then not opened at all), if a
    /// migration fails ([`LibraryError::Migration`], with the database left at its
    /// previous version), or if the database is newer than this build
    /// ([`LibraryError::TooNew`]).
    pub fn open(path: &Path) -> Result<Self, LibraryError> {
        Self::open_impl(path, schema::MIGRATIONS)
    }

    /// [`Library::open`] with a custom migration list, to test migration failures.
    #[cfg(feature = "test-hooks")]
    #[doc(hidden)]
    pub fn open_with_migrations(path: &Path, migrations: &[&str]) -> Result<Self, LibraryError> {
        Self::open_impl(path, migrations)
    }

    fn open_impl(path: &Path, migrations: &[&str]) -> Result<Self, LibraryError> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        if path.exists() {
            backup::backup_if_needed(path, migrations.len())?;
        }
        let mut conn = Connection::open(path)?;
        configure_writer(&conn)?;
        migrate(&mut conn, migrations)?;
        let (writer, thread) = writer::spawn(conn)?;
        Ok(Self {
            path: path.to_owned(),
            writer,
            thread: Some(thread),
            taps: Taps::default(),
        })
    }

    /// Opens a read-only connection for the calling thread.
    ///
    /// # Errors
    ///
    /// Fails if the database file cannot be opened.
    pub fn reader(&self) -> Result<Reader, LibraryError> {
        Self::reader_at(&self.path)
    }

    /// [`Library::reader`] from the database path, for threads that do not hold the
    /// library.
    pub(crate) fn reader_at(path: &Path) -> Result<Reader, LibraryError> {
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        conn.busy_timeout(BUSY_TIMEOUT)?;
        Ok(Reader { conn })
    }

    /// A handle for submitting changes. Cheap to clone and send to other threads.
    #[must_use]
    pub fn writer(&self) -> Writer {
        self.writer.clone()
    }

    /// Takes the ops queued with [`Writer::submit`] that failed since the last call; see
    /// [`Writer::take_failed_writes`].
    #[must_use]
    pub fn take_failed_writes(&self) -> Vec<crate::FailedWrite> {
        self.writer.take_failed_writes()
    }

    /// How many ops queued with [`Writer::submit`] have failed since the library opened.
    #[must_use]
    pub fn failed_write_count(&self) -> u64 {
        self.writer.failed_write_count()
    }

    /// The database file.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for Library {
    fn drop(&mut self) {
        self.writer.shutdown();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// WAL mode, foreign keys on, and `synchronous = FULL` so a commit survives power loss.
fn configure_writer(conn: &Connection) -> Result<(), LibraryError> {
    conn.busy_timeout(BUSY_TIMEOUT)?;
    let mode: String =
        conn.pragma_update_and_check(None, "journal_mode", "WAL", |row| row.get(0))?;
    if !mode.eq_ignore_ascii_case("wal") {
        return Err(LibraryError::Sqlite(rusqlite::Error::InvalidQuery));
    }
    conn.pragma_update(None, "foreign_keys", true)?;
    conn.pragma_update(None, "synchronous", "FULL")?;
    Ok(())
}

/// Brings the database to `migrations.len()` in a single transaction: on any failure
/// nothing is applied and `user_version` keeps its previous value.
fn migrate(conn: &mut Connection, migrations: &[&str]) -> Result<(), LibraryError> {
    let current: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    let latest = migrations.len();
    let applied = usize::try_from(current).unwrap_or(usize::MAX);
    if applied > latest {
        return Err(LibraryError::TooNew {
            found: current,
            supported: latest,
        });
    }
    if applied == latest {
        return Ok(());
    }
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    for (index, sql) in migrations.iter().enumerate().skip(applied) {
        tx.execute_batch(sql)
            .map_err(|source| LibraryError::Migration {
                version: index + 1,
                source,
            })?;
    }
    let fail = |source| LibraryError::Migration {
        version: latest,
        source,
    };
    let version = i64::try_from(latest)
        .map_err(|e| fail(rusqlite::Error::ToSqlConversionFailure(e.into())))?;
    tx.pragma_update(None, "user_version", version)
        .map_err(fail)?;
    tx.commit().map_err(fail)?;
    Ok(())
}
