#[path = "support/synth.rs"]
mod synth;

use analysis::{Waveform, waveform};

const SR: u32 = 44_100;

#[test]
fn bucket_count_matches_length() {
    let t = synth::stereo(&synth::sine(440.0, 1.0, SR), SR);
    let w = waveform(&t, 256);
    assert_eq!(w.frames_per_bucket, 256);
    assert_eq!(w.buckets.len(), 44_100usize.div_ceil(256));
}

#[test]
fn silence_is_zero() {
    let t = synth::stereo(&vec![0.0; SR as usize], SR);
    let w = waveform(&t, 256);
    assert!(w.buckets.iter().all(|b| *b == [0, 0, 0, 0]));
}

fn mean(w: &Waveform, i: usize) -> f32 {
    // Skip the first buckets (filter settling).
    let b = &w.buckets[10..];
    b.iter().map(|b| f32::from(b[i])).sum::<f32>() / b.len() as f32
}

#[test]
fn bass_sine_lands_in_low_band() {
    // 2205 frames = exactly 3 periods of 60 Hz, so per-bucket RMS is phase-independent.
    let w = waveform(&synth::stereo(&synth::sine(60.0, 4.0, SR), SR), 2205);
    assert!(mean(&w, 1) > 200.0);
    assert!(mean(&w, 2) < 20.0);
    assert!(mean(&w, 3) < 5.0);
}

#[test]
fn treble_sine_lands_in_high_band() {
    let w = waveform(&synth::stereo(&synth::sine(8000.0, 2.0, SR), SR), 256);
    assert!(mean(&w, 3) > 200.0);
    assert!(mean(&w, 1) < 5.0);
    assert!(mean(&w, 2) < 20.0);
}

#[test]
fn peak_tracks_amplitude() {
    let loud = synth::sine(1000.0, 1.0, SR);
    let quiet: Vec<f32> = loud.iter().map(|s| s * 0.5).collect();
    let pl = waveform(&synth::stereo(&loud, SR), 256).buckets[50][0];
    let pq = waveform(&synth::stereo(&quiet, SR), 256).buckets[50][0];
    assert!(pl >= 250);
    assert!((i16::from(pq) - 128).abs() <= 3, "{pq}");
}

#[test]
fn bytes_round_trip() {
    let t = synth::stereo(&synth::chord(&[100.0, 1000.0, 5000.0], 1.0, SR), SR);
    let w = waveform(&t, 256);
    let bytes = w.to_bytes();
    assert_eq!(bytes[0], 1);
    assert_eq!(bytes.len(), 9 + 4 * w.buckets.len());
    assert_eq!(Waveform::from_bytes(&bytes).unwrap(), w);
}

#[test]
fn from_bytes_rejects_bad_input() {
    let w = Waveform {
        frames_per_bucket: 4,
        buckets: vec![[1, 2, 3, 4]; 3],
    };
    let bytes = w.to_bytes();
    assert!(Waveform::from_bytes(&[]).is_err());
    assert!(Waveform::from_bytes(&bytes[..bytes.len() - 1]).is_err());
    assert!(Waveform::from_bytes(&bytes[..5]).is_err());
    let mut bad = bytes.clone();
    bad[0] = 9;
    assert!(Waveform::from_bytes(&bad).is_err());
    let mut huge = bytes;
    huge[5..9].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(Waveform::from_bytes(&huge).is_err());
}

#[test]
fn downsample_keeps_max_peak() {
    let w = Waveform {
        frames_per_bucket: 256,
        buckets: vec![
            [10, 0, 0, 0],
            [200, 0, 0, 0],
            [30, 0, 0, 0],
            [5, 0, 0, 0],
            [7, 0, 0, 0],
        ],
    };
    let d = w.downsample(2);
    assert_eq!(d.frames_per_bucket, 512);
    assert_eq!(d.buckets.len(), 3);
    assert_eq!(d.buckets[0][0], 200);
    assert_eq!(d.buckets[1][0], 30);
    assert_eq!(d.buckets[2][0], 7);
}
