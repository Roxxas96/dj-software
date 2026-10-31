//! Deterministic test signal generators shared by the analysis tests.
//!
//! Phases and beat times are computed in `f64` and cast to `f32` at the end.
#![allow(dead_code)]

use std::f64::consts::TAU;

use engine::TrackBuffer;

fn len(secs: f32, sr: u32) -> usize {
    (f64::from(secs) * f64::from(sr)) as usize
}

/// A mono sine of amplitude 1.0.
pub fn sine(freq: f32, secs: f32, sr: u32) -> Vec<f32> {
    sine_f64(f64::from(freq), secs, sr)
}

fn sine_f64(freq: f64, secs: f32, sr: u32) -> Vec<f32> {
    (0..len(secs, sr))
        .map(|i| (TAU * freq * i as f64 / f64::from(sr)).sin() as f32)
        .collect()
}

/// Tiny xorshift PRNG so noise is reproducible without dependencies.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed.max(1))
    }

    /// Uniform in `[-1, 1)`.
    pub fn next_f32(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        ((self.0 >> 40) as f32 / (1u64 << 24) as f32) * 2.0 - 1.0
    }
}

/// Uniform white noise in `[-amp, amp)`.
pub fn noise(secs: f32, sr: u32, seed: u64, amp: f32) -> Vec<f32> {
    let mut rng = Rng::new(seed);
    (0..len(secs, sr)).map(|_| rng.next_f32() * amp).collect()
}

pub fn silence(secs: f32, sr: u32) -> Vec<f32> {
    vec![0.0; len(secs, sr)]
}

pub fn gain(sig: &[f32], g: f32) -> Vec<f32> {
    sig.iter().map(|s| s * g).collect()
}

/// Sample-wise sum; the result has the longer length.
pub fn mix(a: &[f32], b: &[f32]) -> Vec<f32> {
    (0..a.len().max(b.len()))
        .map(|i| a.get(i).copied().unwrap_or(0.0) + b.get(i).copied().unwrap_or(0.0))
        .collect()
}

/// Kick-like clicks: a 60 Hz sine burst with a 50 ms decay plus a short noise transient,
/// one per beat, the first at `first_beat_secs`. Bursts last 250 ms and overlap at high
/// BPM, so the sum may exceed 1.0 there; the output is not normalized.
pub fn clicks(bpm: f32, secs: f32, sr: u32, first_beat_secs: f32) -> Vec<f32> {
    let n = len(secs, sr);
    let sr_f = f64::from(sr);
    let mut out = vec![0.0f64; n];
    let period = 60.0 / f64::from(bpm) * sr_f;
    let mut rng = Rng::new(0x5eed);
    let burst = (0.25 * sr_f) as usize;
    let noise_len = (0.003 * sr_f) as usize;
    let mut beat = 0u32;
    loop {
        let start = (f64::from(first_beat_secs) * sr_f + f64::from(beat) * period).round() as usize;
        if start >= n {
            break;
        }
        for k in 0..burst.min(n - start) {
            let t = k as f64 / sr_f;
            let mut v = (TAU * 60.0 * t).sin() * (-t / 0.05).exp();
            if k < noise_len {
                v += 0.5 * f64::from(rng.next_f32()) * (1.0 - k as f64 / noise_len as f64);
            }
            out[start + k] += v;
        }
        beat += 1;
    }
    out.into_iter().map(|v| v as f32).collect()
}

/// Equal-amplitude sum of sines, scaled so the peak stays below 1.
pub fn chord(notes_hz: &[f32], secs: f32, sr: u32) -> Vec<f32> {
    let mut out = vec![0.0f32; len(secs, sr)];
    for &f in notes_hz {
        for (o, s) in out.iter_mut().zip(sine(f, secs, sr)) {
            *o += s / notes_hz.len() as f32;
        }
    }
    out
}

/// Notes with `partials` harmonics each, harmonic `h` (1-based) at amplitude `1/h`
/// (decaying), normalized so the peak stays below 1.
pub fn chord_harmonic(notes_hz: &[f64], secs: f32, sr: u32, partials: u32) -> Vec<f32> {
    let norm: f64 = notes_hz.len() as f64 * (1..=partials).map(|h| 1.0 / f64::from(h)).sum::<f64>();
    let mut out = vec![0.0f32; len(secs, sr)];
    for &f in notes_hz {
        for h in 1..=partials {
            let amp = (1.0 / f64::from(h) / norm) as f32;
            for (o, s) in out.iter_mut().zip(sine_f64(f * f64::from(h), secs, sr)) {
                *o += s * amp;
            }
        }
    }
    out
}

/// Frequency of a MIDI note number (69.0 = A4 = 440 Hz).
pub fn note_hz(midi: f64) -> f64 {
    440.0 * ((midi - 69.0) / 12.0).exp2()
}

pub fn concat(parts: &[&[f32]]) -> Vec<f32> {
    parts.iter().flat_map(|p| p.iter().copied()).collect()
}

/// Duplicates a mono signal into both channels of a `TrackBuffer`.
pub fn stereo(mono: &[f32], sr: u32) -> TrackBuffer {
    stereo_lr(mono, mono, sr)
}

/// Builds a `TrackBuffer` from separate channels (the shorter is padded with silence).
pub fn stereo_lr(l: &[f32], r: &[f32], sr: u32) -> TrackBuffer {
    let n = l.len().max(r.len());
    let interleaved = (0..n)
        .flat_map(|i| {
            [
                l.get(i).copied().unwrap_or(0.0),
                r.get(i).copied().unwrap_or(0.0),
            ]
        })
        .collect();
    TrackBuffer::new(sr, interleaved).expect("valid buffer")
}

/// Beat times in seconds: every `step_beats` beats starting at beat `offset_beats`, the
/// first beat (beat 0) at `first_beat_secs`, up to `secs`.
pub fn beat_times(
    bpm: f64,
    secs: f32,
    first_beat_secs: f64,
    step_beats: f64,
    offset_beats: f64,
) -> Vec<f64> {
    let period = 60.0 / bpm;
    (0..)
        .map(|k| first_beat_secs + (offset_beats + f64::from(k) * step_beats) * period)
        .take_while(|&t| t < f64::from(secs))
        .collect()
}

/// Places a copy of `hit` at each time (seconds, rounded to the nearest frame), summing
/// overlaps; the output is `secs` long and not normalized.
pub fn place(hit: &[f32], times_secs: &[f64], secs: f32, sr: u32) -> Vec<f32> {
    let n = len(secs, sr);
    let mut out = vec![0.0f32; n];
    for &t in times_secs {
        let start = (t * f64::from(sr)).round() as usize;
        for (o, h) in out.iter_mut().skip(start).zip(hit) {
            *o += h;
        }
    }
    out
}

/// Snare-like hit: a 200 Hz tone (decay 40 ms) plus white noise (decay 80 ms), 250 ms long.
pub fn snare_hit(sr: u32, seed: u64) -> Vec<f32> {
    let sr_f = f64::from(sr);
    let mut rng = Rng::new(seed);
    (0..(0.25 * sr_f) as usize)
        .map(|k| {
            let t = k as f64 / sr_f;
            let tone = 0.4 * (TAU * 200.0 * t).sin() * (-t / 0.04).exp();
            let noise = 0.6 * f64::from(rng.next_f32()) * (-t / 0.08).exp();
            (tone + noise) as f32
        })
        .collect()
}

/// Hi-hat-like hit: first-differenced (high-passed) white noise with a 15 ms decay, 60 ms long.
pub fn hat_hit(sr: u32, seed: u64) -> Vec<f32> {
    let sr_f = f64::from(sr);
    let mut rng = Rng::new(seed);
    let mut prev = 0.0f64;
    (0..(0.06 * sr_f) as usize)
        .map(|k| {
            let t = k as f64 / sr_f;
            let x = f64::from(rng.next_f32());
            let hp = x - prev;
            prev = x;
            (0.3 * hp * (-t / 0.015).exp()) as f32
        })
        .collect()
}
