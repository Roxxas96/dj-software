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

/// Checks `slot` is 1-8 and `track` exists.
fn check_hotcue(tx: &Transaction<'_>, track: i64, slot: u8) -> Result<(), LibraryError> {
    if !(1..=8).contains(&slot) {
        return Err(LibraryError::InvalidHotcueSlot(slot));
    }
    let exists = tx
        .query_row("SELECT 1 FROM tracks WHERE id = ?1", [track], |_| Ok(()))
        .optional()?
        .is_some();
    if exists {
        Ok(())
    } else {
        Err(LibraryError::NoSuchTrack(track))
    }
}

/// Stores `frames` as hotcue `slot` of `track` (upsert keeping color and name) inside `tx`.
pub(crate) fn set_hotcue(
    tx: &Transaction<'_>,
    track: i64,
    slot: u8,
    frames: f64,
) -> Result<(), LibraryError> {
    if !(1..=8).contains(&slot) {
        return Err(LibraryError::InvalidHotcueSlot(slot));
    }
    if !frames.is_finite() || frames < 0.0 {
        return Err(LibraryError::InvalidCue(frames));
    }
    check_hotcue(tx, track, slot)?;
    tx.execute(
        "INSERT INTO hotcues(track_id, slot, frames) VALUES (?1, ?2, ?3) \
         ON CONFLICT(track_id, slot) DO UPDATE SET frames = excluded.frames",
        rusqlite::params![track, slot, frames],
    )?;
    Ok(())
}

/// Removes hotcue `slot` of `track` inside `tx`.
pub(crate) fn delete_hotcue(
    tx: &Transaction<'_>,
    track: i64,
    slot: u8,
) -> Result<(), LibraryError> {
    check_hotcue(tx, track, slot)?;
    tx.execute(
        "DELETE FROM hotcues WHERE track_id = ?1 AND slot = ?2",
        rusqlite::params![track, slot],
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

    /// Track `track`'s hotcues in frames at its native sample rate, indexed by slot minus 1.
    ///
    /// # Errors
    ///
    /// Any database error.
    pub fn hotcues(&self, track: i64) -> Result<[Option<f64>; 8], LibraryError> {
        let mut out = [None; 8];
        let mut stmt = self
            .connection()
            .prepare_cached("SELECT slot, frames FROM hotcues WHERE track_id = ?1")?;
        let rows = stmt.query_map([track], |r| Ok((r.get::<_, u8>(0)?, r.get::<_, f64>(1)?)))?;
        for row in rows {
            let (slot, frames) = row?;
            out[usize::from(slot) - 1] = Some(frames);
        }
        Ok(out)
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
