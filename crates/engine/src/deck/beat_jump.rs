//! Beat jump of a deck: the playhead moves by whole beats of the grid.

use super::Deck;

impl Deck {
    /// Moves the playhead by `beats` beats of the grid (negative goes back), clamped to the
    /// track. Needs a grid and a finite `beats`. An active normal loop moves with the
    /// playhead as long as it stays inside the track, else it is exited and the playhead
    /// jumps alone. A running slip ghost moves by the same amount (the beat jump is the one
    /// deliberate jump that does not drop it). A pending phase sync is dropped.
    pub(super) fn beat_jump(&mut self, beats: f64) {
        let Some(grid) = self.grid else {
            return;
        };
        let delta = beats * grid.beat_period_frames;
        if self.track.is_none() || !delta.is_finite() {
            return;
        }
        let end = self.frames() as f64;
        // The ghost's place in its loop is read before the loop moves or exits.
        let ghost = self.ghost_frames();
        let target = (self.position + delta).clamp(0.0, end);
        self.shift_loop_like(delta, target);
        self.position = target;
        if let (Some(ghost), Some(slot)) = (ghost, self.ghost.as_mut()) {
            slot.position = (ghost + delta).clamp(0.0, end);
        }
        self.pending_sync = None;
    }

    /// Moves every loop-like position by `delta` frames with the playhead jumping to
    /// `target`: today the normal loop. A loop that would leave the track is exited, and
    /// one that does not contain `target` after the move is exited too.
    // Task 3 moves the held roll's loop here as well, so a beat jump never cancels a pad.
    fn shift_loop_like(&mut self, delta: f64, target: f64) {
        if !self.loop_active {
            return;
        }
        let (Some(start), Some(end)) = (self.loop_in, self.loop_out) else {
            return;
        };
        let (start, end) = (start + delta, end + delta);
        if start >= 0.0 && end <= self.frames() as f64 && (start..end).contains(&target) {
            self.loop_in = Some(start);
            self.loop_out = Some(end);
        } else {
            self.loop_active = false;
            self.slip_loop_exited(true);
        }
    }
}
