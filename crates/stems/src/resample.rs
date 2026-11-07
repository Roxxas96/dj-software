//! Band-limited resampling between a track's rate and the model's 44.1 kHz, streamed in
//! small blocks so a whole track never needs a second f32 copy.

use std::sync::atomic::{AtomicBool, Ordering};

use rubato::audioadapter_buffers::direct::SequentialSliceOfVecs;
use rubato::{Fft, FixedSync, Resampler};

use crate::SeparateError;

/// Fills both channels with the input frames from a start frame (zeros past the end).
pub(crate) type Read<'a> = dyn FnMut(usize, &mut [f32], &mut [f32]) + 'a;
/// Takes both channels' output frames from a start frame.
pub(crate) type Write<'a> = dyn FnMut(usize, &[f32], &[f32]) + 'a;

/// Input frames per block.
const BLOCK: usize = 1024;

/// Resamples stereo from `rate_in` to `rate_out`, producing exactly `frames_out` frames.
/// `read(start, left, right)` fills both slices with the input frames from `start` (zeros
/// past the input's end); `write(start, left, right)` takes output frames from `start`.
/// Checks `cancel` once per block.
pub(crate) fn resample(
    rate_in: u32,
    rate_out: u32,
    frames_out: usize,
    read: &mut Read<'_>,
    write: &mut Write<'_>,
    cancel: &AtomicBool,
) -> Result<(), SeparateError> {
    let failed = |e: &dyn std::fmt::Display| SeparateError::Model(format!("resampling: {e}"));
    let mut resampler = Fft::<f32>::new(
        rate_in as usize,
        rate_out as usize,
        BLOCK,
        2,
        FixedSync::Input,
    )
    .map_err(|e| failed(&e))?;
    let mut input = vec![vec![0.0; resampler.input_frames_max()]; 2];
    let mut output = vec![vec![0.0; resampler.output_frames_max()]; 2];
    // The resampler's own delay: its first `delay` output frames are dropped.
    let mut skip = resampler.output_delay();
    let (mut read_at, mut written) = (0, 0);
    while written < frames_out {
        if cancel.load(Ordering::Relaxed) {
            return Err(SeparateError::Cancelled);
        }
        let needed = resampler.input_frames_next();
        let [left, right] = &mut input[..] else {
            unreachable!("two channels")
        };
        read(read_at, &mut left[..needed], &mut right[..needed]);
        let capacity = output[0].len();
        let input_adapter =
            SequentialSliceOfVecs::new(&input, 2, needed).map_err(|e| failed(&e))?;
        let mut output_adapter =
            SequentialSliceOfVecs::new_mut(&mut output, 2, capacity).map_err(|e| failed(&e))?;
        let (consumed, produced) = resampler
            .process_into_buffer(&input_adapter, &mut output_adapter, None)
            .map_err(|e| failed(&e))?;
        read_at += consumed;
        let dropped = skip.min(produced);
        skip -= dropped;
        let take = (produced - dropped).min(frames_out - written);
        let range = dropped..dropped + take;
        write(written, &output[0][range.clone()], &output[1][range]);
        written += take;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(frames: usize, rate: u32) -> Vec<f32> {
        (0..frames)
            .map(|i| (std::f32::consts::TAU * 440.0 * i as f32 / rate as f32).sin() * 0.5)
            .collect()
    }

    #[test]
    fn a_sine_survives_the_round_trip() {
        let frames = 48_000;
        let x = sine(frames, 48_000);
        let reader = |x: &[f32]| {
            let x = x.to_vec();
            move |start: usize, left: &mut [f32], right: &mut [f32]| {
                for (i, (l, r)) in left.iter_mut().zip(right.iter_mut()).enumerate() {
                    let v = x.get(start + i).copied().unwrap_or(0.0);
                    (*l, *r) = (v, -v);
                }
            }
        };
        let mut down = vec![0.0; 44_100];
        resample(
            48_000,
            44_100,
            44_100,
            &mut reader(&x),
            &mut |start, left, right| {
                down[start..start + left.len()].copy_from_slice(left);
                assert_eq!(left[0], -right[0]);
            },
            &AtomicBool::new(false),
        )
        .unwrap();
        let reference = sine(44_100, 44_100);
        for i in 2_000..42_000 {
            assert!((down[i] - reference[i]).abs() < 1e-3, "frame {i}");
        }
        let mut back = vec![0.0; frames];
        resample(
            44_100,
            48_000,
            frames,
            &mut reader(&down),
            &mut |start, left, _| back[start..start + left.len()].copy_from_slice(left),
            &AtomicBool::new(false),
        )
        .unwrap();
        for i in 2_000..46_000 {
            assert!((back[i] - x[i]).abs() < 1e-3, "frame {i}");
        }
    }
}
