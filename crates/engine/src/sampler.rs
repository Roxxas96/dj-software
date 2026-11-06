//! Sampler: eight one-shot slots mixed into the master bus.
//!
//! Each playing slot reads its buffer at `sample_rate / output_rate` frames per output
//! frame with the deck's linear interpolation and stops at the buffer's end. The summed
//! output is scaled by the sampler volume and handed to the mixer, which adds it to the
//! master sum after the channels, so the headphone master mix hears it too.

use std::sync::Arc;

use crate::TrackBuffer;
use crate::command::SamplerCommand;
use crate::dsp::smooth::Ramp;
use crate::mixer::level_gain;

/// Number of sampler slots.
pub const SAMPLER_SLOTS: usize = 8;

/// Which sampler slots are playing, as of the last rendered block (for display).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SamplerSnapshot {
    /// Per slot, whether a sample is sounding.
    pub playing: [bool; SAMPLER_SLOTS],
}

#[derive(Debug, Default)]
struct Slot {
    buffer: Option<Arc<TrackBuffer>>,
    /// Read position in the buffer's frames; only meaningful while `playing`.
    position: f64,
    playing: bool,
}

/// Sampler state and its preallocated mix buffer. Owned by the renderer.
#[derive(Debug)]
pub(crate) struct Sampler {
    slots: [Slot; SAMPLER_SLOTS],
    volume: Ramp,
    /// Interleaved stereo mix of the playing slots, preallocated to the largest block.
    mix: Box<[f32]>,
}

impl Sampler {
    pub(crate) fn new(max_block_frames: usize) -> Self {
        Self {
            slots: Default::default(),
            volume: Ramp::new(level_gain(1.0)),
            mix: vec![0.0; max_block_frames * 2].into_boxed_slice(),
        }
    }

    /// Applies `cmd` and returns a replaced buffer for the renderer to discard. An
    /// out-of-range slot and a non-finite volume are ignored.
    pub(crate) fn apply(&mut self, cmd: SamplerCommand) -> Option<Arc<TrackBuffer>> {
        match cmd {
            SamplerCommand::Set(index, buffer) => {
                let slot = self.slots.get_mut(index)?;
                slot.playing = false;
                slot.position = 0.0;
                std::mem::replace(&mut slot.buffer, buffer)
            }
            SamplerCommand::Play(index) => {
                if let Some(slot) = self.slots.get_mut(index) {
                    slot.playing = slot.buffer.as_ref().is_some_and(|b| b.frames() > 0);
                    slot.position = 0.0;
                }
                None
            }
            SamplerCommand::Stop(index) => {
                if let Some(slot) = self.slots.get_mut(index) {
                    slot.playing = false;
                }
                None
            }
            SamplerCommand::Volume(v) => {
                if v.is_finite() {
                    self.volume.target = level_gain(v);
                }
                None
            }
        }
    }

    pub(crate) fn snapshot(&self) -> SamplerSnapshot {
        SamplerSnapshot {
            playing: std::array::from_fn(|i| self.slots[i].playing),
        }
    }

    /// Renders the first `frames` frames of the playing slots, times the volume, into the
    /// mix buffer and returns them as interleaved stereo.
    pub(crate) fn render(&mut self, frames: usize, output_sample_rate: u32) -> &[f32] {
        let out = &mut self.mix[..frames * 2];
        out.fill(0.0);
        let frame_pairs = out.as_chunks_mut::<2>().0;
        for slot in &mut self.slots {
            let (true, Some(buffer)) = (slot.playing, slot.buffer.as_deref()) else {
                continue;
            };
            let end = buffer.frames() as f64;
            let step = f64::from(buffer.sample_rate()) / f64::from(output_sample_rate);
            for frame in frame_pairs.iter_mut() {
                if slot.position >= end {
                    slot.playing = false;
                    break;
                }
                add_frame(frame, buffer, slot.position);
                slot.position += step;
            }
            if slot.position >= end {
                slot.playing = false;
            }
        }
        let volume = self.volume.block(frames);
        for (i, frame) in frame_pairs.iter_mut().enumerate() {
            let gain = volume.at(i);
            frame[0] *= gain;
            frame[1] *= gain;
        }
        out
    }
}

/// Adds the buffer's sound at `position` (in `[0, frames)`) to `frame`, interpolating
/// linearly between the frames around it, like a deck does.
fn add_frame(frame: &mut [f32; 2], buffer: &TrackBuffer, position: f64) {
    let last = buffer.frames() - 1;
    let index = (position as usize).min(last);
    let frac = (position - index as f64) as f32;
    let a = buffer.frame(index);
    let b = buffer.frame((index + 1).min(last));
    frame[0] += a[0] + (b[0] - a[0]) * frac;
    frame[1] += a[1] + (b[1] - a[1]) * frac;
}
