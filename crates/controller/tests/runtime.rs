use actions::{Action, Input, Side};
use controller::builtin::{builtin_mapping, load_mapping, load_mapping_from};
use controller::mapping::{Mapping, parse_mapping};
use controller::{MidiMessage, MidiOut, PresenceChange, Reconnect, Runtime, TimedMidi};
use std::sync::Arc;

use engine::{
    Command, DeckCommand, DeckId, EngineConfig, EngineController, EngineRenderer, TrackBuffer,
    engine,
};

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
/// Tempo slider on CC 34 and SYNC on note 41 (channel 7), left side.
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
midi = { cc = 34, channel = 7 }
action = "deck.tempo"
side = "left"

[[input]]
midi = { note = 41, channel = 7 }
action = "deck.sync"
side = "left"

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

    rig.runtime.on_disconnect(&mut rig.engine);
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
fn disconnect_releases_held_presses() {
    let mut rig = Rig::attached();
    let track = Arc::new(TrackBuffer::new(44_100, vec![0.5; 200_000 * 2]).unwrap());
    rig.engine
        .send(Command::Deck(
            d(0),
            DeckCommand::Load {
                track,
                memory: engine::TrackMemory::default(),
            },
        ))
        .unwrap();
    rig.render();

    rig.batch(&[on(0, 12)]);
    rig.render();
    assert!(rig.engine.deck(d(0)).previewing, "CUE held: preview runs");

    rig.runtime.on_disconnect(&mut rig.engine);
    rig.render();
    assert!(
        !rig.engine.deck(d(0)).previewing,
        "the held CUE is released when the cable goes"
    );
}

#[test]
fn batches_are_ignored_while_detached() {
    // Messages drained after the controller was lost must not bring back SHIFT or held
    // presses.
    let mut rig = Rig::attached();
    rig.runtime.on_disconnect(&mut rig.engine);
    let stats = rig.batch(&[on(0, 63), on(0, 12), fader(5)]);
    assert_eq!(stats, controller::BatchStats::default());
    let state = rig.runtime.state();
    assert!(!state.side(Side::Left).shift);
    assert_eq!(state.deck(d(0)).fader, 1.0);

    let mut fresh = Rig::new();
    let stats = fresh.batch(&[on(0, 63)]);
    assert_eq!(
        stats.applied, 0,
        "nothing is handled before the first attach"
    );
}

#[test]
fn reconnect_does_not_rerun_init() {
    let mut rig = Rig::attached();
    rig.batch(&[cc(6, 31, 127)]);
    assert_eq!(rig.runtime.state().crossfader, 1.0);

    rig.runtime.on_disconnect(&mut rig.engine);
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
    rig.runtime.on_disconnect(&mut rig.engine);
    rig.runtime.on_reconnect(&mut rig.engine, &mut rig.out);
    let stats = rig.batch(&[cc(6, 33, 0)]);
    assert_eq!(stats.applied, 0);
    assert_eq!(rig.runtime.state().master_level, f32::from(start) / 127.0);
}

#[test]
fn first_attach_takes_first_physical_value() {
    // The master level (takeover on) starts at 0.8. On the first attach nothing is known
    // about where the knob is, so its first value is taken as is, however far away.
    let mut rig = Rig::attached();
    let stats = rig.batch(&[cc(6, 33, 0)]);
    assert_eq!(stats.applied, 1);
    assert_eq!(rig.runtime.state().master_level, 0.0);
    // Once taken, a value changed elsewhere still arms takeover as usual: covered by
    // the resolver tests. Here, a later move keeps passing.
    rig.batch(&[cc(6, 33, 127)]);
    assert_eq!(rig.runtime.state().master_level, 1.0);
}

#[test]
fn reconnect_still_arms_takeover() {
    // Unlike the first attach, a reconnect arms takeover even for a control never moved
    // since startup: the knob may have moved while the controller was away.
    let mut rig = Rig::attached();
    rig.runtime.on_disconnect(&mut rig.engine);
    rig.runtime.on_reconnect(&mut rig.engine, &mut rig.out);
    let stats = rig.batch(&[cc(6, 33, 0)]);
    assert_eq!(stats.applied, 0);
    assert_eq!(rig.runtime.state().master_level, 0.8);
    let start = (0.8f32 * 127.0).round() as u8;
    let stats = rig.batch(&[cc(6, 33, start)]);
    assert_eq!(stats.applied, 1, "picked up near the current value");
}

/// A connection whose reopening succeeds or fails on demand, with no real ports.
struct FakeConnection {
    out: Recorder,
    connected: bool,
    reopen_works: bool,
    reconnects: u32,
}

impl FakeConnection {
    fn new(reopen_works: bool) -> Self {
        Self {
            out: Recorder::default(),
            connected: true,
            reopen_works,
            reconnects: 0,
        }
    }
}

impl MidiOut for FakeConnection {
    fn send(&mut self, msg: MidiMessage) {
        if self.connected {
            self.out.send(msg);
        }
    }
}

impl Reconnect for FakeConnection {
    fn force_reconnect(&mut self) -> PresenceChange {
        self.reconnects += 1;
        let was_connected = self.connected;
        self.connected = self.reopen_works;
        match (was_connected, self.connected) {
            (_, true) => PresenceChange::Back,
            (true, false) => PresenceChange::Lost,
            (false, false) => PresenceChange::None,
        }
    }
}

#[test]
fn force_reconnect_releases_resyncs_and_arms_takeover() {
    let mut rig = Rig::attached();
    let start = (0.8f32 * 127.0).round() as u8;
    rig.batch(&[cc(6, 33, start), cc(6, 31, 127), on(0, 63)]);
    assert!(rig.runtime.state().side(Side::Left).shift);

    let mut connection = FakeConnection::new(true);
    let change = rig
        .runtime
        .force_reconnect(&mut rig.engine, &mut connection);
    assert_eq!(change, PresenceChange::Back);
    assert_eq!(connection.reconnects, 1);
    let state = rig.runtime.state();
    assert!(!state.side(Side::Left).shift, "held SHIFT released");
    assert_eq!(state.crossfader, 1.0, "init not applied again");
    assert_eq!(
        connection.out.take(),
        [led(63, 0), led(114, 0), led(84, 0)],
        "every LED sent again"
    );
    // Takeover is armed: the knob must be picked up again.
    assert_eq!(rig.batch(&[cc(6, 33, 0)]).applied, 0);
    assert_eq!(rig.batch(&[cc(6, 33, start)]).applied, 1);
}

#[test]
fn failed_force_reconnect_leaves_the_controller_detached() {
    let mut rig = Rig::attached();
    let mut connection = FakeConnection::new(false);
    let change = rig
        .runtime
        .force_reconnect(&mut rig.engine, &mut connection);
    assert_eq!(change, PresenceChange::Lost);
    assert_eq!(rig.batch(&[on(0, 63)]).applied, 0, "ignored while detached");
    rig.runtime.tick(10, &rig.engine, &mut connection);
    assert!(connection.out.take().is_empty());

    // Presence polling later finds it again: the usual reconnect.
    connection.connected = true;
    rig.runtime.on_reconnect(&mut rig.engine, &mut connection);
    assert_eq!(rig.batch(&[on(0, 63)]).applied, 1);
}

#[test]
fn force_reconnect_before_the_first_attach_attaches() {
    let mut rig = Rig::new();
    let mut connection = FakeConnection::new(true);
    connection.connected = false;
    let change = rig
        .runtime
        .force_reconnect(&mut rig.engine, &mut connection);
    assert_eq!(change, PresenceChange::Back);
    assert_eq!(rig.runtime.state().deck(d(0)).trim, 0.25, "init applied");
    assert_eq!(
        rig.batch(&[cc(6, 33, 0)]).applied,
        1,
        "first attach: the first value is taken"
    );
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
    let dir = std::env::temp_dir().join(format!("open-jockey-runtime-{}", std::process::id()));
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
    let dir = std::env::temp_dir().join(format!("open-jockey-path-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("custom.toml");
    std::fs::write(&path, OVERRIDE).unwrap();
    let mapping = load_mapping(path.to_str().unwrap()).unwrap();
    assert_eq!(mapping.controller.name, "My SB3");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn load_mapping_does_not_read_a_directory_as_a_file() {
    // A `ddj-sb3` directory in the working directory (like the repo's `controllers/ddj-sb3`)
    // is a name to look up, not a mapping file.
    let dir = std::env::temp_dir().join(format!("open-jockey-dir-{}", std::process::id()));
    let named = dir.join("ddj-sb3");
    std::fs::create_dir_all(&named).unwrap();
    let mapping = load_mapping(named.to_str().unwrap());
    std::fs::remove_dir_all(&dir).unwrap();
    let err = mapping.unwrap_err();
    assert!(err.to_string().contains("built-in mappings"), "{err}");
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

#[test]
fn requests_are_returned_in_order() {
    use actions::{AppRequest, GridEditRequest as E};
    let map = format!(
        "{MAP}{}",
        r#"
[[input]]
midi = { note = 1, channel = 1 }
action = "deck.grid_tap"
side = "left"

[[input]]
midi = { note = 2, channel = 1 }
action = "deck.grid_shift_forward"
side = "left"

[[input]]
midi = { note = 3, channel = 1 }
action = "deck.grid_shift_back"
side = "left"

[[input]]
midi = { note = 4, channel = 1 }
action = "deck.grid_lock_toggle"
side = "left"
"#
    );
    let mut rig = Rig::new();
    rig.runtime = Runtime::new(parse_mapping("test.toml", &map).unwrap());
    rig.runtime.attach(&mut rig.engine, &mut rig.out);
    // Each message's time is 1 s plus 100 ms per message.
    let msgs = [
        fader(10),
        on(0, 1),
        fader(20),
        fader(30),
        on(0, 63),
        on(0, 2),
        off(0, 2),
        on(0, 114),
        on(0, 4),
        off(0, 63),
        on(0, 3),
        off(0, 1),
        on(0, 1),
    ];
    let timed: Vec<TimedMidi> = msgs
        .iter()
        .enumerate()
        .map(|(i, &message)| TimedMidi {
            micros: 1_000_000 + 100_000 * i as u64,
            message,
        })
        .collect();
    let stats = rig.runtime.handle_batch(&timed, &mut rig.engine);
    let grid = |deck, edit| AppRequest::Grid { deck, edit };
    assert_eq!(
        stats.requests,
        [
            // A tap carries its message's time.
            grid(
                d(0),
                E::Tap {
                    at_micros: 1_100_000
                }
            ),
            // SHIFT held on the left: 1 ms steps.
            grid(d(0), E::Shift { millis: 1 }),
            // The deck toggle took effect before the next press.
            grid(d(2), E::ToggleLock),
            grid(d(2), E::Shift { millis: -5 }),
            grid(
                d(2),
                E::Tap {
                    at_micros: 2_200_000
                }
            ),
        ]
    );
    assert_eq!(stats.coalesced, 1);
    // Two fader values, SHIFT on and off, the deck toggle and the five requests.
    assert_eq!(stats.applied, 2 + 3 + 5);
    assert_eq!(stats.queue_full, 0);
    // Nothing is left over for the next batch.
    assert!(rig.batch(&[fader(40)]).requests.is_empty());
}

#[test]
fn relative_steps_are_summed_per_run() {
    use actions::{AppRequest, BrowserRequest};
    let map = format!(
        "{MAP}{}",
        r#"
[[input]]
midi = { cc = 0x40, channel = 7 }
action = "browser.scroll"
value = "relative_twos"

[[input]]
midi = { note = 0x41, channel = 7 }
action = "browser.toggle_focus"
"#
    );
    let mut rig = Rig::new();
    rig.runtime = Runtime::new(parse_mapping("test.toml", &map).unwrap());
    rig.runtime.attach(&mut rig.engine, &mut rig.out);
    let knob = |value| cc(6, 0x40, value);
    let scroll = |steps| AppRequest::Browser(BrowserRequest::Scroll(steps));

    let stats = rig.batch(&[
        // +1 +1 +1 -1, with a fader move in between that does not end the run.
        knob(1),
        knob(1),
        fader(10),
        knob(1),
        knob(127),
        // A press ends the run; its release too.
        on(6, 0x41),
        off(6, 0x41),
        knob(2),
        knob(1),
    ]);
    assert_eq!(
        stats.requests,
        [
            scroll(2),
            AppRequest::Browser(BrowserRequest::ToggleFocus),
            scroll(3)
        ]
    );
    // Three knob messages folded into the first scroll and one into the last.
    assert_eq!(stats.coalesced, 4);
    // The fader, two scrolls and the press.
    assert_eq!(stats.applied, 4);
    assert_eq!(rig.runtime.state().deck(d(0)).fader, 10.0 / 127.0);
    assert_eq!(rig.runtime.last_resolved().len(), 9);

    // Steps that cancel out ask for nothing.
    let stats = rig.batch(&[knob(1), knob(127)]);
    assert!(stats.requests.is_empty(), "{:?}", stats.requests);
    assert_eq!(stats.coalesced, 1);

    // Each batch is its own run.
    let stats = rig.batch(&[knob(127)]);
    assert_eq!(stats.requests, [scroll(-1)]);
    assert_eq!(stats.coalesced, 0);
}

#[test]
fn tempo_slider_is_armed_after_sync() {
    let mut rig = Rig::attached();
    // Deck 1 at 128 BPM, deck 2 playing at 122 BPM.
    for (deck, bpm, playing) in [(0, 128.0, false), (1, 122.0, true)] {
        let track = Arc::new(TrackBuffer::new(44_100, vec![0.5; 100_000 * 2]).unwrap());
        let memory = engine::TrackMemory {
            grid: engine::Grid::from_bpm(bpm, 0.0, 44_100),
            ..Default::default()
        };
        let load = DeckCommand::Load { track, memory };
        rig.engine.send(Command::Deck(d(deck), load)).unwrap();
        if playing {
            let toggle = DeckCommand::TogglePlay;
            rig.engine.send(Command::Deck(d(deck), toggle)).unwrap();
        }
    }
    rig.render();
    // Pick the slider up at the middle, then press SYNC.
    rig.batch(&[cc(6, 34, 64)]);
    let picked = rig.runtime.state().deck(d(0)).tempo;
    rig.batch(&[on(6, 41)]);
    let synced = rig.runtime.state().deck(d(0)).tempo;
    assert_ne!(synced, picked);
    rig.render();
    let ratio = rig.engine.deck(d(0)).tempo_ratio;
    assert!((ratio - 122.0 / 128.0).abs() < 1e-6);

    // The slider is still where it was: moving it away from the new value is held back.
    let stats = rig.batch(&[cc(6, 34, 127)]);
    assert_eq!(stats.applied, 0);
    assert_eq!(rig.runtime.state().deck(d(0)).tempo, synced);
    rig.render();
    assert_eq!(rig.engine.deck(d(0)).tempo_ratio, ratio);
}

const JOG_MAP: &str = r#"
[[input]]
midi = { cc = 33, channel = 1 }
action = "deck.jog_ring"
side = "left"
value = "relative_offset"

[[input]]
midi = { note = 54, channel = 1 }
action = "deck.jog_touch"
side = "left"

[[input]]
midi = { note = 28, channel = 1 }
action = "deck.hotcue_1"
side = "left"
"#;

fn jog_runtime(per_turn: &str) -> Runtime {
    let map = MAP.replacen(
        "port = \"Test\"\n",
        &format!("port = \"Test\"\n{per_turn}\n"),
        1,
    );
    Runtime::new(parse_mapping("test.toml", &format!("{map}{JOG_MAP}")).unwrap())
}

#[test]
fn steps_per_turn_reaches_control_state() {
    let runtime = jog_runtime("jog_steps_per_turn = 1000");
    assert_eq!(runtime.state().jog_steps_per_turn, 1000);
    // Without the key the default stays.
    assert_eq!(Runtime::new(mapping()).state().jog_steps_per_turn, 720);
}

#[test]
fn jog_steps_are_summed_per_run() {
    let position_after = |messages: &[MidiMessage]| {
        let mut rig = Rig::new();
        rig.runtime = jog_runtime("jog_steps_per_turn = 100");
        rig.runtime.attach(&mut rig.engine, &mut rig.out);
        let track = Arc::new(TrackBuffer::new(44_100, vec![0.5; 400_000 * 2]).unwrap());
        let load = DeckCommand::Load {
            track,
            memory: engine::TrackMemory::default(),
        };
        rig.engine.send(Command::Deck(d(0), load)).unwrap();
        rig.render();
        let before = rig.engine.deck(d(0)).position_frames;
        let stats = rig.batch(messages);
        rig.render();
        (stats, rig.engine.deck(d(0)).position_frames - before)
    };
    // Three +1 steps on the ring of a paused deck.
    let (stats, moved) = position_after(&[cc(0, 33, 65), cc(0, 33, 65), cc(0, 33, 65)]);
    assert_eq!(stats.applied, 1, "one Jog for the run");
    assert_eq!(stats.coalesced, 2);
    let (_, single) = position_after(&[cc(0, 33, 67)]);
    assert!(moved > 0.0);
    assert_eq!(moved, single, "the sum of the steps, as one move");
}

#[test]
fn held_jog_touch_and_hotcue_release_on_the_pressed_deck() {
    let mut rig = Rig::new();
    rig.runtime = jog_runtime("jog_steps_per_turn = 720");
    rig.runtime.attach(&mut rig.engine, &mut rig.out);
    rig.batch(&[on(0, 54), on(0, 28), on(0, 114)]);
    assert_eq!(rig.runtime.state().side(Side::Left).active_deck, d(2));
    rig.batch(&[off(0, 54), off(0, 28)]);
    let last = rig.runtime.last_resolved();
    assert_eq!(last[0].1, [(Action::JogTouch(d(0)), Input::Release)]);
    assert_eq!(
        last[1].1,
        [(
            Action::Hotcue(d(0), engine::HotcueSlot::ALL[0]),
            Input::Release
        )]
    );
}

/// Left pads: beat jump 1 on note 20 in beat-jump mode, loop roll 1 on the same note in
/// loop-roll mode; mode buttons on notes 21 and 22; sampler pad 1 on note 23 in slicer
/// mode, scoped to the left side.
const PAD_MAP: &str = r#"
[[input]]
midi = { note = 21, channel = 1 }
action = "pads.mode_beat_jump"
side = "left"

[[input]]
midi = { note = 22, channel = 1 }
action = "pads.mode_loop_roll"
side = "left"

[[input]]
midi = { note = 20, channel = 1 }
action = "deck.beat_jump_1"
side = "left"
pad_mode = "beat_jump"

[[input]]
midi = { note = 20, channel = 1 }
action = "deck.loop_roll_1"
side = "left"
pad_mode = "loop_roll"

[[input]]
midi = { note = 23, channel = 1 }
action = "sampler.play_1"
side = "left"
pad_mode = "slicer"
"#;

fn pad_runtime() -> Runtime {
    Runtime::new(parse_mapping("test.toml", &format!("{MAP}{PAD_MAP}")).unwrap())
}

fn resolved_actions(rig: &Rig) -> Vec<Action> {
    rig.runtime
        .last_resolved()
        .iter()
        .flat_map(|(_, resolved)| resolved.iter().map(|&(action, _)| action))
        .collect()
}

#[test]
fn pad_mode_condition_selects_the_binding() {
    let mut rig = Rig::new();
    rig.runtime = pad_runtime();
    rig.runtime.attach(&mut rig.engine, &mut rig.out);
    // Hardware mode (the start): the pad drives nothing.
    rig.batch(&[on(0, 20), off(0, 20)]);
    assert!(resolved_actions(&rig).is_empty());
    rig.batch(&[on(0, 21)]);
    rig.batch(&[on(0, 20), off(0, 20)]);
    let jump = Action::new(
        actions::ActionKind::BeatJump(engine::HotcueSlot::ALL[0]),
        actions::Target::Deck(d(0)),
    )
    .unwrap();
    assert_eq!(resolved_actions(&rig), [jump, jump]);
    rig.batch(&[on(0, 22)]);
    rig.batch(&[on(0, 20), off(0, 20)]);
    let roll = Action::new(
        actions::ActionKind::LoopRoll(engine::HotcueSlot::ALL[0]),
        actions::Target::Deck(d(0)),
    )
    .unwrap();
    assert_eq!(resolved_actions(&rig), [roll, roll]);
    // The sampler pad is scoped to slicer mode, which the left side is not in.
    rig.batch(&[on(0, 23)]);
    assert!(resolved_actions(&rig).is_empty());
}

#[test]
fn release_after_a_pad_mode_change_reaches_the_pressed_action() {
    let mut rig = Rig::new();
    rig.runtime = pad_runtime();
    rig.runtime.attach(&mut rig.engine, &mut rig.out);
    rig.batch(&[on(0, 22)]);
    rig.batch(&[on(0, 20)]);
    // The mode changes while the pad is held.
    rig.batch(&[on(0, 21)]);
    rig.batch(&[off(0, 20)]);
    let roll = Action::new(
        actions::ActionKind::LoopRoll(engine::HotcueSlot::ALL[0]),
        actions::Target::Deck(d(0)),
    )
    .unwrap();
    assert_eq!(
        rig.runtime.last_resolved()[0].1,
        [(roll, Input::Release)],
        "the release ends the roll, not a beat jump"
    );
}
