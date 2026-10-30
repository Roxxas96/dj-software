//! Replays the committed DDJ-SB3 capture (`controllers/ddj-sb3/capture.toml`) through the
//! built-in mapping and a real engine, so every SB3 binding is checked against what the
//! hardware actually sends.

use std::sync::Arc;

use actions::{Action, Side};
use controller::builtin::load_mapping_from;
use controller::capture::{Control, Fixture, parse_fixture};
use controller::mapping::Mapping;
use controller::{BatchStats, MidiMessage, MidiOut, Runtime, TimedMidi};
use engine::{
    Command, DeckCommand, DeckId, EngineConfig, EngineController, EngineRenderer, TrackBuffer,
    engine,
};

const CAPTURE: &str = include_str!("../../../controllers/ddj-sb3/capture.toml");

/// Frames rendered per block; also the engine's sample rate base.
const BLOCK: usize = 512;
const SAMPLE_RATE: u32 = 44_100;

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

fn fixture() -> Fixture {
    parse_fixture(CAPTURE).expect("the committed capture parses")
}

fn mapping() -> Mapping {
    // `None`: the built-in mapping, never a user override.
    load_mapping_from("ddj-sb3", None).expect("the built-in SB3 mapping loads")
}

fn d(n: usize) -> DeckId {
    DeckId::new(n - 1).unwrap()
}

/// A real engine with a track on every deck, the built-in mapping attached.
struct Rig {
    fixture: Fixture,
    runtime: Runtime,
    engine: EngineController,
    renderer: EngineRenderer,
    out: Recorder,
    micros: u64,
}

impl Rig {
    fn attached() -> Self {
        let (mut engine, renderer) = engine(EngineConfig {
            sample_rate: SAMPLE_RATE,
            max_block_frames: BLOCK,
        });
        for deck in DeckId::ALL {
            let track = Arc::new(TrackBuffer::new(SAMPLE_RATE, vec![0.1; 441_000 * 2]).unwrap());
            engine
                .send(Command::Deck(deck, DeckCommand::Load(track)))
                .unwrap();
        }
        let mut rig = Self {
            fixture: fixture(),
            runtime: Runtime::new(mapping()),
            engine,
            renderer,
            out: Recorder::default(),
            micros: 0,
        };
        rig.render();
        rig.runtime.attach(&mut rig.engine, &mut rig.out);
        rig.render();
        rig
    }

    fn render(&mut self) {
        let mut master = vec![0.0; BLOCK * 2];
        let mut headphones = vec![0.0; BLOCK * 2];
        self.renderer.render(&mut master, &mut headphones);
    }

    fn control(&self, id: &str) -> &Control {
        self.fixture
            .control(id)
            .unwrap_or_else(|| panic!("control `{id}` is not in the capture"))
    }

    /// Sends `msgs` as one batch, renders a block and returns what each message resolved
    /// to, flattened.
    fn batch(&mut self, msgs: &[MidiMessage]) -> (Vec<Action>, BatchStats) {
        let timed: Vec<TimedMidi> = msgs
            .iter()
            .map(|&message| {
                self.micros += 1_000;
                TimedMidi {
                    micros: self.micros,
                    message,
                }
            })
            .collect();
        let stats = self.runtime.handle_batch(&timed, &mut self.engine);
        let actions = self
            .runtime
            .last_resolved()
            .iter()
            .flat_map(|(_, resolved)| resolved.iter().map(|(action, _)| *action))
            .collect();
        self.render();
        (actions, stats)
    }

    /// Replays the captured messages of `id` (see [`replay_messages`]), one batch per
    /// message.
    fn replay(&mut self, id: &str) -> (Vec<Action>, BatchStats) {
        let msgs = replay_messages(self.control(id));
        let mut actions = Vec::new();
        let mut total = BatchStats::default();
        for msg in msgs {
            let (a, stats) = self.batch(&[msg]);
            actions.extend(a);
            total.applied += stats.applied;
            total.coalesced += stats.coalesced;
            total.queue_full += stats.queue_full;
        }
        (actions, total)
    }

    /// Presses and releases the captured button `id`.
    fn press(&mut self, id: &str) -> Vec<Action> {
        self.replay(id).0
    }

    /// Sends the captured message of the button `id` with `velocity` (0 releases).
    fn button(&mut self, id: &str, velocity: u8) -> Vec<Action> {
        let range = self.control(id).messages[0];
        assert_eq!(range.status & 0xF0, 0x90, "`{id}` is a note");
        self.batch(&[note(range.status & 0x0F, range.data1, velocity)])
            .0
    }

    /// Sends the 14-bit control `id` at the captured `(msb, lsb)` values, MSB first.
    fn cc14(&mut self, id: &str, msb_value: u8, lsb_value: u8) -> Vec<Action> {
        let (channel, msb, lsb) = cc14_of(self.control(id));
        let mut actions = self.batch(&[cc(channel, msb, msb_value)]).0;
        actions.extend(self.batch(&[cc(channel, lsb, lsb_value)]).0);
        actions
    }

    fn tick(&mut self) {
        self.micros += 1_000;
        self.runtime.tick(self.micros, &self.engine, &mut self.out);
    }
}

fn note(channel: u8, note: u8, velocity: u8) -> MidiMessage {
    if velocity == 0 {
        MidiMessage::NoteOff {
            channel,
            note,
            velocity: 0,
        }
    } else {
        MidiMessage::NoteOn {
            channel,
            note,
            velocity,
        }
    }
}

fn cc(channel: u8, controller: u8, value: u8) -> MidiMessage {
    MidiMessage::ControlChange {
        channel,
        controller,
        value,
    }
}

/// The channel and MSB/LSB controllers of a captured 14-bit control.
fn cc14_of(control: &Control) -> (u8, u8, u8) {
    let pair = control
        .cc14
        .unwrap_or_else(|| panic!("`{}` is not a 14-bit control", control.id));
    let range = control
        .messages
        .iter()
        .find(|m| m.data1 == pair.msb)
        .unwrap_or_else(|| panic!("`{}` has no MSB message", control.id));
    (range.status & 0x0F, pair.msb, pair.lsb)
}

/// Messages rebuilt from what the capture recorded for `control`, using only captured
/// values:
/// - a 14-bit control sweeps from its captured minimum to its captured maximum (MSB then
///   LSB each time), so soft takeover picks the value up on the way;
/// - a note is pressed at its captured maximum and released;
/// - a plain CC sends its first, minimum and maximum values.
fn replay_messages(control: &Control) -> Vec<MidiMessage> {
    if let Some(pair) = control.cc14 {
        let range = |data1: u8| {
            control
                .messages
                .iter()
                .find(|m| m.data1 == data1)
                .copied()
                .unwrap_or_else(|| panic!("`{}` lacks controller {data1}", control.id))
        };
        let (msb, lsb) = (range(pair.msb), range(pair.lsb));
        let channel = msb.status & 0x0F;
        return vec![
            cc(channel, pair.msb, msb.min),
            cc(channel, pair.lsb, lsb.min),
            cc(channel, pair.msb, msb.max),
            cc(channel, pair.lsb, lsb.max),
        ];
    }
    control
        .messages
        .iter()
        .flat_map(|m| {
            let channel = m.status & 0x0F;
            match m.status & 0xF0 {
                0x90 => vec![note(channel, m.data1, m.max), note(channel, m.data1, 0)],
                0xB0 => [m.first.unwrap_or(m.min), m.min, m.max]
                    .into_iter()
                    .map(|v| cc(channel, m.data1, v))
                    .collect(),
                other => panic!("`{}`: unexpected status {other:02X}", control.id),
            }
        })
        .collect()
}

/// Which deck layer a control is replayed on: on `Alt`, the side's DECK button is pressed
/// first so the side shows deck 3 (left) or 4 (right).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Layer {
    Main,
    Alt(Side),
}

use Layer::{Alt, Main};

/// Every mapped control: capture id, layer, and the only action its messages may drive.
fn table() -> Vec<(&'static str, Layer, Action)> {
    let l = Side::Left;
    let r = Side::Right;
    vec![
        // Left deck, deck 1.
        ("deck1.play", Main, Action::PlayPause(d(1))),
        ("deck1.cue", Main, Action::Cue(d(1))),
        ("deck1.cue.shift", Main, Action::ReturnToStart(d(1))),
        ("deck1.key_lock.shift", Main, Action::TempoRangeNext(d(1))),
        ("deck1.tempo", Main, Action::Tempo(d(1))),
        ("deck1.shift", Main, Action::Shift(l)),
        ("deck1.deck_select", Main, Action::DeckToggle(l)),
        ("channel1.trim", Main, Action::Trim(d(1))),
        ("channel1.eq_high", Main, Action::EqHigh(d(1))),
        ("channel1.eq_mid", Main, Action::EqMid(d(1))),
        ("channel1.eq_low", Main, Action::EqLow(d(1))),
        ("channel1.filter", Main, Action::Filter(d(1))),
        ("channel1.fader", Main, Action::Fader(d(1))),
        ("channel1.pfl", Main, Action::PflToggle(d(1))),
        // Right deck, deck 2.
        ("deck2.play", Main, Action::PlayPause(d(2))),
        ("deck2.cue", Main, Action::Cue(d(2))),
        ("deck2.cue.shift", Main, Action::ReturnToStart(d(2))),
        ("deck2.key_lock.shift", Main, Action::TempoRangeNext(d(2))),
        ("deck2.tempo", Main, Action::Tempo(d(2))),
        ("deck2.shift", Main, Action::Shift(r)),
        ("deck2.deck_select", Main, Action::DeckToggle(r)),
        ("channel2.trim", Main, Action::Trim(d(2))),
        ("channel2.eq_high", Main, Action::EqHigh(d(2))),
        ("channel2.eq_mid", Main, Action::EqMid(d(2))),
        ("channel2.eq_low", Main, Action::EqLow(d(2))),
        ("channel2.filter", Main, Action::Filter(d(2))),
        ("channel2.fader", Main, Action::Fader(d(2))),
        ("channel2.pfl", Main, Action::PflToggle(d(2))),
        // Left deck, deck 3 (software layer).
        ("deck3.play", Alt(l), Action::PlayPause(d(3))),
        ("deck3.cue", Alt(l), Action::Cue(d(3))),
        ("deck3.cue.shift", Alt(l), Action::ReturnToStart(d(3))),
        ("deck3.key_lock.shift", Alt(l), Action::TempoRangeNext(d(3))),
        ("deck3.tempo", Alt(l), Action::Tempo(d(3))),
        ("deck3.shift", Alt(l), Action::Shift(l)),
        ("deck3.deck_select", Alt(l), Action::DeckToggle(l)),
        ("channel3.trim", Alt(l), Action::Trim(d(3))),
        ("channel3.eq_high", Alt(l), Action::EqHigh(d(3))),
        ("channel3.eq_mid", Alt(l), Action::EqMid(d(3))),
        ("channel3.eq_low", Alt(l), Action::EqLow(d(3))),
        ("channel3.filter", Alt(l), Action::Filter(d(3))),
        ("channel3.fader", Alt(l), Action::Fader(d(3))),
        ("channel3.pfl", Alt(l), Action::PflToggle(d(3))),
        // Right deck, deck 4 (software layer).
        ("deck4.play", Alt(r), Action::PlayPause(d(4))),
        ("deck4.cue", Alt(r), Action::Cue(d(4))),
        ("deck4.cue.shift", Alt(r), Action::ReturnToStart(d(4))),
        ("deck4.key_lock.shift", Alt(r), Action::TempoRangeNext(d(4))),
        ("deck4.tempo", Alt(r), Action::Tempo(d(4))),
        ("deck4.shift", Alt(r), Action::Shift(r)),
        ("deck4.deck_select", Alt(r), Action::DeckToggle(r)),
        ("channel4.trim", Alt(r), Action::Trim(d(4))),
        ("channel4.eq_high", Alt(r), Action::EqHigh(d(4))),
        ("channel4.eq_mid", Alt(r), Action::EqMid(d(4))),
        ("channel4.eq_low", Alt(r), Action::EqLow(d(4))),
        ("channel4.filter", Alt(r), Action::Filter(d(4))),
        ("channel4.fader", Alt(r), Action::Fader(d(4))),
        ("channel4.pfl", Alt(r), Action::PflToggle(d(4))),
        // Mixer, global.
        ("crossfader", Main, Action::Crossfader),
        ("headphone_master", Main, Action::HeadphoneMasterToggle),
    ]
}

fn deck_select(side: Side) -> &'static str {
    match side {
        Side::Left => "deck1.deck_select",
        Side::Right => "deck2.deck_select",
    }
}

#[test]
fn every_mapped_control_has_capture_and_effect() {
    let fixture = fixture();
    for (id, layer, expected) in table() {
        assert!(
            fixture.control(id).is_some(),
            "`{id}` is mapped but has no capture"
        );
        let mut rig = Rig::attached();
        if let Alt(side) = layer {
            rig.press(deck_select(side));
            assert_eq!(
                rig.runtime.state().side(side).active_deck,
                side.decks()[1],
                "DECK switches the {side:?} side to its second deck"
            );
        }
        let (actions, stats) = rig.replay(id);
        assert!(
            !actions.is_empty(),
            "`{id}`: the captured messages resolved to nothing"
        );
        assert!(
            actions.iter().all(|a| *a == expected),
            "`{id}`: expected only {expected:?}, got {actions:?}"
        );
        assert!(stats.applied > 0, "`{id}`: no action took effect");
        assert_eq!(stats.queue_full, 0, "`{id}`");
    }
}

#[test]
fn every_binding_is_in_the_table() {
    // The reverse check: each `[[input]]` of the mapping listens to a message some
    // control of the table sends, so no binding escapes the replay test.
    let fixture = fixture();
    let sent: Vec<MidiMessage> = table()
        .iter()
        .flat_map(|(id, _, _)| replay_messages(fixture.control(id).unwrap()))
        .collect();
    let mapping = mapping();
    for (i, binding) in mapping.inputs.iter().enumerate() {
        assert!(
            sent.iter().any(|msg| binding.midi.matches(msg)),
            "input[{i}] ({:?} on {:?}) is not covered by the replay table",
            binding.kind,
            binding.midi
        );
    }
}

#[test]
fn deck_switch_routes_controls_to_deck_3_and_back() {
    let mut rig = Rig::attached();
    assert_eq!(rig.press("deck1.play"), [Action::PlayPause(d(1)); 2]);

    rig.press("deck1.deck_select");
    // The SB3 sends the same bytes for decks 1 and 3: the side decides.
    assert_eq!(rig.press("deck3.play"), [Action::PlayPause(d(3)); 2]);
    assert!(
        rig.replay("channel3.trim")
            .0
            .iter()
            .all(|a| *a == Action::Trim(d(3)))
    );
    // The right side is untouched by the left DECK button.
    assert_eq!(rig.press("deck2.play"), [Action::PlayPause(d(2)); 2]);

    rig.press("deck3.deck_select");
    assert_eq!(rig.press("deck1.play"), [Action::PlayPause(d(1)); 2]);
    assert!(
        rig.replay("channel1.trim")
            .0
            .iter()
            .all(|a| *a == Action::Trim(d(1)))
    );

    // The same on the right side, deck 4.
    rig.press("deck2.deck_select");
    assert_eq!(rig.press("deck4.cue"), [Action::Cue(d(4)); 2]);
    rig.press("deck4.deck_select");
    assert_eq!(rig.press("deck2.cue"), [Action::Cue(d(2)); 2]);
}

#[test]
fn deck_switch_affects_deck_3_only_and_headphone_master_led_follows_state() {
    let mut rig = Rig::attached();
    let led = |velocity| MidiMessage::NoteOn {
        channel: 6,
        note: 0x5B,
        velocity,
    };
    assert_eq!(
        rig.out.take(),
        [led(0)],
        "attach sends the headphone MASTER LED, the only output"
    );
    let before = *rig.runtime.state().deck(d(1));

    rig.press("deck1.deck_select");
    rig.press("deck3.play");
    rig.press("channel3.pfl");
    rig.press("deck3.key_lock.shift");
    rig.cc14("channel3.trim", 0, 0);
    rig.cc14("channel3.trim", 127, 127);
    rig.cc14("channel3.trim", 32, 0);

    let state = rig.runtime.state();
    let deck3 = state.deck(d(3));
    assert!(deck3.pfl, "headphone CUE toggled deck 3's PFL");
    assert_ne!(deck3.tempo_range, before.tempo_range, "SHIFT+KEY LOCK");
    assert!((deck3.trim - 0.25).abs() < 0.01, "trim {}", deck3.trim);
    assert!(rig.engine.deck(d(3)).playing, "PLAY started deck 3");
    assert_eq!(*state.deck(d(1)), before, "deck 1 is untouched");
    assert!(!rig.engine.deck(d(1)).playing, "deck 1 did not start");

    // The DECK switch drives no LED (the SB3 has none for it).
    rig.tick();
    assert!(rig.out.take().is_empty());

    rig.press("headphone_master");
    rig.tick();
    assert_eq!(rig.out.take(), [led(127)]);
    rig.press("headphone_master");
    rig.tick();
    assert_eq!(rig.out.take(), [led(0)]);
}

#[test]
fn soft_takeover_after_deck_switch_on_fader() {
    let mut rig = Rig::attached();
    // Pick deck 1's fader up at the top (its default, 1.0), then pull it down to 0.
    rig.cc14("channel1.fader", 127, 127);
    rig.cc14("channel1.fader", 0, 0);
    assert_eq!(rig.runtime.state().deck(d(1)).fader, 0.0);

    // Deck 3's fader is still at 1.0 while the physical fader sits at 0.
    rig.press("deck1.deck_select");
    let moved = rig.cc14("channel3.fader", 40, 0);
    assert!(moved.is_empty(), "no jump to deck 3: {moved:?}");
    assert_eq!(rig.runtime.state().deck(d(3)).fader, 1.0);

    // Reaching deck 3's value picks it up.
    let moved = rig.cc14("channel3.fader", 127, 127);
    assert!(moved.contains(&Action::Fader(d(3))));
    rig.cc14("channel3.fader", 64, 0);
    assert!((rig.runtime.state().deck(d(3)).fader - 0.5).abs() < 0.01);

    // Back on deck 1 (at 0) with the physical fader at the middle: no jump either.
    rig.press("deck3.deck_select");
    let moved = rig.cc14("channel1.fader", 80, 0);
    assert!(moved.is_empty(), "no jump to deck 1: {moved:?}");
    assert_eq!(rig.runtime.state().deck(d(1)).fader, 0.0);
}

#[test]
fn shift_cue_returns_to_start_but_cue_alone_previews() {
    let mut rig = Rig::attached();

    // Paused at the cue point, CUE previews while held.
    assert_eq!(rig.button("deck1.cue", 127), [Action::Cue(d(1))]);
    assert!(rig.engine.deck(d(1)).previewing);
    assert_eq!(rig.button("deck1.cue", 0), [Action::Cue(d(1))]);
    let snap = rig.engine.deck(d(1));
    assert!(!snap.previewing && !snap.playing);

    // Play for a while, then SHIFT+CUE: the SB3 sends its own note for it.
    rig.press("deck1.play");
    for _ in 0..20 {
        rig.render();
    }
    let played = rig.engine.deck(d(1)).position_frames;
    assert!(played > (BLOCK * 20) as f64, "played {played}");
    assert_eq!(rig.button("deck1.shift", 127), [Action::Shift(Side::Left)]);
    let actions = rig.press("deck1.cue.shift");
    assert!(
        actions.iter().all(|a| *a == Action::ReturnToStart(d(1))),
        "SHIFT+CUE must not cue: {actions:?}"
    );
    rig.button("deck1.shift", 0);
    let snap = rig.engine.deck(d(1));
    assert!(snap.playing, "return to start keeps playing");
    // Each batch renders one block: the press, the release and the SHIFT release.
    assert!(
        snap.position_frames <= (BLOCK * 3) as f64,
        "back at the start: {} (was {played})",
        snap.position_frames
    );
    assert!(!snap.previewing);
}

#[test]
fn tempo_slider_toward_you_is_faster() {
    // The capture's tempo slider reached 0 when pulled fully toward the player, which is
    // the "+" end on Pioneer controllers. Checked on every deck, each in its DECK state.
    for (n, layer) in [
        (1, Main),
        (2, Main),
        (3, Alt(Side::Left)),
        (4, Alt(Side::Right)),
    ] {
        let mut rig = Rig::attached();
        if let Alt(side) = layer {
            rig.press(deck_select(side));
        }
        let id = format!("deck{n}.tempo");
        let tempo = rig.control(&id).messages.clone();
        let msb_max = tempo.iter().find(|m| m.data1 == 0x00).unwrap().max;
        let lsb_max = tempo.iter().find(|m| m.data1 == 0x20).unwrap().max;
        // Start from the captured maximum (soft takeover picks the slider up near or
        // across the middle), then pull it to 0.
        rig.cc14(&id, msb_max, lsb_max);
        let actions = rig.cc14(&id, 0, 0);
        assert!(
            actions.contains(&Action::Tempo(d(n))),
            "deck {n}: {actions:?}"
        );
        let ratio = rig.engine.deck(d(n)).tempo_ratio;
        assert!(
            ratio > 1.0,
            "deck {n}: slider at 0 (toward you) gives ratio {ratio}"
        );
        for other in (1..=4).filter(|&o| o != n) {
            assert_eq!(rig.engine.deck(d(other)).tempo_ratio, 1.0, "deck {other}");
        }
    }
}

#[test]
fn master_and_headphone_knobs_are_left_to_the_sb3() {
    // The SB3 applies its MASTER and HEADPHONES knobs in its own audio interface: they stay
    // unbound and the software levels start at full.
    let mut rig = Rig::attached();
    assert_eq!(rig.runtime.state().master_level, 1.0);
    assert_eq!(rig.runtime.state().headphone_level, 1.0);
    for id in ["master_level", "headphone_level"] {
        let (actions, _) = rig.replay(id);
        assert!(actions.is_empty(), "`{id}` is unbound: {actions:?}");
    }
    assert_eq!(rig.runtime.state().master_level, 1.0);
}
