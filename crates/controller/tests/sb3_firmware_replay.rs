//! Replays the SB3's own pad routines, recorded in
//! `controllers/ddj-sb3/firmware-modes-2.txt`, through the built-in mapping and a real
//! engine playing a gridded track. Each message is delivered at its recorded time, with the
//! engine rendered in between, so the routines run as they would on the hardware.
//!
//! The recording (see its header): left fader at 0x58, left FILTER at 0x15, a track
//! playing; FX FADE pad 1 on/off, pad 3 on/off, pad 4 twice; TRANS (SHIFT + SAMPLER) pad 1
//! held 3 s; PAD SCRATCH pad 1 held 3 s.

use std::sync::Arc;

use controller::builtin::load_mapping_from;
use controller::{MidiMessage, MidiOut, Runtime, TimedMidi};
use engine::{
    Command, DeckCommand, DeckId, DeckSnapshot, EngineConfig, EngineController, EngineRenderer,
    Grid, TrackBuffer, engine,
};

const RECORDING: &str = include_str!("../../../controllers/ddj-sb3/firmware-modes-2.txt");

const SAMPLE_RATE: u32 = 44_100;
const BLOCK: usize = 512;
/// The PAD SCRATCH routine repeats about every 373 ms, close to a beat at 160 BPM.
const BPM: f64 = 160.0;
/// Where the decks start playing, far enough in for a backspin.
const START_SECONDS: f64 = 10.0;
/// Long enough for the start, the whole recording (about 52 s) and some playing after it.
const TRACK_SECONDS: usize = 75;

/// The physical positions the routines restore: fader and FILTER, MSB then LSB.
const FADER: (u8, u8) = (0x58, 0x57);
const FILTER: (u8, u8) = (0x15, 0x36);

/// The routines' segments in the recording, in ms (start inclusive, end exclusive).
const FX_FADE_PAD_1: (u64, u64) = (35_405, 45_342);
const FX_FADE_PAD_3: (u64, u64) = (45_342, 55_222);
/// Pressed at 55 222 ms and again at 61 643 ms.
const FX_FADE_PAD_4: (u64, u64) = (55_222, 63_838);
/// SHIFT + SAMPLER at 64 444 ms, pad 1 held from 70 513 to 74 013 ms.
const TRANS: (u64, u64) = (63_838, 77_629);
/// PAD SCRATCH at 77 629 ms, pad 1 held from 79 418 to 83 389 ms.
const PAD_SCRATCH: (u64, u64) = (77_629, u64::MAX);

/// The recorded messages, `(ms, message)`, in order; headers are skipped.
fn recording() -> Vec<(u64, MidiMessage)> {
    RECORDING
        .lines()
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            let ms = words.next()?.parse::<u64>().ok()?;
            if words.next()? != "ms" {
                return None;
            }
            let bytes: Vec<u8> = words
                .take(3)
                .map(|w| u8::from_str_radix(w, 16).expect("a hex byte"))
                .collect();
            let msg = MidiMessage::parse(&bytes).expect("a channel message");
            Some((ms, msg))
        })
        .collect()
}

fn segment((start, end): (u64, u64)) -> Vec<(u64, MidiMessage)> {
    recording()
        .into_iter()
        .filter(|&(ms, _)| start <= ms && ms < end)
        .collect()
}

fn d(n: usize) -> DeckId {
    DeckId::new(n - 1).unwrap()
}

fn note(channel: u8, note: u8, velocity: u8) -> MidiMessage {
    MidiMessage::parse(&[0x90 | (channel - 1), note, velocity]).unwrap()
}

fn cc(channel: u8, controller: u8, value: u8) -> MidiMessage {
    MidiMessage::parse(&[0xB0 | (channel - 1), controller, value]).unwrap()
}

fn fourteen((msb, lsb): (u8, u8)) -> f32 {
    f32::from(u16::from(msb) << 7 | u16::from(lsb)) / 16383.0
}

struct NoOut;

impl MidiOut for NoOut {
    fn send(&mut self, _: MidiMessage) {}
}

/// The engine, the SB3 runtime and a clock in ms.
struct Rig {
    runtime: Runtime,
    engine: EngineController,
    renderer: EngineRenderer,
    /// The recording time the engine has rendered up to, in ms.
    now_ms: u64,
    /// Frames owed to the engine, below one (ms do not divide into frames).
    frac: f64,
    buf: Vec<f32>,
    buf2: Vec<f32>,
}

impl Rig {
    /// Decks 1 and 3 hold a gridded track and play from `START_SECONDS`; the mapping is attached; the left
    /// fader and FILTER sit where the recording has them, as of `start_ms`.
    fn playing(start_ms: u64) -> Self {
        let (mut engine, renderer) = engine(EngineConfig {
            sample_rate: SAMPLE_RATE,
            max_block_frames: BLOCK,
        });
        let frames = TRACK_SECONDS * SAMPLE_RATE as usize;
        let track = Arc::new(TrackBuffer::new(SAMPLE_RATE, vec![0.1; frames * 2]).unwrap());
        let grid = Grid::from_bpm(BPM, 0.0, SAMPLE_RATE).unwrap();
        for deck in [d(1), d(3)] {
            for command in [
                DeckCommand::Load {
                    track: Arc::clone(&track),
                    memory: engine::TrackMemory::default(),
                },
                DeckCommand::SetGrid(Some(grid)),
                DeckCommand::Seek(START_SECONDS * f64::from(SAMPLE_RATE)),
                DeckCommand::Play,
            ] {
                engine.send(Command::Deck(deck, command)).unwrap();
            }
        }
        let mapping = load_mapping_from("ddj-sb3", None).expect("the built-in SB3 mapping");
        let mut rig = Self {
            runtime: Runtime::new(mapping),
            engine,
            renderer,
            now_ms: start_ms,
            frac: 0.0,
            buf: vec![0.0; BLOCK * 2],
            buf2: vec![0.0; BLOCK * 2],
        };
        rig.render_frames(BLOCK);
        rig.runtime.attach(&mut rig.engine, &mut NoOut);
        rig.render_frames(BLOCK);
        let at = rig.now_ms;
        rig.batch(
            at,
            &[
                cc(1, 0x13, FADER.0),
                cc(1, 0x33, FADER.1),
                cc(7, 0x17, FILTER.0),
                cc(7, 0x37, FILTER.1),
            ],
        );
        rig
    }

    fn render_frames(&mut self, mut frames: usize) {
        while frames > 0 {
            let n = frames.min(BLOCK);
            self.renderer
                .render(&mut self.buf[..n * 2], &mut self.buf2[..n * 2]);
            frames -= n;
        }
    }

    /// Renders the engine up to `ms`.
    fn advance_to(&mut self, ms: u64) {
        if ms <= self.now_ms {
            return;
        }
        let exact = (ms - self.now_ms) as f64 * f64::from(SAMPLE_RATE) / 1000.0 + self.frac;
        let frames = exact.floor();
        self.frac = exact - frames;
        self.render_frames(frames as usize);
        self.now_ms = ms;
    }

    /// Delivers `msgs` as one batch received at `ms`, rendering up to it first.
    fn batch(&mut self, ms: u64, msgs: &[MidiMessage]) {
        self.advance_to(ms);
        let timed: Vec<TimedMidi> = msgs
            .iter()
            .map(|&message| TimedMidi {
                micros: ms * 1000,
                message,
            })
            .collect();
        let stats = self.runtime.handle_batch(&timed, &mut self.engine);
        assert_eq!(
            stats.queue_full, 0,
            "the engine queue overflowed at {ms} ms"
        );
        // A tiny render so the engine takes the commands in and publishes its snapshot.
        self.render_frames(1);
    }

    /// Plays `messages` at their recorded times, one batch per ms, and calls `each` after
    /// every batch with the batch's time.
    fn play(&mut self, messages: &[(u64, MidiMessage)], mut each: impl FnMut(&Rig, u64)) {
        let mut i = 0;
        while i < messages.len() {
            let ms = messages[i].0;
            let n = messages[i..].iter().take_while(|(t, _)| *t == ms).count();
            let batch: Vec<MidiMessage> = messages[i..i + n].iter().map(|(_, m)| *m).collect();
            self.batch(ms, &batch);
            each(self, ms);
            i += n;
        }
    }

    fn deck(&self, n: usize) -> DeckSnapshot {
        self.engine.deck(d(n))
    }

    fn fader(&self, n: usize) -> f32 {
        self.runtime.state().deck(d(n)).fader
    }

    fn filter(&self, n: usize) -> f32 {
        self.runtime.state().deck(d(n)).filter
    }

    /// Seconds of track played, from the playhead.
    fn seconds(&self, n: usize) -> f64 {
        self.deck(n).position_frames / f64::from(SAMPLE_RATE)
    }
}

fn assert_close(actual: f32, expected: f32, what: &str) {
    assert!(
        (actual - expected).abs() < 1e-6,
        "{what}: {actual} != {expected}"
    );
}

/// No routine left anything on deck 1: fader and FILTER as the controls are, no loop, no
/// spin, no hold, and the deck plays.
fn assert_left_as_the_controls_are(rig: &Rig) {
    assert_close(rig.fader(1), fourteen(FADER), "deck 1 fader");
    assert_close(rig.filter(1), fourteen(FILTER), "deck 1 filter");
    let deck = rig.deck(1);
    assert!(!deck.loop_active, "a loop is left on");
    assert!(!deck.backspinning, "a backspin is left running");
    assert!(!deck.jog_touched, "the jog is left touched");
    assert!(deck.playing, "the deck stopped");
}

#[test]
fn the_recording_parses() {
    let messages = recording();
    assert_eq!(messages.len(), 3612);
    assert_eq!(messages[0].0, 31_363);
    assert!(messages.windows(2).all(|w| w[0].0 <= w[1].0));
}

#[test]
fn recorded_routines_leave_the_deck_as_the_controls_are() {
    let messages = recording();
    let mut rig = Rig::playing(messages[0].0);
    rig.play(&messages, |_, _| {});
    let end = rig.now_ms;
    rig.advance_to(end + 3_000);
    assert_left_as_the_controls_are(&rig);
    // Deck 3 was never shown on the left: untouched.
    assert!(!rig.deck(3).loop_active);
    // (its fader stays closed, as the mapping's init left it)
    assert_close(rig.runtime.state().deck(d(3)).fader, 0.0, "deck 3 fader");
}

#[test]
fn fx_fade_pad_1_sweeps_the_filter_then_restores() {
    let messages = segment(FX_FADE_PAD_1);
    let mut rig = Rig::playing(FX_FADE_PAD_1.0);
    let mut highest = 0.0f32;
    rig.play(&messages, |rig, _| highest = highest.max(rig.filter(1)));
    assert!(highest > 0.99, "the filter swept up to {highest}");
    rig.advance_to(FX_FADE_PAD_1.1);
    assert_left_as_the_controls_are(&rig);
}

#[test]
fn fx_fade_pad_3_loops_and_fades_then_restores() {
    let messages = segment(FX_FADE_PAD_3);
    let mut rig = Rig::playing(FX_FADE_PAD_3.0);
    let mut faded_in_loop = false;
    rig.play(&messages, |rig, ms| {
        if (45_400..50_766).contains(&ms) {
            let deck = rig.deck(1);
            assert!(deck.loop_active, "no loop at {ms} ms");
            assert_eq!(deck.loop_beats, 1.0, "at {ms} ms");
            faded_in_loop |= rig.fader(1) == 0.0;
        }
    });
    assert!(faded_in_loop, "the fader never reached 0 during the loop");
    rig.advance_to(FX_FADE_PAD_3.1);
    assert_left_as_the_controls_are(&rig);
}

#[test]
fn fx_fade_pad_3_replaces_the_djs_own_loop() {
    let mut rig = Rig::playing(FX_FADE_PAD_3.0 - 1_000);
    // The DJ's own loop: 2X then AUTO LOOP.
    let at = rig.now_ms + 10;
    rig.batch(at, &[note(1, 0x13, 127), note(1, 0x13, 0)]);
    rig.batch(at + 10, &[note(1, 0x14, 127), note(1, 0x14, 0)]);
    let dj_loop = rig.deck(1);
    assert!(dj_loop.loop_active);
    assert_ne!(dj_loop.loop_beats, 1.0);

    let messages = segment(FX_FADE_PAD_3);
    let (first, second): (Vec<_>, Vec<_>) = messages.iter().partition(|(ms, _)| *ms < 50_766);
    rig.play(&first, |_, _| {});
    let fade_loop = rig.deck(1);
    assert!(fade_loop.loop_active, "the first press left no loop");
    assert_eq!(
        fade_loop.loop_beats, 1.0,
        "the first press kept the DJ's loop"
    );
    assert_ne!(fade_loop.loop_in, dj_loop.loop_in);
    rig.play(&second, |_, _| {});
    assert!(
        !rig.deck(1).loop_active,
        "the second press left the loop on"
    );
}

#[test]
fn fx_fade_pad_4_backspins() {
    let messages = segment(FX_FADE_PAD_4);
    let mut rig = Rig::playing(FX_FADE_PAD_4.0);
    // The first press comes with the SB3's own jog touch and backward nudge (JOG_VINYL),
    // the second without.
    for press in [55_222, 61_643] {
        let at_press: Vec<_> = messages
            .iter()
            .copied()
            .filter(|(ms, _)| (press..press + 500).contains(ms))
            .collect();
        rig.advance_to(press);
        let start = rig.seconds(1);
        let mut spun = false;
        let mut lowest = start;
        rig.play(&at_press, |rig, _| spun |= rig.deck(1).backspinning);
        // Watch the spin play out (the short backspin lasts 1 s).
        while rig.now_ms < press + 1_500 {
            let next = rig.now_ms + 20;
            rig.advance_to(next);
            spun |= rig.deck(1).backspinning;
            lowest = lowest.min(rig.seconds(1));
        }
        assert!(spun, "pad 4 at {press} ms did not backspin");
        // 1 s from 3x speed down to 0 travels 1.5 s back (less the press's own play).
        assert!(
            start - lowest > 1.0,
            "pad 4 at {press} ms went back only {} s",
            start - lowest
        );
        let low = rig.seconds(1);
        let next = rig.now_ms + 500;
        rig.advance_to(next);
        assert!(
            rig.seconds(1) > low,
            "the deck does not play on after the spin"
        );
    }
    rig.play(
        &messages
            .iter()
            .copied()
            .filter(|(ms, _)| *ms > rig.now_ms)
            .collect::<Vec<_>>(),
        |_, _| {},
    );
    rig.advance_to(FX_FADE_PAD_4.1);
    assert_left_as_the_controls_are(&rig);
}

#[test]
fn trans_cuts_and_restores_the_fader() {
    let messages = segment(TRANS);
    let mut rig = Rig::playing(TRANS.0);
    let (mut cuts, mut opens) = (0, 0);
    rig.play(&messages, |rig, ms| {
        if (70_520..74_013).contains(&ms) {
            match rig.fader(1) {
                0.0 => cuts += 1,
                v if (v - fourteen(FADER)).abs() < 1e-6 => opens += 1,
                v => panic!("fader at {v} at {ms} ms"),
            }
        }
    });
    assert!(cuts > 100 && opens > 100, "{cuts} cuts, {opens} opens");
    rig.advance_to(TRANS.1);
    assert_left_as_the_controls_are(&rig);
}

#[test]
fn pad_scratch_scratches_from_hotcue_1() {
    let messages = segment(PAD_SCRATCH);
    let mut rig = Rig::playing(PAD_SCRATCH.0);
    let first = messages
        .iter()
        .find(|(_, m)| {
            matches!(
                m,
                MidiMessage::NoteOn {
                    channel: 7,
                    note: 0x20,
                    ..
                }
            )
        })
        .unwrap()
        .0;
    let (before, held): (Vec<_>, Vec<_>) = messages.iter().partition(|(ms, _)| *ms < first);
    rig.play(&before, |_, _| {});
    assert_eq!(rig.deck(1).hotcues[0], None);
    rig.advance_to(first);
    let at_press = rig.deck(1).position_frames;
    let mut cue = None;
    let mut furthest = 0.0f64;
    rig.play(&held, |rig, ms| {
        let deck = rig.deck(1);
        let hotcue = deck.hotcues[0].expect("hotcue 1 is set at the first message");
        let cue = *cue.get_or_insert(hotcue);
        assert_eq!(hotcue, cue, "hotcue 1 moved at {ms} ms");
        furthest = furthest.max((deck.position_frames - cue).abs());
    });
    let cue = cue.unwrap();
    assert!(
        (cue - at_press).abs() < 0.05 * f64::from(SAMPLE_RATE),
        "hotcue 1 at {cue}, pressed at {at_press}"
    );
    assert!(
        furthest < 0.5 * f64::from(SAMPLE_RATE),
        "the playhead strayed {furthest} frames from hotcue 1"
    );
    // After the release the deck plays on from the cue.
    let end = rig.now_ms;
    rig.advance_to(end + 2_000);
    assert!(rig.seconds(1) - cue / f64::from(SAMPLE_RATE) > 1.5);
    assert_left_as_the_controls_are(&rig);
}

#[test]
fn fx_fade_loop_ends_on_its_deck_after_deck_switch() {
    let mut rig = Rig::playing(1_000);
    // FX FADE pad 3, as recorded: the pad, then LOOP_FADE_LOOP and LOOP_FADE_SEL_1.
    rig.batch(1_010, &[note(8, 0x12, 127)]);
    rig.batch(1_011, &[note(1, 0x4D, 127), note(1, 0x4C, 127)]);
    rig.batch(1_120, &[note(8, 0x12, 0)]);
    assert!(rig.deck(1).loop_active);
    // DECK: the left side shows deck 3.
    rig.batch(1_500, &[note(1, 0x72, 127)]);
    rig.batch(1_600, &[note(1, 0x72, 0)]);
    assert_eq!(
        rig.runtime.state().side(actions::Side::Left).active_deck,
        d(3)
    );
    // Pad 3 again ends the routine on the deck it started on.
    rig.batch(2_000, &[note(8, 0x12, 127)]);
    rig.batch(2_100, &[note(8, 0x12, 0)]);
    assert!(!rig.deck(1).loop_active, "deck 1 still loops");
    assert!(!rig.deck(3).loop_active, "deck 3 started looping");
    // The next routine starts on the deck the side now shows.
    rig.batch(3_000, &[note(8, 0x12, 127)]);
    rig.batch(3_001, &[note(1, 0x4D, 127), note(1, 0x4C, 127)]);
    rig.batch(3_100, &[note(8, 0x12, 0)]);
    assert!(rig.deck(3).loop_active);
    assert!(!rig.deck(1).loop_active);
}

#[test]
fn fx_fade_pad_8_backspins_longer() {
    // FX FADE pad 8 as condensed in firmware-modes.txt (110 572 ms): the pad, the touch
    // burst with five JOG_VINYL steps of -8 (0x38), the release, the pad release. The fader
    // and FILTER messages of that recording are left out (other positions).
    let mut rig = Rig::playing(110_000);
    let start = rig.seconds(1) + 0.572;
    rig.batch(110_572, &[note(8, 0x17, 127)]);
    rig.batch(110_573, &[note(1, 0x36, 127); 3]);
    rig.batch(
        110_574,
        &[
            cc(1, 0x22, 0x38),
            cc(1, 0x22, 0x38),
            note(1, 0x36, 127),
            note(1, 0x36, 127),
        ],
    );
    rig.batch(
        110_575,
        &[cc(1, 0x22, 0x38), cc(1, 0x22, 0x38), cc(1, 0x22, 0x38)],
    );
    rig.batch(110_575, &[note(1, 0x36, 127)]);
    rig.batch(110_600, &[note(1, 0x36, 0)]);
    rig.batch(110_720, &[note(8, 0x17, 0)]);
    assert!(rig.deck(1).backspinning);
    let mut lowest = start;
    while rig.now_ms < 112_700 {
        let next = rig.now_ms + 20;
        rig.advance_to(next);
        lowest = lowest.min(rig.seconds(1));
    }
    // The long backspin, 2 s from 3x speed: about 3 s back.
    assert!(start - lowest > 2.5, "went back only {} s", start - lowest);
    rig.advance_to(113_500);
    assert!(!rig.deck(1).backspinning);
    assert!(rig.seconds(1) > lowest);
}
