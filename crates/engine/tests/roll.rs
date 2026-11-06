mod common;

use common::{ramp_track, render};
use engine::{
    Command, DeckCommand, DeckId, EngineConfig, EngineController, EngineRenderer, Grid, HotcueSlot,
    TrackMemory, engine,
};

const SR: u32 = 44_100;
const FRAMES: usize = 600_000;
/// Frames in one beat at the tests' 120 BPM grid.
const BEAT: f64 = SR as f64 / 2.0;

fn load_with(grid: Option<Grid>) -> DeckCommand {
    DeckCommand::Load {
        track: ramp_track(SR, FRAMES),
        memory: TrackMemory {
            grid,
            ..TrackMemory::default()
        },
    }
}

fn load() -> DeckCommand {
    load_with(Grid::from_bpm(120.0, 0.0, SR))
}

/// A deck playing from `start`, slip off, loaded by `load`.
fn setup_with(load: DeckCommand, start: f64) -> (EngineController, EngineRenderer, DeckId) {
    let (mut c, mut r) = engine(EngineConfig {
        sample_rate: SR,
        max_block_frames: 512,
    });
    let id = DeckId::new(0).unwrap();
    for cmd in [load, DeckCommand::Seek(start), DeckCommand::Play] {
        c.send(Command::Deck(id, cmd)).unwrap();
    }
    render(&mut r, 1);
    (c, r, id)
}

fn setup(start: f64) -> (EngineController, EngineRenderer, DeckId) {
    setup_with(load(), start)
}

/// Sends `cmd` and renders one frame so the renderer applies it and publishes state.
fn step(c: &mut EngineController, r: &mut EngineRenderer, id: DeckId, cmd: DeckCommand) {
    c.send(Command::Deck(id, cmd)).unwrap();
    render(r, 1);
}

fn press(beats: f64) -> DeckCommand {
    DeckCommand::RollPress { beats }
}

fn release(beats: f64) -> DeckCommand {
    DeckCommand::RollRelease { beats }
}

fn position(c: &EngineController, id: DeckId) -> f64 {
    c.deck(id).position_frames
}

fn ghost(c: &EngineController, id: DeckId) -> f64 {
    c.deck(id).ghost_frames.expect("a ghost runs")
}

#[test]
fn roll_loops_then_slips_back() {
    let (mut c, mut r, id) = setup(100_000.0);
    assert!(!c.deck(id).slip, "a roll slips with slip off");
    let start = position(&c, id);
    step(&mut c, &mut r, id, press(0.5));
    let s = c.deck(id);
    assert!(s.roll_active);
    assert_eq!(s.ghost_frames, Some(start + 1.0));
    // Half a beat is 11 025 frames: four of them in one second, plus the press frame.
    render(&mut r, SR as usize);
    assert_eq!(position(&c, id), start + 1.0);
    assert_eq!(ghost(&c, id), start + 1.0 + f64::from(SR));
    step(&mut c, &mut r, id, release(0.5));
    let s = c.deck(id);
    assert!(!s.roll_active);
    assert_eq!(s.ghost_frames, None);
    assert_eq!(s.position_frames, start + 2.0 + f64::from(SR));
    assert!(s.playing);
    // Past the roll, the playhead no longer wraps.
    render(&mut r, SR as usize);
    assert_eq!(position(&c, id), start + 2.0 + 2.0 * f64::from(SR));
}

#[test]
fn roll_keeps_a_normal_loop_intact() {
    let (mut c, mut r, id) = setup(30_000.0);
    step(&mut c, &mut r, id, DeckCommand::AutoLoop);
    let (loop_in, loop_out) = (BEAT, 5.0 * BEAT);
    assert_eq!(c.deck(id).loop_out, Some(loop_out));
    let start = position(&c, id);
    step(&mut c, &mut r, id, press(0.25));
    // Two seconds: the playhead wraps in the quarter-beat roll, the ghost in the loop.
    render(&mut r, 2 * SR as usize);
    let s = c.deck(id);
    assert!(s.loop_active);
    assert_eq!((s.loop_in, s.loop_out), (Some(loop_in), Some(loop_out)));
    assert!((start..start + BEAT / 4.0).contains(&s.position_frames));
    let expected = loop_in + (start + 1.0 + 2.0 * f64::from(SR) - loop_in) % (4.0 * BEAT);
    assert_eq!(ghost(&c, id), expected);
    step(&mut c, &mut r, id, release(0.25));
    let s = c.deck(id);
    assert!(s.loop_active, "the normal loop goes on");
    assert_eq!((s.loop_in, s.loop_out), (Some(loop_in), Some(loop_out)));
    assert_eq!(s.position_frames, expected + 1.0);
}

#[test]
fn roll_needs_grid_and_playing() {
    let (mut c, mut r, id) = setup_with(load_with(None), 100_000.0);
    step(&mut c, &mut r, id, press(1.0));
    step(&mut c, &mut r, id, DeckCommand::SlicePress(0));
    let s = c.deck(id);
    assert!(!s.roll_active);
    assert_eq!(s.ghost_frames, None);
    assert_eq!(s.slicer_block_beat, None);

    let (mut c, mut r, id) = setup(100_000.0);
    step(&mut c, &mut r, id, DeckCommand::Pause);
    step(&mut c, &mut r, id, press(1.0));
    step(&mut c, &mut r, id, DeckCommand::SlicePress(0));
    assert!(!c.deck(id).roll_active);
    assert_eq!(c.deck(id).ghost_frames, None);

    // Sizes that are not finite and positive, and slices past 7, do nothing.
    step(&mut c, &mut r, id, DeckCommand::Play);
    for cmd in [
        press(0.0),
        press(-1.0),
        press(f64::NAN),
        press(f64::INFINITY),
        DeckCommand::SlicePress(8),
    ] {
        step(&mut c, &mut r, id, cmd);
        assert!(!c.deck(id).roll_active);
    }
}

#[test]
fn release_of_another_roll_size_is_ignored() {
    let (mut c, mut r, id) = setup(100_000.0);
    step(&mut c, &mut r, id, press(1.0));
    let g0 = ghost(&c, id);
    render(&mut r, 1000);
    // A second pad replaces the roll at the playhead; the ghost keeps running.
    let second = position(&c, id);
    step(&mut c, &mut r, id, press(0.5));
    assert_eq!(ghost(&c, id), g0 + 1001.0);
    step(&mut c, &mut r, id, release(1.0));
    let s = c.deck(id);
    assert!(s.roll_active, "the first pad's release is ignored");
    assert_eq!(s.position_frames, second + 2.0);
    render(&mut r, BEAT as usize / 2);
    assert_eq!(position(&c, id), second + 2.0, "it loops half a beat");
    step(&mut c, &mut r, id, release(0.5));
    let s = c.deck(id);
    assert!(!s.roll_active);
    assert_eq!(s.position_frames, g0 + 1003.0 + BEAT / 2.0);
}

#[test]
fn slicer_block_follows_playback() {
    let (mut c, mut r, id) = setup(7.5 * BEAT);
    assert_eq!(c.deck(id).slicer_block_beat, Some(0.0));
    render(&mut r, BEAT as usize);
    assert_eq!(c.deck(id).slicer_block_beat, Some(8.0));
    // While a slice is held the block stays, though the ghost crosses into the next one.
    step(&mut c, &mut r, id, DeckCommand::SlicePress(0));
    render(&mut r, 8 * BEAT as usize);
    assert!(ghost(&c, id) > 16.0 * BEAT);
    assert_eq!(c.deck(id).slicer_block_beat, Some(8.0));
    step(&mut c, &mut r, id, DeckCommand::SliceRelease(0));
    assert_eq!(c.deck(id).slicer_block_beat, Some(16.0));
    // A loop roll is followed through its ghost: the block is where the track would be.
    step(&mut c, &mut r, id, press(1.0 / 16.0));
    render(&mut r, 8 * BEAT as usize);
    assert_eq!(c.deck(id).slicer_block_beat, Some(24.0));
}

#[test]
fn slice_press_repeats_its_beat_then_slips_back() {
    let (mut c, mut r, id) = setup(9.25 * BEAT);
    let start = position(&c, id);
    step(&mut c, &mut r, id, DeckCommand::SlicePress(3));
    let s = c.deck(id);
    assert!(s.roll_active);
    assert_eq!(s.position_frames, 11.0 * BEAT + 1.0);
    assert_eq!(s.ghost_frames, Some(start + 1.0));
    render(&mut r, 2 * BEAT as usize);
    assert_eq!(position(&c, id), 11.0 * BEAT + 1.0, "beat 11 repeats");
    step(&mut c, &mut r, id, DeckCommand::SliceRelease(2));
    assert!(c.deck(id).roll_active, "another slice's release is ignored");
    step(&mut c, &mut r, id, DeckCommand::SliceRelease(3));
    let s = c.deck(id);
    assert!(!s.roll_active);
    assert_eq!(s.ghost_frames, None);
    assert_eq!(s.position_frames, start + 3.0 + 2.0 * BEAT);
}

#[test]
fn slice_to_slice_moves_the_loop() {
    let (mut c, mut r, id) = setup(9.25 * BEAT);
    let start = position(&c, id);
    step(&mut c, &mut r, id, DeckCommand::SlicePress(3));
    render(&mut r, 1000);
    step(&mut c, &mut r, id, DeckCommand::SlicePress(5));
    assert_eq!(position(&c, id), 13.0 * BEAT + 1.0);
    step(&mut c, &mut r, id, DeckCommand::SliceRelease(3));
    assert!(
        c.deck(id).roll_active,
        "the first slice's release is ignored"
    );
    render(&mut r, BEAT as usize);
    assert_eq!(position(&c, id), 13.0 * BEAT + 2.0, "beat 13 repeats");
    assert_eq!(c.deck(id).slicer_block_beat, Some(8.0));
    step(&mut c, &mut r, id, DeckCommand::SliceRelease(5));
    let s = c.deck(id);
    assert!(!s.roll_active);
    assert_eq!(s.position_frames, start + 1004.0 + BEAT);
}

#[test]
fn load_resets_roll_and_slice() {
    let (mut c, mut r, id) = setup(100_000.0);
    step(&mut c, &mut r, id, press(1.0));
    step(&mut c, &mut r, id, load());
    let s = c.deck(id);
    assert!(!s.roll_active);
    assert_eq!(s.ghost_frames, None);
    assert_eq!(s.position_frames, 0.0);

    // Seek and a hotcue jump end a roll or slice too, and the jump wins.
    for jump in [
        DeckCommand::Seek(300_000.0),
        DeckCommand::HotcuePress(HotcueSlot::new(1).unwrap()),
    ] {
        let (mut c, mut r, id) = setup(100_000.0);
        step(
            &mut c,
            &mut r,
            id,
            DeckCommand::SetHotcue(HotcueSlot::new(1).unwrap(), Some(300_000.0)),
        );
        for pad in [press(1.0), DeckCommand::SlicePress(2)] {
            step(&mut c, &mut r, id, pad);
            assert!(c.deck(id).roll_active);
            step(&mut c, &mut r, id, jump.clone());
            let s = c.deck(id);
            assert!(!s.roll_active, "{jump:?}");
            assert_eq!(s.ghost_frames, None);
            assert_eq!(s.position_frames, 300_001.0);
            render(&mut r, 2 * BEAT as usize);
            assert_eq!(position(&c, id), 300_001.0 + 2.0 * BEAT, "no more wrapping");
        }
    }
}

#[test]
fn beat_jump_moves_a_held_roll() {
    let (mut c, mut r, id) = setup(100_000.0);
    let start = position(&c, id);
    step(&mut c, &mut r, id, press(1.0));
    render(&mut r, 1000);
    step(&mut c, &mut r, id, DeckCommand::BeatJump { beats: 4.0 });
    let s = c.deck(id);
    assert!(s.roll_active, "a beat jump never cancels a held pad");
    assert_eq!(s.position_frames, start + 1002.0 + 4.0 * BEAT);
    assert_eq!(ghost(&c, id), start + 1002.0 + 4.0 * BEAT);
    // The roll loop moved with it: one beat later the playhead is back in the same place.
    render(&mut r, BEAT as usize);
    assert_eq!(position(&c, id), start + 1002.0 + 4.0 * BEAT);
    step(&mut c, &mut r, id, release(1.0));
    assert_eq!(position(&c, id), start + 1003.0 + 5.0 * BEAT);
}
