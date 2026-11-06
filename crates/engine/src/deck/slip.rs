//! Slip mode of a deck: a ghost playhead keeps moving at normal speed during a scratch,
//! loop or held hotcue, and the playhead returns to it when the action ends.

use super::{Deck, looping};
use crate::HotcueSlot;

/// The silent playhead that keeps going where the track would be.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Ghost {
    /// In track frames.
    pub(super) position: f64,
    /// The normal loop the ghost wraps in: the one active when it began. Returning to the
    /// ghost puts that loop back.
    pub(super) bounds: Option<(f64, f64)>,
    /// What began the ghost; only its end returns to the ghost.
    pub(super) owner: SlipOwner,
}

/// The action a ghost belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SlipOwner {
    /// The jog wheel's top is held in vinyl mode.
    Scratch,
    /// A normal loop is active.
    Loop,
    /// A hotcue pad is held after a jump.
    Hotcue(HotcueSlot),
    /// A held pad that always slips (loop roll, slicer); ended by [`Deck::slip_end`].
    Pad,
}

impl Deck {
    pub(super) fn set_slip(&mut self, on: bool) {
        self.slip = on;
        if !on {
            self.ghost = None;
        }
    }

    /// Starts the ghost at the playhead when slip is on or `forced`, unless a ghost is
    /// already running. The ghost wraps in the normal loop active now, if any.
    pub(super) fn slip_begin(&mut self, forced: bool) {
        if self.ghost.is_some() || !(self.slip || forced) || self.track.is_none() {
            return;
        }
        self.ghost = Some(Ghost {
            position: self.position,
            bounds: self.active_loop(),
            owner: SlipOwner::Pad,
        });
    }

    /// Moves the playhead to the ghost and drops the ghost. A ghost that wrapped in a loop
    /// returns into that loop, active; otherwise a loop the return lands outside of exits.
    pub(super) fn slip_end(&mut self) {
        let Some(ghost) = self.ghost.take() else {
            return;
        };
        self.position = ghost.position;
        match ghost.bounds {
            Some(bounds) if self.active_loop() != Some(bounds) => {
                (self.loop_in, self.loop_out) = (Some(bounds.0), Some(bounds.1));
                self.loop_active = true;
            }
            Some(_) => {}
            None => self.exit_loop_if_outside(self.position),
        }
    }

    /// Begins a ghost for `owner` (slip on only). A hotcue takes over a ghost of a loop or
    /// of another hotcue, so its release returns where the track would be.
    pub(super) fn slip_begin_for(&mut self, owner: SlipOwner) {
        if !self.slip {
            return;
        }
        if let Some(ghost) = &mut self.ghost {
            if matches!(owner, SlipOwner::Hotcue(_))
                && matches!(ghost.owner, SlipOwner::Loop | SlipOwner::Hotcue(_))
            {
                ghost.owner = owner;
            }
            return;
        }
        self.slip_begin(false);
        if let Some(ghost) = &mut self.ghost {
            ghost.owner = owner;
        }
    }

    /// Ends the ghost if `owner` began it.
    pub(super) fn slip_end_for(&mut self, owner: SlipOwner) {
        if self.ghost.is_some_and(|g| g.owner == owner) {
            self.slip_end();
        }
    }

    /// A jump left the active loop: the loop's own ghost is dropped, so the jump stands.
    pub(super) fn slip_loop_jumped_out(&mut self) {
        if self.ghost.is_some_and(|g| g.owner == SlipOwner::Loop) {
            self.ghost = None;
        }
    }

    /// The active loop changed from `before` to its current bounds: a ghost wrapping in it
    /// follows.
    pub(super) fn slip_loop_resized(&mut self, before: Option<(f64, f64)>) {
        let after = self.active_loop();
        if let Some(ghost) = &mut self.ghost
            && before.is_some()
            && ghost.bounds == before
        {
            ghost.bounds = after;
        }
    }

    /// The ghost's position wrapped into its loop, as published.
    pub(super) fn ghost_frames(&self) -> Option<f64> {
        self.ghost
            .map(|g| looping::wrap_position(g.position, g.bounds, 1.0))
    }
}

/// Moves `ghost` by `step` track frames, wrapping in its loop like the playhead and
/// holding at the track's `end`.
pub(super) fn advance(ghost: &mut Ghost, step: f64, end: f64) {
    ghost.position = (looping::wrap_position(ghost.position, ghost.bounds, step) + step).min(end);
}
