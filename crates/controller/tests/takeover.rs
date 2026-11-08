//! The runtime reports where a control waits to pick a value up.

use actions::ActionKind;
use controller::mapping::parse_mapping;
use controller::{MidiMessage, Runtime, TimedMidi};
use engine::{DeckId, EngineConfig, engine};

const MAP: &str = r#"
[controller]
name = "Test"
port = "Test"

[[input]]
midi = { cc = 19, channel = 1 }
action = "channel.fader"
deck = 1
"#;

fn cc(value: u8) -> TimedMidi {
    TimedMidi {
        micros: 0,
        message: MidiMessage::ControlChange {
            channel: 0,
            controller: 19,
            value,
        },
    }
}

#[test]
fn pickup_reports_the_physical_value() {
    let (mut engine, _renderer) = engine(EngineConfig {
        sample_rate: 44_100,
        max_block_frames: 64,
    });
    let mut runtime = Runtime::new(parse_mapping("test.toml", MAP).unwrap());
    struct Null;
    impl controller::MidiOut for Null {
        fn send(&mut self, _: MidiMessage) {}
    }
    runtime.attach(&mut engine, &mut Null);
    let deck = DeckId::new(0).unwrap();
    let fader = (ActionKind::Fader, Some(deck));
    // The first attach takes the first value as is; the fader then changes elsewhere and
    // is armed, so the control must come back to it.
    runtime.handle_batch(&[cc(25)], &mut engine);
    assert_eq!(runtime.pickup(fader), None);
    runtime.control_state_mut().deck_mut(deck).fader = 0.8;
    runtime.handle_batch(&[cc(20)], &mut engine);
    assert_eq!(runtime.pickup(fader), Some(20.0 / 127.0));
    assert_eq!(runtime.state().deck(deck).fader, 0.8);
    runtime.handle_batch(&[cc(102)], &mut engine);
    assert_eq!(runtime.pickup(fader), None, "picked up");
}
