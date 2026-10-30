//! The capture fixture: what each control sent, saved as TOML and committed as
//! the ground truth for a controller's mapping.
//!
//! Recording rules (see [`Control::from_messages`]):
//! - Note-offs, including note-on with velocity 0, are folded into the note-on
//!   status (`0x9n`) with value 0, so a button reads `status = 0x9n, min = 0, max = 127`.
//! - A pair whose first message in the step is a release is dropped: it belongs
//!   to a key held across the start of the step (SHIFT, a pad-mode button).
//! - Pitch bend keeps only its MSB: `data1 = 0` and the MSB as the value.
//! - `first` is the first value seen, so direction (jog, BROWSE, TEMPO) is recoverable.

use std::fmt::Write as _;

use serde::{Deserialize, Serialize};

use super::checklist::StepKind;
use crate::midi::{MidiMessage, TimedMidi};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fixture {
    pub meta: Meta,
    #[serde(rename = "control", default)]
    pub controls: Vec<Control>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Meta {
    pub controller: String,
    pub port: String,
    pub tool_version: String,
    /// Steps the user skipped (for example a knob that sends no MIDI).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skipped: Vec<String>,
}

impl Default for Meta {
    fn default() -> Self {
        Meta {
            controller: String::new(),
            port: String::new(),
            tool_version: env!("CARGO_PKG_VERSION").to_owned(),
            skipped: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Control {
    pub id: String,
    pub kind: StepKind,
    /// Distinct `(status, data1)` pairs in the order first seen.
    pub messages: Vec<MessageRange>,
    /// Set when CC `msb` (< 32) and CC `msb + 32` were both seen on one channel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cc14: Option<Cc14>,
    /// Filled by the LED probe; absent if not probed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub led: Option<bool>,
    /// Pads only: whether different velocities gave different colors.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub led_values_vary: Option<bool>,
}

/// One distinct `(status, data1)` pair with the first value and the range of
/// data2 values seen (see the module docs for note-off and pitch-bend folding).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MessageRange {
    pub status: u8,
    pub data1: u8,
    /// The first value seen in the step; optional so older fixtures still parse.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first: Option<u8>,
    pub min: u8,
    pub max: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cc14 {
    pub msb: u8,
    pub lsb: u8,
}

#[derive(Debug, thiserror::Error)]
#[error("invalid capture fixture: {0}")]
pub struct FixtureError(#[from] toml::de::Error);

pub fn parse_fixture(text: &str) -> Result<Fixture, FixtureError> {
    Ok(toml::from_str(text)?)
}

impl Fixture {
    pub fn new(meta: Meta) -> Self {
        Fixture {
            meta,
            controls: Vec::new(),
        }
    }

    pub fn control(&self, id: &str) -> Option<&Control> {
        self.controls.iter().find(|c| c.id == id)
    }

    /// Serializes in the committed layout: hex status/data1, one inline table
    /// per message with a decoded comment.
    pub fn to_toml(&self) -> String {
        // `write!` into a String cannot fail.
        let mut out = String::new();
        let meta = &self.meta;
        let _ = writeln!(out, "[meta]");
        let _ = writeln!(out, "controller = {}", quote(&meta.controller));
        let _ = writeln!(out, "port = {}", quote(&meta.port));
        let _ = writeln!(out, "tool_version = {}", quote(&meta.tool_version));
        if !meta.skipped.is_empty() {
            let list: Vec<String> = meta.skipped.iter().map(|s| quote(s)).collect();
            let _ = writeln!(out, "skipped = [{}]", list.join(", "));
        }
        for control in &self.controls {
            let _ = writeln!(out, "\n[[control]]");
            let _ = writeln!(out, "id = {}", quote(&control.id));
            let _ = writeln!(out, "kind = {}", quote(control.kind.as_str()));
            if control.messages.is_empty() {
                let _ = writeln!(out, "messages = []");
            } else {
                let _ = writeln!(out, "messages = [");
                for m in &control.messages {
                    let _ = writeln!(
                        out,
                        "  {{ status = 0x{:02X}, data1 = 0x{:02X}, {}min = {}, max = {} }}, # {}",
                        m.status,
                        m.data1,
                        m.first
                            .map(|v| format!("first = {v}, "))
                            .unwrap_or_default(),
                        m.min,
                        m.max,
                        m.describe()
                    );
                }
                let _ = writeln!(out, "]");
            }
            if let Some(cc14) = control.cc14 {
                let _ = writeln!(
                    out,
                    "cc14 = {{ msb = 0x{:02X}, lsb = 0x{:02X} }}",
                    cc14.msb, cc14.lsb
                );
            }
            if let Some(led) = control.led {
                let _ = writeln!(out, "led = {led}");
            }
            if let Some(vary) = control.led_values_vary {
                let _ = writeln!(out, "led_values_vary = {vary}");
            }
        }
        out
    }
}

impl Control {
    /// Builds a control record from the raw messages of one step, dropping
    /// pairs whose first message is a release.
    pub fn from_messages(id: &str, kind: StepKind, messages: &[TimedMidi]) -> Control {
        let mut ranges: Vec<MessageRange> = Vec::new();
        let mut released_first: Vec<(u8, u8)> = Vec::new();
        for timed in messages {
            let (status, data1, value) = key_and_value(timed.message);
            if released_first.contains(&(status, data1)) {
                continue;
            }
            match ranges
                .iter_mut()
                .find(|r| r.status == status && r.data1 == data1)
            {
                Some(range) => {
                    range.min = range.min.min(value);
                    range.max = range.max.max(value);
                }
                None if is_release(timed.message) => released_first.push((status, data1)),
                None => ranges.push(MessageRange {
                    status,
                    data1,
                    first: Some(value),
                    min: value,
                    max: value,
                }),
            }
        }
        let cc14 = ranges.iter().find_map(|msb| {
            let is_cc = msb.status & 0xF0 == 0xB0;
            let has_lsb = ranges
                .iter()
                .any(|lsb| lsb.status == msb.status && lsb.data1 == msb.data1 + 32);
            (is_cc && msb.data1 < 32 && has_lsb).then_some(Cc14 {
                msb: msb.data1,
                lsb: msb.data1 + 32,
            })
        });
        Control {
            id: id.to_owned(),
            kind,
            messages: ranges,
            cc14,
            led: None,
            led_values_vary: None,
        }
    }

    /// One-line summary such as `deck1.play: NoteOn ch1 note 11 [0..127]`.
    pub fn summary(&self) -> String {
        let parts: Vec<String> = self
            .messages
            .iter()
            .map(|m| match m.first {
                // Direction matters for moving controls; buttons always start at the press.
                Some(first) if matches!(self.kind, StepKind::Continuous | StepKind::Jog) => {
                    format!("{} [{}..{}, first {first}]", m.describe(), m.min, m.max)
                }
                _ => format!("{} [{}..{}]", m.describe(), m.min, m.max),
            })
            .collect();
        let mut line = format!("{}: {}", self.id, parts.join(", "));
        if let Some(cc14) = self.cc14 {
            let _ = write!(line, " (14-bit: MSB {} + LSB {})", cc14.msb, cc14.lsb);
        }
        line
    }
}

impl MessageRange {
    /// Decoded form with a 1-based channel, e.g. `NoteOn ch1 note 11`.
    pub fn describe(&self) -> String {
        let channel = (self.status & 0x0F) + 1;
        match self.status & 0xF0 {
            0x90 => format!("NoteOn ch{channel} note {}", self.data1),
            0xB0 => format!("ControlChange ch{channel} controller {}", self.data1),
            0xE0 => format!("PitchBend ch{channel} (MSB)"),
            _ => format!("status 0x{:02X} data1 {}", self.status, self.data1),
        }
    }
}

/// A note release: note-off, or note-on with velocity 0 (parsed as note-off).
pub(super) fn is_release(message: MidiMessage) -> bool {
    matches!(message, MidiMessage::NoteOff { .. })
}

/// `(status, data1, value)` under which a message is recorded.
pub(super) fn key_and_value(message: MidiMessage) -> (u8, u8, u8) {
    match message {
        MidiMessage::NoteOn {
            channel,
            note,
            velocity,
        } => (0x90 | (channel & 0x0F), note, velocity),
        MidiMessage::NoteOff { channel, note, .. } => (0x90 | (channel & 0x0F), note, 0),
        MidiMessage::ControlChange {
            channel,
            controller,
            value,
        } => (0xB0 | (channel & 0x0F), controller, value),
        MidiMessage::PitchBend { channel, value } => {
            (0xE0 | (channel & 0x0F), 0, ((value >> 7) & 0x7F) as u8)
        }
    }
}

/// A TOML basic string.
fn quote(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {
                let _ = write!(out, "\\u{:04X}", u32::from(c));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn timed(micros: u64, bytes: [u8; 3]) -> TimedMidi {
        TimedMidi {
            micros,
            message: MidiMessage::parse(&bytes).unwrap(),
        }
    }

    fn sample() -> Fixture {
        Fixture {
            meta: Meta {
                controller: "Pioneer DDJ-SB3".to_owned(),
                port: "DDJ-SB3".to_owned(),
                tool_version: "0.0.0".to_owned(),
                skipped: vec!["master_level".to_owned()],
            },
            controls: vec![
                Control {
                    id: "deck1.play".to_owned(),
                    kind: StepKind::Button,
                    messages: vec![MessageRange {
                        status: 0x90,
                        data1: 0x0B,
                        first: None,
                        min: 0,
                        max: 127,
                    }],
                    cc14: None,
                    led: Some(true),
                    led_values_vary: None,
                },
                Control {
                    id: "deck1.tempo".to_owned(),
                    kind: StepKind::Continuous,
                    messages: vec![
                        MessageRange {
                            status: 0xB0,
                            data1: 0x00,
                            first: None,
                            min: 0,
                            max: 127,
                        },
                        MessageRange {
                            status: 0xB0,
                            data1: 0x20,
                            first: None,
                            min: 0,
                            max: 126,
                        },
                    ],
                    cc14: Some(Cc14 {
                        msb: 0x00,
                        lsb: 0x20,
                    }),
                    led: None,
                    led_values_vary: None,
                },
                Control {
                    id: "deck1.pads \"odd\" \\ id".to_owned(),
                    kind: StepKind::Pads,
                    messages: vec![MessageRange {
                        status: 0x97,
                        data1: 0x00,
                        first: None,
                        min: 0,
                        max: 127,
                    }],
                    cc14: None,
                    led: Some(true),
                    led_values_vary: Some(false),
                },
            ],
        }
    }

    #[test]
    fn fixture_round_trips_toml() {
        let fixture = sample();
        let text = fixture.to_toml();
        assert!(text.contains("status = 0x90, data1 = 0x0B"), "{text}");
        assert!(text.contains("cc14 = { msb = 0x00, lsb = 0x20 }"), "{text}");
        assert_eq!(parse_fixture(&text).unwrap(), fixture);
    }

    #[test]
    fn fixture_parses_brief_example_and_rejects_unknown_keys() {
        let text = r#"
[meta]
controller = "Pioneer DDJ-SB3"
port = "DDJ-SB3"
tool_version = "0.0.0"
[[control]]
id = "deck1.play"
kind = "button"
messages = [{ status = 0x90, data1 = 0x0B, min = 0, max = 127 }]
led = true
led_values_vary = false
"#;
        let fixture = parse_fixture(text).unwrap();
        assert_eq!(fixture.controls[0].messages[0].status, 0x90);
        assert_eq!(fixture.controls[0].led, Some(true));
        assert!(fixture.meta.skipped.is_empty());
        assert!(parse_fixture(&text.replace("led = true", "lamp = true")).is_err());
    }

    #[test]
    fn empty_fixture_round_trips() {
        let fixture = Fixture::new(Meta::default());
        assert_eq!(parse_fixture(&fixture.to_toml()).unwrap(), fixture);
    }

    #[test]
    fn note_off_folds_into_note_on_range() {
        let control = Control::from_messages(
            "deck1.play",
            StepKind::Button,
            &[timed(0, [0x90, 0x0B, 0x7F]), timed(10, [0x90, 0x0B, 0x00])],
        );
        assert_eq!(
            control.messages,
            [MessageRange {
                status: 0x90,
                data1: 0x0B,
                first: Some(127),
                min: 0,
                max: 127
            }]
        );
        assert_eq!(control.cc14, None);
        assert_eq!(control.summary(), "deck1.play: NoteOn ch1 note 11 [0..127]");
    }

    #[test]
    fn records_first_value_for_direction() {
        // Jog turned counter-clockwise first (0x3F), then clockwise (0x41).
        let control = Control::from_messages(
            "deck1.jog_outer",
            StepKind::Jog,
            &[
                timed(0, [0xB0, 0x21, 0x3F]),
                timed(1, [0xB0, 0x21, 0x3E]),
                timed(2, [0xB0, 0x21, 0x41]),
            ],
        );
        let m = control.messages[0];
        assert_eq!((m.first, m.min, m.max), (Some(0x3F), 0x3E, 0x41));
        let fixture = Fixture {
            meta: Meta::default(),
            controls: vec![control],
        };
        let text = fixture.to_toml();
        assert!(text.contains("first = 63"), "{text}");
        assert_eq!(parse_fixture(&text).unwrap(), fixture);
    }

    #[test]
    fn from_messages_drops_pairs_starting_with_a_release() {
        let control = Control::from_messages(
            "deck1.cue",
            StepKind::Button,
            &[
                timed(0, [0x80, 0x3F, 0x40]),
                timed(1, [0x90, 0x0C, 0x7F]),
                timed(2, [0x90, 0x3F, 0x7F]),
                timed(3, [0x90, 0x0C, 0x00]),
            ],
        );
        assert_eq!(control.messages.len(), 1);
        assert_eq!(control.messages[0].data1, 0x0C);
    }
}
