//! Keylock of a deck: a tempo change keeps the track's pitch. The deck reads the source at
//! its original pitch, through the same reader as the normal path (so loops, rolls and the
//! slip ghost behave the same), and Signalsmith Stretch time-stretches it to the output.

use std::fmt;

use signalsmith_stretch::Stretch;

use super::{Deck, Source, jog, read_frame, slip};
use crate::{StemSet, TrackBuffer};

/// Tempo ratios this close to 1.0 play without the stretcher, as at the original tempo. The
/// nudge bend never counts, so a nudge never switches between the paths.
const BYPASS_EPSILON: f64 = 1e-4;
/// Highest tempo ratio, at the ±100 % range: input buffers are sized for it.
const MAX_TEMPO_RATIO: usize = 2;
const CHANNELS: usize = 2;

/// A deck's time-stretcher and its preallocated buffers. Built off the audio thread.
pub(crate) struct Stretcher {
    stretch: Stretch,
    /// Interleaved stereo source frames for one `seek` or `process` call.
    input: Box<[f32]>,
    /// Interleaved stereo stretched frames for one block.
    output: Box<[f32]>,
    input_latency: usize,
    output_latency: usize,
    /// Where the next source frame fed to the stretcher is read, in track frames. It runs
    /// ahead of the playhead by what the stretcher holds.
    feed: f64,
    /// The fraction of a source frame left over by the last block's rounding.
    carry: f64,
    /// Where the last stretched block left the deck; `None` after anything else (bypass, a
    /// switch), so the next stretched block restarts.
    resume: Option<Resume>,
}

/// The deck state a stretched block ended in. A next block starting from anything else is
/// a discontinuity.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Resume {
    position: f64,
    load_seq: u64,
    bounds: Option<(f64, f64)>,
}

impl fmt::Debug for Stretcher {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Stretcher")
            .field("input_latency", &self.input_latency)
            .field("output_latency", &self.output_latency)
            .field("feed", &self.feed)
            .field("carry", &self.carry)
            .field("resume", &self.resume)
            .finish_non_exhaustive()
    }
}

impl Stretcher {
    /// A stereo stretcher for `output_rate`, with buffers for blocks of up to
    /// `max_block_frames`. Allocates: call it off the audio thread.
    pub(crate) fn new(output_rate: u32, max_block_frames: usize) -> Self {
        let stretch = Stretch::preset_cheaper(CHANNELS as u32, output_rate);
        let input_latency = stretch.input_latency();
        let output_latency = stretch.output_latency();
        let input_frames = max_block_frames * MAX_TEMPO_RATIO + input_latency;
        Self {
            stretch,
            input: vec![0.0; input_frames * CHANNELS].into_boxed_slice(),
            output: vec![0.0; max_block_frames * CHANNELS].into_boxed_slice(),
            input_latency,
            output_latency,
            feed: 0.0,
            carry: 0.0,
            resume: None,
        }
    }

    /// From a source read to its output, in output frames, at `speed`.
    pub(super) fn latency_frames(&self, speed: f64) -> usize {
        let input = if speed > 0.0 {
            self.input_latency as f64 / speed
        } else {
            0.0
        };
        input.round() as usize + self.output_latency
    }

    fn input_capacity(&self) -> usize {
        self.input.len() / CHANNELS
    }

    fn output_capacity(&self) -> usize {
        self.output.len() / CHANNELS
    }

    /// Fills the first `frames` input frames from the feed, which wraps in `bounds` and
    /// reads silence past the track's end. Input frame `i` is the source's read `i`.
    fn fill(&mut self, source: Source<'_>, bounds: Option<(f64, f64)>, step: f64, frames: usize) {
        for (i, frame) in self.input.as_chunks_mut::<2>().0[..frames]
            .iter_mut()
            .enumerate()
        {
            *frame = read_frame(source, &mut self.feed, bounds, step, i).unwrap_or([0.0; 2]);
        }
    }

    /// Starts the stream over at `position`: drops what the stretcher holds, feeds it
    /// `input_latency` frames from `position` with `seek`, then renders (and discards) its
    /// output latency, so the next output is the track at `position` at full level.
    fn restart(
        &mut self,
        source: Source<'_>,
        position: f64,
        bounds: Option<(f64, f64)>,
        step: f64,
        speed: f64,
    ) {
        self.stretch.reset();
        self.feed = position;
        self.carry = 0.0;
        let preroll = self.input_latency;
        self.fill(source, bounds, step, preroll);
        self.stretch.seek(&self.input[..preroll * CHANNELS], speed);
        let mut left = self.output_latency;
        while left > 0 {
            let frames = left.min(self.output_capacity());
            let read = self.source_frames(frames, speed);
            self.fill(source, bounds, step, read);
            self.stretch.process(
                &self.input[..read * CHANNELS],
                &mut self.output[..frames * CHANNELS],
            );
            left -= frames;
        }
    }

    /// The source frames to read for `frames` output frames at `speed`: rounded, with the
    /// remainder carried to the next block so no drift builds up, and at most what the
    /// input buffer holds.
    fn source_frames(&mut self, frames: usize, speed: f64) -> usize {
        let exact = frames as f64 * speed + self.carry;
        let rounded = exact.round().max(0.0);
        let capacity = self.input_capacity();
        if rounded > capacity as f64 {
            self.carry = 0.0;
            return capacity;
        }
        self.carry = exact - rounded;
        rounded as usize
    }
}

impl Deck {
    /// Turns keylock on or off. Switching it on restarts the stretcher.
    pub(super) fn set_keylock(&mut self, on: bool) {
        self.keylock = on;
        if on && let Some(stretcher) = &mut self.stretcher {
            stretcher.resume = None;
        }
    }

    /// Whether a playing, not scratching deck plays through the stretcher: decided by the
    /// tempo ratio alone, so a nudge bends whichever path runs.
    pub(super) fn stretches(&self) -> bool {
        self.keylock
            && self.stretcher.is_some()
            && (self.tempo_ratio() - 1.0).abs() >= BYPASS_EPSILON
    }

    /// Forgets where the last stretched block ended: the deck played without the
    /// stretcher, so the next stretched block restarts it.
    pub(super) fn keylock_bypassed(&mut self) {
        if let Some(stretcher) = &mut self.stretcher {
            stretcher.resume = None;
        }
    }

    /// Renders a block through the stretcher and adds it to the interleaved stereo `out`.
    /// Reads `round_carry(n × speed)` source frames at the original pitch (the track, or
    /// its `stems` with the gains ramped across those reads); the playhead and the ghost
    /// advance by exactly those frames. `step` is the track frames per output frame at the
    /// original tempo, and `bounds` the loop the playhead wraps in.
    pub(super) fn render_stretched(
        &mut self,
        out: &mut [f32],
        track: &TrackBuffer,
        stems: Option<&StemSet>,
        output_rate: u32,
        step: f64,
        bounds: Option<(f64, f64)>,
    ) {
        // Taken out for the block, so the deck's own fields stay free to borrow; a move,
        // not an allocation.
        let Some(mut stretcher) = self.stretcher.take() else {
            return;
        };
        let frames = out.len() / CHANNELS;
        let speed = self.tempo_ratio() * (1.0 + self.bend);
        let here = Resume {
            position: self.position,
            load_seq: self.load_seq,
            bounds,
        };
        if stretcher.resume != Some(here) {
            // The preroll hears the gains where the last block left them.
            let source = Source::new(track, stems, self.stem_held());
            stretcher.restart(source, self.position, bounds, step, speed);
        }

        let read = stretcher.source_frames(frames, speed);
        let source = Source::new(track, stems, self.stem_segments(read));
        stretcher.fill(source, bounds, step, read);
        stretcher.stretch.process(
            &stretcher.input[..read * CHANNELS],
            &mut stretcher.output[..frames * CHANNELS],
        );
        for (o, s) in out.iter_mut().zip(&stretcher.output[..frames * CHANNELS]) {
            *o += s;
        }

        // The playhead follows the source read, through the same reader.
        let end = track.frames() as f64;
        let ghost_moves = self.plays_for_real();
        let ghost_bounds = self.ghost_bounds();
        let mut ended = false;
        for _ in 0..read {
            if read_frame(Source::mix(track), &mut self.position, bounds, step, 0).is_none() {
                ended = true;
                break;
            }
            if ghost_moves && let Some(ghost) = &mut self.ghost {
                slip::advance(ghost, ghost_bounds, step, end);
            }
        }
        let decay = (-1.0 / (jog::NUDGE_DECAY_SECONDS * f64::from(output_rate))).exp();
        self.bend *= decay.powi(i32::try_from(frames).unwrap_or(i32::MAX));
        if ended {
            self.pause();
            stretcher.resume = None;
        } else {
            stretcher.resume = Some(Resume {
                position: self.position,
                load_seq: self.load_seq,
                bounds,
            });
        }
        self.stretcher = Some(stretcher);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounding_carries_the_remainder() {
        let mut stretcher = Stretcher::new(44_100, 512);
        let total: usize = (0..1000)
            .map(|_| stretcher.source_frames(100, 1.0837))
            .sum();
        assert_eq!(total, 108_370);
    }

    #[test]
    fn source_frames_stay_within_the_input_buffer() {
        let mut stretcher = Stretcher::new(44_100, 512);
        let capacity = stretcher.input_capacity();
        assert_eq!(capacity, 1024 + stretcher.input_latency);
        assert_eq!(stretcher.source_frames(512, 100.0), capacity);
    }
}
