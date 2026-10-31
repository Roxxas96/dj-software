//! Schema migrations, as plain SQL.
//!
//! The database's `PRAGMA user_version` is the number of migrations applied: version N is
//! reached by running `MIGRATIONS[N - 1]`. Migrations are append-only; never edit one that
//! has shipped. All pending migrations run inside one transaction, so a migration must not
//! contain `BEGIN`/`COMMIT` or pragmas that are no-ops inside a transaction (such as
//! `foreign_keys` or `journal_mode`).

/// Version 1: tracks, folders, analysis, cues, hot cues, playlists and settings.
///
/// Cues and hot cues deliberately have no `ON DELETE CASCADE`: with foreign keys on, a
/// track that still has cues cannot be deleted by accident. Scans never delete tracks;
/// missing files are only flagged.
const V1: &str = "
CREATE TABLE tracks (
    id INTEGER PRIMARY KEY,
    path TEXT UNIQUE NOT NULL,
    fingerprint BLOB NOT NULL,
    size INTEGER,
    mtime INTEGER,
    missing INTEGER NOT NULL DEFAULT 0,
    title TEXT,
    artist TEXT,
    album TEXT,
    genre TEXT,
    duration_frames INTEGER,
    sample_rate INTEGER,
    added_at INTEGER NOT NULL
);
CREATE INDEX tracks_fingerprint ON tracks(fingerprint);
CREATE INDEX tracks_artist ON tracks(artist COLLATE NOCASE);
CREATE INDEX tracks_title ON tracks(title COLLATE NOCASE);
CREATE INDEX tracks_added ON tracks(added_at);

CREATE TABLE folders (
    id INTEGER PRIMARY KEY,
    path TEXT UNIQUE NOT NULL
);

CREATE TABLE analysis (
    track_id INTEGER PRIMARY KEY REFERENCES tracks(id) ON DELETE CASCADE,
    analyzer_version INTEGER NOT NULL,
    bpm REAL,
    first_beat_frames REAL,
    grid_locked INTEGER NOT NULL DEFAULT 0,
    key INTEGER,
    waveform BLOB,
    analyzed_at INTEGER
);

CREATE TABLE cues (
    track_id INTEGER PRIMARY KEY REFERENCES tracks(id),
    cue_frames REAL NOT NULL,
    updated_at INTEGER
);

CREATE TABLE hotcues (
    track_id INTEGER NOT NULL REFERENCES tracks(id),
    slot INTEGER NOT NULL CHECK (slot BETWEEN 1 AND 8),
    frames REAL NOT NULL,
    color INTEGER,
    name TEXT,
    PRIMARY KEY (track_id, slot)
);

CREATE TABLE playlists (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    position INTEGER NOT NULL
);

CREATE TABLE playlist_tracks (
    playlist_id INTEGER NOT NULL REFERENCES playlists(id) ON DELETE CASCADE,
    track_id INTEGER NOT NULL REFERENCES tracks(id),
    position INTEGER NOT NULL,
    PRIMARY KEY (playlist_id, position)
);

CREATE TABLE settings (
    key TEXT PRIMARY KEY,
    value TEXT
);
";

/// Every migration, in order. The latest schema version is `MIGRATIONS.len()`.
pub const MIGRATIONS: &[&str] = &[V1];
