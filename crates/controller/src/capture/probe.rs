//! The LED probe: light a just-captured control and ask whether a light came on.
//!
//! It runs right after a button or pads step is captured, while the controller
//! is still in the state that step needed (deck, pad mode). The logic is generic
//! over [`LedSink`] and a question callback, so it is tested without a device.

use super::checklist::{Step, StepKind};
use super::fixture::Control;
use crate::midi::MidiMessage;
use crate::ports::{OutputPort, PortError};

/// Velocities shown on lit pads to see whether the color depends on the value.
pub const PROBE_VELOCITIES: [u8; 5] = [1, 16, 32, 64, 127];

/// Where LED messages go. Unlike [`crate::MidiOut`], a failed send is reported.
pub trait LedSink {
    fn send(&mut self, msg: MidiMessage) -> Result<(), PortError>;
}

impl LedSink for OutputPort {
    fn send(&mut self, msg: MidiMessage) -> Result<(), PortError> {
        OutputPort::send(self, msg)
    }
}

/// One LED probe: every note of a control, lit at once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LedProbe {
    pub id: String,
    pub pads: bool,
    /// `(channel, note)` pairs, 0-based channel.
    pub notes: Vec<(u8, u8)>,
}

impl LedProbe {
    /// The probe for a just-captured step: buttons and pads with at least one
    /// note, never SHIFT steps (their notes are the unshifted control's LED).
    pub fn for_step(step: &Step, control: &Control) -> Option<LedProbe> {
        if step.shift || !matches!(step.kind, StepKind::Button | StepKind::Pads) {
            return None;
        }
        let notes: Vec<(u8, u8)> = control
            .messages
            .iter()
            .filter(|m| m.status & 0xF0 == 0x90)
            .map(|m| (m.status & 0x0F, m.data1))
            .collect();
        (!notes.is_empty()).then(|| LedProbe {
            id: control.id.clone(),
            pads: step.kind == StepKind::Pads,
            notes,
        })
    }

    /// Note-on messages setting every note of this control to `velocity`
    /// (0 turns them off).
    pub fn messages(&self, velocity: u8) -> Vec<MidiMessage> {
        self.notes
            .iter()
            .map(|&(channel, note)| MidiMessage::NoteOn {
                channel,
                note,
                velocity,
            })
            .collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Question {
    /// Asked with the control lit at velocity 127.
    LightOn,
    /// Asked after cycling the pads through [`PROBE_VELOCITIES`].
    ColorChanged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProbeAnswer {
    pub led: bool,
    /// Pads that lit only.
    pub values_vary: Option<bool>,
}

/// Runs one probe. `ask` returns `None` when the user quits (or input ends);
/// the probe then returns `Ok(None)`. `shown` is called after each pad velocity
/// is sent (the CLI prints it and waits). Every note is switched off again on
/// every path, including a failed send.
pub fn run_led_probe<S: LedSink>(
    sink: &mut S,
    probe: &LedProbe,
    mut ask: impl FnMut(Question) -> Option<bool>,
    mut shown: impl FnMut(u8),
) -> Result<Option<ProbeAnswer>, PortError> {
    let result = probe_steps(sink, probe, &mut ask, &mut shown);
    let off = send_all(sink, probe, 0);
    let answer = result?;
    off?;
    Ok(answer)
}

fn probe_steps<S: LedSink>(
    sink: &mut S,
    probe: &LedProbe,
    ask: &mut impl FnMut(Question) -> Option<bool>,
    shown: &mut impl FnMut(u8),
) -> Result<Option<ProbeAnswer>, PortError> {
    send_all(sink, probe, 127)?;
    let Some(led) = ask(Question::LightOn) else {
        return Ok(None);
    };
    send_all(sink, probe, 0)?;
    if !(led && probe.pads) {
        return Ok(Some(ProbeAnswer {
            led,
            values_vary: None,
        }));
    }
    for velocity in PROBE_VELOCITIES {
        send_all(sink, probe, velocity)?;
        shown(velocity);
    }
    send_all(sink, probe, 0)?;
    let Some(vary) = ask(Question::ColorChanged) else {
        return Ok(None);
    };
    Ok(Some(ProbeAnswer {
        led,
        values_vary: Some(vary),
    }))
}

/// Sends `velocity` to every note, trying all of them even if one fails.
fn send_all<S: LedSink>(sink: &mut S, probe: &LedProbe, velocity: u8) -> Result<(), PortError> {
    let mut result = Ok(());
    for msg in probe.messages(velocity) {
        if let Err(error) = sink.send(msg) {
            result = result.and(Err(error));
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::fixture::MessageRange;
    use crate::midi::TimedMidi;

    /// Records sends; fails sends with a velocity in `fail_velocities`.
    #[derive(Default)]
    struct Recorder {
        sent: Vec<MidiMessage>,
        fail_velocities: Vec<u8>,
    }

    impl LedSink for Recorder {
        fn send(&mut self, msg: MidiMessage) -> Result<(), PortError> {
            self.sent.push(msg);
            match msg {
                MidiMessage::NoteOn { velocity, .. }
                    if self.fail_velocities.contains(&velocity) =>
                {
                    Err(PortError::Send("unplugged".to_owned()))
                }
                _ => Ok(()),
            }
        }
    }

    impl Recorder {
        /// The last velocity sent to each note of `probe`.
        fn last_velocities(&self, probe: &LedProbe) -> Vec<Option<u8>> {
            probe
                .notes
                .iter()
                .map(|&(ch, n)| {
                    self.sent.iter().rev().find_map(|m| match *m {
                        MidiMessage::NoteOn {
                            channel,
                            note,
                            velocity,
                        } if channel == ch && note == n => Some(velocity),
                        _ => None,
                    })
                })
                .collect()
        }

        fn all_off(&self, probe: &LedProbe) -> bool {
            self.last_velocities(probe).iter().all(|v| *v == Some(0))
        }
    }

    fn button() -> LedProbe {
        LedProbe {
            id: "deck1.play".to_owned(),
            pads: false,
            notes: vec![(0, 0x0B)],
        }
    }

    fn pads() -> LedProbe {
        LedProbe {
            id: "deck1.pads_hot_cue".to_owned(),
            pads: true,
            notes: (0..8).map(|n| (7, n)).collect(),
        }
    }

    /// Runs a probe with scripted answers; returns the result and the questions asked.
    fn run(
        sink: &mut Recorder,
        probe: &LedProbe,
        answers: &[Option<bool>],
    ) -> (Result<Option<ProbeAnswer>, PortError>, Vec<Question>) {
        let mut answers = answers.iter().copied();
        let mut asked = Vec::new();
        let result = run_led_probe(
            sink,
            probe,
            |q| {
                asked.push(q);
                answers.next().flatten()
            },
            |_| {},
        );
        (result, asked)
    }

    #[test]
    fn button_yes_and_no_switch_off() {
        for answer in [true, false] {
            let mut sink = Recorder::default();
            let (result, asked) = run(&mut sink, &button(), &[Some(answer)]);
            assert_eq!(
                result.unwrap(),
                Some(ProbeAnswer {
                    led: answer,
                    values_vary: None
                })
            );
            assert_eq!(asked, [Question::LightOn]);
            assert_eq!(
                sink.sent[0],
                MidiMessage::NoteOn {
                    channel: 0,
                    note: 0x0B,
                    velocity: 127
                }
            );
            assert!(sink.all_off(&button()));
        }
    }

    #[test]
    fn pads_light_all_at_once_and_cycle_velocities() {
        let probe = pads();
        let mut sink = Recorder::default();
        let mut shown = Vec::new();
        let mut answers = [Some(true), Some(true)].into_iter();
        let result = run_led_probe(
            &mut sink,
            &probe,
            |_| answers.next().flatten(),
            |v| shown.push(v),
        );
        assert_eq!(
            result.unwrap(),
            Some(ProbeAnswer {
                led: true,
                values_vary: Some(true)
            })
        );
        assert_eq!(shown, PROBE_VELOCITIES);
        // The first 8 messages light every pad at 127 before the first question.
        assert!(
            sink.sent[..8]
                .iter()
                .all(|m| matches!(m, MidiMessage::NoteOn { velocity: 127, .. }))
        );
        assert!(sink.all_off(&probe));
    }

    #[test]
    fn unlit_pads_skip_the_color_question() {
        let mut sink = Recorder::default();
        let (result, asked) = run(&mut sink, &pads(), &[Some(false)]);
        assert_eq!(result.unwrap().unwrap().values_vary, None);
        assert_eq!(asked, [Question::LightOn]);
        assert!(sink.all_off(&pads()));
    }

    #[test]
    fn quit_or_end_of_input_switches_off() {
        // Quit at the first question, and at the color question (input ends).
        for answers in [vec![None], vec![Some(true)]] {
            let mut sink = Recorder::default();
            let (result, _) = run(&mut sink, &pads(), &answers);
            assert_eq!(result.unwrap(), None);
            assert!(sink.all_off(&pads()));
        }
    }

    #[test]
    fn send_error_still_switches_off() {
        // Lighting fails: no question is asked, every note still gets velocity 0.
        let mut sink = Recorder {
            fail_velocities: vec![127],
            ..Recorder::default()
        };
        let (result, asked) = run(&mut sink, &pads(), &[Some(true)]);
        assert!(result.is_err());
        assert!(asked.is_empty());
        assert!(sink.all_off(&pads()));

        // A failure in the middle of the velocity cycle.
        let mut sink = Recorder {
            fail_velocities: vec![32],
            ..Recorder::default()
        };
        let (result, _) = run(&mut sink, &pads(), &[Some(true), Some(true)]);
        assert!(result.is_err());
        assert!(sink.all_off(&pads()));
    }

    fn step(id: &str, kind: StepKind, shift: bool) -> Step {
        Step {
            id: id.to_owned(),
            section: "S".to_owned(),
            prompt: "p".to_owned(),
            setup: None,
            prepare: None,
            kind,
            shift,
        }
    }

    fn control(id: &str, kind: StepKind, bytes: &[[u8; 3]]) -> Control {
        let messages: Vec<TimedMidi> = bytes
            .iter()
            .map(|b| TimedMidi {
                micros: 0,
                message: MidiMessage::parse(b).unwrap(),
            })
            .collect();
        Control::from_messages(id, kind, &messages)
    }

    #[test]
    fn probes_only_unshifted_buttons_and_pads_with_notes() {
        let play = control("deck1.play", StepKind::Button, &[[0x90, 0x0B, 0x7F]]);
        let probe =
            LedProbe::for_step(&step("deck1.play", StepKind::Button, false), &play).unwrap();
        assert_eq!(probe.notes, [(0, 0x0B)]);
        assert!(!probe.pads);
        assert_eq!(
            LedProbe::for_step(&step("deck1.play.shift", StepKind::Button, true), &play),
            None
        );

        let fader = control("channel1.fader", StepKind::Continuous, &[[0xB0, 0x13, 5]]);
        let fader_step = step("channel1.fader", StepKind::Continuous, false);
        assert_eq!(LedProbe::for_step(&fader_step, &fader), None);

        // A button that sends a CC has no note to light.
        let cc_button = control("x", StepKind::Button, &[[0xB0, 0x40, 0x7F]]);
        assert_eq!(
            LedProbe::for_step(&step("x", StepKind::Button, false), &cc_button),
            None
        );

        let pad_control = Control {
            messages: (0..8)
                .map(|n| MessageRange {
                    status: 0x97,
                    data1: n,
                    first: Some(127),
                    min: 0,
                    max: 127,
                })
                .collect(),
            ..control("deck1.pads", StepKind::Pads, &[])
        };
        let probe =
            LedProbe::for_step(&step("deck1.pads", StepKind::Pads, false), &pad_control).unwrap();
        assert!(probe.pads);
        assert_eq!(probe.notes.len(), 8);
    }
}
