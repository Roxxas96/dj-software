//! The music library: a SQLite database holding tracks, analysis, cues, hot cues and
//! playlists.
//!
//! Data safety comes first. The database is backed up every time it is opened, schema
//! migrations run in a single transaction, and every write goes through one writer thread
//! that applies each [`WriteOp`] in its own transaction, so a change is either fully
//! committed or not there at all.

mod backup;
mod browser;
mod covers;
mod cues;
mod db;
pub mod fingerprint;
mod grid;
mod jobs;
mod playlists;
mod scan;
pub mod schema;
mod settings;
mod stems_cache;
pub mod tags;
mod watch;
mod writer;

pub use browser::{
    BROWSER_PAGE_ROWS, Browser, BrowserEdit, BrowserRequest, BrowserView, Drive, Focus,
    ListIdentity, ListRow, TreeKind, TreeRow, drives, remix_of,
};
pub use covers::{COVERS_RETRY_MISSED, CoverQuery, CoverRow, CoverState};
pub use cues::HotcuePoint;
pub use db::{Library, Reader};
pub use grid::{
    Beatgrid, GridEdit, GridError, MAX_GRID_BPM, MIN_GRID_BPM, MIN_TAPS, TAP_RESET_MICROS,
    TAP_WINDOW,
};
pub use jobs::{
    ANALYZER_VERSION, AnalysisProgress, AnalysisRow, AnalysisWarning, Analyzer, PROGRESS_TAIL,
    PendingTrack, REPORT_CAPACITY, WAVEFORM_FRAMES_PER_BUCKET, default_threads,
};
pub use playlists::{
    PlaylistSummary, TrackDetails, TrackFilter, TrackRow, TrackSort, TrackSortKey, TrackSource,
};
pub use scan::{
    EXTENSIONS, ScanProgress, ScanReport, TrackFile, add_folder, add_folder_files, remove_folder,
    scan_folder,
};
pub use settings::{
    BPM_RANGE, BpmRangeError, InvalidBpmRange, KEY_NOTATION, LOW_BPM_RANGE_WARNING,
    format_bpm_range, parse_bpm_range, validate_bpm_range,
};
pub use stems_cache::{Eviction, RECENT_SECONDS, StemCache, StemKey};
pub use watch::{DEBOUNCE, RESCAN_INTERVAL, WatchHealth, Watcher, watch, watch_with_debounce};
pub use writer::{
    FailedWrite, FileStamp, RawStatement, StemsRow, StoreAnalysis, WriteOp, WriteOutcome, Writer,
};

/// Everything that can go wrong opening or writing the library.
#[derive(Debug, thiserror::Error)]
pub enum LibraryError {
    #[error("library I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("library database error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("could not back up the library before opening it: {0}")]
    Backup(#[source] rusqlite::Error),
    #[error(
        "the library database {} failed its integrity check ({detail}); refusing to open \
         it. Restore a backup from {}",
        .db.display(),
        .backups.display()
    )]
    Corrupt {
        db: std::path::PathBuf,
        backups: std::path::PathBuf,
        detail: String,
    },
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
    #[error("track {0} does not exist")]
    NoSuchTrack(i64),
    #[error("{0} is not a valid cue point: it must be finite and not negative")]
    InvalidCue(f64),
    #[error("{0} is not a valid loop length: it must be finite and above zero")]
    InvalidLoop(f64),
    #[error("{0} is not a hotcue slot: slots are 1 to 8")]
    InvalidHotcueSlot(u8),
    #[error("hotcue {slot} of track {track} is not set")]
    NoSuchHotcue { track: i64, slot: u8 },
    #[error("position {position} is out of range (the playlist has {len} entries)")]
    PositionOutOfRange { position: usize, len: usize },
    #[error("playlist {0} changed: its entries are not where they were")]
    PlaylistChanged(i64),
    #[error("there is already a playlist called {0}")]
    PlaylistNameTaken(String),
    #[error("track {0}'s audio changed while it was being analyzed; analyze it again")]
    TrackChanged(i64),
    #[error("track {track_id} has a corrupt stored waveform: {source}")]
    CorruptWaveform {
        track_id: i64,
        #[source]
        source: analysis::WaveformError,
    },
    #[error(transparent)]
    Grid(#[from] GridError),
    #[error("could not encode the stems: {0}")]
    StemEncode(String),
    #[error("the library writer has shut down")]
    WriterGone,
    #[error("the library writer panicked while applying a change; it was rolled back")]
    WriterPanicked,
}
