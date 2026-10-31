//! Remembered cue points: one per track, written through immediately.

use rusqlite::{OptionalExtension, Transaction};

use crate::{Library, LibraryError, Reader};

/// Stores `frames` as track `track`'s cue point (upsert) inside `tx`.
pub(crate) fn set(tx: &Transaction<'_>, track: i64, frames: f64) -> Result<(), LibraryError> {
    if !frames.is_finite() || frames < 0.0 {
        return Err(LibraryError::InvalidCue(frames));
    }
    let exists = tx
        .query_row("SELECT 1 FROM tracks WHERE id = ?1", [track], |_| Ok(()))
        .optional()?
        .is_some();
    if !exists {
        return Err(LibraryError::NoSuchTrack(track));
    }
    tx.execute(
        "INSERT INTO cues(track_id, cue_frames, updated_at) VALUES (?1, ?2, ?3) \
         ON CONFLICT(track_id) DO UPDATE SET \
             cue_frames = excluded.cue_frames, updated_at = excluded.updated_at",
        rusqlite::params![track, frames, crate::writer::unix_now()],
    )?;
    Ok(())
}

impl Reader {
    /// Track `track`'s remembered cue point in frames at its native sample rate, if any.
    ///
    /// # Errors
    ///
    /// Any database error.
    pub fn cue(&self, track: i64) -> Result<Option<f64>, LibraryError> {
        Ok(self
            .connection()
            .prepare_cached("SELECT cue_frames FROM cues WHERE track_id = ?1")?
            .query_row([track], |r| r.get(0))
            .optional()?)
    }
}

impl Library {
    /// Track `track`'s remembered cue point, if any. See [`Reader::cue`].
    ///
    /// # Errors
    ///
    /// Any database error.
    pub fn cue(&self, track: i64) -> Result<Option<f64>, LibraryError> {
        self.reader()?.cue(track)
    }
}
