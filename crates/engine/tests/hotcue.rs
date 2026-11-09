mod common;

use common::{ramp_track, render};
use engine::{
    Command, DeckCommand, DeckId, EngineConfig, EngineController, EngineRenderer, Grid, HOTCUES,
    Hotcue, HotcueSlot, TrackMemory, engine,
};

const SR: u32 = 44_100;

fn slot(n: u8) -> HotcueSlot {
    HotcueSlot::new(n).unwrap()
}

fn setup() -> (EngineController, EngineRenderer, DeckId) {
    setup_with(100_000, TrackMemory::default())
}

/// A paused deck with a track of `frames` frames and `memory`.
fn setup_with(frames: usize, memory: TrackMemory) -> (EngineController, EngineRenderer, DeckId) {
    let (mut c, r) = engine(EngineConfig {
        sample_rate: SR,
        max_block_frames: 512,
    });
    let id = DeckId::new(0).unwrap();
    c.send(Command::Deck(
        id,
        DeckCommand::Load {
            track: ramp_track(SR, frames),
            memory,
        },
    ))
    .unwrap();
    (c, r, id)
}

/// Frames of the loop tests' track: 10 s.
const LOOP_TRACK: usize = 441_000;

/// A paused deck with a 10 s track and a 120 BPM grid from frame 0.
fn setup_with_grid() -> (EngineController, EngineRenderer, DeckId) {
    setup_with(
        LOOP_TRACK,
        TrackMemory {
            grid: Grid::from_bpm(120.0, 0.0, SR),
            ..TrackMemory::default()
        },
    )
}

fn point(frames: f64) -> Option<Hotcue> {
    Some(Hotcue::point(frames))
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
    assert_eq!(s.hotcues[2], point(700.0));
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
        DeckCommand::SetHotcue(slot(1), point(5000.0)),
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
        DeckCommand::SetHotcue(slot(2), point(5000.0)),
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
        DeckCommand::SetHotcue(slot(2), point(5000.0)),
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
        DeckCommand::SetHotcue(slot(4), point(5000.0)),
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
        DeckCommand::SetHotcue(slot(4), point(5000.0)),
    );
    step(
        &mut c,
        &mut r,
        id,
        DeckCommand::SetHotcue(slot(5), point(9000.0)),
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
        DeckCommand::SetHotcue(slot(1), point(1000.0)),
    );
    step(
        &mut c,
        &mut r,
        id,
        DeckCommand::SetHotcue(slot(2), point(5000.0)),
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
        DeckCommand::SetHotcue(slot(1), point(1234.0)),
    );
    step(
        &mut c,
        &mut r,
        id,
        DeckCommand::SetHotcue(slot(2), point(-5.0)),
    );
    step(
        &mut c,
        &mut r,
        id,
        DeckCommand::SetHotcue(slot(3), point(1e9)),
    );
    step(
        &mut c,
        &mut r,
        id,
        DeckCommand::SetHotcue(slot(4), point(f64::NAN)),
    );
    let s = c.deck(id);
    assert_eq!(
        s.hotcues[..4],
        [point(1234.0), point(0.0), point(100_000.0), None]
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
        DeckCommand::SetHotcue(slot(1), point(1000.0)),
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

/// Frames in one beat of the 120 BPM grid.
const BEAT: f64 = SR as f64 / 2.0;

#[test]
fn an_empty_pad_during_a_loop_saves_the_loop() {
    let (mut c, mut r, id) = setup_with_grid();
    step(&mut c, &mut r, id, DeckCommand::Seek(50_000.0));
    step(&mut c, &mut r, id, DeckCommand::AutoLoopBeats(4.0));
    let before = c.deck(id);
    let (loop_in, loop_out) = (before.loop_in.unwrap(), before.loop_out.unwrap());
    assert_eq!((loop_in, loop_out), (2.0 * BEAT, 6.0 * BEAT));
    step(&mut c, &mut r, id, DeckCommand::HotcuePress(slot(2)));
    let s = c.deck(id);
    assert_eq!(
        s.hotcues[1],
        Some(Hotcue {
            frames: loop_in,
            loop_frames: Some(loop_out - loop_in),
        })
    );
    assert!(s.loop_active);
    assert_eq!(s.position_frames, 50_000.0);
    assert!(!s.playing && !s.previewing);
}

#[test]
fn an_empty_pad_without_a_loop_saves_a_hotcue() {
    let (mut c, mut r, id) = setup_with_grid();
    step(&mut c, &mut r, id, DeckCommand::Seek(50_000.0));
    step(&mut c, &mut r, id, DeckCommand::AutoLoopBeats(4.0));
    step(&mut c, &mut r, id, DeckCommand::LoopExit);
    step(&mut c, &mut r, id, DeckCommand::HotcuePress(slot(2)));
    let s = c.deck(id);
    assert_eq!(
        s.hotcues[1],
        point(50_000.0),
        "a stored inactive loop is not saved"
    );
    assert!(!s.loop_active);
}

#[test]
fn a_roll_is_not_saved_as_a_loop() {
    let (mut c, mut r, id) = setup_with_grid();
    step(&mut c, &mut r, id, DeckCommand::Seek(50_000.0));
    step(&mut c, &mut r, id, DeckCommand::AutoLoopBeats(4.0));
    step(&mut c, &mut r, id, DeckCommand::Play);
    let held = [
        ("roll", DeckCommand::RollPress { beats: 1.0 }),
        ("slice", DeckCommand::SlicePress(3)),
    ];
    for (n, (name, press)) in (3..).zip(held) {
        step(&mut c, &mut r, id, press);
        assert!(c.deck(id).roll_active);
        let at = c.deck(id).position_frames;
        step(&mut c, &mut r, id, DeckCommand::HotcuePress(slot(n)));
        let s = c.deck(id);
        assert_eq!(s.hotcues[usize::from(n) - 1], point(at), "{name}");
        assert!(s.roll_active && s.loop_active);
    }
}

#[test]
fn a_loop_slot_jumps_and_loops() {
    let (mut c, mut r, id) = setup_with_grid();
    let saved = Hotcue {
        frames: 44_100.0,
        loop_frames: Some(88_200.0),
    };
    step(
        &mut c,
        &mut r,
        id,
        DeckCommand::SetHotcue(slot(3), Some(saved)),
    );
    step(&mut c, &mut r, id, DeckCommand::Seek(300_000.0));
    step(&mut c, &mut r, id, DeckCommand::Play);
    c.send(Command::Deck(id, DeckCommand::HotcuePress(slot(3))))
        .unwrap();
    render(&mut r, 1);
    let s = c.deck(id);
    assert_eq!(s.position_frames, 44_101.0);
    assert_eq!(s.loop_in, Some(44_100.0));
    assert_eq!(s.loop_out, Some(132_300.0));
    assert!(s.loop_active && s.playing && !s.previewing);
    assert_eq!(s.loop_beats, 4.0, "the auto loop length is untouched");
    assert_eq!(s.hotcues[2], Some(saved));
    step(&mut c, &mut r, id, DeckCommand::HotcueRelease(slot(3)));
    for _ in 0..30 {
        render(&mut r, SR as usize / 10);
        let s = c.deck(id);
        assert!(
            (44_100.0..132_300.0).contains(&s.position_frames),
            "{}",
            s.position_frames
        );
        assert!(s.loop_active);
    }
}

#[test]
fn a_loop_slot_while_paused_previews_inside_the_loop() {
    let (mut c, mut r, id) = setup_with_grid();
    let saved = Hotcue {
        frames: 44_100.0,
        loop_frames: Some(BEAT),
    };
    step(
        &mut c,
        &mut r,
        id,
        DeckCommand::SetHotcue(slot(1), Some(saved)),
    );
    step(&mut c, &mut r, id, DeckCommand::HotcuePress(slot(1)));
    let s = c.deck(id);
    assert!(s.playing && s.previewing && s.loop_active);
    for _ in 0..10 {
        render(&mut r, SR as usize / 10);
        let at = c.deck(id).position_frames;
        assert!((44_100.0..44_100.0 + BEAT).contains(&at), "{at}");
    }
    step(&mut c, &mut r, id, DeckCommand::HotcueRelease(slot(1)));
    let s = c.deck(id);
    assert_eq!(s.position_frames, 44_100.0);
    assert!(!s.playing && !s.previewing);
}

#[test]
fn a_loop_slot_held_with_slip_returns_and_leaves_the_loop() {
    let (mut c, mut r, id) = setup_with_grid();
    let saved = Hotcue {
        frames: 44_100.0,
        loop_frames: Some(BEAT),
    };
    step(
        &mut c,
        &mut r,
        id,
        DeckCommand::SetHotcue(slot(1), Some(saved)),
    );
    step(&mut c, &mut r, id, DeckCommand::SetSlip(true));
    step(&mut c, &mut r, id, DeckCommand::Seek(300_000.0));
    step(&mut c, &mut r, id, DeckCommand::Play);
    step(&mut c, &mut r, id, DeckCommand::HotcuePress(slot(1)));
    assert!(c.deck(id).loop_active);
    render(&mut r, SR as usize);
    step(&mut c, &mut r, id, DeckCommand::HotcueRelease(slot(1)));
    let s = c.deck(id);
    assert!(s.position_frames > 300_000.0 + SR as f64, "{s:?}");
    assert!(!s.loop_active, "{s:?}");
    assert!(s.playing);
}

#[test]
fn an_occupied_pad_during_a_loop_jumps_as_today() {
    let (mut c, mut r, id) = setup_with_grid();
    step(
        &mut c,
        &mut r,
        id,
        DeckCommand::SetHotcue(slot(1), point(200_000.0)),
    );
    step(&mut c, &mut r, id, DeckCommand::Seek(50_000.0));
    step(&mut c, &mut r, id, DeckCommand::AutoLoopBeats(4.0));
    step(&mut c, &mut r, id, DeckCommand::Play);
    c.send(Command::Deck(id, DeckCommand::HotcuePress(slot(1))))
        .unwrap();
    render(&mut r, 1);
    let s = c.deck(id);
    assert_eq!(s.hotcues[0], point(200_000.0));
    assert_eq!(s.position_frames, 200_001.0);
    assert!(!s.loop_active, "a jump outside the loop leaves it");
    assert_eq!(s.loop_in, Some(2.0 * BEAT));
}

#[test]
fn memory_clamps_loop_slots() {
    let mut hotcues = [None; HOTCUES];
    hotcues[0] = Some(Hotcue {
        frames: 900.0,
        loop_frames: Some(500.0),
    });
    hotcues[1] = Some(Hotcue {
        frames: 100.0,
        loop_frames: Some(f64::NAN),
    });
    hotcues[2] = Some(Hotcue {
        frames: 100.0,
        loop_frames: Some(-4.0),
    });
    hotcues[3] = Some(Hotcue {
        frames: 2000.0,
        loop_frames: Some(50.0),
    });
    hotcues[4] = Some(Hotcue {
        frames: 200.0,
        loop_frames: Some(300.0),
    });
    let memory = TrackMemory {
        hotcues,
        ..TrackMemory::default()
    };
    let clamped = memory.clamped(1000);
    assert_eq!(
        clamped.hotcues[..5],
        [
            Some(Hotcue {
                frames: 900.0,
                loop_frames: Some(100.0),
            }),
            point(100.0),
            point(100.0),
            point(1000.0),
            Some(Hotcue {
                frames: 200.0,
                loop_frames: Some(300.0),
            }),
        ]
    );

    // `SetHotcue` keeps a slot the same way.
    let (mut c, mut r, id) = setup_with(1000, TrackMemory::default());
    for (slot, hotcue) in HotcueSlot::ALL.into_iter().zip(hotcues) {
        step(&mut c, &mut r, id, DeckCommand::SetHotcue(slot, hotcue));
    }
    assert_eq!(c.deck(id).hotcues, clamped.hotcues);
}
