use engine::{DeckId, HotcueSlot};

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
    JogTop,
    JogRing,
    JogSearch,
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

impl ActionKind {
    /// Every kind: both hotcue families once per slot.
    pub const ALL: [ActionKind; 57] = {
        let mut all = [ActionKind::PlayPause; 57];
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
            ActionKind::JogTop,
            ActionKind::JogRing,
            ActionKind::JogSearch,
        ];
        let mut i = 0;
        while i < fixed.len() {
            all[i] = fixed[i];
            i += 1;
        }
        let mut slot = 0;
        while slot < HotcueSlot::ALL.len() {
            all[fixed.len() + slot] = ActionKind::Hotcue(HotcueSlot::ALL[slot]);
            all[fixed.len() + 8 + slot] = ActionKind::HotcueDelete(HotcueSlot::ALL[slot]);
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
            ActionKind::JogTop => "deck.jog_top",
            ActionKind::JogRing => "deck.jog_ring",
            ActionKind::JogSearch => "deck.jog_search",
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
            | ActionKind::JogTop
            | ActionKind::JogRing
            | ActionKind::JogSearch
            | ActionKind::Hotcue(_)
            | ActionKind::HotcueDelete(_)
            | ActionKind::LoadSelected => Scope::Deck,
            ActionKind::Crossfader
            | ActionKind::MasterLevel
            | ActionKind::HeadphoneLevel
            | ActionKind::HeadphoneMix
            | ActionKind::HeadphoneMasterToggle
            | ActionKind::BrowserScroll
            | ActionKind::BrowserToggleFocus => Scope::Global,
            ActionKind::DeckToggle | ActionKind::Shift => Scope::Side,
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

    /// Whether this action reacts to [`Input::Release`] as well as [`Input::Press`].
    pub fn takes_release(self) -> bool {
        matches!(
            self,
            ActionKind::Cue | ActionKind::Shift | ActionKind::Hotcue(_) | ActionKind::JogTouch
        )
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
    JogTop(DeckId),
    JogRing(DeckId),
    JogSearch(DeckId),
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
            (K::JogTop, Target::Deck(d)) => Action::JogTop(d),
            (K::JogRing, Target::Deck(d)) => Action::JogRing(d),
            (K::JogSearch, Target::Deck(d)) => Action::JogSearch(d),
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
            Action::JogTop(_) => ActionKind::JogTop,
            Action::JogRing(_) => ActionKind::JogRing,
            Action::JogSearch(_) => ActionKind::JogSearch,
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
            | Action::JogTop(d)
            | Action::JogRing(d)
            | Action::JogSearch(d) => Target::Deck(d),
            Action::Crossfader
            | Action::MasterLevel
            | Action::HeadphoneLevel
            | Action::HeadphoneMix
            | Action::HeadphoneMasterToggle
            | Action::BrowserScroll
            | Action::BrowserToggleFocus => Target::Global,
            Action::DeckToggle(s) | Action::Shift(s) => Target::Side(s),
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
