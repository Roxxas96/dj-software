use engine::{DeckId, HotcueSlot, Stem};

use crate::{HardwarePadMode, PadMode};

/// One half of the controller. Decks 1 and 3 sit on the left, decks 2 and 4 on the right.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Side {
    Left,
    Right,
}

impl Side {
    pub const ALL: [Side; 2] = [Side::Left, Side::Right];

    /// The side `deck` belongs to.
    pub fn of(deck: DeckId) -> Side {
        if deck.index().is_multiple_of(2) {
            Side::Left
        } else {
            Side::Right
        }
    }

    /// 0 for the left side, 1 for the right.
    pub fn index(self) -> usize {
        match self {
            Side::Left => 0,
            Side::Right => 1,
        }
    }

    /// The two decks of this side: the main one (1 or 2) and the alternate one (3 or 4).
    pub fn decks(self) -> [DeckId; 2] {
        let main = self.index();
        [DeckId::ALL[main], DeckId::ALL[main + 2]]
    }
}

/// What an action acts on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Target {
    Deck(DeckId),
    Side(Side),
    Global,
}

/// The kind of target an [`ActionKind`] needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Scope {
    Deck,
    Side,
    Global,
}

/// A continuous value an absolute action drives: the key for soft takeover and coalescing.
/// The deck is `None` for global values.
pub type ValueTarget = (ActionKind, Option<DeckId>);

/// What a control sends.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Input {
    Press,
    Release,
    /// A position in `[0, 1]`.
    Absolute(f32),
    /// Signed steps.
    Relative(i32),
}

/// The kinds of [`Input`], for checking what an action accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InputKind {
    /// [`Input::Press`] and [`Input::Release`].
    Button,
    Absolute,
    Relative,
}

impl Input {
    pub fn kind(self) -> InputKind {
        match self {
            Input::Press | Input::Release => InputKind::Button,
            Input::Absolute(_) => InputKind::Absolute,
            Input::Relative(_) => InputKind::Relative,
        }
    }
}

/// An action without its target. Each kind has a stable string id used in mapping files.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ActionKind {
    PlayPause,
    Cue,
    Tempo,
    TempoRangeNext,
    ReturnToStart,
    GridShiftBack,
    GridShiftForward,
    GridSetDownbeat,
    GridHalve,
    GridDouble,
    GridTap,
    GridLockToggle,
    Trim,
    EqHigh,
    EqMid,
    EqLow,
    Filter,
    Fader,
    PflToggle,
    Crossfader,
    MasterLevel,
    HeadphoneLevel,
    HeadphoneMix,
    HeadphoneMasterToggle,
    DeckToggle,
    Shift,
    BrowserScroll,
    BrowserToggleFocus,
    /// Opens or closes the highlighted folder in the browser tree.
    BrowserToggleFolder,
    WaveformZoomIn,
    WaveformZoomOut,
    LoadSelected,
    /// A hotcue pad: sets, jumps to or previews the slot.
    Hotcue(HotcueSlot),
    HotcueDelete(HotcueSlot),
    LoopIn,
    LoopOut,
    AutoLoop,
    LoopHalve,
    LoopDouble,
    ReloopExit,
    Sync,
    VinylToggle,
    JogTouch,
    /// Ends a jog touch without ever starting one: both press and release let go. Bound
    /// to the touch note a controller sends with SHIFT held, so a SHIFT search stays
    /// audible while a touch begun without SHIFT still ends.
    JogRelease,
    JogTop,
    JogRing,
    JogSearch,
    /// The jog platter as a scratch surface: scratches a touched deck even with vinyl off.
    JogScratch,
    /// A backspin of `ControlState::backspin_short` seconds.
    BackspinShort,
    /// A backspin of `ControlState::backspin_long` seconds.
    BackspinLong,
    /// Stores the deck's playhead as its scratch anchor.
    ScratchAnchorSet,
    /// Seeks the deck to its scratch anchor; with none yet, stores the playhead as the
    /// anchor and does not move.
    ScratchAnchorReturn,
    /// With no normal loop active, an auto loop of 1 beat.
    LoopFade1,
    /// With no normal loop active, an auto loop of 4 beats.
    LoopFade4,
    /// Exits an active normal loop; otherwise nothing.
    LoopExit,
    /// Sets the side's [`PadMode`]; no engine command.
    PadMode(PadMode),
    /// Sets the side's pad mode to hardware and remembers which hardware button it was;
    /// no engine command.
    HardwarePadMode(HardwarePadMode),
    /// Beat jump pad `slot`, by the size in `ControlState::beat_jump_pads`.
    BeatJump(HotcueSlot),
    /// Beat jump pad `slot`, by the size in `ControlState::beat_jump_shift_pads`.
    BeatJumpShift(HotcueSlot),
    /// Loop roll pad `slot`: held, by the size in `ControlState::loop_roll_pads`.
    LoopRoll(HotcueSlot),
    /// Slicer pad `slot`: held, plays that slice.
    Slice(HotcueSlot),
    SlipToggle,
    KeylockToggle,
    /// Turns effect slot N (0 to 2) of the deck's channel on or off.
    FxToggle(u8),
    FxLevel,
    /// A stem pad: turns the deck's stem on or off (all on after a load). It works whatever
    /// the EQ mode, once the deck has stems; a press before then is kept.
    StemToggle(Stem),
    /// Plays sampler slot `slot` (the sampler has as many slots as there are pads).
    SamplerPlay(HotcueSlot),
    SamplerStop(HotcueSlot),
}

/// Ids of [`ActionKind::Hotcue`], by slot index.
const HOTCUE_IDS: [&str; 8] = [
    "deck.hotcue_1",
    "deck.hotcue_2",
    "deck.hotcue_3",
    "deck.hotcue_4",
    "deck.hotcue_5",
    "deck.hotcue_6",
    "deck.hotcue_7",
    "deck.hotcue_8",
];

/// Ids of [`ActionKind::HotcueDelete`], by slot index.
const HOTCUE_DELETE_IDS: [&str; 8] = [
    "deck.hotcue_delete_1",
    "deck.hotcue_delete_2",
    "deck.hotcue_delete_3",
    "deck.hotcue_delete_4",
    "deck.hotcue_delete_5",
    "deck.hotcue_delete_6",
    "deck.hotcue_delete_7",
    "deck.hotcue_delete_8",
];

/// Ids of [`ActionKind::BeatJump`], by slot index.
const BEAT_JUMP_IDS: [&str; 8] = [
    "deck.beat_jump_1",
    "deck.beat_jump_2",
    "deck.beat_jump_3",
    "deck.beat_jump_4",
    "deck.beat_jump_5",
    "deck.beat_jump_6",
    "deck.beat_jump_7",
    "deck.beat_jump_8",
];

/// Ids of [`ActionKind::BeatJumpShift`], by slot index.
const BEAT_JUMP_SHIFT_IDS: [&str; 8] = [
    "deck.beat_jump_shift_1",
    "deck.beat_jump_shift_2",
    "deck.beat_jump_shift_3",
    "deck.beat_jump_shift_4",
    "deck.beat_jump_shift_5",
    "deck.beat_jump_shift_6",
    "deck.beat_jump_shift_7",
    "deck.beat_jump_shift_8",
];

/// Ids of [`ActionKind::LoopRoll`], by slot index.
const LOOP_ROLL_IDS: [&str; 8] = [
    "deck.loop_roll_1",
    "deck.loop_roll_2",
    "deck.loop_roll_3",
    "deck.loop_roll_4",
    "deck.loop_roll_5",
    "deck.loop_roll_6",
    "deck.loop_roll_7",
    "deck.loop_roll_8",
];

/// Ids of [`ActionKind::Slice`], by slot index.
const SLICE_IDS: [&str; 8] = [
    "deck.slice_1",
    "deck.slice_2",
    "deck.slice_3",
    "deck.slice_4",
    "deck.slice_5",
    "deck.slice_6",
    "deck.slice_7",
    "deck.slice_8",
];

/// Ids of [`ActionKind::FxToggle`], by effect slot.
const FX_TOGGLE_IDS: [&str; 3] = ["deck.fx_toggle_1", "deck.fx_toggle_2", "deck.fx_toggle_3"];

/// Ids of [`ActionKind::SamplerPlay`], by slot index.
const SAMPLER_PLAY_IDS: [&str; 8] = [
    "sampler.play_1",
    "sampler.play_2",
    "sampler.play_3",
    "sampler.play_4",
    "sampler.play_5",
    "sampler.play_6",
    "sampler.play_7",
    "sampler.play_8",
];

/// Ids of [`ActionKind::SamplerStop`], by slot index.
const SAMPLER_STOP_IDS: [&str; 8] = [
    "sampler.stop_1",
    "sampler.stop_2",
    "sampler.stop_3",
    "sampler.stop_4",
    "sampler.stop_5",
    "sampler.stop_6",
    "sampler.stop_7",
    "sampler.stop_8",
];

impl ActionKind {
    /// Every kind: each slot family (hotcues, beat jumps, rolls, slices, sampler) once per slot.
    pub const ALL: [ActionKind; 139] = {
        let mut all = [ActionKind::PlayPause; 139];
        let fixed = [
            ActionKind::PlayPause,
            ActionKind::Cue,
            ActionKind::Tempo,
            ActionKind::TempoRangeNext,
            ActionKind::ReturnToStart,
            ActionKind::GridShiftBack,
            ActionKind::GridShiftForward,
            ActionKind::GridSetDownbeat,
            ActionKind::GridHalve,
            ActionKind::GridDouble,
            ActionKind::GridTap,
            ActionKind::GridLockToggle,
            ActionKind::Trim,
            ActionKind::EqHigh,
            ActionKind::EqMid,
            ActionKind::EqLow,
            ActionKind::Filter,
            ActionKind::Fader,
            ActionKind::PflToggle,
            ActionKind::Crossfader,
            ActionKind::MasterLevel,
            ActionKind::HeadphoneLevel,
            ActionKind::HeadphoneMix,
            ActionKind::HeadphoneMasterToggle,
            ActionKind::DeckToggle,
            ActionKind::Shift,
            ActionKind::BrowserScroll,
            ActionKind::BrowserToggleFocus,
            ActionKind::BrowserToggleFolder,
            ActionKind::WaveformZoomIn,
            ActionKind::WaveformZoomOut,
            ActionKind::LoadSelected,
            ActionKind::LoopIn,
            ActionKind::LoopOut,
            ActionKind::AutoLoop,
            ActionKind::LoopHalve,
            ActionKind::LoopDouble,
            ActionKind::ReloopExit,
            ActionKind::Sync,
            ActionKind::VinylToggle,
            ActionKind::JogTouch,
            ActionKind::JogRelease,
            ActionKind::JogTop,
            ActionKind::JogRing,
            ActionKind::JogSearch,
            ActionKind::JogScratch,
            ActionKind::BackspinShort,
            ActionKind::BackspinLong,
            ActionKind::ScratchAnchorSet,
            ActionKind::ScratchAnchorReturn,
            ActionKind::LoopFade1,
            ActionKind::LoopFade4,
            ActionKind::LoopExit,
            ActionKind::PadMode(PadMode::Hardware),
            ActionKind::PadMode(PadMode::BeatJump),
            ActionKind::PadMode(PadMode::LoopRoll),
            ActionKind::PadMode(PadMode::Slicer),
            ActionKind::PadMode(PadMode::PadScratch),
            ActionKind::HardwarePadMode(HardwarePadMode::HotCue),
            ActionKind::HardwarePadMode(HardwarePadMode::FxFade),
            ActionKind::HardwarePadMode(HardwarePadMode::Sampler),
            ActionKind::HardwarePadMode(HardwarePadMode::Trans),
            ActionKind::SlipToggle,
            ActionKind::KeylockToggle,
            ActionKind::FxToggle(0),
            ActionKind::FxToggle(1),
            ActionKind::FxToggle(2),
            ActionKind::FxLevel,
            ActionKind::StemToggle(Stem::Vocals),
            ActionKind::StemToggle(Stem::Instruments),
            ActionKind::StemToggle(Stem::Bass),
            ActionKind::StemToggle(Stem::Rhythm),
            ActionKind::StemToggle(Stem::Kicks),
            ActionKind::StemToggle(Stem::Melody),
            ActionKind::StemToggle(Stem::Hats),
        ];
        let mut i = 0;
        while i < fixed.len() {
            all[i] = fixed[i];
            i += 1;
        }
        let mut slot = 0;
        while slot < HotcueSlot::ALL.len() {
            let s = HotcueSlot::ALL[slot];
            let at = fixed.len() + slot;
            all[at] = ActionKind::Hotcue(s);
            all[at + 8] = ActionKind::HotcueDelete(s);
            all[at + 16] = ActionKind::BeatJump(s);
            all[at + 24] = ActionKind::BeatJumpShift(s);
            all[at + 32] = ActionKind::LoopRoll(s);
            all[at + 40] = ActionKind::Slice(s);
            all[at + 48] = ActionKind::SamplerPlay(s);
            all[at + 56] = ActionKind::SamplerStop(s);
            slot += 1;
        }
        all
    };

    /// The stable id used in mapping files.
    pub fn id(self) -> &'static str {
        match self {
            ActionKind::PlayPause => "deck.play_pause",
            ActionKind::Cue => "deck.cue",
            ActionKind::Tempo => "deck.tempo",
            ActionKind::TempoRangeNext => "deck.tempo_range_next",
            ActionKind::ReturnToStart => "deck.return_to_start",
            ActionKind::GridShiftBack => "deck.grid_shift_back",
            ActionKind::GridShiftForward => "deck.grid_shift_forward",
            ActionKind::GridSetDownbeat => "deck.grid_set_downbeat",
            ActionKind::GridHalve => "deck.grid_halve",
            ActionKind::GridDouble => "deck.grid_double",
            ActionKind::GridTap => "deck.grid_tap",
            ActionKind::GridLockToggle => "deck.grid_lock_toggle",
            ActionKind::Trim => "channel.trim",
            ActionKind::EqHigh => "channel.eq_high",
            ActionKind::EqMid => "channel.eq_mid",
            ActionKind::EqLow => "channel.eq_low",
            ActionKind::Filter => "channel.filter",
            ActionKind::Fader => "channel.fader",
            ActionKind::PflToggle => "channel.pfl_toggle",
            ActionKind::Crossfader => "master.crossfader",
            ActionKind::MasterLevel => "master.level",
            ActionKind::HeadphoneLevel => "master.headphone_level",
            ActionKind::HeadphoneMix => "master.headphone_mix",
            ActionKind::HeadphoneMasterToggle => "master.headphone_master_toggle",
            ActionKind::DeckToggle => "side.deck_toggle",
            ActionKind::Shift => "modifier.shift",
            ActionKind::BrowserScroll => "browser.scroll",
            ActionKind::BrowserToggleFocus => "browser.toggle_focus",
            ActionKind::BrowserToggleFolder => "browser.toggle_folder",
            ActionKind::WaveformZoomIn => "ui.waveform_zoom_in",
            ActionKind::WaveformZoomOut => "ui.waveform_zoom_out",
            ActionKind::LoadSelected => "deck.load_selected",
            ActionKind::Hotcue(slot) => HOTCUE_IDS[slot.index()],
            ActionKind::HotcueDelete(slot) => HOTCUE_DELETE_IDS[slot.index()],
            ActionKind::LoopIn => "deck.loop_in",
            ActionKind::LoopOut => "deck.loop_out",
            ActionKind::AutoLoop => "deck.auto_loop",
            ActionKind::LoopHalve => "deck.loop_halve",
            ActionKind::LoopDouble => "deck.loop_double",
            ActionKind::ReloopExit => "deck.reloop_exit",
            ActionKind::Sync => "deck.sync",
            ActionKind::VinylToggle => "deck.vinyl_toggle",
            ActionKind::JogTouch => "deck.jog_touch",
            ActionKind::JogRelease => "deck.jog_release",
            ActionKind::JogTop => "deck.jog_top",
            ActionKind::JogRing => "deck.jog_ring",
            ActionKind::JogSearch => "deck.jog_search",
            ActionKind::JogScratch => "deck.jog_scratch",
            ActionKind::BackspinShort => "deck.backspin_short",
            ActionKind::BackspinLong => "deck.backspin_long",
            ActionKind::ScratchAnchorSet => "deck.scratch_anchor_set",
            ActionKind::ScratchAnchorReturn => "deck.scratch_anchor_return",
            ActionKind::LoopFade1 => "deck.loop_fade_1",
            ActionKind::LoopFade4 => "deck.loop_fade_4",
            ActionKind::LoopExit => "deck.loop_exit",
            ActionKind::PadMode(PadMode::Hardware) => "pads.mode_hardware",
            ActionKind::PadMode(PadMode::BeatJump) => "pads.mode_beat_jump",
            ActionKind::PadMode(PadMode::LoopRoll) => "pads.mode_loop_roll",
            ActionKind::PadMode(PadMode::Slicer) => "pads.mode_slicer",
            ActionKind::PadMode(PadMode::PadScratch) => "pads.mode_pad_scratch",
            ActionKind::HardwarePadMode(HardwarePadMode::HotCue) => "pads.mode_hot_cue",
            ActionKind::HardwarePadMode(HardwarePadMode::FxFade) => "pads.mode_fx_fade",
            ActionKind::HardwarePadMode(HardwarePadMode::Sampler) => "pads.mode_sampler",
            ActionKind::HardwarePadMode(HardwarePadMode::Trans) => "pads.mode_trans",
            ActionKind::BeatJump(slot) => BEAT_JUMP_IDS[slot.index()],
            ActionKind::BeatJumpShift(slot) => BEAT_JUMP_SHIFT_IDS[slot.index()],
            ActionKind::LoopRoll(slot) => LOOP_ROLL_IDS[slot.index()],
            ActionKind::Slice(slot) => SLICE_IDS[slot.index()],
            ActionKind::SlipToggle => "deck.slip_toggle",
            ActionKind::KeylockToggle => "deck.keylock_toggle",
            ActionKind::FxToggle(slot) => FX_TOGGLE_IDS[slot as usize],
            ActionKind::FxLevel => "deck.fx_level",
            ActionKind::StemToggle(Stem::Vocals) => "deck.stem_toggle_vocals",
            ActionKind::StemToggle(Stem::Instruments) => "deck.stem_toggle_instruments",
            ActionKind::StemToggle(Stem::Bass) => "deck.stem_toggle_bass",
            ActionKind::StemToggle(Stem::Rhythm) => "deck.stem_toggle_rhythm",
            ActionKind::StemToggle(Stem::Kicks) => "deck.stem_toggle_kicks",
            ActionKind::StemToggle(Stem::Melody) => "deck.stem_toggle_melody",
            ActionKind::StemToggle(Stem::Hats) => "deck.stem_toggle_hats",
            ActionKind::SamplerPlay(slot) => SAMPLER_PLAY_IDS[slot.index()],
            ActionKind::SamplerStop(slot) => SAMPLER_STOP_IDS[slot.index()],
        }
    }

    pub fn from_id(id: &str) -> Option<ActionKind> {
        Self::ALL.into_iter().find(|kind| kind.id() == id)
    }

    /// The kind of target this action needs.
    pub fn scope(self) -> Scope {
        match self {
            ActionKind::PlayPause
            | ActionKind::Cue
            | ActionKind::Tempo
            | ActionKind::TempoRangeNext
            | ActionKind::ReturnToStart
            | ActionKind::GridShiftBack
            | ActionKind::GridShiftForward
            | ActionKind::GridSetDownbeat
            | ActionKind::GridHalve
            | ActionKind::GridDouble
            | ActionKind::GridTap
            | ActionKind::GridLockToggle
            | ActionKind::Trim
            | ActionKind::EqHigh
            | ActionKind::EqMid
            | ActionKind::EqLow
            | ActionKind::Filter
            | ActionKind::Fader
            | ActionKind::PflToggle
            | ActionKind::LoopIn
            | ActionKind::LoopOut
            | ActionKind::AutoLoop
            | ActionKind::LoopHalve
            | ActionKind::LoopDouble
            | ActionKind::ReloopExit
            | ActionKind::Sync
            | ActionKind::VinylToggle
            | ActionKind::JogTouch
            | ActionKind::JogRelease
            | ActionKind::JogTop
            | ActionKind::JogRing
            | ActionKind::JogSearch
            | ActionKind::JogScratch
            | ActionKind::BackspinShort
            | ActionKind::BackspinLong
            | ActionKind::ScratchAnchorSet
            | ActionKind::ScratchAnchorReturn
            | ActionKind::LoopFade1
            | ActionKind::LoopFade4
            | ActionKind::LoopExit
            | ActionKind::Hotcue(_)
            | ActionKind::HotcueDelete(_)
            | ActionKind::BeatJump(_)
            | ActionKind::BeatJumpShift(_)
            | ActionKind::LoopRoll(_)
            | ActionKind::Slice(_)
            | ActionKind::SlipToggle
            | ActionKind::KeylockToggle
            | ActionKind::FxToggle(_)
            | ActionKind::FxLevel
            | ActionKind::StemToggle(_)
            | ActionKind::LoadSelected => Scope::Deck,
            ActionKind::Crossfader
            | ActionKind::MasterLevel
            | ActionKind::HeadphoneLevel
            | ActionKind::HeadphoneMix
            | ActionKind::HeadphoneMasterToggle
            | ActionKind::BrowserScroll
            | ActionKind::BrowserToggleFocus
            | ActionKind::BrowserToggleFolder
            | ActionKind::WaveformZoomIn
            | ActionKind::WaveformZoomOut
            | ActionKind::SamplerPlay(_)
            | ActionKind::SamplerStop(_) => Scope::Global,
            ActionKind::DeckToggle
            | ActionKind::Shift
            | ActionKind::PadMode(_)
            | ActionKind::HardwarePadMode(_) => Scope::Side,
        }
    }

    /// Whether this action accepts inputs of `kind`. This is the one table of what each
    /// action takes: absolute actions take positions, `browser.scroll` and the jog wheel
    /// actions take relative steps, and every other action takes button presses.
    pub fn accepts(self, kind: InputKind) -> bool {
        let absolute = matches!(
            self,
            ActionKind::Tempo
                | ActionKind::Trim
                | ActionKind::EqHigh
                | ActionKind::EqMid
                | ActionKind::EqLow
                | ActionKind::Filter
                | ActionKind::Fader
                | ActionKind::FxLevel
                | ActionKind::Crossfader
                | ActionKind::MasterLevel
                | ActionKind::HeadphoneLevel
                | ActionKind::HeadphoneMix
        );
        let relative = matches!(
            self,
            ActionKind::BrowserScroll
                | ActionKind::JogTop
                | ActionKind::JogRing
                | ActionKind::JogSearch
                | ActionKind::JogScratch
        );
        match kind {
            InputKind::Button => !absolute && !relative,
            InputKind::Absolute => absolute,
            InputKind::Relative => relative,
        }
    }

    /// Whether this action takes [`Input::Absolute`] values (and so has a [`ValueTarget`]).
    pub fn is_absolute(self) -> bool {
        self.accepts(InputKind::Absolute)
    }

    /// Whether this action reacts to [`Input::Release`] as well as [`Input::Press`]. The
    /// releases of `deck.hotcue_delete_N` and `deck.return_to_start` end the hold of their
    /// unshifted siblings (`deck.hotcue_N`, `deck.cue`), since a controller that sends a
    /// different note with SHIFT held sends the note-off there when SHIFT goes down
    /// during the hold.
    pub fn takes_release(self) -> bool {
        matches!(
            self,
            ActionKind::Cue
                | ActionKind::Shift
                | ActionKind::ReturnToStart
                | ActionKind::HotcueDelete(_)
                | ActionKind::Hotcue(_)
                | ActionKind::JogTouch
                | ActionKind::JogRelease
                | ActionKind::LoopRoll(_)
                | ActionKind::Slice(_)
        )
    }

    /// The hold whose release this kind's release ends, when the two sit on one physical
    /// control: a controller that sends a different note with SHIFT held sends the
    /// note-off there when SHIFT goes down during the hold. `deck.jog_release` ends
    /// `deck.jog_touch`, `deck.hotcue_delete_N` ends `deck.hotcue_N` (same slot) and
    /// `deck.return_to_start` ends `deck.cue`. `deck.loop_roll_N` and `deck.slice_N` are
    /// their own partner: a controller binds them to the shifted pad note as well, and a
    /// release arriving on that sibling binding ends the held press of the same kind on the
    /// deck it went to. Other kinds have no partner. The partner lookup ignores binding
    /// conditions (pad mode, SHIFT, deck): a held press always gets its release, even when
    /// the layer it was pressed on no longer holds.
    pub fn hold_partner(self) -> Option<ActionKind> {
        match self {
            ActionKind::JogRelease => Some(ActionKind::JogTouch),
            ActionKind::HotcueDelete(slot) => Some(ActionKind::Hotcue(slot)),
            ActionKind::ReturnToStart => Some(ActionKind::Cue),
            ActionKind::LoopRoll(_) | ActionKind::Slice(_) => Some(self),
            _ => None,
        }
    }
}

/// An action bound to its target. Only valid kind/target pairs can be built.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    PlayPause(DeckId),
    Cue(DeckId),
    Tempo(DeckId),
    TempoRangeNext(DeckId),
    ReturnToStart(DeckId),
    GridShiftBack(DeckId),
    GridShiftForward(DeckId),
    GridSetDownbeat(DeckId),
    GridHalve(DeckId),
    GridDouble(DeckId),
    GridTap(DeckId),
    GridLockToggle(DeckId),
    Trim(DeckId),
    EqHigh(DeckId),
    EqMid(DeckId),
    EqLow(DeckId),
    Filter(DeckId),
    Fader(DeckId),
    PflToggle(DeckId),
    Crossfader,
    MasterLevel,
    HeadphoneLevel,
    HeadphoneMix,
    HeadphoneMasterToggle,
    DeckToggle(Side),
    Shift(Side),
    BrowserScroll,
    BrowserToggleFocus,
    BrowserToggleFolder,
    WaveformZoomIn,
    WaveformZoomOut,
    LoadSelected(DeckId),
    Hotcue(DeckId, HotcueSlot),
    HotcueDelete(DeckId, HotcueSlot),
    LoopIn(DeckId),
    LoopOut(DeckId),
    AutoLoop(DeckId),
    LoopHalve(DeckId),
    LoopDouble(DeckId),
    ReloopExit(DeckId),
    Sync(DeckId),
    VinylToggle(DeckId),
    JogTouch(DeckId),
    JogRelease(DeckId),
    JogTop(DeckId),
    JogRing(DeckId),
    JogSearch(DeckId),
    JogScratch(DeckId),
    BackspinShort(DeckId),
    BackspinLong(DeckId),
    ScratchAnchorSet(DeckId),
    ScratchAnchorReturn(DeckId),
    LoopFade1(DeckId),
    LoopFade4(DeckId),
    LoopExit(DeckId),
    PadMode(Side, PadMode),
    HardwarePadMode(Side, HardwarePadMode),
    BeatJump(DeckId, HotcueSlot),
    BeatJumpShift(DeckId, HotcueSlot),
    LoopRoll(DeckId, HotcueSlot),
    Slice(DeckId, HotcueSlot),
    SlipToggle(DeckId),
    KeylockToggle(DeckId),
    FxToggle(DeckId, u8),
    FxLevel(DeckId),
    StemToggle(DeckId, Stem),
    SamplerPlay(HotcueSlot),
    SamplerStop(HotcueSlot),
}

impl Action {
    /// Binds `kind` to `target`, or `None` if the target is not the kind's [`Scope`].
    pub fn new(kind: ActionKind, target: Target) -> Option<Action> {
        use ActionKind as K;
        Some(match (kind, target) {
            (K::PlayPause, Target::Deck(d)) => Action::PlayPause(d),
            (K::Cue, Target::Deck(d)) => Action::Cue(d),
            (K::Tempo, Target::Deck(d)) => Action::Tempo(d),
            (K::TempoRangeNext, Target::Deck(d)) => Action::TempoRangeNext(d),
            (K::ReturnToStart, Target::Deck(d)) => Action::ReturnToStart(d),
            (K::GridShiftBack, Target::Deck(d)) => Action::GridShiftBack(d),
            (K::GridShiftForward, Target::Deck(d)) => Action::GridShiftForward(d),
            (K::GridSetDownbeat, Target::Deck(d)) => Action::GridSetDownbeat(d),
            (K::GridHalve, Target::Deck(d)) => Action::GridHalve(d),
            (K::GridDouble, Target::Deck(d)) => Action::GridDouble(d),
            (K::GridTap, Target::Deck(d)) => Action::GridTap(d),
            (K::GridLockToggle, Target::Deck(d)) => Action::GridLockToggle(d),
            (K::Trim, Target::Deck(d)) => Action::Trim(d),
            (K::EqHigh, Target::Deck(d)) => Action::EqHigh(d),
            (K::EqMid, Target::Deck(d)) => Action::EqMid(d),
            (K::EqLow, Target::Deck(d)) => Action::EqLow(d),
            (K::Filter, Target::Deck(d)) => Action::Filter(d),
            (K::Fader, Target::Deck(d)) => Action::Fader(d),
            (K::PflToggle, Target::Deck(d)) => Action::PflToggle(d),
            (K::Crossfader, Target::Global) => Action::Crossfader,
            (K::MasterLevel, Target::Global) => Action::MasterLevel,
            (K::HeadphoneLevel, Target::Global) => Action::HeadphoneLevel,
            (K::HeadphoneMix, Target::Global) => Action::HeadphoneMix,
            (K::HeadphoneMasterToggle, Target::Global) => Action::HeadphoneMasterToggle,
            (K::DeckToggle, Target::Side(s)) => Action::DeckToggle(s),
            (K::Shift, Target::Side(s)) => Action::Shift(s),
            (K::BrowserScroll, Target::Global) => Action::BrowserScroll,
            (K::BrowserToggleFocus, Target::Global) => Action::BrowserToggleFocus,
            (K::BrowserToggleFolder, Target::Global) => Action::BrowserToggleFolder,
            (K::WaveformZoomIn, Target::Global) => Action::WaveformZoomIn,
            (K::WaveformZoomOut, Target::Global) => Action::WaveformZoomOut,
            (K::LoadSelected, Target::Deck(d)) => Action::LoadSelected(d),
            (K::Hotcue(slot), Target::Deck(d)) => Action::Hotcue(d, slot),
            (K::HotcueDelete(slot), Target::Deck(d)) => Action::HotcueDelete(d, slot),
            (K::LoopIn, Target::Deck(d)) => Action::LoopIn(d),
            (K::LoopOut, Target::Deck(d)) => Action::LoopOut(d),
            (K::AutoLoop, Target::Deck(d)) => Action::AutoLoop(d),
            (K::LoopHalve, Target::Deck(d)) => Action::LoopHalve(d),
            (K::LoopDouble, Target::Deck(d)) => Action::LoopDouble(d),
            (K::ReloopExit, Target::Deck(d)) => Action::ReloopExit(d),
            (K::Sync, Target::Deck(d)) => Action::Sync(d),
            (K::VinylToggle, Target::Deck(d)) => Action::VinylToggle(d),
            (K::JogTouch, Target::Deck(d)) => Action::JogTouch(d),
            (K::JogRelease, Target::Deck(d)) => Action::JogRelease(d),
            (K::JogTop, Target::Deck(d)) => Action::JogTop(d),
            (K::JogRing, Target::Deck(d)) => Action::JogRing(d),
            (K::JogSearch, Target::Deck(d)) => Action::JogSearch(d),
            (K::JogScratch, Target::Deck(d)) => Action::JogScratch(d),
            (K::BackspinShort, Target::Deck(d)) => Action::BackspinShort(d),
            (K::BackspinLong, Target::Deck(d)) => Action::BackspinLong(d),
            (K::ScratchAnchorSet, Target::Deck(d)) => Action::ScratchAnchorSet(d),
            (K::ScratchAnchorReturn, Target::Deck(d)) => Action::ScratchAnchorReturn(d),
            (K::LoopFade1, Target::Deck(d)) => Action::LoopFade1(d),
            (K::LoopFade4, Target::Deck(d)) => Action::LoopFade4(d),
            (K::LoopExit, Target::Deck(d)) => Action::LoopExit(d),
            (K::PadMode(mode), Target::Side(s)) => Action::PadMode(s, mode),
            (K::HardwarePadMode(mode), Target::Side(s)) => Action::HardwarePadMode(s, mode),
            (K::BeatJump(slot), Target::Deck(d)) => Action::BeatJump(d, slot),
            (K::BeatJumpShift(slot), Target::Deck(d)) => Action::BeatJumpShift(d, slot),
            (K::LoopRoll(slot), Target::Deck(d)) => Action::LoopRoll(d, slot),
            (K::Slice(slot), Target::Deck(d)) => Action::Slice(d, slot),
            (K::SlipToggle, Target::Deck(d)) => Action::SlipToggle(d),
            (K::KeylockToggle, Target::Deck(d)) => Action::KeylockToggle(d),
            (K::FxToggle(n), Target::Deck(d)) => Action::FxToggle(d, n),
            (K::FxLevel, Target::Deck(d)) => Action::FxLevel(d),
            (K::StemToggle(stem), Target::Deck(d)) => Action::StemToggle(d, stem),
            (K::SamplerPlay(slot), Target::Global) => Action::SamplerPlay(slot),
            (K::SamplerStop(slot), Target::Global) => Action::SamplerStop(slot),
            _ => return None,
        })
    }

    /// The id of this action's kind.
    pub fn id(&self) -> &'static str {
        self.kind().id()
    }

    /// Looks up an action kind by its id; it needs a target to become an [`Action`].
    pub fn from_id(id: &str) -> Option<ActionKind> {
        ActionKind::from_id(id)
    }

    pub fn kind(&self) -> ActionKind {
        match self {
            Action::PlayPause(_) => ActionKind::PlayPause,
            Action::Cue(_) => ActionKind::Cue,
            Action::Tempo(_) => ActionKind::Tempo,
            Action::TempoRangeNext(_) => ActionKind::TempoRangeNext,
            Action::ReturnToStart(_) => ActionKind::ReturnToStart,
            Action::GridShiftBack(_) => ActionKind::GridShiftBack,
            Action::GridShiftForward(_) => ActionKind::GridShiftForward,
            Action::GridSetDownbeat(_) => ActionKind::GridSetDownbeat,
            Action::GridHalve(_) => ActionKind::GridHalve,
            Action::GridDouble(_) => ActionKind::GridDouble,
            Action::GridTap(_) => ActionKind::GridTap,
            Action::GridLockToggle(_) => ActionKind::GridLockToggle,
            Action::Trim(_) => ActionKind::Trim,
            Action::EqHigh(_) => ActionKind::EqHigh,
            Action::EqMid(_) => ActionKind::EqMid,
            Action::EqLow(_) => ActionKind::EqLow,
            Action::Filter(_) => ActionKind::Filter,
            Action::Fader(_) => ActionKind::Fader,
            Action::PflToggle(_) => ActionKind::PflToggle,
            Action::Crossfader => ActionKind::Crossfader,
            Action::MasterLevel => ActionKind::MasterLevel,
            Action::HeadphoneLevel => ActionKind::HeadphoneLevel,
            Action::HeadphoneMix => ActionKind::HeadphoneMix,
            Action::HeadphoneMasterToggle => ActionKind::HeadphoneMasterToggle,
            Action::DeckToggle(_) => ActionKind::DeckToggle,
            Action::Shift(_) => ActionKind::Shift,
            Action::BrowserScroll => ActionKind::BrowserScroll,
            Action::BrowserToggleFocus => ActionKind::BrowserToggleFocus,
            Action::BrowserToggleFolder => ActionKind::BrowserToggleFolder,
            Action::WaveformZoomIn => ActionKind::WaveformZoomIn,
            Action::WaveformZoomOut => ActionKind::WaveformZoomOut,
            Action::LoadSelected(_) => ActionKind::LoadSelected,
            Action::Hotcue(_, slot) => ActionKind::Hotcue(*slot),
            Action::HotcueDelete(_, slot) => ActionKind::HotcueDelete(*slot),
            Action::LoopIn(_) => ActionKind::LoopIn,
            Action::LoopOut(_) => ActionKind::LoopOut,
            Action::AutoLoop(_) => ActionKind::AutoLoop,
            Action::LoopHalve(_) => ActionKind::LoopHalve,
            Action::LoopDouble(_) => ActionKind::LoopDouble,
            Action::ReloopExit(_) => ActionKind::ReloopExit,
            Action::Sync(_) => ActionKind::Sync,
            Action::VinylToggle(_) => ActionKind::VinylToggle,
            Action::JogTouch(_) => ActionKind::JogTouch,
            Action::JogRelease(_) => ActionKind::JogRelease,
            Action::JogTop(_) => ActionKind::JogTop,
            Action::JogRing(_) => ActionKind::JogRing,
            Action::JogSearch(_) => ActionKind::JogSearch,
            Action::JogScratch(_) => ActionKind::JogScratch,
            Action::BackspinShort(_) => ActionKind::BackspinShort,
            Action::BackspinLong(_) => ActionKind::BackspinLong,
            Action::ScratchAnchorSet(_) => ActionKind::ScratchAnchorSet,
            Action::ScratchAnchorReturn(_) => ActionKind::ScratchAnchorReturn,
            Action::LoopFade1(_) => ActionKind::LoopFade1,
            Action::LoopFade4(_) => ActionKind::LoopFade4,
            Action::LoopExit(_) => ActionKind::LoopExit,
            Action::PadMode(_, mode) => ActionKind::PadMode(*mode),
            Action::HardwarePadMode(_, mode) => ActionKind::HardwarePadMode(*mode),
            Action::BeatJump(_, slot) => ActionKind::BeatJump(*slot),
            Action::BeatJumpShift(_, slot) => ActionKind::BeatJumpShift(*slot),
            Action::LoopRoll(_, slot) => ActionKind::LoopRoll(*slot),
            Action::Slice(_, slot) => ActionKind::Slice(*slot),
            Action::SlipToggle(_) => ActionKind::SlipToggle,
            Action::KeylockToggle(_) => ActionKind::KeylockToggle,
            Action::FxToggle(_, n) => ActionKind::FxToggle(*n),
            Action::FxLevel(_) => ActionKind::FxLevel,
            Action::StemToggle(_, stem) => ActionKind::StemToggle(*stem),
            Action::SamplerPlay(slot) => ActionKind::SamplerPlay(*slot),
            Action::SamplerStop(slot) => ActionKind::SamplerStop(*slot),
        }
    }

    pub fn target(&self) -> Target {
        match *self {
            Action::PlayPause(d)
            | Action::Cue(d)
            | Action::Tempo(d)
            | Action::TempoRangeNext(d)
            | Action::ReturnToStart(d)
            | Action::GridShiftBack(d)
            | Action::GridShiftForward(d)
            | Action::GridSetDownbeat(d)
            | Action::GridHalve(d)
            | Action::GridDouble(d)
            | Action::GridTap(d)
            | Action::GridLockToggle(d)
            | Action::Trim(d)
            | Action::EqHigh(d)
            | Action::EqMid(d)
            | Action::EqLow(d)
            | Action::Filter(d)
            | Action::Fader(d)
            | Action::PflToggle(d)
            | Action::LoadSelected(d)
            | Action::Hotcue(d, _)
            | Action::HotcueDelete(d, _)
            | Action::LoopIn(d)
            | Action::LoopOut(d)
            | Action::AutoLoop(d)
            | Action::LoopHalve(d)
            | Action::LoopDouble(d)
            | Action::ReloopExit(d)
            | Action::Sync(d)
            | Action::VinylToggle(d)
            | Action::JogTouch(d)
            | Action::JogRelease(d)
            | Action::JogTop(d)
            | Action::JogRing(d)
            | Action::JogSearch(d)
            | Action::JogScratch(d)
            | Action::BackspinShort(d)
            | Action::BackspinLong(d)
            | Action::ScratchAnchorSet(d)
            | Action::ScratchAnchorReturn(d)
            | Action::LoopFade1(d)
            | Action::LoopFade4(d)
            | Action::LoopExit(d)
            | Action::BeatJump(d, _)
            | Action::BeatJumpShift(d, _)
            | Action::LoopRoll(d, _)
            | Action::Slice(d, _)
            | Action::SlipToggle(d)
            | Action::KeylockToggle(d)
            | Action::FxToggle(d, _)
            | Action::FxLevel(d)
            | Action::StemToggle(d, _) => Target::Deck(d),
            Action::Crossfader
            | Action::MasterLevel
            | Action::HeadphoneLevel
            | Action::HeadphoneMix
            | Action::HeadphoneMasterToggle
            | Action::BrowserScroll
            | Action::BrowserToggleFocus
            | Action::BrowserToggleFolder
            | Action::WaveformZoomIn
            | Action::WaveformZoomOut
            | Action::SamplerPlay(_)
            | Action::SamplerStop(_) => Target::Global,
            Action::DeckToggle(s)
            | Action::Shift(s)
            | Action::PadMode(s, _)
            | Action::HardwarePadMode(s, _) => Target::Side(s),
        }
    }

    /// The deck this action acts on, if any.
    pub fn deck(&self) -> Option<DeckId> {
        match self.target() {
            Target::Deck(d) => Some(d),
            Target::Side(_) | Target::Global => None,
        }
    }

    /// The value this action drives, for absolute actions.
    pub fn value_target(&self) -> Option<ValueTarget> {
        let kind = self.kind();
        kind.is_absolute().then(|| (kind, self.deck()))
    }
}
