//! Covers found online and kept for tracks: one row per track that was looked up, the
//! picture a file at `<library folder>/covers/<track id>.jpg`.

use std::path::{Path, PathBuf};

use rusqlite::{OptionalExtension, Transaction};

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
                    state: match state.as_str() {
                        "found" => CoverState::Found,
                        "missed" => CoverState::Missed,
                        _ => CoverState::Removed,
                    },
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
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let mut stmt = self.connection().prepare_cached(
            "SELECT t.id, t.artist, t.title, t.path FROM tracks t \
             LEFT JOIN covers c ON c.track_id = t.id \
             WHERE t.missing = 0 AND (c.track_id IS NULL OR (c.state = 'missed' AND ?1)) \
             ORDER BY t.id LIMIT ?2",
        )?;
        let rows = stmt.query_map((retry, limit), |r| {
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
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }
}

/// The file [`WriteOp::ClearCover`] deletes for `track`, built from its id alone.
fn cover_file(folder: &Path, track: i64) -> PathBuf {
    folder.join("covers").join(format!("{track}.jpg"))
}

/// Applies a cover op's row change. Its file is deleted by [`delete_cleared_files`] once
/// the transaction has committed.
pub(crate) fn apply(tx: &Transaction<'_>, op: &WriteOp) -> Result<WriteOutcome, LibraryError> {
    let (track, sql, file) = match op {
        WriteOp::SetCover { track, file } => {
            let file = file
                .to_str()
                .ok_or_else(|| LibraryError::NonUtf8Path(file.clone()))?;
            (
                *track,
                "INSERT INTO covers(track_id, state, file, updated_at) \
                 SELECT id, 'found', ?2, ?3 FROM tracks WHERE id = ?1 \
                 ON CONFLICT(track_id) DO UPDATE SET state = 'found', \
                     file = excluded.file, updated_at = excluded.updated_at",
                Some(file),
            )
        }
        // A miss never undoes a cover that was found or removed.
        WriteOp::MissCover { track } => (
            *track,
            "INSERT INTO covers(track_id, state, file, updated_at) \
             SELECT id, 'missed', ?2, ?3 FROM tracks WHERE id = ?1 \
             ON CONFLICT(track_id) DO UPDATE SET updated_at = excluded.updated_at \
             WHERE covers.state = 'missed'",
            None,
        ),
        WriteOp::ClearCover { track } => (
            *track,
            "INSERT INTO covers(track_id, state, file, updated_at) \
             SELECT id, 'removed', ?2, ?3 FROM tracks WHERE id = ?1 \
             ON CONFLICT(track_id) DO UPDATE SET state = 'removed', file = NULL, \
                 updated_at = excluded.updated_at",
            None,
        ),
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
    tx.execute(sql, rusqlite::params![track, file, unix_now()])?;
    Ok(WriteOutcome::Done)
}

/// Deletes the picture of each [`WriteOp::ClearCover`] in a committed `op`. Only
/// `<folder>/covers/<id>.jpg` is touched, whatever the row said; a missing file is fine.
pub(crate) fn delete_cleared_files(folder: &Path, op: &WriteOp) {
    match op {
        WriteOp::ClearCover { track } => {
            let _ = std::fs::remove_file(cover_file(folder, *track));
        }
        WriteOp::Batch(ops) => {
            for op in ops {
                delete_cleared_files(folder, op);
            }
        }
        _ => {}
    }
}
