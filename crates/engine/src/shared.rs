use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};

use crate::{DeckId, Grid, HOTCUES};

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
    /// Cue is held and the deck is previewing from the cue point.
    pub previewing: bool,
    pub grid: Option<Grid>,
    /// Hotcue positions in track frames, `None` for an empty slot.
    pub hotcues: [Option<f64>; HOTCUES],
    /// Active or stored loop start and end in track frames.
    pub loop_in: Option<f64>,
    pub loop_out: Option<f64>,
    pub loop_active: bool,
    /// Length in beats of the next auto loop.
    pub loop_beats: f64,
    /// The jog wheel scratches (vinyl mode) rather than only nudges.
    pub vinyl: bool,
    /// The jog wheel's top is touched.
    pub jog_touched: bool,
}

impl Default for DeckSnapshot {
    /// An empty deck: nothing loaded, original tempo, a 4-beat auto loop, vinyl mode.
    fn default() -> Self {
        Self {
            loaded: false,
            playing: false,
            position_frames: 0.0,
            duration_frames: 0,
            track_sample_rate: 0,
            tempo_ratio: 1.0,
            cue_frames: 0.0,
            previewing: false,
            grid: None,
            hotcues: [None; HOTCUES],
            loop_in: None,
            loop_out: None,
            loop_active: false,
            loop_beats: 4.0,
            vinyl: true,
            jog_touched: false,
        }
    }
}

/// Peak absolute sample values of the last rendered block, for metering.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Meters {
    /// Per channel, in deck order, post-fader (before the crossfader).
    pub channel_peak: [f32; 4],
    /// Left and right of the master output, after the clamp.
    pub master_peak: [f32; 2],
}

impl Meters {
    /// Keeps the larger of each peak.
    pub(crate) fn merge(&mut self, other: &Meters) {
        for (peak, other) in self.channel_peak.iter_mut().zip(other.channel_peak) {
            *peak = peak.max(other);
        }
        for (peak, other) in self.master_peak.iter_mut().zip(other.master_peak) {
            *peak = peak.max(other);
        }
    }
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
    previewing: AtomicBool,
    // `Option<f64>`s are stored as bits, with NaN for `None`.
    grid_period: AtomicU64,
    grid_first_beat: AtomicU64,
    hotcues: [AtomicU64; HOTCUES],
    loop_in: AtomicU64,
    loop_out: AtomicU64,
    loop_active: AtomicBool,
    loop_beats: AtomicU64,
    vinyl: AtomicBool,
    jog_touched: AtomicBool,
}

fn opt_bits(v: Option<f64>) -> u64 {
    v.unwrap_or(f64::NAN).to_bits()
}

fn bits_opt(bits: u64) -> Option<f64> {
    Some(f64::from_bits(bits)).filter(|v| !v.is_nan())
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
        self.previewing.store(s.previewing, Ordering::Relaxed);
        // A grid is published as its two fields; no grid is a NaN period.
        let period = s.grid.map(|g| g.beat_period_frames);
        self.grid_period.store(opt_bits(period), Ordering::Relaxed);
        self.grid_first_beat.store(
            s.grid.map_or(0.0, |g| g.first_beat_frames).to_bits(),
            Ordering::Relaxed,
        );
        for (atomic, hotcue) in self.hotcues.iter().zip(s.hotcues) {
            atomic.store(opt_bits(hotcue), Ordering::Relaxed);
        }
        self.loop_in.store(opt_bits(s.loop_in), Ordering::Relaxed);
        self.loop_out.store(opt_bits(s.loop_out), Ordering::Relaxed);
        self.loop_active.store(s.loop_active, Ordering::Relaxed);
        self.loop_beats
            .store(s.loop_beats.to_bits(), Ordering::Relaxed);
        self.vinyl.store(s.vinyl, Ordering::Relaxed);
        self.jog_touched.store(s.jog_touched, Ordering::Relaxed);
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
            previewing: self.previewing.load(Ordering::Relaxed),
            grid: bits_opt(self.grid_period.load(Ordering::Relaxed)).map(|beat_period_frames| {
                Grid {
                    beat_period_frames,
                    first_beat_frames: f64::from_bits(self.grid_first_beat.load(Ordering::Relaxed)),
                }
            }),
            hotcues: self
                .hotcues
                .each_ref()
                .map(|a| bits_opt(a.load(Ordering::Relaxed))),
            loop_in: bits_opt(self.loop_in.load(Ordering::Relaxed)),
            loop_out: bits_opt(self.loop_out.load(Ordering::Relaxed)),
            loop_active: self.loop_active.load(Ordering::Relaxed),
            loop_beats: f64::from_bits(self.loop_beats.load(Ordering::Relaxed)),
            vinyl: self.vinyl.load(Ordering::Relaxed),
            jog_touched: self.jog_touched.load(Ordering::Relaxed),
        }
    }
}
