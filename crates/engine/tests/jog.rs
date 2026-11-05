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
    step(&mut c, &mut r, id, top(0.25));
    render(&mut r, SR as usize);
    let expected = start + 1.0 + SR as f64 + 0.125 * SR as f64;
    let pos = position(&c, id);
    assert!((pos - expected).abs() < 1e-5, "{pos} vs {expected}");
}

#[test]
fn nudge_shifts_by_the_turn_then_returns_to_tempo() {
    let (mut c, mut r, id) = setup();
    step(&mut c, &mut r, id, DeckCommand::SetTempoSlider(0.75));
    step(&mut c, &mut r, id, DeckCommand::Play);
    let ratio = c.deck(id).tempo_ratio;
    assert!((ratio - 1.05).abs() < 1e-9);
    let start = position(&c, id);
    step(&mut c, &mut r, id, ring(0.25));
    render(&mut r, SR as usize);
    let expected = start + (1.0 + SR as f64) * ratio + 0.125 * SR as f64;
    let pos = position(&c, id);
    assert!((pos - expected).abs() < 1e-5, "{pos} vs {expected}");
    render(&mut r, 1000);
    let speed = (position(&c, id) - pos) / 1000.0;
    assert!((speed - ratio).abs() < 1e-8, "{speed}");
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
