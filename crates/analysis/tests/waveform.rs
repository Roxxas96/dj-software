#[path = "support/synth.rs"]
mod synth;

use analysis::{Waveform, WaveformError, waveform};

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
    let mut huge = bytes.clone();
    huge[5..9].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(Waveform::from_bytes(&huge).is_err());
    let mut zero = bytes.clone();
    zero[1..5].copy_from_slice(&0u32.to_le_bytes());
    assert_eq!(
        Waveform::from_bytes(&zero),
        Err(WaveformError::ZeroFramesPerBucket)
    );
    let mut long = bytes.clone();
    long.push(0);
    assert_eq!(Waveform::from_bytes(&long), Err(WaveformError::BadLength));
    assert_eq!(
        Waveform::from_bytes(&bytes[..bytes.len() - 1]),
        Err(WaveformError::Truncated)
    );
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

#[test]
fn downsample_rms_combines_bands() {
    let w = Waveform {
        frames_per_bucket: 256,
        buckets: vec![[0, 100, 0, 0], [0, 0, 0, 0]],
    };
    let d = w.downsample(2);
    assert_eq!(d.buckets[0][1], 71);
}

#[test]
fn low_band_is_stable_at_default_bucket_size() {
    let w = waveform(&synth::stereo(&synth::sine(60.0, 2.0, SR), SR), 256);
    let lows: Vec<f32> = w.buckets[40..].iter().map(|b| f32::from(b[1])).collect();
    let max = lows.iter().copied().fold(0.0, f32::max);
    let min = lows.iter().copied().fold(f32::MAX, f32::min);
    assert!(max / min < 1.05, "max {max} min {min}");
}

#[test]
fn mid_sine_lands_in_mid_band() {
    let w = waveform(&synth::stereo(&synth::sine(1000.0, 2.0, SR), SR), 256);
    assert!(mean(&w, 2) > 200.0);
    assert!(mean(&w, 1) < 20.0);
    assert!(mean(&w, 3) < 20.0);
}

#[test]
fn bands_route_correctly_at_48k() {
    let sr = 48_000;
    for (freq, band) in [(60.0, 1), (1000.0, 2), (8000.0, 3)] {
        let w = waveform(&synth::stereo(&synth::sine(freq, 2.0, sr), sr), 256);
        for other in 1..4 {
            if other != band {
                assert!(mean(&w, band) > 4.0 * mean(&w, other), "{freq} Hz");
            }
        }
    }
}

#[test]
fn loudest_band_is_normalized_to_255() {
    let quiet = synth::gain(&synth::chord(&[100.0, 1000.0, 5000.0], 2.0, SR), 0.01);
    let w = waveform(&synth::stereo(&quiet, SR), 256);
    let max = w.buckets.iter().flat_map(|b| b[1..].iter().copied()).max();
    assert_eq!(max, Some(255));
}

#[test]
fn band_values_do_not_depend_on_bucket_size() {
    let t = synth::stereo(&synth::sine(1000.0, 3.0, SR), SR);
    let a = waveform(&t, 256);
    let b = waveform(&t, 2205);
    let ma = mean(&a, 2);
    let mb =
        b.buckets[3..].iter().map(|x| f32::from(x[2])).sum::<f32>() / b.buckets[3..].len() as f32;
    assert!((ma - mb).abs() < 5.0, "{ma} vs {mb}");
}

#[test]
fn non_finite_samples_do_not_poison_later_buckets() {
    let mut s = synth::sine(440.0, 1.0, SR);
    s[1000] = f32::NAN;
    s[1001] = f32::INFINITY;
    let w = waveform(&synth::stereo(&s, SR), 256);
    assert!(w.buckets[100][0] > 0);
    assert!(w.buckets[100][1..].iter().any(|&v| v > 0));
}
