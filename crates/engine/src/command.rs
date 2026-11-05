use std::sync::Arc;

use crate::{DeckId, Grid, HotcueSlot, TrackBuffer, TrackMemory};

/// A message from the controller to the audio thread.
#[derive(Debug, Clone)]
pub enum Command {
    Deck(DeckId, DeckCommand),
    Channel(DeckId, ChannelCommand),
    Master(MasterCommand),
    /// Aligns `deck`'s beat phase to `master`'s: moves the playhead by the smallest amount, at
    /// most half a beat, so that the fraction of its beat position equals the fraction of the
    /// master's beat position times `beat_ratio` (the deck's beats per master beat: 1, 2 or
    /// 0.5). Needs both grids, both decks loaded, the master playing and `deck != master`.
    ///
    /// A paused deck remembers the request and aligns in the first block where it plays,
    /// against the master's position then. That pending alignment is dropped by `Load`,
    /// `Seek`, a hotcue jump, `SetGrid` and when the master stops. A `beat_ratio` that is not
    /// finite and positive does nothing.
    PhaseSync {
        deck: DeckId,
        master: DeckId,
        beat_ratio: f64,
    },
}

#[derive(Debug, Clone)]
pub enum DeckCommand {
    /// Loads a track, paused at frame 0, then applies `memory.clamped(track.frames())`.
    /// The tempo slider keeps its position.
    Load {
        track: Arc<TrackBuffer>,
        memory: TrackMemory,
    },
    Unload,
    Play,
    Pause,
    TogglePlay,
    CuePress,
    CueRelease,
    /// Slider position in `[0, 1]`: 0.5 is the original tempo, 1.0 the fastest.
    SetTempoSlider(f32),
    SetTempoRange(TempoRange),
    /// Moves the playhead, in track frames, clamped to `[0, frames]`. A seek outside an active
    /// loop exits it.
    Seek(f64),
    /// Sets the cue point, in track frames, clamped to `[0, frames]`, without moving the
    /// playhead or changing the play state. Non-finite values are ignored. Used to restore
    /// a remembered cue after loading a track.
    ///
    /// While the cue is held (previewing) this still updates the stored cue point, and
    /// `CueRelease` returns to the cue point as stored at release time.
    SetCue(f64),
    /// Replaces the beatgrid; `None` clears it. A grid whose period is not finite and
    /// positive, or whose first beat is not finite, is ignored.
    SetGrid(Option<Grid>),
    /// Hotcue button down. An empty slot stores the playhead; a set slot jumps there and,
    /// while paused, previews until [`DeckCommand::HotcueRelease`].
    HotcuePress(HotcueSlot),
    /// Hotcue button up. Only ends the preview started by this same slot: it returns to the
    /// hotcue and pauses, unless play was pressed during the hold.
    HotcueRelease(HotcueSlot),
    /// Clears a hotcue, ending its preview as a release would.
    HotcueDelete(HotcueSlot),
    /// Sets or clears a hotcue, in track frames clamped to `[0, frames]`, without moving the
    /// playhead or changing the play state. A non-finite position is ignored. Used to restore
    /// remembered hotcues; `None` behaves like [`DeckCommand::HotcueDelete`].
    SetHotcue(HotcueSlot, Option<f64>),
    /// Sets the loop-in point at the playhead, clears loop-out and deactivates the loop.
    LoopIn,
    /// With a loop-in before the playhead, sets loop-out at the playhead and activates the
    /// loop. Otherwise does nothing.
    LoopOut,
    /// Exits an active loop and keeps playing. Otherwise, with a grid, loops the last beat
    /// at or before the playhead (loop-in clamps to 0 before the first beat) for the current
    /// loop length in beats; without a grid it does nothing.
    AutoLoop,
    /// Halves the loop length in beats (at least 1/32); an active loop's own length is halved
    /// too (at least 1 frame).
    LoopHalve,
    /// Doubles the loop length in beats (at most 64); an active loop's own length is doubled
    /// too.
    LoopDouble,
    /// Exits an active loop. Otherwise, with both points set, jumps to loop-in and loops.
    ReloopExit,
    /// Turns vinyl mode on or off: with it on, touching the jog wheel's top scratches. On
    /// for a new deck and kept across loads.
    SetVinyl(bool),
    /// Jog wheel top touched or released. With vinyl on, a touch holds the record: normal
    /// motion stops and the deck follows the platter until release, which resumes playback
    /// if the deck is playing.
    JogTouch(bool),
    /// The jog wheel turned by `turns` (negative is backwards). Scratches while the top is
    /// held in vinyl mode, otherwise nudges a playing deck or moves a paused one. Non-finite
    /// values are ignored.
    Jog {
        turns: f64,
        surface: JogSurface,
    },
    /// The jog wheel turned with SHIFT: moves the playhead quickly, playing or paused.
    /// Non-finite values are ignored.
    JogSearch {
        turns: f64,
    },
}

/// Which part of the jog wheel turned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JogSurface {
    /// The touch-sensitive top platter.
    Top,
    /// The outer ring.
    Ring,
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
