use actions::{Action, ActionKind, ControlState, Input, Side, apply};
use controller::MidiMessage;
use controller::mapping::{Resolved, Resolver, parse_mapping};
use engine::{DeckId, EngineConfig, EngineController, EngineRenderer, engine};

/// A resolver over a mapping, with the control state and engine it drives. Every resolved
/// action is applied before the next message, as the runtime does.
struct Rig {
    resolver: Resolver,
    state: ControlState,
    engine: EngineController,
    renderer: EngineRenderer,
}

impl Rig {
    /// A rig over the `[[input]]` bindings in `inputs`.
    fn new(inputs: &str) -> Self {
        let text = format!("[controller]\nname = \"Test\"\nport = \"Test\"\n\n{inputs}");
        let mapping = parse_mapping("test.toml", &text).unwrap();
        let (engine, renderer) = engine(EngineConfig {
            sample_rate: 44_100,
            max_block_frames: 64,
        });
        Self {
            resolver: Resolver::new(&mapping),
            state: ControlState::default(),
            engine,
            renderer,
        }
    }

    /// Resolves `msg`, applies the result and returns it.
    fn send(&mut self, msg: MidiMessage) -> Resolved {
        let resolved = self.resolver.resolve(&msg, &self.state);
        for (action, input) in &resolved {
            apply(action, *input, &mut self.state, &mut self.engine);
        }
        let mut master = [0.0; 128];
        let mut headphones = [0.0; 128];
        self.renderer.render(&mut master, &mut headphones);
        resolved
    }
}

fn d(index: usize) -> DeckId {
    DeckId::new(index).unwrap()
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

fn cc(channel: u8, controller: u8, value: u8) -> MidiMessage {
    MidiMessage::ControlChange {
        channel,
        controller,
        value,
    }
}

/// The value of a single absolute action in `resolved`.
fn absolute(resolved: &Resolved) -> f32 {
    match resolved.as_slice() {
        [(_, Input::Absolute(v))] => *v,
        other => panic!("expected one absolute value, got {other:?}"),
    }
}

/// SHIFT on note 63 and the deck toggle on note 114, both for the left side (channel 1).
const LEFT_SIDE: &str = r#"
[[input]]
midi = { note = 63, channel = 1 }
action = "modifier.shift"
side = "left"

[[input]]
midi = { note = 114, channel = 1 }
action = "side.deck_toggle"
side = "left"
"#;

#[test]
fn note_binding_press_and_release() {
    let mut rig = Rig::new(
        r#"
[[input]]
midi = { note = 12, channel = 1 }
action = "deck.cue"
deck = 1
"#,
    );
    assert_eq!(rig.send(on(0, 12)), vec![(Action::Cue(d(0)), Input::Press)]);
    assert_eq!(
        rig.send(off(0, 12)),
        vec![(Action::Cue(d(0)), Input::Release)]
    );
}

#[test]
fn side_binding_follows_active_deck() {
    let mut rig = Rig::new(&format!(
        r#"{LEFT_SIDE}
[[input]]
midi = {{ note = 11, channel = 1 }}
action = "deck.play_pause"
side = "left"
"#
    ));
    assert_eq!(
        rig.send(on(0, 11)),
        vec![(Action::PlayPause(d(0)), Input::Press)]
    );
    rig.send(off(0, 11));
    assert_eq!(
        rig.send(on(0, 114)),
        vec![(Action::DeckToggle(Side::Left), Input::Press)]
    );
    rig.send(off(0, 114));
    assert_eq!(rig.state.side(Side::Left).active_deck, d(2));
    assert_eq!(
        rig.send(on(0, 11)),
        vec![(Action::PlayPause(d(2)), Input::Press)]
    );
}

#[test]
fn shift_condition_selects_binding() {
    let mut rig = Rig::new(&format!(
        r#"{LEFT_SIDE}
[[input]]
midi = {{ note = 11, channel = 1 }}
action = "deck.play_pause"
side = "left"
shift = false

[[input]]
midi = {{ note = 11, channel = 1 }}
action = "deck.return_to_start"
side = "left"
shift = true
"#
    ));
    assert_eq!(
        rig.send(on(0, 11)),
        vec![(Action::PlayPause(d(0)), Input::Press)]
    );
    rig.send(off(0, 11));
    assert_eq!(
        rig.send(on(0, 63)),
        vec![(Action::Shift(Side::Left), Input::Press)]
    );
    assert_eq!(
        rig.send(on(0, 11)),
        vec![(Action::ReturnToStart(d(0)), Input::Press)]
    );
    rig.send(off(0, 11));
    assert_eq!(
        rig.send(off(0, 63)),
        vec![(Action::Shift(Side::Left), Input::Release)]
    );
    assert_eq!(
        rig.send(on(0, 11)),
        vec![(Action::PlayPause(d(0)), Input::Press)]
    );
}

#[test]
fn active_deck_condition() {
    let mut rig = Rig::new(&format!(
        r#"{LEFT_SIDE}
[[input]]
midi = {{ note = 20, channel = 1 }}
action = "deck.cue"
deck = 3
active_deck = 3
"#
    ));
    assert!(rig.send(on(0, 20)).is_empty());
    rig.send(off(0, 20));
    rig.send(on(0, 114));
    rig.send(off(0, 114));
    assert_eq!(rig.send(on(0, 20)), vec![(Action::Cue(d(2)), Input::Press)]);
}

#[test]
fn one_message_can_fire_two_bindings() {
    let mut rig = Rig::new(
        r#"
[[input]]
midi = { note = 30, channel = 1 }
action = "deck.play_pause"
deck = 1

[[input]]
midi = { note = 30, channel = 1 }
action = "deck.play_pause"
deck = 2
"#,
    );
    assert_eq!(
        rig.send(on(0, 30)),
        vec![
            (Action::PlayPause(d(0)), Input::Press),
            (Action::PlayPause(d(1)), Input::Press),
        ]
    );
}

#[test]
fn unmapped_message_resolves_to_nothing() {
    let mut rig = Rig::new(
        r#"
[[input]]
midi = { note = 11, channel = 1 }
action = "deck.play_pause"
deck = 1
"#,
    );
    assert!(rig.send(on(1, 11)).is_empty());
    assert!(rig.send(on(0, 12)).is_empty());
    assert!(rig.send(cc(0, 11, 64)).is_empty());
    assert!(
        rig.send(MidiMessage::PitchBend {
            channel: 0,
            value: 8192
        })
        .is_empty()
    );
}

#[test]
fn release_goes_to_deck_that_got_the_press() {
    let mut rig = Rig::new(&format!(
        r#"{LEFT_SIDE}
[[input]]
midi = {{ note = 12, channel = 1 }}
action = "deck.cue"
side = "left"
"#
    ));
    assert_eq!(rig.send(on(0, 12)), vec![(Action::Cue(d(0)), Input::Press)]);
    rig.send(on(0, 114));
    rig.send(off(0, 114));
    assert_eq!(rig.state.side(Side::Left).active_deck, d(2));
    assert_eq!(
        rig.send(off(0, 12)),
        vec![(Action::Cue(d(0)), Input::Release)]
    );
    // The next press goes to the newly active deck.
    assert_eq!(rig.send(on(0, 12)), vec![(Action::Cue(d(2)), Input::Press)]);
}

#[test]
fn release_goes_to_binding_that_got_the_press_after_shift_change() {
    let mut rig = Rig::new(&format!(
        r#"{LEFT_SIDE}
[[input]]
midi = {{ note = 12, channel = 1 }}
action = "deck.cue"
side = "left"
shift = false

[[input]]
midi = {{ note = 12, channel = 1 }}
action = "deck.play_pause"
side = "left"
shift = true
"#
    ));
    rig.send(on(0, 12));
    rig.send(on(0, 63));
    assert_eq!(
        rig.send(off(0, 12)),
        vec![(Action::Cue(d(0)), Input::Release)]
    );
}

#[test]
fn cc14_combines_msb_lsb() {
    let mut rig = Rig::new(
        r#"
[[input]]
midi = { cc14 = [19, 51], channel = 1 }
action = "channel.fader"
deck = 1
soft_takeover = false
"#,
    );
    rig.send(cc(0, 19, 0x7F));
    assert_eq!(absolute(&rig.send(cc(0, 51, 0x7F))), 1.0);
    // The MSB is emitted combined with the last LSB seen.
    assert_eq!(
        absolute(&rig.send(cc(0, 19, 0x40))),
        ((0x40 << 7) | 0x7F) as f32 / 16383.0
    );
    assert_eq!(absolute(&rig.send(cc(0, 51, 0x00))), 8192.0 / 16383.0);
}

#[test]
fn relative_twos_and_offset() {
    let mut rig = Rig::new(
        r#"
[[input]]
midi = { cc = 33, channel = 1 }
action = "deck.tempo"
deck = 1
value = "relative_twos"

[[input]]
midi = { cc = 34, channel = 1 }
action = "deck.tempo"
deck = 1
value = "relative_offset"
"#,
    );
    let rel = |r: Resolved| match r.as_slice() {
        [(Action::Tempo(_), Input::Relative(n))] => *n,
        other => panic!("{other:?}"),
    };
    assert_eq!(rel(rig.send(cc(0, 33, 1))), 1);
    assert_eq!(rel(rig.send(cc(0, 33, 63))), 63);
    assert_eq!(rel(rig.send(cc(0, 33, 127))), -1);
    assert_eq!(rel(rig.send(cc(0, 33, 65))), -63);
    assert_eq!(rel(rig.send(cc(0, 34, 65))), 1);
    assert_eq!(rel(rig.send(cc(0, 34, 63))), -1);
    assert_eq!(rel(rig.send(cc(0, 34, 64))), 0);
}

#[test]
fn invert_absolute() {
    let mut rig = Rig::new(
        r#"
[[input]]
midi = { cc = 8, channel = 1 }
action = "master.crossfader"
invert = true
soft_takeover = false
"#,
    );
    assert_eq!(absolute(&rig.send(cc(0, 8, 127))), 0.0);
    assert_eq!(absolute(&rig.send(cc(0, 8, 0))), 1.0);
    assert_eq!(rig.state.crossfader, 1.0);
}

const FADERS: &str = r#"
[[input]]
midi = { cc = 19, channel = 1 }
action = "channel.fader"
side = "left"
"#;

/// The CC value closest to `v`.
fn cc_of(v: f32) -> u8 {
    (v * 127.0).round() as u8
}

#[test]
fn takeover_blocks_until_within_tolerance() {
    let mut rig = Rig::new(FADERS);
    rig.state.deck_mut(d(0)).fader = 0.5;
    assert!(rig.send(cc(0, 19, 0)).is_empty());
    assert!(rig.send(cc(0, 19, 20)).is_empty());
    let waiting: Vec<_> = rig.resolver.takeover().waiting().collect();
    assert_eq!(
        waiting,
        vec![((ActionKind::Fader, Some(d(0))), 0.5, 20.0 / 127.0)]
    );
    // 64/127 is within 0.02 of 0.5.
    assert_eq!(absolute(&rig.send(cc(0, 19, 64))), 64.0 / 127.0);
    assert_eq!(rig.resolver.takeover().waiting().count(), 0);
    // Disarmed: any move passes.
    assert_eq!(absolute(&rig.send(cc(0, 19, 0))), 0.0);
}

#[test]
fn takeover_picks_up_on_crossing() {
    let mut rig = Rig::new(FADERS);
    rig.state.deck_mut(d(0)).fader = 0.5;
    assert!(rig.send(cc(0, 19, 40)).is_empty());
    // 40 -> 90 jumps over 0.5 without landing near it.
    assert_eq!(absolute(&rig.send(cc(0, 19, 90))), 90.0 / 127.0);
    assert_eq!(rig.state.deck(d(0)).fader, 90.0 / 127.0);
}

#[test]
fn takeover_arms_on_deck_switch() {
    let mut rig = Rig::new(&format!("{LEFT_SIDE}{FADERS}"));
    rig.state.deck_mut(d(2)).fader = 0.3;
    // Deck 1's fader is at 1.0 and the physical fader agrees.
    assert_eq!(absolute(&rig.send(cc(0, 19, 127))), 1.0);
    rig.send(on(0, 114));
    rig.send(off(0, 114));
    assert!(rig.send(cc(0, 19, cc_of(0.9))).is_empty());
    assert!(rig.send(cc(0, 19, cc_of(0.6))).is_empty());
    assert_eq!(rig.state.deck(d(2)).fader, 0.3);
    // 39/127 ≈ 0.307, within 0.02 of 0.3.
    assert_eq!(absolute(&rig.send(cc(0, 19, 39))), 39.0 / 127.0);
    assert_eq!(
        rig.send(cc(0, 19, cc_of(0.5))),
        vec![(Action::Fader(d(2)), Input::Absolute(64.0 / 127.0))]
    );
    // Deck 1 kept its value.
    assert_eq!(rig.state.deck(d(0)).fader, 1.0);
}

#[test]
fn takeover_arms_when_value_changed_elsewhere() {
    let mut rig = Rig::new(FADERS);
    assert_eq!(absolute(&rig.send(cc(0, 19, 127))), 1.0);
    assert_eq!(absolute(&rig.send(cc(0, 19, 100))), 100.0 / 127.0);
    // The UI moves the fader.
    rig.state.deck_mut(d(0)).fader = 0.2;
    assert!(rig.send(cc(0, 19, 99)).is_empty());
    assert!(rig.send(cc(0, 19, 60)).is_empty());
    assert_eq!(rig.state.deck(d(0)).fader, 0.2);
    // 60 -> 20 crosses 0.2.
    assert_eq!(absolute(&rig.send(cc(0, 19, 20))), 20.0 / 127.0);
}

#[test]
fn reattach_arms_takeover() {
    let mut rig = Rig::new(FADERS);
    assert_eq!(absolute(&rig.send(cc(0, 19, 127))), 1.0);
    rig.resolver.attach();
    assert!(rig.send(cc(0, 19, 60)).is_empty());
}
