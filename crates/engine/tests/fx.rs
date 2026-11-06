use engine::{EffectKind, effect_for_tests};

const SR: u32 = 48_000;
/// Beat length at 120 BPM and 48 kHz.
const BEAT: f64 = 24_000.0;

/// Left-channel response to a unit impulse, `n` frames (right channel fed silence).
fn impulse(kind: EffectKind, beat: f64, n: usize) -> Vec<f32> {
    let mut fx = effect_for_tests(SR, kind);
    (0..n)
        .map(|i| {
            let x = if i == 0 { 1.0 } else { 0.0 };
            let y = fx.process(0, x, beat);
            fx.process(1, 0.0, beat);
            y
        })
        .collect()
}

#[test]
fn echo_repeats_after_half_a_beat() {
    let y = impulse(EffectKind::Echo, BEAT, 30_000);
    assert!((y[12_000] - 0.5).abs() < 1e-6, "{}", y[12_000]);
    assert!((y[24_000] - 0.25).abs() < 1e-6, "{}", y[24_000]);
    let others = y
        .iter()
        .enumerate()
        .filter(|(i, v)| *i != 12_000 && *i != 24_000 && v.abs() > 1e-6);
    assert_eq!(others.count(), 0);
}

#[test]
fn echo_time_is_clamped() {
    // A 10 s beat would need a 5 s delay; the buffer holds 2 s.
    let y = impulse(EffectKind::Echo, 480_000.0, 100_000);
    let first = y.iter().position(|v| v.abs() > 1e-6).unwrap();
    assert_eq!(first, 96_000);
}

#[test]
fn beat_fx_without_grid_use_120_bpm() {
    let reference = impulse(EffectKind::Echo, BEAT, 13_000);
    for beat in [0.0, f64::NAN, f64::INFINITY, -5.0] {
        assert_eq!(impulse(EffectKind::Echo, beat, 13_000), reference);
    }
}

#[test]
fn reverb_tail_decays() {
    let y = impulse(EffectKind::Reverb, BEAT, SR as usize * 6);
    let energy = |a: usize, b: usize| y[a..b].iter().map(|v| v * v).sum::<f32>();
    let early = energy(0, SR as usize);
    let late = energy(SR as usize * 2, SR as usize * 3);
    let end = energy(SR as usize * 5, SR as usize * 6);
    assert!(early > 0.0);
    assert!(late < early && end < late, "{early} {late} {end}");
    assert!(y.iter().all(|v| v.is_finite()));
}

#[test]
fn flanger_sweeps_within_1_to_7_ms() {
    // Feed an impulse at several points of the 4 beat LFO and find the delayed tap.
    let n = (BEAT * 4.0) as usize;
    let mut fx = effect_for_tests(SR, EffectKind::Flanger);
    let mut delays = Vec::new();
    for start in (0..n).step_by(n / 16) {
        // Fresh run up to `start`, then an impulse.
        fx.set_kind(EffectKind::Flanger);
        for _ in 0..start {
            fx.process(0, 0.0, BEAT);
            fx.process(1, 0.0, BEAT);
        }
        let y: Vec<f32> = (0..500)
            .map(|i| {
                let v = fx.process(0, if i == 0 { 1.0 } else { 0.0 }, BEAT);
                fx.process(1, 0.0, BEAT);
                v
            })
            .collect();
        // Index 0 holds the dry half; the delayed tap is the largest later value.
        let (idx, _) = y
            .iter()
            .enumerate()
            .skip(1)
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .unwrap();
        delays.push(idx as f64 * 1000.0 / f64::from(SR));
    }
    let min = delays.iter().cloned().fold(f64::MAX, f64::min);
    let max = delays.iter().cloned().fold(0.0, f64::max);
    assert!(min >= 0.9 && max <= 7.1, "{min} {max}");
    assert!(max - min > 4.0, "sweep too narrow: {min} {max}");
}

fn sine_gain(fx: &mut engine::EffectForTests, hz: f64) -> f32 {
    let mut peak = 0.0_f32;
    for i in 0..20_000 {
        let x = (std::f64::consts::TAU * hz * f64::from(i) / f64::from(SR)).sin() as f32;
        let y = fx.process(0, x, BEAT);
        fx.process(1, x, BEAT);
        if i >= 10_000 {
            peak = peak.max(y.abs());
        }
    }
    peak
}

#[test]
fn filter_amount_lowers_the_cutoff() {
    let mut fx = effect_for_tests(SR, EffectKind::Filter);
    fx.set_amount(0.0);
    let open = sine_gain(&mut fx, 5_000.0);
    fx.set_amount(1.0);
    let closed = sine_gain(&mut fx, 5_000.0);
    fx.set_amount(0.5);
    let mid = sine_gain(&mut fx, 5_000.0);
    assert!(open > 0.9, "{open}");
    assert!(closed < 0.01, "{closed}");
    assert!(mid < open && mid > closed, "{open} {mid} {closed}");
}

#[test]
fn set_kind_clears_state() {
    for kind in [
        EffectKind::Echo,
        EffectKind::Reverb,
        EffectKind::Flanger,
        EffectKind::Filter,
    ] {
        let mut fx = effect_for_tests(SR, kind);
        for _ in 0..5_000 {
            fx.process(0, 1.0, BEAT);
            fx.process(1, 1.0, BEAT);
        }
        fx.set_kind(kind);
        for _ in 0..30_000 {
            assert_eq!(fx.process(0, 0.0, BEAT), 0.0, "{kind:?}");
            assert_eq!(fx.process(1, 0.0, BEAT), 0.0, "{kind:?}");
        }
    }
}

#[test]
fn kind_ids_round_trip() {
    for kind in [
        EffectKind::Echo,
        EffectKind::Reverb,
        EffectKind::Flanger,
        EffectKind::Filter,
    ] {
        assert_eq!(EffectKind::from_id(kind.id()), Some(kind));
    }
    assert_eq!(EffectKind::from_id("nope"), None);
}

#[test]
fn effects_stay_finite_on_full_scale_noise() {
    let mut seed = 0x1234_5678_u32;
    let mut noise = move || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        if seed & 1 == 0 { 1.0_f32 } else { -1.0 }
    };
    for kind in [
        EffectKind::Echo,
        EffectKind::Reverb,
        EffectKind::Flanger,
        EffectKind::Filter,
    ] {
        let mut fx = effect_for_tests(SR, kind);
        fx.set_amount(0.7);
        for frame in 0..SR as usize * 10 {
            if frame % 512 == 0 {
                fx.flush_denormals();
            }
            for ch in 0..2 {
                let y = fx.process(ch, noise(), BEAT);
                assert!(y.is_finite() && y.abs() < 4.0, "{kind:?} {y}");
            }
        }
    }
}
