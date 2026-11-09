//! Replays the committed DDJ-SB3 capture (`controllers/ddj-sb3/capture.toml`) through the
//! built-in mapping and a real engine, so every SB3 binding is checked against what the
//! hardware actually sends.

use std::sync::Arc;

use actions::{Action, HardwarePadMode, PadMode, Side};
use controller::builtin::load_mapping_from;
use controller::capture::{Control, Fixture, parse_fixture};
use controller::mapping::Mapping;
use controller::{BatchStats, MidiMessage, MidiOut, Runtime, TimedMidi};
use engine::{
    Command, DeckCommand, DeckId, EngineConfig, EngineController, EngineRenderer, Grid, Hotcue,
    HotcueSlot, SamplerCommand, TrackBuffer, engine,
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
                .send(Command::Deck(
                    deck,
                    DeckCommand::Load {
                        track,
                        memory: engine::TrackMemory::default(),
                    },
                ))
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
            total.requests.extend(stats.requests);
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

    /// Sends `command` to `deck` and renders a block so it takes effect.
    fn deck_command(&mut self, deck: usize, command: DeckCommand) {
        self.engine.send(Command::Deck(d(deck), command)).unwrap();
        self.render();
    }

    /// Gives `deck` a constant grid of `bpm` with a beat at frame 0.
    fn set_grid(&mut self, deck: usize, bpm: f64) {
        let grid = Grid::from_bpm(bpm, 0.0, SAMPLE_RATE).unwrap();
        self.deck_command(deck, DeckCommand::SetGrid(Some(grid)));
    }

    /// Sends the captured message `index` of the pad control `id` (a `pads_hot_cue`
    /// control, one note per pad) with `velocity` (0 releases).
    fn pad(&mut self, id: &str, index: usize, velocity: u8) -> Vec<Action> {
        let range = self.control(id).messages[index];
        self.batch(&[note(range.status & 0x0F, range.data1, velocity)])
            .0
    }

    /// Sends the captured relative message `data1` of the jog control `id` as `steps`
    /// steps of +1 (65) or -1 (63), in one batch.
    fn jog(&mut self, id: &str, data1: u8, steps: usize, forward: bool) -> Vec<Action> {
        let range = *self
            .control(id)
            .messages
            .iter()
            .find(|m| m.status & 0xF0 == 0xB0 && m.data1 == data1)
            .unwrap_or_else(|| panic!("`{id}` has no controller {data1:#04X}"));
        let value = if forward { 65 } else { 63 };
        assert!(
            range.min <= value && value <= range.max,
            "`{id}` sends {value}"
        );
        let msg = cc(range.status & 0x0F, data1, value);
        self.batch(&vec![msg; steps]).0
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
        ("deck1.key_lock", Main, Action::KeylockToggle(d(1))),
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
        ("fx.left.1", Main, Action::FxToggle(d(1), 0)),
        ("fx.left.2", Main, Action::FxToggle(d(1), 1)),
        ("fx.left.3", Main, Action::FxToggle(d(1), 2)),
        ("fx.left.level", Main, Action::FxLevel(d(1))),
        // Right deck, deck 2.
        ("deck2.play", Main, Action::PlayPause(d(2))),
        ("deck2.cue", Main, Action::Cue(d(2))),
        ("deck2.cue.shift", Main, Action::ReturnToStart(d(2))),
        ("deck2.key_lock", Main, Action::KeylockToggle(d(2))),
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
        ("fx.right.1", Main, Action::FxToggle(d(2), 0)),
        ("fx.right.2", Main, Action::FxToggle(d(2), 1)),
        ("fx.right.3", Main, Action::FxToggle(d(2), 2)),
        ("fx.right.level", Main, Action::FxLevel(d(2))),
        // Left deck, deck 3 (software layer).
        ("deck3.play", Alt(l), Action::PlayPause(d(3))),
        ("deck3.cue", Alt(l), Action::Cue(d(3))),
        ("deck3.cue.shift", Alt(l), Action::ReturnToStart(d(3))),
        ("deck3.key_lock", Alt(l), Action::KeylockToggle(d(3))),
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
        ("deck4.key_lock", Alt(r), Action::KeylockToggle(d(4))),
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
        // Browser.
        ("browse.rotate", Main, Action::BrowserScroll),
        ("browse.press", Main, Action::BrowserToggleFocus),
        ("load.left", Main, Action::LoadSelected(d(1))),
        ("load.right", Main, Action::LoadSelected(d(2))),
        ("load.left", Alt(l), Action::LoadSelected(d(3))),
        ("load.right", Alt(r), Action::LoadSelected(d(4))),
    ]
    .into_iter()
    .chain(performance_table())
    .collect()
}

/// The single-action performance controls of every deck: SYNC, VINYL, the loop buttons
/// (SHIFT variants included), the jog touch and the outer ring. The multi-message jog tops
/// and the pads are covered by their own tests.
fn performance_table() -> Vec<(&'static str, Layer, Action)> {
    let l = Side::Left;
    let r = Side::Right;
    vec![
        // Deck 1.
        ("deck1.sync", Main, Action::Sync(d(1))),
        ("deck1.vinyl", Main, Action::VinylToggle(d(1))),
        ("deck1.auto_loop", Main, Action::AutoLoop(d(1))),
        ("deck1.loop_half", Main, Action::LoopHalve(d(1))),
        ("deck1.loop_double", Main, Action::LoopDouble(d(1))),
        ("deck1.loop_half.shift", Main, Action::LoopIn(d(1))),
        ("deck1.loop_double.shift", Main, Action::LoopOut(d(1))),
        ("deck1.auto_loop.shift", Main, Action::ReloopExit(d(1))),
        ("deck1.jog_touch", Main, Action::JogTouch(d(1))),
        ("deck1.jog_outer", Main, Action::JogRing(d(1))),
        ("deck1.vinyl.shift", Main, Action::SlipToggle(d(1))),
        (
            "deck1.mode_hot_cue",
            Main,
            Action::HardwarePadMode(l, HardwarePadMode::HotCue),
        ),
        (
            "deck1.mode_fx_fade",
            Main,
            Action::HardwarePadMode(l, HardwarePadMode::FxFade),
        ),
        (
            "deck1.mode_pad_scratch",
            Main,
            Action::PadMode(l, PadMode::PadScratch),
        ),
        (
            "deck1.mode_sampler",
            Main,
            Action::HardwarePadMode(l, HardwarePadMode::Sampler),
        ),
        (
            "deck1.mode_sampler.shift",
            Main,
            Action::HardwarePadMode(l, HardwarePadMode::Trans),
        ),
        (
            "deck1.mode_hot_cue.shift",
            Main,
            Action::PadMode(l, PadMode::BeatJump),
        ),
        (
            "deck1.mode_fx_fade.shift",
            Main,
            Action::PadMode(l, PadMode::LoopRoll),
        ),
        (
            "deck1.mode_pad_scratch.shift",
            Main,
            Action::PadMode(l, PadMode::Slicer),
        ),
        // Deck 2.
        ("deck2.sync", Main, Action::Sync(d(2))),
        ("deck2.vinyl", Main, Action::VinylToggle(d(2))),
        ("deck2.auto_loop", Main, Action::AutoLoop(d(2))),
        ("deck2.loop_half", Main, Action::LoopHalve(d(2))),
        ("deck2.loop_double", Main, Action::LoopDouble(d(2))),
        ("deck2.loop_half.shift", Main, Action::LoopIn(d(2))),
        ("deck2.loop_double.shift", Main, Action::LoopOut(d(2))),
        ("deck2.auto_loop.shift", Main, Action::ReloopExit(d(2))),
        ("deck2.jog_touch", Main, Action::JogTouch(d(2))),
        ("deck2.jog_outer", Main, Action::JogRing(d(2))),
        ("deck2.vinyl.shift", Main, Action::SlipToggle(d(2))),
        (
            "deck2.mode_hot_cue",
            Main,
            Action::HardwarePadMode(r, HardwarePadMode::HotCue),
        ),
        (
            "deck2.mode_fx_fade",
            Main,
            Action::HardwarePadMode(r, HardwarePadMode::FxFade),
        ),
        (
            "deck2.mode_pad_scratch",
            Main,
            Action::PadMode(r, PadMode::PadScratch),
        ),
        (
            "deck2.mode_sampler",
            Main,
            Action::HardwarePadMode(r, HardwarePadMode::Sampler),
        ),
        (
            "deck2.mode_sampler.shift",
            Main,
            Action::HardwarePadMode(r, HardwarePadMode::Trans),
        ),
        (
            "deck2.mode_hot_cue.shift",
            Main,
            Action::PadMode(r, PadMode::BeatJump),
        ),
        (
            "deck2.mode_fx_fade.shift",
            Main,
            Action::PadMode(r, PadMode::LoopRoll),
        ),
        (
            "deck2.mode_pad_scratch.shift",
            Main,
            Action::PadMode(r, PadMode::Slicer),
        ),
        // Deck 3.
        ("deck3.sync", Alt(l), Action::Sync(d(3))),
        ("deck3.vinyl", Alt(l), Action::VinylToggle(d(3))),
        ("deck3.auto_loop", Alt(l), Action::AutoLoop(d(3))),
        ("deck3.loop_half", Alt(l), Action::LoopHalve(d(3))),
        ("deck3.loop_double", Alt(l), Action::LoopDouble(d(3))),
        ("deck3.loop_half.shift", Alt(l), Action::LoopIn(d(3))),
        ("deck3.loop_double.shift", Alt(l), Action::LoopOut(d(3))),
        ("deck3.auto_loop.shift", Alt(l), Action::ReloopExit(d(3))),
        ("deck3.jog_touch", Alt(l), Action::JogTouch(d(3))),
        ("deck3.jog_outer", Alt(l), Action::JogRing(d(3))),
        ("deck3.vinyl.shift", Alt(l), Action::SlipToggle(d(3))),
        // Deck 4.
        ("deck4.sync", Alt(r), Action::Sync(d(4))),
        ("deck4.vinyl", Alt(r), Action::VinylToggle(d(4))),
        ("deck4.auto_loop", Alt(r), Action::AutoLoop(d(4))),
        ("deck4.loop_half", Alt(r), Action::LoopHalve(d(4))),
        ("deck4.loop_double", Alt(r), Action::LoopDouble(d(4))),
        ("deck4.loop_half.shift", Alt(r), Action::LoopIn(d(4))),
        ("deck4.loop_double.shift", Alt(r), Action::LoopOut(d(4))),
        ("deck4.auto_loop.shift", Alt(r), Action::ReloopExit(d(4))),
        ("deck4.jog_touch", Alt(r), Action::JogTouch(d(4))),
        ("deck4.jog_outer", Alt(r), Action::JogRing(d(4))),
        ("deck4.vinyl.shift", Alt(r), Action::SlipToggle(d(4))),
    ]
}

/// Controls that send several kinds of messages (jog tops also send a touch and firmware
/// ring noise, pads are one note each): their own tests check what they drive.
const MULTI_MESSAGE_CONTROLS: [&str; 28] = [
    "browse.press.shift",
    "deck1.pads_fx_fade",
    "deck1.pads_pad_scratch",
    "deck2.pads_fx_fade",
    "deck2.pads_pad_scratch",
    "deck1.jog_top",
    "deck1.jog_top.shift",
    "deck1.pads_hot_cue",
    "deck1.pads_hot_cue.shift",
    "deck2.jog_top",
    "deck2.jog_top.shift",
    "deck2.pads_hot_cue",
    "deck2.pads_hot_cue.shift",
    "deck1.pads_sampler",
    "deck2.pads_sampler",
    "pads.left.beat_jump",
    "pads.left.beat_jump.shift",
    "pads.left.loop_roll",
    "pads.left.loop_roll.shift",
    "pads.left.slicer",
    "pads.left.slicer.shift",
    "pads.right.beat_jump",
    "pads.right.beat_jump.shift",
    "pads.right.loop_roll",
    "pads.right.loop_roll.shift",
    "pads.right.slicer",
    "pads.right.slicer.shift",
    "pads.right.sampler.shift",
];

/// Mapped controls that do nothing on a fresh rig (no playing master, no grid, no loop):
/// their effect is checked by the tests of the loops and of SYNC.
const NEEDS_SETUP: [&str; 16] = [
    "deck1.sync",
    "deck2.sync",
    "deck3.sync",
    "deck4.sync",
    "deck1.auto_loop",
    "deck2.auto_loop",
    "deck3.auto_loop",
    "deck4.auto_loop",
    "deck1.loop_double.shift",
    "deck2.loop_double.shift",
    "deck3.loop_double.shift",
    "deck4.loop_double.shift",
    "deck1.auto_loop.shift",
    "deck2.auto_loop.shift",
    "deck3.auto_loop.shift",
    "deck4.auto_loop.shift",
];

/// The same pad note on the left side's pad channel (channel 8).
fn left_pad(msg: MidiMessage) -> MidiMessage {
    match msg {
        MidiMessage::NoteOn { note, velocity, .. } => MidiMessage::NoteOn {
            channel: 7,
            note,
            velocity,
        },
        MidiMessage::NoteOff { note, velocity, .. } => MidiMessage::NoteOff {
            channel: 7,
            note,
            velocity,
        },
        other => other,
    }
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
        if !NEEDS_SETUP.contains(&id) {
            assert!(stats.applied > 0, "`{id}`: no action took effect");
        }
        assert_eq!(stats.queue_full, 0, "`{id}`");
    }
}

#[test]
fn every_binding_is_in_the_table() {
    // The reverse check: each `[[input]]` of the mapping listens to a message some
    // control of the table sends, so no binding escapes the replay test.
    let fixture = fixture();
    let mut sent: Vec<MidiMessage> = table()
        .iter()
        .map(|(id, _, _)| *id)
        .chain(MULTI_MESSAGE_CONTROLS)
        .flat_map(|id| replay_messages(fixture.control(id).unwrap()))
        .collect();
    // The left SHIFT + SAMPLER pads were captured on the unshifted notes (see the mapping);
    // the right side's shifted notes stand in for them on channel 8.
    sent.extend(
        replay_messages(fixture.control("pads.right.sampler.shift").unwrap())
            .into_iter()
            .map(left_pad),
    );
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
fn faders_start_closed_and_first_move_is_taken() {
    // `[[init]]` closes every channel fader, so nothing is heard until a fader is touched,
    // and the first move of each fader is taken wherever the hardware fader sits.
    let mut rig = Rig::attached();
    for n in 1..=4 {
        assert_eq!(rig.runtime.state().deck(d(n)).fader, 0.0, "deck {n}");
    }
    let moved = rig.cc14("channel1.fader", 100, 0);
    assert!(moved.contains(&Action::Fader(d(1))), "{moved:?}");
    assert!((rig.runtime.state().deck(d(1)).fader - 100.0 / 127.0).abs() < 0.01);
}

#[test]
fn soft_takeover_after_deck_switch_on_fader() {
    let mut rig = Rig::attached();
    // Deck 1's fader takes the first physical value (the top) at once.
    rig.cc14("channel1.fader", 127, 127);
    assert_eq!(rig.runtime.state().deck(d(1)).fader, 1.0);

    // Deck 3's fader is still closed while the physical fader sits at the top.
    rig.press("deck1.deck_select");
    let moved = rig.cc14("channel3.fader", 40, 0);
    assert!(moved.is_empty(), "no jump to deck 3: {moved:?}");
    assert_eq!(rig.runtime.state().deck(d(3)).fader, 0.0);

    // Reaching deck 3's value picks it up.
    let moved = rig.cc14("channel3.fader", 0, 0);
    assert!(moved.contains(&Action::Fader(d(3))));
    rig.cc14("channel3.fader", 64, 0);
    assert!((rig.runtime.state().deck(d(3)).fader - 0.5).abs() < 0.01);

    // Back on deck 1 (at the top) with the physical fader at the middle: no jump either.
    rig.press("deck3.deck_select");
    let moved = rig.cc14("channel1.fader", 80, 0);
    assert!(moved.is_empty(), "no jump to deck 1: {moved:?}");
    assert_eq!(rig.runtime.state().deck(d(1)).fader, 1.0);
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

#[test]
fn browse_scrolls_and_load_requests_side_deck() {
    use actions::{AppRequest, BrowserRequest};
    let mut rig = Rig::attached();
    // The capture's BROWSE knob sent 1 turned clockwise (down the list) and 127 turned
    // counter-clockwise: its minimum and maximum.
    let rotate = rig.control("browse.rotate").messages[0];
    assert_eq!((rotate.status, rotate.data1), (0xB6, 0x40));
    let knob = |value| cc(rotate.status & 0x0F, rotate.data1, value);
    let (down, up) = (knob(rotate.min), knob(rotate.max));

    // A burst of clicks in one batch is one scroll of their sum.
    let (actions, stats) = rig.batch(&[down, down, down, up]);
    assert_eq!(actions, [Action::BrowserScroll; 4]);
    assert_eq!(
        stats.requests,
        [AppRequest::Browser(BrowserRequest::Scroll(2))]
    );
    let (_, stats) = rig.batch(&[up]);
    assert_eq!(
        stats.requests,
        [AppRequest::Browser(BrowserRequest::Scroll(-1))]
    );

    // Pushing the knob switches between the tree and the list.
    let stats = rig.replay("browse.press").1;
    assert_eq!(
        stats.requests,
        [AppRequest::Browser(BrowserRequest::ToggleFocus)]
    );

    // SHIFT + push opens or closes the highlighted folder.
    let stats = rig.replay("browse.press.shift").1;
    assert_eq!(
        stats.requests,
        [AppRequest::Browser(BrowserRequest::ToggleFolder)]
    );

    let load = |deck| AppRequest::LoadSelected { deck };
    assert_eq!(rig.replay("load.left").1.requests, [load(d(1))]);
    assert_eq!(rig.replay("load.right").1.requests, [load(d(2))]);
    // After the left DECK switch, LOAD left loads onto deck 3; the right side stays on 2.
    rig.press("deck1.deck_select");
    assert_eq!(rig.replay("load.left").1.requests, [load(d(3))]);
    assert_eq!(rig.replay("load.right").1.requests, [load(d(2))]);
    // Loading is the app's job: no deck changed here.
    assert_eq!(rig.runtime.state().side(Side::Left).active_deck, d(3));
}

fn slot(n: usize) -> HotcueSlot {
    HotcueSlot::new(n as u8).unwrap()
}

#[test]
fn pads_set_trigger_and_delete_hotcues_on_the_side_deck() {
    let mut rig = Rig::attached();

    // Left side, deck 1: a press on an empty pad sets it at the playhead.
    for (n, at) in [(1, 10_000.0), (2, 20_000.0)] {
        rig.deck_command(1, DeckCommand::Seek(at));
        let pressed = rig.pad("deck1.pads_hot_cue", n - 1, 127);
        assert_eq!(pressed, [Action::Hotcue(d(1), slot(n))]);
        let released = rig.pad("deck1.pads_hot_cue", n - 1, 0);
        assert_eq!(released, [Action::Hotcue(d(1), slot(n))]);
    }
    let snap = rig.engine.deck(d(1));
    assert_eq!(snap.hotcues[0], Some(Hotcue::point(10_000.0)));
    assert_eq!(snap.hotcues[1], Some(Hotcue::point(20_000.0)));
    assert!(!snap.previewing, "setting a pad does not preview");

    // Paused, a set pad jumps there and previews while held; the release stops.
    rig.deck_command(1, DeckCommand::Seek(0.0));
    rig.pad("deck1.pads_hot_cue", 0, 127);
    let snap = rig.engine.deck(d(1));
    assert!(snap.previewing && snap.playing, "held pad previews");
    assert!(snap.position_frames >= 10_000.0);
    rig.pad("deck1.pads_hot_cue", 0, 0);
    let snap = rig.engine.deck(d(1));
    assert!(!snap.previewing && !snap.playing);
    assert_eq!(snap.position_frames, 10_000.0, "back at the hotcue");

    // SHIFT + pad deletes (the SB3 sends notes 0x08-0x0F for it).
    let deleted = rig.pad("deck1.pads_hot_cue.shift", 1, 127);
    assert_eq!(deleted, [Action::HotcueDelete(d(1), slot(2))]);
    let snap = rig.engine.deck(d(1));
    assert_eq!(snap.hotcues[1], None);
    assert_eq!(
        snap.hotcues[0],
        Some(Hotcue::point(10_000.0)),
        "pad 1 stays"
    );

    // After DECK the left pads drive deck 3 and leave deck 1 alone.
    rig.press("deck1.deck_select");
    rig.deck_command(3, DeckCommand::Seek(30_000.0));
    let pressed = rig.pad("deck1.pads_hot_cue", 2, 127);
    assert_eq!(pressed, [Action::Hotcue(d(3), slot(3))]);
    rig.pad("deck1.pads_hot_cue", 2, 0);
    assert_eq!(
        rig.engine.deck(d(3)).hotcues[2],
        Some(Hotcue::point(30_000.0))
    );
    assert_eq!(rig.engine.deck(d(1)).hotcues[2], None);
    let deleted = rig.pad("deck1.pads_hot_cue.shift", 2, 127);
    assert_eq!(deleted, [Action::HotcueDelete(d(3), slot(3))]);
    assert_eq!(rig.engine.deck(d(3)).hotcues[2], None);
    assert_eq!(
        rig.engine.deck(d(1)).hotcues[0],
        Some(Hotcue::point(10_000.0))
    );

    // The right side's pads (channel 9) drive deck 2, all eight of them.
    for n in 1..=8 {
        let pressed = rig.pad("deck2.pads_hot_cue", n - 1, 127);
        assert_eq!(pressed, [Action::Hotcue(d(2), slot(n))]);
        rig.pad("deck2.pads_hot_cue", n - 1, 0);
        let deleted = rig.pad("deck2.pads_hot_cue.shift", n - 1, 127);
        assert_eq!(deleted, [Action::HotcueDelete(d(2), slot(n))]);
    }
    assert_eq!(rig.engine.deck(d(2)).hotcues, [None; 8]);
}

#[test]
fn held_jog_and_pad_across_deck_switch_release_on_the_pressed_deck() {
    let mut rig = Rig::attached();
    rig.deck_command(
        1,
        DeckCommand::SetHotcue(slot(1), Some(Hotcue::point(10_000.0))),
    );
    rig.deck_command(1, DeckCommand::Seek(10_000.0));
    let deck3_before = rig.engine.deck(d(3));

    // On deck 1 (paused, hotcue 1 set): touch the jog and hold pad 1, which previews.
    rig.button("deck1.jog_touch", 127);
    rig.pad("deck1.pads_hot_cue", 0, 127);
    let snap = rig.engine.deck(d(1));
    assert!(snap.jog_touched && snap.previewing);

    // DECK moves the side to deck 3; the releases still belong to deck 1.
    rig.press("deck1.deck_select");
    assert_eq!(rig.runtime.state().side(Side::Left).active_deck, d(3));
    assert_eq!(
        rig.button("deck1.jog_touch", 0),
        [Action::JogTouch(d(1))],
        "the release goes to the deck that was touched"
    );
    assert_eq!(
        rig.pad("deck1.pads_hot_cue", 0, 0),
        [Action::Hotcue(d(1), slot(1))]
    );

    let snap = rig.engine.deck(d(1));
    assert!(!snap.jog_touched, "deck 1's jog is released");
    assert!(!snap.previewing && !snap.playing, "no preview left");
    assert_eq!(snap.position_frames, 10_000.0, "paused back at the hotcue");
    assert_eq!(rig.engine.deck(d(3)), deck3_before, "deck 3 is unchanged");
}

#[test]
fn jog_ring_nudges_and_top_scratches_when_touched() {
    let mut rig = Rig::attached();

    // Paused: the ring moves the playhead both ways.
    let at = rig.engine.deck(d(1)).position_frames;
    let actions = rig.jog("deck1.jog_outer", 0x21, 20, true);
    assert_eq!(actions, [Action::JogRing(d(1)); 20]);
    let forward = rig.engine.deck(d(1)).position_frames;
    assert!(forward > at, "ring forward: {at} -> {forward}");
    rig.jog("deck1.jog_outer", 0x21, 20, false);
    assert!(rig.engine.deck(d(1)).position_frames < forward);

    // Playing, the ring only bends the speed: the track nudges and keeps playing.
    rig.press("deck1.play");
    let actions = rig.jog("deck1.jog_outer", 0x21, 5, true);
    assert_eq!(actions, [Action::JogRing(d(1)); 5]);
    assert!(rig.engine.deck(d(1)).playing);

    // The top without a touch does the same as the ring.
    let actions = rig.jog("deck1.jog_top", 0x23, 5, true);
    assert_eq!(actions, [Action::JogTop(d(1)); 5]);

    // VINYL on and the top touched: scratching holds the record.
    assert!(rig.engine.deck(d(1)).vinyl, "vinyl mode starts on");
    rig.press("deck1.vinyl");
    assert!(!rig.engine.deck(d(1)).vinyl, "VINYL turns it off");
    rig.press("deck1.vinyl");
    assert!(rig.engine.deck(d(1)).vinyl, "VINYL turns vinyl mode on");
    assert_eq!(rig.button("deck1.jog_touch", 127), [Action::JogTouch(d(1))]);
    assert!(rig.engine.deck(d(1)).jog_touched);
    let before = rig.engine.deck(d(1)).position_frames;
    rig.jog("deck1.jog_top", 0x23, 40, true);
    for _ in 0..8 {
        rig.render();
    }
    let forward = rig.engine.deck(d(1)).position_frames;
    assert!(forward > before, "scratch forward: {before} -> {forward}");
    rig.jog("deck1.jog_top", 0x23, 80, false);
    for _ in 0..8 {
        rig.render();
    }
    let back = rig.engine.deck(d(1)).position_frames;
    assert!(back < forward, "scratch back: {forward} -> {back}");

    // Releasing the top resumes playback.
    assert_eq!(rig.button("deck1.jog_touch", 0), [Action::JogTouch(d(1))]);
    let snap = rig.engine.deck(d(1));
    assert!(!snap.jog_touched && snap.playing);
    rig.render();
    assert!(rig.engine.deck(d(1)).position_frames > back);

    // Deck 3 after DECK, and the right wheel for deck 2, use their own wheels.
    rig.press("deck1.deck_select");
    let actions = rig.jog("deck3.jog_outer", 0x21, 2, true);
    assert_eq!(actions, [Action::JogRing(d(3)); 2]);
    let actions = rig.jog("deck2.jog_outer", 0x21, 2, true);
    assert_eq!(actions, [Action::JogRing(d(2)); 2]);
}

#[test]
fn loop_buttons_drive_the_loop() {
    let mut rig = Rig::attached();
    // 120 BPM: a beat is 22,050 frames. Start playing a little after the second beat.
    rig.set_grid(1, 120.0);
    rig.deck_command(1, DeckCommand::Seek(22_050.0 + 1_000.0));
    rig.press("deck1.play");

    // AUTO LOOP loops 4 beats from the beat before the playhead.
    assert_eq!(rig.press("deck1.auto_loop"), [Action::AutoLoop(d(1)); 2]);
    let snap = rig.engine.deck(d(1));
    assert!(snap.loop_active);
    assert_eq!(snap.loop_in, Some(22_050.0));
    assert_eq!(snap.loop_out, Some(22_050.0 + 4.0 * 22_050.0));

    // 1/2X halves it, 2X doubles it back.
    rig.press("deck1.loop_half");
    let snap = rig.engine.deck(d(1));
    assert_eq!(snap.loop_out, Some(22_050.0 + 2.0 * 22_050.0));
    assert_eq!(snap.loop_beats, 2.0);
    rig.press("deck1.loop_double");
    assert_eq!(rig.engine.deck(d(1)).loop_beats, 4.0);

    // AUTO LOOP again exits.
    rig.press("deck1.auto_loop");
    let snap = rig.engine.deck(d(1));
    assert!(!snap.loop_active && snap.playing);

    // Manual loop: SHIFT+1/2X sets the loop in, SHIFT+2X the loop out.
    let loop_in = rig.engine.deck(d(1)).position_frames;
    rig.press("deck1.loop_half.shift");
    let snap = rig.engine.deck(d(1));
    assert!(snap.loop_in.is_some() && !snap.loop_active);
    assert!(snap.loop_in.unwrap() >= loop_in - 1.0);
    for _ in 0..20 {
        rig.render();
    }
    rig.press("deck1.loop_double.shift");
    let snap = rig.engine.deck(d(1));
    assert!(snap.loop_active, "SHIFT+2X closes and starts the loop");
    assert!(snap.loop_out.unwrap() > snap.loop_in.unwrap());

    // SHIFT+AUTO LOOP exits, and once more jumps back in and loops.
    rig.press("deck1.auto_loop.shift");
    assert!(!rig.engine.deck(d(1)).loop_active);
    rig.press("deck1.auto_loop.shift");
    let snap = rig.engine.deck(d(1));
    assert!(snap.loop_active);
    assert!(snap.position_frames >= snap.loop_in.unwrap());

    // The right deck's loop buttons (channel 2) drive deck 2.
    assert_eq!(rig.press("deck2.auto_loop"), [Action::AutoLoop(d(2)); 2]);
}

#[test]
fn sync_button_syncs_to_the_other_side() {
    let mut rig = Rig::attached();
    // Deck 1 (left) plays at 120 BPM; deck 2 (right) is a 128 BPM track, paused.
    rig.set_grid(1, 120.0);
    rig.set_grid(2, 128.0);
    rig.press("deck1.play");
    for _ in 0..4 {
        rig.render();
    }
    rig.deck_command(2, DeckCommand::Seek(7_777.0));

    // SYNC on the paused right deck matches the tempo to the playing left deck.
    assert_eq!(rig.press("deck2.sync"), [Action::Sync(d(2)); 2]);
    let ratio = rig.engine.deck(d(2)).tempo_ratio;
    assert!((ratio - 120.0 / 128.0).abs() < 0.001, "tempo ratio {ratio}");
    assert_eq!(
        rig.engine.deck(d(1)).tempo_ratio,
        1.0,
        "the master is untouched"
    );

    // PLAY then starts it in phase with the master.
    rig.press("deck2.play");
    for _ in 0..2 {
        rig.render();
    }
    let phase = |rig: &Rig, n: usize| {
        let snap = rig.engine.deck(d(n));
        let beats = snap.grid.unwrap().beats_at(snap.position_frames);
        beats - beats.floor()
    };
    let (master, synced) = (phase(&rig, 1), phase(&rig, 2));
    let distance = (master - synced).abs().min(1.0 - (master - synced).abs());
    assert!(distance < 0.02, "beat phases {master} vs {synced}");

    // The same button on the right side's second deck after DECK drives deck 4.
    rig.press("deck2.deck_select");
    assert_eq!(rig.press("deck4.sync"), [Action::Sync(d(4)); 2]);
}

#[test]
fn shift_jog_searches() {
    let mut rig = Rig::attached();
    let at = 100_000.0;
    rig.deck_command(1, DeckCommand::Seek(at));

    // SHIFT + jog top searches 10 s of track per turn: 72 of 720 steps is 1 s.
    let actions = rig.jog("deck1.jog_top.shift", 0x1F, 72, true);
    assert_eq!(actions, [Action::JogSearch(d(1)); 72]);
    let moved = rig.engine.deck(d(1)).position_frames - at;
    assert!(
        (moved - f64::from(SAMPLE_RATE)).abs() < 1_000.0,
        "searched {moved} frames"
    );
    rig.jog("deck1.jog_top.shift", 0x1F, 72, false);
    let back = rig.engine.deck(d(1)).position_frames;
    assert!((back - at).abs() < 1_000.0, "back at {back}");

    // The touch SHIFT turns into note 0x67 still ends the jog touch, so a touch that ends
    // with SHIFT held pairs with the one that started without it.
    rig.button("deck1.jog_touch", 127);
    assert!(rig.engine.deck(d(1)).jog_touched);
    let range = rig.control("deck1.jog_top.shift").messages[0];
    assert_eq!(range.data1, 0x67);
    let (actions, _) = rig.batch(&[note(range.status & 0x0F, range.data1, 0)]);
    assert_eq!(actions, [Action::JogTouch(d(1))]);
    assert!(!rig.engine.deck(d(1)).jog_touched);

    // Searching also works while playing.
    rig.press("deck1.play");
    let before = rig.engine.deck(d(1)).position_frames;
    rig.jog("deck1.jog_top.shift", 0x1F, 72, true);
    assert!(rig.engine.deck(d(1)).position_frames > before + 40_000.0);
}

#[test]
fn shift_touch_search_keeps_a_playing_deck_playing() {
    let mut rig = Rig::attached();
    rig.press("deck1.play");
    assert!(
        rig.engine.deck(d(1)).vinyl,
        "vinyl mode is on, so a touch would scratch"
    );
    let shift_touch = rig.control("deck1.jog_top.shift").messages[0];
    let (channel, touch) = (shift_touch.status & 0x0F, shift_touch.data1);

    // SHIFT held, then the hand on the top (note 0x67): no scratch hold starts.
    rig.button("deck1.shift", 127);
    let (actions, _) = rig.batch(&[note(channel, touch, 127)]);
    assert_eq!(actions, [Action::JogRelease(d(1))]);
    let snap = rig.engine.deck(d(1));
    assert!(!snap.jog_touched && snap.playing);

    // Search turns move the playhead, and the deck keeps playing between them.
    let before = rig.engine.deck(d(1)).position_frames;
    let actions = rig.jog("deck1.jog_top.shift", 0x1F, 72, true);
    assert_eq!(actions, [Action::JogSearch(d(1)); 72]);
    let searched = rig.engine.deck(d(1)).position_frames;
    assert!(
        searched > before + 40_000.0,
        "searched {before} -> {searched}"
    );
    for _ in 0..4 {
        rig.render();
    }
    let after = rig.engine.deck(d(1)).position_frames;
    assert!(
        after >= searched + 4.0 * BLOCK as f64 - 1.0,
        "still playing: {searched} -> {after}"
    );

    // Letting go (still SHIFT) and releasing SHIFT leave it playing.
    let (actions, _) = rig.batch(&[note(channel, touch, 0)]);
    assert_eq!(actions, [Action::JogRelease(d(1))]);
    rig.button("deck1.shift", 0);
    let snap = rig.engine.deck(d(1));
    assert!(!snap.jog_touched && snap.playing);
}

#[test]
fn shift_during_a_held_pad_or_cue_still_ends_the_preview() {
    let mut rig = Rig::attached();
    rig.deck_command(
        1,
        DeckCommand::SetHotcue(slot(1), Some(Hotcue::point(10_000.0))),
    );

    // Paused, pad 1 held previews; SHIFT goes down and the note-off arrives on the shifted
    // note (0x08).
    rig.pad("deck1.pads_hot_cue", 0, 127);
    assert!(rig.engine.deck(d(1)).previewing);
    rig.button("deck1.shift", 127);
    let shifted = rig.control("deck1.pads_hot_cue.shift").messages[0];
    assert_eq!(shifted.data1, 0x08);
    let released = rig.pad("deck1.pads_hot_cue.shift", 0, 0);
    assert_eq!(released, [Action::Hotcue(d(1), slot(1))]);
    let snap = rig.engine.deck(d(1));
    assert!(!snap.previewing && !snap.playing, "preview ended");
    assert_eq!(snap.position_frames, 10_000.0, "back at the hotcue");
    assert_eq!(
        snap.hotcues[0],
        Some(Hotcue::point(10_000.0)),
        "the release deletes nothing"
    );
    rig.button("deck1.shift", 0);

    // The same for CUE (0x0C), whose note with SHIFT held is 0x48.
    rig.button("deck1.cue", 127);
    rig.button("deck1.cue", 0);
    rig.button("deck1.cue", 127);
    assert!(rig.engine.deck(d(1)).previewing, "held CUE previews");
    rig.button("deck1.shift", 127);
    let released = rig.button("deck1.cue.shift", 0);
    assert_eq!(released, [Action::Cue(d(1))]);
    let snap = rig.engine.deck(d(1));
    assert!(!snap.previewing && !snap.playing, "CUE preview ended");
    assert_eq!(snap.position_frames, 10_000.0, "back at the cue point");
    rig.button("deck1.shift", 0);
}

#[test]
fn touch_ended_with_shift_then_deck_switch_releases_the_new_deck() {
    let mut rig = Rig::attached();
    // Deck 3 plays in vinyl mode, so a touch left on would hold it silent.
    rig.deck_command(3, DeckCommand::TogglePlay);
    assert!(rig.engine.deck(d(3)).playing && rig.engine.deck(d(3)).vinyl);
    let shift_touch = rig.control("deck1.jog_top.shift").messages[0];
    assert_eq!(shift_touch.data1, 0x67);

    // On deck 1: touch, SHIFT down, lift (the note-off arrives on 0x67), SHIFT up.
    rig.button("deck1.jog_touch", 127);
    rig.button("deck1.shift", 127);
    rig.batch(&[note(shift_touch.status & 0x0F, shift_touch.data1, 0)]);
    rig.button("deck1.shift", 0);
    assert!(!rig.engine.deck(d(1)).jog_touched);
    let deck1_before = rig.engine.deck(d(1));

    // DECK: the left side shows deck 3; touch and lift.
    rig.press("deck1.deck_select");
    assert_eq!(rig.runtime.state().side(Side::Left).active_deck, d(3));
    rig.button("deck3.jog_touch", 127);
    assert!(rig.engine.deck(d(3)).jog_touched);
    assert_eq!(
        rig.button("deck3.jog_touch", 0),
        [Action::JogTouch(d(3))],
        "the lift ends the touch it pairs with"
    );

    let snap = rig.engine.deck(d(3));
    assert!(!snap.jog_touched, "deck 3's jog is released");
    let at = snap.position_frames;
    rig.render();
    assert!(
        rig.engine.deck(d(3)).position_frames > at,
        "deck 3 plays on"
    );
    assert_eq!(rig.engine.deck(d(1)), deck1_before, "deck 1 is unchanged");
}

#[test]
fn pad_or_cue_ended_with_shift_then_deck_switch_previews_the_new_deck() {
    let mut rig = Rig::attached();
    rig.deck_command(
        1,
        DeckCommand::SetHotcue(slot(1), Some(Hotcue::point(10_000.0))),
    );
    rig.deck_command(
        3,
        DeckCommand::SetHotcue(slot(1), Some(Hotcue::point(20_000.0))),
    );

    // On deck 1: hold pad 1 (previews), SHIFT down, lift on the shifted note, SHIFT up;
    // the same for CUE.
    rig.pad("deck1.pads_hot_cue", 0, 127);
    rig.button("deck1.shift", 127);
    rig.pad("deck1.pads_hot_cue.shift", 0, 0);
    rig.button("deck1.shift", 0);
    rig.button("deck1.cue", 127);
    rig.button("deck1.shift", 127);
    rig.button("deck1.cue.shift", 0);
    rig.button("deck1.shift", 0);
    assert!(!rig.engine.deck(d(1)).previewing);
    let deck1_before = rig.engine.deck(d(1));

    // DECK: deck 3 (paused, hotcue 1 set). Holding pad 1 previews; the lift ends it.
    rig.press("deck1.deck_select");
    assert_eq!(rig.runtime.state().side(Side::Left).active_deck, d(3));
    rig.pad("deck3.pads_hot_cue", 0, 127);
    assert!(rig.engine.deck(d(3)).previewing, "pad 1 previews deck 3");
    assert_eq!(
        rig.pad("deck3.pads_hot_cue", 0, 0),
        [Action::Hotcue(d(3), slot(1))]
    );
    let snap = rig.engine.deck(d(3));
    assert!(!snap.previewing && !snap.playing, "pad preview ended");
    assert_eq!(snap.position_frames, 20_000.0, "back at the hotcue");

    // CUE on deck 3: set the cue here (paused), then hold to preview and lift.
    rig.button("deck3.cue", 127);
    rig.button("deck3.cue", 0);
    rig.button("deck3.cue", 127);
    assert!(rig.engine.deck(d(3)).previewing, "held CUE previews deck 3");
    assert_eq!(rig.button("deck3.cue", 0), [Action::Cue(d(3))]);
    let snap = rig.engine.deck(d(3));
    assert!(!snap.previewing && !snap.playing, "CUE preview ended");
    assert_eq!(snap.position_frames, 20_000.0, "back at the cue point");

    assert_eq!(rig.engine.deck(d(1)), deck1_before, "deck 1 is unchanged");
}

#[test]
fn touch_held_across_deck_switch_then_shift_lift_releases_the_held_deck() {
    let mut rig = Rig::attached();
    // Deck 1 plays in vinyl mode, so a touch left on holds it silent.
    rig.press("deck1.play");
    assert!(rig.engine.deck(d(1)).playing && rig.engine.deck(d(1)).vinyl);
    let deck3_before = rig.engine.deck(d(3));

    // Touch deck 1, DECK (the left side shows deck 3), SHIFT down, lift: the note-off
    // arrives on the shifted note 0x67.
    rig.button("deck1.jog_touch", 127);
    assert!(rig.engine.deck(d(1)).jog_touched);
    rig.press("deck1.deck_select");
    assert_eq!(rig.runtime.state().side(Side::Left).active_deck, d(3));
    rig.button("deck3.shift", 127);
    let shift_touch = rig.control("deck3.jog_top.shift").messages[0];
    assert_eq!(shift_touch.data1, 0x67);
    let (actions, _) = rig.batch(&[note(shift_touch.status & 0x0F, shift_touch.data1, 0)]);
    assert_eq!(
        actions,
        [Action::JogTouch(d(1))],
        "the lift ends deck 1's touch"
    );
    rig.button("deck3.shift", 0);

    let snap = rig.engine.deck(d(1));
    assert!(!snap.jog_touched, "deck 1's jog is released");
    let at = snap.position_frames;
    rig.render();
    assert!(
        rig.engine.deck(d(1)).position_frames > at,
        "deck 1 plays on"
    );
    assert_eq!(rig.engine.deck(d(3)), deck3_before, "deck 3 is unchanged");
}

#[test]
fn pad_held_across_deck_switch_then_shift_lift_ends_the_held_preview() {
    let mut rig = Rig::attached();
    rig.deck_command(
        1,
        DeckCommand::SetHotcue(slot(1), Some(Hotcue::point(10_000.0))),
    );
    rig.deck_command(
        3,
        DeckCommand::SetHotcue(slot(1), Some(Hotcue::point(20_000.0))),
    );
    let deck3_before = rig.engine.deck(d(3));

    // Paused deck 1: hold pad 1 (previews), DECK, SHIFT down, lift on 0x08.
    rig.pad("deck1.pads_hot_cue", 0, 127);
    assert!(rig.engine.deck(d(1)).previewing);
    rig.press("deck1.deck_select");
    assert_eq!(rig.runtime.state().side(Side::Left).active_deck, d(3));
    rig.button("deck3.shift", 127);
    let shifted = rig.control("deck3.pads_hot_cue.shift").messages[0];
    assert_eq!(shifted.data1, 0x08);
    let released = rig.pad("deck3.pads_hot_cue.shift", 0, 0);
    assert_eq!(released, [Action::Hotcue(d(1), slot(1))]);
    rig.button("deck3.shift", 0);

    let snap = rig.engine.deck(d(1));
    assert!(!snap.previewing && !snap.playing, "deck 1's preview ended");
    assert_eq!(snap.position_frames, 10_000.0, "back at the hotcue");
    assert_eq!(
        snap.hotcues[0],
        Some(Hotcue::point(10_000.0)),
        "the release deletes nothing"
    );
    assert_eq!(rig.engine.deck(d(3)), deck3_before, "deck 3 is unchanged");
}

#[test]
fn cue_held_across_deck_switch_then_shift_lift_ends_the_held_preview() {
    let mut rig = Rig::attached();
    rig.deck_command(1, DeckCommand::Seek(10_000.0));
    let deck3_before = rig.engine.deck(d(3));

    // Paused deck 1: set the cue, hold CUE (previews), DECK, SHIFT down, lift on 0x48.
    rig.button("deck1.cue", 127);
    rig.button("deck1.cue", 0);
    rig.button("deck1.cue", 127);
    assert!(rig.engine.deck(d(1)).previewing, "held CUE previews");
    rig.press("deck1.deck_select");
    assert_eq!(rig.runtime.state().side(Side::Left).active_deck, d(3));
    rig.button("deck3.shift", 127);
    let shifted = rig.control("deck3.cue.shift").messages[0];
    assert_eq!(shifted.data1, 0x48);
    assert_eq!(rig.button("deck3.cue.shift", 0), [Action::Cue(d(1))]);
    rig.button("deck3.shift", 0);

    let snap = rig.engine.deck(d(1));
    assert!(
        !snap.previewing && !snap.playing,
        "deck 1's CUE preview ended"
    );
    assert_eq!(snap.position_frames, 10_000.0, "back at the cue point");
    assert_eq!(rig.engine.deck(d(3)), deck3_before, "deck 3 is unchanged");
}

/// Sets the mode of `side` with its captured SHIFT mode buttons (`n` is deck 1 or 2).
fn set_mode(rig: &mut Rig, side: Side, mode: PadMode) {
    let n = if side == Side::Left { 1 } else { 2 };
    let id = match mode {
        PadMode::Hardware => format!("deck{n}.mode_hot_cue"),
        PadMode::BeatJump => format!("deck{n}.mode_hot_cue.shift"),
        PadMode::LoopRoll => format!("deck{n}.mode_fx_fade.shift"),
        PadMode::Slicer => format!("deck{n}.mode_pad_scratch.shift"),
        PadMode::PadScratch => format!("deck{n}.mode_pad_scratch"),
    };
    rig.press(&id);
    assert_eq!(rig.runtime.state().side(side).pad_mode, mode, "{id}");
}

/// Starts `deck` playing from `at` with a 120 BPM grid (a beat is 22 050 frames).
fn play_from(rig: &mut Rig, deck: usize, at: f64) {
    rig.set_grid(deck, 120.0);
    rig.deck_command(deck, DeckCommand::Seek(at));
    rig.deck_command(deck, DeckCommand::Play);
    assert!(rig.engine.deck(d(deck)).playing);
}

const BEAT: f64 = 22_050.0;

fn hardware(side: Side, mode: HardwarePadMode) -> Action {
    Action::HardwarePadMode(side, mode)
}

#[test]
fn mode_buttons_set_the_side_pad_mode() {
    let mut rig = Rig::attached();
    for (side, n) in [(Side::Left, 1), (Side::Right, 2)] {
        let other = if side == Side::Left {
            Side::Right
        } else {
            Side::Left
        };
        for (id, action) in [
            (
                "mode_hot_cue.shift",
                Action::PadMode(side, PadMode::BeatJump),
            ),
            ("mode_hot_cue", hardware(side, HardwarePadMode::HotCue)),
            (
                "mode_fx_fade.shift",
                Action::PadMode(side, PadMode::LoopRoll),
            ),
            ("mode_fx_fade", hardware(side, HardwarePadMode::FxFade)),
            (
                "mode_pad_scratch.shift",
                Action::PadMode(side, PadMode::Slicer),
            ),
            (
                "mode_pad_scratch",
                Action::PadMode(side, PadMode::PadScratch),
            ),
            (
                "mode_hot_cue.shift",
                Action::PadMode(side, PadMode::BeatJump),
            ),
            ("mode_sampler", hardware(side, HardwarePadMode::Sampler)),
            (
                "mode_pad_scratch.shift",
                Action::PadMode(side, PadMode::Slicer),
            ),
            ("mode_sampler.shift", hardware(side, HardwarePadMode::Trans)),
        ] {
            let mode = match action {
                Action::PadMode(_, mode) => mode,
                _ => PadMode::Hardware,
            };
            let actions = rig.press(&format!("deck{n}.{id}"));
            assert_eq!(actions, [action, action]);
            assert_eq!(
                rig.runtime.state().side(side).pad_mode,
                mode,
                "deck{n}.{id}"
            );
            assert_eq!(
                rig.runtime.state().side(other).pad_mode,
                PadMode::Hardware,
                "the other side keeps its mode"
            );
        }
    }
}

#[test]
fn beat_jump_pads_jump_by_the_configured_beats() {
    let mut rig = Rig::attached();
    rig.set_grid(1, 120.0);
    rig.set_grid(2, 120.0);
    rig.deck_command(1, DeckCommand::Seek(200_000.0));

    // In the hardware mode the same note sets a hotcue instead.
    rig.pad("pads.left.beat_jump", 1, 127);
    rig.pad("pads.left.beat_jump", 1, 0);
    assert_eq!(rig.engine.deck(d(1)).position_frames, 200_000.0);
    assert!(rig.engine.deck(d(1)).hotcues[1].is_some());

    set_mode(&mut rig, Side::Left, PadMode::BeatJump);
    // Pad 2 is +4 beats, pad 1 -4 beats, SHIFT + pad 1 -1 beat, SHIFT + pad 6 +4 beats.
    for (id, index, beats) in [
        ("pads.left.beat_jump", 1, 4.0),
        ("pads.left.beat_jump", 0, -4.0),
        ("pads.left.beat_jump.shift", 0, -1.0),
        ("pads.left.beat_jump.shift", 5, 4.0),
    ] {
        let before = rig.engine.deck(d(1)).position_frames;
        let pressed = rig.pad(id, index, 127);
        let expected = if id.ends_with("shift") {
            Action::BeatJumpShift(d(1), slot(index + 1))
        } else {
            Action::BeatJump(d(1), slot(index + 1))
        };
        assert_eq!(pressed, [expected], "{id} {index}");
        rig.pad(id, index, 0);
        let after = rig.engine.deck(d(1)).position_frames;
        assert_eq!(after - before, beats * BEAT, "{id} {index}");
    }
    assert_eq!(
        rig.engine.deck(d(2)).position_frames,
        0.0,
        "deck 2 is untouched"
    );

    // The right side has its own mode and drives deck 2.
    set_mode(&mut rig, Side::Right, PadMode::BeatJump);
    rig.pad("pads.right.beat_jump", 3, 127);
    assert_eq!(rig.engine.deck(d(2)).position_frames, 8.0 * BEAT);
}

#[test]
fn shift_mode_reads_the_sampler_bank_too() {
    // The SB3 keeps sending the notes of its last hardware mode, so with the pads in the
    // SAMPLER bank (0x30-0x37, SHIFT 0x38-0x3F) a software mode still reads them: they
    // act as that mode, not as samples. The right side's captures are on that bank.
    let mut rig = Rig::attached();
    for i in 0..8 {
        let buffer = Arc::new(TrackBuffer::new(SAMPLE_RATE, vec![0.1; 441_000 * 2]).unwrap());
        rig.engine
            .send(Command::Sampler(SamplerCommand::Set(i, Some(buffer))))
            .unwrap();
    }
    rig.set_grid(1, 120.0);
    rig.set_grid(2, 120.0);
    rig.deck_command(1, DeckCommand::Seek(200_000.0));
    rig.deck_command(2, DeckCommand::Seek(200_000.0));

    // Hardware mode: the sampler bank plays samples.
    assert_eq!(
        rig.pad("deck1.pads_sampler", 1, 127),
        [Action::SamplerPlay(slot(2))]
    );
    assert!(rig.engine.sampler().playing[1]);
    rig.pad("deck1.pads_sampler", 1, 0);

    // Beat jump: pad 2 jumps +4 beats, SHIFT + pad 1 -1 beat; no sample plays.
    set_mode(&mut rig, Side::Left, PadMode::BeatJump);
    rig.batch(&[note(7, 0x39, 127), note(7, 0x39, 0)]);
    assert_eq!(rig.engine.deck(d(1)).position_frames, 200_000.0 + BEAT);
    assert_eq!(
        rig.pad("deck1.pads_sampler", 1, 127),
        [Action::BeatJump(d(1), slot(2))]
    );
    assert_eq!(
        rig.engine.deck(d(1)).position_frames,
        200_000.0 + 5.0 * BEAT
    );
    let shifted = rig.batch(&[note(7, 0x38, 127)]).0;
    assert_eq!(shifted, [Action::BeatJumpShift(d(1), slot(1))]);
    assert_eq!(
        rig.engine.deck(d(1)).position_frames,
        200_000.0 + 4.0 * BEAT
    );
    assert!(!rig.engine.sampler().playing[0], "no sample plays");

    // The right side's captured sampler-bank pads, in each software mode.
    set_mode(&mut rig, Side::Right, PadMode::BeatJump);
    assert_eq!(
        rig.pad("pads.right.beat_jump", 3, 127),
        [Action::BeatJump(d(2), slot(4))]
    );
    assert_eq!(
        rig.engine.deck(d(2)).position_frames,
        200_000.0 + 8.0 * BEAT
    );
    assert_eq!(
        rig.pad("pads.right.beat_jump.shift", 7, 127),
        [Action::BeatJumpShift(d(2), slot(8))]
    );
    play_from(&mut rig, 2, 100_000.0);
    set_mode(&mut rig, Side::Right, PadMode::LoopRoll);
    assert_eq!(
        rig.pad("pads.right.loop_roll", 4, 127),
        [Action::LoopRoll(d(2), slot(5))]
    );
    assert!(rig.engine.deck(d(2)).roll_active);
    assert_eq!(
        rig.pad("pads.right.loop_roll.shift", 4, 0),
        [Action::LoopRoll(d(2), slot(5))]
    );
    assert!(!rig.engine.deck(d(2)).roll_active);
    set_mode(&mut rig, Side::Right, PadMode::Slicer);
    assert_eq!(
        rig.pad("pads.right.slicer", 2, 127),
        [Action::Slice(d(2), slot(3))]
    );
    assert!(rig.engine.deck(d(2)).roll_active);
    rig.pad("pads.right.slicer", 2, 0);
    assert!(!rig.engine.deck(d(2)).roll_active);

    // SHIFT + SAMPLER (TRANS) is a hardware mode: the bank plays samples again.
    rig.press("deck1.mode_sampler.shift");
    assert_eq!(
        rig.pad("deck1.pads_sampler", 0, 127),
        [Action::SamplerPlay(slot(1))]
    );
    assert!(rig.engine.sampler().playing[0]);
}

#[test]
fn roll_pads_roll_and_slip_back() {
    let mut rig = Rig::attached();
    play_from(&mut rig, 1, 100_000.0);
    set_mode(&mut rig, Side::Left, PadMode::LoopRoll);

    // Pad 5 rolls one beat; the playhead stays inside it while the ghost moves on.
    let pressed = rig.pad("pads.left.loop_roll", 4, 127);
    assert_eq!(pressed, [Action::LoopRoll(d(1), slot(5))]);
    for _ in 0..200 {
        rig.render();
    }
    let rolling = rig.engine.deck(d(1));
    assert!(rolling.roll_active, "the roll is held");
    assert!(rolling.position_frames < 100_000.0 + 2.0 * BEAT, "looping");
    let ghost = rolling.ghost_frames.expect("the ghost keeps moving");
    assert!(ghost > 100_000.0 + 4.0 * BEAT);

    let released = rig.pad("pads.left.loop_roll", 4, 0);
    assert_eq!(released, [Action::LoopRoll(d(1), slot(5))]);
    let after = rig.engine.deck(d(1));
    assert!(!after.roll_active);
    assert!(
        after.position_frames >= ghost,
        "back where the track would be: {} vs {ghost}",
        after.position_frames
    );

    // The shifted bank rolls too, and a release on the other bank's note still pairs.
    rig.pad("pads.left.loop_roll.shift", 6, 127);
    assert!(rig.engine.deck(d(1)).roll_active);
    let released = rig.pad("pads.left.loop_roll", 6, 0);
    assert_eq!(released, [Action::LoopRoll(d(1), slot(7))]);
    assert!(!rig.engine.deck(d(1)).roll_active);

    // Hardware mode ignores the roll bindings.
    set_mode(&mut rig, Side::Left, PadMode::Hardware);
    rig.pad("pads.left.loop_roll", 4, 127);
    assert!(!rig.engine.deck(d(1)).roll_active);
}

#[test]
fn slicer_pads_repeat_and_slip_back() {
    let mut rig = Rig::attached();
    play_from(&mut rig, 1, 100_000.0);
    set_mode(&mut rig, Side::Left, PadMode::Slicer);

    let pressed = rig.pad("pads.left.slicer", 2, 127);
    assert_eq!(pressed, [Action::Slice(d(1), slot(3))]);
    for _ in 0..200 {
        rig.render();
    }
    let slicing = rig.engine.deck(d(1));
    assert!(slicing.roll_active, "the slice is held");
    let block = slicing.slicer_block_beat.expect("a grid gives a block");
    let start = (block + 2.0) * BEAT;
    assert!(
        (start..start + BEAT).contains(&slicing.position_frames),
        "inside beat 3 of the block: {} vs {start}",
        slicing.position_frames
    );
    let ghost = slicing.ghost_frames.expect("the ghost keeps moving");

    let released = rig.pad("pads.left.slicer", 2, 0);
    assert_eq!(released, [Action::Slice(d(1), slot(3))]);
    let after = rig.engine.deck(d(1));
    assert!(!after.roll_active);
    assert!(after.position_frames >= ghost, "back on the ghost");

    // A release on the shifted bank's note pairs with the press too.
    rig.pad("pads.left.slicer", 5, 127);
    assert!(rig.engine.deck(d(1)).roll_active);
    let released = rig.pad("pads.left.slicer.shift", 5, 0);
    assert_eq!(released, [Action::Slice(d(1), slot(6))]);
    assert!(!rig.engine.deck(d(1)).roll_active);
}

#[test]
fn held_roll_and_slice_release_on_the_pressed_deck() {
    for (mode, id, press_roll) in [
        (PadMode::LoopRoll, "pads.left.loop_roll", true),
        (PadMode::Slicer, "pads.left.slicer", false),
    ] {
        let mut rig = Rig::attached();
        play_from(&mut rig, 1, 100_000.0);
        set_mode(&mut rig, Side::Left, mode);
        rig.pad(id, 4, 127);
        assert!(rig.engine.deck(d(1)).roll_active, "{id}");
        let deck3_before = rig.engine.deck(d(3));

        // DECK moves the side to deck 3 while the pad is held; the release is deck 1's.
        rig.press("deck1.deck_select");
        assert_eq!(rig.runtime.state().side(Side::Left).active_deck, d(3));
        for _ in 0..50 {
            rig.render();
        }
        let released = rig.pad(id, 4, 0);
        let expected = if press_roll {
            Action::LoopRoll(d(1), slot(5))
        } else {
            Action::Slice(d(1), slot(5))
        };
        assert_eq!(released, [expected], "{id}");
        let snap = rig.engine.deck(d(1));
        assert!(!snap.roll_active, "{id}: deck 1 slipped back");
        assert_eq!(
            rig.engine.deck(d(3)),
            deck3_before,
            "{id}: deck 3 is unchanged"
        );
    }
}

#[test]
fn roll_or_slice_released_after_a_mode_change_with_shift_still_ends() {
    for (mode, id) in [
        (PadMode::LoopRoll, "pads.left.loop_roll"),
        (PadMode::Slicer, "pads.left.slicer"),
    ] {
        let mut rig = Rig::attached();
        play_from(&mut rig, 1, 100_000.0);
        set_mode(&mut rig, Side::Left, mode);
        rig.pad(id, 4, 127);
        assert!(rig.engine.deck(d(1)).roll_active, "{id}");

        // SHIFT + HOT CUE switches the side to beat jump while the pad is held; the pad
        // then sends its note-off on the shifted note, where only the beat jump binding's
        // condition holds. The held press still gets its release.
        rig.button("deck1.shift", 127);
        rig.press("deck1.mode_hot_cue.shift");
        assert_eq!(
            rig.runtime.state().side(Side::Left).pad_mode,
            PadMode::BeatJump
        );
        let before = rig.engine.deck(d(1)).position_frames;
        rig.pad(&format!("{id}.shift"), 4, 0);
        rig.button("deck1.shift", 0);
        let snap = rig.engine.deck(d(1));
        assert!(!snap.roll_active, "{id}: the hold ended");
        assert!(
            snap.position_frames > before,
            "{id}: slipped back, no beat jump backwards"
        );
    }
}

#[test]
fn roll_or_slice_released_on_the_shifted_sibling_after_a_deck_switch() {
    for (mode, id) in [
        (PadMode::LoopRoll, "pads.left.loop_roll"),
        (PadMode::Slicer, "pads.left.slicer"),
    ] {
        let mut rig = Rig::attached();
        play_from(&mut rig, 1, 100_000.0);
        set_mode(&mut rig, Side::Left, mode);
        rig.pad(id, 4, 127);
        assert!(rig.engine.deck(d(1)).roll_active, "{id}");
        let deck3_before = rig.engine.deck(d(3));

        // DECK moves the side to deck 3, then SHIFT + HOT CUE switches it to beat jump and
        // the pad is released with SHIFT down: the release is deck 1's, and deck 3 gets
        // neither a release nor a beat jump.
        rig.press("deck1.deck_select");
        assert_eq!(rig.runtime.state().side(Side::Left).active_deck, d(3));
        rig.button("deck1.shift", 127);
        rig.press("deck1.mode_hot_cue.shift");
        rig.pad(&format!("{id}.shift"), 4, 0);
        rig.button("deck1.shift", 0);
        assert!(
            !rig.engine.deck(d(1)).roll_active,
            "{id}: deck 1 slipped back"
        );
        assert_eq!(
            rig.engine.deck(d(3)),
            deck3_before,
            "{id}: deck 3 is unchanged"
        );
    }
}

#[test]
fn sampler_pads_play_and_shift_stops() {
    let mut rig = Rig::attached();
    for i in 0..8 {
        let buffer = Arc::new(TrackBuffer::new(SAMPLE_RATE, vec![0.1; 441_000 * 2]).unwrap());
        rig.engine
            .send(Command::Sampler(SamplerCommand::Set(i, Some(buffer))))
            .unwrap();
    }
    rig.render();

    // Hardware mode: both sides' SAMPLER pads play sample N.
    for (id, shift_id) in [
        ("deck1.pads_sampler", None),
        ("deck2.pads_sampler", Some("pads.right.sampler.shift")),
    ] {
        for n in 1..=8 {
            let pressed = rig.pad(id, n - 1, 127);
            assert_eq!(pressed, [Action::SamplerPlay(slot(n))], "{id} {n}");
            assert!(rig.engine.sampler().playing[n - 1], "{id} {n} plays");
            rig.pad(id, n - 1, 0);
            assert!(rig.engine.sampler().playing[n - 1], "release keeps playing");

            // SHIFT + pad N stops it (the left bank is built from the right capture).
            let stopped = match shift_id {
                Some(shift) => rig.pad(shift, n - 1, 127),
                None => rig.batch(&[note(7, 0x38 + (n as u8 - 1), 127)]).0,
            };
            assert_eq!(stopped, [Action::SamplerStop(slot(n))], "{id} {n}");
            assert!(!rig.engine.sampler().playing[n - 1], "{id} {n} stops");
        }
    }
}

#[test]
fn shift_vinyl_toggles_slip() {
    let mut rig = Rig::attached();
    assert!(!rig.engine.deck(d(1)).slip);
    assert_eq!(
        rig.press("deck1.vinyl.shift"),
        [Action::SlipToggle(d(1)); 2]
    );
    assert!(rig.engine.deck(d(1)).slip, "SHIFT + VINYL turns slip on");
    rig.press("deck1.vinyl.shift");
    assert!(!rig.engine.deck(d(1)).slip, "and off again");

    // The right side drives deck 2, and DECK moves the left side to deck 3.
    rig.press("deck2.vinyl.shift");
    assert!(rig.engine.deck(d(2)).slip);
    assert!(!rig.engine.deck(d(1)).slip);
    rig.press("deck1.deck_select");
    rig.press("deck1.vinyl.shift");
    assert!(rig.engine.deck(d(3)).slip);
    assert!(!rig.engine.deck(d(1)).slip);
}

#[test]
fn fx_buttons_toggle_the_side_deck_effects() {
    let mut rig = Rig::attached();
    assert_eq!(rig.press("fx.left.1"), [Action::FxToggle(d(1), 0); 2]);
    assert_eq!(rig.press("fx.left.3")[0], Action::FxToggle(d(1), 2));
    assert_eq!(rig.engine.channel(d(1)).fx_enabled, [true, false, true]);
    assert_eq!(rig.press("fx.right.2")[0], Action::FxToggle(d(2), 1));
    assert_eq!(rig.engine.channel(d(2)).fx_enabled, [false, true, false]);
    assert_eq!(rig.engine.channel(d(3)).fx_enabled, [false; 3]);
    // Pressing again switches the effect off.
    rig.press("fx.left.1");
    assert_eq!(rig.engine.channel(d(1)).fx_enabled, [false, false, true]);
    // SHIFT + FX is unbound.
    assert!(rig.press("fx.left.1.shift").is_empty());
    assert_eq!(rig.engine.channel(d(1)).fx_enabled, [false, false, true]);
}

#[test]
fn fx_level_drives_the_side_deck_amount() {
    let mut rig = Rig::attached();
    let moved = rig.cc14("fx.left.level", 0, 0);
    let moved_up = rig.cc14("fx.left.level", 127, 127);
    assert!(
        moved
            .iter()
            .chain(&moved_up)
            .all(|a| *a == Action::FxLevel(d(1))),
        "{moved:?} {moved_up:?}"
    );
    assert!(!moved_up.is_empty());
    assert!((rig.engine.channel(d(1)).fx_amount - 1.0).abs() < 0.01);
    rig.cc14("fx.right.level", 0, 0);
    rig.cc14("fx.right.level", 32, 0);
    assert!((rig.engine.channel(d(2)).fx_amount - 0.25).abs() < 0.01);
    assert!((rig.engine.channel(d(1)).fx_amount - 1.0).abs() < 0.01);
}

#[test]
fn fx_stays_with_its_deck_after_deck_switch() {
    let mut rig = Rig::attached();
    rig.press("fx.left.1");
    rig.cc14("fx.left.level", 0, 0);
    rig.cc14("fx.left.level", 96, 0);
    let amount = rig.engine.channel(d(1)).fx_amount;
    assert!(amount > 0.7, "{amount}");

    rig.press("deck1.deck_select");
    assert_eq!(rig.press("fx.left.2")[0], Action::FxToggle(d(3), 1));
    assert_eq!(rig.engine.channel(d(3)).fx_enabled, [false, true, false]);
    // Deck 1 keeps its effect and amount.
    assert_eq!(rig.engine.channel(d(1)).fx_enabled, [true, false, false]);
    assert_eq!(rig.engine.channel(d(1)).fx_amount, amount);
    // Deck 3's amount is still the default: the knob must reach it first.
    let moved = rig.cc14("fx.left.level", 96, 0);
    assert!(moved.is_empty(), "no jump to deck 3: {moved:?}");
    assert_eq!(rig.engine.channel(d(3)).fx_amount, 0.5);
}

#[test]
fn key_lock_toggles_keylock_and_shift_key_lock_still_changes_range() {
    let mut rig = Rig::attached();
    assert_eq!(
        rig.press("deck1.key_lock"),
        [Action::KeylockToggle(d(1)); 2]
    );
    assert!(rig.engine.deck(d(1)).keylock);
    assert_eq!(rig.press("deck2.key_lock")[0], Action::KeylockToggle(d(2)));
    assert!(rig.engine.deck(d(2)).keylock);
    rig.press("deck1.key_lock");
    assert!(!rig.engine.deck(d(1)).keylock);

    let before = rig.runtime.state().deck(d(1)).tempo_range;
    assert_eq!(
        rig.press("deck1.key_lock.shift")[0],
        Action::TempoRangeNext(d(1))
    );
    assert_ne!(rig.runtime.state().deck(d(1)).tempo_range, before);
    assert!(
        !rig.engine.deck(d(1)).keylock,
        "SHIFT + KEY LOCK left keylock alone"
    );
}

/// What each message of the captured FX FADE and PAD SCRATCH pad steps drives, in the
/// order the capture lists them: the pads' own notes and the firmware's messages (the
/// routine's loop notes and its JOG_VINYL scratch). `0x4D` (LOOP_FADE_LOOP) is unbound.
fn firmware_pad_rows(deck: DeckId) -> Vec<(&'static str, Vec<Action>)> {
    let fx_fade = vec![
        vec![],                            // pad 1 (filter sweep: fader/filter messages)
        vec![],                            // pad 2
        vec![Action::LoopExit(deck)],      // pad 3
        vec![],                            // 0x4D LOOP_FADE_LOOP
        vec![Action::LoopFade1(deck)],     // 0x4C LOOP_FADE_SEL_1
        vec![Action::BackspinShort(deck)], // pad 4
        vec![Action::JogScratch(deck)],    // cc 0x22 JOG_VINYL
        vec![Action::JogTouch(deck)],      // jog touch
        vec![],                            // pad 5
        vec![],                            // pad 6
        vec![Action::LoopExit(deck)],      // pad 7
        vec![Action::LoopFade4(deck)],     // 0x4A LOOP_FADE_SEL_4
        vec![Action::BackspinLong(deck)],  // pad 8
    ];
    let pad_scratch = vec![
        vec![],                              // pad 1
        vec![Action::JogTouch(deck)],        // jog touch
        vec![Action::Hotcue(deck, slot(1))], // HOT CUE pad 1
        vec![Action::JogScratch(deck)],      // cc 0x22 JOG_VINYL
    ];
    let (fx, ps) = if deck == d(1) {
        ("deck1.pads_fx_fade", "deck1.pads_pad_scratch")
    } else {
        ("deck2.pads_fx_fade", "deck2.pads_pad_scratch")
    };
    let mut rows: Vec<(&'static str, Vec<Action>)> = fx_fade.into_iter().map(|a| (fx, a)).collect();
    rows.extend(pad_scratch.into_iter().map(|a| (ps, a)));
    rows
}

#[test]
fn firmware_pad_messages_drive_their_actions() {
    for deck in [d(1), d(2)] {
        let rows = firmware_pad_rows(deck);
        let mut index = std::collections::HashMap::<&str, usize>::new();
        for (id, expected) in rows {
            let i = index.entry(id).or_insert(0);
            let mut rig = Rig::attached();
            let m = rig.control(id).messages[*i];
            let channel = m.status & 0x0F;
            let msg = match m.status & 0xF0 {
                0x90 => note(channel, m.data1, m.max),
                _ => cc(channel, m.data1, m.max),
            };
            let (actions, _) = rig.batch(&[msg]);
            assert_eq!(actions, expected, "`{id}` message {i} ({msg:?})");
            *i += 1;
        }
    }
}

#[test]
fn pad_scratch_mode_anchors_the_playhead_and_leaves_hotcues_alone() {
    let mut rig = Rig::attached();
    play_from(&mut rig, 1, 10_000.0);
    set_mode(&mut rig, Side::Left, PadMode::PadScratch);

    // A routine pad sets the anchor; the firmware's HOT CUE 1 note (index 2) returns to it.
    let set = rig.pad("deck1.pads_pad_scratch", 0, 127);
    assert_eq!(set, [Action::ScratchAnchorSet(d(1))]);
    let anchor = rig.runtime.state().deck(d(1)).scratch_anchor.unwrap();
    assert!((anchor - 10_000.0).abs() < 5_000.0, "anchor {anchor}");
    rig.deck_command(1, DeckCommand::Seek(150_000.0));
    let back = rig.pad("deck1.pads_pad_scratch", 2, 127);
    assert_eq!(back, [Action::ScratchAnchorReturn(d(1))]);
    let returned = rig.engine.deck(d(1)).position_frames;
    assert!(
        (returned - anchor).abs() < 2_000.0,
        "returned to {returned}, anchor {anchor}"
    );
    rig.pad("deck1.pads_pad_scratch", 2, 0);
    assert_eq!(rig.engine.deck(d(1)).hotcues, [None; 8], "no hotcue is set");

    // Every routine pad (0x20-0x27) sets the anchor.
    for index in [4, 10] {
        rig.deck_command(1, DeckCommand::Seek(60_000.0));
        let set = rig.pad("deck1.pads_pad_scratch", index, 127);
        assert_eq!(set, [Action::ScratchAnchorSet(d(1))], "message {index}");
    }

    // HOT CUE mode brings the hotcue pads back.
    set_mode(&mut rig, Side::Left, PadMode::Hardware);
    rig.deck_command(1, DeckCommand::Seek(30_000.0));
    let set = rig.pad("deck1.pads_hot_cue", 0, 127);
    assert_eq!(set, [Action::Hotcue(d(1), slot(1))]);
    let cue = rig.engine.deck(d(1)).hotcues[0]
        .expect("pad 1 set hotcue 1")
        .frames;
    assert!((cue - 30_000.0).abs() < 2_000.0, "hotcue 1 at {cue}");
    // And the routine pads do nothing outside PAD SCRATCH mode.
    assert_eq!(rig.pad("deck1.pads_pad_scratch", 0, 127), []);
}
