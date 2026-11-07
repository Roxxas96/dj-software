//! Backspin of a deck: the playhead runs backwards from three times the normal speed and
//! slows linearly to a stop, then normal playback resumes forward from there.

use super::{Deck, add_frame};

/// Speed at the start of a spin, backwards, as a multiple of the deck's normal speed.
const START_SPEED: f64 = 3.0;
const MIN_SECONDS: f64 = 0.2;
const MAX_SECONDS: f64 = 10.0;

/// A running backspin.
#[derive(Debug, Clone, Copy)]
pub(super) struct Backspin {
    /// Output frames played so far.
    elapsed: f64,
    seconds: f64,
}

impl Deck {
    pub(super) fn backspin_start(&mut self, seconds: f64) {
        // A held record is the platter's, not the spin's.
        if self.track.is_none() || !self.playing || self.scratch.is_some() {
            return;
        }
        // A deliberate jump: drops the ghost, ends a roll or slice and any earlier spin.
        self.slip_drop();
        self.bend = 0.0;
        self.backspin = Some(Backspin {
            elapsed: 0.0,
            seconds: seconds.clamp(MIN_SECONDS, MAX_SECONDS),
        });
    }

    /// Plays the spin into `frames` and returns how many it filled; fewer than `frames.len()`
    /// when the spin ended. `rate` is the deck's normal speed in track frames per output
    /// frame.
    pub(super) fn render_backspin(
        &mut self,
        frames: &mut [[f32; 2]],
        rate: f64,
        output_rate: u32,
    ) -> usize {
        let Some(track) = self.track.as_deref() else {
            self.backspin = None;
            return 0;
        };
        let Some(mut spin) = self.backspin else {
            return 0;
        };
        let total = spin.seconds * f64::from(output_rate);
        let mut done = 0;
        for frame in frames.iter_mut() {
            if spin.elapsed >= total || self.position <= 0.0 {
                break;
            }
            let speed = START_SPEED * rate * (1.0 - spin.elapsed / total);
            if speed > 0.0 && track.frames() > 0 {
                add_frame(frame, track, self.position);
            }
            self.position = (self.position - speed).max(0.0);
            spin.elapsed += 1.0;
            done += 1;
        }
        if spin.elapsed >= total || self.position <= 0.0 {
            self.backspin = None;
            self.exit_loop_if_outside(self.position);
        } else {
            self.backspin = Some(spin);
        }
        done
    }
}
