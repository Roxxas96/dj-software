//! The music library: a SQLite database holding tracks, analysis, cues, hot cues and
//! playlists.
//!
//! Data safety comes first. The database is backed up every time it is opened, schema
//! migrations run in a single transaction, and every write goes through one writer thread
//! that applies each [`WriteOp`] in its own transaction, so a change is either fully
//! committed or not there at all.

mod backup;
mod db;
pub mod fingerprint;
mod playlists;
mod scan;
pub mod schema;
pub mod tags;
mod watch;
mod writer;

pub use db::{Library, Reader};
pub use playlists::{PlaylistSummary, TrackFilter, TrackRow, TrackSort, TrackSortKey};
pub use scan::{
    EXTENSIONS, ScanProgress, ScanReport, TrackFile, add_folder, remove_folder, scan_folder,
};
pub use watch::{DEBOUNCE, RESCAN_INTERVAL, WatchHealth, Watcher, watch, watch_with_debounce};
pub use writer::{FailedWrite, RawStatement, WriteOp, WriteOutcome, Writer};

/// Everything that can go wrong opening or writing the library.
#[derive(Debug, thiserror::Error)]
pub enum LibraryError {
    #[error("library I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("library database error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("could not back up the library before opening it: {0}")]
    Backup(#[source] rusqlite::Error),
    #[error("library migration to version {version} failed: {source}")]
    Migration {
        version: usize,
        #[source]
        source: rusqlite::Error,
    },
    #[error(
        "library schema version {found} is newer than this build supports ({supported}); \
         refusing to open it"
    )]
    TooNew { found: i64, supported: usize },
    #[error("path is not valid UTF-8: {}", .0.display())]
    NonUtf8Path(std::path::PathBuf),
    #[error("watching library folders failed: {0}")]
    Watch(#[from] notify::Error),
    #[error("playlist {0} does not exist")]
    NoSuchPlaylist(i64),
    #[error("position {position} is out of range (the playlist has {len} entries)")]
    PositionOutOfRange { position: usize, len: usize },
    #[error("the library writer has shut down")]
    WriterGone,
    #[error("the library writer panicked while applying a change; it was rolled back")]
    WriterPanicked,
}
