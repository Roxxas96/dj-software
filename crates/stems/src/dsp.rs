//! The STFT and iSTFT around the htdemucs network, exactly as demucs 4.1.0's `_spec` and
//! `_ispec` compute them (see docs/stems-backend.md, "Chunking, overlap and model I/O").

use std::sync::Arc;

use realfft::num_complex::Complex32;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};

/// Samples per chunk, at 44.1 kHz: the training segment, the only length the graph takes.
pub(crate) const SEGMENT: usize = 343_980;
const N_FFT: usize = 4096;
const HOP: usize = 1024;
/// Frequency bins kept (the Nyquist bin is dropped).
pub(crate) const BINS: usize = 2048;
/// STFT frames kept: `ceil(SEGMENT / HOP)`.
pub(crate) const FRAMES: usize = 336;
/// demucs' extra reflect padding before the centered STFT: `HOP / 2 × 3`.
const PAD: usize = HOP / 2 * 3;
/// Every STFT frame of the padded signal, before 2 are dropped on each side.
const ALL_FRAMES: usize = FRAMES + 4;
/// The overlap-added length of [`ALL_FRAMES`] frames.
const OLA_LEN: usize = N_FFT + HOP * (ALL_FRAMES - 1);

/// The FFT plans, the window and scratch buffers for one chunk's STFT and iSTFT.
pub(crate) struct Stft {
    forward: Arc<dyn RealToComplex<f32>>,
    inverse: Arc<dyn ComplexToReal<f32>>,
    window: Vec<f32>,
    /// The summed squared window over every frame (the iSTFT's normalization), fixed.
    envelope: Vec<f32>,
    padded: Vec<f32>,
    frame: Vec<f32>,
    spectrum: Vec<Complex32>,
    overlap: Vec<f32>,
    scratch_forward: Vec<Complex32>,
    scratch_inverse: Vec<Complex32>,
}

/// Reflect-pads `x` by `left` and `right` samples into `out` (numpy/torch "reflect": the
/// edge sample is not repeated).
fn reflect_pad(x: &[f32], left: usize, right: usize, out: &mut Vec<f32>) {
    let n = x.len();
    out.clear();
    out.extend((0..left).map(|j| x[left - j]));
    out.extend_from_slice(x);
    out.extend((0..right).map(|k| x[n - 2 - k]));
}

impl Stft {
    pub(crate) fn new() -> Self {
        let mut planner = RealFftPlanner::<f32>::new();
        let forward = planner.plan_fft_forward(N_FFT);
        let inverse = planner.plan_fft_inverse(N_FFT);
        // Periodic Hann, as torch.hann_window.
        let window: Vec<f32> = (0..N_FFT)
            .map(|n| 0.5 - 0.5 * (std::f64::consts::TAU * n as f64 / N_FFT as f64).cos() as f32)
            .collect();
        let mut envelope = vec![0.0; OLA_LEN];
        for frame in 0..ALL_FRAMES {
            for (n, w) in window.iter().enumerate() {
                envelope[frame * HOP + n] += w * w;
            }
        }
        Self {
            scratch_forward: forward.make_scratch_vec(),
            scratch_inverse: inverse.make_scratch_vec(),
            spectrum: forward.make_output_vec(),
            frame: forward.make_input_vec(),
            forward,
            inverse,
            window,
            envelope,
            padded: Vec::new(),
            overlap: vec![0.0; OLA_LEN],
        }
    }

    /// demucs `_spec` + `_magnitude` for one channel of [`SEGMENT`] samples: writes the
    /// real and imaginary planes, `[BINS][FRAMES]` each.
    pub(crate) fn spec(&mut self, x: &[f32], re: &mut [f32], im: &mut [f32]) {
        assert_eq!(x.len(), SEGMENT);
        let mut padded = std::mem::take(&mut self.padded);
        let mut first = Vec::with_capacity(SEGMENT + 2 * PAD + HOP);
        reflect_pad(x, PAD, PAD + FRAMES * HOP - SEGMENT, &mut first);
        reflect_pad(&first, N_FFT / 2, N_FFT / 2, &mut padded);
        let norm = 1.0 / (N_FFT as f32).sqrt();
        for t in 0..FRAMES {
            let start = (t + 2) * HOP;
            for (n, sample) in self.frame.iter_mut().enumerate() {
                *sample = padded[start + n] * self.window[n];
            }
            self.forward
                .process_with_scratch(
                    &mut self.frame,
                    &mut self.spectrum,
                    &mut self.scratch_forward,
                )
                .expect("FFT buffers have the planned sizes");
            for f in 0..BINS {
                re[f * FRAMES + t] = self.spectrum[f].re * norm;
                im[f * FRAMES + t] = self.spectrum[f].im * norm;
            }
        }
        self.padded = padded;
    }

    /// demucs `_ispec` for one channel (`[BINS][FRAMES]` planes): adds the [`SEGMENT`]
    /// samples to `out`.
    pub(crate) fn ispec_add(&mut self, re: &[f32], im: &[f32], out: &mut [f32]) {
        assert_eq!(out.len(), SEGMENT);
        let scale = 1.0 / (N_FFT as f32).sqrt();
        self.overlap.fill(0.0);
        // Frames 0, 1 and the last two are zero: they add nothing but their window.
        for t in 0..FRAMES {
            for f in 0..BINS {
                self.spectrum[f] = Complex32::new(re[f * FRAMES + t], im[f * FRAMES + t]);
            }
            self.spectrum[0].im = 0.0;
            self.spectrum[BINS] = Complex32::new(0.0, 0.0);
            self.inverse
                .process_with_scratch(
                    &mut self.spectrum,
                    &mut self.frame,
                    &mut self.scratch_inverse,
                )
                .expect("FFT buffers have the planned sizes");
            let start = (t + 2) * HOP;
            for (n, sample) in self.frame.iter().enumerate() {
                self.overlap[start + n] += sample * scale * self.window[n];
            }
        }
        // The centered iSTFT drops N_FFT / 2, then demucs keeps [PAD, PAD + SEGMENT).
        let offset = N_FFT / 2 + PAD;
        for (i, sample) in out.iter_mut().enumerate() {
            let envelope = self.envelope[offset + i];
            if envelope > 1e-11 {
                *sample += self.overlap[offset + i] / envelope;
            }
        }
    }
}

/// The test vector's `mix` (docs/stems-backend.md, Verification): `[2][SEGMENT]` planar
/// from a 64-bit LCG, channel 0 first.
pub(crate) fn test_vector_mix() -> Vec<f32> {
    let mut s: u64 = 0x853c_49e6_748f_ea9b;
    (0..2 * SEGMENT)
        .map(|_| {
            s = s
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            ((((s >> 40) as f64 / 16_777_216.0) * 2.0 - 1.0) as f32) * 0.5
        })
        .collect()
}

/// The network's `mag` input for a planar `[2][SEGMENT]` `mix`: `[4][BINS][FRAMES]`,
/// L re, L im, R re, R im.
pub(crate) fn magnitude(stft: &mut Stft, mix: &[f32], mag: &mut [f32]) {
    let plane = BINS * FRAMES;
    for channel in 0..2 {
        let (re, rest) = mag[2 * channel * plane..].split_at_mut(plane);
        stft.spec(
            &mix[channel * SEGMENT..(channel + 1) * SEGMENT],
            re,
            &mut rest[..plane],
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vector_mix_matches_the_record() {
        let mix = test_vector_mix();
        assert_eq!(
            mix[..4],
            [-0.492_319_64, 0.030_946_195, -0.348_879_93, 0.336_138_67]
        );
        let sum: f64 = mix.iter().map(|&v| f64::from(v)).sum();
        assert!((sum - -102.522_62).abs() < 1e-3, "{sum}");
    }

    /// The Rust STFT against PyTorch's on the test vector: `mag` sum 1.905753, sum of
    /// squares 43022.49 (docs/stems-backend.md).
    #[test]
    fn stft_matches_the_pytorch_reference_sums() {
        let mix = test_vector_mix();
        let mut mag = vec![0.0; 4 * BINS * FRAMES];
        magnitude(&mut Stft::new(), &mix, &mut mag);
        let sum: f64 = mag.iter().map(|&v| f64::from(v)).sum();
        let squares: f64 = mag.iter().map(|&v| f64::from(v).powi(2)).sum();
        // The sum is small next to its terms (it mostly cancels), so it gets an absolute
        // bound; the sum of squares a relative one.
        assert!((sum - 1.905_753).abs() < 2e-3, "sum {sum}");
        assert!(
            (squares / 43_022.49 - 1.0).abs() < 1e-5,
            "sum of squares {squares}"
        );
    }

    #[test]
    fn istft_inverts_the_stft_away_from_the_edges() {
        // Band-limited: the dropped Nyquist bin would make white noise come back short.
        let x: Vec<f32> = (0..SEGMENT)
            .map(|i| {
                let t = i as f32 / 44_100.0;
                0.3 * (std::f32::consts::TAU * 220.0 * t).sin()
                    + 0.2 * (std::f32::consts::TAU * 5_000.0 * t).cos()
            })
            .collect();
        let x = &x[..];
        let mut stft = Stft::new();
        let (mut re, mut im) = (vec![0.0; BINS * FRAMES], vec![0.0; BINS * FRAMES]);
        stft.spec(x, &mut re, &mut im);
        let mut y = vec![0.0; SEGMENT];
        stft.ispec_add(&re, &im, &mut y);
        // The two dropped frames on each side leave the outer ~2.5k samples short.
        for i in 4096..SEGMENT - 4096 {
            assert!(
                (y[i] - x[i]).abs() < 1e-4,
                "sample {i}: {} vs {}",
                y[i],
                x[i]
            );
        }
    }
}
