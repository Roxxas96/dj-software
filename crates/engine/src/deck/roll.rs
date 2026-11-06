//! Loop roll and slicer of a deck: a held pad loops a short *roll loop*, separate from the
//! normal loop, and always slips back to where the track would be on release.

use super::Deck;
use super::looping::MIN_LOOP_FRAMES;
use super::slip::SlipOwner;
use crate::Grid;

/// Slicer pads, and so beats in a slicer block.
pub(super) const SLICES: u8 = 8;
/// Beats past which a playhead counts as on the next beat, so a position a rounding error
/// short of a block boundary still lands in the new block.
const BEAT_EPSILON: f64 = 1e-6;

/// The pad holding a roll loop.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum RollPad {
    /// A loop roll pad of this length in beats.
    Roll(f64),
    /// A slicer pad, in the slicer block starting at `block` (in beats from the grid's
    /// first beat).
    Slice { slice: u8, block: f64 },
}

/// A held loop roll or slice.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Roll {
    pub(super) pad: RollPad,
    /// Roll loop start and end in track frames; may reach outside the track after a beat
    /// jump, and is cut to it when wrapping.
    pub(super) start: f64,
    pub(super) end: f64,
}

impl Deck {
    /// Starts a roll loop of `beats` beats at the playhead, replacing a held roll or slice.
    pub(super) fn roll_press(&mut self, beats: f64) {
        let Some(grid) = self.roll_grid() else {
            return;
        };
        let length = beats * grid.beat_period_frames;
        if !(length.is_finite() && length > 0.0) {
            return;
        }
        self.slip_begin(SlipOwner::Roll);
        self.roll = Some(Roll {
            pad: RollPad::Roll(beats),
            start: self.position,
            end: self.position + length.max(MIN_LOOP_FRAMES),
        });
    }

    /// Ends a roll started by a press of the same `beats` and returns to the ghost.
    pub(super) fn roll_release(&mut self, beats: f64) {
        if self.roll.is_some_and(|r| r.pad == RollPad::Roll(beats)) {
            self.roll_end(SlipOwner::Roll);
        }
    }

    /// Loops beat `slice` of the slicer block from its start. A held slice keeps its block;
    /// otherwise the block is the one where the track is.
    pub(super) fn slice_press(&mut self, slice: u8) {
        if slice >= SLICES {
            return;
        }
        let Some(grid) = self.roll_grid() else {
            return;
        };
        let Some(block) = self.slicer_block_beat() else {
            return;
        };
        let period = grid.beat_period_frames;
        let start = grid.first_beat_frames + (block + f64::from(slice)) * period;
        let end = start + period.max(MIN_LOOP_FRAMES);
        // A slice with nothing of the track in it would leave the playhead at an end.
        if start >= self.frames() as f64 || end <= 0.0 {
            return;
        }
        self.slip_begin(SlipOwner::Slice);
        self.roll = Some(Roll {
            pad: RollPad::Slice { slice, block },
            start,
            end,
        });
        self.position = start.max(0.0);
        self.pending_sync = None;
    }

    /// Ends the held slice if it is `slice` and returns to the ghost.
    pub(super) fn slice_release(&mut self, slice: u8) {
        if self
            .roll
            .is_some_and(|r| matches!(r.pad, RollPad::Slice { slice: s, .. } if s == slice))
        {
            self.roll_end(SlipOwner::Slice);
        }
    }

    fn roll_end(&mut self, owner: SlipOwner) {
        self.roll = None;
        self.slip_end(owner);
    }

    /// The grid when a pad may start a roll: a loaded deck with a grid, playing for real
    /// (a preview snaps back on release, so it does not count).
    fn roll_grid(&self) -> Option<Grid> {
        self.grid
            .filter(|_| self.track.is_some() && self.plays_for_real())
    }

    /// The first beat of the slicer block: the one held by a slice, else the 8-beat block
    /// of where the track is (the ghost while one runs, else the playhead).
    pub(super) fn slicer_block_beat(&self) -> Option<f64> {
        self.grid?;
        match self.roll {
            Some(Roll {
                pad: RollPad::Slice { block, .. },
                ..
            }) => Some(block),
            _ => self.slicer_block(),
        }
    }

    /// The 8-beat block of where the track is, counted from the grid's first beat.
    fn slicer_block(&self) -> Option<f64> {
        let grid = self.grid?;
        self.track.as_ref()?;
        let at = self.ghost_frames().unwrap_or(self.position);
        let slices = f64::from(SLICES);
        let block = ((grid.beats_at(at) + BEAT_EPSILON) / slices).floor() * slices;
        block.is_finite().then_some(block)
    }

    /// The roll loop's wrap bounds while a roll or slice is held, cut to the track.
    pub(super) fn roll_bounds(&self) -> Option<(f64, f64)> {
        let roll = self.roll?;
        let (start, end) = (roll.start.max(0.0), roll.end.min(self.frames() as f64));
        (end > start).then_some((start, end))
    }
}
