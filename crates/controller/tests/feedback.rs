use std::sync::Arc;

use actions::{Action, ActionKind, ControlState, Input, Side, Target, apply};
use controller::mapping::parse_mapping;
use controller::{Feedback, MidiMessage, MidiOut};
use engine::{Command, DeckCommand, DeckId, DeckSnapshot, EngineConfig, TrackBuffer, engine};

#[derive(Default)]
struct Recorder(Vec<MidiMessage>);

impl MidiOut for Recorder {
    fn send(&mut self, msg: MidiMessage) {
        self.0.push(msg);
    }
}

impl Recorder {
    fn take(&mut self) -> Vec<MidiMessage> {
        std::mem::take(&mut self.0)
    }
}

const MAP: &str = r#"
[controller]
name = "X"
port = "X"

[[output]]
state = "deck.play_led"
deck = 1
midi = { note = 11, channel = 1 }
on = 127
off = 0

[[output]]
state = "deck.cue_led"
deck = 1
midi = { note = 12, channel = 1 }
on = 100
off = 1

[[output]]
state = "channel.pfl"
deck = 1
midi = { cc = 5, channel = 2 }
on = 64
off = 2
"#;

const SIDE_MAP: &str = r#"
[controller]
name = "X"
port = "X"

[[output]]
state = "deck.play_led"
side = "left"
midi = { note = 11, channel = 1 }
on = 127
off = 0
"#;

fn d(i: usize) -> DeckId {
    DeckId::new(i).unwrap()
}

fn snap(loaded: bool, playing: bool, position: f64, cue: f64) -> DeckSnapshot {
    DeckSnapshot {
        loaded,
        playing,
        position_frames: position,
        duration_frames: 100_000,
        track_sample_rate: 44_100,
        tempo_ratio: 1.0,
        cue_frames: cue,
        previewing: false,
    }
}

fn idle() -> [DeckSnapshot; 4] {
    [snap(false, false, 0.0, 0.0); 4]
}

fn note(note: u8, velocity: u8) -> MidiMessage {
    MidiMessage::NoteOn {
        channel: 0,
        note,
        velocity,
    }
}

#[test]
fn sends_only_changes() {
    let mapping = parse_mapping("t", MAP).unwrap();
    let mut fb = Feedback::new(&mapping);
    let mut out = Recorder::default();
    let state = ControlState::default();
    let mut snaps = idle();

    fb.update(0, &state, &snaps, &mut out);
    assert_eq!(
        out.take(),
        [
            note(11, 0),
            note(12, 1),
            MidiMessage::ControlChange {
                channel: 1,
                controller: 5,
                value: 2
            }
        ]
    );
    fb.update(1000, &state, &snaps, &mut out);
    assert!(out.take().is_empty());

    snaps[0] = snap(true, true, 5000.0, 100.0);
    fb.update(2000, &state, &snaps, &mut out);
    assert_eq!(out.take(), [note(11, 127)]);
}

#[test]
fn blink_alternates_in_phase() {
    let mapping = parse_mapping("t", MAP).unwrap();
    let mut fb = Feedback::new(&mapping);
    let mut out = Recorder::default();
    let state = ControlState::default();
    // Loaded, paused away from cue: both play and cue LEDs blink.
    let mut snaps = idle();
    snaps[0] = snap(true, false, 5000.0, 100.0);

    fb.update(0, &state, &snaps, &mut out);
    out.take();
    fb.update(100_000, &state, &snaps, &mut out);
    assert!(out.take().is_empty(), "still in the first half");
    fb.update(250_000, &state, &snaps, &mut out);
    assert_eq!(
        out.take(),
        [note(11, 0), note(12, 1)],
        "both turn off together"
    );
    fb.update(500_000, &state, &snaps, &mut out);
    assert_eq!(
        out.take(),
        [note(11, 127), note(12, 100)],
        "both turn on together"
    );
    fb.update(1_000_000 + 249_999, &state, &snaps, &mut out);
    assert!(out.take().is_empty());
}

#[test]
fn resync_resends_everything() {
    let mapping = parse_mapping("t", MAP).unwrap();
    let mut fb = Feedback::new(&mapping);
    let mut out = Recorder::default();
    let state = ControlState::default();
    let snaps = idle();

    fb.update(0, &state, &snaps, &mut out);
    let first = out.take();
    assert_eq!(first.len(), 3);
    fb.update(10, &state, &snaps, &mut out);
    assert!(out.take().is_empty());
    fb.resync();
    fb.update(20, &state, &snaps, &mut out);
    assert_eq!(out.take(), first);
}

#[test]
fn side_output_follows_active_deck() {
    let mapping = parse_mapping("t", SIDE_MAP).unwrap();
    let mut fb = Feedback::new(&mapping);
    let mut out = Recorder::default();
    let mut state = ControlState::default();
    let mut snaps = idle();
    snaps[0] = snap(true, true, 5000.0, 100.0);

    fb.update(0, &state, &snaps, &mut out);
    assert_eq!(out.take(), [note(11, 127)], "left shows deck 1 playing");

    state.side_mut(Side::Left).active_deck = d(2);
    fb.update(10, &state, &snaps, &mut out);
    assert_eq!(out.take(), [note(11, 0)], "deck 3 is not loaded");

    snaps[2] = snap(true, true, 10.0, 0.0);
    fb.update(20, &state, &snaps, &mut out);
    assert_eq!(out.take(), [note(11, 127)], "deck 3 playing");

    snaps[0] = snap(true, false, 5000.0, 100.0);
    fb.update(30, &state, &snaps, &mut out);
    assert!(out.take().is_empty(), "deck 1 is no longer shown");
}

#[test]
fn cc_and_note_use_velocity_off_not_note_off() {
    let mapping = parse_mapping("t", MAP).unwrap();
    let mut fb = Feedback::new(&mapping);
    let mut out = Recorder::default();
    fb.update(0, &ControlState::default(), &idle(), &mut out);
    assert!(
        out.0
            .iter()
            .all(|m| !matches!(m, MidiMessage::NoteOff { .. }))
    );
}

#[test]
fn play_and_cue_leds_follow_engine() {
    const SR: u32 = 44_100;
    const BLOCK: usize = 512;
    let (mut c, mut r) = engine(EngineConfig {
        sample_rate: SR,
        max_block_frames: BLOCK,
    });
    let render = |r: &mut engine::EngineRenderer, blocks: usize| {
        let mut m = vec![0.0; BLOCK * 2];
        let mut h = vec![0.0; BLOCK * 2];
        for _ in 0..blocks {
            r.render(&mut m, &mut h);
        }
    };
    let mapping = parse_mapping("t", MAP).unwrap();
    let mut fb = Feedback::new(&mapping);
    let mut out = Recorder::default();
    let mut state = ControlState::default();
    let snaps =
        |c: &engine::EngineController| [c.deck(d(0)), c.deck(d(1)), c.deck(d(2)), c.deck(d(3))];

    let track = Arc::new(TrackBuffer::new(SR, vec![0.5; 200_000 * 2]).unwrap());
    c.send(Command::Deck(d(0), DeckCommand::Load(track)))
        .unwrap();
    render(&mut r, 1);

    // Loaded, paused at the cue point: play blinks (lit at t=0), cue is solid.
    fb.update(0, &state, &snaps(&c), &mut out);
    assert_eq!(out.take()[..2], [note(11, 127), note(12, 100)]);

    let play = Action::new(ActionKind::PlayPause, Target::Deck(d(0))).unwrap();
    apply(&play, Input::Press, &mut state, &mut c);
    render(&mut r, 8);
    fb.update(0, &state, &snaps(&c), &mut out);
    assert_eq!(out.take(), [note(12, 1)], "playing: play solid on, cue off");

    // Pause away from the cue point: both blink, off in the second half of the period.
    apply(&play, Input::Press, &mut state, &mut c);
    render(&mut r, 1);
    fb.update(250_000, &state, &snaps(&c), &mut out);
    assert_eq!(
        out.take(),
        [note(11, 0)],
        "paused away from cue: play blinks (off phase)"
    );
    fb.update(500_000, &state, &snaps(&c), &mut out);
    assert_eq!(out.take(), [note(11, 127), note(12, 100)]);
    fb.update(750_000, &state, &snaps(&c), &mut out);
    assert_eq!(
        out.take(),
        [note(11, 0), note(12, 1)],
        "cue blinks in phase"
    );
}
