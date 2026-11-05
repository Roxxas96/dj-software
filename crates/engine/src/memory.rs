//! What the engine is told about a track when it loads: cue, hotcues and beatgrid.

/// Number of hotcue slots per deck.
pub const HOTCUES: usize = 8;

/// A hotcue slot, numbered 1 to 8 as on the deck.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct HotcueSlot(u8);

impl HotcueSlot {
    /// Every slot, in order.
    pub const ALL: [HotcueSlot; HOTCUES] = [
        HotcueSlot(1),
        HotcueSlot(2),
        HotcueSlot(3),
        HotcueSlot(4),
        HotcueSlot(5),
        HotcueSlot(6),
        HotcueSlot(7),
        HotcueSlot(8),
    ];

    /// The slot numbered `n` (1 to 8), or `None` when out of range.
    pub fn new(n: u8) -> Option<HotcueSlot> {
        (1..=HOTCUES as u8).contains(&n).then_some(HotcueSlot(n))
    }

    /// Zero-based index into a hotcue array.
    pub fn index(self) -> usize {
        usize::from(self.0) - 1
    }

    /// The slot number, 1 to 8.
    pub fn number(self) -> u8 {
        self.0
    }
}

/// A constant-tempo beatgrid, in track frames.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Grid {
    /// Frames between two beats, at the track's own sample rate.
    pub beat_period_frames: f64,
    /// Frame of a beat; the grid extends both ways from it.
    pub first_beat_frames: f64,
}

impl Grid {
    /// A grid from a tempo and a first beat, or `None` unless `bpm` and `first_beat_frames`
    /// are finite, `bpm` is positive and `sample_rate` is not zero.
    pub fn from_bpm(bpm: f64, first_beat_frames: f64, sample_rate: u32) -> Option<Grid> {
        (bpm.is_finite() && bpm > 0.0 && first_beat_frames.is_finite() && sample_rate > 0).then(
            || Grid {
                beat_period_frames: 60.0 * f64::from(sample_rate) / bpm,
                first_beat_frames,
            },
        )
    }

    /// The position `frames` in beats from the first beat (negative before it).
    pub fn beats_at(&self, frames: f64) -> f64 {
        (frames - self.first_beat_frames) / self.beat_period_frames
    }
}

/// A track's remembered cue points and beatgrid, applied by the engine on load.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct TrackMemory {
    /// Cue point in track frames.
    pub cue: f64,
    /// Hotcue positions in track frames, indexed by [`HotcueSlot::index`].
    pub hotcues: [Option<f64>; HOTCUES],
    pub grid: Option<Grid>,
}

impl TrackMemory {
    /// The memory as the engine applies it to a track of `frames` frames: cue and hotcues
    /// clamped to `[0, frames]`, a non-finite cue reset to 0, a non-finite hotcue dropped.
    pub fn clamped(self, frames: usize) -> TrackMemory {
        let end = frames as f64;
        let clamp = |v: f64| v.clamp(0.0, end);
        TrackMemory {
            cue: if self.cue.is_finite() {
                clamp(self.cue)
            } else {
                0.0
            },
            hotcues: self.hotcues.map(|h| h.filter(|v| v.is_finite()).map(clamp)),
            grid: self.grid,
        }
    }
}
