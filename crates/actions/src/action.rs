use engine::DeckId;

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
}

impl ActionKind {
    pub const ALL: [ActionKind; 19] = [
        ActionKind::PlayPause,
        ActionKind::Cue,
        ActionKind::Tempo,
        ActionKind::TempoRangeNext,
        ActionKind::ReturnToStart,
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
    ];

    /// The stable id used in mapping files.
    pub fn id(self) -> &'static str {
        match self {
            ActionKind::PlayPause => "deck.play_pause",
            ActionKind::Cue => "deck.cue",
            ActionKind::Tempo => "deck.tempo",
            ActionKind::TempoRangeNext => "deck.tempo_range_next",
            ActionKind::ReturnToStart => "deck.return_to_start",
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
            | ActionKind::Trim
            | ActionKind::EqHigh
            | ActionKind::EqMid
            | ActionKind::EqLow
            | ActionKind::Filter
            | ActionKind::Fader
            | ActionKind::PflToggle => Scope::Deck,
            ActionKind::Crossfader
            | ActionKind::MasterLevel
            | ActionKind::HeadphoneLevel
            | ActionKind::HeadphoneMix
            | ActionKind::HeadphoneMasterToggle => Scope::Global,
            ActionKind::DeckToggle | ActionKind::Shift => Scope::Side,
        }
    }

    /// Whether this action accepts inputs of `kind`. This is the one table of what each
    /// action takes: absolute actions take positions, every other action takes button
    /// presses, and no action takes relative steps yet.
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
        match kind {
            InputKind::Button => !absolute,
            InputKind::Absolute => absolute,
            InputKind::Relative => false,
        }
    }

    /// Whether this action takes [`Input::Absolute`] values (and so has a [`ValueTarget`]).
    pub fn is_absolute(self) -> bool {
        self.accepts(InputKind::Absolute)
    }

    /// Whether this action reacts to [`Input::Release`] as well as [`Input::Press`].
    pub fn takes_release(self) -> bool {
        matches!(self, ActionKind::Cue | ActionKind::Shift)
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
        }
    }

    pub fn target(&self) -> Target {
        match *self {
            Action::PlayPause(d)
            | Action::Cue(d)
            | Action::Tempo(d)
            | Action::TempoRangeNext(d)
            | Action::ReturnToStart(d)
            | Action::Trim(d)
            | Action::EqHigh(d)
            | Action::EqMid(d)
            | Action::EqLow(d)
            | Action::Filter(d)
            | Action::Fader(d)
            | Action::PflToggle(d) => Target::Deck(d),
            Action::Crossfader
            | Action::MasterLevel
            | Action::HeadphoneLevel
            | Action::HeadphoneMix
            | Action::HeadphoneMasterToggle => Target::Global,
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
