use actions::{Action, Input, Side};
use controller::builtin::{builtin_mapping, load_mapping, load_mapping_from};
use controller::mapping::{Mapping, parse_mapping};
use controller::{MidiMessage, MidiOut, Runtime, TimedMidi};
use engine::{DeckId, EngineConfig, EngineController, EngineRenderer, engine};

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

/// Left side: fader on CC 19, SHIFT on note 63, deck toggle on note 114, cue on note 12.
/// Global: crossfader on CC 31, headphone mix on CC 32, headphone-master toggle on note 40.
/// Outputs are all non-blinking, so their values do not depend on the clock.
const MAP: &str = r#"
[controller]
name = "Test"
port = "Test"

[[init]]
action = "channel.trim"
deck = 1
value = 0.25

[[init]]
action = "master.crossfader"
value = 0.0

[[input]]
midi = { cc = 19, channel = 1 }
action = "channel.fader"
side = "left"
soft_takeover = false

[[input]]
midi = { cc = 31, channel = 7 }
action = "master.crossfader"
soft_takeover = false

[[input]]
midi = { cc = 32, channel = 7 }
action = "master.headphone_mix"
soft_takeover = false

[[input]]
midi = { cc = 33, channel = 7 }
action = "master.level"

[[input]]
midi = { note = 63, channel = 1 }
action = "modifier.shift"
side = "left"

[[input]]
midi = { note = 114, channel = 1 }
action = "side.deck_toggle"
side = "left"

[[input]]
midi = { note = 12, channel = 1 }
action = "deck.cue"
side = "left"

[[input]]
midi = { note = 40, channel = 7 }
action = "master.headphone_master_toggle"

[[output]]
state = "side.shift"
side = "left"
midi = { note = 63, channel = 1 }

[[output]]
state = "side.alt_deck"
side = "left"
midi = { note = 114, channel = 1 }

[[output]]
state = "channel.pfl"
deck = 1
midi = { note = 84, channel = 1 }
"#;

struct Rig {
    runtime: Runtime,
    engine: EngineController,
    renderer: EngineRenderer,
    out: Recorder,
}

impl Rig {
    fn new() -> Self {
        let (engine, renderer) = engine(EngineConfig {
            sample_rate: 44_100,
            max_block_frames: 64,
        });
        Self {
            runtime: Runtime::new(mapping()),
            engine,
            renderer,
            out: Recorder::default(),
        }
    }

    fn attached() -> Self {
        let mut rig = Self::new();
        rig.runtime.attach(&mut rig.engine, &mut rig.out);
        rig.render();
        rig.out.take();
        rig
    }

    fn render(&mut self) {
        let mut master = [0.0; 128];
        let mut headphones = [0.0; 128];
        self.renderer.render(&mut master, &mut headphones);
    }

    fn batch(&mut self, msgs: &[MidiMessage]) -> controller::BatchStats {
        let timed: Vec<TimedMidi> = msgs
            .iter()
            .enumerate()
            .map(|(i, &message)| TimedMidi {
                micros: i as u64,
                message,
            })
            .collect();
        self.runtime.handle_batch(&timed, &mut self.engine)
    }
}

fn mapping() -> Mapping {
    parse_mapping("test.toml", MAP).unwrap()
}

fn d(index: usize) -> DeckId {
    DeckId::new(index).unwrap()
}

fn cc(channel: u8, controller: u8, value: u8) -> MidiMessage {
    MidiMessage::ControlChange {
        channel,
        controller,
        value,
    }
}

fn fader(value: u8) -> MidiMessage {
    cc(0, 19, value)
}

fn on(channel: u8, note: u8) -> MidiMessage {
    MidiMessage::NoteOn {
        channel,
        note,
        velocity: 127,
    }
}

fn off(channel: u8, note: u8) -> MidiMessage {
    MidiMessage::NoteOff {
        channel,
        note,
        velocity: 0,
    }
}

fn led(note: u8, velocity: u8) -> MidiMessage {
    MidiMessage::NoteOn {
        channel: 0,
        note,
        velocity,
    }
}

#[test]
fn attach_applies_init_and_resyncs() {
    let mut rig = Rig::new();
    rig.runtime.attach(&mut rig.engine, &mut rig.out);
    let state = rig.runtime.state();
    assert_eq!(state.deck(d(0)).trim, 0.25);
    assert_eq!(state.crossfader, 0.0);
    assert_eq!(
        rig.out.take(),
        [led(63, 0), led(114, 0), led(84, 0)],
        "attach sends every LED"
    );

    rig.runtime.tick(1_000, &rig.engine, &mut rig.out);
    assert!(rig.out.take().is_empty(), "nothing changed since attach");
}

#[test]
fn burst_of_cc_is_coalesced() {
    let mut rig = Rig::attached();
    let msgs: Vec<MidiMessage> = (0..1000).map(|i| fader((i % 128) as u8)).collect();
    let last = 999 % 128;
    let stats = rig.batch(&msgs);
    assert_eq!(stats.applied, 1);
    assert_eq!(stats.coalesced, 999);
    assert_eq!(stats.queue_full, 0);
    assert_eq!(
        rig.runtime.state().deck(d(0)).fader,
        f32::from(last as u8) / 127.0
    );
    assert_eq!(rig.runtime.last_resolved().len(), 1000);
}

#[test]
fn coalesced_ramp_passes_soft_takeover() {
    // The master level (takeover on) starts at 0.8. A ramp from there in steps smaller
    // than the takeover tolerance must be picked up all the way, even though the values
    // in between are coalesced and only the last reaches the engine.
    let mut rig = Rig::attached();
    let start = (0.8f32 * 127.0).round() as u8;
    let msgs: Vec<MidiMessage> = (start..=127).map(|v| cc(6, 33, v)).collect();
    let stats = rig.batch(&msgs);
    assert_eq!(stats.applied, 1);
    assert_eq!(rig.runtime.state().master_level, 1.0);
}

#[test]
fn buttons_keep_order_in_batch() {
    let mut rig = Rig::attached();
    let stats = rig.batch(&[on(0, 12), off(0, 12), on(0, 12)]);
    assert_eq!(stats.applied, 3);
    assert_eq!(stats.coalesced, 0);
    let inputs: Vec<Input> = rig
        .runtime
        .last_resolved()
        .iter()
        .flat_map(|(_, resolved)| resolved.iter().map(|(_, input)| *input))
        .collect();
    assert_eq!(inputs, [Input::Press, Input::Release, Input::Press]);
}

#[test]
fn coalescing_respects_order_with_deck_switch() {
    let mut rig = Rig::attached();
    let stats = rig.batch(&[
        fader(10),
        fader(20),
        on(0, 114),
        off(0, 114),
        fader(30),
        fader(40),
    ]);
    // Fader on deck 1, the toggle, fader on deck 3; the toggle's release does nothing.
    assert_eq!(stats.applied, 3);
    assert_eq!(stats.coalesced, 2);
    let state = rig.runtime.state();
    assert_eq!(state.side(Side::Left).active_deck, d(2));
    assert_eq!(state.deck(d(0)).fader, 20.0 / 127.0);
    assert_eq!(state.deck(d(2)).fader, 40.0 / 127.0);
}

#[test]
fn same_target_is_not_coalesced_across_a_button() {
    // The headphone-master toggle reads the mix the knob just set: the knob value before
    // the press must reach the state (and engine) before the press is applied.
    let mut rig = Rig::attached();
    let stats = rig.batch(&[cc(6, 32, 127), on(6, 40), off(6, 40)]);
    assert_eq!(stats.applied, 2);
    assert_eq!(stats.coalesced, 0);
    let state = rig.runtime.state();
    assert_eq!(state.headphone_mix, 0.0, "the toggle flipped from master");
    assert!(!state.headphone_master);

    let stats = rig.batch(&[cc(6, 31, 10), on(0, 12), cc(6, 31, 100), cc(6, 31, 120)]);
    assert_eq!(stats.applied, 3);
    assert_eq!(stats.coalesced, 1);
    assert_eq!(rig.runtime.state().crossfader, 120.0 / 127.0);
}

#[test]
fn queue_full_is_counted_not_fatal() {
    let mut rig = Rig::attached();
    let msgs: Vec<MidiMessage> = (0..300).flat_map(|_| [on(0, 12), off(0, 12)]).collect();
    let stats = rig.batch(&msgs);
    assert!(stats.queue_full > 0);
    assert_eq!(stats.applied + stats.queue_full, 600);

    // Values that could not be sent leave the state as the engine knows it.
    let stats = rig.batch(&[fader(5)]);
    assert_eq!(stats.queue_full, 1);
    assert_eq!(rig.runtime.state().deck(d(0)).fader, 1.0);

    rig.render();
    let stats = rig.batch(&[fader(5)]);
    assert_eq!(stats.applied, 1);
}

#[test]
fn reconnect_resends_all_leds() {
    let mut rig = Rig::attached();
    rig.batch(&[on(0, 63)]);
    rig.runtime.tick(10, &rig.engine, &mut rig.out);
    assert_eq!(rig.out.take(), [led(63, 127)]);

    rig.runtime.on_disconnect();
    rig.runtime.tick(20, &rig.engine, &mut rig.out);
    assert!(
        rig.out.take().is_empty(),
        "nothing is sent while disconnected"
    );

    rig.runtime.on_reconnect(&mut rig.engine, &mut rig.out);
    // SHIFT was held when the cable went: it is released rather than stuck.
    assert_eq!(rig.out.take(), [led(63, 0), led(114, 0), led(84, 0)]);
}

#[test]
fn reconnect_does_not_rerun_init() {
    let mut rig = Rig::attached();
    rig.batch(&[cc(6, 31, 127)]);
    assert_eq!(rig.runtime.state().crossfader, 1.0);

    rig.runtime.on_disconnect();
    rig.runtime.on_reconnect(&mut rig.engine, &mut rig.out);
    assert_eq!(
        rig.runtime.state().crossfader,
        1.0,
        "init not applied again"
    );
    assert_eq!(rig.runtime.state().deck(d(0)).trim, 0.25);
}

#[test]
fn reconnect_arms_takeover() {
    // The master level (takeover on) was picked up, then the controller went away and
    // came back with the knob elsewhere: the jump is held back.
    let mut rig = Rig::attached();
    let start = (0.8f32 * 127.0).round() as u8;
    rig.batch(&[cc(6, 33, start)]);
    rig.runtime.on_disconnect();
    rig.runtime.on_reconnect(&mut rig.engine, &mut rig.out);
    let stats = rig.batch(&[cc(6, 33, 0)]);
    assert_eq!(stats.applied, 0);
    assert_eq!(rig.runtime.state().master_level, f32::from(start) / 127.0);
}

#[test]
fn last_resolved_pairs_messages_with_actions() {
    let mut rig = Rig::attached();
    rig.batch(&[on(0, 12), cc(9, 99, 1)]);
    let last = rig.runtime.last_resolved();
    assert_eq!(last.len(), 2);
    assert_eq!(last[0].0, on(0, 12));
    assert_eq!(last[0].1, [(Action::Cue(d(0)), Input::Press)]);
    assert_eq!(last[1].0, cc(9, 99, 1));
    assert!(
        last[1].1.is_empty(),
        "unmapped messages are kept, unresolved"
    );

    rig.batch(&[]);
    assert_eq!(
        rig.runtime.last_resolved().len(),
        2,
        "an empty batch keeps the last one"
    );
}

const OVERRIDE: &str = r#"
[controller]
name = "My SB3"
port = "SB3"
"#;

#[test]
fn load_mapping_prefers_user_override() {
    let dir = std::env::temp_dir().join(format!("dj-software-runtime-{}", std::process::id()));
    let mapping_dir = dir.join("controllers").join("ddj-sb3");
    std::fs::create_dir_all(&mapping_dir).unwrap();

    let builtin = load_mapping_from("ddj-sb3", Some(&dir)).unwrap();
    assert_eq!(builtin.controller.name, "Pioneer DDJ-SB3");

    std::fs::write(mapping_dir.join("mapping.toml"), OVERRIDE).unwrap();
    let user = load_mapping_from("ddj-sb3", Some(&dir)).unwrap();
    assert_eq!(user.controller.name, "My SB3");

    std::fs::write(mapping_dir.join("mapping.toml"), "[controller]\n").unwrap();
    let err = load_mapping_from("ddj-sb3", Some(&dir)).unwrap_err();
    assert!(
        err.file.ends_with("mapping.toml") && err.file.contains("controllers"),
        "errors name the override file: {err}"
    );

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn load_mapping_reads_existing_path() {
    let dir = std::env::temp_dir().join(format!("dj-software-path-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("custom.toml");
    std::fs::write(&path, OVERRIDE).unwrap();
    let mapping = load_mapping(path.to_str().unwrap()).unwrap();
    assert_eq!(mapping.controller.name, "My SB3");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn unknown_mapping_lists_builtins() {
    let err = load_mapping_from("nope", None).unwrap_err();
    assert!(err.to_string().contains("ddj-sb3"), "{err}");
    assert!(builtin_mapping("nope").is_none());
}

#[test]
fn builtin_mappings_parse() {
    let text = builtin_mapping("ddj-sb3").unwrap();
    let mapping = parse_mapping("ddj-sb3", text).unwrap();
    assert_eq!(mapping.controller.port, "DDJ-SB3");
}
