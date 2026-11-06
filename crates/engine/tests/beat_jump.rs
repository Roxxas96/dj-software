mod common;

use common::{ramp_track, render};
use engine::{
    Command, DeckCommand, DeckId, EngineConfig, EngineController, EngineRenderer, Grid,
    TrackMemory, engine,
};

const SR: u32 = 44_100;
const FRAMES: usize = 600_000;
const BLOCK: usize = 512;
/// Frames in one beat at 120 BPM.
const BEAT: f64 = 22_050.0;

fn setup_with(grid: Option<Grid>) -> (EngineController, EngineRenderer, DeckId) {
    let (mut c, mut r) = engine(EngineConfig {
        sample_rate: SR,
        max_block_frames: BLOCK,
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
    render(&mut r, 1);
    (c, r, id)
}

fn setup() -> (EngineController, EngineRenderer, DeckId) {
    setup_with(Grid::from_bpm(120.0, 0.0, SR))
}

/// Sends `cmd` and renders one block so the renderer applies it and publishes state.
fn step(c: &mut EngineController, r: &mut EngineRenderer, id: DeckId, cmd: DeckCommand) {
    c.send(Command::Deck(id, cmd)).unwrap();
    render(r, 1);
}

fn jump(beats: f64) -> DeckCommand {
    DeckCommand::BeatJump { beats }
}

#[test]
fn beat_jump_moves_by_whole_beats() {
    let (mut c, mut r, id) = setup();
    step(&mut c, &mut r, id, DeckCommand::Seek(100_000.0));
    step(&mut c, &mut r, id, jump(4.0));
    assert_eq!(c.deck(id).position_frames, 100_000.0 + 88_200.0);
    step(&mut c, &mut r, id, jump(-1.0));
    assert_eq!(c.deck(id).position_frames, 100_000.0 + 88_200.0 - BEAT);
    step(&mut c, &mut r, id, jump(0.5));
    assert_eq!(
        c.deck(id).position_frames,
        100_000.0 + 88_200.0 - BEAT + BEAT / 2.0
    );
}

#[test]
fn beat_jump_moves_an_active_loop_with_it() {
    let (mut c, mut r, id) = setup();
    step(&mut c, &mut r, id, DeckCommand::Seek(30_000.0));
    step(&mut c, &mut r, id, DeckCommand::AutoLoop);
    let (start, end) = (c.deck(id).loop_in.unwrap(), c.deck(id).loop_out.unwrap());
    step(&mut c, &mut r, id, jump(4.0));
    let s = c.deck(id);
    assert!(s.loop_active);
    assert_eq!(s.loop_in, Some(start + 4.0 * BEAT));
    assert_eq!(s.loop_out, Some(end + 4.0 * BEAT));
    assert_eq!(s.position_frames, 30_000.0 + 4.0 * BEAT);
    // A jump that would push the loop past the track end exits it; the playhead jumps alone.
    step(&mut c, &mut r, id, jump(-64.0));
    let s = c.deck(id);
    assert!(!s.loop_active);
    assert_eq!(s.position_frames, 0.0);
}

#[test]
fn beat_jump_moves_a_running_slip_ghost() {
    let (mut c, mut r, id) = setup();
    step(&mut c, &mut r, id, DeckCommand::Seek(100_000.0));
    step(&mut c, &mut r, id, DeckCommand::SetSlip(true));
    step(&mut c, &mut r, id, DeckCommand::Play);
    step(&mut c, &mut r, id, DeckCommand::AutoLoop);
    step(&mut c, &mut r, id, jump(2.0));
    let s = c.deck(id);
    assert!(s.loop_active);
    let ghost = s.ghost_frames.expect("the ghost survives a beat jump");
    let loop_in = s.loop_in.unwrap();
    assert!(ghost >= loop_in && ghost < s.loop_out.unwrap());
}

#[test]
fn beat_jump_clamps_and_needs_a_grid() {
    let (mut c, mut r, id) = setup();
    step(&mut c, &mut r, id, DeckCommand::Seek(1000.0));
    step(&mut c, &mut r, id, jump(-8.0));
    assert_eq!(c.deck(id).position_frames, 0.0);
    step(&mut c, &mut r, id, jump(1000.0));
    assert_eq!(c.deck(id).position_frames, FRAMES as f64);
    step(&mut c, &mut r, id, DeckCommand::Seek(5000.0));
    step(&mut c, &mut r, id, jump(f64::NAN));
    step(&mut c, &mut r, id, jump(f64::INFINITY));
    assert_eq!(c.deck(id).position_frames, 5000.0);

    let (mut c, mut r, id) = setup_with(None);
    step(&mut c, &mut r, id, DeckCommand::Seek(5000.0));
    step(&mut c, &mut r, id, jump(4.0));
    assert_eq!(c.deck(id).position_frames, 5000.0);
}

#[test]
fn beat_jump_drops_a_pending_sync() {
    let (mut c, mut r) = engine(EngineConfig {
        sample_rate: SR,
        max_block_frames: BLOCK,
    });
    let (master, deck) = (DeckId::new(0).unwrap(), DeckId::new(1).unwrap());
    for id in [master, deck] {
        c.send(Command::Deck(
            id,
            DeckCommand::Load {
                track: ramp_track(SR, FRAMES),
                memory: TrackMemory {
                    grid: Grid::from_bpm(120.0, 0.0, SR),
                    ..TrackMemory::default()
                },
            },
        ))
        .unwrap();
    }
    for (id, cmd) in [
        (master, DeckCommand::Seek(BEAT * 4.25)),
        (deck, DeckCommand::Seek(BEAT * 4.75)),
        (master, DeckCommand::Play),
    ] {
        c.send(Command::Deck(id, cmd)).unwrap();
    }
    render(&mut r, BLOCK);
    c.send(Command::PhaseSync {
        deck,
        master,
        beat_ratio: 1.0,
    })
    .unwrap();
    step(&mut c, &mut r, deck, jump(2.0));
    step(&mut c, &mut r, deck, DeckCommand::Play);
    // No phase correction on play: the deck advanced only by the frame rendered since.
    let expected = BEAT * 6.75 + 1.0;
    assert_eq!(c.deck(deck).position_frames, expected);
}
