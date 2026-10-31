//! Deterministic test signal generators shared by the analysis tests.
#![allow(dead_code)]

use std::f32::consts::TAU;

use engine::TrackBuffer;

/// A mono sine of amplitude 1.0.
pub fn sine(freq: f32, secs: f32, sr: u32) -> Vec<f32> {
    let n = (secs * sr as f32) as usize;
    (0..n)
        .map(|i| (TAU * freq * i as f32 / sr as f32).sin())
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

/// Kick-like clicks: a 60 Hz sine burst with a 50 ms decay plus a short noise transient,
/// one per beat, the first at `first_beat_secs`.
pub fn clicks(bpm: f32, secs: f32, sr: u32, first_beat_secs: f32) -> Vec<f32> {
    let n = (secs * sr as f32) as usize;
    let mut out = vec![0.0f32; n];
    let period = 60.0 / bpm * sr as f32;
    let mut rng = Rng::new(0x5eed);
    let burst = (0.25 * sr as f32) as usize;
    let noise_len = (0.003 * sr as f32) as usize;
    let mut beat = 0u32;
    loop {
        let start = (first_beat_secs * sr as f32 + beat as f32 * period).round() as usize;
        if start >= n {
            break;
        }
        for k in 0..burst.min(n - start) {
            let t = k as f32 / sr as f32;
            let mut v = (TAU * 60.0 * t).sin() * (-t / 0.05).exp();
            if k < noise_len {
                v += 0.5 * rng.next_f32() * (1.0 - k as f32 / noise_len as f32);
            }
            out[start + k] += v;
        }
        beat += 1;
    }
    out
}

/// Equal-amplitude sum of sines, scaled so the peak stays below 1.
pub fn chord(notes_hz: &[f32], secs: f32, sr: u32) -> Vec<f32> {
    let mut out = vec![0.0f32; (secs * sr as f32) as usize];
    for &f in notes_hz {
        for (o, s) in out.iter_mut().zip(sine(f, secs, sr)) {
            *o += s / notes_hz.len() as f32;
        }
    }
    out
}

pub fn concat(parts: &[&[f32]]) -> Vec<f32> {
    parts.iter().flat_map(|p| p.iter().copied()).collect()
}

/// Duplicates a mono signal into both channels of a `TrackBuffer`.
pub fn stereo(mono: &[f32], sr: u32) -> TrackBuffer {
    let interleaved = mono.iter().flat_map(|&s| [s, s]).collect();
    TrackBuffer::new(sr, interleaved).expect("valid buffer")
}
