mod common;

use std::f32::consts::FRAC_1_SQRT_2;

use approx::assert_abs_diff_eq;
use common::{constant_track, render};
use engine::mixer::{crossfader_gains, fader_gain, trim_gain};
use engine::{
    ChannelCommand, Command, DeckCommand, DeckId, EngineConfig, EngineController, EngineRenderer,
    MasterCommand, XfaderAssign, engine,
};

const SR: u32 = 44_100;
const BLOCK: usize = 512;
const SIGNAL: f32 = 0.5;

fn setup() -> (EngineController, EngineRenderer) {
    engine(EngineConfig {
        sample_rate: SR,
        max_block_frames: BLOCK,
    })
}

fn deck(index: usize) -> DeckId {
    DeckId::new(index).unwrap()
}

fn channel(c: &mut EngineController, index: usize, cmd: ChannelCommand) {
    c.send(Command::Channel(deck(index), cmd)).unwrap();
}

fn master(c: &mut EngineController, cmd: MasterCommand) {
    c.send(Command::Master(cmd)).unwrap();
}

/// Loads a constant `SIGNAL` track on deck `index` and starts it.
fn play(c: &mut EngineController, index: usize) {
    let id = deck(index);
    c.send(Command::Deck(
        id,
        DeckCommand::Load(constant_track(SR, 100_000, SIGNAL)),
    ))
    .unwrap();
    c.send(Command::Deck(id, DeckCommand::Play)).unwrap();
}

/// Renders one block so queued mixer commands finish ramping before any deck plays.
fn settle(r: &mut EngineRenderer) {
    render(r, BLOCK);
}

fn assert_all_near(samples: &[f32], expected: f32) {
    for &s in samples {
        assert_abs_diff_eq!(s, expected, epsilon = 1e-5);
    }
}

#[test]
fn curves_have_expected_values() {
    assert_eq!(trim_gain(0.5), 1.0);
    assert_abs_diff_eq!(trim_gain(1.0), 3.981, epsilon = 1e-3);
    assert_abs_diff_eq!(trim_gain(0.0), 0.2512, epsilon = 1e-4);
    assert_eq!(fader_gain(0.5), 0.25);
    assert_eq!(crossfader_gains(0.0), (1.0, 0.0));
    let (a, b) = crossfader_gains(0.5);
    assert_abs_diff_eq!(a, FRAC_1_SQRT_2, epsilon = 1e-6);
    assert_abs_diff_eq!(b, FRAC_1_SQRT_2, epsilon = 1e-6);
    let (a, b) = crossfader_gains(1.0);
    assert_abs_diff_eq!(a, 0.0, epsilon = 1e-6);
    assert_abs_diff_eq!(b, 1.0, epsilon = 1e-6);
}

#[test]
fn fader_zero_mutes_channel_in_master() {
    let (mut c, mut r) = setup();
    channel(&mut c, 0, ChannelCommand::Fader(0.0));
    settle(&mut r);
    play(&mut c, 0);
    let (m, _) = render(&mut r, 1024);
    assert!(m.iter().all(|&s| s == 0.0));
}

#[test]
fn crossfader_full_a_mutes_b_channels_but_not_thru() {
    let (mut c, mut r) = setup();
    master(&mut c, MasterCommand::MasterLevel(1.0));
    master(&mut c, MasterCommand::Crossfader(0.0));
    // Deck 2 (index 1) defaults to B, deck 4 (index 3) is switched to thru.
    channel(&mut c, 3, ChannelCommand::XfaderAssign(XfaderAssign::Thru));
    settle(&mut r);

    play(&mut c, 1);
    let (m, _) = render(&mut r, 1024);
    assert!(m.iter().all(|&s| s == 0.0), "B channel must be muted");

    c.send(Command::Deck(deck(1), DeckCommand::Pause)).unwrap();
    play(&mut c, 3);
    let (m, _) = render(&mut r, 1024);
    assert_all_near(&m, SIGNAL);
}

#[test]
fn pfl_sends_prefader_signal_to_headphones() {
    let (mut c, mut r) = setup();
    channel(&mut c, 0, ChannelCommand::Fader(0.0));
    channel(&mut c, 0, ChannelCommand::Pfl(true));
    master(&mut c, MasterCommand::HeadphoneMix(0.0));
    settle(&mut r);
    play(&mut c, 0);
    let (m, h) = render(&mut r, 1024);
    assert!(m.iter().all(|&s| s == 0.0));
    assert_all_near(&h, SIGNAL * 0.64);
}

#[test]
fn headphone_mix_one_is_master() {
    let (mut c, mut r) = setup();
    master(&mut c, MasterCommand::HeadphoneMix(1.0));
    channel(&mut c, 1, ChannelCommand::Pfl(true));
    settle(&mut r);
    play(&mut c, 0);
    play(&mut c, 1);
    let (m, h) = render(&mut r, 1024);
    // Two decks at the crossfader center, master level 0.64: well below clipping.
    assert_all_near(&m, 2.0 * SIGNAL * FRAC_1_SQRT_2 * 0.64);
    for (&ms, &hs) in m.iter().zip(&h) {
        assert_abs_diff_eq!(hs, ms * 0.64, epsilon = 1e-5);
    }
}

#[test]
fn headphones_silent_without_pfl_at_mix_zero() {
    let (mut c, mut r) = setup();
    for index in 0..4 {
        play(&mut c, index);
    }
    let (m, h) = render(&mut r, 1024);
    assert!(m.iter().any(|&s| s != 0.0));
    assert!(h.iter().all(|&s| s == 0.0));
}

#[test]
fn master_output_is_clamped() {
    let (mut c, mut r) = setup();
    for index in 0..4 {
        channel(&mut c, index, ChannelCommand::Trim(1.0));
        channel(&mut c, index, ChannelCommand::Pfl(true));
        c.send(Command::Deck(
            deck(index),
            DeckCommand::Load(constant_track(SR, 100_000, 0.9)),
        ))
        .unwrap();
        c.send(Command::Deck(deck(index), DeckCommand::Play))
            .unwrap();
    }
    master(&mut c, MasterCommand::HeadphoneLevel(1.0));
    let (m, h) = render(&mut r, 2048);
    assert!(m.iter().all(|s| s.abs() <= 1.0));
    assert!(h.iter().all(|s| s.abs() <= 1.0));
    assert_eq!(m[m.len() - 1], 1.0);
}

#[test]
fn gain_changes_are_ramped() {
    let (mut c, mut r) = setup();
    master(&mut c, MasterCommand::MasterLevel(1.0));
    channel(&mut c, 0, ChannelCommand::XfaderAssign(XfaderAssign::Thru));
    settle(&mut r);
    play(&mut c, 0);
    let (m, _) = render(&mut r, BLOCK);
    assert_all_near(&m, SIGNAL);

    channel(&mut c, 0, ChannelCommand::Fader(0.0));
    let (m, _) = render(&mut r, BLOCK);
    let left: Vec<f32> = m.iter().step_by(2).copied().collect();
    assert_abs_diff_eq!(left[0], SIGNAL, epsilon = 0.01);
    assert_abs_diff_eq!(left[BLOCK - 1], 0.0, epsilon = 1e-6);
    assert!(left.windows(2).all(|w| w[1] <= w[0]), "ramp is monotonic");
    assert!(left[BLOCK / 2] > 0.1 && left[BLOCK / 2] < 0.4);
}

#[test]
fn meters_report_channel_and_master_peaks() {
    let (mut c, mut r) = setup();
    master(&mut c, MasterCommand::MasterLevel(1.0));
    channel(&mut c, 0, ChannelCommand::XfaderAssign(XfaderAssign::Thru));
    channel(&mut c, 0, ChannelCommand::Fader(0.5));
    settle(&mut r);
    assert_eq!(c.meters().channel_peak, [0.0; 4]);
    play(&mut c, 0);
    render(&mut r, BLOCK);
    let meters = c.meters();
    assert_abs_diff_eq!(meters.channel_peak[0], SIGNAL * 0.25, epsilon = 1e-6);
    assert_eq!(meters.channel_peak[1..], [0.0; 3]);
    assert_abs_diff_eq!(meters.master_peak[0], SIGNAL * 0.25, epsilon = 1e-6);
    assert_abs_diff_eq!(meters.master_peak[1], SIGNAL * 0.25, epsilon = 1e-6);
}

#[test]
fn non_finite_mixer_values_are_ignored() {
    let (mut c, mut r) = setup();
    channel(&mut c, 0, ChannelCommand::Pfl(true));
    settle(&mut r);
    play(&mut c, 0);
    let (m_before, h_before) = render(&mut r, BLOCK);

    for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        for cmd in [
            ChannelCommand::Trim(value),
            ChannelCommand::EqLow(value),
            ChannelCommand::EqMid(value),
            ChannelCommand::EqHigh(value),
            ChannelCommand::Filter(value),
            ChannelCommand::Fader(value),
        ] {
            channel(&mut c, 0, cmd);
        }
        for cmd in [
            MasterCommand::Crossfader(value),
            MasterCommand::MasterLevel(value),
            MasterCommand::HeadphoneLevel(value),
            MasterCommand::HeadphoneMix(value),
        ] {
            master(&mut c, cmd);
        }
    }
    let (m, h) = render(&mut r, BLOCK);
    assert!(m.iter().chain(&h).all(|s| s.is_finite()));
    assert_eq!(m, m_before);
    assert_eq!(h, h_before);
}
