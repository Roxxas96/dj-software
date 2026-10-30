use std::sync::Arc;

use crate::{DeckId, TrackBuffer};

/// A message from the controller to the audio thread.
#[derive(Debug, Clone)]
pub enum Command {
    Deck(DeckId, DeckCommand),
    Channel(DeckId, ChannelCommand),
    Master(MasterCommand),
}

#[derive(Debug, Clone)]
pub enum DeckCommand {
    /// Loads a track, paused at frame 0. The tempo slider keeps its position.
    Load(Arc<TrackBuffer>),
    Unload,
    Play,
    Pause,
    TogglePlay,
    CuePress,
    CueRelease,
    /// Slider position in `[0, 1]`: 0.5 is the original tempo, 1.0 the fastest.
    SetTempoSlider(f32),
    SetTempoRange(TempoRange),
    /// Moves the playhead, in track frames, clamped to `[0, frames]`.
    Seek(f64),
}

/// How far the tempo slider reaches either side of the original tempo.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TempoRange {
    /// ±6 %.
    Six,
    /// ±10 %.
    #[default]
    Ten,
    /// ±16 %.
    Sixteen,
    /// ±100 %.
    Hundred,
}

impl TempoRange {
    pub fn fraction(self) -> f64 {
        match self {
            Self::Six => 0.06,
            Self::Ten => 0.10,
            Self::Sixteen => 0.16,
            Self::Hundred => 1.00,
        }
    }
}

/// Per-channel mixer commands. Every `f32` is a knob or fader position in `[0, 1]`
/// (clamped); the engine owns the curves (see [`crate::mixer`]). Non-finite values are
/// ignored.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ChannelCommand {
    /// Input gain: 0.5 is unity, the ends are ±12 dB.
    Trim(f32),
    EqLow(f32),
    EqMid(f32),
    EqHigh(f32),
    /// 0.5 is off.
    Filter(f32),
    /// Channel fader: 1.0 is unity, 0.0 mutes.
    Fader(f32),
    /// Sends the pre-fader signal to the headphone cue bus.
    Pfl(bool),
    XfaderAssign(XfaderAssign),
}

/// Which side of the crossfader a channel follows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XfaderAssign {
    A,
    /// Ignores the crossfader.
    Thru,
    B,
}

/// Master bus commands. Every `f32` is a position in `[0, 1]` (clamped); non-finite values
/// are ignored.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MasterCommand {
    /// 0.0 is full A, 1.0 full B.
    Crossfader(f32),
    MasterLevel(f32),
    HeadphoneLevel(f32),
    /// 0.0 is cue only, 1.0 master only.
    HeadphoneMix(f32),
}
