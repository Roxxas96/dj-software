mod common;

use common::{ramp_track, render};
use engine::{
    Command, DeckCommand, DeckId, EngineConfig, EngineController, EngineRenderer, Grid,
    TrackMemory, engine,
};

const SR: u32 = 44_100;
const BLOCK: usize = 512;
/// Frames per beat at 120 BPM.
const PERIOD: f64 = 22_050.0;

fn grid() -> Option<Grid> {
    Grid::from_bpm(120.0, 0.0, SR)
}

fn load(c: &mut EngineController, id: DeckId, grid: Option<Grid>) {
    c.send(Command::Deck(
        id,
        DeckCommand::Load {
            track: ramp_track(SR, 400_000),
            memory: TrackMemory {
                grid,
                ..TrackMemory::default()
            },
        },
    ))
    .unwrap();
}

fn send(c: &mut EngineController, id: DeckId, cmd: DeckCommand) {
    c.send(Command::Deck(id, cmd)).unwrap();
}

fn sync(c: &mut EngineController, deck: DeckId, master: DeckId, beat_ratio: f64) {
    c.send(Command::PhaseSync {
        deck,
        master,
        beat_ratio,
    })
    .unwrap();
}

/// Master (deck 0) playing at beat phase 0.25 and deck 1 paused at phase 0.75, one beat in.
fn setup(deck_grid: Option<Grid>) -> (EngineController, EngineRenderer, DeckId, DeckId) {
    let (mut c, mut r) = engine(EngineConfig {
        sample_rate: SR,
        max_block_frames: BLOCK,
    });
    let (master, deck) = (DeckId::new(0).unwrap(), DeckId::new(1).unwrap());
    load(&mut c, master, grid());
    load(&mut c, deck, deck_grid);
    send(&mut c, master, DeckCommand::Seek(PERIOD * 4.25));
    send(&mut c, deck, DeckCommand::Seek(PERIOD * 4.75));
    send(&mut c, master, DeckCommand::Play);
    render(&mut r, BLOCK);
    (c, r, master, deck)
}

/// Distance in beats between `deck`'s phase and the master's phase times `ratio`, with both
/// positions taken before the last block advanced them by `BLOCK` frames.
fn phase_error(c: &EngineController, deck: DeckId, master: DeckId, ratio: f64) -> f64 {
    let g = grid().unwrap();
    let d = g.beats_at(c.deck(deck).position_frames - BLOCK as f64)
        - g.beats_at(c.deck(master).position_frames - BLOCK as f64) * ratio;
    (d - d.round()).abs()
}

#[test]
fn phase_sync_aligns_beats() {
    let (mut c, mut r, master, deck) = setup(grid());
    send(&mut c, deck, DeckCommand::Play);
    render(&mut r, BLOCK);
    let before = c.deck(deck).position_frames;
    sync(&mut c, deck, master, 1.0);
    render(&mut r, BLOCK);
    assert!(phase_error(&c, deck, master, 1.0) < 1e-6);
    let moved = c.deck(deck).position_frames - before - BLOCK as f64;
    assert!(moved.abs() <= PERIOD / 2.0 + 1e-6, "moved {moved}");
    assert!(moved.abs() > 1.0, "moved {moved}");
}

#[test]
fn phase_sync_with_double_ratio() {
    let (mut c, mut r, master, deck) = setup(grid());
    send(&mut c, deck, DeckCommand::Play);
    render(&mut r, BLOCK);
    let before = c.deck(deck).position_frames;
    sync(&mut c, deck, master, 2.0);
    render(&mut r, BLOCK);
    assert!(phase_error(&c, deck, master, 2.0) < 1e-6);
    let moved = c.deck(deck).position_frames - before - BLOCK as f64;
    // Target phase 0.5 against 0.75 at the previous block's end.
    assert!(moved.abs() <= PERIOD / 2.0 + 1e-6, "moved {moved}");
    assert!(moved.abs() > 1.0);
}

#[test]
fn phase_sync_on_paused_deck_happens_on_play() {
    let (mut c, mut r, master, deck) = setup(grid());
    let start = c.deck(deck).position_frames;
    sync(&mut c, deck, master, 1.0);
    render(&mut r, BLOCK * 3);
    assert_eq!(c.deck(deck).position_frames, start);
    send(&mut c, deck, DeckCommand::Play);
    render(&mut r, BLOCK);
    assert!(c.deck(deck).playing);
    assert!(phase_error(&c, deck, master, 1.0) < 1e-6);
}

#[test]
fn phase_sync_without_grid_or_playing_master_does_nothing() {
    // No grid on the deck.
    let (mut c, mut r, master, deck) = setup(None);
    send(&mut c, deck, DeckCommand::Play);
    sync(&mut c, deck, master, 1.0);
    render(&mut r, BLOCK);
    assert_eq!(c.deck(deck).position_frames, PERIOD * 4.75 + BLOCK as f64);

    // Master not playing: nothing now and nothing remembered for later.
    let (mut c, mut r, master, deck) = setup(grid());
    send(&mut c, master, DeckCommand::Pause);
    sync(&mut c, deck, master, 1.0);
    send(&mut c, deck, DeckCommand::Play);
    render(&mut r, BLOCK);
    assert_eq!(c.deck(deck).position_frames, PERIOD * 4.75 + BLOCK as f64);

    // Invalid ratios and syncing a deck to itself.
    let (mut c, mut r, master, deck) = setup(grid());
    for ratio in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        sync(&mut c, deck, master, ratio);
    }
    sync(&mut c, deck, deck, 1.0);
    send(&mut c, deck, DeckCommand::Play);
    render(&mut r, BLOCK);
    assert_eq!(c.deck(deck).position_frames, PERIOD * 4.75 + BLOCK as f64);
}

#[test]
fn pending_sync_dropped_by_seek() {
    let (mut c, mut r, master, deck) = setup(grid());
    sync(&mut c, deck, master, 1.0);
    send(&mut c, deck, DeckCommand::Seek(PERIOD * 6.75));
    send(&mut c, deck, DeckCommand::Play);
    render(&mut r, BLOCK);
    assert_eq!(c.deck(deck).position_frames, PERIOD * 6.75 + BLOCK as f64);
}

#[test]
fn pending_sync_dropped_when_master_stops() {
    let (mut c, mut r, master, deck) = setup(grid());
    sync(&mut c, deck, master, 1.0);
    // Stops and restarts before the next render: still a stop.
    send(&mut c, master, DeckCommand::Pause);
    send(&mut c, master, DeckCommand::Play);
    send(&mut c, deck, DeckCommand::Play);
    render(&mut r, BLOCK);
    assert_eq!(c.deck(deck).position_frames, PERIOD * 4.75 + BLOCK as f64);
}

#[test]
fn cue_preview_keeps_a_pending_sync_until_play() {
    let (mut c, mut r, master, deck) = setup(grid());
    let cue = PERIOD * 4.75;
    send(&mut c, deck, DeckCommand::SetCue(cue));
    sync(&mut c, deck, master, 1.0);
    render(&mut r, BLOCK);

    // Holding CUE previews from the cue point without aligning; the release returns there.
    send(&mut c, deck, DeckCommand::CuePress);
    render(&mut r, BLOCK);
    let snap = c.deck(deck);
    assert!(snap.previewing);
    assert_eq!(
        snap.position_frames,
        cue + BLOCK as f64,
        "the preview is not moved"
    );
    send(&mut c, deck, DeckCommand::CueRelease);
    render(&mut r, BLOCK);
    assert_eq!(c.deck(deck).position_frames, cue, "no move");

    // PLAY then aligns.
    send(&mut c, deck, DeckCommand::Play);
    render(&mut r, BLOCK);
    assert!(phase_error(&c, deck, master, 1.0) < 1e-6);
}

#[test]
fn play_latched_during_a_preview_resolves_the_sync() {
    let (mut c, mut r, master, deck) = setup(grid());
    send(&mut c, deck, DeckCommand::SetCue(PERIOD * 4.75));
    sync(&mut c, deck, master, 1.0);
    send(&mut c, deck, DeckCommand::CuePress);
    render(&mut r, BLOCK);
    // PLAY during the hold latches: the deck now plays for real and aligns.
    send(&mut c, deck, DeckCommand::Play);
    render(&mut r, BLOCK);
    assert!(phase_error(&c, deck, master, 1.0) < 1e-6);
    send(&mut c, deck, DeckCommand::CueRelease);
    render(&mut r, BLOCK);
    assert!(c.deck(deck).playing);
    assert!(phase_error(&c, deck, master, 1.0) < 1e-6);
}

#[test]
fn slipped_loop_returns_in_phase_after_a_sync() {
    let (mut c, mut r, master, deck) = setup(grid());
    send(&mut c, deck, DeckCommand::SetSlip(true));
    send(&mut c, deck, DeckCommand::Play);
    send(&mut c, deck, DeckCommand::AutoLoop);
    render(&mut r, BLOCK);
    assert!(c.deck(deck).ghost_frames.is_some(), "the loop slips");
    sync(&mut c, deck, master, 1.0);
    render(&mut r, BLOCK);
    assert!(c.deck(deck).loop_active, "the sync stays inside the loop");

    // Exiting returns to the ghost, which the sync moved along with the playhead.
    send(&mut c, deck, DeckCommand::AutoLoop);
    render(&mut r, BLOCK);
    assert!(!c.deck(deck).loop_active);
    assert!(c.deck(deck).ghost_frames.is_none());
    let error = phase_error(&c, deck, master, 1.0);
    assert!(error < 1e-6, "phase error {error}");
}

#[test]
fn pending_sync_moves_the_slip_ghost_too() {
    let (mut c, mut r, master, deck) = setup(grid());
    send(&mut c, deck, DeckCommand::SetSlip(true));
    send(&mut c, deck, DeckCommand::AutoLoop);
    sync(&mut c, deck, master, 1.0);
    render(&mut r, BLOCK);
    assert!(c.deck(deck).ghost_frames.is_some(), "the loop slips");

    // The sync waits for PLAY, then aligns the playhead and the ghost.
    send(&mut c, deck, DeckCommand::Play);
    render(&mut r, BLOCK);
    send(&mut c, deck, DeckCommand::AutoLoop);
    render(&mut r, BLOCK);
    assert!(c.deck(deck).ghost_frames.is_none());
    let error = phase_error(&c, deck, master, 1.0);
    assert!(error < 1e-6, "phase error {error}");
}

#[test]
fn held_roll_returns_in_phase_after_a_sync() {
    let (mut c, mut r, master, deck) = setup(grid());
    send(&mut c, deck, DeckCommand::Play);
    render(&mut r, BLOCK);
    send(&mut c, deck, DeckCommand::RollPress { beats: 1.0 });
    render(&mut r, BLOCK);
    assert!(c.deck(deck).roll_active);
    sync(&mut c, deck, master, 1.0);
    render(&mut r, BLOCK);

    send(&mut c, deck, DeckCommand::RollRelease { beats: 1.0 });
    render(&mut r, BLOCK);
    assert!(!c.deck(deck).roll_active);
    let error = phase_error(&c, deck, master, 1.0);
    assert!(error < 1e-6, "phase error {error}");
}
