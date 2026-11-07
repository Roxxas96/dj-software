//! The synthetic separator: the stand-in for the model in tests and CI.

use std::cell::Cell;
use std::sync::atomic::{AtomicBool, Ordering};

use engine::{Stem, StemSet, TrackBuffer};
use stems::{SeparateError, Separator, SyntheticSeparator};

const RATE: u32 = 44_100;

/// Interleaved stereo: a 60 Hz, a 1 kHz and a 10 kHz sine, `seconds` long.
fn three_tones(seconds: f32) -> TrackBuffer {
    let frames = (seconds * RATE as f32) as usize;
    let mut samples = Vec::with_capacity(frames * 2);
    for i in 0..frames {
        let t = i as f32 / RATE as f32;
        let tau = std::f32::consts::TAU;
        let v = 0.25 * (tau * 60.0 * t).sin()
            + 0.25 * (tau * 1_000.0 * t).sin()
            + 0.25 * (tau * 10_000.0 * t).sin();
        samples.extend([v, -v]);
    }
    TrackBuffer::new(RATE, samples).unwrap()
}

fn stored(set: &StemSet, stem: Stem) -> Vec<f32> {
    set.stored(stem)
        .unwrap()
        .iter()
        .map(|&v| f32::from(v) / 32_768.0)
        .collect()
}

fn energy(samples: &[f32]) -> f64 {
    samples.iter().map(|&v| f64::from(v).powi(2)).sum()
}

/// Energy of one sine of `freq` Hz in `samples` (left channel), by correlation.
fn tone_energy(samples: &[f32], freq: f32) -> f64 {
    let (mut re, mut im) = (0.0f64, 0.0f64);
    for (i, frame) in samples.as_chunks::<2>().0.iter().enumerate() {
        let phase = f64::from(std::f32::consts::TAU * freq) * i as f64 / f64::from(RATE);
        re += f64::from(frame[0]) * phase.cos();
        im += f64::from(frame[0]) * phase.sin();
    }
    (re * re + im * im) / (samples.len() / 2) as f64
}

#[test]
fn synthetic_split_sums_with_residual_to_the_mix() {
    let track = three_tones(2.0);
    let set = SyntheticSeparator::new()
        .separate(&track, &|_| {}, &AtomicBool::new(false))
        .unwrap();
    assert_eq!(set.frames(), track.frames());
    assert!(set.stored(Stem::Instruments).is_none());

    let [vocals, bass, rhythm] = [Stem::Vocals, Stem::Bass, Stem::Rhythm].map(|s| stored(&set, s));
    let mix: Vec<f32> = (0..track.frames()).flat_map(|i| track.frame(i)).collect();
    // Instruments is the residual, exactly as the engine derives it.
    let instruments: Vec<f32> = (0..mix.len())
        .map(|i| mix[i] - (vocals[i] + bass[i] + rhythm[i]))
        .collect();
    for i in 0..mix.len() {
        let sum = vocals[i] + bass[i] + rhythm[i] + instruments[i];
        assert!(
            (sum - mix[i]).abs() < 1e-6,
            "sample {i}: {sum} vs {}",
            mix[i]
        );
    }

    // Each band lands where it belongs: the stems are a real split, not one copy of the mix.
    let low = tone_energy(&bass, 60.0);
    assert!(low > 10.0 * tone_energy(&vocals, 60.0), "60 Hz is bass");
    assert!(low > 10.0 * tone_energy(&rhythm, 60.0), "60 Hz is bass");
    let mid = tone_energy(&vocals, 1_000.0);
    assert!(mid > 10.0 * tone_energy(&bass, 1_000.0), "1 kHz is vocals");
    assert!(
        mid > 10.0 * tone_energy(&rhythm, 1_000.0),
        "1 kHz is vocals"
    );
    let high = tone_energy(&rhythm, 10_000.0);
    assert!(
        high > 10.0 * tone_energy(&bass, 10_000.0),
        "10 kHz is rhythm"
    );
    assert!(
        high > 10.0 * tone_energy(&vocals, 10_000.0),
        "10 kHz is rhythm"
    );
    // The residual carries what the three bands leave out, but not the whole mix.
    assert!(energy(&instruments) < 0.5 * energy(&mix));
}

#[test]
fn cancel_stops_within_a_chunk() {
    let track = three_tones(10.0);
    let cancel = AtomicBool::new(false);
    let calls = Cell::new(0);
    let progress = |fraction: f32| {
        assert!((0.0..=1.0).contains(&fraction));
        calls.set(calls.get() + 1);
        // Cancel as the first chunk reports.
        cancel.store(true, Ordering::Relaxed);
    };
    let result = SyntheticSeparator::new().separate(&track, &progress, &cancel);
    assert!(
        matches!(result, Err(SeparateError::Cancelled)),
        "{result:?}"
    );
    // The flag was raised during chunk 1; chunk 2 must not run to completion.
    assert_eq!(calls.get(), 1);

    // A flag already set stops before any chunk.
    calls.set(0);
    let result = SyntheticSeparator::new().separate(&track, &progress, &cancel);
    assert!(matches!(result, Err(SeparateError::Cancelled)));
    assert_eq!(calls.get(), 0);
}

#[test]
fn progress_reaches_one() {
    let track = three_tones(3.0);
    let last = Cell::new(0.0);
    SyntheticSeparator::new()
        .separate(&track, &|f| last.set(f), &AtomicBool::new(false))
        .unwrap();
    assert_eq!(last.get(), 1.0);
}

#[test]
fn twenty_minutes_is_the_limit() {
    assert!(stems::check_length(20 * 60 * 48_000, 48_000).is_ok());
    let error = stems::check_length(20 * 60 * 48_000 + 1, 48_000).unwrap_err();
    assert!(matches!(error, SeparateError::TooLong { .. }), "{error:?}");
    assert!(error.to_string().contains("20 minutes"), "{error}");
}
