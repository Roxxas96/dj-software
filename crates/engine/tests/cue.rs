mod common;

use common::{ramp_track, render};
use engine::{
    Command, DeckCommand, DeckId, EngineConfig, EngineController, EngineRenderer, engine,
};

const SR: u32 = 44_100;

fn setup() -> (EngineController, EngineRenderer, DeckId) {
    let (mut c, r) = engine(EngineConfig {
        sample_rate: SR,
        max_block_frames: 512,
    });
    let id = DeckId::new(0).unwrap();
    send(&mut c, id, DeckCommand::Load(ramp_track(SR, 100_000)));
    (c, r, id)
}

fn send(c: &mut EngineController, id: DeckId, cmd: DeckCommand) {
    c.send(Command::Deck(id, cmd)).unwrap();
}

/// Sends `cmd` and renders one block so the renderer applies it and publishes state.
fn step(c: &mut EngineController, r: &mut EngineRenderer, id: DeckId, cmd: DeckCommand) {
    send(c, id, cmd);
    render(r, 1);
}

#[test]
fn cue_while_paused_sets_cue_point() {
    let (mut c, mut r, id) = setup();
    step(&mut c, &mut r, id, DeckCommand::Seek(500.0));
    step(&mut c, &mut r, id, DeckCommand::CuePress);
    let s = c.deck(id);
    assert_eq!(s.cue_frames, 500.0);
    assert!(!s.playing);
}

#[test]
fn cue_held_at_cue_point_previews() {
    let (mut c, mut r, id) = setup();
    step(&mut c, &mut r, id, DeckCommand::Seek(500.0));
    step(&mut c, &mut r, id, DeckCommand::CuePress);
    step(&mut c, &mut r, id, DeckCommand::CuePress);
    assert!(c.deck(id).playing);
    render(&mut r, 100);
    assert!(c.deck(id).position_frames > 500.0);
    step(&mut c, &mut r, id, DeckCommand::CueRelease);
    let s = c.deck(id);
    assert_eq!(s.position_frames, 500.0);
    assert!(!s.playing);
}

#[test]
fn play_during_preview_keeps_playing() {
    let (mut c, mut r, id) = setup();
    step(&mut c, &mut r, id, DeckCommand::Seek(500.0));
    step(&mut c, &mut r, id, DeckCommand::CuePress);
    step(&mut c, &mut r, id, DeckCommand::CuePress);
    step(&mut c, &mut r, id, DeckCommand::Play);
    step(&mut c, &mut r, id, DeckCommand::CueRelease);
    render(&mut r, 100);
    let s = c.deck(id);
    assert!(s.playing);
    assert!(s.position_frames > 500.0);
}

#[test]
fn cue_while_playing_returns_and_pauses() {
    let (mut c, mut r, id) = setup();
    step(&mut c, &mut r, id, DeckCommand::Seek(500.0));
    step(&mut c, &mut r, id, DeckCommand::CuePress);
    step(&mut c, &mut r, id, DeckCommand::Play);
    render(&mut r, 1000);
    assert!(c.deck(id).position_frames > 1400.0);
    step(&mut c, &mut r, id, DeckCommand::CuePress);
    let s = c.deck(id);
    assert_eq!(s.position_frames, 500.0);
    assert!(!s.playing);
}

#[test]
fn cue_release_while_playing_is_ignored() {
    let (mut c, mut r, id) = setup();
    step(&mut c, &mut r, id, DeckCommand::Play);
    step(&mut c, &mut r, id, DeckCommand::CueRelease);
    render(&mut r, 100);
    let s = c.deck(id);
    assert!(s.playing);
    assert!(s.position_frames > 0.0);
}

#[test]
fn load_resets_cue_to_zero() {
    let (mut c, mut r, id) = setup();
    step(&mut c, &mut r, id, DeckCommand::Seek(500.0));
    step(&mut c, &mut r, id, DeckCommand::CuePress);
    assert_eq!(c.deck(id).cue_frames, 500.0);
    step(&mut c, &mut r, id, DeckCommand::Load(ramp_track(SR, 1000)));
    assert_eq!(c.deck(id).cue_frames, 0.0);
}

fn start_preview(c: &mut EngineController, r: &mut EngineRenderer, id: DeckId) {
    step(c, r, id, DeckCommand::Seek(500.0));
    step(c, r, id, DeckCommand::CuePress);
    step(c, r, id, DeckCommand::CuePress);
    let s = c.deck(id);
    assert!(s.playing && s.previewing);
}

#[test]
fn toggle_play_during_preview_keeps_playing() {
    let (mut c, mut r, id) = setup();
    start_preview(&mut c, &mut r, id);
    step(&mut c, &mut r, id, DeckCommand::TogglePlay);
    assert!(c.deck(id).playing);
    step(&mut c, &mut r, id, DeckCommand::CueRelease);
    render(&mut r, 100);
    let s = c.deck(id);
    assert!(s.playing);
    assert!(!s.previewing);
    assert!(s.position_frames > 500.0);
}

#[test]
fn pause_during_preview_stops_and_clears_preview() {
    let (mut c, mut r, id) = setup();
    start_preview(&mut c, &mut r, id);
    render(&mut r, 100);
    step(&mut c, &mut r, id, DeckCommand::Pause);
    let paused_at = c.deck(id).position_frames;
    let s = c.deck(id);
    assert!(!s.playing);
    assert!(!s.previewing);
    assert!(paused_at > 500.0);
    step(&mut c, &mut r, id, DeckCommand::CueRelease);
    render(&mut r, 100);
    let s = c.deck(id);
    assert!(!s.playing);
    assert!(!s.previewing);
    assert_eq!(s.position_frames, paused_at);
}

#[test]
fn load_during_preview_resets() {
    let (mut c, mut r, id) = setup();
    start_preview(&mut c, &mut r, id);
    step(
        &mut c,
        &mut r,
        id,
        DeckCommand::Load(common::ramp_track(SR, 50_000)),
    );
    let s = c.deck(id);
    assert!(!s.playing);
    assert!(!s.previewing);
    assert_eq!(s.position_frames, 0.0);
    assert_eq!(s.cue_frames, 0.0);
    step(&mut c, &mut r, id, DeckCommand::CueRelease);
    render(&mut r, 100);
    let s = c.deck(id);
    assert!(!s.playing);
    assert_eq!(s.position_frames, 0.0);
}
