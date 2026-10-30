mod common;

use std::sync::Arc;

use approx::{assert_abs_diff_eq, assert_relative_eq};
use common::{SETTLE_FRAMES, SETTLED_EPSILON, constant_track, ramp_track, render};
use engine::{
    ChannelCommand, Command, DeckCommand, DeckId, EngineConfig, EngineController, EngineRenderer,
    MasterCommand, TempoRange, TrackBuffer, TrackError, XfaderAssign, engine,
};

const SR: u32 = 44_100;

/// An engine whose mixer passes every deck to master at unity gain: master level 1.0 and
/// every channel off the crossfader. The always-on EQ isolator still shifts phase (it is an
/// allpass at unity), so sample-exact transport output is pinned by the unit tests in
/// `src/deck.rs`; these tests check the end-to-end path with settled DC levels.
fn setup(sample_rate: u32) -> (EngineController, EngineRenderer) {
    let (mut c, mut r) = engine(EngineConfig {
        sample_rate,
        max_block_frames: 512,
    });
    c.send(Command::Master(MasterCommand::MasterLevel(1.0)))
        .unwrap();
    for id in DeckId::ALL {
        c.send(Command::Channel(
            id,
            ChannelCommand::XfaderAssign(XfaderAssign::Thru),
        ))
        .unwrap();
    }
    // One frame with nothing loaded applies the changes and finishes their gain ramps.
    render(&mut r, 1);
    (c, r)
}

fn deck(index: usize) -> DeckId {
    DeckId::new(index).unwrap()
}

fn send(c: &mut EngineController, id: DeckId, cmd: DeckCommand) {
    c.send(Command::Deck(id, cmd)).unwrap();
}

fn assert_silent(samples: &[f32]) {
    assert!(samples.iter().all(|&s| s == 0.0));
}

#[test]
fn silent_when_nothing_loaded() {
    let (_c, mut r) = setup(SR);
    let (master, headphones) = render(&mut r, 2048);
    assert_silent(&master);
    assert_silent(&headphones);
}

#[test]
fn loaded_but_paused_is_silent() {
    let (mut c, mut r) = setup(SR);
    send(&mut c, deck(0), DeckCommand::Load(ramp_track(SR, 4096)));
    let (master, headphones) = render(&mut r, 2048);
    assert_silent(&master);
    assert_silent(&headphones);
    let snap = c.deck(deck(0));
    assert!(snap.loaded);
    assert!(!snap.playing);
    assert_eq!(snap.position_frames, 0.0);
}

#[test]
fn play_outputs_track_at_unity() {
    let (mut c, mut r) = setup(SR);
    // Left and right differ, so a channel swap or mono sum would fail.
    let samples = [0.3, -0.2].repeat(2 * SETTLE_FRAMES);
    let track = Arc::new(TrackBuffer::new(SR, samples).unwrap());
    send(&mut c, deck(0), DeckCommand::Load(track));
    send(&mut c, deck(0), DeckCommand::SetTempoSlider(0.5));
    send(&mut c, deck(0), DeckCommand::Play);
    let (_, headphones) = render(&mut r, SETTLE_FRAMES);
    assert_silent(&headphones);
    let (master, headphones) = render(&mut r, 1000);
    for frame in master.chunks(2) {
        assert_abs_diff_eq!(frame[0], 0.3, epsilon = SETTLED_EPSILON);
        assert_abs_diff_eq!(frame[1], -0.2, epsilon = SETTLED_EPSILON);
    }
    assert_silent(&headphones);
    let snap = c.deck(deck(0));
    assert!(snap.playing);
    assert_eq!(snap.position_frames, (SETTLE_FRAMES + 1000) as f64);
    assert_eq!(snap.duration_frames, 2 * SETTLE_FRAMES);
    assert_eq!(snap.track_sample_rate, SR);
}

#[test]
fn resamples_track_rate_to_output_rate() {
    let (mut c, mut r) = setup(44_100);
    send(
        &mut c,
        deck(0),
        DeckCommand::Load(ramp_track(48_000, 96_000)),
    );
    send(&mut c, deck(0), DeckCommand::Play);
    render(&mut r, 44_100);
    assert_abs_diff_eq!(c.deck(deck(0)).position_frames, 48_000.0, epsilon = 1.0);
}

#[test]
fn tempo_slider_scales_rate() {
    let (mut c, mut r) = setup(SR);
    send(&mut c, deck(0), DeckCommand::Load(ramp_track(SR, 4096)));
    send(
        &mut c,
        deck(0),
        DeckCommand::SetTempoRange(TempoRange::Sixteen),
    );
    send(&mut c, deck(0), DeckCommand::SetTempoSlider(1.0));
    send(&mut c, deck(0), DeckCommand::Play);
    render(&mut r, 1000);
    let snap = c.deck(deck(0));
    assert_relative_eq!(snap.tempo_ratio, 1.16, epsilon = 1e-12);
    assert_abs_diff_eq!(snap.position_frames, 1160.0, epsilon = 1.0);
}

#[test]
fn tempo_slider_minus_hundred_stops_advancing() {
    let (mut c, mut r) = setup(SR);
    send(&mut c, deck(0), DeckCommand::Load(ramp_track(SR, 4096)));
    send(&mut c, deck(0), DeckCommand::Seek(100.0));
    send(
        &mut c,
        deck(0),
        DeckCommand::SetTempoRange(TempoRange::Hundred),
    );
    send(&mut c, deck(0), DeckCommand::SetTempoSlider(0.0));
    send(&mut c, deck(0), DeckCommand::Play);
    render(&mut r, 1000);
    let (master, _) = render(&mut r, SETTLE_FRAMES);
    assert!(master.iter().all(|&s| s.abs() < SETTLED_EPSILON));
    let snap = c.deck(deck(0));
    assert_eq!(snap.tempo_ratio, 0.0);
    assert_eq!(snap.position_frames, 100.0);
    assert!(snap.playing);
}

#[test]
fn interpolates_between_frames() {
    let (mut c, mut r) = setup(SR);
    let track = ramp_track(SR, 4096);
    send(&mut c, deck(0), DeckCommand::Load(track.clone()));
    send(
        &mut c,
        deck(0),
        DeckCommand::SetTempoRange(TempoRange::Hundred),
    );
    send(&mut c, deck(0), DeckCommand::SetTempoSlider(0.25));
    send(&mut c, deck(0), DeckCommand::Play);
    // Sample values are pinned by `deck::tests::interpolates_between_frames`; end to end,
    // half speed advances half a frame per output frame.
    let (master, _) = render(&mut r, 5);
    let snap = c.deck(deck(0));
    assert_relative_eq!(snap.tempo_ratio, 0.5);
    assert_eq!(snap.position_frames, 2.5);
    assert!(master.iter().all(|s| s.is_finite()));
    assert_eq!(track.frames(), 4096);
}

#[test]
fn stops_at_end_of_track() {
    let (mut c, mut r) = setup(SR);
    send(
        &mut c,
        deck(0),
        DeckCommand::Load(constant_track(SR, 100, 0.5)),
    );
    send(&mut c, deck(0), DeckCommand::Play);
    let (master, _) = render(&mut r, 200);
    let snap = c.deck(deck(0));
    assert!(!snap.playing);
    assert_eq!(snap.position_frames, 100.0);
    // Exact samples are pinned by `deck::tests::stops_at_end_of_track`. End to end, the
    // track is heard, then only the EQ's decaying tail, which settles to true silence.
    assert!(master[..200].iter().any(|&s| s > 0.25));
    assert!(master[200..].iter().all(|s| s.is_finite()));
    render(&mut r, SETTLE_FRAMES);
    let (master, _) = render(&mut r, 1024);
    assert_silent(&master);
}

#[test]
fn empty_track_renders_silence() {
    let (mut c, mut r) = setup(SR);
    let empty = Arc::new(TrackBuffer::new(SR, vec![]).unwrap());
    send(&mut c, deck(0), DeckCommand::Load(empty));
    send(&mut c, deck(0), DeckCommand::Play);
    let (master, headphones) = render(&mut r, 1024);
    assert_silent(&master);
    assert_silent(&headphones);
    assert!(c.deck(deck(0)).loaded);
}

#[test]
fn toggle_play_and_pause() {
    let (mut c, mut r) = setup(SR);
    send(&mut c, deck(0), DeckCommand::Load(ramp_track(SR, 4096)));

    send(&mut c, deck(0), DeckCommand::TogglePlay);
    render(&mut r, 100);
    assert!(c.deck(deck(0)).playing);

    send(&mut c, deck(0), DeckCommand::TogglePlay);
    render(&mut r, 100);
    let snap = c.deck(deck(0));
    assert!(!snap.playing);
    assert_eq!(snap.position_frames, 100.0);

    send(&mut c, deck(0), DeckCommand::Play);
    render(&mut r, 100);
    assert!(c.deck(deck(0)).playing);

    send(&mut c, deck(0), DeckCommand::Pause);
    render(&mut r, 100);
    let snap = c.deck(deck(0));
    assert!(!snap.playing);
    assert_eq!(snap.position_frames, 200.0);
}

#[test]
fn seek_clamps_to_track() {
    let (mut c, mut r) = setup(SR);
    send(&mut c, deck(0), DeckCommand::Load(ramp_track(SR, 4096)));

    send(&mut c, deck(0), DeckCommand::Seek(-5.0));
    render(&mut r, 1);
    assert_eq!(c.deck(deck(0)).position_frames, 0.0);

    send(&mut c, deck(0), DeckCommand::Seek(1e9));
    render(&mut r, 1);
    assert_eq!(c.deck(deck(0)).position_frames, 4096.0);
}

#[test]
fn seek_nan_is_ignored() {
    let (mut c, mut r) = setup(SR);
    send(&mut c, deck(0), DeckCommand::Load(ramp_track(SR, 4096)));
    send(&mut c, deck(0), DeckCommand::Seek(500.0));
    render(&mut r, 1);
    send(&mut c, deck(0), DeckCommand::Seek(f64::NAN));
    send(&mut c, deck(0), DeckCommand::Play);
    render(&mut r, 1);
    assert!(c.deck(deck(0)).position_frames < 505.0);
    assert!(c.deck(deck(0)).position_frames >= 500.0);
    let (master, _) = render(&mut r, 10);
    assert!(master.iter().all(|s| s.is_finite()));
}

#[test]
fn tempo_slider_nan_is_ignored() {
    let (mut c, mut r) = setup(SR);
    send(&mut c, deck(0), DeckCommand::SetTempoSlider(f32::NAN));
    render(&mut r, 1);
    assert_relative_eq!(c.deck(deck(0)).tempo_ratio, 1.0);
}

#[test]
fn load_replaces_track_and_old_one_is_returned_as_garbage() {
    let (mut c, mut r) = setup(SR);
    let old = ramp_track(SR, 4096);
    send(&mut c, deck(0), DeckCommand::Load(old.clone()));
    render(&mut r, 1);
    assert_eq!(Arc::strong_count(&old), 2);

    send(&mut c, deck(0), DeckCommand::Load(ramp_track(SR, 2048)));
    render(&mut r, 1);
    assert_eq!(Arc::strong_count(&old), 2);
    assert_eq!(c.deck(deck(0)).duration_frames, 2048);

    c.collect_garbage();
    assert_eq!(Arc::strong_count(&old), 1);
}

#[test]
fn four_decks_sum_into_master() {
    let (mut c, mut r) = setup(SR);
    for id in [deck(0), deck(2)] {
        send(
            &mut c,
            id,
            DeckCommand::Load(constant_track(SR, 2 * SETTLE_FRAMES, 0.25)),
        );
        send(&mut c, id, DeckCommand::Play);
    }
    let (_, headphones) = render(&mut r, SETTLE_FRAMES);
    assert_silent(&headphones);
    let (master, headphones) = render(&mut r, 1024);
    for &s in &master {
        assert_abs_diff_eq!(s, 0.5, epsilon = SETTLED_EPSILON);
    }
    assert_silent(&headphones);
}

#[test]
fn track_buffer_rejects_odd_length_and_zero_rate() {
    assert!(matches!(
        TrackBuffer::new(SR, vec![0.0; 3]),
        Err(TrackError::OddSampleCount(3))
    ));
    assert!(matches!(
        TrackBuffer::new(0, vec![0.0; 4]),
        Err(TrackError::ZeroSampleRate)
    ));
    let track = TrackBuffer::new(SR, vec![0.1, 0.2, 0.3, 0.4]).unwrap();
    assert_eq!(track.frames(), 2);
    assert_eq!(track.sample_rate(), SR);
    assert_eq!(track.frame(1), [0.3, 0.4]);
}

#[test]
fn non_finite_track_samples_are_silenced() {
    let (mut c, mut r) = setup(SR);
    let mut samples = vec![0.5; 4000];
    samples[10] = f32::NAN;
    samples[11] = f32::INFINITY;
    samples[20] = f32::NEG_INFINITY;
    let bad = std::sync::Arc::new(engine::TrackBuffer::new(SR, samples).unwrap());
    send(&mut c, deck(0), DeckCommand::Load(bad));
    send(&mut c, deck(0), DeckCommand::Play);
    let (master, headphones) = render(&mut r, 2000);
    assert!(master.iter().chain(&headphones).all(|s| s.is_finite()));

    send(&mut c, deck(0), DeckCommand::Load(ramp_track(SR, 100_000)));
    send(&mut c, deck(0), DeckCommand::Play);
    render(&mut r, SETTLE_FRAMES);
    let (master, _) = render(&mut r, 512);
    assert!(master.iter().all(|s| s.is_finite()));
    assert!(master.iter().any(|&s| s.abs() > 0.01));
}
