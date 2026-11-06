//! Slip mode of a deck: a ghost playhead keeps moving at normal speed during a scratch,
//! loop, held hotcue, roll or slice, and the playhead returns to it when the action ends.

use super::{Deck, looping};
use crate::HotcueSlot;

/// The silent playhead that keeps going where the track would be.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Ghost {
    /// In track frames.
    pub(super) position: f64,
    /// The action the ghost belongs to; only its end returns to the ghost.
    pub(super) owner: SlipOwner,
    /// The owner a roll or slice took the ghost over from, if it was not a roll or slice.
    pub(super) prior: Option<SlipOwner>,
    /// The ghost wraps in the active normal loop: one was active when it began and has
    /// not been exited since. A loop exited during the slip is gone for the ghost too.
    pub(super) wraps: bool,
}

/// The action a ghost belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SlipOwner {
    /// The jog wheel's top is held in vinyl mode on a playing deck.
    Scratch,
    /// A normal loop is active.
    Loop,
    /// A hotcue pad is held after a jump.
    Hotcue(HotcueSlot),
    /// A loop roll pad is held; always slips.
    Roll,
    /// A slicer pad is held; always slips.
    Slice,
}

impl SlipOwner {
    /// A roll or slice: slips whether slip mode is on or not.
    fn forced(self) -> bool {
        matches!(self, Self::Roll | Self::Slice)
    }
}

impl Deck {
    /// Turning slip off drops a running ghost, except a held roll's or slice's.
    pub(super) fn set_slip(&mut self, on: bool) {
        self.slip = on;
        if !on && self.ghost.is_some_and(|g| !g.owner.forced()) {
            self.ghost = None;
        }
    }

    /// Starts a ghost for `owner` at the playhead when slip is on or `owner` always slips
    /// (a roll or slice), and returns whether `owner` now holds a ghost. One ghost runs at
    /// a time: while one runs, a roll or slice takes it over as it is (it still is where the
    /// track would be) and any other begin does nothing.
    pub(super) fn slip_begin(&mut self, owner: SlipOwner) -> bool {
        let forced = owner.forced();
        if let Some(ghost) = &mut self.ghost {
            if forced {
                if !ghost.owner.forced() {
                    ghost.prior = Some(ghost.owner);
                }
                ghost.owner = owner;
            }
            return forced;
        }
        if !(self.slip || forced) || self.track.is_none() {
            return false;
        }
        self.ghost = Some(Ghost {
            position: self.position,
            owner,
            prior: None,
            wraps: self.active_loop().is_some(),
        });
        true
    }

    /// If `owner` holds the ghost, moves the playhead to it and drops it. Loops are left as
    /// they are: a ghost that wraps already lives in the active loop. A roll or slice that
    /// took the ghost over from a slipped loop that is still active hands it back instead,
    /// and the playhead lands in the loop where the looping playhead would be.
    pub(super) fn slip_end(&mut self, owner: SlipOwner) {
        let Some(ghost) = self.ghost.filter(|g| g.owner == owner) else {
            return;
        };
        let Some(position) = self.ghost_frames() else {
            return;
        };
        if ghost.prior == Some(SlipOwner::Loop) && self.loop_active {
            self.position = looping::wrap_position(position, self.active_loop(), 1.0);
            self.ghost = Some(Ghost {
                owner: SlipOwner::Loop,
                prior: None,
                ..ghost
            });
        } else {
            self.position = position;
            self.ghost = None;
        }
    }

    /// A deliberate jump wins over any slip: the ghost is dropped, and a held roll or slice
    /// ends with it (its loop would hold the playhead away from the jump).
    pub(super) fn slip_drop(&mut self) {
        self.ghost = None;
        self.roll = None;
    }

    /// The normal loop was deactivated, by request or by a jump outside it. The ghost stops
    /// wrapping; a slipped loop's ghost returns on request and is dropped after a jump.
    pub(super) fn slip_loop_exited(&mut self, by_jump: bool) {
        let Some(ghost) = &mut self.ghost else {
            return;
        };
        ghost.wraps = false;
        if ghost.owner == SlipOwner::Loop {
            if by_jump {
                self.ghost = None;
            } else {
                self.slip_end(SlipOwner::Loop);
            }
        }
    }

    /// The loop the ghost wraps in now, if any.
    pub(super) fn ghost_bounds(&self) -> Option<(f64, f64)> {
        self.ghost
            .filter(|g| g.wraps)
            .and_then(|_| self.active_loop())
    }

    /// The ghost's position wrapped into its loop, as published and returned to.
    pub(super) fn ghost_frames(&self) -> Option<f64> {
        let bounds = self.ghost_bounds();
        self.ghost
            .map(|g| looping::wrap_position(g.position, bounds, 1.0))
    }
}

/// Moves `ghost` by `step` track frames, wrapping in `bounds` like the playhead and
/// holding at the track's `end`.
pub(super) fn advance(ghost: &mut Ghost, bounds: Option<(f64, f64)>, step: f64, end: f64) {
    ghost.position = (looping::wrap_position(ghost.position, bounds, step) + step).min(end);
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::{DeckCommand, TrackBuffer, TrackMemory};

    const SR: u32 = 44_100;

    /// A deck playing a silent 100 000-frame track from 50 000, slip on, record touched.
    fn scratching_deck() -> Deck {
        let track = Arc::new(TrackBuffer::new(SR, vec![0.0; 200_000]).unwrap());
        let mut deck = Deck::default();
        for cmd in [
            DeckCommand::Load {
                track,
                memory: TrackMemory::default(),
            },
            DeckCommand::Seek(50_000.0),
            DeckCommand::SetSlip(true),
            DeckCommand::Play,
            DeckCommand::JogTouch(true),
        ] {
            assert!(deck.apply(cmd).is_none());
        }
        deck
    }

    fn render(deck: &mut Deck, frames: usize) {
        let mut out = vec![0.0; frames * 2];
        deck.render_add(&mut out, SR);
    }

    #[test]
    fn a_forced_begin_takes_over_the_running_ghost() {
        let mut deck = scratching_deck();
        render(&mut deck, 1000);
        let ghost = deck.ghost.unwrap();
        assert_eq!(
            (ghost.owner, ghost.position),
            (SlipOwner::Scratch, 51_000.0)
        );
        assert!(deck.slip_begin(SlipOwner::Roll));
        let ghost = deck.ghost.unwrap();
        assert_eq!((ghost.owner, ghost.position), (SlipOwner::Roll, 51_000.0));
        // A non-forced begin while a ghost runs changes nothing.
        assert!(!deck.slip_begin(SlipOwner::Loop));
        assert_eq!(deck.ghost.unwrap().owner, SlipOwner::Roll);
    }

    #[test]
    fn only_the_owner_ends_the_ghost() {
        let mut deck = scratching_deck();
        assert!(deck.slip_begin(SlipOwner::Slice));
        render(&mut deck, 1000);
        let _ = deck.apply(DeckCommand::JogTouch(false));
        deck.slip_end(SlipOwner::Scratch);
        assert_eq!(deck.position, 50_000.0, "a non-owner leaves the playhead");
        assert!(deck.ghost.is_some());
        deck.slip_end(SlipOwner::Slice);
        assert_eq!(deck.position, 51_000.0);
        assert_eq!(deck.ghost, None);
    }

    #[test]
    fn slip_off_keeps_a_forced_ghost() {
        let mut deck = scratching_deck();
        let _ = deck.apply(DeckCommand::SetSlip(false));
        assert_eq!(deck.ghost, None, "the scratch's ghost is dropped");
        assert!(deck.slip_begin(SlipOwner::Roll));
        let _ = deck.apply(DeckCommand::SetSlip(false));
        assert_eq!(deck.ghost.map(|g| g.owner), Some(SlipOwner::Roll));
    }
}
