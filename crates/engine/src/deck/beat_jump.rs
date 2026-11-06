//! Beat jump of a deck: the playhead moves by whole beats of the grid.

use super::Deck;

impl Deck {
    /// Moves the playhead by `beats` beats of the grid (negative goes back), clamped to the
    /// track. Needs a grid and a finite `beats`. An active normal loop moves with the
    /// playhead as long as it stays inside the track, else it is exited and the playhead
    /// jumps alone. A running slip ghost and a held roll's loop move by the same amount (the
    /// beat jump is the one deliberate jump that drops neither). A pending phase sync is
    /// dropped.
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
        let ghost_target = ghost.map(|g| (g + delta).clamp(0.0, end));
        // During a roll the normal loop holds the ghost, not the playhead.
        let loop_point = match ghost_target {
            Some(g) if self.roll.is_some() => g,
            _ => target,
        };
        self.shift_loop_like(delta, loop_point);
        self.position = target;
        if let (Some(ghost), Some(slot)) = (ghost_target, self.ghost.as_mut()) {
            slot.position = ghost;
        }
        self.pending_sync = None;
    }

    /// Moves every loop-like position by `delta` frames: the held roll's loop, which always
    /// moves (a beat jump never cancels a held pad; the part outside the track is cut when
    /// wrapping), and the normal loop, with what it holds jumping to `target`. A normal loop
    /// that would leave the track is exited, and one that does not contain `target` after
    /// the move is exited too.
    fn shift_loop_like(&mut self, delta: f64, target: f64) {
        if let Some(roll) = &mut self.roll {
            roll.start += delta;
            roll.end += delta;
        }
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
