mod common;

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
    for id in decks {
        for cmd in [
            DeckCommand::Load(ramp_track(48_000, 200_000)),
            DeckCommand::Play,
            DeckCommand::SetTempoSlider(0.7),
        ] {
            c.send(Command::Deck(id, cmd)).unwrap();
        }
    }
    // Built up front: allocating it inside the guarded section would abort the test.
    let mut replacement = Some(ramp_track(44_100, 200_000));
    let mut master = vec![0.0; BLOCK_FRAMES * 2];
    let mut headphones = vec![0.0; BLOCK_FRAMES * 2];

    assert_no_alloc(|| {
        for block in 0..100 {
            if block == 50 {
                let track = replacement.take().unwrap();
                c.send(Command::Deck(decks[0], DeckCommand::Load(track)))
                    .unwrap();
                c.send(Command::Deck(decks[0], DeckCommand::Play)).unwrap();
            }
            r.render(&mut master, &mut headphones);
        }
    });

    // The replaced track went back to the controller, which frees it outside the guard.
    c.collect_garbage();
    assert!(c.deck(decks[0]).playing);
    assert!(master.iter().any(|&s| s != 0.0));
}
