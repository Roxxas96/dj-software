mod common;

use std::sync::Arc;

use assert_no_alloc::{AllocDisabler, assert_no_alloc};
use common::ramp_track;
use engine::{Command, DeckCommand, DeckId, EngineConfig, engine};

#[global_allocator]
static A: AllocDisabler = AllocDisabler;

const BLOCK_FRAMES: usize = 512;

#[test]
fn render_does_not_allocate() {
    let (mut c, mut r) = engine(EngineConfig {
        sample_rate: 44_100,
        max_block_frames: BLOCK_FRAMES,
    });
    let decks = [DeckId::new(0).unwrap(), DeckId::new(1).unwrap()];
    let first = ramp_track(48_000, 200_000);
    let old = first.clone();
    for id in decks {
        let track = if id == decks[0] {
            first.clone()
        } else {
            ramp_track(48_000, 200_000)
        };
        for cmd in [
            DeckCommand::Load(track),
            DeckCommand::Play,
            DeckCommand::SetTempoSlider(0.7),
        ] {
            c.send(Command::Deck(id, cmd)).unwrap();
        }
    }
    drop(first);
    // Built up front: allocating it inside the guarded section would abort the test.
    let replacement = ramp_track(44_100, 200_000);
    let mut master = vec![0.0; BLOCK_FRAMES * 2];
    let mut headphones = vec![0.0; BLOCK_FRAMES * 2];

    // `send` may free garbage on the control thread, so it stays outside the guarded sections.
    let mut render_blocks = |blocks: usize| {
        assert_no_alloc(|| {
            for _ in 0..blocks {
                r.render(&mut master, &mut headphones);
            }
        });
    };

    render_blocks(50);
    c.send(Command::Deck(decks[0], DeckCommand::Load(replacement)))
        .unwrap();
    c.send(Command::Deck(decks[0], DeckCommand::Play)).unwrap();
    render_blocks(10);
    for cmd in [
        DeckCommand::CuePress,
        DeckCommand::CuePress,
        DeckCommand::CueRelease,
        DeckCommand::Play,
    ] {
        c.send(Command::Deck(decks[0], cmd)).unwrap();
        render_blocks(10);
    }

    // The replaced track went back to the controller, which frees it outside the guard.
    c.collect_garbage();
    assert_eq!(Arc::strong_count(&old), 1);
    assert!(c.deck(decks[0]).playing);
    assert!(master.iter().any(|&s| s != 0.0));
}
