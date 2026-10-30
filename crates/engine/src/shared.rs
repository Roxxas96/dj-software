use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};

use crate::DeckId;

/// A deck's state as last published by the renderer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DeckSnapshot {
    pub loaded: bool,
    pub playing: bool,
    /// Playhead in track frames.
    pub position_frames: f64,
    pub duration_frames: usize,
    /// 0 when no track is loaded.
    pub track_sample_rate: u32,
    pub tempo_ratio: f64,
}

/// State the renderer publishes for the controller. Values are for display only, so each
/// field is an independent `Relaxed` atomic and a snapshot may mix two consecutive blocks.
#[derive(Debug, Default)]
pub(crate) struct Shared {
    decks: [DeckShared; DeckId::ALL.len()],
}

impl Shared {
    pub(crate) fn store_deck(&self, id: DeckId, snapshot: &DeckSnapshot) {
        self.decks[id.index()].store(snapshot);
    }

    pub(crate) fn load_deck(&self, id: DeckId) -> DeckSnapshot {
        self.decks[id.index()].load()
    }
}

#[derive(Debug, Default)]
struct DeckShared {
    loaded: AtomicBool,
    playing: AtomicBool,
    position_frames: AtomicU64,
    duration_frames: AtomicUsize,
    track_sample_rate: AtomicU32,
    tempo_ratio: AtomicU64,
}

impl DeckShared {
    fn store(&self, s: &DeckSnapshot) {
        self.loaded.store(s.loaded, Ordering::Relaxed);
        self.playing.store(s.playing, Ordering::Relaxed);
        self.position_frames
            .store(s.position_frames.to_bits(), Ordering::Relaxed);
        self.duration_frames
            .store(s.duration_frames, Ordering::Relaxed);
        self.track_sample_rate
            .store(s.track_sample_rate, Ordering::Relaxed);
        self.tempo_ratio
            .store(s.tempo_ratio.to_bits(), Ordering::Relaxed);
    }

    fn load(&self) -> DeckSnapshot {
        DeckSnapshot {
            loaded: self.loaded.load(Ordering::Relaxed),
            playing: self.playing.load(Ordering::Relaxed),
            position_frames: f64::from_bits(self.position_frames.load(Ordering::Relaxed)),
            duration_frames: self.duration_frames.load(Ordering::Relaxed),
            track_sample_rate: self.track_sample_rate.load(Ordering::Relaxed),
            tempo_ratio: f64::from_bits(self.tempo_ratio.load(Ordering::Relaxed)),
        }
    }
}
