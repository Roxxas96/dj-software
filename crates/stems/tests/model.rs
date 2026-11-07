//! The real separator, on a short synthetic fixture. Needs the filled model: it runs in the
//! owner's hands-on, never in CI.
//!
//! ```text
//! open-jockey stems setup
//! cargo nextest run -p stems --run-ignored only model_separator
//! ```
//!
//! `OPEN_JOCKEY_STEMS_MODEL` may point at a filled model elsewhere, and
//! `OPEN_JOCKEY_STEMS_COREML_CACHE` at the CoreML cache to use.
#![cfg(feature = "model")]

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use engine::{Stem, TrackBuffer};
use stems::{ModelOptions, ModelSeparator, Separator};

const RATE: u32 = 44_100;

/// 12 s (two chunks and a short third): a 55 Hz bass sine with a slow swell, and a
/// 2 kHz tone gated in short bursts.
fn fixture() -> TrackBuffer {
    let frames = 12 * RATE as usize;
    let tau = std::f32::consts::TAU;
    let mut samples = Vec::with_capacity(frames * 2);
    for i in 0..frames {
        let t = i as f32 / RATE as f32;
        let bass = 0.4 * (tau * 55.0 * t).sin() * (0.75 + 0.25 * (tau * 0.5 * t).sin());
        let gate = if (t * 4.0).fract() < 0.1 { 0.1 } else { 0.0 };
        let tone = gate * (tau * 2_000.0 * t).sin();
        samples.extend([bass + tone, bass + tone]);
    }
    TrackBuffer::new(RATE, samples).unwrap()
}

#[test]
#[ignore = "needs the filled model: run `open-jockey stems setup`"]
fn model_separator_on_a_short_fixture() {
    let model = std::env::var_os("OPEN_JOCKEY_STEMS_MODEL")
        .map(PathBuf::from)
        .unwrap_or_else(stems::model_path);
    assert!(model.is_file(), "no model at {}", model.display());
    let options = ModelOptions {
        coreml_cache: std::env::var_os("OPEN_JOCKEY_STEMS_COREML_CACHE").map(PathBuf::from),
        ..ModelOptions::default()
    };
    let started = Instant::now();
    let mut separator = ModelSeparator::new(&model, &options).unwrap();
    eprintln!("session {:.1} s", started.elapsed().as_secs_f32());

    let track = fixture();
    let started = Instant::now();
    let set = separator
        .separate(&track, &|_| {}, &AtomicBool::new(false))
        .unwrap();
    eprintln!("separation {:.1} s", started.elapsed().as_secs_f32());
    assert_eq!(set.frames(), track.frames());

    let energy = |stem: Stem| -> f64 {
        set.stored(stem)
            .unwrap()
            .iter()
            .map(|&v| (f64::from(v) / 32_768.0).powi(2))
            .sum()
    };
    let mix: f64 = (0..track.frames())
        .flat_map(|i| track.frame(i))
        .map(|v| f64::from(v).powi(2))
        .sum();
    let [vocals, bass, rhythm] = [Stem::Vocals, Stem::Bass, Stem::Rhythm].map(energy);
    eprintln!(
        "energy / mix: vocals {:.3}, bass {:.3}, rhythm {:.3}",
        vocals / mix,
        bass / mix,
        rhythm / mix
    );
    // The model puts a low sine in the bass stem.
    assert!(bass > 0.5 * mix, "bass {bass} of {mix}");
    assert!(bass > vocals && bass > rhythm);
}
