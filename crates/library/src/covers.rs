//! Covers found online and kept for tracks: one row per track that was looked up, the
//! picture a file at `<library folder>/covers/<track id>.jpg` (or `<track id>-2.jpg`, see
//! [`cover_files`]).

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension, Transaction};

use crate::writer::{WriteOp, WriteOutcome, unix_now};
use crate::{LibraryError, Reader};

/// The setting that makes [`Reader::tracks_needing_covers`] ask about tracks whose lookup
/// missed again, when it is `1`.
pub const COVERS_RETRY_MISSED: &str = "covers.retry_missed";

/// What the library knows of a track's cover.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoverState {
    /// A cover was found; its file is kept.
    Found,
    /// The lookup found nothing.
    Missed,
    /// The user removed the cover: never looked up again.
    Removed,
}

/// A track's row of the covers table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoverRow {
    pub state: CoverState,
    /// The picture's file, when `state` is [`CoverState::Found`].
    pub file: Option<PathBuf>,
    /// Seconds since the Unix epoch.
    pub updated_at: i64,
}

/// A track to look a cover up for. Only these fields ever go to a cover service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoverQuery {
    pub track: i64,
    pub artist: Option<String>,
    pub title: Option<String>,
    /// The file's name without its extension, for a track with neither tag.
    pub file_stem: String,
}

impl Reader {
    /// `track`'s cover row; `None` when it was never looked up.
    ///
    /// # Errors
    ///
    /// Any database error.
    pub fn cover(&self, track: i64) -> Result<Option<CoverRow>, LibraryError> {
        Ok(self
            .connection()
            .prepare_cached("SELECT state, file, updated_at FROM covers WHERE track_id = ?1")?
            .query_row([track], |r| {
                let state: String = r.get(0)?;
                let file: Option<String> = r.get(1)?;
                Ok(CoverRow {
                    state: state_of(&state),
                    file: file.map(PathBuf::from),
                    updated_at: r.get(2)?,
                })
            })
            .optional()?)
    }

    /// Up to `limit` tracks, by id, whose files exist and that have no cover row, plus the
    /// ones whose lookup missed when the [`COVERS_RETRY_MISSED`] setting is `1`. Found and
    /// removed covers are never listed.
    ///
    /// # Errors
    ///
    /// Any database error.
    pub fn tracks_needing_covers(&self, limit: usize) -> Result<Vec<CoverQuery>, LibraryError> {
        let retry = self.setting(COVERS_RETRY_MISSED)?.as_deref() == Some("1");
        self.tracks_needing_covers_after(i64::MIN, limit, retry)
    }

    /// [`Reader::tracks_needing_covers`] from the track after `after` on, by id, with the
    /// missed tracks when `retry` (the [`COVERS_RETRY_MISSED`] flag as the caller read it
    /// once for its whole walk): a lookup walks the library a batch at a time, past the
    /// tracks it could not look up yet.
    ///
    /// # Errors
    ///
    /// Any database error.
    pub fn tracks_needing_covers_after(
        &self,
        after: i64,
        limit: usize,
        retry: bool,
    ) -> Result<Vec<CoverQuery>, LibraryError> {
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let mut stmt = self.connection().prepare_cached(
            "SELECT t.id, t.artist, t.title, t.path FROM tracks t \
             LEFT JOIN covers c ON c.track_id = t.id \
             WHERE t.missing = 0 AND t.id > ?3 \
                 AND (c.track_id IS NULL OR (c.state = 'missed' AND ?1)) \
             ORDER BY t.id LIMIT ?2",
        )?;
        let rows = stmt.query_map((retry, limit, after), cover_query)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// What a lookup of `track`'s cover would send, whatever its cover's state; `None`
    /// when there is no such track.
    ///
    /// # Errors
    ///
    /// Any database error.
    pub fn cover_query(&self, track: i64) -> Result<Option<CoverQuery>, LibraryError> {
        Ok(self
            .connection()
            .prepare_cached("SELECT id, artist, title, path FROM tracks WHERE id = ?1")?
            .query_row([track], cover_query)
            .optional()?)
    }
}

/// The cover state of each of `tracks` that has a cover row, in one query.
pub(crate) fn cover_states(
    reader: &Reader,
    tracks: &[i64],
) -> Result<std::collections::HashMap<i64, CoverState>, LibraryError> {
    if tracks.is_empty() {
        return Ok(std::collections::HashMap::new());
    }
    let marks = vec!["?"; tracks.len()].join(", ");
    let sql = format!("SELECT track_id, state FROM covers WHERE track_id IN ({marks})");
    let mut stmt = reader.connection().prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(tracks), |r| {
        let state: String = r.get(1)?;
        Ok((r.get(0)?, state_of(&state)))
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// A stored state as a [`CoverState`].
fn state_of(state: &str) -> CoverState {
    match state {
        "found" => CoverState::Found,
        "missed" => CoverState::Missed,
        _ => CoverState::Removed,
    }
}

/// A row of `id, artist, title, path` as the lookup's query: the path is cut to its stem.
fn cover_query(r: &rusqlite::Row<'_>) -> rusqlite::Result<CoverQuery> {
    let path: String = r.get(3)?;
    Ok(CoverQuery {
        track: r.get(0)?,
        artist: r.get(1)?,
        title: r.get(2)?,
        file_stem: Path::new(&path)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default(),
    })
}

/// The two files a looked-up cover of `track` may be kept in, for the library whose
/// database is `library_db`: `<folder>/covers/<id>.jpg` and `<folder>/covers/<id>-2.jpg`.
/// A new cover for a track that has one goes to the file not in use, so whoever shows it
/// sees a new path and reads it again; the other file is deleted once the new one is
/// recorded.
#[must_use]
pub fn cover_files(library_db: &Path, track: i64) -> [PathBuf; 2] {
    files(library_db.parent().unwrap_or(Path::new(".")), track)
}

/// [`cover_files`] from the library's folder.
fn files(folder: &Path, track: i64) -> [PathBuf; 2] {
    let covers = folder.join("covers");
    [
        covers.join(format!("{track}.jpg")),
        covers.join(format!("{track}-2.jpg")),
    ]
}

/// Applies a cover op's row change. Its file is deleted by [`delete_after_commit`] once
/// the transaction has committed.
pub(crate) fn apply(tx: &Transaction<'_>, op: &WriteOp) -> Result<WriteOutcome, LibraryError> {
    let track = match op {
        WriteOp::SetCover { track, .. }
        | WriteOp::MissCover { track }
        | WriteOp::ClearCover { track } => *track,
        _ => return Ok(WriteOutcome::Done),
    };
    let exists: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM tracks WHERE id = ?1)",
        [track],
        |r| r.get(0),
    )?;
    if !exists {
        return Err(LibraryError::NoSuchTrack(track));
    }
    let now = unix_now();
    match op {
        // Without `force`, a removed cover stays removed: a lookup that was in flight when
        // the user removed it must not bring it back.
        WriteOp::SetCover { file, force, .. } => {
            let file = file
                .to_str()
                .ok_or_else(|| LibraryError::NonUtf8Path(file.clone()))?;
            tx.execute(
                "INSERT INTO covers(track_id, state, file, updated_at) \
                 VALUES (?1, 'found', ?2, ?3) \
                 ON CONFLICT(track_id) DO UPDATE SET state = 'found', \
                     file = excluded.file, updated_at = excluded.updated_at \
                 WHERE ?4 OR covers.state <> 'removed'",
                rusqlite::params![track, file, now, force],
            )?;
        }
        // A miss never undoes a cover that was found or removed.
        WriteOp::MissCover { .. } => {
            tx.execute(
                "INSERT INTO covers(track_id, state, file, updated_at) \
                 VALUES (?1, 'missed', NULL, ?2) \
                 ON CONFLICT(track_id) DO UPDATE SET updated_at = excluded.updated_at \
                 WHERE covers.state = 'missed'",
                rusqlite::params![track, now],
            )?;
        }
        _ => {
            tx.execute(
                "INSERT INTO covers(track_id, state, file, updated_at) \
                 VALUES (?1, 'removed', NULL, ?2) \
                 ON CONFLICT(track_id) DO UPDATE SET state = 'removed', file = NULL, \
                     updated_at = excluded.updated_at",
                rusqlite::params![track, now],
            )?;
        }
    }
    Ok(WriteOutcome::Done)
}

/// After a committed `op`: deletes the pictures of each [`WriteOp::ClearCover`], the
/// picture of a [`WriteOp::SetCover`] without `force` that was refused because the cover
/// is removed, and, once a [`WriteOp::SetCover`] recorded one of the track's two files,
/// the other one. Only the track's own files ([`cover_files`], built from its id) are ever
/// touched, whatever the row said; a missing file is fine.
pub(crate) fn delete_after_commit(conn: &Connection, folder: &Path, op: &WriteOp) {
    match op {
        WriteOp::ClearCover { track } => {
            for file in files(folder, *track) {
                let _ = std::fs::remove_file(file);
            }
        }
        WriteOp::SetCover { track, file, .. } => {
            let [first, second] = files(folder, *track);
            let other = if *file == first {
                second
            } else if *file == second {
                first
            } else {
                return;
            };
            let row = conn
                .query_row(
                    "SELECT state, file FROM covers WHERE track_id = ?1",
                    [track],
                    |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)),
                )
                .optional()
                .ok()
                .flatten();
            let Some((state, kept)) = row else { return };
            if state == "removed" {
                let _ = std::fs::remove_file(file);
            } else if state == "found" && kept.as_deref() == file.to_str() {
                let _ = std::fs::remove_file(other);
            }
        }
        WriteOp::Batch(ops) => {
            for op in ops {
                delete_after_commit(conn, folder, op);
            }
        }
        _ => {}
    }
}
