//! Remembered cue points: one per track, written through immediately.

use std::path::Path;

use rusqlite::{OptionalExtension, Transaction};

use crate::{Library, LibraryError, Reader};

/// A hotcue slot's contents: its position, and a loop length when it is a saved loop.
#[derive(Debug, Clone, PartialEq)]
pub struct HotcuePoint {
    /// Position in frames at the track's native sample rate.
    pub frames: f64,
    /// The saved loop's length in frames, or `None` for a plain hotcue.
    pub loop_frames: Option<f64>,
    /// Colour as `0xRRGGBB`, if any.
    pub color: Option<u32>,
    pub name: Option<String>,
}

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

/// Checks a hotcue's position and loop length are storable.
fn check_point(frames: f64, loop_frames: Option<f64>) -> Result<(), LibraryError> {
    if !frames.is_finite() || frames < 0.0 {
        return Err(LibraryError::InvalidCue(frames));
    }
    match loop_frames {
        Some(len) if !len.is_finite() || len <= 0.0 => Err(LibraryError::InvalidLoop(len)),
        _ => Ok(()),
    }
}

/// Stores `frames` and `loop_frames` in hotcue `slot` of `track` (upsert keeping color and
/// name) inside `tx`.
pub(crate) fn set_hotcue(
    tx: &Transaction<'_>,
    track: i64,
    slot: u8,
    frames: f64,
    loop_frames: Option<f64>,
) -> Result<(), LibraryError> {
    if !(1..=8).contains(&slot) {
        return Err(LibraryError::InvalidHotcueSlot(slot));
    }
    check_point(frames, loop_frames)?;
    check_hotcue(tx, track, slot)?;
    tx.execute(
        "INSERT INTO hotcues(track_id, slot, frames, loop_frames) VALUES (?1, ?2, ?3, ?4) \
         ON CONFLICT(track_id, slot) DO UPDATE SET \
             frames = excluded.frames, loop_frames = excluded.loop_frames",
        rusqlite::params![track, slot, frames, loop_frames],
    )?;
    Ok(())
}

/// Replaces hotcue `slot` of `track` with `point` inside `tx`.
pub(crate) fn put_hotcue(
    tx: &Transaction<'_>,
    track: i64,
    slot: u8,
    point: &HotcuePoint,
) -> Result<(), LibraryError> {
    if !(1..=8).contains(&slot) {
        return Err(LibraryError::InvalidHotcueSlot(slot));
    }
    check_point(point.frames, point.loop_frames)?;
    check_hotcue(tx, track, slot)?;
    tx.execute(
        "INSERT OR REPLACE INTO hotcues(track_id, slot, frames, loop_frames, color, name) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params![
            track,
            slot,
            point.frames,
            point.loop_frames,
            point.color,
            point.name
        ],
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

    /// Track `track`'s hotcue slots in full (position, loop length, color and name),
    /// indexed by slot minus 1.
    ///
    /// # Errors
    ///
    /// Any database error.
    pub fn hotcue_points(&self, track: i64) -> Result<[Option<HotcuePoint>; 8], LibraryError> {
        let mut out = [const { None }; 8];
        let mut stmt = self.connection().prepare_cached(
            "SELECT slot, frames, loop_frames, color, name FROM hotcues WHERE track_id = ?1",
        )?;
        let rows = stmt.query_map([track], |r| {
            Ok((
                r.get::<_, u8>(0)?,
                HotcuePoint {
                    frames: r.get(1)?,
                    loop_frames: r.get(2)?,
                    color: r.get(3)?,
                    name: r.get(4)?,
                },
            ))
        })?;
        for row in rows {
            let (slot, point) = row?;
            out[usize::from(slot) - 1] = Some(point);
        }
        Ok(out)
    }

    /// The id of the track stored at exactly `path` (case-sensitive), if any.
    ///
    /// # Errors
    ///
    /// Any database error, or [`LibraryError::NonUtf8Path`].
    pub fn track_id_by_path(&self, path: &Path) -> Result<Option<i64>, LibraryError> {
        let text = path
            .to_str()
            .ok_or_else(|| LibraryError::NonUtf8Path(path.to_owned()))?;
        Ok(self
            .connection()
            .prepare_cached("SELECT id FROM tracks WHERE path = ?1")?
            .query_row([text], |r| r.get(0))
            .optional()?)
    }

    /// The ids of the tracks whose audio fingerprint is `fp`.
    ///
    /// # Errors
    ///
    /// Any database error.
    pub fn track_ids_by_fingerprint(&self, fp: &[u8; 32]) -> Result<Vec<i64>, LibraryError> {
        let mut stmt = self
            .connection()
            .prepare_cached("SELECT id FROM tracks WHERE fingerprint = ?1 ORDER BY id")?;
        let ids = stmt.query_map([fp.as_slice()], |r| r.get(0))?;
        Ok(ids.collect::<rusqlite::Result<_>>()?)
    }

    /// The ids of the tracks that are `size` bytes and whose file name (the last path
    /// component) is exactly `file_name`.
    ///
    /// # Errors
    ///
    /// Any database error.
    pub fn track_ids_by_name_and_size(
        &self,
        file_name: &str,
        size: u64,
    ) -> Result<Vec<i64>, LibraryError> {
        let Ok(size) = i64::try_from(size) else {
            return Ok(Vec::new());
        };
        let mut stmt = self
            .connection()
            .prepare_cached("SELECT id, path FROM tracks WHERE size = ?1 ORDER BY id")?;
        let rows = stmt.query_map([size], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
        })?;
        let mut ids = Vec::new();
        for row in rows {
            let (id, path) = row?;
            if Path::new(&path).file_name().is_some_and(|n| n == file_name) {
                ids.push(id);
            }
        }
        Ok(ids)
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
