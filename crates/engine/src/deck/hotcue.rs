//! Hotcue behavior of a deck.

use super::Deck;
use super::slip::SlipOwner;
use crate::{Hotcue, HotcueSlot};

/// What the held button that started a preview was.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PreviewSource {
    Cue,
    Hotcue(HotcueSlot),
}

impl Deck {
    pub(super) fn hotcue_press(&mut self, slot: HotcueSlot) {
        if self.track.is_none() {
            return;
        }
        let Some(hotcue) = self.hotcues[slot.index()] else {
            self.hotcues[slot.index()] = Some(self.hotcue_here());
            return;
        };
        let point = hotcue.frames;
        // With slip on, a jump while playing lasts as long as the pad is held. Any other
        // jump wins over a running slip.
        let slipping =
            self.playing && self.preview.is_none() && self.slip_begin(SlipOwner::Hotcue(slot));
        if !slipping {
            self.slip_drop();
        }
        self.position = point;
        match hotcue.loop_bounds() {
            // A saved loop replaces the active loop, wherever the jump lands.
            Some((start, end)) => {
                self.exit_loop_by_jump();
                self.loop_in = Some(start);
                self.loop_out = Some(end);
                self.loop_active = true;
            }
            None => self.exit_loop_if_outside(point),
        }
        self.pending_sync = None;
        if self.preview.is_some() {
            // Another button is held: this one takes over the preview.
            self.preview = Some(PreviewSource::Hotcue(slot));
        } else if !self.playing {
            self.preview = Some(PreviewSource::Hotcue(slot));
            self.play_latched = false;
            self.playing = true;
        }
    }

    /// What an empty pad stores: the active normal loop as a saved loop, else the playhead.
    /// A held roll or slice is not a loop to keep.
    fn hotcue_here(&self) -> Hotcue {
        match self.active_loop() {
            Some((start, end)) if self.roll.is_none() => Hotcue {
                frames: start,
                loop_frames: Some(end - start),
            },
            _ => Hotcue::point(self.position),
        }
    }

    pub(super) fn hotcue_release(&mut self, slot: HotcueSlot) {
        let returns = self
            .ghost
            .is_some_and(|g| g.owner == SlipOwner::Hotcue(slot));
        self.slip_end(SlipOwner::Hotcue(slot));
        let hotcue = self.hotcues[slot.index()];
        // A slipped saved loop was only held: back where the track would be, it ends unless
        // that is inside it.
        if returns && hotcue.is_some_and(|h| h.loop_frames.is_some()) {
            self.exit_loop_if_outside(self.position);
        }
        if self.preview == Some(PreviewSource::Hotcue(slot))
            && let Some(hotcue) = hotcue
        {
            self.end_preview(hotcue.frames);
        }
    }

    /// Sets (clamped to the track, see [`Hotcue::clamped`]) or clears a slot without moving
    /// the playhead. Clearing the slot that is previewing ends the preview as a release
    /// would.
    pub(super) fn hotcue_set(&mut self, slot: HotcueSlot, hotcue: Option<Hotcue>) {
        if self.track.is_none() {
            return;
        }
        match hotcue {
            Some(hotcue) => {
                if let Some(hotcue) = hotcue.clamped(self.frames()) {
                    self.hotcues[slot.index()] = Some(hotcue);
                }
            }
            None => {
                self.hotcue_release(slot);
                self.hotcues[slot.index()] = None;
            }
        }
    }
}
