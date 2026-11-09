use std::sync::Arc;

use crate::{
    DeckId, EffectKind, EqMode, Grid, HotcueSlot, Stem, StemKnobs, StemSet, TrackBuffer,
    TrackMemory,
};

/// A message from the controller to the audio thread.
#[derive(Debug, Clone)]
pub enum Command {
    Deck(DeckId, DeckCommand),
    Channel(DeckId, ChannelCommand),
    Master(MasterCommand),
    Sampler(SamplerCommand),
    /// Aligns `deck`'s beat phase to `master`'s: moves the playhead by the smallest amount, at
    /// most half a beat, so that the fraction of its beat position equals the fraction of the
    /// master's beat position times `beat_ratio` (the deck's beats per master beat: 1, 2 or
    /// 0.5). A running slip ghost moves by the same amount, so the return stays in phase.
    /// Needs both grids, both decks loaded, the master playing and `deck != master`.
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
    /// The tempo slider keeps its position. Drops the previous track's stems and turns
    /// every stem pad back on.
    Load {
        track: Arc<TrackBuffer>,
        memory: TrackMemory,
    },
    /// Unloads the track and its stems.
    Unload,
    /// Gives the loaded track its stems (`None` drops them). A set whose frame count or
    /// sample rate is not the track's, or one sent to an empty deck, is ignored. A
    /// replaced or ignored set goes back to the controller to be freed, like a track.
    /// Stems arriving or leaving while the knobs control them crossfade over one block.
    SetStems(Option<Arc<StemSet>>),
    /// A stem pad: turns one stem or knob part on or off, ramped over one block. A part
    /// plays only while every pad covering it is on ([`pad_gains`](crate::pad_gains)). It
    /// applies whatever the [`EqMode`] and [`StemKnobs`], once the deck has stems (a press
    /// before then is kept). Every pad is on after `Load`.
    StemOn(Stem, bool),
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
    /// Starts an auto loop of exactly `beats` beats at the last beat at or before the playhead,
    /// like [`DeckCommand::AutoLoop`], and makes `beats` the loop length. Does nothing while a
    /// normal loop is active, without a grid, or for a `beats` that is not finite and positive.
    AutoLoopBeats(f64),
    /// Exits an active loop and keeps playing. Otherwise does nothing (it never reloops).
    LoopExit,
    /// Halves the loop length in beats (at least 1/32); an active loop's own length is halved
    /// too (at least 1 frame).
    LoopHalve,
    /// Doubles the loop length in beats (at most 64); an active loop's own length is doubled
    /// too.
    LoopDouble,
    /// Exits an active loop. Otherwise, with both points set, jumps to loop-in and loops.
    ReloopExit,
    /// Jumps the playhead by `beats` beats of the grid (negative goes back), clamped to the
    /// track. An active normal loop moves with it while it stays inside the track, else it
    /// is exited; a running slip ghost moves by the same amount; a pending phase sync is
    /// dropped. Ignored without a grid or for a non-finite `beats`.
    BeatJump {
        /// Signed distance in beats.
        beats: f64,
    },
    /// Loop roll pad down: with a grid on a deck playing for real, loops `beats` beats (a
    /// *roll loop*, separate from the normal loop) and always slips. The loop starts at the
    /// playhead snapped down to a multiple of `beats` counted from the grid's first beat;
    /// the playhead does not move and the ghost starts at it. A press while another roll or
    /// slice is held replaces it and keeps its ghost. Ignored for a `beats` that is not
    /// finite and positive. While a roll or slice is held, the normal loop's commands
    /// (`LoopIn` to `ReloopExit`) are ignored; ending it over a slipped loop that is still
    /// active hands the ghost back to that loop.
    RollPress {
        /// Roll length in beats.
        beats: f64,
    },
    /// Loop roll pad up: ends the roll and returns to the ghost, but only if the held roll
    /// was started by a press of this same `beats`.
    RollRelease {
        /// Length in beats of the pad released.
        beats: f64,
    },
    /// Slicer pad `0..=7` down: with a grid on a deck playing for real, loops beat
    /// `block + slice` of the current 8-beat block (see
    /// [`DeckSnapshot::slicer_block_beat`](crate::DeckSnapshot::slicer_block_beat)) as a
    /// 1-beat roll loop, and always slips. A press while a slice is held moves the loop
    /// to the new slice of the same block; any other press replaces a held roll. A slice
    /// with no part inside the track is ignored.
    SlicePress(u8),
    /// Slicer pad up: ends the slice and returns to the ghost, but only if this slice is
    /// the one held.
    SliceRelease(u8),
    /// Turns vinyl mode on or off: with it on, touching the jog wheel's top scratches. On
    /// for a new deck and kept across loads.
    SetVinyl(bool),
    /// Turns slip mode on or off. While it is on, a scratch (vinyl touch to release), a
    /// normal loop (activation to exit) and a hotcue jump while playing (pad press to
    /// release, so the hotcue is momentary) end where the track would have been: a silent
    /// ghost playhead keeps moving at the deck's tempo, nudge included, and the playhead
    /// returns to it. A scratch slips only on a deck playing for real, and a deliberate jump
    /// (seek, cue, search, a hotcue that does not slip) drops the ghost and ends a held roll
    /// or slice. Turning slip off drops a running ghost, except a held roll's or slice's,
    /// and leaves the playhead where it is. Off for a new deck and kept across loads;
    /// `Load` drops the ghost.
    SetSlip(bool),
    /// Turns keylock on or off: with it on, a tempo change keeps the track's pitch (the
    /// playing audio is time-stretched rather than resampled). Scratching, a paused deck and
    /// the original tempo (a tempo ratio within 1e-4 of 1, whatever the nudge) play without
    /// the stretcher; a nudge bends whichever path plays. Off for a new deck and kept across
    /// loads.
    SetKeylock(bool),
    /// Jog wheel top touched or released. With vinyl on, a touch holds the record: normal
    /// motion stops and the deck follows the platter until release, which resumes playback
    /// if the deck is playing.
    JogTouch(bool),
    /// The jog wheel turned by `turns` (negative is backwards). Scratches while the top is
    /// held in vinyl mode, otherwise bends the speed of a playing deck (a nudge that fades in 0.1 s) or moves a paused one. Non-finite
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
    /// Spins a playing deck backwards like a stopped record: the playhead starts at -3 times
    /// the deck's normal speed and slows linearly to 0 over `seconds` (clamped to 0.2-10),
    /// then normal playback resumes forward from there. The spin stops at the track's
    /// start, and playback resumes from 0. A paused deck or one without a track ignores it,
    /// as does one held by a scratch. A deliberate jump: it drops a slip ghost and ends a
    /// roll or slice. A `Seek`, hotcue jump, `Load` or `JogTouch(true)` cancels it. Keylock is
    /// bypassed while it spins. Non-finite values are ignored.
    Backspin {
        seconds: f64,
    },
}

/// Which part of the jog wheel turned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JogSurface {
    /// The touch-sensitive top platter.
    Top,
    /// The outer ring.
    Ring,
    /// The whole platter as the SB3's firmware routines see it: scratches while the deck is
    /// touched, whatever the vinyl flag says, and nudges like the ring otherwise.
    Platter,
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
    /// Turns effect slot `0..=2` on or off, crossfading over one block. A slot index `>= 3`
    /// is ignored. Turning a slot on from off starts the effect from clear state.
    FxEnabled(usize, bool),
    /// The channel's effect amount in `[0, 1]` (clamped, 0.5 for a new engine): the dry/wet
    /// of all its effects, or the cutoff of a Filter slot (which is always fully wet). Echo
    /// and reverb keep the dry signal at unity up to 0.5 while the wet rises to unity, then
    /// fade the dry out (dry `min(1, 2(1 − a))`, wet `min(1, 2a)`); flanger crossfades.
    FxAmount(f32),
    /// Sets the kind of the channel's effect slot `0..=2` and clears its state, as
    /// [`MasterCommand::FxSlot`] does for every channel. A slot index `>= 3` is ignored.
    FxSlot(usize, EffectKind),
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
    /// Turns the crossfader on or off (on for a new engine). While off, every channel's
    /// crossfader gain is 1.0 whatever its assignment or the crossfader position; turning
    /// it on again restores the gains from the current position.
    CrossfaderEnabled(bool),
    /// Sets the kind of effect slot `0..=2` on every channel and clears its state (the new
    /// engine runs Echo, Reverb and Flanger). A slot index `>= 3` is ignored.
    FxSlot(usize, EffectKind),
    /// What every channel's EQ knobs control (`Eq` for a new engine). In
    /// [`EqMode::Stems`], a deck with stems plays them with the
    /// [`knob_law`](crate::knob_law) gains in place of the EQ bands; switching crossfades
    /// over one block.
    EqMode(EqMode),
    /// Which parts the EQ knobs control in [`EqMode::Stems`] (`Vdj` for a new engine);
    /// switching crossfades over one block.
    StemKnobs(StemKnobs),
}

/// Sampler commands. A slot index `>= SAMPLER_SLOTS` is ignored.
#[derive(Debug, Clone)]
pub enum SamplerCommand {
    /// Puts a sample in a slot (`None` empties it), stopping what the slot was playing. A
    /// replaced buffer goes back to the controller to be freed, like a track.
    Set(usize, Option<Arc<TrackBuffer>>),
    /// Plays the slot's sample from the start, restarting it if it is playing. An empty
    /// slot does nothing.
    Play(usize),
    /// Stops the slot.
    Stop(usize),
    /// Sampler volume in `[0, 1]` (clamped, 1.0 for a new engine); the engine owns the
    /// curve ([`level_gain`](crate::mixer::level_gain)). Non-finite values are ignored.
    Volume(f32),
}
