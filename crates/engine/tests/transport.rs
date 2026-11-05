mod common;

use std::sync::Arc;

use approx::{assert_abs_diff_eq, assert_relative_eq};
use common::{SETTLE_FRAMES, SETTLED_EPSILON, constant_track, ramp_track, render};
use engine::{
    ChannelCommand, Command, CommandQueueFull, DeckCommand, DeckId, EngineConfig, EngineController,
    EngineRenderer, Grid, HOTCUES, HotcueSlot, MasterCommand, TempoRange, TrackBuffer, TrackError,
    TrackMemory, XfaderAssign, engine,
};

const SR: u32 = 44_100;

/// An engine whose mixer passes every deck to master at unity gain: master level 1.0 and
/// every channel off the crossfader. The always-on EQ isolator still shifts phase (it is an
/// allpass at unity), so sample-exact transport output is pinned by the unit tests in
/// `src/deck/mod.rs`; these tests check the end-to-end path with settled DC levels.
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
    send(
        &mut c,
        deck(0),
        DeckCommand::Load {
            track: ramp_track(SR, 4096),
            memory: engine::TrackMemory::default(),
        },
    );
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
    send(
        &mut c,
        deck(0),
        DeckCommand::Load {
            track,
            memory: engine::TrackMemory::default(),
        },
    );
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
        DeckCommand::Load {
            track: ramp_track(48_000, 96_000),
            memory: engine::TrackMemory::default(),
        },
    );
    send(&mut c, deck(0), DeckCommand::Play);
    render(&mut r, 44_100);
    assert_abs_diff_eq!(c.deck(deck(0)).position_frames, 48_000.0, epsilon = 1.0);
}

#[test]
fn tempo_slider_scales_rate() {
    let (mut c, mut r) = setup(SR);
    send(
        &mut c,
        deck(0),
        DeckCommand::Load {
            track: ramp_track(SR, 4096),
            memory: engine::TrackMemory::default(),
        },
    );
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
    send(
        &mut c,
        deck(0),
        DeckCommand::Load {
            track: ramp_track(SR, 4096),
            memory: engine::TrackMemory::default(),
        },
    );
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
    send(
        &mut c,
        deck(0),
        DeckCommand::Load {
            track: track.clone(),
            memory: engine::TrackMemory::default(),
        },
    );
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
        DeckCommand::Load {
            track: constant_track(SR, 100, 0.5),
            memory: engine::TrackMemory::default(),
        },
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
    send(
        &mut c,
        deck(0),
        DeckCommand::Load {
            track: empty,
            memory: engine::TrackMemory::default(),
        },
    );
    send(&mut c, deck(0), DeckCommand::Play);
    let (master, headphones) = render(&mut r, 1024);
    assert_silent(&master);
    assert_silent(&headphones);
    assert!(c.deck(deck(0)).loaded);
}

#[test]
fn toggle_play_and_pause() {
    let (mut c, mut r) = setup(SR);
    send(
        &mut c,
        deck(0),
        DeckCommand::Load {
            track: ramp_track(SR, 4096),
            memory: engine::TrackMemory::default(),
        },
    );

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
    send(
        &mut c,
        deck(0),
        DeckCommand::Load {
            track: ramp_track(SR, 4096),
            memory: engine::TrackMemory::default(),
        },
    );

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
    send(
        &mut c,
        deck(0),
        DeckCommand::Load {
            track: ramp_track(SR, 4096),
            memory: engine::TrackMemory::default(),
        },
    );
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
    send(
        &mut c,
        deck(0),
        DeckCommand::Load {
            track: old.clone(),
            memory: engine::TrackMemory::default(),
        },
    );
    render(&mut r, 1);
    assert_eq!(Arc::strong_count(&old), 2);

    send(
        &mut c,
        deck(0),
        DeckCommand::Load {
            track: ramp_track(SR, 2048),
            memory: engine::TrackMemory::default(),
        },
    );
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
            DeckCommand::Load {
                track: constant_track(SR, 2 * SETTLE_FRAMES, 0.25),
                memory: engine::TrackMemory::default(),
            },
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
    send(
        &mut c,
        deck(0),
        DeckCommand::Load {
            track: bad,
            memory: engine::TrackMemory::default(),
        },
    );
    send(&mut c, deck(0), DeckCommand::Play);
    let (master, headphones) = render(&mut r, 2000);
    assert!(master.iter().chain(&headphones).all(|s| s.is_finite()));

    send(
        &mut c,
        deck(0),
        DeckCommand::Load {
            track: ramp_track(SR, 100_000),
            memory: engine::TrackMemory::default(),
        },
    );
    send(&mut c, deck(0), DeckCommand::Play);
    render(&mut r, SETTLE_FRAMES);
    let (master, _) = render(&mut r, 512);
    assert!(master.iter().all(|s| s.is_finite()));
    assert!(master.iter().any(|&s| s.abs() > 0.01));
}

fn grid_120_bpm() -> Grid {
    Grid::from_bpm(120.0, 1000.0, SR).unwrap()
}

#[test]
fn load_applies_track_memory() {
    let (mut c, mut r) = setup(SR);
    let mut hotcues = [None; HOTCUES];
    hotcues[HotcueSlot::new(3).unwrap().index()] = Some(900.0);
    let memory = TrackMemory {
        cue: 500.0,
        hotcues,
        grid: Some(grid_120_bpm()),
    };
    send(
        &mut c,
        deck(0),
        DeckCommand::Load {
            track: ramp_track(SR, 4096),
            memory,
        },
    );
    render(&mut r, 64);
    let s = c.deck(deck(0));
    assert_eq!(s.cue_frames, 500.0);
    assert_eq!(s.hotcues[2], Some(900.0));
    let grid = s.grid.unwrap();
    assert_eq!(grid.beat_period_frames, 22_050.0);
    assert_eq!(grid.first_beat_frames, 1000.0);
    assert!(!s.playing);
    assert_eq!(s.position_frames, 0.0);
}

#[test]
fn load_seq_counts_every_load() {
    let (mut c, mut r) = setup(SR);
    assert_eq!(c.deck(deck(0)).load_seq, 0);
    for expected in 1..=2 {
        send(
            &mut c,
            deck(0),
            DeckCommand::Load {
                track: ramp_track(SR, 4096),
                memory: TrackMemory::default(),
            },
        );
        render(&mut r, 64);
        assert_eq!(c.deck(deck(0)).load_seq, expected);
    }
    // Other commands and other decks leave it alone.
    send(&mut c, deck(0), DeckCommand::Play);
    render(&mut r, 64);
    assert_eq!(c.deck(deck(0)).load_seq, 2);
    assert_eq!(c.deck(deck(1)).load_seq, 0);
}

#[test]
fn load_clamps_memory_like_clamped() {
    let (mut c, mut r) = setup(SR);
    let memory = TrackMemory {
        cue: 5000.0,
        hotcues: [
            Some(-5.0),
            Some(f64::NAN),
            None,
            None,
            None,
            None,
            None,
            None,
        ],
        grid: Some(grid_120_bpm()),
    };
    send(
        &mut c,
        deck(0),
        DeckCommand::Load {
            track: ramp_track(SR, 1000),
            memory,
        },
    );
    render(&mut r, 64);
    let s = c.deck(deck(0));
    let expected = memory.clamped(1000);
    assert_eq!(expected.cue, 1000.0);
    assert_eq!(expected.hotcues[..2], [Some(0.0), None]);
    assert_eq!(s.cue_frames, expected.cue);
    assert_eq!(s.hotcues, expected.hotcues);
    assert_eq!(s.grid, Some(grid_120_bpm()));
    assert_eq!(s.grid, expected.grid);
}

#[test]
fn set_grid_replaces_and_clears() {
    let (mut c, mut r) = setup(SR);
    send(
        &mut c,
        deck(0),
        DeckCommand::Load {
            track: ramp_track(SR, 4096),
            memory: TrackMemory::default(),
        },
    );
    let g = grid_120_bpm();
    send(&mut c, deck(0), DeckCommand::SetGrid(Some(g)));
    render(&mut r, 64);
    assert_eq!(c.deck(deck(0)).grid, Some(g));

    let zero = Grid {
        beat_period_frames: 0.0,
        first_beat_frames: 0.0,
    };
    send(&mut c, deck(0), DeckCommand::SetGrid(Some(zero)));
    render(&mut r, 64);
    assert_eq!(c.deck(deck(0)).grid, Some(g), "an invalid grid is ignored");

    send(&mut c, deck(0), DeckCommand::SetGrid(None));
    render(&mut r, 64);
    assert_eq!(c.deck(deck(0)).grid, None);
}

#[test]
fn set_grid_ignores_a_nan_first_beat() {
    let (mut c, mut r) = setup(SR);
    send(
        &mut c,
        deck(0),
        DeckCommand::Load {
            track: ramp_track(SR, 4096),
            memory: TrackMemory::default(),
        },
    );
    let g = grid_120_bpm();
    send(&mut c, deck(0), DeckCommand::SetGrid(Some(g)));
    let nan = Grid {
        beat_period_frames: 22_050.0,
        first_beat_frames: f64::NAN,
    };
    send(&mut c, deck(0), DeckCommand::SetGrid(Some(nan)));
    render(&mut r, 64);
    assert_eq!(c.deck(deck(0)).grid, Some(g));
}

#[test]
fn send_all_is_all_or_nothing() {
    let (mut c, mut r) = engine(EngineConfig {
        sample_rate: SR,
        max_block_frames: 512,
    });
    let cmd = Command::Master(MasterCommand::MasterLevel(1.0));
    for _ in 0..254 {
        c.send(cmd.clone()).unwrap();
    }
    let three = [cmd.clone(), cmd.clone(), cmd];
    assert_eq!(c.send_all(&three), Err(CommandQueueFull));
    // The failed call queued nothing: exactly the 2 free slots remain.
    for _ in 0..2 {
        c.send(cmd_one()).unwrap();
    }
    assert_eq!(c.send(cmd_one()), Err(CommandQueueFull));
    render(&mut r, 1);
    assert_eq!(c.send_all(&three), Ok(()));
}

fn cmd_one() -> Command {
    Command::Master(MasterCommand::MasterLevel(1.0))
}

#[test]
fn load_drops_an_invalid_grid() {
    let (mut c, mut r) = setup(SR);
    for (period, first) in [(0.0, 0.0), (-1.0, 0.0), (f64::NAN, 0.0), (100.0, f64::NAN)] {
        let memory = TrackMemory {
            grid: Some(Grid {
                beat_period_frames: period,
                first_beat_frames: first,
            }),
            ..TrackMemory::default()
        };
        assert_eq!(memory.clamped(1000).grid, None);
        send(
            &mut c,
            deck(0),
            DeckCommand::Load {
                track: ramp_track(SR, 1000),
                memory,
            },
        );
        render(&mut r, 64);
        assert_eq!(c.deck(deck(0)).grid, None);
    }
}

#[test]
fn grid_from_bpm_validates() {
    for bpm in [0.0, -120.0, f64::NAN, f64::INFINITY] {
        assert_eq!(Grid::from_bpm(bpm, 0.0, SR), None);
    }
    assert_eq!(Grid::from_bpm(120.0, f64::NAN, SR), None);
    assert_eq!(Grid::from_bpm(120.0, 0.0, 0), None);
    let g = grid_120_bpm();
    assert_eq!(g.beats_at(1000.0 + 2.0 * 22_050.0), 2.0);
}
