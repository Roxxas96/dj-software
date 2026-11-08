use actions::{Action, ActionKind, PadMode, Side, StateKey, Target};
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
            jog_steps_per_turn: None,
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
                pad_mode: None,
                pad_scope: None,
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
                pad_mode: None,
                pad_scope: None,
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
                pad_mode: None,
                pad_scope: None,
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
                pad_mode: None,
                pad_scope: None,
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
fn relative_values_drive_browser_scroll_only() {
    for (value, kind) in [
        ("relative_twos", ValueKind::RelativeTwos),
        ("relative_offset", ValueKind::RelativeOffset),
    ] {
        let text = format!(
            "[controller]\nname = \"X\"\nport = \"X\"\n\n[[input]]\n\
             midi = {{ cc = 0x40, channel = 7 }}\naction = \"browser.scroll\"\nvalue = \"{value}\"\n"
        );
        let mapping = parse_mapping("ok.toml", &text).unwrap();
        let binding = &mapping.inputs[0];
        assert_eq!(binding.kind, ActionKind::BrowserScroll);
        assert_eq!(binding.target, Target::Global);
        assert_eq!(binding.value, kind);
        assert!(!binding.soft_takeover);
    }
    // The knob's push is a button, and `browser.scroll` takes no button or absolute values.
    let err = input_error("midi = { cc = 0x40, channel = 7 }\naction = \"browser.scroll\"");
    assert!(
        err.contains("action `browser.scroll` doesn't accept absolute values"),
        "{err}"
    );
    let err = input_error(
        "midi = { cc = 0x40, channel = 7 }\naction = \"browser.toggle_focus\"\nvalue = \"relative_twos\"",
    );
    assert!(
        err.contains("action `browser.toggle_focus` doesn't accept relative values"),
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

const JOG_BINDING: &str = r#"
[[input]]
midi = { cc = 33, channel = 1 }
action = "deck.jog_ring"
deck = 1
value = "relative_offset"
"#;

#[test]
fn jog_binding_requires_steps_per_turn() {
    let head = "[controller]\nname = \"X\"\nport = \"X\"\n";
    for kind in ["jog_top", "jog_ring", "jog_search"] {
        let binding = JOG_BINDING.replace("jog_ring", kind);
        let err = parse_mapping("bad.toml", &format!("{head}{binding}")).unwrap_err();
        assert!(
            err.to_string()
                .contains("jog_steps_per_turn is required by jog bindings"),
            "{err}"
        );
    }
    let ok = format!("{head}jog_steps_per_turn = 1000\n{JOG_BINDING}");
    let mapping = parse_mapping("ok.toml", &ok).unwrap();
    assert_eq!(mapping.controller.jog_steps_per_turn, Some(1000));
    // Jog touch is a button and needs no resolution.
    let touch =
        "[[input]]\nmidi = { note = 1, channel = 1 }\naction = \"deck.jog_touch\"\ndeck = 1\n";
    parse_mapping("ok.toml", &format!("{head}{touch}")).unwrap();
}

#[test]
fn steps_per_turn_must_be_in_range() {
    for bad in ["0", "10001", "-5"] {
        let text =
            format!("[controller]\nname = \"X\"\nport = \"X\"\njog_steps_per_turn = {bad}\n");
        let err = parse_mapping("bad.toml", &text).unwrap_err().to_string();
        assert!(err.contains("jog_steps_per_turn"), "{err}");
    }
}

fn pad_mode_error(binding: &str) -> String {
    let text = format!("[controller]\nname = \"X\"\nport = \"X\"\n\n[[input]]\n{binding}");
    parse_mapping("bad.toml", &text).unwrap_err().to_string()
}

#[test]
fn pad_mode_needs_a_side() {
    let err = pad_mode_error(
        "midi = { note = 1, channel = 1 }\naction = \"sampler.play_1\"\npad_mode = \"slicer\"",
    );
    assert!(
        err.contains("input[0]") && err.contains("pad_mode needs a side"),
        "{err}"
    );
}

#[test]
fn unknown_pad_mode_lists_the_ids() {
    let err = pad_mode_error(
        "midi = { note = 1, channel = 1 }\naction = \"deck.beat_jump_1\"\nside = \"left\"\npad_mode = \"drums\"",
    );
    assert!(
        err.contains("unknown pad_mode `drums`")
            && err.contains("hardware, beat_jump, loop_roll, slicer"),
        "{err}"
    );
}

#[test]
fn pad_mode_on_a_master_action_uses_the_side_as_scope() {
    let head = "[controller]\nname = \"X\"\nport = \"X\"\n\n[[input]]\n";
    let ok = format!(
        "{head}midi = {{ note = 1, channel = 1 }}\naction = \"sampler.play_3\"\nside = \"right\"\npad_mode = \"slicer\""
    );
    let mapping = parse_mapping("ok.toml", &ok).unwrap();
    let binding = mapping.inputs[0];
    assert_eq!(binding.target, Target::Global, "the action stays global");
    assert_eq!(binding.pad_mode, Some(PadMode::Slicer));
    assert_eq!(binding.side(), Some(Side::Right));
    // A side without a pad mode on a master action is still an error.
    let err = pad_mode_error(
        "midi = { note = 1, channel = 1 }\naction = \"sampler.play_3\"\nside = \"right\"",
    );
    assert!(err.contains("takes no `deck` or `side`"), "{err}");
}

#[test]
fn pad_mode_on_a_deck_binding_scopes_to_its_side() {
    let head = "[controller]\nname = \"X\"\nport = \"X\"\n\n[[input]]\n";
    let text = format!(
        "{head}midi = {{ note = 1, channel = 1 }}\naction = \"deck.beat_jump_1\"\ndeck = 4\npad_mode = \"beat_jump\""
    );
    let binding = parse_mapping("ok.toml", &text).unwrap().inputs[0];
    assert_eq!(binding.pad_mode, Some(PadMode::BeatJump));
    assert_eq!(binding.side(), Some(Side::Right));
}

fn parse_inputs(body: &str) -> Vec<InputBinding> {
    let text = format!("[controller]\nname = \"X\"\nport = \"X\"\n\n{body}\n");
    parse_mapping("range.toml", &text).unwrap().inputs
}

#[test]
fn note_range_expands_with_n() {
    let inputs = parse_inputs(
        r#"
[[input]]
midi = { channel = 8, notes = [0x00, 0x07] }
action = "deck.hotcue_{n}"
side = "left"
pad_mode = "hardware"
"#,
    );
    assert_eq!(inputs.len(), 8);
    for (i, binding) in inputs.iter().enumerate() {
        assert_eq!(
            binding.midi,
            MidiSource::Note {
                channel: 7,
                note: i as u8
            }
        );
        assert_eq!(
            binding.kind,
            ActionKind::from_id(&format!("deck.hotcue_{}", i + 1)).unwrap()
        );
    }
}

#[test]
fn range_keeps_other_keys_and_order() {
    let inputs = parse_inputs(
        r#"
[[input]]
midi = { channel = 1, note = 0x0B }
action = "deck.play_pause"
side = "left"

[[input]]
midi = { channel = 9, notes = [0x38, 0x3A] }
action = "sampler.stop_{n}"
side = "right"
pad_mode = "hardware"

[[input]]
midi = { channel = 1, note = 0x0C }
action = "deck.cue"
side = "left"
"#,
    );
    let notes: Vec<u8> = inputs
        .iter()
        .map(|b| match b.midi {
            MidiSource::Note { note, .. } => note,
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(notes, [0x0B, 0x38, 0x39, 0x3A, 0x0C]);
    for binding in &inputs[1..4] {
        assert_eq!(binding.pad_scope, Some(Side::Right));
        assert_eq!(binding.pad_mode, Some(PadMode::Hardware));
        assert_eq!(binding.value, ValueKind::Button);
    }
    assert_eq!(
        inputs[1].kind,
        ActionKind::from_id("sampler.stop_1").unwrap()
    );
    assert_eq!(
        inputs[3].kind,
        ActionKind::from_id("sampler.stop_3").unwrap()
    );
}

#[test]
fn bad_note_ranges_are_errors() {
    let cases = [
        (
            "midi = { channel = 1, notes = [0, 7], note = 3 }\naction = \"deck.hotcue_{n}\"\nside = \"left\"",
            "can't be combined",
        ),
        (
            "midi = { channel = 1, notes = [0, 7], cc = 3 }\naction = \"deck.hotcue_{n}\"\nside = \"left\"",
            "can't be combined",
        ),
        (
            "midi = { channel = 1, notes = [7, 0] }\naction = \"deck.hotcue_{n}\"\nside = \"left\"",
            "reversed",
        ),
        (
            "midi = { channel = 1, notes = [0, 128] }\naction = \"deck.hotcue_{n}\"\nside = \"left\"",
            "above 127",
        ),
        (
            "midi = { channel = 1, notes = [-1, 7] }\naction = \"deck.hotcue_{n}\"\nside = \"left\"",
            "negative",
        ),
        (
            "midi = { channel = 1, notes = [0, 1, 2] }\naction = \"deck.hotcue_{n}\"\nside = \"left\"",
            "two integers",
        ),
        (
            "midi = { channel = 1, note = 3 }\naction = \"deck.hotcue_{n}\"\nside = \"left\"",
            "`{n}`",
        ),
        (
            "midi = { channel = 1, notes = [0, 8] }\naction = \"deck.hotcue_{n}\"\nside = \"left\"",
            "input[0] (note 0x08): unknown action `deck.hotcue_9`",
        ),
    ];
    for (body, expected) in cases {
        let text = format!("[controller]\nname = \"X\"\nport = \"X\"\n\n[[input]]\n{body}\n");
        let err = parse_mapping("bad.toml", &text).unwrap_err().to_string();
        assert!(
            err.contains("input[0]") && err.contains(expected),
            "{body}: {err}"
        );
    }
}

#[test]
fn expanded_binding_errors_name_the_note() {
    let text = "[controller]\nname = \"X\"\nport = \"X\"\n\n[[input]]\nmidi = { channel = 1, notes = [2, 4] }\naction = \"deck.hotcue_{n}\"\nside = \"left\"\nvalue = \"absolute\"\n";
    let err = parse_mapping("bad.toml", text).unwrap_err().to_string();
    assert!(err.contains("input[0] (note 0x02):"), "{err}");
}

#[test]
fn sb3_mapping_expands_to_the_same_bindings() {
    let text = include_str!("../../../controllers/ddj-sb3/mapping.toml");
    let mapping = parse_mapping("sb3.toml", text).unwrap();
    let mut actual: Vec<String> = mapping
        .inputs
        .iter()
        .map(|b| {
            format!(
                "{:?} | {:?} | {:?} | {:?} | {:?}",
                b.midi, b.kind, b.target, b.pad_mode, b.shift
            )
        })
        .collect();
    let mut expected: Vec<String> = include_str!("fixtures/sb3_inputs.txt")
        .lines()
        .map(str::to_owned)
        .collect();
    actual.sort();
    expected.sort();
    assert_eq!(actual.len(), expected.len());
    assert_eq!(actual, expected);
}

// Mapping `extends` and `[[remove]]`.

const SB3: &str = include_str!("../../../controllers/ddj-sb3/mapping.toml");

fn sb3() -> Mapping {
    parse_mapping("sb3.toml", SB3).unwrap()
}

/// A fresh, empty folder for mapping files.
fn temp_folder(name: &str) -> std::path::PathBuf {
    let dir =
        std::env::temp_dir().join(format!("open-jockey-extends-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn note(channel: u8, note: u8) -> MidiSource {
    MidiSource::Note { channel, note }
}

#[test]
fn extends_inherits_everything() {
    // A child with nothing of its own is its parent.
    let child = parse_mapping("mine.toml", "extends = \"ddj-sb3\"\n").unwrap();
    assert_eq!(child, sb3());

    // Its [controller] keys override the parent's one by one; [[init]] and [[output]] are
    // inherited, and the child's own come after.
    let text = r#"
extends = "ddj-sb3"

[controller]
name = "My SB3"

[[init]]
action = "master.level"
value = 0.5

[[output]]
state = "deck.play_led"
deck = 1
midi = { note = 0x70, channel = 1 }
"#;
    let child = parse_mapping("mine.toml", text).unwrap();
    let parent = sb3();
    assert_eq!(child.controller.name, "My SB3");
    assert_eq!(child.controller.port, parent.controller.port);
    assert_eq!(
        child.controller.jog_steps_per_turn,
        parent.controller.jog_steps_per_turn
    );
    assert_eq!(child.init[..parent.init.len()], parent.init[..]);
    assert_eq!(child.init.len(), parent.init.len() + 1);
    assert_eq!(child.outputs[..parent.outputs.len()], parent.outputs[..]);
    assert_eq!(
        child.outputs.last().unwrap().midi,
        OutputMidi::Note {
            channel: 0,
            note: 0x70
        }
    );
    assert_eq!(child.inputs, parent.inputs);

    // A chain of files, the parent named by a path relative to the child's folder.
    let dir = temp_folder("chain");
    std::fs::create_dir_all(dir.join("base")).unwrap();
    std::fs::write(
        dir.join("base").join("b.toml"),
        "extends = \"ddj-sb3\"\n[controller]\nport = \"B port\"\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("a.toml"),
        "extends = \"base/b.toml\"\n[controller]\nname = \"A\"\n",
    )
    .unwrap();
    let a = controller::builtin::load_mapping(dir.join("a.toml").to_str().unwrap()).unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    assert_eq!(a.controller.name, "A");
    assert_eq!(a.controller.port, "B port");
    assert_eq!(a.inputs, parent.inputs);
}

#[test]
fn child_input_replaces_matching_bindings() {
    let parent = sb3();
    // SAMPLER pad 1 on the left: channel 8, note 0x30, hardware mode, no SHIFT condition.
    let pad = note(7, 0x30);
    let text = r#"
extends = "ddj-sb3"

[[input]]
midi = { channel = 8, note = 0x30 }
action = "deck.stem_toggle_vocals"
side = "left"
pad_mode = "hardware"
"#;
    let child = parse_mapping("mine.toml", text).unwrap();
    let on_pad = |m: &Mapping| -> Vec<(ActionKind, Option<PadMode>)> {
        m.inputs
            .iter()
            .filter(|b| b.midi == pad)
            .map(|b| (b.kind, b.pad_mode))
            .collect()
    };
    let before = on_pad(&parent);
    assert!(before.contains(&(
        ActionKind::SamplerPlay(engine::HotcueSlot::new(1).unwrap()),
        Some(PadMode::Hardware)
    )));
    let after = on_pad(&child);
    // The hardware binding is replaced; the software modes' bindings on the note stay.
    let mut expected: Vec<_> = before
        .iter()
        .copied()
        .filter(|(_, mode)| *mode != Some(PadMode::Hardware))
        .collect();
    expected.push((
        ActionKind::from_id("deck.stem_toggle_vocals").unwrap(),
        Some(PadMode::Hardware),
    ));
    assert_eq!(after, expected);
    // Every other inherited binding carries over unchanged, in order.
    let others = |m: &Mapping| -> Vec<InputBinding> {
        m.inputs.iter().copied().filter(|b| b.midi != pad).collect()
    };
    assert_eq!(others(&child), others(&parent));
    assert_eq!(child.inputs.len(), parent.inputs.len());

    // Matching is per expanded note, and `shift` is part of the match: a binding with
    // `shift = true` replaces nothing here (the inherited pads have no SHIFT condition).
    let text = r#"
extends = "ddj-sb3"

[[input]]
midi = { channel = 8, notes = [0x30, 0x31] }
action = "deck.hotcue_{n}"
side = "left"
pad_mode = "hardware"
shift = true
"#;
    let child = parse_mapping("mine.toml", text).unwrap();
    assert_eq!(child.inputs.len(), parent.inputs.len() + 2);
}

#[test]
fn remove_deletes_matching_bindings() {
    let parent = sb3();
    let left_pads: Vec<MidiSource> = (0x30..=0x37).map(|n| note(7, n)).collect();
    let count = |m: &Mapping, mode: Option<PadMode>| {
        m.inputs
            .iter()
            .filter(|b| left_pads.contains(&b.midi) && (mode.is_none() || b.pad_mode == mode))
            .count()
    };
    assert_eq!(count(&parent, Some(PadMode::Hardware)), 8);

    // With `pad_mode`, only that mode's bindings go.
    let text = r#"
extends = "ddj-sb3"

[[remove]]
midi = { channel = 8, notes = [0x30, 0x37] }
pad_mode = "hardware"
"#;
    let child = parse_mapping("mine.toml", text).unwrap();
    assert_eq!(count(&child, Some(PadMode::Hardware)), 0);
    assert_eq!(count(&child, None), count(&parent, None) - 8);
    assert_eq!(child.inputs.len(), parent.inputs.len() - 8);

    // Without `pad_mode` and `shift`, every binding of the message goes.
    let text = r#"
extends = "ddj-sb3"

[[remove]]
midi = { channel = 8, note = 0x30 }
"#;
    let child = parse_mapping("mine.toml", text).unwrap();
    assert!(child.inputs.iter().all(|b| b.midi != note(7, 0x30)));
    let on_note = parent
        .inputs
        .iter()
        .filter(|b| b.midi == note(7, 0x30))
        .count();
    assert!(on_note > 1, "the SB3 binds pad 1 in several modes");
    assert_eq!(child.inputs.len(), parent.inputs.len() - on_note);

    // A removed message can be bound again by the child.
    let text = r#"
extends = "ddj-sb3"

[[remove]]
midi = { channel = 8, note = 0x30 }

[[input]]
midi = { channel = 8, note = 0x30 }
action = "deck.stem_toggle_bass"
side = "left"
"#;
    let child = parse_mapping("mine.toml", text).unwrap();
    let on_pad: Vec<_> = child
        .inputs
        .iter()
        .filter(|b| b.midi == note(7, 0x30))
        .collect();
    assert_eq!(on_pad.len(), 1);
    assert_eq!(
        on_pad[0].kind,
        ActionKind::from_id("deck.stem_toggle_bass").unwrap()
    );

    // `[[remove]]` needs `extends`, and its entries are checked like bindings.
    let err = parse_mapping(
        "mine.toml",
        "[controller]\nname = \"X\"\nport = \"X\"\n\n[[remove]]\nmidi = { channel = 1, note = 1 }\n",
    )
    .unwrap_err()
    .to_string();
    assert!(
        err.contains("mine.toml") && err.contains("remove[0]") && err.contains("extends"),
        "{err}"
    );
    let err = parse_mapping(
        "mine.toml",
        "extends = \"ddj-sb3\"\n\n[[remove]]\nmidi = { channel = 8, note = 0x30 }\npad_mode = \"disco\"\n",
    )
    .unwrap_err()
    .to_string();
    assert!(
        err.contains("remove[0]") && err.contains("unknown pad_mode `disco`"),
        "{err}"
    );
}

#[test]
fn unknown_parent_and_cycles_are_errors() {
    let err = parse_mapping("mine.toml", "extends = \"ddj-nope\"\n").unwrap_err();
    assert_eq!(err.file, "mine.toml");
    assert!(
        err.message.contains("extends") && err.message.contains("ddj-nope"),
        "{err}"
    );

    // A file that extends itself, and two files that extend each other.
    let dir = temp_folder("cycle");
    std::fs::write(dir.join("self.toml"), "extends = \"self.toml\"\n").unwrap();
    std::fs::write(dir.join("a.toml"), "extends = \"b.toml\"\n").unwrap();
    std::fs::write(dir.join("b.toml"), "extends = \"./a.toml\"\n").unwrap();
    let load = |name: &str| {
        controller::builtin::load_mapping(dir.join(name).to_str().unwrap()).unwrap_err()
    };
    let self_err = load("self.toml");
    let pair_err = load("a.toml");
    std::fs::remove_dir_all(&dir).unwrap();
    assert!(self_err.file.ends_with("self.toml"), "{self_err}");
    assert!(self_err.message.contains("cycle"), "{self_err}");
    assert!(pair_err.file.ends_with("b.toml"), "{pair_err}");
    assert!(
        pair_err.message.contains("cycle") && pair_err.message.contains("a.toml"),
        "{pair_err}"
    );

    // Without `extends`, `[controller]` with `name` and `port` is still required.
    let err = parse_mapping("x.toml", "[controller]\nname = \"X\"\n").unwrap_err();
    assert!(err.to_string().contains("port"), "{err}");
}

#[test]
fn remove_matching_nothing_is_an_error() {
    // Pad 1 has no SHIFT binding on the SAMPLER bank: SHIFT + SAMPLER sends 0x38-0x3F.
    let text = r#"
extends = "ddj-sb3"

[[remove]]
midi = { channel = 8, note = 0x30 }
pad_mode = "hardware"

[[remove]]
midi = { channel = 8, notes = [0x30, 0x31] }
shift = true
"#;
    let err = parse_mapping("mine.toml", text).unwrap_err();
    assert_eq!(err.file, "mine.toml");
    assert!(
        err.message.contains("remove[1]") && err.message.contains("matches no inherited binding"),
        "{err}"
    );
    // A range is an error only if none of its notes matches.
    let text = r#"
extends = "ddj-sb3"

[[remove]]
midi = { channel = 8, notes = [0x37, 0x38] }
pad_mode = "beat_jump"
"#;
    parse_mapping("mine.toml", text).unwrap();
    // The child's own bindings can't be removed, only inherited ones.
    let text = r#"
extends = "ddj-sb3"

[[input]]
midi = { channel = 16, note = 0x7F }
action = "master.crossfader"
value = "absolute"

[[remove]]
midi = { channel = 16, note = 0x7F }
"#;
    let err = parse_mapping("mine.toml", text).unwrap_err().to_string();
    assert!(
        err.contains("remove[0]: matches no inherited binding"),
        "{err}"
    );
}

/// The example user file in the mapping reference: the TOML block holding the
/// `# SAMPLER pads 1-7` comment. Line endings may be CRLF (a Windows checkout).
fn reference_stems_example() -> &'static str {
    let doc = include_str!("../../../docs/controllers/mapping-reference.md");
    let comment = doc
        .find("# SAMPLER pads 1-7")
        .expect("the reference has the stems example");
    let fence = doc[..comment].rfind("```toml").unwrap();
    let start = fence + doc[fence..].find('\n').unwrap() + 1;
    let len = doc[start..].find("```").unwrap();
    assert!(start + len > comment, "the comment is in the block");
    &doc[start..start + len]
}

#[test]
fn stems_on_sampler_example_loads_and_routes() {
    use actions::{ControlState, Input};
    use controller::MidiMessage;
    use controller::mapping::Resolver;
    use engine::{HotcueSlot, Stem};

    let text = reference_stems_example();
    let mapping = parse_mapping("mapping.toml", text).unwrap();
    let parent = sb3();
    assert_eq!(mapping.inputs.len(), parent.inputs.len());

    // Saved where the reference says, it overrides the built-in it extends.
    let config = temp_folder("override");
    let folder = config.join("controllers").join("ddj-sb3");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("mapping.toml"), text).unwrap();
    let loaded = controller::builtin::load_mapping_from("ddj-sb3", Some(&config));
    std::fs::remove_dir_all(&config).unwrap();
    assert_eq!(loaded.unwrap(), mapping);

    let mut resolver = Resolver::new(&mapping);
    let mut state = ControlState::default();
    // A tap: the press's actions, after which the pad is released.
    let mut press = |state: &ControlState, channel: u8, note: u8| {
        let pressed = resolver.resolve(
            &MidiMessage::NoteOn {
                channel,
                note,
                velocity: 127,
            },
            state,
        );
        resolver.resolve(
            &MidiMessage::NoteOff {
                channel,
                note,
                velocity: 0,
            },
            state,
        );
        pressed
    };
    let stems = [
        Stem::Vocals,
        Stem::Instruments,
        Stem::Bass,
        Stem::Rhythm,
        Stem::Kicks,
        Stem::Melody,
        Stem::Hats,
    ];
    // SAMPLER pads 1-7 toggle the stems of the deck each side shows (channels 8 and 9).
    for (channel, deck) in [(7, d(0)), (8, d(1))] {
        for (i, stem) in stems.into_iter().enumerate() {
            assert_eq!(
                press(&state, channel, 0x30 + i as u8),
                vec![(Action::StemToggle(deck, stem), Input::Press)]
            );
        }
        // Pad 8 still plays sample 8.
        assert_eq!(
            press(&state, channel, 0x37),
            vec![(
                Action::SamplerPlay(HotcueSlot::new(8).unwrap()),
                Input::Press
            )]
        );
    }
    // The left side switched to deck 3: its stem pads follow.
    state.side_mut(Side::Left).active_deck = d(2);
    assert_eq!(
        press(&state, 7, 0x32),
        vec![(Action::StemToggle(d(2), Stem::Bass), Input::Press)]
    );
    // In another pad mode the pads do what the SB3 mapping says.
    state.side_mut(Side::Left).pad_mode = PadMode::BeatJump;
    let resolved = press(&state, 7, 0x30);
    assert!(
        resolved
            .iter()
            .all(|(a, _)| !matches!(a, Action::StemToggle(..))),
        "{resolved:?}"
    );
    assert!(!resolved.is_empty());
}
