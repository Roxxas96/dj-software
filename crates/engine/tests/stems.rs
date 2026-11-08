//! Stems on a deck: the knob layouts and law, the stem pads, the instruments residual, the
//! kicks/hats split and the changeover between the EQ and the stems.
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
    EqMode, JogSurface, MasterCommand, Stem, StemError, StemKnobs, StemSet, TrackBuffer,
    TrackMemory, engine, knob_law, pad_gains,
};

const SR: u32 = 44_100;
const BLOCK: usize = 512;
const FRAMES: usize = 120_000;
const LAYOUTS: [StemKnobs; 2] = [StemKnobs::Vdj, StemKnobs::Raw];
/// The parts a deck mixes, in the order of [`knob_law`]'s and [`pad_gains`]' gains.
const PARTS: usize = 5;

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

/// A mix made of four tones, the three stored stems quantized to 16 bits, and the five
/// parts as the engine hears them, in [`knob_law`] order: instruments is the residual
/// `mix − (vocals + bass + rhythm)` and hats is `rhythm − kicks`.
struct Fixture {
    mix: Arc<TrackBuffer>,
    stored: [Vec<i16>; 3],
    heard: [Vec<f32>; PARTS],
}

impl Fixture {
    fn new() -> Self {
        Self::of(
            tone(600.0, 0.2, 0.0),
            tone(900.0, 0.2, 2.0),
            tone(60.0, 0.25, 1.0),
            tone(120.0, 0.2, 0.5),
        )
    }

    /// The fixture whose only low end is a 60 Hz kick in rhythm, under a 5 kHz hat.
    fn kick_and_hat() -> Self {
        let rhythm = tone(60.0, 0.3, 1.0)
            .iter()
            .zip(tone(5000.0, 0.2, 0.0))
            .map(|(k, h)| k + h)
            .collect();
        Self::of(
            tone(600.0, 0.2, 0.0),
            tone(900.0, 0.2, 2.0),
            vec![0.0; FRAMES * 2],
            rhythm,
        )
    }

    fn of(vocals: Vec<f32>, other: Vec<f32>, bass: Vec<f32>, rhythm: Vec<f32>) -> Self {
        let mix: Vec<f32> = (0..FRAMES * 2)
            .map(|i| vocals[i] + other[i] + bass[i] + rhythm[i])
            .collect();
        let stored = [to_i16(&vocals), to_i16(&bass), to_i16(&rhythm)];
        let [v, b, r] = stored.each_ref().map(|s| from_i16(s));
        let instruments = (0..FRAMES * 2)
            .map(|i| mix[i] - (v[i] + b[i] + r[i]))
            .collect();
        let [sv, sb, sr] = stored.clone();
        let set = StemSet::new(sv.into(), sb.into(), sr.into(), FRAMES, SR).unwrap();
        let kicks = from_i16(set.stored(Stem::Kicks).unwrap());
        let hats = (0..FRAMES * 2).map(|i| r[i] - kicks[i]).collect();
        Self {
            mix: Arc::new(TrackBuffer::new(SR, mix).unwrap()),
            stored,
            heard: [v, instruments, b, kicks, hats],
        }
    }

    fn set(&self) -> Arc<StemSet> {
        let [v, b, r] = self.stored.clone();
        Arc::new(StemSet::new(v.into(), b.into(), r.into(), FRAMES, SR).unwrap())
    }

    /// A set that hears no vocals: they are left in instruments.
    fn set_without_vocals(&self) -> Arc<StemSet> {
        let [v, b, r] = self.stored.clone();
        let silent = vec![0; v.len()];
        Arc::new(StemSet::new(silent.into(), b.into(), r.into(), FRAMES, SR).unwrap())
    }

    /// A track holding `Σ gains[p] × part[p]`.
    fn track_of(&self, gains: [f32; PARTS]) -> Arc<TrackBuffer> {
        let samples = (0..FRAMES * 2)
            .map(|i| (0..PARTS).map(|p| gains[p] * self.heard[p][i]).sum())
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

/// An engine with `track` loaded and paused, in `mode` with the default knob layout.
fn loaded(track: &Arc<TrackBuffer>, mode: EqMode) -> (EngineController, EngineRenderer) {
    loaded_with(track, mode, StemKnobs::default())
}

/// An engine with `track` loaded and paused, in `mode` with the knob layout `knobs`.
fn loaded_with(
    track: &Arc<TrackBuffer>,
    mode: EqMode,
    knobs: StemKnobs,
) -> (EngineController, EngineRenderer) {
    let (mut c, r) = new_engine();
    c.send(Command::Master(MasterCommand::EqMode(mode)))
        .unwrap();
    c.send(Command::Master(MasterCommand::StemKnobs(knobs)))
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
    let boost = 10_f32.powf(6.0 / 20.0);
    // (layout, [low, mid, high]) → [vocals, instruments, bass, kicks, hats].
    let table: [(StemKnobs, [f32; 3], [f32; PARTS]); 10] = [
        (StemKnobs::Vdj, [0.0; 3], [0.0; PARTS]),
        (StemKnobs::Vdj, [0.5; 3], [1.0; PARTS]),
        (StemKnobs::Vdj, [1.0, 0.5, 0.5], [1.0, 1.0, 1.0, boost, 1.0]),
        (
            StemKnobs::Vdj,
            [0.5, 0.25, 0.5],
            [0.25, 0.25, 0.25, 1.0, 1.0],
        ),
        (StemKnobs::Vdj, [0.5, 0.5, 0.0], [1.0, 1.0, 1.0, 1.0, 0.0]),
        (StemKnobs::Raw, [0.5; 3], [1.0; PARTS]),
        (StemKnobs::Raw, [0.5, 0.5, 1.0], [boost, 1.0, 1.0, 1.0, 1.0]),
        (StemKnobs::Raw, [0.0; 3], [0.0, 0.0, 0.0, 1.0, 1.0]),
        (
            StemKnobs::Raw,
            [0.25, 1.0, 0.5],
            [1.0, boost, 0.25, 1.0, 1.0],
        ),
        // A knob only ever changes its own part: no isolation.
        (
            StemKnobs::Raw,
            [1.0, 1.0, 1.0],
            [boost, boost, boost, 1.0, 1.0],
        ),
    ];
    for (layout, knobs, expected) in table {
        let gains = knob_law(layout, knobs);
        for (g, e) in gains.iter().zip(expected) {
            assert!((g - e).abs() < 1e-6, "{layout:?} {knobs:?}: {gains:?}");
        }
    }
}

#[test]
fn pad_gains_multiply() {
    let pads = |off: &[Stem]| {
        let mut on = [true; 7];
        for stem in off {
            on[stem.index()] = false;
        }
        pad_gains(on)
    };
    assert_eq!(pads(&[]), [1.0; PARTS]);
    assert_eq!(pads(&[Stem::Rhythm]), [1.0, 1.0, 1.0, 0.0, 0.0]);
    assert_eq!(pads(&[Stem::Melody]), [0.0, 0.0, 0.0, 1.0, 1.0]);
    assert_eq!(pads(&[Stem::Kicks]), [1.0, 1.0, 1.0, 0.0, 1.0]);
    assert_eq!(pads(&[Stem::Hats]), [1.0, 1.0, 1.0, 1.0, 0.0]);
    assert_eq!(pads(&[Stem::Vocals]), [0.0, 1.0, 1.0, 1.0, 1.0]);
    assert_eq!(pads(&[Stem::Instruments]), [1.0, 0.0, 1.0, 1.0, 1.0]);
    assert_eq!(pads(&[Stem::Bass]), [1.0, 1.0, 0.0, 1.0, 1.0]);
    // Turning a covering pad back on brings back only what the other pads allow.
    assert_eq!(
        pads(&[Stem::Kicks, Stem::Vocals]),
        [0.0, 1.0, 1.0, 0.0, 1.0]
    );
    for (i, stem) in Stem::ALL.into_iter().enumerate() {
        assert_eq!(stem.index(), i);
    }
}

/// The amplitude of the `freq` sine in `samples` (mono), over whole cycles.
fn amplitude(samples: &[f32], freq: f32, sample_rate: u32) -> f32 {
    let (mut i, mut q) = (0.0_f64, 0.0_f64);
    for (n, &x) in samples.iter().enumerate() {
        let angle = f64::from(TAU) * f64::from(freq) * n as f64 / f64::from(sample_rate);
        i += f64::from(x) * angle.cos();
        q += f64::from(x) * angle.sin();
    }
    (2.0 * (i * i + q * q).sqrt() / samples.len() as f64) as f32
}

#[test]
fn kicks_split_low_frequencies() {
    for rate in [44_100_u32, 48_000] {
        // Two seconds of a 60 Hz kick under a 5 kHz hat; one second in the middle is
        // measured, whole cycles of both.
        let frames = 2 * rate as usize;
        let sine =
            |freq: f32, amp: f32, n: usize| amp * (TAU * freq * (n as f32 / rate as f32)).sin();
        let rhythm: Vec<f32> = (0..frames)
            .flat_map(|n| {
                let v = sine(60.0, 0.4, n) + sine(5000.0, 0.3, n);
                [v, v]
            })
            .collect();
        let silent = vec![0_i16; frames * 2].into_boxed_slice();
        let rhythm = to_i16(&rhythm);
        let set =
            StemSet::new(silent.clone(), silent, rhythm.clone().into(), frames, rate).unwrap();
        assert_eq!(set.sample_rate(), rate);
        let kicks = set.stored(Stem::Kicks).unwrap();
        let left = |s: &[i16]| -> Vec<f32> {
            s.as_chunks::<2>().0[frames / 4..frames / 4 + rate as usize]
                .iter()
                .map(|f| f32::from(f[0]) / 32768.0)
                .collect()
        };
        let (kicks_left, rhythm_left) = (left(kicks), left(&rhythm));
        let energy = |s: &[f32], freq: f32| amplitude(s, freq, rate).powi(2);
        let low = energy(&kicks_left, 60.0) / energy(&rhythm_left, 60.0);
        let high = energy(&kicks_left, 5000.0) / energy(&rhythm_left, 5000.0);
        assert!(low > 0.9, "{rate} Hz: kicks keep {low} of the 60 Hz energy");
        assert!(
            high < 0.01,
            "{rate} Hz: kicks keep {high} of the 5 kHz energy"
        );
        // The split sits at 250 Hz: half the amplitude there, whatever the rate (a filter
        // built for another rate would read about 0.58).
        let probe: Vec<f32> = (0..frames)
            .flat_map(|n| {
                let v = sine(250.0, 0.4, n);
                [v, v]
            })
            .collect();
        let probe = to_i16(&probe);
        let silent = vec![0_i16; frames * 2].into_boxed_slice();
        let probed =
            StemSet::new(silent.clone(), silent, probe.clone().into(), frames, rate).unwrap();
        let ratio = amplitude(&left(probed.stored(Stem::Kicks).unwrap()), 250.0, rate)
            / amplitude(&left(&probe), 250.0, rate);
        assert!(
            (ratio - 0.5).abs() <= 0.03,
            "{rate} Hz: kicks keep {ratio} of the 250 Hz amplitude"
        );
        // Hats, what kicks leave of rhythm, sums back to it with kicks: played with only
        // kicks and hats, the deck plays rhythm itself (here, the whole mix).
        let track = Arc::new(TrackBuffer::new(rate, from_i16(&rhythm)).unwrap());
        let config = EngineConfig {
            sample_rate: rate,
            max_block_frames: BLOCK,
        };
        let [(mut c, mut r), (mut ref_c, mut ref_r)] = [(); 2].map(|()| engine(config));
        for (c, stems) in [(&mut c, Some(Arc::new(set))), (&mut ref_c, None)] {
            for cmd in [
                DeckCommand::Load {
                    track: Arc::clone(&track),
                    memory: TrackMemory::default(),
                },
                DeckCommand::SetStems(stems),
                DeckCommand::StemOn(Stem::Melody, false),
                DeckCommand::Play,
            ] {
                c.send(Command::Deck(deck(), cmd)).unwrap();
            }
        }
        let diff = max_diff(
            &render(&mut r, BLOCK * 40).0,
            &render(&mut ref_r, BLOCK * 40).0,
        );
        assert!(c.deck(deck()).stems);
        assert!(diff <= 1e-4, "{rate} Hz: kicks + hats off rhythm by {diff}");
    }
    // The set computes kicks at the rate it is given, and rejects a rate of 0.
    assert!(matches!(
        StemSet::new(
            vec![0; 4].into(),
            vec![0; 4].into(),
            vec![0; 4].into(),
            2,
            0
        ),
        Err(StemError::SampleRate)
    ));
}

#[test]
fn all_knobs_left_is_silence_in_vdj() {
    let fx = Fixture::new();
    let (mut c, mut r) = loaded(&fx.mix, EqMode::Stems);
    for knob in [
        ChannelCommand::EqLow(0.0),
        ChannelCommand::EqMid(0.0),
        ChannelCommand::EqHigh(0.0),
    ] {
        c.send(Command::Channel(deck(), knob)).unwrap();
    }
    send_all(
        &mut c,
        &[DeckCommand::SetStems(Some(fx.set())), DeckCommand::Play],
    );
    let _ = render(&mut r, SETTLE_FRAMES.div_ceil(BLOCK) * BLOCK);
    assert!(c.deck(deck()).stems_active);
    let out = render(&mut r, BLOCK * 40).0;
    let peak = out.iter().fold(0.0_f32, |m, s| m.max(s.abs()));
    assert!(peak < 1e-4, "a peak of {peak}");
}

#[test]
fn centered_knobs_changeover_is_inaudible() {
    for layout in LAYOUTS {
        centered_knobs_changeover_is_inaudible_in(layout);
    }
}

fn centered_knobs_changeover_is_inaudible_in(layout: StemKnobs) {
    let fx = Fixture::new();
    let (mut c, mut r) = loaded_with(&fx.mix, EqMode::Stems, layout);
    let (mut ref_c, mut ref_r) = loaded_with(&fx.mix, EqMode::Stems, layout);
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
    assert!(
        diff <= 1e-4,
        "{layout:?}: changeover changed the output by {diff}"
    );
}

#[test]
fn changeover_has_no_click() {
    // Off-centre knobs, then LOW fully left: the stems arrive with the kicks killed.
    for layout in LAYOUTS {
        for low in [0.2, 0.0] {
            changeover_has_no_click_in(layout, low);
        }
    }
}

fn changeover_has_no_click_in(layout: StemKnobs, low: f32) {
    let fx = Fixture::new();
    let knobs = [
        ChannelCommand::EqLow(low),
        ChannelCommand::EqMid(0.9),
        ChannelCommand::EqHigh(0.0),
    ];
    let start = |stems: bool| {
        let (mut c, r) = loaded_with(&fx.mix, EqMode::Stems, layout);
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
    assert!(
        jump > 0.1,
        "{layout:?} {low}: the paths are too alike to test: {jump}"
    );

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
        Command::Deck(deck(), DeckCommand::StemOn(Stem::Kicks, false)),
        Command::Deck(deck(), DeckCommand::StemOn(Stem::Melody, false)),
        Command::Deck(deck(), DeckCommand::StemOn(Stem::Melody, true)),
        Command::Deck(deck(), DeckCommand::StemOn(Stem::Kicks, true)),
        // The other layout, then back.
        Command::Master(MasterCommand::StemKnobs(other(layout))),
        Command::Master(MasterCommand::StemKnobs(layout)),
        Command::Deck(deck(), DeckCommand::SetStems(None)),
    ] {
        c.send(cmd).unwrap();
        out.extend(render(&mut r, BLOCK * 8).0);
    }
    let step = max_step(&out);
    assert!(step <= 0.05, "{layout:?} {low}: a step of {step}");
}

fn other(layout: StemKnobs) -> StemKnobs {
    match layout {
        StemKnobs::Vdj => StemKnobs::Raw,
        StemKnobs::Raw => StemKnobs::Vdj,
    }
}

#[test]
fn stem_pads_mute_their_stem() {
    let fx = Fixture::new();
    for mode in [EqMode::Stems, EqMode::Eq] {
        for stem in Stem::ALL {
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
            let mut on = [true; 7];
            on[stem.index()] = false;
            let (mut ref_c, mut ref_r) = loaded(&fx.track_of(pad_gains(on)), mode);
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
    let raw = [Stem::Vocals, Stem::Instruments, Stem::Bass, Stem::Rhythm];
    for (on, gains) in [
        ([false, true, false, false], [0.0, 1.0, 0.0, 0.0, 0.0]),
        ([true, false, true, true], [1.0, 0.0, 1.0, 1.0, 1.0]),
    ] {
        let (mut c, mut r) = loaded(&fx.mix, EqMode::Stems);
        send_all(&mut c, &[DeckCommand::SetStems(Some(fx.set()))]);
        for (stem, on) in raw.into_iter().zip(on) {
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
        StemSet::new(v.clone(), v.clone(), v, frames, SR).unwrap()
    };
    assert!(matches!(
        StemSet::new(
            vec![0; 10].into(),
            vec![0; 10].into(),
            vec![0; 8].into(),
            5,
            SR
        ),
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
    // A set computed at another rate splits kicks at the wrong frequency: ignored too.
    let silent = vec![0_i16; FRAMES * 2].into_boxed_slice();
    let other_rate =
        Arc::new(StemSet::new(silent.clone(), silent.clone(), silent, FRAMES, 48_000).unwrap());
    send_all(
        &mut c,
        &[DeckCommand::SetStems(Some(Arc::clone(&other_rate)))],
    );
    let _ = render(&mut r, BLOCK);
    assert!(!c.deck(deck()).stems);
    c.collect_garbage();
    assert_eq!(Arc::strong_count(&other_rate), 1, "the set was discarded");

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
        start(&fx.track_of([0.0, 1.0, 1.0, 1.0, 1.0]), false),
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

/// Under keylock, LOW at 0.25 plays the 60 Hz part at 0.25 through the EQ band and through
/// the stems. Changing over early or late would play it at 1.0 (bands at unity over the
/// mix) or 0.0625 (both).
fn check_keylock_changeover(fx: &Fixture, layout: StemKnobs) {
    let start = |low: f32| {
        let (mut c, mut r) = loaded_with(&fx.mix, EqMode::Eq, layout);
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
    // The 60 Hz part at full level, as heard through keylock.
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
            "{layout:?}: window {n}: the 60 Hz part at {level} of its level"
        );
    }
}

#[test]
fn keylock_changeover_keeps_the_bass_level() {
    // Raw: LOW is the bass.
    check_keylock_changeover(&Fixture::new(), StemKnobs::Raw);
}

#[test]
fn keylock_changeover_keeps_the_kick_level() {
    // Vdj: LOW is the kicks, here the 60 Hz kick in rhythm.
    check_keylock_changeover(&Fixture::kick_and_hat(), StemKnobs::Vdj);
}

#[test]
fn split_survives_scratch_and_keylock() {
    // With the kicks pad off, the 60 Hz kick stays out of stretched, scratched and
    // backspun output: the deck plays the same as a track that never had it.
    let fx = Fixture::kick_and_hat();
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
                    DeckCommand::StemOn(Stem::Kicks, false),
                ],
            );
        }
        let _ = render(&mut r, BLOCK);
        send_all(&mut c, &[DeckCommand::Play]);
        (c, r)
    };
    // The stem engine, a reference playing the track without kicks and one playing the mix.
    let mut engines = [
        start(&fx.mix, true),
        start(&fx.track_of([1.0, 1.0, 1.0, 0.0, 1.0]), false),
        start(&fx.mix, false),
    ];
    let mut check = |cmds: &[DeckCommand], what: &str| {
        let [out, expected, full] = engines.each_mut().map(|(c, r)| {
            send_all(c, cmds);
            render(r, BLOCK * 30).0
        });
        let diff = max_diff(&out, &expected);
        assert!(diff <= 1e-4, "{what}: off by {diff}");
        assert!(max_diff(&out, &full) > 0.05, "{what}: the kick is there");
        (out, full)
    };
    let (out, full) = check(&[], "stretched");
    // Past the stretcher's latency, the kick is gone from the stretched output.
    let [kick, full_kick] = [&out, &full].map(|s| {
        let levels = bass_levels(&s[BLOCK * 20..]);
        levels.iter().sum::<f32>() / levels.len() as f32
    });
    assert!(full_kick > 0.1, "{full_kick}");
    assert!(
        kick < 0.05 * full_kick,
        "the kick at {} of its level",
        kick / full_kick
    );
    check(
        &[
            DeckCommand::JogTouch(true),
            DeckCommand::Jog {
                turns: -0.3,
                surface: JogSurface::Top,
            },
        ],
        "scratched",
    );
    check(
        &[DeckCommand::Jog {
            turns: 0.5,
            surface: JogSurface::Top,
        }],
        "scratched forward",
    );
    check(
        &[
            DeckCommand::JogTouch(false),
            DeckCommand::Seek(FRAMES as f64 / 2.0),
        ],
        "after a jump",
    );
    check(&[DeckCommand::Backspin { seconds: 0.5 }], "backspun");
}
