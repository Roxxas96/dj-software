//! Channel-voice MIDI messages: parsing, encoding and display.

use std::fmt;

/// A MIDI channel-voice message. Channels are 0-based (0..=15) in code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MidiMessage {
    NoteOn {
        channel: u8,
        note: u8,
        velocity: u8,
    },
    NoteOff {
        channel: u8,
        note: u8,
        velocity: u8,
    },
    ControlChange {
        channel: u8,
        controller: u8,
        value: u8,
    },
    /// 14-bit value, 8192 is the centre.
    PitchBend {
        channel: u8,
        value: u16,
    },
}

/// A message with the backend's timestamp in microseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimedMidi {
    pub micros: u64,
    pub message: MidiMessage,
}

impl MidiMessage {
    /// Parses one message. A note-on with velocity 0 becomes a [`MidiMessage::NoteOff`].
    /// SysEx, realtime, truncated, malformed and unsupported messages return `None`.
    pub fn parse(bytes: &[u8]) -> Option<MidiMessage> {
        let &[status, d1, d2] = bytes else {
            return None;
        };
        if d1 >= 0x80 || d2 >= 0x80 {
            return None;
        }
        let channel = status & 0x0F;
        match status & 0xF0 {
            0x80 => Some(MidiMessage::NoteOff {
                channel,
                note: d1,
                velocity: d2,
            }),
            0x90 if d2 == 0 => Some(MidiMessage::NoteOff {
                channel,
                note: d1,
                velocity: 0,
            }),
            0x90 => Some(MidiMessage::NoteOn {
                channel,
                note: d1,
                velocity: d2,
            }),
            0xB0 => Some(MidiMessage::ControlChange {
                channel,
                controller: d1,
                value: d2,
            }),
            0xE0 => Some(MidiMessage::PitchBend {
                channel,
                value: u16::from(d1) | (u16::from(d2) << 7),
            }),
            _ => None,
        }
    }

    /// The three wire bytes of this message.
    pub fn encode(&self) -> [u8; 3] {
        match *self {
            MidiMessage::NoteOn {
                channel,
                note,
                velocity,
            } => [0x90 | (channel & 0x0F), note, velocity],
            MidiMessage::NoteOff {
                channel,
                note,
                velocity,
            } => [0x80 | (channel & 0x0F), note, velocity],
            MidiMessage::ControlChange {
                channel,
                controller,
                value,
            } => [0xB0 | (channel & 0x0F), controller, value],
            MidiMessage::PitchBend { channel, value } => [
                0xE0 | (channel & 0x0F),
                (value & 0x7F) as u8,
                ((value >> 7) & 0x7F) as u8,
            ],
        }
    }
}

impl fmt::Display for MidiMessage {
    /// Hex bytes then a decoded form with a 1-based channel, e.g.
    /// `90 0B 7F  NoteOn ch1 note 11 vel 127`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let [a, b, c] = self.encode();
        write!(f, "{a:02X} {b:02X} {c:02X}  ")?;
        match *self {
            MidiMessage::NoteOn {
                channel,
                note,
                velocity,
            } => {
                write!(f, "NoteOn ch{} note {note} vel {velocity}", channel + 1)
            }
            MidiMessage::NoteOff {
                channel,
                note,
                velocity,
            } => {
                write!(f, "NoteOff ch{} note {note} vel {velocity}", channel + 1)
            }
            MidiMessage::ControlChange {
                channel,
                controller,
                value,
            } => {
                write!(
                    f,
                    "ControlChange ch{} controller {controller} value {value}",
                    channel + 1
                )
            }
            MidiMessage::PitchBend { channel, value } => {
                write!(f, "PitchBend ch{} value {value}", channel + 1)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_note_on_off_cc_pitchbend() {
        assert_eq!(
            MidiMessage::parse(&[0x90, 0x0B, 0x7F]),
            Some(MidiMessage::NoteOn {
                channel: 0,
                note: 11,
                velocity: 127
            })
        );
        assert_eq!(
            MidiMessage::parse(&[0x85, 0x20, 0x10]),
            Some(MidiMessage::NoteOff {
                channel: 5,
                note: 32,
                velocity: 16
            })
        );
        assert_eq!(
            MidiMessage::parse(&[0xBF, 0x07, 0x64]),
            Some(MidiMessage::ControlChange {
                channel: 15,
                controller: 7,
                value: 100
            })
        );
        assert_eq!(
            MidiMessage::parse(&[0xE0, 0x00, 0x40]),
            Some(MidiMessage::PitchBend {
                channel: 0,
                value: 8192
            })
        );
        assert_eq!(
            MidiMessage::parse(&[0xE3, 0x7F, 0x7F]),
            Some(MidiMessage::PitchBend {
                channel: 3,
                value: 16383
            })
        );
    }

    #[test]
    fn note_on_velocity_zero_is_note_off() {
        assert_eq!(
            MidiMessage::parse(&[0x91, 0x30, 0x00]),
            Some(MidiMessage::NoteOff {
                channel: 1,
                note: 0x30,
                velocity: 0
            })
        );
    }

    #[test]
    fn parse_rejects_malformed_and_sysex() {
        assert_eq!(MidiMessage::parse(&[]), None);
        assert_eq!(MidiMessage::parse(&[0x90, 0x10]), None);
        assert_eq!(MidiMessage::parse(&[0xF0, 1, 2, 0xF7]), None);
        assert_eq!(MidiMessage::parse(&[0xF8]), None);
        assert_eq!(MidiMessage::parse(&[0x90, 0x80, 0x10]), None);
        assert_eq!(MidiMessage::parse(&[0x90, 0x10, 0x80]), None);
        assert_eq!(MidiMessage::parse(&[0xC0, 0x10, 0x00]), None);
    }

    #[test]
    fn encode_round_trips() {
        for bytes in [
            [0x90, 0x0B, 0x7F],
            [0x80, 0x0B, 0x40],
            [0xB2, 0x13, 0x01],
            [0xE1, 0x05, 0x7A],
        ] {
            let msg = MidiMessage::parse(&bytes).unwrap();
            assert_eq!(msg.encode(), bytes);
        }
    }

    #[test]
    fn display_uses_one_based_channel() {
        let msg = MidiMessage::NoteOn {
            channel: 0,
            note: 11,
            velocity: 127,
        };
        assert_eq!(msg.to_string(), "90 0B 7F  NoteOn ch1 note 11 vel 127");
        let msg = MidiMessage::ControlChange {
            channel: 15,
            controller: 7,
            value: 100,
        };
        assert_eq!(
            msg.to_string(),
            "BF 07 64  ControlChange ch16 controller 7 value 100"
        );
    }
}
