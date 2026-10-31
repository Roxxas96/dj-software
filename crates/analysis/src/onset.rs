//! Onset-strength envelope: log-magnitude spectral flux of a mono mix resampled to
//! [`ENVELOPE_SR`] Hz.

use rustfft::FftPlanner;
use rustfft::num_complex::Complex;

use engine::TrackBuffer;

/// Rate the track is resampled to before the STFT.
pub const ENVELOPE_SR: f64 = 11_025.0;
/// STFT window length in samples at [`ENVELOPE_SR`].
const WINDOW: usize = 1024;
/// STFT hop in samples at [`ENVELOPE_SR`]: about 86 envelope frames per second.
pub const HOP: usize = 128;
/// Envelope frames per second.
pub const FRAME_RATE: f64 = ENVELOPE_SR / HOP as f64;
/// Length of the moving mean subtracted from the flux.
const MEAN_SECS: f64 = 0.5;
/// Log compression: `ln(1 + GAMMA * |X|)` with `|X|` normalized so a full-scale sine is
/// about 1. Stronger compression balances quiet onsets against loud ones but makes the flux
/// peak earlier the louder the onset is (the log saturates while the transient is still on
/// the window's leading slope); 10 keeps that level dependence to a few milliseconds.
const GAMMA: f32 = 10.0;
/// The flux peaks this long before the onset it reacts to: measured on kick, snare and hat
/// hits at levels from 0 to -60 dBFS (14-17 ms, 7-9 ms and 11.5 ms early respectively).
pub const LATENCY_SECS: f64 = 0.014;

/// The onset envelope; frame `t` is centered on resampled sample `t * HOP` (the signal is
/// padded with half a window of silence at the start).
pub struct Envelope {
    pub frames: Vec<f64>,
}

/// Mono-mixes `track`, resamples it linearly to [`ENVELOPE_SR`] and computes the half-wave
/// rectified, mean-removed spectral flux. Streams the resampled signal through one STFT
/// window, so memory is bounded by the envelope (one value per hop), not the track.
/// Returns `None` if the track is silent.
pub fn onset_envelope(track: &TrackBuffer) -> Option<Envelope> {
    let frames = track.frames();
    if frames < 2 {
        return None;
    }
    let step = f64::from(track.sample_rate()) / ENVELOPE_SR;
    let out_len = ((frames - 1) as f64 / step).floor() as usize + 1;

    let fft = FftPlanner::<f32>::new().plan_fft_forward(WINDOW);
    let mut scratch = vec![Complex::default(); fft.get_inplace_scratch_len()];
    let mut spectrum = vec![Complex::default(); WINDOW];
    let hann: Vec<f32> = (0..WINDOW)
        .map(|i| {
            let x = std::f64::consts::TAU * i as f64 / WINDOW as f64;
            (0.5 - 0.5 * x.cos()) as f32
        })
        .collect();
    // Normalize so a full-scale sine peaks near 1: the Hann window sums to WINDOW / 2,
    // and a real sine splits its energy between two bins.
    let norm = 4.0 / WINDOW as f32;

    let bins = WINDOW / 2 + 1;
    let mut prev_log = vec![0.0f32; bins];
    let mut cur_log = vec![0.0f32; bins];
    let mut have_prev = false;
    let mut flux = Vec::with_capacity(out_len / HOP + 2);
    let mut peak = 0.0f32;

    // Sliding window over the resampled signal, starting with half a window of padding.
    let mut window: Vec<f32> = vec![0.0; WINDOW / 2];
    window.reserve(WINDOW + HOP);
    let mono = |i: usize| {
        let [l, r] = track.frame(i);
        0.5 * (l + r)
    };
    let pad_tail = WINDOW / 2;
    for j in 0..out_len + pad_tail {
        let s = if j < out_len {
            let x = j as f64 * step;
            let i = (x.floor() as usize).min(frames - 1);
            let f = (x - i as f64) as f32;
            let a = mono(i);
            let b = if i + 1 < frames { mono(i + 1) } else { a };
            a + f * (b - a)
        } else {
            0.0
        };
        peak = peak.max(s.abs());
        window.push(s);
        if window.len() < WINDOW {
            continue;
        }
        for ((c, &w), &h) in spectrum.iter_mut().zip(&window).zip(&hann) {
            *c = Complex::new(w * h, 0.0);
        }
        fft.process_with_scratch(&mut spectrum, &mut scratch);
        for (l, c) in cur_log.iter_mut().zip(&spectrum) {
            *l = (GAMMA * norm * c.norm()).ln_1p();
        }
        let value = if have_prev {
            cur_log
                .iter()
                .zip(&prev_log)
                .map(|(c, p)| (c - p).max(0.0))
                .sum()
        } else {
            0.0
        };
        flux.push(value);
        std::mem::swap(&mut prev_log, &mut cur_log);
        have_prev = true;
        window.drain(..HOP);
    }

    if peak < 1e-5 || flux.is_empty() {
        return None;
    }
    let frames = subtract_moving_mean(&flux);
    if frames.iter().all(|&v| v <= 0.0) {
        return None;
    }
    Some(Envelope { frames })
}

/// Subtracts a centered moving mean of [`MEAN_SECS`] and half-wave rectifies.
fn subtract_moving_mean(x: &[f32]) -> Vec<f64> {
    let half = ((MEAN_SECS * FRAME_RATE) / 2.0).round() as usize;
    let mut prefix = Vec::with_capacity(x.len() + 1);
    prefix.push(0.0f64);
    let mut acc = 0.0f64;
    for &v in x {
        acc += f64::from(v);
        prefix.push(acc);
    }
    (0..x.len())
        .map(|i| {
            let lo = i.saturating_sub(half);
            let hi = (i + half + 1).min(x.len());
            let mean = (prefix[hi] - prefix[lo]) / (hi - lo) as f64;
            (f64::from(x[i]) - mean).max(0.0)
        })
        .collect()
}
