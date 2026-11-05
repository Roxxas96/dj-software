//! Beatgrid editing: manual corrections to a track's BPM and first beat.
//!
//! Each edit is one [`WriteOp::EditGrid`] that reads the current grid inside its
//! transaction and writes the new one, so an analysis job storing its result at the same
//! time cannot undo it: every edit except [`GridEdit::ToggleLock`] locks the grid, and
//! [`WriteOp::StoreAnalysis`] keeps a locked grid.
//!
//! Tap tempo needs the previous taps, which live in memory in the [`Library`] (per track),
//! never in the database: [`Library::edit_grid`] turns a tap into a BPM before it reaches
//! the writer.

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};

use rusqlite::{OptionalExtension, Transaction};

use crate::{Library, LibraryError, Reader, WriteOp, WriteOutcome};

/// The lowest BPM a grid can be set to.
pub const MIN_GRID_BPM: f64 = 40.0;
/// The highest BPM a grid can be set to.
pub const MAX_GRID_BPM: f64 = 300.0;
/// Taps further apart than this start a new tap sequence.
pub const TAP_RESET_MICROS: u64 = 2_000_000;
/// The fewest taps that give a BPM.
pub const MIN_TAPS: usize = 4;
/// How many of the latest taps of a sequence the BPM is averaged over.
pub const TAP_WINDOW: usize = 8;

/// A manual change to a track's beatgrid. Positions are in frames at the track's native
/// sample rate.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GridEdit {
    /// Moves every beat by `frames` (negative moves earlier).
    Shift { frames: f64 },
    /// Moves the grid so a beat falls exactly at `at_frames`, keeping the BPM.
    SetDownbeat { at_frames: f64 },
    /// Halves the BPM, keeping the beat nearest the playhead (or the first beat).
    Halve,
    /// Doubles the BPM, keeping the beat nearest the playhead (or the first beat).
    Double,
    /// Sets the BPM ([`MIN_GRID_BPM`] to [`MAX_GRID_BPM`]), keeping the first beat.
    SetBpm(f64),
    /// A tap-tempo tap at `at_micros` on a monotonic clock. From [`MIN_TAPS`] taps of a
    /// sequence on, the BPM is set from their mean interval.
    Tap { at_micros: u64 },
    /// Locks or unlocks the grid (a locked grid survives re-analysis).
    ToggleLock,
}

/// Why a grid edit was refused. Nothing is changed.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum GridError {
    #[error("the track has no BPM yet: analyze or set a BPM first")]
    NoBpm,
    #[error("BPM {0} is out of range: a grid takes 40 to 300 BPM")]
    BpmOutOfRange(f64),
    #[error("the track's sample rate is unknown: rescan it before editing its grid")]
    NoSampleRate,
    #[error("{0} is not a valid grid position or offset")]
    NotFinite(f64),
}

/// A track's beatgrid: the grid columns of its analysis row.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Beatgrid {
    pub bpm: Option<f64>,
    /// In frames at the track's native sample rate.
    pub first_beat_frames: Option<f64>,
    /// Re-analysis keeps a locked grid.
    pub grid_locked: bool,
}

/// Applies `edit` to track `track`'s grid inside `tx`. A [`GridEdit::Tap`] here only makes
/// sure the analysis row exists ([`Library::edit_grid`] has already turned a tap that
/// gives a BPM into [`GridEdit::SetBpm`]). Beat periods use `decoded_rate` (the decoded
/// track's rate) when given, else the rate from the track's tags.
pub(crate) fn apply(
    tx: &Transaction<'_>,
    track: i64,
    edit: GridEdit,
    playhead_frames: Option<f64>,
    decoded_rate: Option<u32>,
) -> Result<WriteOutcome, LibraryError> {
    let row: Option<(Option<u32>, Option<Beatgrid>)> = tx
        .query_row(
            "SELECT t.sample_rate, a.track_id IS NOT NULL, a.bpm, a.first_beat_frames, \
                    coalesce(a.grid_locked, 0) \
             FROM tracks t LEFT JOIN analysis a ON a.track_id = t.id WHERE t.id = ?1",
            [track],
            |r| {
                let grid = r.get::<_, bool>(1)?.then_some(Beatgrid {
                    bpm: r.get(2)?,
                    first_beat_frames: r.get(3)?,
                    grid_locked: r.get(4)?,
                });
                Ok((r.get(0)?, grid))
            },
        )
        .optional()?;
    let Some((tag_rate, current)) = row else {
        return Err(LibraryError::NoSuchTrack(track));
    };
    let exists = current.is_some();
    let current = current.unwrap_or(Beatgrid {
        bpm: None,
        first_beat_frames: None,
        grid_locked: false,
    });
    let sample_rate = decoded_rate.filter(|&r| r > 0).or(tag_rate);
    let new = edited(current, sample_rate, edit, playhead_frames)?;
    if exists && new == current {
        return Ok(WriteOutcome::Grid(new));
    }
    // A new row has never been analyzed: analyzer version 0, so analysis still runs (and
    // keeps the grid if it is locked).
    tx.execute(
        "INSERT INTO analysis(track_id, analyzer_version, bpm, first_beat_frames, grid_locked) \
         VALUES (?1, 0, ?2, ?3, ?4) \
         ON CONFLICT(track_id) DO UPDATE SET \
             bpm = excluded.bpm, \
             first_beat_frames = excluded.first_beat_frames, \
             grid_locked = excluded.grid_locked",
        rusqlite::params![track, new.bpm, new.first_beat_frames, new.grid_locked],
    )?;
    Ok(WriteOutcome::Grid(new))
}

/// The grid after `edit`.
fn edited(
    current: Beatgrid,
    sample_rate: Option<u32>,
    edit: GridEdit,
    playhead_frames: Option<f64>,
) -> Result<Beatgrid, GridError> {
    let period = |bpm: f64| {
        sample_rate
            .filter(|&sr| sr > 0)
            .map(|sr| 60.0 * f64::from(sr) / bpm)
            .ok_or(GridError::NoSampleRate)
    };
    let first = current.first_beat_frames.unwrap_or(0.0);
    let locked = |bpm, first_beat_frames| Beatgrid {
        bpm: Some(bpm),
        first_beat_frames: Some(first_beat_frames),
        grid_locked: true,
    };
    match edit {
        GridEdit::ToggleLock => Ok(Beatgrid {
            grid_locked: !current.grid_locked,
            ..current
        }),
        GridEdit::Tap { .. } => Ok(current),
        GridEdit::Shift { frames } => {
            let bpm = current.bpm.ok_or(GridError::NoBpm)?;
            let frames = finite(frames)?;
            Ok(locked(bpm, wrap(first + frames, period(bpm)?)))
        }
        GridEdit::SetDownbeat { at_frames } => {
            let bpm = current.bpm.ok_or(GridError::NoBpm)?;
            let at = finite(at_frames)?;
            Ok(locked(bpm, wrap(at, period(bpm)?)))
        }
        GridEdit::Halve | GridEdit::Double => {
            let bpm = current.bpm.ok_or(GridError::NoBpm)?;
            let new_bpm = in_range(if edit == GridEdit::Halve {
                bpm / 2.0
            } else {
                bpm * 2.0
            })?;
            let anchor = match playhead_frames {
                Some(p) => {
                    let p = finite(p)?;
                    let old = period(bpm)?;
                    first + ((p - first) / old).round() * old
                }
                None => first,
            };
            Ok(locked(new_bpm, wrap(anchor, period(new_bpm)?)))
        }
        GridEdit::SetBpm(bpm) => {
            let bpm = in_range(bpm)?;
            let first = match current.first_beat_frames {
                Some(f) => wrap(f, period(bpm)?),
                None => 0.0,
            };
            Ok(locked(bpm, first))
        }
    }
}

fn finite(v: f64) -> Result<f64, GridError> {
    if v.is_finite() {
        Ok(v)
    } else {
        Err(GridError::NotFinite(v))
    }
}

fn in_range(bpm: f64) -> Result<f64, GridError> {
    if (MIN_GRID_BPM..=MAX_GRID_BPM).contains(&bpm) {
        Ok(bpm)
    } else {
        Err(GridError::BpmOutOfRange(bpm))
    }
}

/// `x` wrapped into `[0, period)`.
fn wrap(x: f64, period: f64) -> f64 {
    let w = x.rem_euclid(period);
    // Rounding can land exactly on `period` for tiny negative `x`.
    if w >= period { 0.0 } else { w }
}

/// Recent taps per track, for tap tempo.
#[derive(Debug, Default)]
pub(crate) struct Taps(Mutex<HashMap<i64, Vec<u64>>>);

impl Taps {
    /// Records a tap on `track` at `at_micros` and returns the BPM once the sequence has
    /// [`MIN_TAPS`] taps: 60 / the mean interval, rounded to 0.01. A tap more than
    /// [`TAP_RESET_MICROS`] after the previous one (or before it) starts a new sequence.
    fn tap(&self, track: i64, at_micros: u64) -> Option<f64> {
        let mut all = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        // Forget sequences that have ended, so the map stays small.
        all.retain(|_, taps| {
            taps.last()
                .is_some_and(|&last| at_micros.saturating_sub(last) <= TAP_RESET_MICROS)
        });
        let taps = all.entry(track).or_default();
        if taps
            .last()
            .is_some_and(|&last| at_micros < last || at_micros - last > TAP_RESET_MICROS)
        {
            taps.clear();
        }
        taps.push(at_micros);
        if taps.len() > TAP_WINDOW {
            taps.remove(0);
        }
        let (&first, &last) = (taps.first()?, taps.last()?);
        if taps.len() < MIN_TAPS || last == first {
            return None;
        }
        #[expect(
            clippy::cast_precision_loss,
            reason = "tap spans are far below 2^52 µs"
        )]
        let mean = (last - first) as f64 / (taps.len() - 1) as f64;
        Some((60_000_000.0 / mean * 100.0).round() / 100.0)
    }
}

impl Reader {
    /// Track `track`'s beatgrid, or `None` if it has no analysis row (not analyzed and
    /// never edited). Reads only the grid columns.
    ///
    /// # Errors
    ///
    /// Any database error.
    pub fn beatgrid(&self, track: i64) -> Result<Option<Beatgrid>, LibraryError> {
        Ok(self
            .connection()
            .prepare_cached(
                "SELECT bpm, first_beat_frames, grid_locked FROM analysis WHERE track_id = ?1",
            )?
            .query_row([track], |r| {
                Ok(Beatgrid {
                    bpm: r.get(0)?,
                    first_beat_frames: r.get(1)?,
                    grid_locked: r.get(2)?,
                })
            })
            .optional()?)
    }
}

impl Library {
    /// Applies `edit` to track `track`'s beatgrid and returns the grid as committed.
    /// `playhead_frames` is where the track is playing, for [`GridEdit::Halve`] and
    /// [`GridEdit::Double`].
    ///
    /// A track that has not been analyzed gets an analysis row with no waveform or key.
    /// Every edit except [`GridEdit::ToggleLock`] locks the grid; a tap that does not give
    /// a BPM yet (fewer than [`MIN_TAPS`] in its sequence) leaves the grid as it is,
    /// unlocked included.
    ///
    /// # Errors
    ///
    /// [`LibraryError::Grid`] when the edit is refused (no BPM to shift or halve, a BPM out
    /// of range, an unknown sample rate), [`LibraryError::NoSuchTrack`], or any database
    /// error. A refused edit changes nothing.
    pub fn edit_grid(
        &self,
        track: i64,
        edit: GridEdit,
        playhead_frames: Option<f64>,
    ) -> Result<Beatgrid, LibraryError> {
        self.edit_grid_at_rate(track, edit, playhead_frames, None)
    }

    /// [`Library::edit_grid`] with the decoded track's `sample_rate`, used for beat
    /// periods instead of the rate from the tags (which is the fallback when `None`).
    ///
    /// # Errors
    ///
    /// As [`Library::edit_grid`]; the sample rate is unknown only when neither is known.
    pub fn edit_grid_at_rate(
        &self,
        track: i64,
        edit: GridEdit,
        playhead_frames: Option<f64>,
        sample_rate: Option<u32>,
    ) -> Result<Beatgrid, LibraryError> {
        let edit = match edit {
            GridEdit::Tap { at_micros } => self
                .taps
                .tap(track, at_micros)
                .map_or(edit, GridEdit::SetBpm),
            other => other,
        };
        match self.writer().submit_wait(WriteOp::EditGrid {
            track,
            edit,
            playhead_frames,
            sample_rate,
        })? {
            WriteOutcome::Grid(grid) => Ok(grid),
            // `EditGrid` always reports its grid.
            other => unreachable!("EditGrid reported {other:?}"),
        }
    }
}
