//! Loop behavior of a deck.

use super::Deck;
use super::slip::SlipOwner;

/// Length in beats of an auto loop until it is halved or doubled.
pub(super) const DEFAULT_LOOP_BEATS: f64 = 4.0;
/// Shortest loop length in beats.
pub(super) const MIN_LOOP_BEATS: f64 = 1.0 / 32.0;
/// Longest loop length in beats.
pub(super) const MAX_LOOP_BEATS: f64 = 64.0;
/// Shortest loop in track frames, so wrapping never divides by zero.
const MIN_LOOP_FRAMES: f64 = 1.0;

impl Deck {
    pub(super) fn loop_in_press(&mut self) {
        if self.track.is_none() {
            return;
        }
        // Leaving a slipped loop returns first, so loop-in lands where the track is heard.
        self.exit_loop();
        self.loop_in = Some(self.position);
        self.loop_out = None;
    }

    pub(super) fn loop_out_press(&mut self) {
        if self.track.is_none() {
            return;
        }
        if let Some(start) = self.loop_in
            && self.position - start >= MIN_LOOP_FRAMES
        {
            if !self.loop_active {
                self.slip_begin(SlipOwner::Loop, false);
            }
            self.loop_out = Some(self.position);
            self.loop_active = true;
        }
    }

    pub(super) fn auto_loop(&mut self) {
        if self.loop_active {
            self.exit_loop();
            return;
        }
        let Some(grid) = self.grid else {
            return;
        };
        if self.track.is_none() {
            return;
        }
        self.slip_begin(SlipOwner::Loop, false);
        let beat = (grid.beats_at(self.position) + 1e-6).floor();
        let start = (grid.first_beat_frames + beat * grid.beat_period_frames).max(0.0);
        self.loop_in = Some(start);
        self.loop_out =
            Some(start + (self.loop_beats * grid.beat_period_frames).max(MIN_LOOP_FRAMES));
        self.loop_active = true;
    }

    /// Scales the loop length in beats by `factor` (clamped) and the length of an active loop
    /// by the same factor, grid or not. With a grid the active loop's length stays within
    /// [`MIN_LOOP_BEATS`, `MAX_LOOP_BEATS`] beats; without one it stops changing once the
    /// loop length in beats does.
    pub(super) fn loop_resize(&mut self, factor: f64) {
        let beats = self.loop_beats;
        self.loop_beats = (beats * factor).clamp(MIN_LOOP_BEATS, MAX_LOOP_BEATS);
        if !self.loop_active {
            return;
        }
        let (Some(start), Some(end)) = (self.loop_in, self.loop_out) else {
            return;
        };
        let length = end - start;
        let resized = length * factor;
        let resized = match self.grid {
            Some(grid) => {
                let period = grid.beat_period_frames;
                let clamped = resized.clamp(MIN_LOOP_BEATS * period, MAX_LOOP_BEATS * period);
                // A manual loop already past a limit stays as it is rather than jumping
                // the wrong way (a halve never grows a loop, a double never shrinks it).
                if factor < 1.0 {
                    clamped.min(length)
                } else {
                    clamped.max(length)
                }
            }
            None if self.loop_beats == beats => return,
            None => resized,
        };
        self.loop_out = Some(start + resized.max(MIN_LOOP_FRAMES));
    }

    pub(super) fn reloop_exit(&mut self) {
        if self.loop_active {
            self.exit_loop();
        } else if let (Some(start), Some(_)) = (self.loop_in, self.loop_out) {
            self.slip_begin(SlipOwner::Loop, false);
            self.position = start;
            self.loop_active = true;
        }
    }

    /// Deactivates the loop on request; a slipped loop returns to its ghost.
    fn exit_loop(&mut self) {
        if self.loop_active {
            self.loop_active = false;
            self.slip_loop_exited(false);
        }
    }

    /// A jump to `point` outside the active loop leaves the loop. The jump wins over a
    /// slipped loop: its ghost is dropped.
    pub(super) fn exit_loop_if_outside(&mut self, point: f64) {
        if let (true, Some(start), Some(end)) = (self.loop_active, self.loop_in, self.loop_out)
            && !(start..end).contains(&point)
        {
            self.loop_active = false;
            self.slip_loop_exited(true);
        }
    }

    /// The wrap bounds while a loop is active: loop-out is cut at the track end, so a loop
    /// overhanging the end wraps there.
    pub(super) fn active_loop(&self) -> Option<(f64, f64)> {
        match (self.loop_active, self.loop_in, self.loop_out) {
            (true, Some(start), Some(end)) => {
                let end = end.min(self.frames() as f64);
                (end > start).then_some((start, end))
            }
            _ => None,
        }
    }
}

/// Wraps a playhead that reached loop-out back into the loop. `step` is the playhead's
/// motion per frame; moving backwards never wraps.
pub(super) fn wrap_position(position: f64, bounds: Option<(f64, f64)>, step: f64) -> f64 {
    match bounds {
        Some((start, end)) if step > 0.0 && position >= end => {
            start + (position - start) % (end - start)
        }
        _ => position,
    }
}

#[cfg(test)]
mod tests {
    use super::wrap_position;

    #[test]
    fn wraps_forward_only() {
        let bounds = Some((100.0, 200.0));
        assert_eq!(wrap_position(250.0, bounds, 1.0), 150.0);
        assert_eq!(wrap_position(200.0, bounds, 1.0), 100.0);
        assert_eq!(wrap_position(250.0, bounds, 0.0), 250.0);
        assert_eq!(wrap_position(250.0, bounds, -1.0), 250.0);
        assert_eq!(wrap_position(150.0, None, 1.0), 150.0);
    }
}
