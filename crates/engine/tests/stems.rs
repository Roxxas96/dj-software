//! Stems on a deck: the knob law, the stem pads, the instruments residual and the changeover
//! between the EQ and the stems.
//!
//! The output passes the mixer's isolator, which is never bit-exact dry, so the stem
//! output is compared against a reference engine playing a track that holds the expected
//! sum: both pass the same processing.

mod common;

use std::f32::consts::TAU;
use std::sync::Arc;

use common::{SETTLE_FRAMES, render};
use engine::{
    ChannelCommand, Command, DeckCommand, DeckId, EngineConfig, EngineController, EngineRenderer,
    EqMode, JogSurface, MasterCommand, Stem, StemError, StemSet, TrackBuffer, TrackMemory, engine,
    knob_law,
};

const SR: u32 = 44_100;
const BLOCK: usize = 512;
const FRAMES: usize = 120_000;
const STEMS: [Stem; 4] = [Stem::Vocals, Stem::Instruments, Stem::Bass, Stem::Rhythm];

fn deck() -> DeckId {
    DeckId::new(0).unwrap()
}

/// An interleaved stereo tone; the right channel is shifted so the channels differ.
fn tone(freq: f32, amp: f32, phase: f32) -> Vec<f32> {
    (0..FRAMES)
        .flat_map(|i| {
            let t = i as f32 / SR as f32;
            [
                amp * (TAU * freq * t + phase).sin(),
                amp * (TAU * freq * t + phase + 0.4).sin(),
            ]
        })
        .collect()
}

fn to_i16(samples: &[f32]) -> Vec<i16> {
    samples
        .iter()
        .map(|s| (s * 32768.0).round().clamp(-32768.0, 32767.0) as i16)
        .collect()
}

fn from_i16(samples: &[i16]) -> Vec<f32> {
    samples.iter().map(|&s| f32::from(s) / 32768.0).collect()
}

/// A mix made of four tones, the three stored stems quantized to 16 bits, and the stems
/// as the engine hears them, in [`STEMS`] order: instruments is the residual
/// `mix − (vocals + bass + rhythm)`.
struct Fixture {
    mix: Arc<TrackBuffer>,
    stored: [Vec<i16>; 3],
    heard: [Vec<f32>; 4],
}

impl Fixture {
    fn new() -> Self {
        let vocals = tone(600.0, 0.2, 0.0);
        let other = tone(900.0, 0.2, 2.0);
        let bass = tone(60.0, 0.25, 1.0);
        let rhythm = tone(120.0, 0.2, 0.5);
        let mix: Vec<f32> = (0..FRAMES * 2)
            .map(|i| vocals[i] + other[i] + bass[i] + rhythm[i])
            .collect();
        let stored = [to_i16(&vocals), to_i16(&bass), to_i16(&rhythm)];
        let [v, b, r] = stored.each_ref().map(|s| from_i16(s));
        let instruments = (0..FRAMES * 2)
            .map(|i| mix[i] - (v[i] + b[i] + r[i]))
            .collect();
        Self {
            mix: Arc::new(TrackBuffer::new(SR, mix).unwrap()),
            stored,
            heard: [v, instruments, b, r],
        }
    }

    fn set(&self) -> Arc<StemSet> {
        let [v, b, r] = self.stored.clone();
        Arc::new(StemSet::new(v.into(), b.into(), r.into(), FRAMES).unwrap())
    }

    /// A set that hears no vocals: they are left in instruments.
    fn set_without_vocals(&self) -> Arc<StemSet> {
        let [v, b, r] = self.stored.clone();
        let silent = vec![0; v.len()];
        Arc::new(StemSet::new(silent.into(), b.into(), r.into(), FRAMES).unwrap())
    }

    /// A track holding `Σ gains[s] × stem[s]`.
    fn track_of(&self, gains: [f32; 4]) -> Arc<TrackBuffer> {
        let samples = (0..FRAMES * 2)
            .map(|i| (0..4).map(|s| gains[s] * self.heard[s][i]).sum())
            .collect();
        Arc::new(TrackBuffer::new(SR, samples).unwrap())
    }
}

fn new_engine() -> (EngineController, EngineRenderer) {
    engine(EngineConfig {
        sample_rate: SR,
        max_block_frames: BLOCK,
    })
}

/// An engine with `track` loaded and paused, in `mode`.
fn loaded(track: &Arc<TrackBuffer>, mode: EqMode) -> (EngineController, EngineRenderer) {
    let (mut c, r) = new_engine();
    c.send(Command::Master(MasterCommand::EqMode(mode)))
        .unwrap();
    c.send(Command::Deck(
        deck(),
        DeckCommand::Load {
            track: Arc::clone(track),
            memory: TrackMemory::default(),
        },
    ))
    .unwrap();
    (c, r)
}

fn send_all(c: &mut EngineController, cmds: &[DeckCommand]) {
    for cmd in cmds {
        c.send(Command::Deck(deck(), cmd.clone())).unwrap();
    }
}

fn max_diff(a: &[f32], b: &[f32]) -> f32 {
    assert_eq!(a.len(), b.len());
    a.iter()
        .zip(b)
        .map(|(x, y)| (x - y).abs())
        .fold(0.0, f32::max)
}

fn max_step(samples: &[f32]) -> f32 {
    samples
        .as_chunks::<2>()
        .0
        .windows(2)
        .flat_map(|w| [(w[1][0] - w[0][0]).abs(), (w[1][1] - w[0][1]).abs()])
        .fold(0.0, f32::max)
}

#[test]
fn knob_law_matches_the_spec() {
    // (low, mid, high) → [vocals, instruments, bass, rhythm].
    let table: [((f32, f32, f32), [f32; 4]); 12] = [
        ((0.5, 0.5, 0.5), [1.0, 1.0, 1.0, 1.0]),
        ((0.5, 0.5, 0.25), [0.5, 1.0, 1.0, 1.0]),
        ((0.5, 0.5, 0.0), [0.0, 1.0, 1.0, 1.0]),
        ((0.0, 0.0, 0.0), [0.0, 0.0, 0.0, 1.0]),
        ((0.5, 0.5, 0.75), [1.0, 0.5, 0.5, 0.5]),
        ((0.5, 0.5, 1.0), [1.0, 0.0, 0.0, 0.0]),
        ((0.5, 1.0, 0.5), [0.0, 1.0, 0.0, 0.0]),
        ((1.0, 0.5, 0.5), [0.0, 0.0, 1.0, 0.0]),
        // HI and LOW fully right: vocals and bass, instruments and rhythm off.
        ((1.0, 0.5, 1.0), [1.0, 0.0, 1.0, 0.0]),
        // Every knob turned right keeps its own stem, however little.
        ((0.75, 0.5, 1.0), [1.0, 0.0, 1.0, 0.0]),
        ((0.25, 0.5, 0.75), [1.0, 0.5, 0.25, 0.5]),
        ((0.0, 0.25, 0.625), [1.0, 0.375, 0.0, 0.75]),
    ];
    for ((low, mid, high), expected) in table {
        let gains = knob_law(low, mid, high);
        for (g, e) in gains.iter().zip(expected) {
            assert!((g - e).abs() < 1e-6, "{low} {mid} {high}: {gains:?}");
        }
    }
}

#[test]
fn centered_knobs_changeover_is_inaudible() {
    let fx = Fixture::new();
    let (mut c, mut r) = loaded(&fx.mix, EqMode::Stems);
    let (mut ref_c, mut ref_r) = loaded(&fx.mix, EqMode::Stems);
    send_all(&mut c, &[DeckCommand::Play]);
    send_all(&mut ref_c, &[DeckCommand::Play]);
    let mut out = Vec::new();
    let mut expected = Vec::new();
    let mut step = |c: &mut EngineController, cmd: Option<Command>, blocks: usize| {
        if let Some(cmd) = cmd {
            c.send(cmd).unwrap();
        }
        out.extend(render(&mut r, BLOCK * blocks).0);
        expected.extend(render(&mut ref_r, BLOCK * blocks).0);
    };
    step(&mut c, None, 10);
    // Stems arrive, the mode goes to EQ and back, and the stems go away.
    step(
        &mut c,
        Some(Command::Deck(deck(), DeckCommand::SetStems(Some(fx.set())))),
        10,
    );
    assert!(c.deck(deck()).stems && c.deck(deck()).stems_active);
    step(
        &mut c,
        Some(Command::Master(MasterCommand::EqMode(EqMode::Eq))),
        10,
    );
    assert!(c.deck(deck()).stems && !c.deck(deck()).stems_active);
    step(
        &mut c,
        Some(Command::Master(MasterCommand::EqMode(EqMode::Stems))),
        10,
    );
    assert!(c.deck(deck()).stems_active);
    step(
        &mut c,
        Some(Command::Deck(deck(), DeckCommand::SetStems(None))),
        10,
    );
    assert!(!c.deck(deck()).stems && !c.deck(deck()).stems_active);
    assert!(expected.iter().any(|&s| s.abs() > 0.1));
    let diff = max_diff(&out, &expected);
    assert!(diff <= 1e-4, "changeover changed the output by {diff}");
}

#[test]
fn changeover_has_no_click() {
    let fx = Fixture::new();
    let knobs = [
        ChannelCommand::EqLow(0.2),
        ChannelCommand::EqMid(0.4),
        ChannelCommand::EqHigh(1.0),
    ];
    let start = |stems: bool| {
        let (mut c, r) = loaded(&fx.mix, EqMode::Stems);
        for knob in knobs {
            c.send(Command::Channel(deck(), knob)).unwrap();
        }
        if stems {
            send_all(&mut c, &[DeckCommand::SetStems(Some(fx.set()))]);
        }
        send_all(&mut c, &[DeckCommand::Play]);
        (c, r)
    };
    let (mut c, mut r) = start(false);
    let (_eq_c, mut eq_r) = start(false);
    let (_stem_c, mut stem_r) = start(true);
    let settle = SETTLE_FRAMES.div_ceil(BLOCK) * BLOCK;
    let _ = render(&mut r, settle);
    let eq_path = render(&mut eq_r, settle + BLOCK).0;
    let stem_path = render(&mut stem_r, settle + BLOCK).0;
    // Switching in one step at the changeover would jump this much.
    let jump = max_diff(&eq_path[settle * 2..], &stem_path[settle * 2..]);
    assert!(jump > 0.1, "the paths are too alike to test: {jump}");

    let mut out = Vec::new();
    for cmd in [
        Command::Deck(deck(), DeckCommand::SetStems(Some(fx.set()))),
        Command::Master(MasterCommand::EqMode(EqMode::Eq)),
        Command::Master(MasterCommand::EqMode(EqMode::Stems)),
        // Replacing the set ramps out of the old one, then into the new one.
        Command::Deck(deck(), DeckCommand::SetStems(Some(fx.set_without_vocals()))),
        Command::Deck(deck(), DeckCommand::SetStems(Some(fx.set()))),
        Command::Deck(deck(), DeckCommand::StemOn(Stem::Vocals, false)),
        Command::Deck(deck(), DeckCommand::StemOn(Stem::Vocals, true)),
        Command::Deck(deck(), DeckCommand::SetStems(None)),
    ] {
        c.send(cmd).unwrap();
        out.extend(render(&mut r, BLOCK * 8).0);
    }
    let step = max_step(&out);
    assert!(step <= 0.05, "a step of {step}");
}

#[test]
fn stem_pads_mute_their_stem() {
    let fx = Fixture::new();
    for mode in [EqMode::Stems, EqMode::Eq] {
        for (muted, stem) in STEMS.into_iter().enumerate() {
            let (mut c, mut r) = loaded(&fx.mix, mode);
            send_all(
                &mut c,
                &[
                    DeckCommand::SetStems(Some(fx.set())),
                    DeckCommand::StemOn(stem, false),
                ],
            );
            // The pad's ramp runs while paused.
            let _ = render(&mut r, BLOCK);
            send_all(&mut c, &[DeckCommand::Play]);
            let mut gains = [1.0; 4];
            gains[muted] = 0.0;
            let (mut ref_c, mut ref_r) = loaded(&fx.track_of(gains), mode);
            let _ = render(&mut ref_r, BLOCK);
            send_all(&mut ref_c, &[DeckCommand::Play]);
            let out = render(&mut r, BLOCK * 40).0;
            let expected = render(&mut ref_r, BLOCK * 40).0;
            let diff = max_diff(&out, &expected);
            assert!(diff <= 1e-4, "{mode:?} {stem:?}: off by {diff}");
        }
    }
}

#[test]
fn instruments_is_the_residual() {
    let fx = Fixture::new();
    // Instruments alone is what the three stored stems leave of the mix.
    for (on, gains) in [
        ([false, true, false, false], [0.0, 1.0, 0.0, 0.0]),
        ([true, false, true, true], [1.0, 0.0, 1.0, 1.0]),
    ] {
        let (mut c, mut r) = loaded(&fx.mix, EqMode::Stems);
        send_all(&mut c, &[DeckCommand::SetStems(Some(fx.set()))]);
        for (stem, on) in STEMS.into_iter().zip(on) {
            send_all(&mut c, &[DeckCommand::StemOn(stem, on)]);
        }
        let _ = render(&mut r, BLOCK);
        send_all(&mut c, &[DeckCommand::Play]);
        let (mut ref_c, mut ref_r) = loaded(&fx.track_of(gains), EqMode::Stems);
        let _ = render(&mut ref_r, BLOCK);
        send_all(&mut ref_c, &[DeckCommand::Play]);
        let out = render(&mut r, BLOCK * 40).0;
        let expected = render(&mut ref_r, BLOCK * 40).0;
        assert!(expected.iter().any(|&s| s.abs() > 0.05));
        let diff = max_diff(&out, &expected);
        assert!(diff <= 1e-4, "{on:?}: off by {diff}");
    }
}

#[test]
fn mismatched_stems_are_ignored() {
    let fx = Fixture::new();
    let short = |frames: usize| {
        let v = vec![0_i16; frames * 2].into_boxed_slice();
        StemSet::new(v.clone(), v.clone(), v, frames).unwrap()
    };
    assert!(matches!(
        StemSet::new(vec![0; 10].into(), vec![0; 10].into(), vec![0; 8].into(), 5),
        Err(StemError::Length { .. })
    ));

    // On an empty deck, then on a deck whose track is longer.
    let (mut c, mut r) = new_engine();
    let wrong = Arc::new(short(FRAMES - 1));
    send_all(&mut c, &[DeckCommand::SetStems(Some(Arc::clone(&wrong)))]);
    let _ = render(&mut r, BLOCK);
    assert!(!c.deck(deck()).stems);
    send_all(
        &mut c,
        &[
            DeckCommand::Load {
                track: Arc::clone(&fx.mix),
                memory: TrackMemory::default(),
            },
            DeckCommand::SetStems(Some(Arc::clone(&wrong))),
            DeckCommand::StemOn(Stem::Vocals, false),
        ],
    );
    let _ = render(&mut r, BLOCK);
    assert!(!c.deck(deck()).stems);
    c.collect_garbage();
    assert_eq!(Arc::strong_count(&wrong), 1, "the set was discarded");

    // The deck still plays the whole mix.
    send_all(&mut c, &[DeckCommand::Play]);
    let (mut ref_c, mut ref_r) = loaded(&fx.mix, EqMode::Eq);
    let _ = render(&mut ref_r, BLOCK);
    send_all(&mut ref_c, &[DeckCommand::Play]);
    let diff = max_diff(
        &render(&mut r, BLOCK * 20).0,
        &render(&mut ref_r, BLOCK * 20).0,
    );
    assert!(diff <= 1e-6, "off by {diff}");
}

#[test]
fn load_clears_stems_and_resets_pads() {
    let fx = Fixture::new();
    let (mut c, mut r) = loaded(&fx.mix, EqMode::Stems);
    let first = fx.set();
    send_all(
        &mut c,
        &[
            DeckCommand::SetStems(Some(Arc::clone(&first))),
            DeckCommand::StemOn(Stem::Vocals, false),
            DeckCommand::StemOn(Stem::Bass, false),
        ],
    );
    let _ = render(&mut r, BLOCK);
    assert!(c.deck(deck()).stems && c.deck(deck()).stems_active);

    send_all(
        &mut c,
        &[DeckCommand::Load {
            track: Arc::clone(&fx.mix),
            memory: TrackMemory::default(),
        }],
    );
    let _ = render(&mut r, BLOCK);
    let snapshot = c.deck(deck());
    assert!(!snapshot.stems && !snapshot.stems_active);
    c.collect_garbage();
    assert_eq!(Arc::strong_count(&first), 1, "the old stems were discarded");

    // New stems for the new track play every stem again.
    send_all(&mut c, &[DeckCommand::SetStems(Some(fx.set()))]);
    let _ = render(&mut r, BLOCK);
    send_all(&mut c, &[DeckCommand::Play]);
    let (mut ref_c, mut ref_r) = loaded(&fx.mix, EqMode::Stems);
    let _ = render(&mut ref_r, BLOCK * 3);
    send_all(&mut ref_c, &[DeckCommand::Play]);
    let diff = max_diff(
        &render(&mut r, BLOCK * 20).0,
        &render(&mut ref_r, BLOCK * 20).0,
    );
    assert!(diff <= 1e-4, "off by {diff}");
}

#[test]
fn keylock_and_scratch_play_the_stem_mix() {
    let fx = Fixture::new();
    let setup = [
        DeckCommand::SetKeylock(true),
        DeckCommand::SetTempoSlider(0.7),
    ];
    let start = |track: &Arc<TrackBuffer>, stems: bool| {
        let (mut c, mut r) = loaded(track, EqMode::Stems);
        send_all(&mut c, &setup);
        if stems {
            send_all(
                &mut c,
                &[
                    DeckCommand::SetStems(Some(fx.set())),
                    DeckCommand::StemOn(Stem::Vocals, false),
                ],
            );
        }
        let _ = render(&mut r, BLOCK);
        send_all(&mut c, &[DeckCommand::Play]);
        (c, r)
    };
    // The stem engine, a reference playing the expected sum and one playing the whole mix.
    let mut engines = [
        start(&fx.mix, true),
        start(&fx.track_of([0.0, 1.0, 1.0, 1.0]), false),
        start(&fx.mix, false),
    ];
    let check = |engines: &mut [(EngineController, EngineRenderer); 3],
                 cmds: &[DeckCommand],
                 what: &str| {
        let [out, expected, full] = engines.each_mut().map(|(c, r)| {
            send_all(c, cmds);
            render(r, BLOCK * 30).0
        });
        let diff = max_diff(&out, &expected);
        assert!(diff <= 1e-4, "{what}: off by {diff}");
        assert!(max_diff(&out, &full) > 0.02, "{what}: the vocals are there");
    };
    check(&mut engines, &[], "stretched");
    let snapshot = engines[0].0.deck(deck());
    assert!(snapshot.keylock && snapshot.playing && snapshot.stems_active);
    check(
        &mut engines,
        &[
            DeckCommand::JogTouch(true),
            DeckCommand::Jog {
                turns: -0.3,
                surface: JogSurface::Top,
            },
        ],
        "scratched",
    );
}

/// The amplitude of the fixture's 60 Hz bass in each window of 1470 frames (two of its
/// cycles, and whole cycles of every other tone), left channel.
fn bass_levels(samples: &[f32]) -> Vec<f32> {
    const WINDOW: usize = 1470;
    samples
        .as_chunks::<2>()
        .0
        .as_chunks::<WINDOW>()
        .0
        .iter()
        .map(|w| {
            let (mut i, mut q) = (0.0_f32, 0.0_f32);
            for (n, frame) in w.iter().enumerate() {
                let angle = TAU * 60.0 * n as f32 / SR as f32;
                i += frame[0] * angle.cos();
                q += frame[0] * angle.sin();
            }
            2.0 * (i * i + q * q).sqrt() / WINDOW as f32
        })
        .collect()
}

#[test]
fn keylock_changeover_keeps_the_bass_level() {
    // LOW at 0.25: the EQ band plays the bass at 0.25, the stems at 0.5. Changing over
    // early or late under keylock would play it at 1.0 (bands at unity over the mix) or
    // 0.125 (both).
    let fx = Fixture::new();
    let start = |low: f32| {
        let (mut c, mut r) = loaded(&fx.mix, EqMode::Eq);
        c.send(Command::Channel(deck(), ChannelCommand::EqLow(low)))
            .unwrap();
        send_all(
            &mut c,
            &[
                DeckCommand::SetKeylock(true),
                DeckCommand::SetTempoSlider(0.7),
            ],
        );
        let _ = render(&mut r, BLOCK);
        send_all(&mut c, &[DeckCommand::Play]);
        (c, r)
    };
    // The bass at full level, as heard through keylock.
    let (_full_c, mut full_r) = start(0.5);
    let full_levels = bass_levels(&render(&mut full_r, BLOCK * 100).0);
    let full = full_levels[10..].iter().sum::<f32>() / (full_levels.len() - 10) as f32;
    assert!(full > 0.05, "{full}");

    let (mut c, mut r) = start(0.25);
    let _ = render(&mut r, BLOCK * 20);
    let mut levels = Vec::new();
    for cmd in [
        Command::Deck(deck(), DeckCommand::SetStems(Some(fx.set()))),
        Command::Master(MasterCommand::EqMode(EqMode::Stems)),
        Command::Master(MasterCommand::EqMode(EqMode::Eq)),
        Command::Master(MasterCommand::EqMode(EqMode::Stems)),
        Command::Deck(deck(), DeckCommand::SetStems(None)),
    ] {
        c.send(cmd).unwrap();
        levels.extend(bass_levels(&render(&mut r, 1470 * 12).0));
    }
    assert!(c.deck(deck()).keylock && !c.deck(deck()).stems);
    for (n, level) in levels.iter().enumerate() {
        let level = level / full;
        assert!(
            (0.2..=0.6).contains(&level),
            "window {n}: the bass at {level} of its level"
        );
    }
}
