//! Three-band waveform overview: per bucket `[peak, low, mid, high]`, each `0..=255`.

use std::f64::consts::{FRAC_1_SQRT_2, TAU};

use engine::TrackBuffer;
use thiserror::Error;

/// Default bucket size in frames.
pub const DEFAULT_FRAMES_PER_BUCKET: u32 = 256;

/// Same crossover points as the mixer's isolator.
const LOW_MID_HZ: f64 = 250.0;
const MID_HIGH_HZ: f64 = 2500.0;

const BLOB_VERSION: u8 = 1;
const HEADER_LEN: usize = 9;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum WaveformError {
    #[error("waveform blob is truncated")]
    Truncated,
    #[error("unknown waveform blob version {0}")]
    UnknownVersion(u8),
}

/// A downsampled overview of a track.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Waveform {
    pub frames_per_bucket: u32,
    /// `[peak, low, mid, high]` per bucket.
    pub buckets: Vec<[u8; 4]>,
}

impl Waveform {
    /// Serializes as: version (1 byte), frames per bucket (u32 LE), bucket count (u32 LE),
    /// then 4 bytes per bucket.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(HEADER_LEN + 4 * self.buckets.len());
        out.push(BLOB_VERSION);
        out.extend_from_slice(&self.frames_per_bucket.to_le_bytes());
        out.extend_from_slice(&(self.buckets.len() as u32).to_le_bytes());
        for b in &self.buckets {
            out.extend_from_slice(b);
        }
        out
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, WaveformError> {
        let (&version, rest) = bytes.split_first().ok_or(WaveformError::Truncated)?;
        if version != BLOB_VERSION {
            return Err(WaveformError::UnknownVersion(version));
        }
        if rest.len() < 8 {
            return Err(WaveformError::Truncated);
        }
        let frames_per_bucket = u32::from_le_bytes(rest[0..4].try_into().unwrap());
        let count = u32::from_le_bytes(rest[4..8].try_into().unwrap()) as usize;
        let body = &rest[8..];
        if count.checked_mul(4) != Some(body.len()) {
            return Err(WaveformError::Truncated);
        }
        let buckets = body.as_chunks::<4>().0.to_vec();
        Ok(Self {
            frames_per_bucket,
            buckets,
        })
    }

    /// Merges every `factor` buckets: the peak is the maximum, each band the RMS of the group.
    pub fn downsample(&self, factor: u32) -> Waveform {
        let factor = factor.max(1);
        let buckets = self
            .buckets
            .chunks(factor as usize)
            .map(|group| {
                let mut out = [0u8; 4];
                out[0] = group.iter().map(|b| b[0]).max().unwrap_or(0);
                for band in 1..4 {
                    let sum: f64 = group.iter().map(|b| f64::from(b[band]).powi(2)).sum();
                    out[band] = (sum / group.len() as f64).sqrt().round().min(255.0) as u8;
                }
                out
            })
            .collect();
        Waveform {
            frames_per_bucket: self.frames_per_bucket.saturating_mul(factor),
            buckets,
        }
    }
}

#[derive(Clone, Copy)]
struct Biquad {
    b: [f64; 3],
    a: [f64; 2],
    z: [f64; 2],
}

impl Biquad {
    fn new(high: bool, freq: f64, sr: f64) -> Self {
        let freq = freq.clamp(1.0, 0.49 * sr);
        let w0 = TAU * freq / sr;
        let (cos, alpha) = (w0.cos(), w0.sin() / (2.0 * FRAC_1_SQRT_2));
        let b = if high {
            (1.0 + cos) / 2.0
        } else {
            (1.0 - cos) / 2.0
        };
        let b1 = if high { -2.0 * b } else { 2.0 * b };
        let a0 = 1.0 + alpha;
        Self {
            b: [b / a0, b1 / a0, b / a0],
            a: [-2.0 * cos / a0, (1.0 - alpha) / a0],
            z: [0.0; 2],
        }
    }

    fn run(&mut self, x: f64) -> f64 {
        let y = self.b[0] * x + self.z[0];
        self.z[0] = self.b[1] * x - self.a[0] * y + self.z[1];
        self.z[1] = self.b[2] * x - self.a[1] * y;
        y
    }
}

/// Linkwitz-Riley 4th order: two cascaded Butterworth sections.
#[derive(Clone, Copy)]
struct Lr4([Biquad; 2]);

impl Lr4 {
    fn new(high: bool, freq: f64, sr: f64) -> Self {
        let s = Biquad::new(high, freq, sr);
        Self([s, s])
    }

    fn run(&mut self, x: f64) -> f64 {
        let y = self.0[0].run(x);
        self.0[1].run(y)
    }
}

/// Computes the waveform with `frames_per_bucket` frames per bucket (the last may be partial).
pub fn waveform(track: &TrackBuffer, frames_per_bucket: u32) -> Waveform {
    let fpb = frames_per_bucket.max(1) as usize;
    let sr = f64::from(track.sample_rate());
    let mut low = Lr4::new(false, LOW_MID_HZ, sr);
    let mut mid_hp = Lr4::new(true, LOW_MID_HZ, sr);
    let mut mid_lp = Lr4::new(false, MID_HIGH_HZ, sr);
    let mut high = Lr4::new(true, MID_HIGH_HZ, sr);

    let count = track.frames().div_ceil(fpb);
    // (peak, low, mid, high) per bucket; bands hold RMS.
    let mut raw: Vec<[f64; 4]> = Vec::with_capacity(count);
    for start in (0..track.frames()).step_by(fpb) {
        let end = (start + fpb).min(track.frames());
        let mut peak = 0.0f64;
        let mut sq = [0.0f64; 3];
        for i in start..end {
            let [l, r] = track.frame(i);
            peak = peak.max(f64::from(l.abs().max(r.abs())));
            let x = f64::from(l + r) * 0.5;
            let lo = low.run(x);
            let mi = mid_lp.run(mid_hp.run(x));
            let hi = high.run(x);
            sq[0] += lo * lo;
            sq[1] += mi * mi;
            sq[2] += hi * hi;
        }
        let n = (end - start) as f64;
        raw.push([
            peak,
            (sq[0] / n).sqrt(),
            (sq[1] / n).sqrt(),
            (sq[2] / n).sqrt(),
        ]);
    }

    let loudest = raw
        .iter()
        .flat_map(|b| b[1..].iter().copied())
        .fold(0.0f64, f64::max);
    let scale = if loudest > 0.0 { 255.0 / loudest } else { 0.0 };
    let buckets = raw
        .iter()
        .map(|b| {
            [
                (b[0] * 255.0).round().clamp(0.0, 255.0) as u8,
                (b[1] * scale).round().clamp(0.0, 255.0) as u8,
                (b[2] * scale).round().clamp(0.0, 255.0) as u8,
                (b[3] * scale).round().clamp(0.0, 255.0) as u8,
            ]
        })
        .collect();
    Waveform {
        frames_per_bucket: fpb as u32,
        buckets,
    }
}
