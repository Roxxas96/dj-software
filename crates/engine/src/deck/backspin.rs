//! Backspin of a deck: the playhead runs backwards from three times the normal speed and
//! slows linearly to a stop, then normal playback resumes forward from there.

use super::{Deck, Source, add_frame};
use crate::dsp::smooth::Segment;

/// Speed at the start of a spin, backwards, as a multiple of the deck's normal speed.
const START_SPEED: f64 = 3.0;
const MIN_SECONDS: f64 = 0.2;
const MAX_SECONDS: f64 = 10.0;
/// Jog touches and platter or top moves this close to a spin's start belong to it (the
/// SB3 sends its own touch and backward nudge with the pad, just before or after it): they
/// neither block nor cancel the spin.
const GRACE_SECONDS: f64 = 0.05;

/// A running backspin.
#[derive(Debug, Clone, Copy)]
pub(super) struct Backspin {
    /// Output frames played so far.
    elapsed: f64,
    seconds: f64,
    /// The deck's clock when the spin started.
    started_at: u64,
}

impl Deck {
    pub(super) fn backspin_start(&mut self, seconds: f64) {
        if self.track.is_none() || !self.playing {
            return;
        }
        // A held record is the platter's, not the spin's, unless the hold began within the
        // grace window: then it was the spin's own touch, and the spin takes over.
        if self.scratch.is_some() {
            let grace = self.grace_frames();
            if !self
                .touched_at
                .is_some_and(|t| self.clock.wrapping_sub(t) <= grace)
            {
                return;
            }
            self.scratch = None;
        }
        // A deliberate jump: drops the ghost, ends a roll or slice and any earlier spin.
        self.slip_drop();
        self.bend = 0.0;
        self.backspin = Some(Backspin {
            elapsed: 0.0,
            seconds: seconds.clamp(MIN_SECONDS, MAX_SECONDS),
            started_at: self.clock,
        });
    }

    fn grace_frames(&self) -> u64 {
        (GRACE_SECONDS * f64::from(self.output_rate)) as u64
    }

    /// A spin is running and started within the grace window: jog touches and moves are
    /// the spin's own and leave it alone.
    pub(super) fn backspin_in_grace(&self) -> bool {
        self.backspin
            .is_some_and(|spin| self.clock.wrapping_sub(spin.started_at) <= self.grace_frames())
    }

    /// Plays the spin into `frames` and returns how many it filled; fewer than `frames.len()`
    /// when the spin ended. `rate` is the deck's normal speed in track frames per output
    /// frame, and `gains` the stem gains across the block, one read per frame.
    pub(super) fn render_backspin(
        &mut self,
        frames: &mut [[f32; 2]],
        rate: f64,
        output_rate: u32,
        gains: Option<[Segment; crate::stems::PARTS]>,
    ) -> usize {
        let Some(track) = self.track.as_deref() else {
            self.backspin = None;
            return 0;
        };
        let Some(mut spin) = self.backspin else {
            return 0;
        };
        let source = Source::new(track, self.stems.as_deref(), gains);
        let total = spin.seconds * f64::from(output_rate);
        let mut done = 0;
        for frame in frames.iter_mut() {
            if spin.elapsed >= total || self.position <= 0.0 {
                break;
            }
            let speed = START_SPEED * rate * (1.0 - spin.elapsed / total);
            if speed > 0.0 && track.frames() > 0 {
                add_frame(frame, source, self.position, done);
            }
            self.position = (self.position - speed).max(0.0);
            spin.elapsed += 1.0;
            done += 1;
        }
        if spin.elapsed >= total || self.position <= 0.0 {
            self.backspin = None;
            // A nudge made during the spin must not bend the playback that follows it.
            self.bend = 0.0;
            self.exit_loop_if_outside(self.position);
        } else {
            self.backspin = Some(spin);
        }
        done
    }
}
