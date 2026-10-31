//! Musical key detection: a track-averaged, log-magnitude chromagram correlated with
//! Temperley's key profiles (the Krumhansl–Schmuckler method with Temperley's revised
//! profiles). Written from first principles for this project.

use rustfft::FftPlanner;
use rustfft::num_complex::Complex;

use engine::TrackBuffer;

use crate::notation::Key;

/// Rate the mono mix is resampled to before the STFT.
const ANALYSIS_SR: f64 = 11_025.0;
/// STFT window and hop in samples at [`ANALYSIS_SR`] (2.7 Hz bins, about 5 frames/s).
const WINDOW: usize = 4096;
const HOP: usize = 2048;
/// Sample rates outside this range return `None`, keeping the work bounded (the same
/// guard as tempo detection).
const MIN_SR: u32 = 8_000;
const MAX_SR: u32 = 384_000;
/// The chromagram covers the 60 semitones from A1 (MIDI 33, 55 Hz) to G♯6 (MIDI 92):
/// five whole octaves from 55 Hz up to 1760 Hz, so every pitch class gets the same
/// number of bands. Each FFT bin is assigned to its nearest semitone.
const LOWEST_NOTE: i32 = 33;
const NOTES: i32 = 60;
/// Per-semitone log compression `ln(1 + GAMMA * a)`, with the band amplitude `a`
/// normalized so a full-scale sine is about 1. Compression keeps loud bass notes from
/// drowning out the rest of the harmony.
const GAMMA: f64 = 100.0;
/// Frames whose RMS is below this level are left out of the average.
const GATE_DBFS: f64 = -60.0;

/// Temperley's key profiles, indexed by semitones above the tonic: D. Temperley, "What's
/// Key for Key? The Krumhansl-Schmuckler Key-Finding Algorithm Reconsidered", Music
/// Perception 17(1), 1999, pp. 65–100; also in "The Cognition of Basic Musical
/// Structures", MIT Press, 2001, ch. 7.
const TEMPERLEY_MAJOR: [f64; 12] = [5.0, 2.0, 3.5, 2.0, 4.5, 4.0, 2.0, 4.5, 2.0, 3.5, 1.5, 4.0];
const TEMPERLEY_MINOR: [f64; 12] = [5.0, 2.0, 3.5, 4.5, 2.0, 4.0, 2.0, 4.5, 3.5, 2.0, 1.5, 4.0];

/// Detects the track's key. Returns `None` for silence (every frame below -60 dBFS),
/// sample rates outside 8–384 kHz, or a chromagram with no pitch-class contrast.
pub fn detect_key(track: &TrackBuffer) -> Option<Key> {
    let chroma = mean_chroma(track)?;
    best_key(&key_correlations(&chroma)?)
}

/// The key with the highest correlation.
fn best_key(scores: &[f64; 24]) -> Option<Key> {
    let (best, _) = scores
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))?;
    Key::new(best as u8)
}

/// Pearson correlation of `chroma` with each key's profile, by key index.
fn key_correlations(chroma: &[f64; 12]) -> Option<[f64; 24]> {
    let mut scores = [0.0; 24];
    for root in 0..12 {
        let rotated = |profile: &[f64; 12]| -> [f64; 12] {
            std::array::from_fn(|pc| profile[(pc + 12 - root) % 12])
        };
        scores[usize::from(Key::major(root).index())] =
            pearson(chroma, &rotated(&TEMPERLEY_MAJOR))?;
        scores[usize::from(Key::minor(root).index())] =
            pearson(chroma, &rotated(&TEMPERLEY_MINOR))?;
    }
    Some(scores)
}

/// `None` when either side is constant.
fn pearson(a: &[f64; 12], b: &[f64; 12]) -> Option<f64> {
    let mean = |x: &[f64; 12]| x.iter().sum::<f64>() / 12.0;
    let (ma, mb) = (mean(a), mean(b));
    let (mut cov, mut va, mut vb) = (0.0, 0.0, 0.0);
    for (x, y) in a.iter().zip(b) {
        let (dx, dy) = (x - ma, y - mb);
        cov += dx * dy;
        va += dx * dx;
        vb += dy * dy;
    }
    let denom = (va * vb).sqrt();
    (denom > 1e-12 && denom.is_finite()).then(|| cov / denom)
}

/// Mono mix resampled to [`ANALYSIS_SR`], streamed through one STFT window; returns the
/// mean chroma of the frames above the gate, or `None` if there are none.
fn mean_chroma(track: &TrackBuffer) -> Option<[f64; 12]> {
    let sr = track.sample_rate();
    if !(MIN_SR..=MAX_SR).contains(&sr) || track.frames() == 0 {
        return None;
    }
    let mut stft = ChromaStft::new();
    let mut window: Vec<f32> = Vec::with_capacity(WINDOW + HOP);
    let mut pending = 0usize; // samples not yet covered by a processed frame
    for s in Resampler::new(track) {
        window.push(s);
        pending += 1;
        if window.len() == WINDOW {
            stft.process(&window);
            window.drain(..HOP);
            pending = 0;
        }
    }
    // A final partial window (or a track shorter than one window), padded with silence.
    if pending > 0 {
        window.resize(WINDOW, 0.0);
        stft.process(&window);
    }
    stft.mean()
}

/// Mono mix of a track resampled to [`ANALYSIS_SR`]. Downsampling averages the input over
/// each output sample's span (a box anti-aliasing filter, which nulls the frequencies that
/// would fold onto DC and attenuates those that would fold into the chroma range);
/// upsampling interpolates linearly.
struct Resampler<'a> {
    track: &'a TrackBuffer,
    step: f64,
    next: usize,
    len: usize,
}

impl<'a> Resampler<'a> {
    fn new(track: &'a TrackBuffer) -> Self {
        let step = f64::from(track.sample_rate()) / ANALYSIS_SR;
        let len = ((track.frames() - 1) as f64 / step).floor() as usize + 1;
        Self {
            track,
            step,
            next: 0,
            len,
        }
    }

    fn mono(&self, i: usize) -> f64 {
        let [l, r] = self.track.frame(i);
        0.5 * (f64::from(l) + f64::from(r))
    }
}

impl Iterator for Resampler<'_> {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        if self.next >= self.len {
            return None;
        }
        let j = self.next as f64;
        self.next += 1;
        let frames = self.track.frames();
        if self.step > 1.0 {
            let lo = ((j - 0.5) * self.step).ceil().max(0.0) as usize;
            let hi = (((j + 0.5) * self.step).ceil() as usize).clamp(lo + 1, frames);
            let sum: f64 = (lo..hi).map(|i| self.mono(i)).sum();
            Some((sum / (hi - lo) as f64) as f32)
        } else {
            let x = j * self.step;
            let i = (x.floor() as usize).min(frames - 1);
            let f = x - i as f64;
            let a = self.mono(i);
            let b = if i + 1 < frames { self.mono(i + 1) } else { a };
            Some((a + f * (b - a)) as f32)
        }
    }
}

/// Per-frame chroma accumulator.
struct ChromaStft {
    fft: std::sync::Arc<dyn rustfft::Fft<f32>>,
    scratch: Vec<Complex<f32>>,
    spectrum: Vec<Complex<f32>>,
    hann: Vec<f32>,
    /// Semitone band (0..NOTES) of each FFT bin, `None` outside the chroma range.
    band_of_bin: Vec<Option<usize>>,
    sum: [f64; 12],
    frames: usize,
}

impl ChromaStft {
    fn new() -> Self {
        let fft = FftPlanner::<f32>::new().plan_fft_forward(WINDOW);
        let scratch = vec![Complex::default(); fft.get_inplace_scratch_len()];
        let hann = (0..WINDOW)
            .map(|i| {
                let x = std::f64::consts::TAU * i as f64 / WINDOW as f64;
                (0.5 - 0.5 * x.cos()) as f32
            })
            .collect();
        let band_of_bin = (0..=WINDOW / 2)
            .map(|k| {
                let hz = k as f64 * ANALYSIS_SR / WINDOW as f64;
                if hz <= 0.0 {
                    return None;
                }
                let note = (69.0 + 12.0 * (hz / 440.0).log2()).round() as i32 - LOWEST_NOTE;
                (0..NOTES).contains(&note).then_some(note as usize)
            })
            .collect();
        Self {
            fft,
            scratch,
            spectrum: vec![Complex::default(); WINDOW],
            hann,
            band_of_bin,
            sum: [0.0; 12],
            frames: 0,
        }
    }

    fn process(&mut self, window: &[f32]) {
        let energy: f64 = window.iter().map(|&s| f64::from(s) * f64::from(s)).sum();
        let rms = (energy / window.len() as f64).sqrt();
        if rms <= 0.0 || 20.0 * rms.log10() < GATE_DBFS {
            return;
        }
        for ((c, &w), &h) in self.spectrum.iter_mut().zip(window).zip(&self.hann) {
            *c = Complex::new(w * h, 0.0);
        }
        self.fft
            .process_with_scratch(&mut self.spectrum, &mut self.scratch);

        let mut band_energy = [0.0f64; NOTES as usize];
        for (c, band) in self.spectrum.iter().zip(&self.band_of_bin) {
            if let Some(b) = *band {
                band_energy[b] += f64::from(c.norm_sqr());
            }
        }
        // A full-scale sine peaks at WINDOW / 4 in its bin (the Hann window sums to
        // WINDOW / 2 and a real sine splits its energy between two bins).
        let norm = 4.0 / WINDOW as f64;
        for (b, e) in band_energy.iter().enumerate() {
            let pc = (b + LOWEST_NOTE as usize) % 12;
            self.sum[pc] += (GAMMA * norm * e.sqrt()).ln_1p();
        }
        self.frames += 1;
    }

    fn mean(&self) -> Option<[f64; 12]> {
        (self.frames > 0).then(|| self.sum.map(|s| s / self.frames as f64))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profiles_rotate_to_their_root() {
        // A chroma equal to a rotated profile correlates perfectly with that key.
        for root in 0..12 {
            let major: [f64; 12] = std::array::from_fn(|pc| TEMPERLEY_MAJOR[(pc + 12 - root) % 12]);
            let minor: [f64; 12] = std::array::from_fn(|pc| TEMPERLEY_MINOR[(pc + 12 - root) % 12]);
            let scores = key_correlations(&major).unwrap();
            assert_eq!(best_key(&scores), Some(Key::major(root)));
            assert!((scores[root] - 1.0).abs() < 1e-12);
            assert_eq!(
                best_key(&key_correlations(&minor).unwrap()),
                Some(Key::minor(root))
            );
        }
    }

    #[test]
    fn flat_chroma_has_no_key() {
        assert_eq!(key_correlations(&[1.0; 12]), None);
    }

    #[test]
    fn bands_cover_five_octaves_evenly() {
        let stft = ChromaStft::new();
        let mut per_band = [0usize; NOTES as usize];
        for b in stft.band_of_bin.iter().flatten() {
            per_band[*b] += 1;
        }
        assert!(per_band.iter().all(|&n| n > 0), "{per_band:?}");
    }
}
