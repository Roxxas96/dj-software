//! Jog wheel behavior of a deck: scratch, nudge, seek and search.
//!
//! All seconds are at the track's own rate and the original tempo.

use super::Deck;
use super::slip::SlipOwner;
use crate::JogSurface;

/// Track seconds per platter turn while scratching or moving a paused deck (33⅓ rpm).
pub(super) const PLATTER_SECONDS_PER_TURN: f64 = 1.8;
/// Speed bend a nudge turn adds to a playing deck, as a fraction of its normal speed.
pub(super) const NUDGE_RATE_PER_TURN: f64 = 2.0;
/// Largest speed bend, up or down, as a fraction of the deck's normal speed. Keeps the speed
/// at 0.5 times normal or more, so a nudge never reverses the deck.
pub(super) const NUDGE_MAX_BEND: f64 = 0.5;
/// Time constant with which the speed bend fades back to normal speed.
pub(super) const NUDGE_DECAY_SECONDS: f64 = 0.1;
/// Track seconds per turn of a search (SHIFT + jog).
pub(super) const SEARCH_SECONDS_PER_TURN: f64 = 10.0;
/// Time constant with which the playhead follows the platter while scratching.
pub(super) const SCRATCH_LAG_SECONDS: f64 = 0.01;
/// Fastest scratch, in track frames per output frame.
pub(super) const SCRATCH_MAX_SPEED: f64 = 8.0;
/// Scratch speed, in track frames per output frame, below which the record counts as held
/// still and outputs silence rather than a constant sample (DC).
pub(super) const SCRATCH_STILL_SPEED: f64 = 1e-3;

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

    /// A held top in vinyl mode holds the record where the playhead is. With slip on, a
    /// scratch of a deck playing for real lasts as long as the hold and then returns to
    /// where the track would be; a paused deck is positioned as without slip.
    fn start_scratch_if_held(&mut self) {
        if self.jog_touched && self.vinyl && self.track.is_some() {
            self.scratch = Some(0.0);
            if self.plays_for_real() {
                self.slip_begin(SlipOwner::Scratch, false);
            }
        } else {
            self.slip_end(SlipOwner::Scratch);
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
            // The hand is on the platter: a ring move neither nudges nor seeks.
            Some(_) => {}
            _ if self.playing => {
                // Clamped, so huge moves stay finite and never stall or reverse the deck.
                self.bend = (self.bend + turns * NUDGE_RATE_PER_TURN)
                    .clamp(-NUDGE_MAX_BEND, NUDGE_MAX_BEND);
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
    /// outside of. A deliberate jump: it drops a running slip ghost.
    fn jump_by(&mut self, frames: f64) {
        self.slip_drop();
        self.position = (self.position + frames).clamp(0.0, self.frames() as f64);
        self.exit_loop_if_outside(self.position);
    }
}
