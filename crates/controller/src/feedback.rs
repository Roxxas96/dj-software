//! LED feedback: evaluates every `[[output]]` of a mapping and sends only what changed.
//!
//! A `note` output is sent as NoteOn with velocity `on` or `off`. The off state is a NoteOn
//! with velocity `off` (not a NoteOff) because many controllers, including the targeted one,
//! expect that. A `cc` output is sent as ControlChange.

use actions::{ControlState, Led, Scope, Target, led};
use engine::DeckSnapshot;

use crate::MidiMessage;
use crate::MidiOut;
use crate::mapping::{Mapping, OutputBinding, OutputMidi};

/// Full blink period; LEDs are lit during the first half.
const BLINK_PERIOD_MICROS: u64 = 500_000;

/// Drives the LEDs of one mapping.
#[derive(Debug)]
pub struct Feedback {
    outputs: Vec<OutputBinding>,
    /// Last value sent per output; `None` before the first send or after a resync.
    sent: Vec<Option<u8>>,
}

impl Feedback {
    pub fn new(mapping: &Mapping) -> Self {
        Self {
            outputs: mapping.outputs.clone(),
            sent: vec![None; mapping.outputs.len()],
        }
    }

    /// Sends the outputs whose value changed since the last update.
    ///
    /// All blinking LEDs share one clock (`now_micros`), so they stay in phase.
    pub fn update(
        &mut self,
        now_micros: u64,
        state: &ControlState,
        snaps: &[DeckSnapshot; 4],
        out: &mut dyn MidiOut,
    ) {
        let blink_on = now_micros % BLINK_PERIOD_MICROS < BLINK_PERIOD_MICROS / 2;
        for (binding, sent) in self.outputs.iter().zip(self.sent.iter_mut()) {
            // A side target is resolved to the side's active deck once, so the target and
            // the snapshot always describe the same deck.
            let (target, snap) = match binding.target {
                Some(Target::Side(side)) => {
                    let deck = state.side(side).active_deck;
                    let target = if binding.state.scope() == Scope::Side {
                        Target::Side(side)
                    } else {
                        Target::Deck(deck)
                    };
                    (Some(target), Some(&snaps[deck.index()]))
                }
                Some(Target::Deck(deck)) => (Some(Target::Deck(deck)), Some(&snaps[deck.index()])),
                other => (other, None),
            };
            let lit = match led(binding.state, target, state, snap) {
                Led::Off => false,
                Led::On => true,
                Led::Blink => blink_on,
            };
            let value = if lit { binding.on } else { binding.off };
            if *sent == Some(value) {
                continue;
            }
            *sent = Some(value);
            out.send(match binding.midi {
                OutputMidi::Note { channel, note } => MidiMessage::NoteOn {
                    channel,
                    note,
                    velocity: value,
                },
                OutputMidi::Cc { channel, cc } => MidiMessage::ControlChange {
                    channel,
                    controller: cc,
                    value,
                },
            });
        }
    }

    /// Forgets what was sent, so the next [`update`](Self::update) sends every output
    /// (after a reconnect, or when the controller's LEDs may have been reset).
    pub fn resync(&mut self) {
        self.sent.fill(None);
    }
}
