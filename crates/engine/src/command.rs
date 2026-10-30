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

/// Per-channel mixer commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelCommand {}

/// Master bus commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MasterCommand {}
