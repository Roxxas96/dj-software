//! A fixed linear split by frequency bands, standing in for the model in tests.

use std::sync::atomic::{AtomicBool, Ordering};

use engine::{StemSet, TrackBuffer};

use crate::{SeparateError, Separator, check_length, to_i16};

/// Frames per chunk: the cancel flag is checked and progress reported once per chunk.
const CHUNK: usize = 65_536;

/// A fixed linear split: bass is a low-pass below 150 Hz, vocals a band from 300 Hz to
/// 3 kHz and rhythm a high-pass above 6 kHz (one-pole filters). What they leave lands in
/// instruments, the residual. It stands in for the model in tests and CI.
#[derive(Debug, Default)]
pub struct SyntheticSeparator {}

impl SyntheticSeparator {
    pub fn new() -> Self {
        Self::default()
    }
}

/// A one-pole low-pass per channel.
#[derive(Clone, Copy)]
struct LowPass {
    coefficient: f32,
    state: [f32; 2],
}

impl LowPass {
    fn new(cutoff: f32, sample_rate: u32) -> Self {
        let coefficient = 1.0 - (-std::f32::consts::TAU * cutoff / sample_rate as f32).exp();
        Self {
            coefficient,
            state: [0.0; 2],
        }
    }

    fn process(&mut self, channel: usize, x: f32) -> f32 {
        let y = &mut self.state[channel];
        *y += self.coefficient * (x - *y);
        *y
    }
}

impl Separator for SyntheticSeparator {
    fn separate(
        &mut self,
        track: &TrackBuffer,
        progress: &dyn Fn(f32),
        cancel: &AtomicBool,
    ) -> Result<StemSet, SeparateError> {
        let frames = track.frames();
        let rate = track.sample_rate();
        check_length(frames, rate)?;
        let mut bass_lp = LowPass::new(150.0, rate);
        let mut vocals_hp = LowPass::new(300.0, rate);
        let mut vocals_lp = [LowPass::new(3_000.0, rate); 2];
        let mut rhythm_hp = LowPass::new(6_000.0, rate);
        let mut stems = [(); 3].map(|()| Vec::with_capacity(frames * 2));
        let chunks = frames.div_ceil(CHUNK);
        for chunk in 0..chunks {
            if cancel.load(Ordering::Relaxed) {
                return Err(SeparateError::Cancelled);
            }
            for index in chunk * CHUNK..((chunk + 1) * CHUNK).min(frames) {
                let mix = track.frame(index);
                for (channel, &x) in mix.iter().enumerate() {
                    let bass = bass_lp.process(channel, x);
                    let band = x - vocals_hp.process(channel, x);
                    let band = vocals_lp[0].process(channel, band);
                    let vocals = vocals_lp[1].process(channel, band);
                    let rhythm = x - rhythm_hp.process(channel, x);
                    for (stem, value) in stems.iter_mut().zip([vocals, bass, rhythm]) {
                        stem.push(to_i16(value));
                    }
                }
            }
            progress((chunk + 1) as f32 / chunks as f32);
        }
        let [vocals, bass, rhythm] = stems.map(Vec::into_boxed_slice);
        Ok(StemSet::new(vocals, bass, rhythm, frames)?)
    }
}
