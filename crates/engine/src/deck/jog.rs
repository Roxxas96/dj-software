//! Jog wheel behavior of a deck: scratch, nudge, seek and search.
//!
//! All seconds are at the track's own rate and the original tempo.

use super::Deck;
use crate::JogSurface;

/// Track seconds per platter turn while scratching or moving a paused deck (33⅓ rpm).
pub(super) const PLATTER_SECONDS_PER_TURN: f64 = 1.8;
/// Track seconds a nudge turn shifts a playing deck by.
pub(super) const NUDGE_SECONDS_PER_TURN: f64 = 0.5;
/// Largest extra motion from a pending nudge per output frame, as a fraction of the deck's
/// normal speed.
pub(super) const NUDGE_MAX_BEND: f64 = 0.25;
/// Track seconds per turn of a search (SHIFT + jog).
pub(super) const SEARCH_SECONDS_PER_TURN: f64 = 10.0;
/// Time constant with which the playhead follows the platter while scratching.
pub(super) const SCRATCH_LAG_SECONDS: f64 = 0.01;
/// Fastest scratch, in track frames per output frame.
pub(super) const SCRATCH_MAX_SPEED: f64 = 8.0;

impl Deck {
    pub(super) fn set_vinyl(&mut self, on: bool) {
        self.vinyl = on;
        self.scratch = None;
        self.start_scratch_if_held();
    }

    pub(super) fn jog_touch(&mut self, touched: bool) {
        self.jog_touched = touched;
        self.scratch = None;
        self.start_scratch_if_held();
    }

    /// A held top in vinyl mode holds the record where the playhead is.
    fn start_scratch_if_held(&mut self) {
        if self.jog_touched && self.vinyl && self.track.is_some() {
            self.scratch = Some(0.0);
        }
    }

    pub(super) fn jog(&mut self, turns: f64, surface: JogSurface) {
        let Some(track_rate) = self.track.as_ref().map(|t| f64::from(t.sample_rate())) else {
            return;
        };
        let end = self.frames() as f64;
        match self.scratch {
            Some(pending) if surface == JogSurface::Top => {
                // The target stays on the record, so turning back from past an end
                // moves at once.
                let target =
                    self.position + pending + turns * PLATTER_SECONDS_PER_TURN * track_rate;
                self.scratch = Some(target.clamp(0.0, end) - self.position);
            }
            _ if self.playing => {
                self.nudge += turns * NUDGE_SECONDS_PER_TURN * track_rate;
            }
            _ => self.jump_by(turns * PLATTER_SECONDS_PER_TURN * track_rate),
        }
    }

    pub(super) fn jog_search(&mut self, turns: f64) {
        if let Some(track) = &self.track {
            self.jump_by(turns * SEARCH_SECONDS_PER_TURN * f64::from(track.sample_rate()));
        }
    }

    /// Moves the playhead by `frames`, clamped to the track, leaving a loop it lands
    /// outside of.
    fn jump_by(&mut self, frames: f64) {
        self.position = (self.position + frames).clamp(0.0, self.frames() as f64);
        self.exit_loop_if_outside(self.position);
    }
}
