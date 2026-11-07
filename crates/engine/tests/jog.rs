mod common;

use common::{ramp_track, render};
use engine::{
    Command, DeckCommand, DeckId, EngineConfig, EngineController, EngineRenderer, Grid, JogSurface,
    TrackMemory, engine,
};

const SR: u32 = 44_100;
const FRAMES: usize = 600_000;
/// Track frames per platter turn: 1.8 s at the track's rate.
const PLATTER_FRAMES: f64 = 1.8 * SR as f64;

fn load(track_frames: usize) -> DeckCommand {
    DeckCommand::Load {
        track: ramp_track(SR, track_frames),
        memory: TrackMemory {
            grid: Grid::from_bpm(120.0, 0.0, SR),
            ..TrackMemory::default()
        },
    }
}

fn setup() -> (EngineController, EngineRenderer, DeckId) {
    let (mut c, r) = engine(EngineConfig {
        sample_rate: SR,
        max_block_frames: 512,
    });
    let id = DeckId::new(0).unwrap();
    c.send(Command::Deck(id, load(FRAMES))).unwrap();
    (c, r, id)
}

/// Sends `cmd` and renders one block so the renderer applies it and publishes state.
fn step(c: &mut EngineController, r: &mut EngineRenderer, id: DeckId, cmd: DeckCommand) {
    c.send(Command::Deck(id, cmd)).unwrap();
    render(r, 1);
}

fn top(turns: f64) -> DeckCommand {
    DeckCommand::Jog {
        turns,
        surface: JogSurface::Top,
    }
}

fn ring(turns: f64) -> DeckCommand {
    DeckCommand::Jog {
        turns,
        surface: JogSurface::Ring,
    }
}

fn position(c: &EngineController, id: DeckId) -> f64 {
    c.deck(id).position_frames
}

#[test]
fn scratch_follows_the_platter() {
    // At SCRATCH_MAX_SPEED (8x) a full turn (1.8 s) needs at least 0.225 s plus the lag's
    // settling, so the brief's 0.2 s cannot be reached; 0.5 s is ample.
    let (mut c, mut r, id) = setup();
    step(&mut c, &mut r, id, DeckCommand::Seek(10_000.0));
    step(&mut c, &mut r, id, DeckCommand::JogTouch(true));
    step(&mut c, &mut r, id, top(1.0));
    render(&mut r, SR as usize / 2);
    let pos = position(&c, id);
    assert!((pos - (10_000.0 + PLATTER_FRAMES)).abs() <= 1.0, "{pos}");
    assert!(c.deck(id).jog_touched);
}

#[test]
fn scratch_back_past_zero_clamps() {
    let (mut c, mut r, id) = setup();
    step(&mut c, &mut r, id, DeckCommand::Seek(10_000.0));
    step(&mut c, &mut r, id, DeckCommand::Play);
    step(&mut c, &mut r, id, DeckCommand::JogTouch(true));
    step(&mut c, &mut r, id, top(-1.0));
    render(&mut r, SR as usize / 2);
    // The target is clamped to the start, which the playhead approaches without passing.
    let pos = position(&c, id);
    assert!((0.0..1e-6).contains(&pos), "{pos}");
    // Forward motion starts from 0 at once: the target was clamped too.
    step(&mut c, &mut r, id, top(0.1));
    render(&mut r, SR as usize / 2);
    let pos = position(&c, id);
    assert!((pos - 0.1 * PLATTER_FRAMES).abs() <= 1.0, "{pos}");
}

#[test]
fn touch_stops_and_release_resumes_playing() {
    let (mut c, mut r, id) = setup();
    step(&mut c, &mut r, id, DeckCommand::Play);
    step(&mut c, &mut r, id, DeckCommand::JogTouch(true));
    let held = position(&c, id);
    render(&mut r, 4096);
    assert_eq!(position(&c, id), held);
    assert!(c.deck(id).playing);
    step(&mut c, &mut r, id, DeckCommand::JogTouch(false));
    render(&mut r, 4096);
    let s = c.deck(id);
    assert!(s.playing && !s.jog_touched);
    assert_eq!(s.position_frames, held + 4097.0);
}

#[test]
fn paused_touch_scratch_is_audible_then_stays_paused() {
    let (mut c, mut r, id) = setup();
    step(&mut c, &mut r, id, DeckCommand::Seek(100_000.0));
    step(&mut c, &mut r, id, DeckCommand::JogTouch(true));
    step(&mut c, &mut r, id, top(0.1));
    let (master, _) = render(&mut r, 8192);
    assert!(master.iter().any(|&s| s.abs() > 0.01));
    assert!(position(&c, id) > 100_000.0);
    step(&mut c, &mut r, id, DeckCommand::JogTouch(false));
    let released = position(&c, id);
    render(&mut r, 4096);
    assert!(!c.deck(id).playing);
    assert_eq!(position(&c, id), released);
}

#[test]
fn vinyl_off_top_nudges() {
    let (mut c, mut r, id) = setup();
    step(&mut c, &mut r, id, DeckCommand::SetVinyl(false));
    step(&mut c, &mut r, id, DeckCommand::Play);
    step(&mut c, &mut r, id, DeckCommand::JogTouch(true));
    let s = c.deck(id);
    assert!(!s.vinyl && s.jog_touched);
    let start = s.position_frames;
    step(&mut c, &mut r, id, top(0.05));
    // The top nudges like the ring: the first frame moves 10 % faster.
    assert!((position(&c, id) - start - 1.1).abs() < 1e-9);
}

#[test]
fn nudge_speeds_up_immediately() {
    let (mut c, mut r, id) = setup();
    step(&mut c, &mut r, id, DeckCommand::Play);
    let start = position(&c, id);
    // 0.05 turn bends the speed by 0.05 * 2 = 10 %, from the very first frame.
    step(&mut c, &mut r, id, ring(0.05));
    let moved = position(&c, id) - start;
    assert!((moved - 1.1).abs() < 1e-9, "{moved}");
}

#[test]
fn nudge_fades_within_a_few_tenths_of_a_second() {
    let (mut c, mut r, id) = setup();
    step(&mut c, &mut r, id, DeckCommand::Play);
    step(&mut c, &mut r, id, ring(0.05));
    render(&mut r, SR as usize / 2);
    let before = position(&c, id);
    render(&mut r, 1000);
    let speed = (position(&c, id) - before) / 1000.0;
    assert!((speed - 1.0).abs() < 0.01, "{speed}");
}

#[test]
fn nudge_bend_is_clamped() {
    let (mut c, mut r, id) = setup();
    step(&mut c, &mut r, id, DeckCommand::Play);
    let start = position(&c, id);
    step(&mut c, &mut r, id, ring(1e6));
    let fast = position(&c, id) - start;
    assert!((fast - 1.5).abs() < 1e-9, "{fast}");
    // Let it fade, then bend backwards: still moving forward at half speed at most.
    render(&mut r, SR as usize);
    let start = position(&c, id);
    step(&mut c, &mut r, id, ring(-1e6));
    let slow = position(&c, id) - start;
    assert!((slow - 0.5).abs() < 1e-9, "{slow}");
}

#[test]
fn paused_ring_seeks_silently() {
    let (mut c, mut r, id) = setup();
    step(&mut c, &mut r, id, DeckCommand::Seek(100_000.0));
    c.send(Command::Deck(id, ring(0.5))).unwrap();
    let (master, _) = render(&mut r, 4096);
    assert!(master.iter().all(|&s| s == 0.0));
    let s = c.deck(id);
    assert!(!s.playing);
    assert!((s.position_frames - (100_000.0 + 0.5 * PLATTER_FRAMES)).abs() < 1e-6);
    step(&mut c, &mut r, id, ring(-1.0));
    assert!((position(&c, id) - (100_000.0 - 0.5 * PLATTER_FRAMES)).abs() < 1e-6);
}

#[test]
fn search_moves_ten_seconds_per_turn() {
    let (mut c, mut r, id) = setup();
    step(&mut c, &mut r, id, DeckCommand::JogSearch { turns: 0.5 });
    assert_eq!(position(&c, id), 5.0 * SR as f64);
    step(&mut c, &mut r, id, DeckCommand::Play);
    let start = position(&c, id);
    step(&mut c, &mut r, id, DeckCommand::JogSearch { turns: -0.25 });
    // Playing: the block that applies the search also advances one frame.
    assert_eq!(position(&c, id), start - 2.5 * SR as f64 + 1.0);
    assert!(c.deck(id).playing);
}

#[test]
fn seek_and_search_outside_a_loop_exit_it() {
    let (mut c, mut r, id) = setup();
    step(&mut c, &mut r, id, DeckCommand::AutoLoop);
    // Inside the 4-beat loop [0, 88_200): kept.
    step(&mut c, &mut r, id, ring(0.5));
    assert!(c.deck(id).loop_active);
    step(&mut c, &mut r, id, ring(1.0));
    assert!(!c.deck(id).loop_active);
    step(&mut c, &mut r, id, DeckCommand::Seek(0.0));
    step(&mut c, &mut r, id, DeckCommand::ReloopExit);
    assert!(c.deck(id).loop_active);
    step(&mut c, &mut r, id, DeckCommand::JogSearch { turns: 1.0 });
    assert!(!c.deck(id).loop_active);
}

#[test]
fn scratch_at_end_does_not_pause() {
    let (mut c, mut r, id) = setup();
    step(
        &mut c,
        &mut r,
        id,
        DeckCommand::Seek(FRAMES as f64 - 1000.0),
    );
    step(&mut c, &mut r, id, DeckCommand::Play);
    step(&mut c, &mut r, id, DeckCommand::JogTouch(true));
    step(&mut c, &mut r, id, top(1.0));
    render(&mut r, SR as usize / 2);
    let s = c.deck(id);
    assert!(
        (FRAMES as f64 - s.position_frames) < 1e-6,
        "{}",
        s.position_frames
    );
    assert!(s.playing);
    // Back from the end while still held.
    step(&mut c, &mut r, id, top(-0.1));
    render(&mut r, SR as usize / 2);
    let pos = position(&c, id);
    assert!((pos - (FRAMES as f64 - 0.1 * PLATTER_FRAMES)).abs() <= 1.0);
}

#[test]
fn scratch_inside_loop_does_not_wrap() {
    let (mut c, mut r, id) = setup();
    step(&mut c, &mut r, id, DeckCommand::AutoLoop);
    step(&mut c, &mut r, id, DeckCommand::Seek(80_000.0));
    step(&mut c, &mut r, id, DeckCommand::Play);
    step(&mut c, &mut r, id, DeckCommand::JogTouch(true));
    let start = position(&c, id);
    step(&mut c, &mut r, id, top(0.25));
    render(&mut r, SR as usize / 2);
    let s = c.deck(id);
    assert!((s.position_frames - (start + 0.25 * PLATTER_FRAMES)).abs() <= 1.0);
    assert!(s.position_frames > 88_200.0);
    assert!(s.loop_active);
}

#[test]
fn ring_while_scratching_playing_is_dropped() {
    let (mut c, mut r, id) = setup();
    step(&mut c, &mut r, id, DeckCommand::Play);
    step(&mut c, &mut r, id, DeckCommand::JogTouch(true));
    let held = position(&c, id);
    step(&mut c, &mut r, id, ring(1.0));
    assert_eq!(position(&c, id), held);
    step(&mut c, &mut r, id, DeckCommand::JogTouch(false));
    render(&mut r, 4096);
    // Plain playback after release: no nudge was stored.
    assert_eq!(position(&c, id), held + 4097.0);
}

#[test]
fn ring_while_scratching_paused_is_dropped() {
    let (mut c, mut r, id) = setup();
    step(&mut c, &mut r, id, DeckCommand::Seek(100_000.0));
    step(&mut c, &mut r, id, DeckCommand::JogTouch(true));
    step(&mut c, &mut r, id, ring(1.0));
    assert_eq!(position(&c, id), 100_000.0);
    step(&mut c, &mut r, id, DeckCommand::JogTouch(false));
    step(&mut c, &mut r, id, DeckCommand::Play);
    render(&mut r, 4096);
    assert_eq!(position(&c, id), 100_000.0 + 4097.0);
}

#[test]
fn huge_turns_keep_the_playhead_finite() {
    let (mut c, mut r, id) = setup();
    step(&mut c, &mut r, id, DeckCommand::Play);
    for turns in [1e300, -1e300, 1e308, 1e308, -1e308, -1e308] {
        step(&mut c, &mut r, id, ring(turns));
        render(&mut r, 512);
        assert!(position(&c, id).is_finite());
    }
    step(&mut c, &mut r, id, DeckCommand::JogTouch(true));
    for turns in [1e308, -1e308, 1e308] {
        step(&mut c, &mut r, id, top(turns));
        render(&mut r, 512);
        assert!(position(&c, id).is_finite());
    }
}

#[test]
fn non_finite_turns_are_ignored() {
    let (mut c, mut r, id) = setup();
    step(&mut c, &mut r, id, DeckCommand::Seek(1000.0));
    for turns in [f64::NAN, f64::INFINITY] {
        step(&mut c, &mut r, id, ring(turns));
        step(&mut c, &mut r, id, DeckCommand::JogSearch { turns });
    }
    assert_eq!(position(&c, id), 1000.0);
}

#[test]
fn load_resets_jog_state() {
    let (mut c, mut r, id) = setup();
    step(&mut c, &mut r, id, DeckCommand::Play);
    step(&mut c, &mut r, id, ring(1.0));
    step(&mut c, &mut r, id, DeckCommand::JogTouch(true));
    step(&mut c, &mut r, id, top(1.0));
    step(&mut c, &mut r, id, load(FRAMES));
    let s = c.deck(id);
    assert!(!s.jog_touched && !s.playing);
    assert_eq!(s.position_frames, 0.0);
    // Neither the scratch target nor the pending nudge moves the new track.
    render(&mut r, 1000);
    assert_eq!(position(&c, id), 0.0);
    step(&mut c, &mut r, id, DeckCommand::Play);
    render(&mut r, 1000);
    assert_eq!(position(&c, id), 1001.0);
    // Vinyl mode is a deck setting: it survives loads.
    step(&mut c, &mut r, id, DeckCommand::SetVinyl(false));
    step(&mut c, &mut r, id, load(FRAMES));
    assert!(!c.deck(id).vinyl);
}

fn platter(turns: f64) -> DeckCommand {
    DeckCommand::Jog {
        turns,
        surface: JogSurface::Platter,
    }
}

#[test]
fn platter_scratches_with_vinyl_off() {
    let (mut c, mut r, id) = setup();
    step(&mut c, &mut r, id, DeckCommand::SetVinyl(false));
    step(&mut c, &mut r, id, DeckCommand::Seek(10_000.0));
    step(&mut c, &mut r, id, DeckCommand::Play);
    step(&mut c, &mut r, id, DeckCommand::JogTouch(true));
    // A Top move does nothing without vinyl; a Platter move scratches.
    step(&mut c, &mut r, id, platter(1.0));
    render(&mut r, SR as usize / 2);
    let pos = position(&c, id);
    // The one-frame steps that played before the touch count for a frame each.
    assert!((pos - (10_000.0 + PLATTER_FRAMES)).abs() <= 4.0, "{pos}");
    // The release ends the scratch: playback resumes.
    step(&mut c, &mut r, id, DeckCommand::JogTouch(false));
    let released = position(&c, id);
    render(&mut r, 4410);
    assert!(position(&c, id) > released + 4000.0);
}

#[test]
fn platter_without_touch_nudges() {
    let (mut c, mut r, id) = setup();
    step(&mut c, &mut r, id, DeckCommand::Seek(10_000.0));
    step(&mut c, &mut r, id, DeckCommand::Play);
    step(&mut c, &mut r, id, platter(0.1));
    render(&mut r, 1000);
    // Untouched, it bends the speed like the ring instead of scratching.
    let pos = position(&c, id);
    assert!(
        pos > 10_000.0 + 1000.0 + 20.0 && pos < 10_000.0 + 1000.0 + PLATTER_FRAMES,
        "{pos}"
    );
    let (mut c2, mut r2, id2) = setup();
    step(&mut c2, &mut r2, id2, DeckCommand::Seek(10_000.0));
    step(&mut c2, &mut r2, id2, DeckCommand::Play);
    step(&mut c2, &mut r2, id2, ring(0.1));
    render(&mut r2, 1000);
    assert_eq!(pos, position(&c2, id2));
}

/// Renders `frames` in blocks, returning the lowest playhead seen.
fn lowest_position(c: &EngineController, r: &mut EngineRenderer, id: DeckId, frames: usize) -> f64 {
    let mut lowest = position(c, id);
    for _ in 0..frames / 512 {
        render(r, 512);
        lowest = lowest.min(position(c, id));
    }
    lowest
}

#[test]
fn backspin_reverses_then_resumes() {
    let (mut c, mut r, id) = setup();
    step(&mut c, &mut r, id, DeckCommand::Seek(200_000.0));
    step(&mut c, &mut r, id, DeckCommand::Play);
    step(&mut c, &mut r, id, DeckCommand::Backspin { seconds: 1.0 });
    assert!(c.deck(id).backspinning);
    let start = position(&c, id);
    let lowest = lowest_position(&c, &mut r, id, SR as usize);
    let back = start - lowest;
    let expected = 1.5 * f64::from(SR);
    assert!((back - expected).abs() <= 3.0 * 512.0, "went back {back}");
    // Afterwards it plays forward again.
    render(&mut r, 1024);
    assert!(!c.deck(id).backspinning);
    let resumed = position(&c, id);
    render(&mut r, 4410);
    assert!((position(&c, id) - resumed - 4410.0).abs() < 1.0);
}

#[test]
fn backspin_clamps_at_the_start() {
    let (mut c, mut r, id) = setup();
    step(&mut c, &mut r, id, DeckCommand::Seek(20_000.0));
    step(&mut c, &mut r, id, DeckCommand::Play);
    step(&mut c, &mut r, id, DeckCommand::Backspin { seconds: 5.0 });
    let lowest = lowest_position(&c, &mut r, id, SR as usize);
    // The spin stops at 0 within a block and playback goes on forward in the same block.
    assert!(lowest < 512.0, "{lowest}");
    assert!(!c.deck(id).backspinning);
    assert!(c.deck(id).playing);
    let before = position(&c, id);
    render(&mut r, 4410);
    assert!(position(&c, id) > before + 4000.0);
}

#[test]
fn backspin_on_a_paused_deck_does_nothing() {
    let (mut c, mut r, id) = setup();
    step(&mut c, &mut r, id, DeckCommand::Seek(20_000.0));
    step(&mut c, &mut r, id, DeckCommand::Backspin { seconds: 1.0 });
    assert!(!c.deck(id).backspinning);
    render(&mut r, 4410);
    assert_eq!(position(&c, id), 20_000.0);
}

#[test]
fn a_platter_jog_cancels_a_running_backspin() {
    let (mut c, mut r, id) = setup();
    step(&mut c, &mut r, id, DeckCommand::Seek(200_000.0));
    step(&mut c, &mut r, id, DeckCommand::Play);
    step(&mut c, &mut r, id, DeckCommand::SetVinyl(false));
    step(&mut c, &mut r, id, DeckCommand::JogTouch(true));
    step(&mut c, &mut r, id, DeckCommand::Backspin { seconds: 2.0 });
    assert!(c.deck(id).backspinning);
    let jog = DeckCommand::Jog {
        turns: 0.1,
        surface: JogSurface::Platter,
    };
    step(&mut c, &mut r, id, jog);
    assert!(!c.deck(id).backspinning);
}

#[test]
fn backspin_is_cancelled_by_a_touch() {
    let (mut c, mut r, id) = setup();
    step(&mut c, &mut r, id, DeckCommand::Seek(200_000.0));
    step(&mut c, &mut r, id, DeckCommand::Play);
    step(&mut c, &mut r, id, DeckCommand::Backspin { seconds: 2.0 });
    render(&mut r, 4410);
    assert!(c.deck(id).backspinning);
    step(&mut c, &mut r, id, DeckCommand::JogTouch(true));
    assert!(!c.deck(id).backspinning);
    let held = position(&c, id);
    render(&mut r, 4410);
    assert!((position(&c, id) - held).abs() < 1.0, "held by the hand");
}

#[test]
fn backspin_is_cancelled_by_a_seek_and_ends_a_roll() {
    let (mut c, mut r, id) = setup();
    step(&mut c, &mut r, id, DeckCommand::Seek(200_000.0));
    step(&mut c, &mut r, id, DeckCommand::Play);
    step(&mut c, &mut r, id, DeckCommand::RollPress { beats: 1.0 });
    assert!(c.deck(id).roll_active);
    step(&mut c, &mut r, id, DeckCommand::Backspin { seconds: 2.0 });
    assert!(c.deck(id).backspinning && !c.deck(id).roll_active);
    step(&mut c, &mut r, id, DeckCommand::Seek(50_000.0));
    assert!(!c.deck(id).backspinning);
}
