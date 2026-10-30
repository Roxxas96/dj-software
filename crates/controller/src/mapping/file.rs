//! The mapping file: TOML with `[controller]`, `[[init]]`, `[[input]]` and `[[output]]`.
//!
//! Parsing happens in two passes. The file's top level is read first, keeping each binding
//! as a raw TOML value, then each binding is read and validated on its own so every error
//! names the file, the binding (`input[N]`, 0-based) and the problem.

use std::fmt;

use actions::{Action, ActionKind, InputKind, Scope, Side, StateKey, Target};
use engine::DeckId;
use serde::Deserialize;

/// A parsed and validated mapping. MIDI channels are 0-based here, 1-based in the file.
#[derive(Debug, Clone, PartialEq)]
pub struct Mapping {
    pub controller: ControllerInfo,
    /// Applied in order when the controller attaches.
    pub init: Vec<InitAction>,
    /// In file order, which is the order bindings fire in.
    pub inputs: Vec<InputBinding>,
    pub outputs: Vec<OutputBinding>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControllerInfo {
    pub name: String,
    /// Substring matched against both input and output port names.
    pub port: String,
}

/// An absolute action set to `value` when the controller attaches.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InitAction {
    pub action: Action,
    pub value: f32,
}

/// What a binding listens to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MidiSource {
    Note {
        channel: u8,
        note: u8,
    },
    Cc {
        channel: u8,
        cc: u8,
    },
    /// A 14-bit value split over two CCs.
    Cc14 {
        channel: u8,
        msb: u8,
        lsb: u8,
    },
    PitchBend {
        channel: u8,
    },
}

/// How a binding reads its message's value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueKind {
    /// Note on (or CC above 0) presses, note off (or CC 0) releases.
    Button,
    /// A position in `[0, 1]`.
    Absolute,
    /// Two's complement steps: 1..=63 forward, 65..=127 backward.
    RelativeTwos,
    /// Steps offset by 64: 65 is +1, 63 is -1.
    RelativeOffset,
}

/// One `[[input]]`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InputBinding {
    pub midi: MidiSource,
    pub kind: ActionKind,
    /// `Deck` for a fixed deck, `Side` for a side (deck actions resolve it to the side's
    /// active deck), `Global` for global actions.
    pub target: Target,
    /// Condition on SHIFT of the binding's side (any side for global bindings).
    pub shift: Option<bool>,
    /// Condition: the side `Side::of(deck)` currently shows this deck.
    pub active_deck: Option<DeckId>,
    pub value: ValueKind,
    /// Absolute values become `1 - v`; relative steps are negated.
    pub invert: bool,
    /// Only ever true for absolute bindings.
    pub soft_takeover: bool,
}

impl InputBinding {
    /// The side whose SHIFT and active deck this binding's conditions read, if any.
    pub fn side(&self) -> Option<Side> {
        side_of(self.target)
    }
}

/// Where an output sends its LED value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OutputMidi {
    Note { channel: u8, note: u8 },
    Cc { channel: u8, cc: u8 },
}

/// One `[[output]]`: an LED following a [`StateKey`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutputBinding {
    pub state: StateKey,
    /// `None` for global state.
    pub target: Option<Target>,
    pub midi: OutputMidi,
    pub on: u8,
    pub off: u8,
}

/// A mapping file that failed to load: `"<file>: <binding>: <problem>"`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{file}: {message}")]
pub struct MappingError {
    pub file: String,
    pub message: String,
}

/// Parses and validates the mapping file `name` (used in error messages) from `text`.
pub fn parse_mapping(name: &str, text: &str) -> Result<Mapping, MappingError> {
    let error = |message: String| MappingError {
        file: name.to_owned(),
        message,
    };
    let raw: RawFile = toml::from_str(text).map_err(|e| error(e.to_string()))?;
    Ok(Mapping {
        controller: ControllerInfo {
            name: raw.controller.name,
            port: raw.controller.port,
        },
        init: each(raw.init, "init", init_action).map_err(error)?,
        inputs: each(raw.input, "input", input_binding).map_err(error)?,
        outputs: each(raw.output, "output", output_binding).map_err(error)?,
    })
}

/// Reads and validates every raw binding of one array, prefixing errors with `what[N]`.
fn each<R, T>(
    values: Vec<toml::Value>,
    what: &str,
    validate: impl Fn(R) -> Result<T, String>,
) -> Result<Vec<T>, String>
where
    R: for<'de> Deserialize<'de>,
{
    values
        .into_iter()
        .enumerate()
        .map(|(i, value)| {
            value
                .try_into::<R>()
                .map_err(|e| e.to_string().trim_end().to_owned())
                .and_then(&validate)
                .map_err(|e| format!("{what}[{i}]: {e}"))
        })
        .collect()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFile {
    controller: RawController,
    #[serde(default)]
    init: Vec<toml::Value>,
    #[serde(default)]
    input: Vec<toml::Value>,
    #[serde(default)]
    output: Vec<toml::Value>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawController {
    name: String,
    port: String,
}

/// A TOML number: `value = 1` and `value = 1.0` both work.
#[derive(Deserialize)]
#[serde(untagged)]
enum RawNumber {
    Int(i64),
    Float(f64),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawInit {
    action: String,
    deck: Option<i64>,
    value: Option<RawNumber>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawMidi {
    channel: i64,
    note: Option<i64>,
    cc: Option<i64>,
    cc14: Option<[i64; 2]>,
    pitchbend: Option<bool>,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum RawSide {
    Left,
    Right,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum RawValueKind {
    Button,
    Absolute,
    RelativeTwos,
    RelativeOffset,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawInput {
    midi: RawMidi,
    action: String,
    deck: Option<i64>,
    side: Option<RawSide>,
    shift: Option<bool>,
    active_deck: Option<i64>,
    value: Option<RawValueKind>,
    #[serde(default)]
    invert: bool,
    soft_takeover: Option<bool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawOutput {
    state: String,
    deck: Option<i64>,
    side: Option<RawSide>,
    midi: RawMidi,
    on: Option<i64>,
    off: Option<i64>,
}

fn init_action(raw: RawInit) -> Result<InitAction, String> {
    let kind = action_kind(&raw.action)?;
    let target = match raw.deck {
        Some(n) => Target::Deck(deck(n)?),
        None => Target::Global,
    };
    let action = Action::new(kind, target).ok_or_else(|| match target {
        Target::Global => format!("action `{}` needs `deck`", raw.action),
        _ => format!("action `{}` takes no `deck`", raw.action),
    })?;
    if !kind.is_absolute() {
        return Err(format!("action `{}` does not take a value", raw.action));
    }
    let value = match raw.value {
        None => return Err("`value` is missing".into()),
        Some(RawNumber::Int(v)) => v as f64,
        Some(RawNumber::Float(v)) => v,
    };
    if !(0.0..=1.0).contains(&value) {
        return Err(format!("value {value} is outside 0-1"));
    }
    Ok(InitAction {
        action,
        value: value as f32,
    })
}

fn input_binding(raw: RawInput) -> Result<InputBinding, String> {
    let kind = action_kind(&raw.action)?;
    let midi = midi_source(&raw.midi)?;
    let target = scoped_target(
        kind.scope(),
        &format!("action `{}`", raw.action),
        raw.deck,
        raw.side,
    )?;
    let value = match raw.value {
        Some(RawValueKind::Button) => ValueKind::Button,
        Some(RawValueKind::Absolute) => ValueKind::Absolute,
        Some(RawValueKind::RelativeTwos) => ValueKind::RelativeTwos,
        Some(RawValueKind::RelativeOffset) => ValueKind::RelativeOffset,
        None => match midi {
            MidiSource::Note { .. } => ValueKind::Button,
            _ => ValueKind::Absolute,
        },
    };
    if !kind.accepts(value.input_kind()) {
        let what = match value.input_kind() {
            InputKind::Button => "button",
            InputKind::Absolute => "absolute",
            InputKind::Relative => "relative",
        };
        return Err(format!(
            "action `{}` doesn't accept {what} values",
            raw.action
        ));
    }
    let fits_midi = match value {
        ValueKind::Button => matches!(midi, MidiSource::Note { .. } | MidiSource::Cc { .. }),
        ValueKind::Absolute => true,
        ValueKind::RelativeTwos | ValueKind::RelativeOffset => {
            matches!(midi, MidiSource::Cc { .. })
        }
    };
    if !fits_midi {
        return Err(format!("value `{value}` does not fit {}", midi.kind_name()));
    }
    let active_deck = raw.active_deck.map(deck).transpose()?;
    if let (Some(ad), Some(side)) = (active_deck, side_of(target))
        && Side::of(ad) != side
    {
        return Err(format!(
            "active_deck {} is not on the binding's side",
            ad.index() + 1
        ));
    }
    Ok(InputBinding {
        midi,
        kind,
        target,
        shift: raw.shift,
        active_deck,
        value,
        invert: raw.invert,
        soft_takeover: value == ValueKind::Absolute && raw.soft_takeover.unwrap_or(true),
    })
}

fn output_binding(raw: RawOutput) -> Result<OutputBinding, String> {
    let state =
        StateKey::from_id(&raw.state).ok_or_else(|| format!("unknown state `{}`", raw.state))?;
    let target = match scoped_target(
        state.scope(),
        &format!("state `{}`", raw.state),
        raw.deck,
        raw.side,
    )? {
        Target::Global => None,
        t => Some(t),
    };
    let midi = match midi_source(&raw.midi)? {
        MidiSource::Note { channel, note } => OutputMidi::Note { channel, note },
        MidiSource::Cc { channel, cc } => OutputMidi::Cc { channel, cc },
        other => {
            return Err(format!(
                "outputs take a note or cc, not {}",
                other.kind_name()
            ));
        }
    };
    Ok(OutputBinding {
        state,
        target,
        midi,
        on: raw.on.map_or(Ok(127), |v| midi_number("on", v))?,
        off: raw.off.map_or(Ok(0), |v| midi_number("off", v))?,
    })
}

fn action_kind(id: &str) -> Result<ActionKind, String> {
    ActionKind::from_id(id).ok_or_else(|| format!("unknown action `{id}`"))
}

/// The target named by `deck`/`side`, checked against `scope`; `what` names the action
/// or state in errors. Deck scopes take a deck or a side (read through its active deck).
fn scoped_target(
    scope: Scope,
    what: &str,
    deck_n: Option<i64>,
    raw_side: Option<RawSide>,
) -> Result<Target, String> {
    let target = match (deck_n, raw_side) {
        (Some(_), Some(_)) => return Err("`deck` and `side` are both set".into()),
        (Some(n), None) => Some(Target::Deck(deck(n)?)),
        (None, Some(s)) => Some(Target::Side(side(s))),
        (None, None) => None,
    };
    match (scope, target) {
        (Scope::Deck, Some(t)) => Ok(t),
        (Scope::Deck, None) => Err(format!("{what} needs `deck` or `side`")),
        (Scope::Side, Some(t @ Target::Side(_))) => Ok(t),
        (Scope::Side, _) => Err(format!("{what} needs `side`")),
        (Scope::Global, None) => Ok(Target::Global),
        (Scope::Global, Some(_)) => Err(format!("{what} takes no `deck` or `side`")),
    }
}

fn side_of(target: Target) -> Option<Side> {
    match target {
        Target::Deck(d) => Some(Side::of(d)),
        Target::Side(s) => Some(s),
        Target::Global => None,
    }
}

fn side(raw: RawSide) -> Side {
    match raw {
        RawSide::Left => Side::Left,
        RawSide::Right => Side::Right,
    }
}

/// A 1-based deck number from the file.
fn deck(n: i64) -> Result<DeckId, String> {
    n.checked_sub(1)
        .and_then(|i| usize::try_from(i).ok())
        .and_then(DeckId::new)
        .ok_or_else(|| format!("deck {n} is outside 1-4"))
}

fn midi_source(raw: &RawMidi) -> Result<MidiSource, String> {
    let channel = raw
        .channel
        .checked_sub(1)
        .and_then(|c| u8::try_from(c).ok())
        .filter(|c| *c < 16)
        .ok_or_else(|| format!("channel {} is outside 1-16", raw.channel))?;
    let source = match (raw.note, raw.cc, raw.cc14, raw.pitchbend) {
        (Some(note), None, None, None) => MidiSource::Note {
            channel,
            note: midi_number("note", note)?,
        },
        (None, Some(cc), None, None) => MidiSource::Cc {
            channel,
            cc: midi_number("cc", cc)?,
        },
        (None, None, Some([msb, lsb]), None) => {
            let msb = midi_number("cc14 msb", msb)?;
            let lsb = midi_number("cc14 lsb", lsb)?;
            if msb == lsb {
                return Err(format!("cc14 msb and lsb are both {msb}"));
            }
            MidiSource::Cc14 { channel, msb, lsb }
        }
        (None, None, None, Some(true)) => MidiSource::PitchBend { channel },
        _ => {
            return Err(
                "`midi` needs exactly one of `note`, `cc`, `cc14` or `pitchbend = true`".into(),
            );
        }
    };
    Ok(source)
}

/// A 7-bit MIDI number.
fn midi_number(what: &str, n: i64) -> Result<u8, String> {
    match u8::try_from(n) {
        Ok(v) if v <= 127 => Ok(v),
        _ if n < 0 => Err(format!("{what} {n} is negative")),
        _ => Err(format!("{what} {n} is above 127")),
    }
}

impl MidiSource {
    fn kind_name(&self) -> &'static str {
        match self {
            MidiSource::Note { .. } => "a note",
            MidiSource::Cc { .. } => "a cc",
            MidiSource::Cc14 { .. } => "a cc14",
            MidiSource::PitchBend { .. } => "pitch bend",
        }
    }
}

impl ValueKind {
    /// The kind of [`actions::Input`] this value kind produces.
    pub fn input_kind(self) -> InputKind {
        match self {
            ValueKind::Button => InputKind::Button,
            ValueKind::Absolute => InputKind::Absolute,
            ValueKind::RelativeTwos | ValueKind::RelativeOffset => InputKind::Relative,
        }
    }
}

impl fmt::Display for ValueKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            ValueKind::Button => "button",
            ValueKind::Absolute => "absolute",
            ValueKind::RelativeTwos => "relative_twos",
            ValueKind::RelativeOffset => "relative_offset",
        })
    }
}
