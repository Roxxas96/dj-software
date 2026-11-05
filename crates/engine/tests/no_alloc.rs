mod common;

use std::sync::Arc;

use assert_no_alloc::{AllocDisabler, assert_no_alloc};
use common::ramp_track;
use engine::{
    ChannelCommand, Command, DeckCommand, DeckId, EngineConfig, Grid, HotcueSlot, MasterCommand,
    TrackMemory, XfaderAssign, engine,
};

#[global_allocator]
static A: AllocDisabler = AllocDisabler;

const BLOCK_FRAMES: usize = 512;

fn slot(n: u8) -> HotcueSlot {
    HotcueSlot::new(n).unwrap()
}

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
            DeckCommand::Load {
                track,
                memory: engine::TrackMemory::default(),
            },
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
    c.send(Command::Deck(
        decks[0],
        DeckCommand::Load {
            track: replacement,
            memory: TrackMemory {
                cue: 5000.0,
                hotcues: [
                    Some(100.0),
                    None,
                    Some(f64::NAN),
                    Some(-1.0),
                    None,
                    None,
                    None,
                    None,
                ],
                grid: Grid::from_bpm(128.0, 300.0, 44_100),
            },
        },
    ))
    .unwrap();
    c.send(Command::Deck(decks[0], DeckCommand::Play)).unwrap();
    render_blocks(10);
    for cmd in [
        DeckCommand::CuePress,
        DeckCommand::CuePress,
        DeckCommand::TogglePlay,
        DeckCommand::CueRelease,
        DeckCommand::CuePress,
        DeckCommand::CuePress,
        DeckCommand::Pause,
        DeckCommand::CueRelease,
        DeckCommand::Play,
        DeckCommand::SetCue(1234.0),
        DeckCommand::SetCue(f64::NAN),
        DeckCommand::CuePress,
        DeckCommand::SetCue(-5.0),
        DeckCommand::CueRelease,
        DeckCommand::Play,
        DeckCommand::SetGrid(Grid::from_bpm(120.0, 0.0, 44_100)),
        DeckCommand::SetGrid(Some(Grid {
            beat_period_frames: 0.0,
            first_beat_frames: 0.0,
        })),
        DeckCommand::SetGrid(None),
        DeckCommand::HotcuePress(slot(1)),
        DeckCommand::HotcueRelease(slot(1)),
        DeckCommand::HotcuePress(slot(2)),
        DeckCommand::HotcueRelease(slot(2)),
        DeckCommand::SetHotcue(slot(3), Some(9000.0)),
        DeckCommand::SetHotcue(slot(3), Some(f64::NAN)),
        DeckCommand::HotcuePress(slot(3)),
        DeckCommand::HotcueDelete(slot(3)),
        DeckCommand::Pause,
        DeckCommand::HotcuePress(slot(1)),
        DeckCommand::TogglePlay,
        DeckCommand::HotcueRelease(slot(1)),
        DeckCommand::HotcuePress(slot(2)),
        DeckCommand::HotcueDelete(slot(2)),
        DeckCommand::SetHotcue(slot(1), None),
        DeckCommand::Play,
    ] {
        c.send(Command::Deck(decks[0], cmd)).unwrap();
        render_blocks(10);
    }
    for cmd in [
        ChannelCommand::Trim(0.7),
        ChannelCommand::EqLow(0.2),
        ChannelCommand::EqMid(0.6),
        ChannelCommand::EqHigh(0.9),
        ChannelCommand::Filter(0.3),
        ChannelCommand::Filter(0.8),
        ChannelCommand::EqHigh(0.0),
        ChannelCommand::Filter(f32::NAN),
        ChannelCommand::Fader(0.8),
        ChannelCommand::Pfl(true),
        ChannelCommand::XfaderAssign(XfaderAssign::Thru),
        ChannelCommand::Fader(f32::NAN),
    ] {
        c.send(Command::Channel(decks[0], cmd)).unwrap();
        render_blocks(2);
    }
    for cmd in [
        MasterCommand::Crossfader(0.2),
        MasterCommand::MasterLevel(0.9),
        MasterCommand::HeadphoneLevel(0.7),
        MasterCommand::HeadphoneMix(0.5),
        MasterCommand::Crossfader(f32::INFINITY),
    ] {
        c.send(Command::Master(cmd)).unwrap();
        render_blocks(2);
    }

    // The replaced track went back to the controller, which frees it outside the guard.
    c.collect_garbage();
    assert_eq!(Arc::strong_count(&old), 1);
    assert!(c.deck(decks[0]).playing);
    assert!(master.iter().any(|&s| s != 0.0));
    assert!(headphones.iter().any(|&s| s != 0.0));
    assert!(c.meters().channel_peak[0] > 0.0);
}
