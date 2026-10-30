//! Helpers shared by the engine integration tests.

#![allow(dead_code)] // each test binary uses a different subset of the helpers

use std::sync::Arc;

use engine::{EngineRenderer, TrackBuffer};

/// Frames after which the always-on EQ isolator's response to a step (a deck starting or
/// a gain change) has settled well below 1e-4: its slowest poles, the 250 Hz LR4 crossover,
/// decay by about 0.025 nepers per frame at 44.1 kHz.
pub const SETTLE_FRAMES: usize = 4096;

/// Settled tolerance for DC levels measured at the output. Far tighter than any gain step
/// the tests distinguish (the smallest is 0.64 vs 0.8 of a level).
pub const SETTLED_EPSILON: f32 = 1e-4;

/// Renders `frames` frames in `max_block_frames` chunks and returns (master, headphones).
pub fn render(r: &mut EngineRenderer, frames: usize) -> (Vec<f32>, Vec<f32>) {
    let mut master = vec![0.0; frames * 2];
    let mut headphones = vec![0.0; frames * 2];
    let block = r.max_block_frames() * 2;
    for (m, h) in master.chunks_mut(block).zip(headphones.chunks_mut(block)) {
        r.render(m, h);
    }
    (master, headphones)
}

/// A stereo track whose left and right channels both ramp as `i / frames`.
pub fn ramp_track(sample_rate: u32, frames: usize) -> Arc<TrackBuffer> {
    let samples = (0..frames)
        .flat_map(|i| {
            let v = i as f32 / frames as f32;
            [v, v]
        })
        .collect();
    Arc::new(TrackBuffer::new(sample_rate, samples).unwrap())
}

/// A stereo track holding `value` on both channels.
pub fn constant_track(sample_rate: u32, frames: usize, value: f32) -> Arc<TrackBuffer> {
    Arc::new(TrackBuffer::new(sample_rate, vec![value; frames * 2]).unwrap())
}
