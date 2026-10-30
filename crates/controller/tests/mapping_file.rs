use actions::{Action, ActionKind, Side, StateKey, Target};
use controller::mapping::{
    ControllerInfo, InitAction, InputBinding, Mapping, MidiSource, OutputBinding, OutputMidi,
    ValueKind, parse_mapping,
};
use engine::DeckId;

const FULL: &str = r#"
[controller]
name = "Pioneer DDJ-SB3"
port = "DDJ-SB3"

[[init]]
action = "master.level"
value = 1.0

[[input]]
midi = { note = 11, channel = 1 }
action = "deck.play_pause"
deck = 1
shift = false
active_deck = 1
value = "button"
invert = false
soft_takeover = true

[[input]]
midi = { cc14 = [0, 32], channel = 2 }
action = "channel.fader"
side = "right"

[[input]]
midi = { pitchbend = true, channel = 3 }
action = "master.crossfader"
invert = true
soft_takeover = false

[[input]]
midi = { cc = 19, channel = 16 }
action = "deck.tempo"
side = "left"
value = "absolute"

[[output]]
state = "deck.play_led"
deck = 1
midi = { note = 11, channel = 1 }
on = 127
off = 0

[[output]]
state = "side.alt_deck"
side = "left"
midi = { cc = 5, channel = 1 }
on = 100
off = 1
"#;

fn d(index: usize) -> DeckId {
    DeckId::new(index).unwrap()
}

#[test]
fn parses_full_example() {
    let mapping = parse_mapping("sb3.toml", FULL).unwrap();
    let expected = Mapping {
        controller: ControllerInfo {
            name: "Pioneer DDJ-SB3".into(),
            port: "DDJ-SB3".into(),
        },
        init: vec![InitAction {
            action: Action::MasterLevel,
            value: 1.0,
        }],
        inputs: vec![
            InputBinding {
                midi: MidiSource::Note {
                    channel: 0,
                    note: 11,
                },
                kind: ActionKind::PlayPause,
                target: Target::Deck(d(0)),
                shift: Some(false),
                active_deck: Some(d(0)),
                value: ValueKind::Button,
                invert: false,
                soft_takeover: false,
            },
            InputBinding {
                midi: MidiSource::Cc14 {
                    channel: 1,
                    msb: 0,
                    lsb: 32,
                },
                kind: ActionKind::Fader,
                target: Target::Side(Side::Right),
                shift: None,
                active_deck: None,
                value: ValueKind::Absolute,
                invert: false,
                soft_takeover: true,
            },
            InputBinding {
                midi: MidiSource::PitchBend { channel: 2 },
                kind: ActionKind::Crossfader,
                target: Target::Global,
                shift: None,
                active_deck: None,
                value: ValueKind::Absolute,
                invert: true,
                soft_takeover: false,
            },
            InputBinding {
                midi: MidiSource::Cc {
                    channel: 15,
                    cc: 19,
                },
                kind: ActionKind::Tempo,
                target: Target::Side(Side::Left),
                shift: None,
                active_deck: None,
                value: ValueKind::Absolute,
                invert: false,
                soft_takeover: true,
            },
        ],
        outputs: vec![
            OutputBinding {
                state: StateKey::DeckPlayLed,
                target: Some(Target::Deck(d(0))),
                midi: OutputMidi::Note {
                    channel: 0,
                    note: 11,
                },
                on: 127,
                off: 0,
            },
            OutputBinding {
                state: StateKey::SideAltDeck,
                target: Some(Target::Side(Side::Left)),
                midi: OutputMidi::Cc { channel: 0, cc: 5 },
                on: 100,
                off: 1,
            },
        ],
    };
    assert_eq!(mapping, expected);
}

#[test]
fn controller_only_file_parses() {
    let mapping = parse_mapping("x.toml", "[controller]\nname = \"X\"\nport = \"X\"\n").unwrap();
    assert!(mapping.init.is_empty() && mapping.inputs.is_empty() && mapping.outputs.is_empty());
}

/// Parses a file with one `[[input]]` made of `body`, and returns the error message.
fn input_error(body: &str) -> String {
    let text = format!("[controller]\nname = \"X\"\nport = \"X\"\n\n[[input]]\n{body}\n");
    let err = parse_mapping("bad.toml", &text).unwrap_err().to_string();
    assert!(err.contains("bad.toml"), "{err}");
    assert!(err.contains("input[0]"), "{err}");
    err
}

#[test]
fn error_unknown_action() {
    let err = input_error("midi = { note = 1, channel = 1 }\naction = \"deck.fly\"\ndeck = 1");
    assert!(err.contains("unknown action `deck.fly`"), "{err}");
}

#[test]
fn error_channel_out_of_range() {
    let err = input_error("midi = { note = 1, channel = 17 }\naction = \"deck.cue\"\ndeck = 1");
    assert!(err.contains("channel 17 is outside 1-16"), "{err}");
    let err = input_error("midi = { note = 1, channel = 0 }\naction = \"deck.cue\"\ndeck = 1");
    assert!(err.contains("channel 0 is outside 1-16"), "{err}");
}

#[test]
fn error_number_above_127() {
    let err = input_error("midi = { note = 128, channel = 1 }\naction = \"deck.cue\"\ndeck = 1");
    assert!(err.contains("note 128 is above 127"), "{err}");
    let err = input_error(
        "midi = { cc14 = [0, 200], channel = 1 }\naction = \"channel.fader\"\ndeck = 1",
    );
    assert!(err.contains("cc14 lsb 200 is above 127"), "{err}");
}

#[test]
fn error_deck_and_side_both_set() {
    let err = input_error(
        "midi = { note = 1, channel = 1 }\naction = \"deck.cue\"\ndeck = 1\nside = \"left\"",
    );
    assert!(err.contains("`deck` and `side` are both set"), "{err}");
}

#[test]
fn error_deck_action_without_target() {
    let err = input_error("midi = { note = 1, channel = 1 }\naction = \"deck.cue\"");
    assert!(
        err.contains("action `deck.cue` needs `deck` or `side`"),
        "{err}"
    );
}

#[test]
fn error_cc14_msb_equals_lsb() {
    let err =
        input_error("midi = { cc14 = [7, 7], channel = 1 }\naction = \"channel.fader\"\ndeck = 1");
    assert!(err.contains("cc14 msb and lsb are both 7"), "{err}");
}

#[test]
fn error_unknown_key_names_binding() {
    let err = input_error(
        "midi = { note = 1, channel = 1 }\naction = \"deck.cue\"\ndeck = 1\nlatch = true",
    );
    assert!(err.contains("unknown field `latch`"), "{err}");
}

#[test]
fn error_output_names_its_index() {
    let text = "[controller]\nname = \"X\"\nport = \"X\"\n\n[[output]]\nstate = \"deck.glow\"\ndeck = 1\nmidi = { note = 1, channel = 1 }\n";
    let err = parse_mapping("bad.toml", text).unwrap_err().to_string();
    assert!(
        err.contains("output[0]") && err.contains("unknown state `deck.glow`"),
        "{err}"
    );
}

#[test]
fn error_init_needs_value() {
    let text = "[controller]\nname = \"X\"\nport = \"X\"\n\n[[init]]\naction = \"master.level\"\n";
    let err = parse_mapping("bad.toml", text).unwrap_err().to_string();
    assert!(
        err.contains("init[0]") && err.contains("`value` is missing"),
        "{err}"
    );
}

#[test]
fn error_deck_overflow_is_a_load_error() {
    let err = input_error(
        "midi = { note = 1, channel = 1 }\naction = \"deck.cue\"\ndeck = -9223372036854775808",
    );
    assert!(
        err.contains("deck -9223372036854775808 is outside 1-4"),
        "{err}"
    );
}

#[test]
fn error_channel_overflow_is_a_load_error() {
    let err = input_error(
        "midi = { note = 1, channel = -9223372036854775808 }\naction = \"deck.cue\"\ndeck = 1",
    );
    assert!(
        err.contains("channel -9223372036854775808 is outside 1-16"),
        "{err}"
    );
}

#[test]
fn error_relative_value_not_accepted() {
    let err = input_error(
        "midi = { cc = 1, channel = 1 }\naction = \"deck.tempo\"\ndeck = 1\nvalue = \"relative_twos\"",
    );
    assert!(
        err.contains("action `deck.tempo` doesn't accept relative values"),
        "{err}"
    );
}

#[test]
fn error_button_on_absolute_action() {
    let err = input_error("midi = { note = 1, channel = 1 }\naction = \"channel.fader\"\ndeck = 1");
    assert!(
        err.contains("action `channel.fader` doesn't accept button values"),
        "{err}"
    );
}

#[test]
fn error_absolute_on_press_action() {
    let err = input_error("midi = { cc = 1, channel = 1 }\naction = \"deck.play_pause\"\ndeck = 1");
    assert!(
        err.contains("action `deck.play_pause` doesn't accept absolute values"),
        "{err}"
    );
}

/// Parses a file with one `[[output]]` made of `body`, and returns the error message.
fn output_error(body: &str) -> String {
    let text = format!(
        "[controller]\nname = \"X\"\nport = \"X\"\n\n[[output]]\nmidi = {{ note = 1, channel = 1 }}\n{body}\n"
    );
    let err = parse_mapping("bad.toml", &text).unwrap_err().to_string();
    assert!(
        err.contains("bad.toml") && err.contains("output[0]"),
        "{err}"
    );
    err
}

#[test]
fn error_output_target_must_fit_state_scope() {
    let err = output_error("state = \"deck.play_led\"");
    assert!(
        err.contains("state `deck.play_led` needs `deck` or `side`"),
        "{err}"
    );
    let err = output_error("state = \"side.alt_deck\"\ndeck = 1");
    assert!(err.contains("state `side.alt_deck` needs `side`"), "{err}");
    let err = output_error("state = \"side.shift\"");
    assert!(err.contains("state `side.shift` needs `side`"), "{err}");
    let err = output_error("state = \"master.headphone_master\"\nside = \"left\"");
    assert!(
        err.contains("state `master.headphone_master` takes no `deck` or `side`"),
        "{err}"
    );
}

#[test]
fn outputs_accept_targets_that_fit() {
    for body in [
        "state = \"deck.play_led\"\nside = \"right\"",
        "state = \"channel.pfl\"\ndeck = 4",
        "state = \"master.headphone_master\"",
    ] {
        let text = format!(
            "[controller]\nname = \"X\"\nport = \"X\"\n\n[[output]]\nmidi = {{ note = 1, channel = 1 }}\n{body}\n"
        );
        parse_mapping("ok.toml", &text).unwrap();
    }
}
