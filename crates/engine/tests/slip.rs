mod common;

use common::{ramp_track, render};
use engine::{
    Command, DeckCommand, DeckId, EngineConfig, EngineController, EngineRenderer, Grid, Hotcue,
    HotcueSlot, JogSurface, TempoRange, TrackMemory, engine,
};

const SR: u32 = 44_100;
const FRAMES: usize = 600_000;
/// Frames in one beat at the tests' 120 BPM grid.
const BEAT: f64 = SR as f64 / 2.0;

fn slot(n: u8) -> HotcueSlot {
    HotcueSlot::new(n).unwrap()
}

fn load() -> DeckCommand {
    DeckCommand::Load {
        track: ramp_track(SR, FRAMES),
        memory: TrackMemory {
            grid: Grid::from_bpm(120.0, 0.0, SR),
            ..TrackMemory::default()
        },
    }
}

/// A deck playing from `start`, with slip on or off.
fn setup(start: f64, slip: bool) -> (EngineController, EngineRenderer, DeckId) {
    let (mut c, mut r) = engine(EngineConfig {
        sample_rate: SR,
        max_block_frames: 512,
    });
    let id = DeckId::new(0).unwrap();
    for cmd in [
        load(),
        DeckCommand::Seek(start),
        DeckCommand::SetSlip(slip),
        DeckCommand::Play,
    ] {
        c.send(Command::Deck(id, cmd)).unwrap();
    }
    render(&mut r, 1);
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

fn position(c: &EngineController, id: DeckId) -> f64 {
    c.deck(id).position_frames
}

fn ghost(c: &EngineController, id: DeckId) -> f64 {
    c.deck(id).ghost_frames.expect("a ghost runs")
}

#[test]
fn slip_scratch_returns_where_the_track_would_be() {
    let (mut c, mut r, id) = setup(100_000.0, true);
    assert!(c.deck(id).slip);
    let start = position(&c, id);
    step(&mut c, &mut r, id, DeckCommand::JogTouch(true));
    assert!((ghost(&c, id) - (start + 1.0)).abs() < 1e-9);
    step(&mut c, &mut r, id, top(-0.5));
    render(&mut r, SR as usize);
    assert!(position(&c, id) < start, "the scratch went back");
    step(&mut c, &mut r, id, DeckCommand::JogTouch(false));
    // Touch, jog, one second, release: each step renders one frame.
    let expected = start + 3.0 + f64::from(SR);
    let pos = position(&c, id);
    assert!((pos - expected).abs() <= 1.0, "{pos} vs {expected}");
    assert_eq!(c.deck(id).ghost_frames, None);
    assert!(c.deck(id).playing);
    assert!(c.deck(id).slip, "slip stays on");
}

#[test]
fn slip_scratch_in_a_loop_keeps_the_loop() {
    // The loop starts before slip is on, so the scratch inside it has its own ghost.
    let (mut c, mut r, id) = setup(10.0 * BEAT, false);
    step(&mut c, &mut r, id, DeckCommand::AutoLoop);
    step(&mut c, &mut r, id, DeckCommand::SetSlip(true));
    let (start, end) = (10.0 * BEAT, 14.0 * BEAT);
    assert_eq!(c.deck(id).loop_in, Some(start));
    step(&mut c, &mut r, id, DeckCommand::JogTouch(true));
    let g0 = ghost(&c, id);
    step(&mut c, &mut r, id, top(-0.2));
    render(&mut r, 5 * SR as usize);
    let g = ghost(&c, id);
    assert!(
        (start..end).contains(&g),
        "the ghost wraps in the loop: {g}"
    );
    let expected = start + (g0 + 1.0 + 5.0 * f64::from(SR) - start) % (end - start);
    assert!((g - expected).abs() <= 1.0, "{g} vs {expected}");
    step(&mut c, &mut r, id, DeckCommand::JogTouch(false));
    let s = c.deck(id);
    assert!(s.loop_active, "returning into the loop keeps it");
    assert!((start..end).contains(&s.position_frames));
    assert!((s.position_frames - (expected + 1.0)).abs() <= 1.0);
}

#[test]
fn slip_loop_exit_returns_to_the_ghost() {
    let (mut c, mut r, id) = setup(10.0 * BEAT + 100.0, true);
    let start = position(&c, id);
    step(&mut c, &mut r, id, DeckCommand::AutoLoop);
    assert!(c.deck(id).loop_active);
    assert!((ghost(&c, id) - (start + 1.0)).abs() < 1e-9);
    // Four beats loop for two seconds; play three.
    render(&mut r, 3 * SR as usize);
    let s = c.deck(id);
    assert!((10.0 * BEAT..14.0 * BEAT).contains(&s.position_frames));
    step(&mut c, &mut r, id, DeckCommand::AutoLoop);
    let s = c.deck(id);
    assert!(!s.loop_active);
    assert_eq!(s.ghost_frames, None);
    let expected = start + 2.0 + 3.0 * f64::from(SR);
    assert!(
        (s.position_frames - expected).abs() <= 1.0,
        "{} vs {expected}",
        s.position_frames
    );

    // A manual loop and reloop/exit slip the same way.
    step(&mut c, &mut r, id, DeckCommand::LoopIn);
    render(&mut r, SR as usize);
    step(&mut c, &mut r, id, DeckCommand::LoopOut);
    let g0 = ghost(&c, id);
    render(&mut r, SR as usize);
    step(&mut c, &mut r, id, DeckCommand::ReloopExit);
    let expected = g0 + 1.0 + f64::from(SR);
    assert!((position(&c, id) - expected).abs() <= 1.0);
    assert!(!c.deck(id).loop_active);
}

#[test]
fn slip_hotcue_is_momentary() {
    let (mut c, mut r, id) = setup(100_000.0, true);
    step(
        &mut c,
        &mut r,
        id,
        DeckCommand::SetHotcue(slot(1), Some(Hotcue::point(5000.0))),
    );
    let start = position(&c, id);
    step(&mut c, &mut r, id, DeckCommand::HotcuePress(slot(1)));
    assert!((position(&c, id) - 5001.0).abs() < 1e-9);
    render(&mut r, SR as usize / 2);
    assert!(position(&c, id) < 50_000.0);
    step(&mut c, &mut r, id, DeckCommand::HotcueRelease(slot(1)));
    let s = c.deck(id);
    let expected = start + 2.0 + f64::from(SR / 2);
    assert!((s.position_frames - expected).abs() <= 1.0);
    assert!(s.playing);
    assert_eq!(s.ghost_frames, None);

    // A paused deck still previews from the hotcue, as without slip.
    step(&mut c, &mut r, id, DeckCommand::Pause);
    step(&mut c, &mut r, id, DeckCommand::HotcuePress(slot(1)));
    assert!(c.deck(id).previewing);
    assert_eq!(c.deck(id).ghost_frames, None);
    render(&mut r, 1000);
    step(&mut c, &mut r, id, DeckCommand::HotcueRelease(slot(1)));
    let s = c.deck(id);
    assert_eq!(s.position_frames, 5000.0);
    assert!(!s.playing);
}

#[test]
fn ghost_follows_tempo_and_nudge() {
    let (mut c, mut r, id) = setup(100_000.0, true);
    step(
        &mut c,
        &mut r,
        id,
        DeckCommand::SetTempoRange(TempoRange::Hundred),
    );
    step(&mut c, &mut r, id, DeckCommand::SetTempoSlider(0.75));
    // The record is held still: only the ghost moves, at 1.5 times normal speed.
    step(&mut c, &mut r, id, DeckCommand::JogTouch(true));
    let g0 = ghost(&c, id);
    render(&mut r, SR as usize);
    let g = ghost(&c, id);
    assert!((g - (g0 + 1.5 * f64::from(SR))).abs() < 1e-6, "{g}");
    step(&mut c, &mut r, id, DeckCommand::JogTouch(false));

    // A nudge during a held hotcue bends the ghost as much as the playhead.
    step(
        &mut c,
        &mut r,
        id,
        DeckCommand::SetHotcue(slot(1), Some(Hotcue::point(5000.0))),
    );
    step(&mut c, &mut r, id, DeckCommand::HotcuePress(slot(1)));
    let (p0, g0) = (position(&c, id), ghost(&c, id));
    step(
        &mut c,
        &mut r,
        id,
        DeckCommand::Jog {
            turns: 0.2,
            surface: JogSurface::Ring,
        },
    );
    render(&mut r, SR as usize / 10);
    let (p, g) = (position(&c, id), ghost(&c, id));
    let unbent = 1.5 * f64::from(SR / 10 + 1);
    assert!(p - p0 > unbent + 100.0, "the nudge sped the deck up");
    assert!(
        ((g - g0) - (p - p0)).abs() < 1e-6,
        "{} vs {}",
        g - g0,
        p - p0
    );
}

#[test]
fn slip_off_drops_the_ghost() {
    let (mut c, mut r, id) = setup(100_000.0, true);
    step(&mut c, &mut r, id, DeckCommand::JogTouch(true));
    step(&mut c, &mut r, id, top(-0.5));
    render(&mut r, SR as usize);
    step(&mut c, &mut r, id, DeckCommand::SetSlip(false));
    let s = c.deck(id);
    assert!(!s.slip);
    assert_eq!(s.ghost_frames, None);
    let scratched = s.position_frames;
    step(&mut c, &mut r, id, DeckCommand::JogTouch(false));
    assert!((position(&c, id) - scratched).abs() <= 2.0, "no return");
}

#[test]
fn without_slip_nothing_changes() {
    let (mut c, mut r, id) = setup(100_000.0, false);
    assert!(!c.deck(id).slip, "off by default");
    // A scratch stays where it left the record.
    step(&mut c, &mut r, id, DeckCommand::JogTouch(true));
    step(&mut c, &mut r, id, top(-0.5));
    render(&mut r, SR as usize);
    assert_eq!(c.deck(id).ghost_frames, None);
    let scratched = position(&c, id);
    step(&mut c, &mut r, id, DeckCommand::JogTouch(false));
    assert!((position(&c, id) - scratched - 1.0).abs() < 1e-6);

    // A hotcue jump keeps playing from the hotcue.
    step(
        &mut c,
        &mut r,
        id,
        DeckCommand::SetHotcue(slot(1), Some(Hotcue::point(5000.0))),
    );
    step(&mut c, &mut r, id, DeckCommand::HotcuePress(slot(1)));
    render(&mut r, 1000);
    step(&mut c, &mut r, id, DeckCommand::HotcueRelease(slot(1)));
    assert_eq!(position(&c, id), 5000.0 + 1002.0);
    assert!(c.deck(id).playing);

    // Exiting a loop keeps playing from inside it.
    step(&mut c, &mut r, id, DeckCommand::Seek(10.0 * BEAT));
    step(&mut c, &mut r, id, DeckCommand::AutoLoop);
    render(&mut r, 3 * SR as usize);
    let looped = position(&c, id);
    assert!((10.0 * BEAT..14.0 * BEAT).contains(&looped));
    step(&mut c, &mut r, id, DeckCommand::AutoLoop);
    assert!((position(&c, id) - looped - 1.0).abs() < 1e-6);
    assert_eq!(c.deck(id).ghost_frames, None);
}

#[test]
fn load_resets_slip_ghost() {
    let (mut c, mut r, id) = setup(100_000.0, true);
    step(&mut c, &mut r, id, DeckCommand::JogTouch(true));
    assert!(c.deck(id).ghost_frames.is_some());
    step(&mut c, &mut r, id, load());
    let s = c.deck(id);
    assert_eq!(s.ghost_frames, None);
    assert_eq!(s.position_frames, 0.0);
    assert!(!s.playing);
    assert!(s.slip, "slip is kept across loads");
}

#[test]
fn slip_hotcue_out_of_a_loop_leaves_it() {
    // The loop is active before slip is on, so it is not the slipping action.
    let (mut c, mut r, id) = setup(10.0 * BEAT, false);
    step(&mut c, &mut r, id, DeckCommand::AutoLoop);
    step(&mut c, &mut r, id, DeckCommand::SetSlip(true));
    step(
        &mut c,
        &mut r,
        id,
        DeckCommand::SetHotcue(slot(1), Some(Hotcue::point(5000.0))),
    );
    step(&mut c, &mut r, id, DeckCommand::HotcuePress(slot(1)));
    assert!(!c.deck(id).loop_active, "the jump left the loop");
    // The loop is gone, so the ghost continues linearly past its end.
    let g0 = ghost(&c, id);
    assert!((10.0 * BEAT..14.0 * BEAT).contains(&g0));
    render(&mut r, 5 * SR as usize);
    let g = ghost(&c, id);
    assert!((g - (g0 + 5.0 * f64::from(SR))).abs() < 1e-6, "{g}");
    step(&mut c, &mut r, id, DeckCommand::HotcueRelease(slot(1)));
    let s = c.deck(id);
    assert!(!s.loop_active, "the return never turns the loop back on");
    assert!((s.position_frames - (g + 1.0)).abs() <= 1e-6);
}

#[test]
fn a_loop_exited_during_a_scratch_stays_off() {
    let (mut c, mut r, id) = setup(10.0 * BEAT, false);
    step(&mut c, &mut r, id, DeckCommand::AutoLoop);
    step(&mut c, &mut r, id, DeckCommand::SetSlip(true));
    step(&mut c, &mut r, id, DeckCommand::JogTouch(true));
    step(&mut c, &mut r, id, top(-0.2));
    render(&mut r, 3 * SR as usize);
    assert!((10.0 * BEAT..14.0 * BEAT).contains(&ghost(&c, id)), "wraps");
    step(&mut c, &mut r, id, DeckCommand::ReloopExit);
    assert!(!c.deck(id).loop_active);
    let g1 = ghost(&c, id);
    render(&mut r, SR as usize);
    step(&mut c, &mut r, id, DeckCommand::JogTouch(false));
    let s = c.deck(id);
    assert!(!s.loop_active, "the loop stays off");
    let expected = g1 + 1.0 + f64::from(SR);
    assert!(expected > 14.0 * BEAT, "the ghost went on past the loop");
    assert!(
        (s.position_frames - expected).abs() <= 1e-6,
        "{}",
        s.position_frames
    );
}

#[test]
fn paused_scratch_does_not_slip() {
    let (mut c, mut r, id) = setup(100_000.0, true);
    step(&mut c, &mut r, id, DeckCommand::Pause);
    let start = position(&c, id);
    step(&mut c, &mut r, id, DeckCommand::JogTouch(true));
    assert_eq!(c.deck(id).ghost_frames, None);
    step(&mut c, &mut r, id, top(0.5));
    render(&mut r, SR as usize);
    step(&mut c, &mut r, id, DeckCommand::JogTouch(false));
    let expected = start + 0.5 * 1.8 * f64::from(SR);
    let pos = position(&c, id);
    assert!((pos - expected).abs() <= 1.0, "{pos} vs {expected}");
    assert!(!c.deck(id).playing);
}

#[test]
fn deliberate_jumps_drop_the_ghost() {
    // A seek during a held hotcue: the seek stands and the release does not return.
    let (mut c, mut r, id) = setup(100_000.0, true);
    step(
        &mut c,
        &mut r,
        id,
        DeckCommand::SetHotcue(slot(1), Some(Hotcue::point(5000.0))),
    );
    step(&mut c, &mut r, id, DeckCommand::HotcuePress(slot(1)));
    assert!(c.deck(id).ghost_frames.is_some());
    step(&mut c, &mut r, id, DeckCommand::Seek(200_000.0));
    assert_eq!(c.deck(id).ghost_frames, None);
    render(&mut r, 1000);
    step(&mut c, &mut r, id, DeckCommand::HotcueRelease(slot(1)));
    assert_eq!(position(&c, id), 200_000.0 + 1002.0);
    assert!(c.deck(id).playing);

    // A search during a scratch: the release stays where the scratch left the record.
    step(&mut c, &mut r, id, DeckCommand::JogTouch(true));
    assert!(c.deck(id).ghost_frames.is_some());
    step(&mut c, &mut r, id, DeckCommand::JogSearch { turns: 0.1 });
    assert_eq!(c.deck(id).ghost_frames, None);
    render(&mut r, SR as usize);
    let held = position(&c, id);
    step(&mut c, &mut r, id, DeckCommand::JogTouch(false));
    assert!((position(&c, id) - held - 1.0).abs() < 1e-6);
}
