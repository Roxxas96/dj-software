//! Chunking, overlap and the cross-fade around a network that separates one 7.8 s chunk,
//! as demucs' `apply_model` does it (docs/stems-backend.md, "Chunking, overlap and model
//! I/O"), with incremental finalization so a track's f32 output is never held whole.

use std::sync::atomic::{AtomicBool, Ordering};

use engine::{StemSet, TrackBuffer};

use crate::dsp::SEGMENT;
use crate::resample::resample;
use crate::{SeparateError, check_length, to_i16};

/// The model's sample rate.
pub(crate) const MODEL_RATE: u32 = 44_100;
/// Overlap 0.25: chunk `k` starts at `k × STRIDE`.
pub(crate) const STRIDE: usize = 257_985;
/// The cross-fade's half length: the weight rises over it, then falls.
const HALF: usize = SEGMENT / 2;
/// The stems kept from the network, in [`StemSet`] order: vocals, bass, rhythm.
pub(crate) const KEPT: usize = 3;

/// One chunk's separation.
pub(crate) trait Network {
    /// `mix` is one normalized chunk, planar `[2][SEGMENT]`; `out` receives the kept stems,
    /// planar `[KEPT][2][SEGMENT]` (vocals, bass, rhythm), in the same normalized scale.
    fn run(&mut self, mix: &[f32], out: &mut [f32]) -> Result<(), SeparateError>;
}

/// What a run measured, for tests.
#[derive(Debug, Default)]
pub(crate) struct Stats {
    /// The most chunk outputs held at once.
    pub(crate) max_pending: usize,
}

/// The cross-fade weight of sample `j` of a chunk.
fn weight(j: usize) -> f32 {
    if j < HALF {
        (j + 1) as f32 / HALF as f32
    } else {
        (SEGMENT - j) as f32 / HALF as f32
    }
}

/// The number of chunks for `frames` model-rate frames.
fn chunk_count(frames: usize) -> usize {
    frames.div_ceil(STRIDE)
}

/// Chunks in the order `p, p+1, p−1, p+2, p−2, …`, within `0..n`.
pub(crate) fn chunk_order(n: usize, p: usize) -> impl Iterator<Item = usize> {
    let p = p.min(n.saturating_sub(1));
    (0..2 * n).filter_map(move |i| {
        let step = i.div_ceil(2);
        if i % 2 == 1 {
            Some(p + step).filter(|&k| k < n)
        } else {
            p.checked_sub(step)
        }
    })
}

/// The mono mix's mean and unbiased standard deviation (1 for silence).
pub(crate) fn mono_mean_std(planar: &[Vec<f32>; 2]) -> (f32, f32) {
    let n = planar[0].len();
    if n < 2 {
        return (0.0, 1.0);
    }
    let mono = |i: usize| (f64::from(planar[0][i]) + f64::from(planar[1][i])) * 0.5;
    let mean = (0..n).map(mono).sum::<f64>() / n as f64;
    let variance = (0..n).map(|i| (mono(i) - mean).powi(2)).sum::<f64>() / (n - 1) as f64;
    let std = variance.sqrt();
    (mean as f32, if std > 1e-8 { std as f32 } else { 1.0 })
}

/// The overlap-add state: pending chunk outputs and the final 16-bit stems at 44.1 kHz.
struct Finalizer {
    frames: usize,
    mean: f32,
    std: f32,
    done: Vec<bool>,
    final_region: Vec<bool>,
    /// Chunk `k`'s trimmed output, planar `[KEPT][2][len(k)]`, until regions `k` and `k+1`
    /// are final.
    pending: Vec<Option<Box<[f32]>>>,
    /// Interleaved stereo, vocals, bass, rhythm.
    out: [Vec<i16>; KEPT],
    stats: Stats,
}

impl Finalizer {
    fn new(frames: usize, mean: f32, std: f32) -> Self {
        let n = chunk_count(frames);
        Self {
            frames,
            mean,
            std,
            done: vec![false; n],
            final_region: vec![false; n],
            pending: (0..n).map(|_| None).collect(),
            out: [(); KEPT].map(|()| vec![0; 2 * frames]),
            stats: Stats::default(),
        }
    }

    fn chunks(&self) -> usize {
        self.done.len()
    }

    /// Chunk `k`'s length in the track (the last chunks may be short).
    fn len(&self, k: usize) -> usize {
        SEGMENT.min(self.frames - k * STRIDE)
    }

    /// Takes chunk `k`'s full network output (`[KEPT][2][SEGMENT]`, centered on the chunk
    /// when it is short) and finalizes the regions it completes.
    fn add(&mut self, k: usize, output: &[f32]) {
        let len = self.len(k);
        let trim = (SEGMENT - len) / 2;
        let mut kept = vec![0.0; KEPT * 2 * len].into_boxed_slice();
        for plane in 0..KEPT * 2 {
            kept[plane * len..(plane + 1) * len]
                .copy_from_slice(&output[plane * SEGMENT + trim..][..len]);
        }
        self.pending[k] = Some(kept);
        self.done[k] = true;
        let held = self.pending.iter().filter(|p| p.is_some()).count();
        self.stats.max_pending = self.stats.max_pending.max(held);

        let n = self.chunks();
        for region in [k, k + 1] {
            if region < n
                && !self.final_region[region]
                && self.done[region]
                && (region == 0 || self.done[region - 1])
            {
                self.finalize(region);
            }
        }
        // Chunk `j` covers regions `j` and `j + 1` only.
        for j in k.saturating_sub(1)..=(k + 1).min(n - 1) {
            if self.final_region[j] && (j + 1 >= n || self.final_region[j + 1]) {
                self.pending[j] = None;
            }
        }
    }

    /// Region `r`, `[r × STRIDE, (r + 1) × STRIDE)`, is covered by chunk `r` and by the
    /// tail of chunk `r − 1`.
    fn finalize(&mut self, r: usize) {
        let frames = self.frames;
        let len = |k: usize| SEGMENT.min(frames - k * STRIDE);
        let (mean, std) = (self.mean, self.std);
        let start = r * STRIDE;
        let end = ((r + 1) * STRIDE).min(frames);
        let own = self.pending[r].as_deref().expect("chunk r is done");
        let own_len = len(r);
        let previous = r.checked_sub(1).map(|p| {
            let data = self.pending[p].as_deref().expect("chunk r-1 is done");
            (p, data, len(p))
        });
        for i in start..end {
            let j = i - start;
            let w = weight(j);
            let (pw, pj, plen, pdata) = match previous {
                Some((p, data, len)) if i < p * STRIDE + len => {
                    let pj = i - p * STRIDE;
                    (weight(pj), pj, len, data)
                }
                _ => (0.0, 0, 1, own),
            };
            for (stem, out) in self.out.iter_mut().enumerate() {
                for channel in 0..2 {
                    let plane = stem * 2 + channel;
                    let mut value = w * own[plane * own_len + j];
                    if pw > 0.0 {
                        value += pw * pdata[plane * plen + pj];
                    }
                    out[2 * i + channel] = to_i16(value / (w + pw) * std + mean);
                }
            }
        }
        self.final_region[r] = true;
    }
}

/// Separates `track` with `network`, chunk by chunk, starting with the chunk around
/// `start_frame` (a frame of the track) and working outward.
pub(crate) fn separate(
    network: &mut dyn Network,
    track: &TrackBuffer,
    start_frame: usize,
    progress: &dyn Fn(f32),
    cancel: &AtomicBool,
) -> Result<(StemSet, Stats), SeparateError> {
    let frames = track.frames();
    let rate = track.sample_rate();
    check_length(frames, rate)?;
    if cancel.load(Ordering::Relaxed) {
        return Err(SeparateError::Cancelled);
    }
    if frames == 0 {
        let empty = || Box::<[i16]>::default();
        return Ok((
            StemSet::new(empty(), empty(), empty(), 0, rate)?,
            Stats::default(),
        ));
    }

    // The mix at the model's rate, planar, then normalized.
    let model_frames = if rate == MODEL_RATE {
        frames
    } else {
        ((frames as u64 * u64::from(MODEL_RATE) + u64::from(rate) / 2) / u64::from(rate)) as usize
    }
    .max(1);
    let mut x = [vec![0.0f32; model_frames], vec![0.0f32; model_frames]];
    if rate == MODEL_RATE {
        let [left, right] = &mut x;
        for (i, (l, r)) in left.iter_mut().zip(right.iter_mut()).enumerate() {
            [*l, *r] = track.frame(i);
        }
    } else {
        let [left_out, right_out] = &mut x;
        resample(
            rate,
            MODEL_RATE,
            model_frames,
            &mut |start, left, right| {
                for (i, (l, r)) in left.iter_mut().zip(right.iter_mut()).enumerate() {
                    [*l, *r] = if start + i < frames {
                        track.frame(start + i)
                    } else {
                        [0.0; 2]
                    };
                }
            },
            &mut |start, left, right| {
                left_out[start..start + left.len()].copy_from_slice(left);
                right_out[start..start + right.len()].copy_from_slice(right);
            },
            cancel,
        )?;
    }
    let (mean, std) = mono_mean_std(&x);
    for channel in &mut x {
        for v in channel.iter_mut() {
            *v = (*v - mean) / std;
        }
    }

    let mut finalizer = Finalizer::new(model_frames, mean, std);
    let n = finalizer.chunks();
    let start_model = (start_frame as u64 * u64::from(MODEL_RATE) / u64::from(rate)) as usize;
    let mut chunk = vec![0.0f32; 2 * SEGMENT];
    let mut output = vec![0.0f32; KEPT * 2 * SEGMENT];
    for (done, k) in chunk_order(n, start_model / STRIDE).enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return Err(SeparateError::Cancelled);
        }
        // A short chunk is cut from a full window centered on it, zeros past the ends.
        let offset = k * STRIDE;
        let len = finalizer.len(k);
        let first = offset as isize - ((SEGMENT - len) / 2) as isize;
        for channel in 0..2 {
            for (i, sample) in chunk[channel * SEGMENT..(channel + 1) * SEGMENT]
                .iter_mut()
                .enumerate()
            {
                let at = first + i as isize;
                *sample = if (0..model_frames as isize).contains(&at) {
                    x[channel][at as usize]
                } else {
                    0.0
                };
            }
        }
        output.fill(0.0);
        network.run(&chunk, &mut output)?;
        finalizer.add(k, &output);
        progress((done + 1) as f32 / n as f32);
    }
    drop(x);
    let Finalizer { out, stats, .. } = finalizer;

    let [vocals, bass, rhythm] = if rate == MODEL_RATE {
        out.map(Vec::into_boxed_slice)
    } else {
        // One stem at a time, each 44.1 kHz stem freed once resampled.
        let mut back = Vec::with_capacity(KEPT);
        for stem in out {
            let mut native = vec![0i16; 2 * frames];
            resample(
                MODEL_RATE,
                rate,
                frames,
                &mut |start, left, right| {
                    for (i, (l, r)) in left.iter_mut().zip(right.iter_mut()).enumerate() {
                        let at = start + i;
                        [*l, *r] = if at < model_frames {
                            [stem[2 * at], stem[2 * at + 1]].map(|v| f32::from(v) / 32_768.0)
                        } else {
                            [0.0; 2]
                        };
                    }
                },
                &mut |start, left, right| {
                    for (i, (&l, &r)) in left.iter().zip(right).enumerate() {
                        native[2 * (start + i)] = to_i16(l);
                        native[2 * (start + i) + 1] = to_i16(r);
                    }
                },
                cancel,
            )?;
            drop(stem);
            back.push(native.into_boxed_slice());
        }
        let [vocals, bass, rhythm]: [Box<[i16]>; KEPT] =
            back.try_into().expect("one per kept stem");
        [vocals, bass, rhythm]
    };
    Ok((StemSet::new(vocals, bass, rhythm, frames, rate)?, stats))
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use engine::Stem;

    use super::*;

    /// Copies the normalized mix to vocals; bass and rhythm stay zero.
    struct Identity;

    impl Network for Identity {
        fn run(&mut self, mix: &[f32], out: &mut [f32]) -> Result<(), SeparateError> {
            out[..2 * SEGMENT].copy_from_slice(mix);
            Ok(())
        }
    }

    /// A stereo track of two slow sines, `frames` long.
    fn track(frames: usize, rate: u32) -> TrackBuffer {
        let tau = std::f32::consts::TAU;
        let samples = (0..frames)
            .flat_map(|i| {
                let t = i as f32 / rate as f32;
                [0.3 * (tau * 110.0 * t).sin(), 0.2 * (tau * 330.0 * t).cos()]
            })
            .collect();
        TrackBuffer::new(rate, samples).unwrap()
    }

    fn as_f32(samples: &[i16]) -> impl Iterator<Item = f32> + '_ {
        samples.iter().map(|&v| f32::from(v) / 32_768.0)
    }

    #[test]
    fn chunks_run_outward_from_the_start() {
        assert_eq!(chunk_order(5, 2).collect::<Vec<_>>(), [2, 3, 1, 4, 0]);
        assert_eq!(chunk_order(5, 0).collect::<Vec<_>>(), [0, 1, 2, 3, 4]);
        assert_eq!(chunk_order(5, 4).collect::<Vec<_>>(), [4, 3, 2, 1, 0]);
        assert_eq!(chunk_order(5, 9).collect::<Vec<_>>(), [4, 3, 2, 1, 0]);
        assert_eq!(chunk_order(1, 0).collect::<Vec<_>>(), [0]);
        assert_eq!(chunk_order(0, 0).count(), 0);
    }

    #[test]
    fn an_identity_network_gives_back_the_mix() {
        // Four chunks, the last two short; started in the middle.
        let frames = 3 * STRIDE + 12_345;
        let track = track(frames, MODEL_RATE);
        let (set, stats) = separate(
            &mut Identity,
            &track,
            2 * STRIDE,
            &|_| {},
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(set.frames(), frames);
        let vocals: Vec<f32> = as_f32(set.stored(Stem::Vocals).unwrap()).collect();
        for i in 0..frames {
            let mix = track.frame(i);
            for ch in 0..2 {
                assert!(
                    (vocals[2 * i + ch] - mix[ch]).abs() < 2.0 / 32_768.0,
                    "frame {i}"
                );
            }
        }
        // Bass and rhythm are the mean only (zero output, de-normalized).
        assert!(as_f32(set.stored(Stem::Bass).unwrap()).all(|v| v.abs() < 1e-3));
        assert!(stats.max_pending <= 3, "{stats:?}");
    }

    #[test]
    fn other_rates_go_through_the_model_rate_and_back() {
        let frames = 2 * 48_000 * 10;
        let track = track(frames, 48_000);
        let (set, _) =
            separate(&mut Identity, &track, 0, &|_| {}, &AtomicBool::new(false)).unwrap();
        assert_eq!(set.frames(), frames);
        let vocals: Vec<f32> = as_f32(set.stored(Stem::Vocals).unwrap()).collect();
        for i in 4_000..frames - 4_000 {
            let mix = track.frame(i);
            for ch in 0..2 {
                assert!((vocals[2 * i + ch] - mix[ch]).abs() < 2e-3, "frame {i}");
            }
        }
    }

    /// Chunk `k` outputs the constant `k + 1`: the overlap is the weighted blend.
    #[test]
    fn overlaps_cross_fade_with_the_triangular_weight() {
        struct Constant(Cell<f32>);
        impl Network for Constant {
            fn run(&mut self, _: &[f32], out: &mut [f32]) -> Result<(), SeparateError> {
                self.0.set(self.0.get() + 1.0);
                out[..2 * SEGMENT].fill(self.0.get());
                Ok(())
            }
        }
        // Two chunks: the second is short, and only it covers the last frames.
        let frames = 2 * STRIDE;
        let track = track(frames, MODEL_RATE);
        let (set, _) = separate(
            &mut Constant(Cell::new(0.0)),
            &track,
            0,
            &|_| {},
            &AtomicBool::new(false),
        )
        .unwrap();
        let planar = [0, 1].map(|ch| (0..frames).map(|i| track.frame(i)[ch]).collect());
        let (mean, std) = mono_mean_std(&planar);
        let vocals = set.stored(Stem::Vocals).unwrap();
        for j in [0, 1_000, 40_000, SEGMENT - STRIDE - 1] {
            let (previous, own) = (weight(STRIDE + j), weight(j));
            let blend = (previous + 2.0 * own) / (previous + own);
            let expected = to_i16(blend * std + mean);
            assert_eq!(vocals[2 * (STRIDE + j)], expected, "overlap sample {j}");
        }
        assert_eq!(vocals[0], to_i16(std + mean));
        assert_eq!(vocals[2 * (frames - 1)], to_i16(2.0 * std + mean));
    }

    #[test]
    fn cancel_is_checked_before_each_chunk() {
        struct Cancelling<'a>(&'a AtomicBool, usize);
        impl Network for Cancelling<'_> {
            fn run(&mut self, _: &[f32], _: &mut [f32]) -> Result<(), SeparateError> {
                self.1 += 1;
                self.0.store(true, Ordering::Relaxed);
                Ok(())
            }
        }
        let cancel = AtomicBool::new(false);
        let mut network = Cancelling(&cancel, 0);
        let track = track(3 * STRIDE, MODEL_RATE);
        let result = separate(&mut network, &track, 0, &|_| {}, &cancel);
        assert!(matches!(result, Err(SeparateError::Cancelled)));
        assert_eq!(network.1, 1);
    }

    #[test]
    fn an_empty_track_gives_empty_stems() {
        let track = TrackBuffer::new(MODEL_RATE, Vec::new()).unwrap();
        let (set, _) =
            separate(&mut Identity, &track, 0, &|_| {}, &AtomicBool::new(false)).unwrap();
        assert_eq!(set.frames(), 0);
    }
}
