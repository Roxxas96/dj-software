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
}

impl Default for ControlState {
    /// The engine's defaults; the sides show decks 1 and 2 with SHIFT released.
    fn default() -> Self {
        Self {
            decks: [DeckControl::default(); 4],
            sides: Side::ALL.map(|side| SideControl {
                active_deck: side.decks()[0],
                shift: false,
            }),
            crossfader: 0.5,
            master_level: 0.8,
            headphone_level: 0.8,
            headphone_mix: 0.0,
            headphone_master: false,
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
