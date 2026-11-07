mod common;

use std::sync::Arc;

use assert_no_alloc::{AllocDisabler, assert_no_alloc};
use common::ramp_track;
use engine::{
    ChannelCommand, Command, DeckCommand, DeckId, EffectKind, EngineConfig, Grid, HotcueSlot,
    JogSurface, MasterCommand, SamplerCommand, TrackMemory, XfaderAssign, engine,
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
    let replacement = ramp_track(44_100, 400_000);
    let sample = ramp_track(44_100, 8_000);
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

    for slot in 0..8 {
        c.send(Command::Sampler(SamplerCommand::Set(
            slot,
            Some(ramp_track(22_050, 30_000)),
        )))
        .unwrap();
    }
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
    assert_eq!(
        c.deck(decks[0]).load_seq,
        2,
        "the second load was published"
    );
    assert_eq!(
        c.deck_consistent(decks[0]).map(|s| s.load_seq),
        Some(2),
        "published whole"
    );
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
        DeckCommand::SetGrid(Grid::from_bpm(128.0, 300.0, 44_100)),
        DeckCommand::AutoLoopBeats(1.0),
        DeckCommand::LoopExit,
        DeckCommand::AutoLoopBeats(4.0),
        DeckCommand::AutoLoopBeats(f64::NAN),
        DeckCommand::LoopExit,
        DeckCommand::AutoLoop,
        DeckCommand::LoopHalve,
        DeckCommand::LoopHalve,
        DeckCommand::LoopDouble,
        DeckCommand::AutoLoop,
        DeckCommand::LoopIn,
        DeckCommand::LoopOut,
        DeckCommand::Seek(20_000.0),
        DeckCommand::LoopIn,
        DeckCommand::Seek(20_100.0),
        DeckCommand::LoopOut,
        DeckCommand::ReloopExit,
        DeckCommand::ReloopExit,
        DeckCommand::ReloopExit,
        DeckCommand::JogTouch(true),
        DeckCommand::Jog {
            turns: 0.2,
            surface: JogSurface::Top,
        },
        DeckCommand::Jog {
            turns: -0.4,
            surface: JogSurface::Top,
        },
        DeckCommand::JogTouch(false),
        DeckCommand::Jog {
            turns: 0.1,
            surface: JogSurface::Ring,
        },
        DeckCommand::Jog {
            turns: f64::NAN,
            surface: JogSurface::Ring,
        },
        DeckCommand::JogSearch { turns: 0.1 },
        DeckCommand::JogSearch { turns: -0.05 },
        DeckCommand::SetVinyl(false),
        DeckCommand::SetVinyl(true),
        // Slip on a playing deck: a scratch, a loop and a hotcue each return to the ghost.
        DeckCommand::Play,
        DeckCommand::SetSlip(true),
        DeckCommand::JogTouch(true),
        DeckCommand::Jog {
            turns: -0.3,
            surface: JogSurface::Top,
        },
        DeckCommand::JogTouch(false),
        DeckCommand::AutoLoop,
        DeckCommand::JogTouch(true),
        DeckCommand::JogTouch(false),
        DeckCommand::LoopHalve,
        DeckCommand::AutoLoop,
        DeckCommand::SetHotcue(slot(4), Some(3000.0)),
        DeckCommand::HotcuePress(slot(4)),
        DeckCommand::Jog {
            turns: 0.1,
            surface: JogSurface::Ring,
        },
        DeckCommand::HotcueRelease(slot(4)),
        DeckCommand::LoopIn,
        DeckCommand::LoopOut,
        DeckCommand::ReloopExit,
        DeckCommand::ReloopExit,
        DeckCommand::JogTouch(true),
        DeckCommand::SetSlip(false),
        DeckCommand::JogTouch(false),
        // Deliberate jumps drop a running ghost; a paused scratch never starts one.
        DeckCommand::SetSlip(true),
        DeckCommand::HotcuePress(slot(4)),
        DeckCommand::Seek(30_000.0),
        DeckCommand::HotcueRelease(slot(4)),
        DeckCommand::JogTouch(true),
        DeckCommand::JogSearch { turns: 0.05 },
        DeckCommand::JogTouch(false),
        DeckCommand::Pause,
        DeckCommand::JogTouch(true),
        DeckCommand::JogTouch(false),
        DeckCommand::Play,
        // Beat jumps: plain, clamped, ignored, with a loop and with a running ghost.
        DeckCommand::BeatJump { beats: 4.0 },
        DeckCommand::BeatJump { beats: -1000.0 },
        DeckCommand::BeatJump { beats: f64::NAN },
        DeckCommand::Seek(60_000.0),
        DeckCommand::AutoLoop,
        DeckCommand::BeatJump { beats: 2.0 },
        DeckCommand::BeatJump { beats: -1000.0 },
        DeckCommand::AutoLoop,
        DeckCommand::BeatJump { beats: 1.0 },
        DeckCommand::ReloopExit,
        DeckCommand::ReloopExit,
        // Loop roll: press, replace, a stale release, a beat jump and the release.
        DeckCommand::RollPress { beats: 0.5 },
        DeckCommand::RollPress { beats: 1.0 / 16.0 },
        DeckCommand::RollRelease { beats: 0.5 },
        DeckCommand::BeatJump { beats: 2.0 },
        DeckCommand::RollRelease { beats: 1.0 / 16.0 },
        // Slicer inside a normal loop: press, move to another slice, release.
        DeckCommand::AutoLoop,
        DeckCommand::SlicePress(2),
        DeckCommand::SlicePress(6),
        DeckCommand::SliceRelease(2),
        DeckCommand::SliceRelease(6),
        DeckCommand::AutoLoop,
        // A roll over a slipped loop hands the ghost back; loop buttons wait meanwhile.
        DeckCommand::SetSlip(true),
        DeckCommand::AutoLoop,
        DeckCommand::RollPress { beats: 0.25 },
        DeckCommand::LoopIn,
        DeckCommand::RollRelease { beats: 0.25 },
        DeckCommand::ReloopExit,
        DeckCommand::SetSlip(false),
        // A jump ends a held pad.
        DeckCommand::SlicePress(1),
        DeckCommand::Seek(40_000.0),
        DeckCommand::SliceRelease(1),
        // Keylock on a playing deck: stretched blocks, a jump, a short loop that wraps, a
        // nudge, a bypass at the original tempo and a toggle.
        DeckCommand::Play,
        DeckCommand::SetKeylock(true),
        DeckCommand::Seek(5_000.0),
        DeckCommand::HotcuePress(slot(5)),
        DeckCommand::HotcuePress(slot(5)),
        DeckCommand::HotcueRelease(slot(5)),
        DeckCommand::LoopHalve,
        DeckCommand::LoopHalve,
        DeckCommand::LoopHalve,
        DeckCommand::LoopHalve,
        DeckCommand::LoopHalve,
        DeckCommand::LoopHalve,
        DeckCommand::AutoLoop,
        DeckCommand::AutoLoop,
        DeckCommand::Jog {
            turns: 0.1,
            surface: JogSurface::Ring,
        },
        DeckCommand::SetTempoSlider(0.5),
        DeckCommand::SetTempoSlider(0.7),
        DeckCommand::SetKeylock(false),
        DeckCommand::SetKeylock(true),
        // A backspin over keylock: bypass while it spins, a restart after, a jump and a
        // touch cancelling it, and one clamped at the start.
        DeckCommand::Backspin { seconds: 0.2 },
        DeckCommand::Backspin { seconds: f64::NAN },
        DeckCommand::Backspin { seconds: 50.0 },
        DeckCommand::Seek(5_000.0),
        DeckCommand::Backspin { seconds: 0.5 },
        DeckCommand::JogTouch(true),
        DeckCommand::JogTouch(false),
        DeckCommand::Seek(1_000.0),
        DeckCommand::Backspin { seconds: 10.0 },
        // The SB3's burst at a spin's start: a scratch taken over, then touches and
        // platter steps within the grace window.
        DeckCommand::Seek(100_000.0),
        DeckCommand::JogTouch(true),
        DeckCommand::Backspin { seconds: 1.0 },
        DeckCommand::JogTouch(true),
        DeckCommand::Jog {
            turns: -0.01,
            surface: JogSurface::Platter,
        },
        DeckCommand::JogTouch(false),
        // A platter scratch with vinyl off, then a nudge.
        DeckCommand::SetVinyl(false),
        DeckCommand::JogTouch(true),
        DeckCommand::Jog {
            turns: 0.3,
            surface: JogSurface::Platter,
        },
        DeckCommand::JogTouch(false),
        DeckCommand::Jog {
            turns: 0.1,
            surface: JogSurface::Platter,
        },
        DeckCommand::SetVinyl(true),
        DeckCommand::Seek(5_000.0),
    ] {
        c.send(Command::Deck(decks[0], cmd)).unwrap();
        render_blocks(10);
    }
    assert!(c.deck(decks[0]).keylock);
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
    // Every effect command, then a render with all three slots on, on all four channels.
    for cmd in [
        MasterCommand::FxSlot(0, EffectKind::Reverb),
        MasterCommand::FxSlot(1, EffectKind::Filter),
        MasterCommand::FxSlot(2, EffectKind::Flanger),
        MasterCommand::FxSlot(7, EffectKind::Echo),
        MasterCommand::FxSlot(0, EffectKind::Echo),
        MasterCommand::FxSlot(1, EffectKind::Reverb),
    ] {
        c.send(Command::Master(cmd)).unwrap();
        render_blocks(1);
    }
    let all: Vec<DeckId> = (0..4).map(|i| DeckId::new(i).unwrap()).collect();
    for &id in &all {
        for cmd in [
            ChannelCommand::FxAmount(0.8),
            ChannelCommand::FxAmount(f32::NAN),
            ChannelCommand::FxEnabled(9, true),
            ChannelCommand::FxEnabled(0, true),
            ChannelCommand::FxEnabled(1, true),
            ChannelCommand::FxEnabled(2, true),
        ] {
            c.send(Command::Channel(id, cmd)).unwrap();
            render_blocks(1);
        }
    }
    render_blocks(6);
    c.send(Command::Master(MasterCommand::FxSlot(
        1,
        EffectKind::Filter,
    )))
    .unwrap();
    render_blocks(3);
    assert_eq!(c.channel(all[3]).fx_enabled, [true; 3]);
    for &id in &all {
        for slot in 0..3 {
            c.send(Command::Channel(id, ChannelCommand::FxEnabled(slot, false)))
                .unwrap();
        }
    }
    render_blocks(3);
    for cmd in [
        MasterCommand::Crossfader(0.2),
        MasterCommand::MasterLevel(0.9),
        MasterCommand::HeadphoneLevel(0.7),
        MasterCommand::HeadphoneMix(0.5),
        MasterCommand::Crossfader(f32::INFINITY),
        MasterCommand::CrossfaderEnabled(false),
        MasterCommand::Crossfader(0.9),
        MasterCommand::CrossfaderEnabled(true),
    ] {
        c.send(Command::Master(cmd)).unwrap();
        render_blocks(2);
    }

    // Sampler: set, play, restart, stop, volume, and a render with every slot playing.
    for cmd in [
        SamplerCommand::Volume(0.6),
        SamplerCommand::Volume(f32::NAN),
        SamplerCommand::Play(0),
        SamplerCommand::Play(0),
        SamplerCommand::Stop(0),
        SamplerCommand::Play(99),
        SamplerCommand::Set(99, Some(ramp_track(44_100, 100))),
    ] {
        c.send(Command::Sampler(cmd)).unwrap();
        render_blocks(2);
    }
    for slot in 0..8 {
        c.send(Command::Sampler(SamplerCommand::Play(slot)))
            .unwrap();
    }
    render_blocks(10);
    assert!(c.sampler().playing.iter().all(|&p| p));
    // Replacing a playing sample hands the old one to the garbage queue.
    c.send(Command::Sampler(SamplerCommand::Set(3, Some(sample))))
        .unwrap();
    render_blocks(2);
    c.send(Command::Sampler(SamplerCommand::Set(4, None)))
        .unwrap();
    render_blocks(2);
    c.send(Command::Sampler(SamplerCommand::Play(3))).unwrap();
    render_blocks(100);
    assert!(!c.sampler().playing[3], "the sample ran out");

    // Phase sync between two playing decks, and a pending one resolved on play.
    c.send(Command::Deck(decks[1], DeckCommand::Play)).unwrap();
    c.send(Command::Deck(decks[0], DeckCommand::Play)).unwrap();
    for cmd in [
        Command::Deck(
            decks[1],
            DeckCommand::SetGrid(Grid::from_bpm(126.0, 0.0, 44_100)),
        ),
        Command::PhaseSync {
            deck: decks[1],
            master: decks[0],
            beat_ratio: 1.0,
        },
        Command::PhaseSync {
            deck: decks[1],
            master: decks[0],
            beat_ratio: 0.5,
        },
        Command::PhaseSync {
            deck: decks[1],
            master: decks[0],
            beat_ratio: 2.0,
        },
        Command::Deck(decks[1], DeckCommand::Pause),
        Command::PhaseSync {
            deck: decks[1],
            master: decks[0],
            beat_ratio: 1.0,
        },
        Command::Deck(decks[1], DeckCommand::Play),
    ] {
        c.send(cmd).unwrap();
        render_blocks(5);
    }

    // The replaced track went back to the controller, which frees it outside the guard.
    c.collect_garbage();
    assert_eq!(Arc::strong_count(&old), 1);
    assert!(c.deck(decks[0]).playing);
    assert!(master.iter().any(|&s| s != 0.0));
    assert!(headphones.iter().any(|&s| s != 0.0));
    assert!(c.meters().channel_peak[0] > 0.0);
}
