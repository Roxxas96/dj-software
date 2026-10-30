//! Turns MIDI messages into actions according to a [`Mapping`].

use std::collections::HashMap;

use actions::{Action, ControlState, Input, Scope, Side, Target, ValueTarget};
use engine::DeckId;

use super::file::{InputBinding, Mapping, MidiSource, ValueKind};
use super::takeover::Takeover;
use crate::MidiMessage;

/// The actions one message resolved to, in binding order.
pub type Resolved = Vec<(Action, Input)>;

/// Resolves messages against a mapping's `[[input]]` bindings.
///
/// The active deck and SHIFT of each side are read from [`ControlState`], the single
/// source of truth: the caller applies each resolved action (including `modifier.shift`
/// and `side.deck_toggle`) before resolving the next message. The resolver keeps only
/// what belongs to it: held presses, 14-bit CC halves and soft-takeover state.
#[derive(Debug)]
pub struct Resolver {
    bindings: Vec<InputBinding>,
    /// Pressed buttons by binding index, with the action the press resolved to, so the
    /// release reaches the same deck even if the side switched or SHIFT changed.
    held: HashMap<usize, Action>,
    /// The last `(msb, lsb)` seen per 14-bit source.
    cc14: HashMap<MidiSource, (u8, u8)>,
    /// Per physical source: the value targets its last message drove and its last
    /// absolute value (before `invert`). When a SHIFT or deck layer moves the control
    /// onto a target it did not drive last time, that target is armed.
    sources: HashMap<MidiSource, SourceMemory>,
    /// The active deck per side as of the last message, to arm takeover on a switch.
    seen_active: Option<[DeckId; 2]>,
    takeover: Takeover,
}

#[derive(Debug, Clone, Default)]
struct SourceMemory {
    targets: Vec<ValueTarget>,
    value: f32,
}

/// A message's value as one binding reads it.
#[derive(Debug, Clone, Copy)]
enum Raw {
    /// Note on/off or CC, as a 7-bit value. Note off is 0.
    Seven(u8),
    /// Pitch bend or a combined 14-bit CC.
    Fourteen(u16),
}

impl Resolver {
    /// A resolver for a controller that just attached: every takeover target is armed.
    pub fn new(mapping: &Mapping) -> Self {
        Self {
            bindings: mapping.inputs.clone(),
            held: HashMap::new(),
            cc14: HashMap::new(),
            sources: HashMap::new(),
            seen_active: None,
            takeover: Takeover::new(),
        }
    }

    /// The controller (re)attached: arm takeover everywhere and forget held presses and
    /// half-received 14-bit values.
    pub fn attach(&mut self) {
        self.takeover.arm_all();
        self.held.clear();
        self.cc14.clear();
        self.sources.clear();
    }

    pub fn takeover(&self) -> &Takeover {
        &self.takeover
    }

    /// The actions `msg` drives. Every binding that matches and whose conditions hold
    /// fires, in file order; unmapped messages resolve to nothing.
    pub fn resolve(&mut self, msg: &MidiMessage, state: &ControlState) -> Resolved {
        self.observe_deck_switches(state);
        let mut resolved = Resolved::new();
        let mut driven: HashMap<MidiSource, SourceMemory> = HashMap::new();

        // A button release goes to whatever its press resolved to. If this control has
        // held presses, its other button bindings stay silent.
        let releasing_held = is_release(msg)
            && self.held.keys().any(|&i| {
                self.bindings[i].value == ValueKind::Button && matches(&self.bindings[i].midi, msg)
            });

        for i in 0..self.bindings.len() {
            let binding = self.bindings[i];
            if !matches(&binding.midi, msg) {
                continue;
            }
            // Read before the conditions so a 14-bit half is stored even when no binding
            // on its control fires.
            let raw = self.raw_value(&binding.midi, msg);
            if binding.value == ValueKind::Button && releasing_held {
                if let Some(action) = self.held.remove(&i) {
                    resolved.push((action, Input::Release));
                }
                continue;
            }
            if !conditions_hold(&binding, state) {
                continue;
            }
            let Some(action) = resolve_action(&binding, state) else {
                continue;
            };
            let input = match binding.value {
                ValueKind::Button => {
                    if button_pressed(raw) {
                        // A repeated press keeps the first one, so the release still
                        // reaches the deck that got it.
                        self.held.entry(i).or_insert(action);
                        Input::Press
                    } else {
                        Input::Release
                    }
                }
                ValueKind::Absolute => {
                    let plain = absolute(raw);
                    let flip = |v: f32| if binding.invert { 1.0 - v } else { v };
                    let v = flip(plain);
                    if let Some(target) = action.value_target() {
                        let memory = driven.entry(binding.midi).or_default();
                        memory.targets.push(target);
                        memory.value = plain;
                        if binding.soft_takeover {
                            if let Some(last) = self.sources.get(&binding.midi)
                                && !last.targets.contains(&target)
                            {
                                self.takeover.arm(target, Some(flip(last.value)));
                            }
                            if !self.take_over(target, v, state) {
                                continue;
                            }
                        }
                    }
                    Input::Absolute(v)
                }
                ValueKind::RelativeTwos | ValueKind::RelativeOffset => {
                    let n = relative(binding.value, raw);
                    Input::Relative(if binding.invert { -n } else { n })
                }
            };
            resolved.push((action, input));
        }
        self.sources.extend(driven);
        resolved
    }

    /// Arms takeover for decks that became active on their side since the last message.
    fn observe_deck_switches(&mut self, state: &ControlState) {
        let active = Side::ALL.map(|side| state.side(side).active_deck);
        if let Some(seen) = self.seen_active {
            for (now, before) in active.into_iter().zip(seen) {
                if now != before {
                    self.takeover.arm_deck(now);
                }
            }
        }
        self.seen_active = Some(active);
    }

    /// Whether the absolute value `v` for `target` passes soft takeover.
    fn take_over(&mut self, target: ValueTarget, v: f32, state: &ControlState) -> bool {
        match state.value(target) {
            Some(current) => self.takeover.filter(target, v, current),
            None => true,
        }
    }

    /// The value of `msg` for a binding on `source`, updating 14-bit CC state.
    fn raw_value(&mut self, source: &MidiSource, msg: &MidiMessage) -> Raw {
        match (*source, *msg) {
            (
                MidiSource::Cc14 { msb, .. },
                MidiMessage::ControlChange {
                    controller, value, ..
                },
            ) => {
                let halves = self.cc14.entry(*source).or_insert((0, 0));
                if controller == msb {
                    halves.0 = value;
                } else {
                    halves.1 = value;
                }
                Raw::Fourteen(u16::from(halves.0) << 7 | u16::from(halves.1))
            }
            (_, MidiMessage::NoteOn { velocity, .. }) => Raw::Seven(velocity),
            (_, MidiMessage::NoteOff { .. }) => Raw::Seven(0),
            (_, MidiMessage::ControlChange { value, .. }) => Raw::Seven(value),
            (_, MidiMessage::PitchBend { value, .. }) => Raw::Fourteen(value),
        }
    }
}

fn is_release(msg: &MidiMessage) -> bool {
    matches!(
        msg,
        MidiMessage::NoteOff { .. } | MidiMessage::ControlChange { value: 0, .. }
    )
}

/// Whether `msg` comes from `source`.
fn matches(source: &MidiSource, msg: &MidiMessage) -> bool {
    match (*source, *msg) {
        (
            MidiSource::Note { channel, note },
            MidiMessage::NoteOn {
                channel: c,
                note: n,
                ..
            }
            | MidiMessage::NoteOff {
                channel: c,
                note: n,
                ..
            },
        ) => channel == c && note == n,
        (
            MidiSource::Cc { channel, cc },
            MidiMessage::ControlChange {
                channel: c,
                controller,
                ..
            },
        ) => channel == c && cc == controller,
        (
            MidiSource::Cc14 { channel, msb, lsb },
            MidiMessage::ControlChange {
                channel: c,
                controller,
                ..
            },
        ) => channel == c && (controller == msb || controller == lsb),
        (MidiSource::PitchBend { channel }, MidiMessage::PitchBend { channel: c, .. }) => {
            channel == c
        }
        _ => false,
    }
}

/// Whether the binding's `shift` and `active_deck` conditions hold.
fn conditions_hold(binding: &InputBinding, state: &ControlState) -> bool {
    let shift_ok = binding.shift.is_none_or(|want| {
        let shift = match binding.side() {
            Some(side) => state.side(side).shift,
            None => state.sides.iter().any(|s| s.shift),
        };
        shift == want
    });
    let deck_ok = binding
        .active_deck
        .is_none_or(|deck| state.side(Side::of(deck)).active_deck == deck);
    shift_ok && deck_ok
}

/// The action a binding drives now: side bindings of deck actions follow the side's
/// active deck.
fn resolve_action(binding: &InputBinding, state: &ControlState) -> Option<Action> {
    let target = match binding.target {
        Target::Side(side) if binding.kind.scope() == Scope::Deck => {
            Target::Deck(state.side(side).active_deck)
        }
        t => t,
    };
    Action::new(binding.kind, target)
}

fn button_pressed(raw: Raw) -> bool {
    match raw {
        Raw::Seven(v) => v > 0,
        Raw::Fourteen(v) => v > 0,
    }
}

fn absolute(raw: Raw) -> f32 {
    match raw {
        Raw::Seven(v) => f32::from(v) / 127.0,
        Raw::Fourteen(v) => f32::from(v) / 16383.0,
    }
}

fn relative(kind: ValueKind, raw: Raw) -> i32 {
    match raw {
        Raw::Seven(v) => decode_relative(kind, v),
        Raw::Fourteen(_) => 0,
    }
}

/// The signed steps a relative control sent as `value`: two's complement (1..=63 forward,
/// 65..=127 backward) or offset by 64. Non-relative kinds decode to 0.
pub fn decode_relative(kind: ValueKind, value: u8) -> i32 {
    let v = i32::from(value);
    match kind {
        ValueKind::RelativeTwos if v >= 64 => v - 128,
        ValueKind::RelativeTwos => v,
        ValueKind::RelativeOffset => v - 64,
        ValueKind::Button | ValueKind::Absolute => 0,
    }
}
