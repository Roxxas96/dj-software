mod common;

use std::f64::consts::TAU;
use std::sync::Arc;

use common::render;
use engine::{
    ChannelCommand, Command, DeckCommand, DeckId, EngineConfig, EngineController, EngineRenderer,
    HotcueSlot, JogSurface, MasterCommand, TempoRange, TrackBuffer, TrackMemory, XfaderAssign,
    engine,
};

const SR: u32 = 44_100;
/// Tempo slider position for +8 % on the ±10 % range.
const PLUS_8: f32 = 0.9;

fn deck() -> DeckId {
    DeckId::new(0).unwrap()
}

/// A stereo sine of `hz` for `seconds`, then (if given) a sine of `then_hz` for as long.
fn sine_track(hz: f64, then_hz: Option<f64>, seconds: f64) -> Arc<TrackBuffer> {
    let half = (seconds * f64::from(SR)) as usize;
    let frames = if then_hz.is_some() { 2 * half } else { half };
    let samples = (0..frames)
        .flat_map(|i| {
            let f = if i < half { hz } else { then_hz.unwrap() };
            let v = (0.5 * (TAU * f * i as f64 / f64::from(SR)).sin()) as f32;
            [v, v]
        })
        .collect();
    Arc::new(TrackBuffer::new(SR, samples).unwrap())
}

/// An engine passing deck 1 to master at unity (EQ at unity: an allpass, so zero
/// crossings keep their frequency), with `track` loaded, the tempo slider at `slider` on
/// the ±10 % range and keylock set to `keylock`. Not playing yet.
fn setup(
    track: Arc<TrackBuffer>,
    slider: f32,
    keylock: bool,
) -> (EngineController, EngineRenderer) {
    let (mut c, r) = engine(EngineConfig {
        sample_rate: SR,
        max_block_frames: 512,
    });
    c.send(Command::Master(MasterCommand::MasterLevel(1.0)))
        .unwrap();
    c.send(Command::Channel(
        deck(),
        ChannelCommand::XfaderAssign(XfaderAssign::Thru),
    ))
    .unwrap();
    for cmd in [
        DeckCommand::Load {
            track,
            memory: TrackMemory::default(),
        },
        DeckCommand::SetTempoRange(TempoRange::Ten),
        DeckCommand::SetTempoSlider(slider),
        DeckCommand::SetKeylock(keylock),
    ] {
        c.send(Command::Deck(deck(), cmd)).unwrap();
    }
    (c, r)
}

fn send(c: &mut EngineController, cmd: DeckCommand) {
    c.send(Command::Deck(deck(), cmd)).unwrap();
}

/// The dominant frequency of the left channel of `out` (interleaved stereo), by zero
/// crossings over `seconds` starting at `from` seconds.
fn frequency(out: &[f32], from: f64, seconds: f64) -> f64 {
    let start = (from * f64::from(SR)) as usize;
    let len = (seconds * f64::from(SR)) as usize;
    let left: Vec<f32> = out
        .iter()
        .step_by(2)
        .skip(start)
        .take(len)
        .copied()
        .collect();
    assert_eq!(left.len(), len, "enough output to measure");
    let crossings = left
        .windows(2)
        .filter(|w| (w[0] < 0.0) != (w[1] < 0.0))
        .count();
    crossings as f64 / 2.0 / seconds
}

fn assert_near(actual: f64, expected: f64, tolerance: f64) {
    assert!(
        (actual - expected).abs() <= expected * tolerance,
        "{actual} Hz is not within {}% of {expected} Hz",
        tolerance * 100.0
    );
}

#[test]
fn keylock_keeps_pitch_at_plus_8_percent() {
    let (mut c, mut r) = setup(sine_track(440.0, None, 3.0), PLUS_8, true);
    send(&mut c, DeckCommand::Play);
    let (out, _) = render(&mut r, (0.8 * f64::from(SR)) as usize);
    assert!((c.deck(deck()).tempo_ratio - 1.08).abs() < 1e-6);
    assert_near(frequency(&out, 0.2, 0.5), 440.0, 0.02);

    let (mut c, mut r) = setup(sine_track(440.0, None, 3.0), PLUS_8, false);
    send(&mut c, DeckCommand::Play);
    let (out, _) = render(&mut r, (0.8 * f64::from(SR)) as usize);
    assert_near(frequency(&out, 0.2, 0.5), 440.0 * 1.08, 0.02);
}

#[test]
fn keylock_keeps_tempo() {
    let (mut c, mut r) = setup(sine_track(440.0, None, 15.0), PLUS_8, true);
    send(&mut c, DeckCommand::Play);
    render(&mut r, 1000);
    let start = c.deck(deck()).position_frames;
    let seconds = 10;
    render(&mut r, seconds * SR as usize);
    let moved = c.deck(deck()).position_frames - start;
    let expected = c.deck(deck()).tempo_ratio * f64::from(SR) * seconds as f64;
    assert!(
        (moved - expected).abs() <= seconds as f64,
        "moved {moved}, expected {expected}"
    );
}

#[test]
fn keylock_jump_is_heard_at_once() {
    let (mut c, mut r) = setup(sine_track(440.0, Some(880.0), 3.0), PLUS_8, true);
    let slot = HotcueSlot::new(1).unwrap();
    send(
        &mut c,
        DeckCommand::SetHotcue(slot, Some(4.0 * f64::from(SR))),
    );
    send(&mut c, DeckCommand::Play);
    let (before, _) = render(&mut r, SR as usize / 2);
    assert_near(frequency(&before, 0.2, 0.3), 440.0, 0.02);
    send(&mut c, DeckCommand::HotcuePress(slot));
    let (after, _) = render(&mut r, SR as usize / 2);
    assert_near(frequency(&after, 0.03, 0.1), 880.0, 0.02);
    // Heard at full level, not faded in from silence.
    let window = &after[2 * 1323..2 * (1323 + 4410)];
    let rms = (window.iter().map(|s| s * s).sum::<f32>() / window.len() as f32).sqrt();
    assert!(rms > 0.25, "rms {rms}");
}

/// Renders a scratch from frame 100 000 of a ramp track: touch, a quick turn back.
fn scratch(keylock: bool) -> (Vec<f32>, f64) {
    let (mut c, mut r) = setup(common::ramp_track(SR, 300_000), PLUS_8, keylock);
    send(&mut c, DeckCommand::Seek(100_000.0));
    send(&mut c, DeckCommand::Play);
    let (mut out, _) = render(&mut r, 4096);
    send(&mut c, DeckCommand::JogTouch(true));
    send(
        &mut c,
        DeckCommand::Jog {
            turns: -0.2,
            surface: JogSurface::Top,
        },
    );
    out.extend(render(&mut r, 8192).0);
    (out, c.deck(deck()).position_frames)
}

#[test]
fn scratch_bypasses_keylock() {
    let (plain, plain_position) = scratch(false);
    let (locked, locked_position) = scratch(true);
    // Before the touch the decks differ (one is stretched); while scratching both follow
    // the platter alone and render the same frames.
    assert!(plain[..2 * 4096] != locked[..2 * 4096]);
    // The stretched deck's playhead moves by whole frames, so it may be a fraction off.
    assert!((plain_position - locked_position).abs() < 1.0);
    let plain_scratch = &plain[2 * 4096..];
    let locked_scratch = &locked[2 * 4096..];
    // The isolator carries a short tail of what came before; compare once it has settled.
    let settled = 2 * common::SETTLE_FRAMES;
    for (a, b) in plain_scratch[settled..]
        .iter()
        .zip(&locked_scratch[settled..])
    {
        assert!((a - b).abs() < 1e-4, "{a} vs {b}");
    }
}

/// Renders one second of a sine at `slider` with or without keylock.
fn play(slider: f32, keylock: bool) -> (Vec<f32>, f64) {
    let (mut c, mut r) = setup(sine_track(440.0, None, 3.0), slider, keylock);
    send(&mut c, DeckCommand::Play);
    let (out, _) = render(&mut r, SR as usize);
    (out, c.deck(deck()).position_frames)
}

#[test]
fn ratio_one_bypasses_keylock() {
    assert!(play(0.5, true) == play(0.5, false));
    // Within 1e-4 of the original tempo counts as the original tempo.
    let nearly_one = 0.5 + 0.00004 / 0.2;
    assert!(play(nearly_one, true) == play(nearly_one, false));
    // Further off, keylock stretches.
    assert!(play(PLUS_8, true).0 != play(PLUS_8, false).0);
}

#[test]
fn keylock_off_by_default_and_kept_across_load() {
    let (mut c, mut r) = engine(EngineConfig {
        sample_rate: SR,
        max_block_frames: 512,
    });
    render(&mut r, 1);
    assert!(!c.deck(deck()).keylock);
    send(&mut c, DeckCommand::SetKeylock(true));
    render(&mut r, 1);
    assert!(c.deck(deck()).keylock);
    send(
        &mut c,
        DeckCommand::Load {
            track: sine_track(440.0, None, 1.0),
            memory: TrackMemory::default(),
        },
    );
    render(&mut r, 1);
    assert_eq!(c.deck(deck()).load_seq, 1);
    assert!(c.deck(deck()).keylock);
    send(&mut c, DeckCommand::SetKeylock(false));
    render(&mut r, 1);
    assert!(!c.deck(deck()).keylock);
}

/// Renders a sine at the original tempo with or without keylock, nudged after 4096 frames.
fn nudge_at_ratio_one(keylock: bool) -> (Vec<f32>, f64) {
    let (mut c, mut r) = setup(sine_track(440.0, None, 3.0), 0.5, keylock);
    send(&mut c, DeckCommand::Play);
    let (mut out, _) = render(&mut r, 4096);
    send(
        &mut c,
        DeckCommand::Jog {
            turns: 0.05,
            surface: JogSurface::Ring,
        },
    );
    out.extend(render(&mut r, SR as usize).0);
    (out, c.deck(deck()).position_frames)
}

#[test]
fn nudge_at_ratio_one_keeps_keylock_bypassed() {
    // The bypass follows the tempo alone: a nudge at the original tempo bends the
    // resampled path (like vinyl) and never switches to the stretcher and back.
    assert!(nudge_at_ratio_one(true) == nudge_at_ratio_one(false));
}

#[test]
fn backspin_bypasses_keylock_and_restarts_it() {
    let (mut c, mut r) = setup(sine_track(440.0, None, 6.0), PLUS_8, true);
    send(&mut c, DeckCommand::Seek(3.0 * f64::from(SR)));
    send(&mut c, DeckCommand::Play);
    render(&mut r, 4096);
    send(&mut c, DeckCommand::Backspin { seconds: 1.0 });
    let (spin, _) = render(&mut r, SR as usize);
    // Raw resampled playback: the pitch falls from 3 x 440 x 1.08 towards 0.
    assert_near(frequency(&spin, 0.03, 0.05), 3.0 * 440.0 * 1.08, 0.1);
    let (after, _) = render(&mut r, SR as usize);
    assert!(!c.deck(deck()).backspinning);
    // Stretched playback again: the pitch is back to the track's own.
    assert_near(frequency(&after, 0.3, 0.3), 440.0, 0.02);
    let window = &after[2 * 13_230..2 * (13_230 + 4410)];
    let rms = (window.iter().map(|s| s * s).sum::<f32>() / window.len() as f32).sqrt();
    assert!(rms > 0.25, "rms {rms}");
}
