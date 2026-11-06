mod common;

use std::sync::Arc;

use approx::assert_abs_diff_eq;
use common::{SETTLE_FRAMES, SETTLED_EPSILON, constant_track, ramp_track, render};
use engine::mixer::level_gain;
use engine::{
    ChannelCommand, Command, DeckCommand, DeckId, EngineConfig, EngineController, EngineRenderer,
    MasterCommand, SAMPLER_SLOTS, SamplerCommand, TrackBuffer, TrackMemory, engine,
};

const SR: u32 = 44_100;
const BLOCK: usize = 512;
/// Default master level is 0.8, squared law.
const MASTER: f32 = 0.64;

fn setup() -> (EngineController, EngineRenderer) {
    engine(EngineConfig {
        sample_rate: SR,
        max_block_frames: BLOCK,
    })
}

fn sampler(c: &mut EngineController, cmd: SamplerCommand) {
    c.send(Command::Sampler(cmd)).unwrap();
}

fn set(c: &mut EngineController, slot: usize, track: &Arc<TrackBuffer>) {
    sampler(c, SamplerCommand::Set(slot, Some(Arc::clone(track))));
}

fn left(samples: &[f32]) -> Vec<f32> {
    samples.iter().step_by(2).copied().collect()
}

fn sounding(samples: &[f32]) -> usize {
    left(samples).iter().filter(|&&s| s != 0.0).count()
}

#[test]
fn sample_plays_once_then_stops() {
    let (mut c, mut r) = setup();
    set(&mut c, 0, &constant_track(SR, 1000, 0.5));
    sampler(&mut c, SamplerCommand::Play(0));
    let (m, _) = render(&mut r, 2000);
    assert_eq!(sounding(&m), 1000);
    for &s in &left(&m)[..1000] {
        assert_abs_diff_eq!(s, 0.5 * MASTER, epsilon = 1e-6);
    }
    assert!(m[2000..].iter().all(|&s| s == 0.0));
    assert!(!c.sampler().playing[0]);
}

#[test]
fn play_again_restarts() {
    let (mut c, mut r) = setup();
    set(&mut c, 2, &ramp_track(SR, 1000));
    sampler(&mut c, SamplerCommand::Play(2));
    let (first, _) = render(&mut r, 300);
    assert!(c.sampler().playing[2]);
    sampler(&mut c, SamplerCommand::Play(2));
    let (again, _) = render(&mut r, 300);
    assert_eq!(first, again);
    // Playing it out after a restart takes the full length again.
    let (rest, _) = render(&mut r, 1000);
    assert_eq!(sounding(&rest), 700);
}

#[test]
fn stop_silences() {
    let (mut c, mut r) = setup();
    set(&mut c, 1, &constant_track(SR, 5000, 0.5));
    sampler(&mut c, SamplerCommand::Play(1));
    render(&mut r, 100);
    assert!(c.sampler().playing[1]);
    sampler(&mut c, SamplerCommand::Stop(1));
    let (m, _) = render(&mut r, 100);
    assert!(m.iter().all(|&s| s == 0.0));
    assert_eq!(c.sampler().playing, [false; SAMPLER_SLOTS]);
}

#[test]
fn samples_play_at_their_own_rate() {
    let (mut c, mut r) = setup();
    set(&mut c, 0, &constant_track(SR / 2, 1000, 0.5));
    sampler(&mut c, SamplerCommand::Play(0));
    let (m, _) = render(&mut r, 3000);
    assert_eq!(sounding(&m), 2000);
}

#[test]
fn samples_reach_master_and_headphone_master_mix() {
    let (mut c, mut r) = setup();
    c.send(Command::Master(MasterCommand::HeadphoneMix(1.0)))
        .unwrap();
    c.send(Command::Master(MasterCommand::HeadphoneLevel(1.0)))
        .unwrap();
    sampler(&mut c, SamplerCommand::Volume(0.5));
    render(&mut r, BLOCK);
    set(&mut c, 0, &constant_track(SR, 1000, 0.5));
    set(&mut c, 3, &constant_track(SR, 1000, 0.25));
    sampler(&mut c, SamplerCommand::Play(0));
    sampler(&mut c, SamplerCommand::Play(3));
    let (m, h) = render(&mut r, 1000);
    let expected = 0.75 * level_gain(0.5) * MASTER;
    assert_abs_diff_eq!(m[1800], expected, epsilon = 1e-6);
    assert_abs_diff_eq!(m[1801], expected, epsilon = 1e-6);
    assert_abs_diff_eq!(h[1800], expected, epsilon = 1e-6);
    // The meters see it too.
    assert_abs_diff_eq!(c.meters().master_peak[0], expected, epsilon = 1e-6);
}

#[test]
fn samples_add_to_the_decks() {
    let (mut c, mut r) = setup();
    c.send(Command::Channel(
        DeckId::new(0).unwrap(),
        ChannelCommand::XfaderAssign(engine::XfaderAssign::Thru),
    ))
    .unwrap();
    c.send(Command::Deck(
        DeckId::new(0).unwrap(),
        DeckCommand::Load {
            track: constant_track(SR, 100_000, 0.25),
            memory: TrackMemory::default(),
        },
    ))
    .unwrap();
    c.send(Command::Deck(DeckId::new(0).unwrap(), DeckCommand::Play))
        .unwrap();
    render(&mut r, SETTLE_FRAMES);
    set(&mut c, 0, &constant_track(SR, 4000, 0.25));
    sampler(&mut c, SamplerCommand::Play(0));
    let (m, _) = render(&mut r, 1024);
    assert_abs_diff_eq!(m[1000], 0.5 * MASTER, epsilon = SETTLED_EPSILON);
}

#[test]
fn replaced_sample_goes_to_garbage() {
    let (mut c, mut r) = setup();
    let first = constant_track(SR, 1000, 0.5);
    set(&mut c, 0, &first);
    sampler(&mut c, SamplerCommand::Play(0));
    render(&mut r, 100);
    assert_eq!(Arc::strong_count(&first), 2);
    set(&mut c, 0, &constant_track(SR, 1000, 0.5));
    let (m, _) = render(&mut r, 100);
    assert!(m.iter().all(|&s| s == 0.0), "Set stops the slot");
    c.collect_garbage();
    assert_eq!(Arc::strong_count(&first), 1);
    // Emptying a slot discards too.
    let second = constant_track(SR, 1000, 0.5);
    set(&mut c, 1, &second);
    render(&mut r, 10);
    sampler(&mut c, SamplerCommand::Set(1, None));
    render(&mut r, 10);
    c.collect_garbage();
    assert_eq!(Arc::strong_count(&second), 1);
}

#[test]
fn out_of_range_slot_is_ignored() {
    let (mut c, mut r) = setup();
    // Only the command holds the buffer: the test keeps a Weak.
    let track = constant_track(SR, 1000, 0.5);
    let weak = Arc::downgrade(&track);
    sampler(&mut c, SamplerCommand::Set(SAMPLER_SLOTS, Some(track)));
    sampler(&mut c, SamplerCommand::Play(SAMPLER_SLOTS));
    sampler(&mut c, SamplerCommand::Play(99));
    sampler(&mut c, SamplerCommand::Stop(SAMPLER_SLOTS));
    sampler(&mut c, SamplerCommand::Volume(f32::NAN));
    let (m, _) = render(&mut r, 100);
    assert!(m.iter().all(|&s| s == 0.0));
    // The rejected buffer went to the garbage queue, not freed on the audio thread.
    assert!(weak.upgrade().is_some());
    c.collect_garbage();
    assert!(weak.upgrade().is_none());
    assert_eq!(c.sampler().playing, [false; SAMPLER_SLOTS]);
}

#[test]
fn empty_samples_and_empty_slots_stay_silent() {
    let (mut c, mut r) = setup();
    sampler(&mut c, SamplerCommand::Play(0));
    set(&mut c, 1, &Arc::new(TrackBuffer::new(SR, vec![]).unwrap()));
    sampler(&mut c, SamplerCommand::Play(1));
    let (m, _) = render(&mut r, 100);
    assert!(m.iter().all(|&s| s == 0.0));
    assert_eq!(c.sampler().playing, [false; SAMPLER_SLOTS]);
}
