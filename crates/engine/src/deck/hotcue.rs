//! Hotcue behavior of a deck.

use super::Deck;
use crate::HotcueSlot;

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
        let Some(point) = self.hotcues[slot.index()] else {
            self.hotcues[slot.index()] = Some(self.position);
            return;
        };
        self.position = point;
        self.exit_loop_if_outside(point);
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

    pub(super) fn hotcue_release(&mut self, slot: HotcueSlot) {
        if self.preview == Some(PreviewSource::Hotcue(slot))
            && let Some(point) = self.hotcues[slot.index()]
        {
            self.end_preview(point);
        }
    }

    /// Sets (clamped to the track) or clears a slot without moving the playhead. Clearing
    /// the slot that is previewing ends the preview as a release would.
    pub(super) fn hotcue_set(&mut self, slot: HotcueSlot, frames: Option<f64>) {
        if self.track.is_none() {
            return;
        }
        match frames {
            Some(frames) => {
                self.hotcues[slot.index()] = Some(frames.clamp(0.0, self.frames() as f64))
            }
            None => {
                self.hotcue_release(slot);
                self.hotcues[slot.index()] = None;
            }
        }
    }
}
