mod common;

use std::f32::consts::TAU;
use std::sync::Arc;

use approx::assert_abs_diff_eq;
use common::render;
use engine::mixer::eq_gain;
use engine::{
    ChannelCommand, Command, DeckCommand, DeckId, EngineConfig, EngineController, EngineRenderer,
    MasterCommand, TrackBuffer, XfaderAssign, engine,
};

const SR: u32 = 44_100;
const BLOCK: usize = 512;
const AMPLITUDE: f32 = 0.5;
const SETTLE_FRAMES: usize = 4096;
const MEASURE_FRAMES: usize = 8192;

/// A stereo sine track at `AMPLITUDE`.
fn sine(freq: f32, sr: u32, frames: usize) -> Arc<TrackBuffer> {
    let samples = (0..frames)
        .flat_map(|i| {
            let v = AMPLITUDE * (TAU * freq * i as f32 / sr as f32).sin();
            [v, v]
        })
        .collect();
    Arc::new(TrackBuffer::new(sr, samples).unwrap())
}

/// A stereo full-scale white noise track (deterministic LCG).
fn noise(frames: usize) -> Arc<TrackBuffer> {
    let mut seed = 0x1234_5678_u32;
    let samples = (0..frames * 2)
        .map(|_| {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) as f32 / (1 << 23) as f32 - 1.0
        })
        .collect();
    Arc::new(TrackBuffer::new(SR, samples).unwrap())
}

fn rms_db(signal: &[f32]) -> f32 {
    let mean_square = signal.iter().map(|s| s * s).sum::<f32>() / signal.len() as f32;
    10.0 * mean_square.log10()
}

fn channel(c: &mut EngineController, cmd: ChannelCommand) {
    c.send(Command::Channel(DeckId::new(0).unwrap(), cmd))
        .unwrap();
}

/// An engine whose deck 1 reaches master at unity (thru, master level 1), with `cmds`
/// applied, playing `track`.
fn setup(track: Arc<TrackBuffer>, cmds: &[ChannelCommand]) -> (EngineController, EngineRenderer) {
    let (mut c, r) = engine(EngineConfig {
        sample_rate: SR,
        max_block_frames: BLOCK,
    });
    c.send(Command::Master(MasterCommand::MasterLevel(1.0)))
        .unwrap();
    channel(&mut c, ChannelCommand::XfaderAssign(XfaderAssign::Thru));
    for &cmd in cmds {
        channel(&mut c, cmd);
    }
    let id = DeckId::new(0).unwrap();
    c.send(Command::Deck(id, DeckCommand::Load(track))).unwrap();
    c.send(Command::Deck(id, DeckCommand::Play)).unwrap();
    (c, r)
}

/// Gain in dB the channel applies to a sine at `freq`, measured on the left master channel
/// after settling.
fn gain_db(freq: f32, cmds: &[ChannelCommand]) -> f32 {
    let (_c, mut r) = setup(sine(freq, SR, 100_000), cmds);
    render(&mut r, SETTLE_FRAMES);
    let (m, _) = render(&mut r, MEASURE_FRAMES);
    let left: Vec<f32> = m.iter().step_by(2).copied().collect();
    rms_db(&left) - 20.0 * (AMPLITUDE / 2.0_f32.sqrt()).log10()
}

#[test]
fn eq_gain_curve() {
    assert_eq!(eq_gain(0.0), 0.0);
    assert_eq!(eq_gain(0.25), 0.25);
    assert_eq!(eq_gain(0.5), 1.0);
    assert_abs_diff_eq!(eq_gain(1.0), 1.9953, epsilon = 1e-4);
}

#[test]
fn neutral_eq_is_flat() {
    let neutral = [
        ChannelCommand::EqLow(0.5),
        ChannelCommand::EqMid(0.5),
        ChannelCommand::EqHigh(0.5),
    ];
    for freq in [50.0, 1000.0, 10_000.0] {
        let db = gain_db(freq, &neutral);
        assert!(db.abs() <= 0.5, "{freq} Hz: {db:.3} dB");
    }
    // Including at crossovers, where the bands overlap, and just off neutral.
    let engaged = [ChannelCommand::EqLow(0.5001)];
    for freq in [50.0, 250.0, 1000.0, 2500.0, 10_000.0] {
        let db = gain_db(freq, &engaged);
        assert!(db.abs() <= 0.5, "engaged, {freq} Hz: {db:.3} dB");
    }
}

#[test]
fn low_kill_removes_bass() {
    let kill = [ChannelCommand::EqLow(0.0)];
    let low = gain_db(50.0, &kill);
    let high = gain_db(5000.0, &kill);
    assert!(low <= -30.0, "50 Hz: {low:.2} dB");
    assert!(high.abs() <= 1.0, "5 kHz: {high:.2} dB");
}

#[test]
fn high_kill_removes_treble() {
    let kill = [ChannelCommand::EqHigh(0.0)];
    let high = gain_db(10_000.0, &kill);
    let low = gain_db(100.0, &kill);
    assert!(high <= -30.0, "10 kHz: {high:.2} dB");
    assert!(low.abs() <= 1.0, "100 Hz: {low:.2} dB");
}

#[test]
fn mid_kill_removes_mids() {
    let mid = gain_db(1000.0, &[ChannelCommand::EqMid(0.0)]);
    assert!(mid <= -20.0, "1 kHz: {mid:.2} dB");
}

#[test]
fn eq_boost_reaches_plus_six_db() {
    let low = gain_db(50.0, &[ChannelCommand::EqLow(1.0)]);
    assert!((low - 6.0).abs() <= 0.5, "50 Hz: {low:.2} dB");
}

#[test]
fn filter_center_is_bypass() {
    for pos in [0.5, 0.51] {
        for freq in [60.0, 1000.0, 10_000.0] {
            let db = gain_db(freq, &[ChannelCommand::Filter(pos)]);
            assert!(db.abs() <= 0.1, "Filter({pos}) {freq} Hz: {db:.3} dB");
        }
    }
}

#[test]
fn filter_low_pass_cuts_highs() {
    let cmds = [ChannelCommand::Filter(0.1)];
    let high = gain_db(10_000.0, &cmds);
    let low = gain_db(60.0, &cmds);
    assert!(high <= -24.0, "10 kHz: {high:.2} dB");
    assert!(low.abs() <= 3.0, "60 Hz: {low:.2} dB");
}

#[test]
fn filter_high_pass_cuts_lows() {
    let cmds = [ChannelCommand::Filter(0.9)];
    let low = gain_db(60.0, &cmds);
    let high = gain_db(10_000.0, &cmds);
    assert!(low <= -24.0, "60 Hz: {low:.2} dB");
    assert!(high.abs() <= 3.0, "10 kHz: {high:.2} dB");
}

#[test]
fn sweeping_filter_never_produces_nan() {
    let (mut c, mut r) = setup(noise(200 * BLOCK), &[ChannelCommand::EqLow(0.8)]);
    for block in 0..200 {
        channel(&mut c, ChannelCommand::Filter(block as f32 / 199.0));
        let (m, h) = render(&mut r, BLOCK);
        assert!(
            m.iter().all(|s| s.is_finite() && s.abs() <= 1.0),
            "block {block}"
        );
        assert!(h.iter().all(|s| s.is_finite()), "block {block}");
    }
}

#[test]
fn non_finite_eq_filter_values_are_ignored() {
    let cmds = [
        ChannelCommand::EqLow(0.2),
        ChannelCommand::EqMid(0.7),
        ChannelCommand::EqHigh(0.4),
        ChannelCommand::Filter(0.3),
    ];
    let (_reference_c, mut reference) = setup(sine(440.0, SR, 100_000), &cmds);
    let (mut c, mut r) = setup(sine(440.0, SR, 100_000), &cmds);
    render(&mut reference, SETTLE_FRAMES);
    render(&mut r, SETTLE_FRAMES);

    for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        for cmd in [
            ChannelCommand::EqLow(value),
            ChannelCommand::EqMid(value),
            ChannelCommand::EqHigh(value),
            ChannelCommand::Filter(value),
        ] {
            channel(&mut c, cmd);
        }
    }
    let (expected, expected_h) = render(&mut reference, MEASURE_FRAMES);
    let (m, h) = render(&mut r, MEASURE_FRAMES);
    assert!(m.iter().chain(&h).all(|s| s.is_finite()));
    assert!(m.iter().any(|&s| s != 0.0));
    assert_eq!(m, expected);
    assert_eq!(h, expected_h);
}
