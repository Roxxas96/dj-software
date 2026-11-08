use engine::{DeckId, DeckSnapshot, TempoRange};

use crate::{ActionKind, Scope, Side, Target, ValueTarget};

/// What the app last told the engine about one deck and its channel.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DeckControl {
    pub tempo_range: TempoRange,
    pub pfl: bool,
    /// Tempo slider position, 0.5 is the original tempo.
    pub tempo: f32,
    pub trim: f32,
    pub eq_high: f32,
    pub eq_mid: f32,
    pub eq_low: f32,
    pub filter: f32,
    pub fader: f32,
    /// Effect amount, 0 to 1.
    pub fx_level: f32,
    /// Which effect slots the app turned on.
    pub fx_on: [bool; 3],
    /// Where `deck.scratch_anchor_return` seeks to, in track frames. Not cleared when a
    /// track loads.
    pub scratch_anchor: Option<f64>,
    /// Which stems the app left on, by `Stem::index`. The engine turns every stem on at
    /// each load, so a stem press first resets this to all on when the deck's `load_seq` has
    /// moved past `stems_load_seq` (the only reset; nothing else reads the flags).
    pub stems_on: [bool; 7],
    /// The deck's `load_seq` when `stems_on` last matched the engine.
    pub stems_load_seq: u64,
}

impl Default for DeckControl {
    /// The engine's defaults.
    fn default() -> Self {
        Self {
            tempo_range: TempoRange::default(),
            pfl: false,
            tempo: 0.5,
            trim: 0.5,
            eq_high: 0.5,
            eq_mid: 0.5,
            eq_low: 0.5,
            filter: 0.5,
            fader: 1.0,
            fx_level: 0.5,
            fx_on: [false; 3],
            scratch_anchor: None,
            stems_on: [true; 7],
            stems_load_seq: 0,
        }
    }
}

/// What a side's performance pads do. The hardware mode leaves the pads to the controller's
/// own modes; the others make the mapping's pad-mode bindings for that side apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum PadMode {
    #[default]
    Hardware,
    BeatJump,
    LoopRoll,
    Slicer,
    /// The SB3's PAD SCRATCH routine: its pads set the deck's scratch anchor and its
    /// repeated HOT CUE 1 presses return to it.
    PadScratch,
}

impl PadMode {
    /// Every mode.
    pub const ALL: [PadMode; 5] = [
        PadMode::Hardware,
        PadMode::BeatJump,
        PadMode::LoopRoll,
        PadMode::Slicer,
        PadMode::PadScratch,
    ];

    /// The id used in mapping files.
    pub fn id(self) -> &'static str {
        match self {
            PadMode::Hardware => "hardware",
            PadMode::BeatJump => "beat_jump",
            PadMode::LoopRoll => "loop_roll",
            PadMode::Slicer => "slicer",
            PadMode::PadScratch => "pad_scratch",
        }
    }

    pub fn from_id(id: &str) -> Option<PadMode> {
        Self::ALL.into_iter().find(|mode| mode.id() == id)
    }
}

/// The SB3's own pad modes, named by the button that picks them. Which one is active is
/// not observable from MIDI; the app remembers the last button pressed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HardwarePadMode {
    HotCue,
    FxFade,
    Sampler,
    Trans,
}

impl HardwarePadMode {
    /// Every hardware mode.
    pub const ALL: [HardwarePadMode; 4] = [
        HardwarePadMode::HotCue,
        HardwarePadMode::FxFade,
        HardwarePadMode::Sampler,
        HardwarePadMode::Trans,
    ];

    /// The name shown to the user.
    pub fn label(self) -> &'static str {
        match self {
            HardwarePadMode::HotCue => "HOT CUE",
            HardwarePadMode::FxFade => "FX FADE",
            HardwarePadMode::Sampler => "SAMPLER",
            HardwarePadMode::Trans => "TRANS",
        }
    }
}

/// One side of the controller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SideControl {
    /// The deck the side's controls currently drive.
    pub active_deck: DeckId,
    /// SHIFT is held.
    pub shift: bool,
    /// What the side's performance pads do.
    pub pad_mode: PadMode,
    /// The last hardware mode button pressed; `None` until one is.
    pub hardware_mode: Option<HardwarePadMode>,
}

impl SideControl {
    /// The name of what the side's pads do: the last hardware mode button while the pads
    /// are in hardware mode ("pads" before any), else the software mode.
    pub fn pad_mode_label(&self) -> &'static str {
        match self.pad_mode {
            PadMode::Hardware => self.hardware_mode.map_or("pads", HardwarePadMode::label),
            PadMode::BeatJump => "BEAT JUMP",
            PadMode::LoopRoll => "LOOP ROLL",
            PadMode::Slicer => "SLICER",
            PadMode::PadScratch => "PAD SCRATCH",
        }
    }
}

/// The app-side model of everything controls have set. [`crate::apply`] updates it only
/// for commands the engine accepted; transport state (playing, cue, position) is read from
/// engine snapshots instead.
#[derive(Debug, Clone, PartialEq)]
pub struct ControlState {
    pub decks: [DeckControl; 4],
    /// Indexed by [`Side::index`].
    pub sides: [SideControl; 2],
    pub crossfader: f32,
    pub master_level: f32,
    pub headphone_level: f32,
    pub headphone_mix: f32,
    /// The headphones hear the master rather than the cue bus.
    pub headphone_master: bool,
    /// Jog wheel steps that make one turn of the platter. The controller runtime sets it
    /// from the mapping.
    pub jog_steps_per_turn: u16,
    /// Beats the beat jump pads 1 to 8 move the playhead (negative is backwards). The
    /// config sets them.
    pub beat_jump_pads: [f64; 8],
    /// The same with SHIFT held.
    pub beat_jump_shift_pads: [f64; 8],
    /// Beats the loop roll pads 1 to 8 roll.
    pub loop_roll_pads: [f64; 8],
    /// Seconds of the short backspin action. The config sets it.
    pub backspin_short: f64,
    /// Seconds of the long backspin action. The config sets it.
    pub backspin_long: f64,
    /// Waveform zoom steps the zoom actions asked for: each `ui.waveform_zoom_in` adds one,
    /// each `ui.waveform_zoom_out` takes one away. The window zooms by the change since it
    /// last looked; the count itself means nothing (it wraps).
    pub waveform_zoom: i32,
}

/// The default [`ControlState::beat_jump_pads`].
const DEFAULT_BEAT_JUMP_PADS: [f64; 8] = [-4.0, 4.0, -8.0, 8.0, -16.0, 16.0, -32.0, 32.0];
/// The default [`ControlState::beat_jump_shift_pads`].
const DEFAULT_BEAT_JUMP_SHIFT_PADS: [f64; 8] = [-1.0, 1.0, -2.0, 2.0, -4.0, 4.0, -64.0, 64.0];
/// The default [`ControlState::loop_roll_pads`].
const DEFAULT_LOOP_ROLL_PADS: [f64; 8] = [
    1.0 / 16.0,
    1.0 / 8.0,
    1.0 / 4.0,
    1.0 / 2.0,
    1.0,
    2.0,
    4.0,
    8.0,
];

/// The default [`ControlState::jog_steps_per_turn`].
const DEFAULT_JOG_STEPS_PER_TURN: u16 = 720;

impl Default for ControlState {
    /// The engine's defaults; the sides show decks 1 and 2 with SHIFT released.
    fn default() -> Self {
        Self {
            decks: [DeckControl::default(); 4],
            sides: Side::ALL.map(|side| SideControl {
                active_deck: side.decks()[0],
                shift: false,
                pad_mode: PadMode::Hardware,
                hardware_mode: None,
            }),
            crossfader: 0.5,
            master_level: 0.8,
            headphone_level: 0.8,
            headphone_mix: 0.0,
            headphone_master: false,
            jog_steps_per_turn: DEFAULT_JOG_STEPS_PER_TURN,
            beat_jump_pads: DEFAULT_BEAT_JUMP_PADS,
            beat_jump_shift_pads: DEFAULT_BEAT_JUMP_SHIFT_PADS,
            loop_roll_pads: DEFAULT_LOOP_ROLL_PADS,
            backspin_short: 1.0,
            backspin_long: 2.0,
            waveform_zoom: 0,
        }
    }
}

impl ControlState {
    pub fn deck(&self, id: DeckId) -> &DeckControl {
        &self.decks[id.index()]
    }

    pub fn deck_mut(&mut self, id: DeckId) -> &mut DeckControl {
        &mut self.decks[id.index()]
    }

    pub fn side(&self, side: Side) -> &SideControl {
        &self.sides[side.index()]
    }

    pub fn side_mut(&mut self, side: Side) -> &mut SideControl {
        &mut self.sides[side.index()]
    }

    /// The current value of an absolute target, for soft takeover. `None` for actions that
    /// are not absolute, or when the deck is missing (deck values) or present (global ones).
    pub fn value(&self, target: ValueTarget) -> Option<f32> {
        match target {
            (kind, Some(id)) => {
                let deck = self.deck(id);
                match kind {
                    ActionKind::Tempo => Some(deck.tempo),
                    ActionKind::Trim => Some(deck.trim),
                    ActionKind::EqHigh => Some(deck.eq_high),
                    ActionKind::EqMid => Some(deck.eq_mid),
                    ActionKind::EqLow => Some(deck.eq_low),
                    ActionKind::Filter => Some(deck.filter),
                    ActionKind::Fader => Some(deck.fader),
                    ActionKind::FxLevel => Some(deck.fx_level),
                    _ => None,
                }
            }
            (kind, None) => match kind {
                ActionKind::Crossfader => Some(self.crossfader),
                ActionKind::MasterLevel => Some(self.master_level),
                ActionKind::HeadphoneLevel => Some(self.headphone_level),
                ActionKind::HeadphoneMix => Some(self.headphone_mix),
                _ => None,
            },
        }
    }

    /// The field behind an absolute target; same mapping as [`Self::value`].
    ///
    /// Writing through it bypasses [`crate::apply`]: the engine is not told and derived
    /// state (`headphone_master`) is not updated. The controller runtime uses it to stage a
    /// value whose engine command is deferred, and restores the field if that send fails.
    pub fn value_mut(&mut self, target: ValueTarget) -> Option<&mut f32> {
        match target {
            (kind, Some(id)) => {
                let deck = self.deck_mut(id);
                match kind {
                    ActionKind::Tempo => Some(&mut deck.tempo),
                    ActionKind::Trim => Some(&mut deck.trim),
                    ActionKind::EqHigh => Some(&mut deck.eq_high),
                    ActionKind::EqMid => Some(&mut deck.eq_mid),
                    ActionKind::EqLow => Some(&mut deck.eq_low),
                    ActionKind::Filter => Some(&mut deck.filter),
                    ActionKind::Fader => Some(&mut deck.fader),
                    ActionKind::FxLevel => Some(&mut deck.fx_level),
                    _ => None,
                }
            }
            (kind, None) => match kind {
                ActionKind::Crossfader => Some(&mut self.crossfader),
                ActionKind::MasterLevel => Some(&mut self.master_level),
                ActionKind::HeadphoneLevel => Some(&mut self.headphone_level),
                ActionKind::HeadphoneMix => Some(&mut self.headphone_mix),
                _ => None,
            },
        }
    }
}

/// Observable state an output (LED) can follow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StateKey {
    DeckPlaying,
    DeckPlayLed,
    DeckCueLed,
    ChannelPfl,
    SideAltDeck,
    HeadphoneMaster,
    Shift,
}

impl StateKey {
    pub const ALL: [StateKey; 7] = [
        StateKey::DeckPlaying,
        StateKey::DeckPlayLed,
        StateKey::DeckCueLed,
        StateKey::ChannelPfl,
        StateKey::SideAltDeck,
        StateKey::HeadphoneMaster,
        StateKey::Shift,
    ];

    /// The stable id used by mapping files (`state = "…"`). This is the one place these
    /// ids are defined.
    pub fn id(&self) -> &'static str {
        match self {
            StateKey::DeckPlaying => "deck.playing",
            StateKey::DeckPlayLed => "deck.play_led",
            StateKey::DeckCueLed => "deck.cue_led",
            StateKey::ChannelPfl => "channel.pfl",
            StateKey::SideAltDeck => "side.alt_deck",
            StateKey::HeadphoneMaster => "master.headphone_master",
            StateKey::Shift => "side.shift",
        }
    }

    pub fn from_id(id: &str) -> Option<StateKey> {
        Self::ALL.into_iter().find(|key| key.id() == id)
    }

    /// The target this key reads: deck keys take a deck (or a side, read through its
    /// active deck), side keys a side, global keys none.
    pub fn scope(&self) -> Scope {
        match self {
            StateKey::DeckPlaying
            | StateKey::DeckPlayLed
            | StateKey::DeckCueLed
            | StateKey::ChannelPfl => Scope::Deck,
            StateKey::SideAltDeck | StateKey::Shift => Scope::Side,
            StateKey::HeadphoneMaster => Scope::Global,
        }
    }
}

/// What an LED shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Led {
    Off,
    On,
    Blink,
}

fn on_if(cond: bool) -> Led {
    if cond { Led::On } else { Led::Off }
}

/// Evaluates `key` for `target`, VirtualDJ-style. Deck keys take a deck target or a side
/// target (read through the side's active deck) and read transport state from `snap`,
/// which must be that deck's snapshot. Side keys take a side target. A key whose target
/// is missing or of the wrong kind is `Off`.
pub fn led(
    key: StateKey,
    target: Option<Target>,
    state: &ControlState,
    snap: Option<&DeckSnapshot>,
) -> Led {
    let deck = match target {
        Some(Target::Deck(id)) => Some(id),
        Some(Target::Side(side)) => Some(state.side(side).active_deck),
        Some(Target::Global) | None => None,
    };
    let side = match target {
        Some(Target::Side(side)) => Some(side),
        _ => None,
    };
    let loaded = snap.filter(|s| s.loaded);
    match key {
        StateKey::DeckPlaying => on_if(deck.is_some() && loaded.is_some_and(|s| s.playing)),
        StateKey::DeckPlayLed => match (deck, loaded) {
            (Some(_), Some(s)) if s.playing => Led::On,
            (Some(_), Some(_)) => Led::Blink,
            _ => Led::Off,
        },
        StateKey::DeckCueLed => match (deck, loaded) {
            (Some(_), Some(s)) if s.previewing => Led::On,
            (Some(_), Some(s)) if s.playing => Led::Off,
            (Some(_), Some(s)) if (s.position_frames - s.cue_frames).abs() < 1.0 => Led::On,
            (Some(_), Some(_)) => Led::Blink,
            _ => Led::Off,
        },
        StateKey::ChannelPfl => on_if(deck.is_some_and(|id| state.deck(id).pfl)),
        StateKey::SideAltDeck => {
            on_if(side.is_some_and(|side| state.side(side).active_deck != side.decks()[0]))
        }
        StateKey::Shift => on_if(side.is_some_and(|side| state.side(side).shift)),
        StateKey::HeadphoneMaster => on_if(state.headphone_master),
    }
}
