mod common;

use common::{ramp_track, render};
use engine::{
    Command, DeckCommand, DeckId, EngineConfig, EngineController, EngineRenderer, Grid, HotcueSlot,
    TrackMemory, engine,
};

const SR: u32 = 44_100;
const FRAMES: usize = 400_000;

fn grid(bpm: f64) -> Option<Grid> {
    Grid::from_bpm(bpm, 0.0, SR)
}

fn setup_with(grid: Option<Grid>) -> (EngineController, EngineRenderer, DeckId) {
    let (mut c, r) = engine(EngineConfig {
        sample_rate: SR,
        max_block_frames: 512,
    });
    let id = DeckId::new(0).unwrap();
    c.send(Command::Deck(
        id,
        DeckCommand::Load {
            track: ramp_track(SR, FRAMES),
            memory: TrackMemory {
                grid,
                ..TrackMemory::default()
            },
        },
    ))
    .unwrap();
    (c, r, id)
}

fn setup() -> (EngineController, EngineRenderer, DeckId) {
    setup_with(grid(120.0))
}

/// Sends `cmd` and renders one block so the renderer applies it and publishes state.
fn step(c: &mut EngineController, r: &mut EngineRenderer, id: DeckId, cmd: DeckCommand) {
    c.send(Command::Deck(id, cmd)).unwrap();
    render(r, 1);
}

#[test]
fn auto_loop_starts_on_the_last_beat() {
    let (mut c, mut r, id) = setup();
    step(&mut c, &mut r, id, DeckCommand::Seek(30_000.0));
    step(&mut c, &mut r, id, DeckCommand::AutoLoop);
    let s = c.deck(id);
    assert_eq!(s.loop_in, Some(22_050.0));
    assert_eq!(s.loop_out, Some(110_250.0));
    assert!(s.loop_active);
    assert_eq!(s.loop_beats, 4.0);
}

#[test]
fn loop_wraps_sample_exactly() {
    let (mut c, mut r, id) = setup();
    step(&mut c, &mut r, id, DeckCommand::Seek(22_050.0));
    step(&mut c, &mut r, id, DeckCommand::AutoLoop);
    step(&mut c, &mut r, id, DeckCommand::Seek(110_000.0));
    step(&mut c, &mut r, id, DeckCommand::Play);
    let start = c.deck(id).position_frames as usize;
    let (out, _) = render(&mut r, 1024);
    // The ramp track rises by one step per frame and drops by ~0.22 where the loop wraps.
    let left: Vec<f32> = out.chunks(2).map(|f| f[0]).collect();
    let drop = left
        .windows(2)
        .position(|w| w[1] < w[0] - 0.05)
        .expect("signal wraps");
    // The wrap happens when the playhead reaches 110_250 (give or take the block's start).
    assert!((start + drop + 1).abs_diff(110_250) < 600, "{start} {drop}");
    let s = c.deck(id);
    assert!((22_050.0..110_250.0).contains(&s.position_frames));
}

#[test]
fn auto_loop_without_grid_does_nothing() {
    let (mut c, mut r, id) = setup_with(None);
    step(&mut c, &mut r, id, DeckCommand::AutoLoop);
    let s = c.deck(id);
    assert!(!s.loop_active);
    assert_eq!((s.loop_in, s.loop_out), (None, None));
}

#[test]
fn auto_loop_again_exits_and_keeps_playing() {
    let (mut c, mut r, id) = setup();
    step(&mut c, &mut r, id, DeckCommand::Play);
    step(&mut c, &mut r, id, DeckCommand::AutoLoop);
    assert!(c.deck(id).loop_active);
    step(&mut c, &mut r, id, DeckCommand::AutoLoop);
    let s = c.deck(id);
    assert!(!s.loop_active);
    assert!(s.playing);
    assert!(s.loop_in.is_some() && s.loop_out.is_some());
}

#[test]
fn manual_loop_in_out_and_reloop() {
    let (mut c, mut r, id) = setup_with(None);
    step(&mut c, &mut r, id, DeckCommand::Seek(1000.0));
    step(&mut c, &mut r, id, DeckCommand::LoopIn);
    let s = c.deck(id);
    assert_eq!(
        (s.loop_in, s.loop_out, s.loop_active),
        (Some(1000.0), None, false)
    );
    step(&mut c, &mut r, id, DeckCommand::Seek(5000.0));
    step(&mut c, &mut r, id, DeckCommand::LoopOut);
    let s = c.deck(id);
    assert_eq!(
        (s.loop_in, s.loop_out, s.loop_active),
        (Some(1000.0), Some(5000.0), true)
    );
    step(&mut c, &mut r, id, DeckCommand::ReloopExit);
    assert!(!c.deck(id).loop_active);
    step(&mut c, &mut r, id, DeckCommand::Seek(9000.0));
    step(&mut c, &mut r, id, DeckCommand::ReloopExit);
    let s = c.deck(id);
    assert!(s.loop_active);
    assert_eq!(s.position_frames, 1000.0);
    // A new loop-in clears the old out and deactivates.
    step(&mut c, &mut r, id, DeckCommand::LoopIn);
    let s = c.deck(id);
    assert_eq!((s.loop_out, s.loop_active), (None, false));
}

#[test]
fn loop_out_before_in_is_ignored() {
    let (mut c, mut r, id) = setup_with(None);
    step(&mut c, &mut r, id, DeckCommand::LoopOut);
    assert!(!c.deck(id).loop_active);
    step(&mut c, &mut r, id, DeckCommand::Seek(5000.0));
    step(&mut c, &mut r, id, DeckCommand::LoopIn);
    step(&mut c, &mut r, id, DeckCommand::Seek(2000.0));
    step(&mut c, &mut r, id, DeckCommand::LoopOut);
    let s = c.deck(id);
    assert!(!s.loop_active);
    assert_eq!(s.loop_out, None);
}

#[test]
fn halve_and_double_clamp() {
    let (mut c, mut r, id) = setup();
    for _ in 0..10 {
        step(&mut c, &mut r, id, DeckCommand::LoopHalve);
    }
    assert_eq!(c.deck(id).loop_beats, 1.0 / 32.0);
    for _ in 0..12 {
        step(&mut c, &mut r, id, DeckCommand::LoopDouble);
    }
    assert_eq!(c.deck(id).loop_beats, 64.0);
    // On an active loop the out point follows, with the grid.
    step(&mut c, &mut r, id, DeckCommand::Seek(22_050.0));
    step(&mut c, &mut r, id, DeckCommand::AutoLoop);
    step(&mut c, &mut r, id, DeckCommand::LoopHalve);
    let s = c.deck(id);
    assert_eq!(s.loop_beats, 32.0);
    assert_eq!(s.loop_out, Some(22_050.0 + 32.0 * 22_050.0));
}

#[test]
fn halve_without_grid_halves_the_length() {
    let (mut c, mut r, id) = setup_with(None);
    step(&mut c, &mut r, id, DeckCommand::Seek(1000.0));
    step(&mut c, &mut r, id, DeckCommand::LoopIn);
    step(&mut c, &mut r, id, DeckCommand::Seek(5000.0));
    step(&mut c, &mut r, id, DeckCommand::LoopOut);
    step(&mut c, &mut r, id, DeckCommand::LoopHalve);
    assert_eq!(c.deck(id).loop_out, Some(3000.0));
    step(&mut c, &mut r, id, DeckCommand::LoopDouble);
    assert_eq!(c.deck(id).loop_out, Some(5000.0));
}

#[test]
fn tiny_loop_stays_inside() {
    let id = DeckId::new(0).unwrap();
    // 1/32 beat at 180 BPM is about 459 frames; render 8192 of them in one block.
    let (mut c2, mut r2) = engine(EngineConfig {
        sample_rate: SR,
        max_block_frames: 8192,
    });
    c2.send(Command::Deck(
        id,
        DeckCommand::Load {
            track: ramp_track(SR, FRAMES),
            memory: TrackMemory {
                grid: grid(180.0),
                ..TrackMemory::default()
            },
        },
    ))
    .unwrap();
    for cmd in [
        DeckCommand::Seek(10_000.0),
        DeckCommand::LoopHalve,
        DeckCommand::LoopHalve,
        DeckCommand::LoopHalve,
        DeckCommand::LoopHalve,
        DeckCommand::LoopHalve,
        DeckCommand::LoopHalve,
        DeckCommand::LoopHalve,
        DeckCommand::AutoLoop,
        DeckCommand::Play,
    ] {
        c2.send(Command::Deck(id, cmd)).unwrap();
    }
    render(&mut r2, 8192);
    let s = c2.deck(id);
    let (start, end) = (s.loop_in.unwrap(), s.loop_out.unwrap());
    assert!(s.loop_active);
    assert!((end - start - 44_100.0 / 3.0 / 32.0).abs() < 1e-6);
    assert!(
        (start..end).contains(&s.position_frames),
        "{start} {end} {}",
        s.position_frames
    );
}

#[test]
fn halving_past_the_playhead_wraps_inside() {
    let (mut c, mut r, id) = setup();
    step(&mut c, &mut r, id, DeckCommand::AutoLoop);
    step(&mut c, &mut r, id, DeckCommand::Seek(80_000.0));
    step(&mut c, &mut r, id, DeckCommand::Play);
    step(&mut c, &mut r, id, DeckCommand::LoopHalve);
    step(&mut c, &mut r, id, DeckCommand::LoopHalve);
    render(&mut r, 512);
    let s = c.deck(id);
    let (start, end) = (s.loop_in.unwrap(), s.loop_out.unwrap());
    assert_eq!(end, start + 22_050.0);
    assert!(
        (start..end).contains(&s.position_frames),
        "{}",
        s.position_frames
    );
}

#[test]
fn hotcue_outside_loop_exits_it() {
    let (mut c, mut r, id) = setup();
    let slot = |n| HotcueSlot::new(n).unwrap();
    step(
        &mut c,
        &mut r,
        id,
        DeckCommand::SetHotcue(slot(1), Some(50_000.0)),
    );
    step(
        &mut c,
        &mut r,
        id,
        DeckCommand::SetHotcue(slot(2), Some(200_000.0)),
    );
    step(&mut c, &mut r, id, DeckCommand::Seek(22_050.0));
    step(&mut c, &mut r, id, DeckCommand::AutoLoop);
    step(&mut c, &mut r, id, DeckCommand::HotcuePress(slot(1)));
    assert!(c.deck(id).loop_active, "jump inside keeps the loop");
    step(&mut c, &mut r, id, DeckCommand::HotcueRelease(slot(1)));
    step(&mut c, &mut r, id, DeckCommand::HotcuePress(slot(2)));
    assert!(!c.deck(id).loop_active);
}

#[test]
fn load_resets_loop_keeps_size() {
    let (mut c, mut r, id) = setup();
    step(&mut c, &mut r, id, DeckCommand::LoopHalve);
    step(&mut c, &mut r, id, DeckCommand::AutoLoop);
    step(
        &mut c,
        &mut r,
        id,
        DeckCommand::Load {
            track: ramp_track(SR, FRAMES),
            memory: TrackMemory::default(),
        },
    );
    let s = c.deck(id);
    assert_eq!((s.loop_in, s.loop_out, s.loop_active), (None, None, false));
    assert_eq!(s.loop_beats, 2.0);
}
