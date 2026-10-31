//! Tempo and constant-beatgrid detection from the onset envelope.
//!
//! Autocorrelation of the onset envelope, weighted towards 120 BPM with harmonic
//! reinforcement, folded into the configured BPM range, then refined by maximizing a comb
//! over the envelope. Written from first principles for this project.

use engine::TrackBuffer;

use crate::onset::{ENVELOPE_SR, FRAME_RATE, HOP, LATENCY_SECS, onset_envelope};

/// The tempo range results are folded into by doubling or halving.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BpmRange {
    pub min: f64,
    pub max: f64,
}

impl Default for BpmRange {
    /// 90–180 BPM: covers dubstep (~140) and drum & bass (~174) without half/double-time
    /// ambiguity.
    fn default() -> Self {
        Self {
            min: 90.0,
            max: 180.0,
        }
    }
}

/// A constant-tempo grid: beat `n` lands at `first_beat_frames + n * 60 * sr / bpm`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Beatgrid {
    pub bpm: f64,
    /// The earliest beat at or after frame 0, in frames at the track's native sample rate.
    pub first_beat_frames: f64,
}

/// Shorter tracks return `None`.
const MIN_SECS: f64 = 10.0;
/// Sample rates outside this range return `None`, keeping the work bounded.
const MIN_SR: u32 = 8_000;
const MAX_SR: u32 = 384_000;
/// `BpmRange` bounds must lie within this span; wider ones return `None`, keeping the comb
/// refinement (whose step count grows with the tempo) bounded.
const RANGE_MIN_BPM: f64 = 30.0;
const RANGE_MAX_BPM: f64 = 480.0;
/// Autocorrelation lags cover this tempo range.
const SEARCH_MIN_BPM: f64 = 60.0;
const SEARCH_MAX_BPM: f64 = 240.0;
/// Center and width (in octaves) of the log-Gaussian tempo prior.
const PRIOR_BPM: f64 = 120.0;
const PRIOR_OCTAVES: f64 = 1.0;
/// Weight of the 2x and 0.5x lag autocorrelation added to each lag.
const HARMONIC_WEIGHT: f64 = 0.5;
/// The comb refinement searches ± this fraction of the tempo ...
const REFINE_SPAN: f64 = 0.005;
/// ... in steps of this many BPM.
const REFINE_STEP_BPM: f64 = 0.01;
/// Phase resolution (envelope frames) while refining the tempo, and for the final phase.
const COARSE_PHASE_STEP: f64 = 0.25;
const FINE_PHASE_STEP: f64 = 0.02;
/// Tempos within this distance of an integer are snapped to it.
const SNAP_BPM: f64 = 0.03;

/// Detects a constant tempo and beat phase. Returns `None` for silence, tracks shorter
/// than 10 s, sample rates outside 8–384 kHz, a range that is empty or reaches outside
/// 30–480 BPM, or when no tempo candidate fits `range`.
pub fn detect_beatgrid(track: &TrackBuffer, range: BpmRange) -> Option<Beatgrid> {
    let sr = track.sample_rate();
    if !(MIN_SR..=MAX_SR).contains(&sr) {
        return None;
    }
    let valid_range = range.min >= RANGE_MIN_BPM && range.max <= RANGE_MAX_BPM;
    if !valid_range || range.max <= range.min {
        return None;
    }
    if (track.frames() as f64) < MIN_SECS * f64::from(sr) {
        return None;
    }
    let env = onset_envelope(track)?.frames;

    let bpm = estimate_tempo(&env, range)?;
    let bpm = refine_tempo(&env, bpm);
    let bpm = if (bpm - bpm.round()).abs() <= SNAP_BPM {
        bpm.round()
    } else {
        bpm
    };

    // Phase is searched with the final (possibly snapped) tempo so the grid lines up
    // across the whole track.
    let period = bpm_to_lag(bpm);
    let (phase, _) = best_phase(&env, period, FINE_PHASE_STEP);
    let sr = f64::from(sr);
    let frames_per_env_frame = HOP as f64 * sr / ENVELOPE_SR;
    let period_frames = 60.0 * sr / bpm;
    let first = (phase * frames_per_env_frame + LATENCY_SECS * sr).rem_euclid(period_frames);
    Some(Beatgrid {
        bpm,
        first_beat_frames: first,
    })
}

fn bpm_to_lag(bpm: f64) -> f64 {
    60.0 * FRAME_RATE / bpm
}

fn lag_to_bpm(lag: f64) -> f64 {
    60.0 * FRAME_RATE / lag
}

/// Doubles or halves `bpm` until it lies in `range`; `None` if it skips over a range
/// narrower than an octave.
fn fold(mut bpm: f64, range: BpmRange) -> Option<f64> {
    while bpm < range.min {
        bpm *= 2.0;
    }
    while bpm > range.max {
        bpm /= 2.0;
    }
    (bpm >= range.min).then_some(bpm)
}

/// Linear interpolation of `x` at fractional index `i` (0 outside).
fn interp(x: &[f64], i: f64) -> f64 {
    if i < 0.0 {
        return 0.0;
    }
    let k = i.floor() as usize;
    let f = i - k as f64;
    let a = x.get(k).copied().unwrap_or(0.0);
    let b = x.get(k + 1).copied().unwrap_or(0.0);
    a + f * (b - a)
}

/// Step 2: weighted, harmonically reinforced autocorrelation; the strongest candidate
/// after folding into `range`, refined by parabolic interpolation.
fn estimate_tempo(env: &[f64], range: BpmRange) -> Option<f64> {
    let min_lag = bpm_to_lag(SEARCH_MAX_BPM).floor().max(1.0) as usize;
    let max_lag = bpm_to_lag(SEARCH_MIN_BPM).ceil() as usize;
    let acf_len = 2 * (max_lag + 1) + 1;
    if env.len() <= acf_len {
        return None;
    }
    // Unbiased autocorrelation up to twice the longest lag (for harmonic reinforcement).
    let acf: Vec<f64> = (0..acf_len)
        .map(|lag| {
            let n = env.len() - lag;
            let sum: f64 = env[..n].iter().zip(&env[lag..]).map(|(a, b)| a * b).sum();
            sum / n as f64
        })
        .collect();

    // Score lags min_lag - 1 ..= max_lag + 1 so every lag in range has both neighbors.
    let score = |lag: usize| -> f64 {
        let l = lag as f64;
        let octaves = (lag_to_bpm(l) / PRIOR_BPM).log2() / PRIOR_OCTAVES;
        let weight = (-0.5 * octaves * octaves).exp();
        weight * (acf[lag] + HARMONIC_WEIGHT * (acf[2 * lag] + interp(&acf, l / 2.0)))
    };
    let scores: Vec<f64> = (min_lag - 1..=max_lag + 1).map(score).collect();
    let at = |lag: usize| scores[lag + 1 - min_lag];

    let mut best: Option<(f64, usize)> = None;
    for lag in min_lag..=max_lag {
        let s = at(lag);
        if !(s > at(lag - 1) && s >= at(lag + 1)) {
            continue;
        }
        if fold(lag_to_bpm(lag as f64), range).is_none() {
            continue;
        }
        if best.is_none_or(|(b, _)| s > b) {
            best = Some((s, lag));
        }
    }
    let (s, lag) = best?;
    if s.is_nan() || s <= 0.0 {
        return None;
    }
    let (a, b, c) = (at(lag - 1), s, at(lag + 1));
    let denom = a - 2.0 * b + c;
    let delta = if denom < 0.0 {
        (0.5 * (a - c) / denom).clamp(-0.5, 0.5)
    } else {
        0.0
    };
    fold(lag_to_bpm(lag as f64 + delta), range)
}

/// Step 2 (end): the tempo within ±0.5% (0.01 BPM steps) maximizing the comb sum.
fn refine_tempo(env: &[f64], bpm: f64) -> f64 {
    let steps = (bpm * REFINE_SPAN / REFINE_STEP_BPM).round() as i64;
    let mut best = (f64::NEG_INFINITY, bpm);
    for k in -steps..=steps {
        let candidate = bpm + k as f64 * REFINE_STEP_BPM;
        let (_, s) = best_phase(env, bpm_to_lag(candidate), COARSE_PHASE_STEP);
        if s > best.0 {
            best = (s, candidate);
        }
    }
    best.1
}

/// Step 3: the offset in `[0, period)` (envelope frames) maximizing the comb sum.
fn best_phase(env: &[f64], period: f64, step: f64) -> (f64, f64) {
    let mut best = (0.0, f64::NEG_INFINITY);
    let n = (period / step).ceil() as usize;
    for k in 0..n {
        let phase = k as f64 * step;
        let s = comb(env, period, phase);
        if s > best.1 {
            best = (phase, s);
        }
    }
    best
}

/// Mean envelope value at `phase + n * period` over the track.
fn comb(env: &[f64], period: f64, phase: f64) -> f64 {
    let last = (env.len() - 1) as f64;
    let mut sum = 0.0;
    let mut count = 0usize;
    let mut t = phase;
    while t <= last {
        sum += interp(env, t);
        count += 1;
        t = phase + count as f64 * period;
    }
    if count == 0 { 0.0 } else { sum / count as f64 }
}
