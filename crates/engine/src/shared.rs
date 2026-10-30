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
    /// Cue point in track frames.
    pub cue_frames: f64,
}

/// Peak absolute sample values of the last rendered block, for metering.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Meters {
    /// Per channel, in deck order, post-fader (before the crossfader).
    pub channel_peak: [f32; 4],
    /// Left and right of the master output, after the clamp.
    pub master_peak: [f32; 2],
}

/// State the renderer publishes for the controller. Values are for display only, so each
/// field is an independent `Relaxed` atomic and a snapshot may mix two consecutive blocks.
#[derive(Debug, Default)]
pub(crate) struct Shared {
    decks: [DeckShared; DeckId::ALL.len()],
    channel_peak: [AtomicU32; DeckId::ALL.len()],
    master_peak: [AtomicU32; 2],
}

impl Shared {
    pub(crate) fn store_deck(&self, id: DeckId, snapshot: &DeckSnapshot) {
        self.decks[id.index()].store(snapshot);
    }

    pub(crate) fn load_deck(&self, id: DeckId) -> DeckSnapshot {
        self.decks[id.index()].load()
    }

    pub(crate) fn store_meters(&self, meters: &Meters) {
        store_f32s(&self.channel_peak, &meters.channel_peak);
        store_f32s(&self.master_peak, &meters.master_peak);
    }

    pub(crate) fn load_meters(&self) -> Meters {
        Meters {
            channel_peak: self
                .channel_peak
                .each_ref()
                .map(|a| f32::from_bits(a.load(Ordering::Relaxed))),
            master_peak: self
                .master_peak
                .each_ref()
                .map(|a| f32::from_bits(a.load(Ordering::Relaxed))),
        }
    }
}

fn store_f32s(atomics: &[AtomicU32], values: &[f32]) {
    for (atomic, value) in atomics.iter().zip(values) {
        atomic.store(value.to_bits(), Ordering::Relaxed);
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
    cue_frames: AtomicU64,
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
        self.cue_frames
            .store(s.cue_frames.to_bits(), Ordering::Relaxed);
    }

    fn load(&self) -> DeckSnapshot {
        DeckSnapshot {
            loaded: self.loaded.load(Ordering::Relaxed),
            playing: self.playing.load(Ordering::Relaxed),
            position_frames: f64::from_bits(self.position_frames.load(Ordering::Relaxed)),
            duration_frames: self.duration_frames.load(Ordering::Relaxed),
            track_sample_rate: self.track_sample_rate.load(Ordering::Relaxed),
            tempo_ratio: f64::from_bits(self.tempo_ratio.load(Ordering::Relaxed)),
            cue_frames: f64::from_bits(self.cue_frames.load(Ordering::Relaxed)),
        }
    }
}
