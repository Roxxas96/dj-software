mod common;

use std::sync::Arc;

use approx::{assert_abs_diff_eq, assert_relative_eq};
use common::{constant_track, ramp_track, render};
use engine::{
    Command, DeckCommand, DeckId, EngineConfig, EngineController, EngineRenderer, TempoRange,
    TrackBuffer, TrackError, engine,
};

const SR: u32 = 44_100;

fn setup(sample_rate: u32) -> (EngineController, EngineRenderer) {
    engine(EngineConfig {
        sample_rate,
        max_block_frames: 512,
    })
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
fn play_outputs_track_samples_exactly() {
    let (mut c, mut r) = setup(SR);
    let track = ramp_track(SR, 4096);
    send(&mut c, deck(0), DeckCommand::Load(track.clone()));
    send(&mut c, deck(0), DeckCommand::SetTempoSlider(0.5));
    send(&mut c, deck(0), DeckCommand::Play);
    let (master, headphones) = render(&mut r, 1000);
    for i in 0..1000 {
        assert_eq!(master[2 * i], track.frame(i)[0]);
        assert_eq!(master[2 * i + 1], track.frame(i)[1]);
    }
    assert_silent(&headphones);
    let snap = c.deck(deck(0));
    assert!(snap.playing);
    assert_eq!(snap.position_frames, 1000.0);
    assert_eq!(snap.duration_frames, 4096);
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
    let (master, _) = render(&mut r, 4);
    assert_relative_eq!(c.deck(deck(0)).tempo_ratio, 0.5);
    let (f0, f1) = (track.frame(0)[0], track.frame(1)[0]);
    assert_relative_eq!(master[2], (f0 + f1) / 2.0);
    assert_relative_eq!(master[3], (f0 + f1) / 2.0);
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
    assert!(master[..200].iter().all(|&s| s == 0.5));
    assert_silent(&master[200..]);
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
            DeckCommand::Load(constant_track(SR, 4096, 0.25)),
        );
        send(&mut c, id, DeckCommand::Play);
    }
    let (master, headphones) = render(&mut r, 1024);
    for &s in &master {
        assert_relative_eq!(s, 0.5);
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
