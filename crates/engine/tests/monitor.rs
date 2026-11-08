mod common;

use common::ramp_track;
use engine::{Command, DeckCommand, DeckId, EngineConfig, TrackMemory, engine};

const SR: u32 = 44_100;
const BLOCK: usize = 512;

fn setup() -> (engine::EngineController, engine::EngineRenderer) {
    engine(EngineConfig {
        sample_rate: SR,
        max_block_frames: BLOCK,
    })
}

#[test]
fn monitor_reads_from_another_thread() {
    let (mut c, mut r) = setup();
    let id = DeckId::new(0).unwrap();
    c.send(Command::Deck(
        id,
        DeckCommand::Load {
            track: ramp_track(SR, SR as usize),
            memory: TrackMemory::default(),
        },
    ))
    .unwrap();
    c.send(Command::Deck(id, DeckCommand::Play)).unwrap();
    let mut master = vec![0.0; BLOCK * 2];
    let mut headphones = vec![0.0; BLOCK * 2];
    for _ in 0..10 {
        r.render(&mut master, &mut headphones);
    }

    let monitor = c.monitor();
    let snapshot = std::thread::spawn(move || monitor.deck(id)).join().unwrap();
    assert!(snapshot.playing);
    assert!(snapshot.position_frames > 0.0);
}

#[test]
fn dsp_load_is_published() {
    let (c, mut r) = setup();
    let monitor = c.monitor();
    let mut master = vec![0.0; BLOCK * 2];
    let mut headphones = vec![0.0; BLOCK * 2];
    r.render(&mut master, &mut headphones);

    let load = monitor.dsp_load();
    assert!(0.0 < load && load < 10.0, "load {load}");
    assert_eq!(monitor.sample_rate(), 44_100);
}

#[test]
fn each_block_stamps_when_it_began_rendering() {
    let (c, mut r) = setup();
    let monitor = c.monitor();
    let id = DeckId::new(0).unwrap();
    let mut master = vec![0.0; BLOCK * 2];
    let mut headphones = vec![0.0; BLOCK * 2];
    let before = engine::clock::micros_at(std::time::Instant::now());
    r.render(&mut master, &mut headphones);
    let after = engine::clock::micros_at(std::time::Instant::now());
    let (snapshot, rendered_at) = monitor.deck_rendered_at(id);
    assert_eq!(snapshot, monitor.deck(id));
    assert!(
        before <= rendered_at && rendered_at <= after,
        "{before} {rendered_at} {after}"
    );

    std::thread::sleep(std::time::Duration::from_millis(2));
    r.render(&mut master, &mut headphones);
    assert!(monitor.deck_rendered_at(id).1 >= after);
}
