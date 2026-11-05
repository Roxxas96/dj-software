mod common;

use common::{ramp_track, render};
use engine::{
    Command, DeckCommand, DeckId, EngineConfig, EngineController, EngineRenderer, HotcueSlot,
    TrackMemory, engine,
};

const SR: u32 = 44_100;

fn slot(n: u8) -> HotcueSlot {
    HotcueSlot::new(n).unwrap()
}

fn setup() -> (EngineController, EngineRenderer, DeckId) {
    let (mut c, r) = engine(EngineConfig {
        sample_rate: SR,
        max_block_frames: 512,
    });
    let id = DeckId::new(0).unwrap();
    c.send(Command::Deck(
        id,
        DeckCommand::Load {
            track: ramp_track(SR, 100_000),
            memory: TrackMemory::default(),
        },
    ))
    .unwrap();
    (c, r, id)
}

/// Sends `cmd` and renders one block so the renderer applies it and publishes state.
fn step(c: &mut EngineController, r: &mut EngineRenderer, id: DeckId, cmd: DeckCommand) {
    c.send(Command::Deck(id, cmd)).unwrap();
    render(r, 1);
}

#[test]
fn press_on_empty_slot_sets_at_playhead() {
    let (mut c, mut r, id) = setup();
    step(&mut c, &mut r, id, DeckCommand::Seek(700.0));
    step(&mut c, &mut r, id, DeckCommand::HotcuePress(slot(3)));
    let s = c.deck(id);
    assert_eq!(s.hotcues[2], Some(700.0));
    assert_eq!(s.hotcues.iter().flatten().count(), 1);
    assert_eq!(s.position_frames, 700.0);
    assert!(!s.playing);
    assert!(!s.previewing);
}

#[test]
fn press_while_playing_jumps_and_keeps_playing() {
    let (mut c, mut r, id) = setup();
    step(
        &mut c,
        &mut r,
        id,
        DeckCommand::SetHotcue(slot(1), Some(5000.0)),
    );
    step(&mut c, &mut r, id, DeckCommand::Play);
    step(&mut c, &mut r, id, DeckCommand::HotcuePress(slot(1)));
    let s = c.deck(id);
    assert!(s.playing);
    assert!(!s.previewing);
    assert!((5000.0..5000.0 + 1024.0).contains(&s.position_frames));
    step(&mut c, &mut r, id, DeckCommand::HotcueRelease(slot(1)));
    assert!(c.deck(id).playing);
}

#[test]
fn press_while_paused_previews_until_release() {
    let (mut c, mut r, id) = setup();
    step(
        &mut c,
        &mut r,
        id,
        DeckCommand::SetHotcue(slot(2), Some(5000.0)),
    );
    step(&mut c, &mut r, id, DeckCommand::SetCue(100.0));
    step(&mut c, &mut r, id, DeckCommand::HotcuePress(slot(2)));
    let s = c.deck(id);
    assert!(s.playing && s.previewing);
    render(&mut r, 100);
    assert!(c.deck(id).position_frames > 5000.0);
    step(&mut c, &mut r, id, DeckCommand::HotcueRelease(slot(2)));
    let s = c.deck(id);
    assert_eq!(s.position_frames, 5000.0);
    assert!(!s.playing && !s.previewing);
    assert_eq!(s.cue_frames, 100.0, "the cue point is untouched");
}

#[test]
fn play_during_hotcue_preview_latches() {
    let (mut c, mut r, id) = setup();
    step(
        &mut c,
        &mut r,
        id,
        DeckCommand::SetHotcue(slot(2), Some(5000.0)),
    );
    step(&mut c, &mut r, id, DeckCommand::HotcuePress(slot(2)));
    step(&mut c, &mut r, id, DeckCommand::TogglePlay);
    step(&mut c, &mut r, id, DeckCommand::HotcueRelease(slot(2)));
    let s = c.deck(id);
    assert!(s.playing && !s.previewing);
    assert!(s.position_frames > 5000.0);
}

#[test]
fn delete_clears_and_ends_its_preview() {
    let (mut c, mut r, id) = setup();
    step(
        &mut c,
        &mut r,
        id,
        DeckCommand::SetHotcue(slot(4), Some(5000.0)),
    );
    step(&mut c, &mut r, id, DeckCommand::HotcuePress(slot(4)));
    step(&mut c, &mut r, id, DeckCommand::HotcueDelete(slot(4)));
    let s = c.deck(id);
    assert_eq!(s.hotcues[3], None);
    assert_eq!(s.position_frames, 5000.0);
    assert!(!s.playing && !s.previewing);

    // Deleting a slot that is not previewing leaves the preview alone.
    step(
        &mut c,
        &mut r,
        id,
        DeckCommand::SetHotcue(slot(4), Some(5000.0)),
    );
    step(
        &mut c,
        &mut r,
        id,
        DeckCommand::SetHotcue(slot(5), Some(9000.0)),
    );
    step(&mut c, &mut r, id, DeckCommand::HotcuePress(slot(4)));
    step(&mut c, &mut r, id, DeckCommand::HotcueDelete(slot(5)));
    let s = c.deck(id);
    assert!(s.playing && s.previewing);
    assert_eq!(s.hotcues[4], None);
}

#[test]
fn release_of_another_slot_is_ignored() {
    let (mut c, mut r, id) = setup();
    step(
        &mut c,
        &mut r,
        id,
        DeckCommand::SetHotcue(slot(1), Some(1000.0)),
    );
    step(
        &mut c,
        &mut r,
        id,
        DeckCommand::SetHotcue(slot(2), Some(5000.0)),
    );
    step(&mut c, &mut r, id, DeckCommand::HotcuePress(slot(1)));
    step(&mut c, &mut r, id, DeckCommand::HotcueRelease(slot(2)));
    assert!(c.deck(id).previewing);
    // CUE release does not end a hotcue preview either.
    step(&mut c, &mut r, id, DeckCommand::CueRelease);
    assert!(c.deck(id).previewing);
    // A second hotcue pressed during the hold takes over the preview.
    step(&mut c, &mut r, id, DeckCommand::HotcuePress(slot(2)));
    step(&mut c, &mut r, id, DeckCommand::HotcueRelease(slot(1)));
    assert!(c.deck(id).previewing);
    step(&mut c, &mut r, id, DeckCommand::HotcueRelease(slot(2)));
    let s = c.deck(id);
    assert_eq!(s.position_frames, 5000.0);
    assert!(!s.playing && !s.previewing);
}

#[test]
fn set_hotcue_restores_and_clamps() {
    let (mut c, mut r, id) = setup();
    step(&mut c, &mut r, id, DeckCommand::Seek(300.0));
    step(
        &mut c,
        &mut r,
        id,
        DeckCommand::SetHotcue(slot(1), Some(1234.0)),
    );
    step(
        &mut c,
        &mut r,
        id,
        DeckCommand::SetHotcue(slot(2), Some(-5.0)),
    );
    step(
        &mut c,
        &mut r,
        id,
        DeckCommand::SetHotcue(slot(3), Some(1e9)),
    );
    step(
        &mut c,
        &mut r,
        id,
        DeckCommand::SetHotcue(slot(4), Some(f64::NAN)),
    );
    let s = c.deck(id);
    assert_eq!(
        s.hotcues[..4],
        [Some(1234.0), Some(0.0), Some(100_000.0), None]
    );
    assert_eq!(s.position_frames, 300.0);
    assert!(!s.playing);
    step(&mut c, &mut r, id, DeckCommand::SetHotcue(slot(1), None));
    assert_eq!(c.deck(id).hotcues[0], None);
}

#[test]
fn load_resets_hotcue_preview() {
    let (mut c, mut r, id) = setup();
    step(
        &mut c,
        &mut r,
        id,
        DeckCommand::SetHotcue(slot(1), Some(1000.0)),
    );
    step(&mut c, &mut r, id, DeckCommand::HotcuePress(slot(1)));
    assert!(c.deck(id).previewing);
    step(
        &mut c,
        &mut r,
        id,
        DeckCommand::Load {
            track: ramp_track(SR, 50_000),
            memory: TrackMemory::default(),
        },
    );
    let s = c.deck(id);
    assert!(!s.previewing && !s.playing);
    assert_eq!(s.hotcues, [None; 8]);
    // A stray release afterwards does nothing.
    step(&mut c, &mut r, id, DeckCommand::HotcueRelease(slot(1)));
    assert_eq!(c.deck(id).position_frames, 0.0);
}
