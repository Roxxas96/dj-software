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
    // The mixer's EQ smears sample values, so sample-exact wrapping is covered by the deck
    // unit tests; here the playhead after a block that crosses loop-out is exact.
    let (mut c, mut r, id) = setup();
    step(&mut c, &mut r, id, DeckCommand::Seek(22_050.0));
    step(&mut c, &mut r, id, DeckCommand::AutoLoop);
    step(&mut c, &mut r, id, DeckCommand::Seek(110_000.0));
    step(&mut c, &mut r, id, DeckCommand::Play);
    let start = c.deck(id).position_frames;
    render(&mut r, 1024);
    let expected = 22_050.0 + (start + 1024.0 - 22_050.0) % 88_200.0;
    assert_eq!(c.deck(id).position_frames, expected);
}

#[test]
fn seek_outside_loop_exits_it() {
    let (mut c, mut r, id) = setup();
    step(&mut c, &mut r, id, DeckCommand::AutoLoop);
    step(&mut c, &mut r, id, DeckCommand::Seek(200_000.0));
    let s = c.deck(id);
    assert!(!s.loop_active);
    assert_eq!(s.position_frames, 200_000.0);
    assert!(s.loop_in.is_some() && s.loop_out.is_some());
}

#[test]
fn seek_inside_loop_keeps_it() {
    let (mut c, mut r, id) = setup();
    step(&mut c, &mut r, id, DeckCommand::AutoLoop);
    step(&mut c, &mut r, id, DeckCommand::Seek(50_000.0));
    assert!(c.deck(id).loop_active);
}

#[test]
fn cue_return_outside_loop_exits_it() {
    let (mut c, mut r, id) = setup();
    step(&mut c, &mut r, id, DeckCommand::SetCue(300_000.0));
    step(&mut c, &mut r, id, DeckCommand::AutoLoop);
    step(&mut c, &mut r, id, DeckCommand::Play);
    step(&mut c, &mut r, id, DeckCommand::CuePress);
    let s = c.deck(id);
    assert!(!s.loop_active);
    assert!(!s.playing);
    assert_eq!(s.position_frames, 300_000.0);
}

#[test]
fn loop_overhanging_the_end_wraps_at_the_end() {
    let (mut c, mut r, id) = setup();
    step(&mut c, &mut r, id, DeckCommand::Seek(390_000.0));
    step(&mut c, &mut r, id, DeckCommand::AutoLoop);
    let s = c.deck(id);
    assert_eq!(s.loop_in, Some(374_850.0));
    assert!(s.loop_out.unwrap() > FRAMES as f64);
    step(&mut c, &mut r, id, DeckCommand::Play);
    render(&mut r, 20_000);
    let s = c.deck(id);
    assert!(s.playing && s.loop_active);
    assert!((374_850.0..FRAMES as f64).contains(&s.position_frames));
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
fn resizing_an_active_loop_stops_at_the_beat_limits() {
    // 120 BPM: a beat is 22,050 frames. Ten halves of a 4-beat loop stop at 1/32 beat.
    let (mut c, mut r, id) = setup();
    step(&mut c, &mut r, id, DeckCommand::Seek(22_050.0));
    step(&mut c, &mut r, id, DeckCommand::AutoLoop);
    for _ in 0..10 {
        step(&mut c, &mut r, id, DeckCommand::LoopHalve);
    }
    let s = c.deck(id);
    assert_eq!(s.loop_beats, 1.0 / 32.0);
    assert_eq!(s.loop_out, Some(22_050.0 + 22_050.0 / 32.0));
    // Doubling stops at 64 beats.
    for _ in 0..20 {
        step(&mut c, &mut r, id, DeckCommand::LoopDouble);
    }
    let s = c.deck(id);
    assert_eq!(s.loop_beats, 64.0);
    assert_eq!(s.loop_out, Some(22_050.0 + 64.0 * 22_050.0));
}

#[test]
fn resizing_without_grid_stops_with_the_loop_beats() {
    // A 4,000-frame manual loop, no grid: the length follows loop_beats (4 at first) until
    // it stops changing at 1/32 (seven halves) and at 64 (eleven doubles from there).
    let (mut c, mut r, id) = setup_with(None);
    step(&mut c, &mut r, id, DeckCommand::Seek(1000.0));
    step(&mut c, &mut r, id, DeckCommand::LoopIn);
    step(&mut c, &mut r, id, DeckCommand::Seek(5000.0));
    step(&mut c, &mut r, id, DeckCommand::LoopOut);
    for _ in 0..10 {
        step(&mut c, &mut r, id, DeckCommand::LoopHalve);
    }
    assert_eq!(c.deck(id).loop_out, Some(1000.0 + 4000.0 / 128.0));
    for _ in 0..20 {
        step(&mut c, &mut r, id, DeckCommand::LoopDouble);
    }
    assert_eq!(c.deck(id).loop_out, Some(1000.0 + 4000.0 / 128.0 * 2048.0));
}

#[test]
fn halving_a_manual_loop_on_a_gridded_track_halves_its_length() {
    let (mut c, mut r, id) = setup();
    // A 1.5-beat manual loop (33,075 frames at 120 BPM) halves to 0.75 beats.
    step(&mut c, &mut r, id, DeckCommand::Seek(1000.0));
    step(&mut c, &mut r, id, DeckCommand::LoopIn);
    step(&mut c, &mut r, id, DeckCommand::Seek(34_075.0));
    step(&mut c, &mut r, id, DeckCommand::LoopOut);
    step(&mut c, &mut r, id, DeckCommand::LoopHalve);
    let s = c.deck(id);
    assert_eq!(s.loop_out, Some(1000.0 + 33_075.0 / 2.0));
    assert_eq!(s.loop_beats, 2.0);
}

#[test]
fn halving_a_manual_loop_below_the_minimum_leaves_it() {
    // 600 frames is under 1/32 beat (689 frames at 120 BPM): a halve does not grow it to
    // the minimum, it leaves it as it is.
    let (mut c, mut r, id) = setup();
    step(&mut c, &mut r, id, DeckCommand::Seek(1000.0));
    step(&mut c, &mut r, id, DeckCommand::LoopIn);
    step(&mut c, &mut r, id, DeckCommand::Seek(1600.0));
    step(&mut c, &mut r, id, DeckCommand::LoopOut);
    step(&mut c, &mut r, id, DeckCommand::LoopHalve);
    assert_eq!(c.deck(id).loop_out, Some(1600.0));
    step(&mut c, &mut r, id, DeckCommand::LoopDouble);
    assert_eq!(c.deck(id).loop_out, Some(2200.0));
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
    // A separate engine: the block must hold 8192 frames. The playhead starts outside the
    // loop and must end up inside it; per-sample checks are in the deck unit tests.
    let id = DeckId::new(0).unwrap();
    let (mut c, mut r) = engine(EngineConfig {
        sample_rate: SR,
        max_block_frames: 8192,
    });
    c.send(Command::Deck(
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
    let mut cmds = vec![DeckCommand::Seek(20_000.0)];
    cmds.extend(std::iter::repeat_n(DeckCommand::LoopHalve, 7));
    cmds.extend([DeckCommand::AutoLoop, DeckCommand::Play]);
    for cmd in cmds {
        c.send(Command::Deck(id, cmd)).unwrap();
    }
    render(&mut r, 8192);
    let s = c.deck(id);
    let (start, end) = (s.loop_in.unwrap(), s.loop_out.unwrap());
    assert!(s.loop_active);
    assert_eq!(start, 14_700.0);
    assert!((end - start - 14_700.0 / 32.0).abs() < 1e-6);
    assert!((start..end).contains(&s.position_frames));
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

#[test]
fn a_backspin_ignores_the_loop_and_exits_it_only_when_it_ends_outside() {
    // Loop 22 050..110 250 (the four beats before 30 000).
    let looped = || {
        let (mut c, mut r, id) = setup();
        step(&mut c, &mut r, id, DeckCommand::Seek(30_000.0));
        step(&mut c, &mut r, id, DeckCommand::AutoLoop);
        step(&mut c, &mut r, id, DeckCommand::Seek(100_000.0));
        step(&mut c, &mut r, id, DeckCommand::Play);
        (c, r, id)
    };

    // Ends outside the loop: it ran back through loop-in without wrapping, then exits.
    let (mut c, mut r, id) = looped();
    step(&mut c, &mut r, id, DeckCommand::Backspin { seconds: 2.0 });
    let mut passed_loop_in = false;
    while c.deck(id).backspinning {
        render(&mut r, 512);
        let s = c.deck(id);
        assert!(s.loop_active || !s.backspinning, "the loop ended mid-spin");
        passed_loop_in |= s.position_frames < 22_050.0 && s.backspinning;
    }
    assert!(passed_loop_in, "the spin wrapped at the loop");
    assert!(!c.deck(id).loop_active);

    // Ends inside the loop: the loop stays on.
    let (mut c, mut r, id) = looped();
    step(&mut c, &mut r, id, DeckCommand::Backspin { seconds: 0.5 });
    while c.deck(id).backspinning {
        render(&mut r, 512);
    }
    let s = c.deck(id);
    assert!(s.position_frames > 22_050.0 && s.position_frames < 110_250.0);
    assert!(s.loop_active);
}
