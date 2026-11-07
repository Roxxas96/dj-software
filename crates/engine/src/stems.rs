//! Stems: a track split into vocals, instruments, bass and rhythm, and the knob law that
//! turns the EQ knobs into stem gains.
//!
//! A [`StemSet`] stores three stems as 16-bit samples at the track's own rate and frame
//! count; the fourth, instruments, is the residual `mix − (vocals + bass + rhythm)` taken
//! from the deck's track, so the four always sum to the mix exactly.

use std::fmt;
use std::sync::Arc;

use thiserror::Error;

use crate::TrackBuffer;

/// One of a track's four stems.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Stem {
    Vocals,
    /// What the other three leave of the mix.
    Instruments,
    Bass,
    Rhythm,
}

impl Stem {
    /// Every stem, in the order of [`knob_law`]'s gains.
    pub const ALL: [Stem; 4] = [Stem::Vocals, Stem::Instruments, Stem::Bass, Stem::Rhythm];

    /// This stem's place in [`Stem::ALL`].
    pub fn index(self) -> usize {
        match self {
            Stem::Vocals => 0,
            Stem::Instruments => 1,
            Stem::Bass => 2,
            Stem::Rhythm => 3,
        }
    }
}

/// What a channel's EQ knobs control.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EqMode {
    /// The frequency bands, always.
    #[default]
    Eq,
    /// The stems, on a deck whose stems are ready (the bands otherwise): HI vocals, MID
    /// instruments and LOW bass, with the [`knob_law`].
    Stems,
}

/// Why a [`StemSet`] could not be built.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum StemError {
    #[error("the {stem} stem has {samples} samples, expected {expected} ({frames} stereo frames)")]
    Length {
        stem: &'static str,
        samples: usize,
        expected: usize,
        frames: usize,
    },
}

/// The three stored stems of a track, each interleaved stereo 16-bit at the track's rate.
#[derive(Clone, PartialEq, Eq)]
pub struct StemSet {
    /// Vocals, bass and rhythm.
    stems: [Box<[i16]>; 3],
    frames: usize,
}

impl fmt::Debug for StemSet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StemSet")
            .field("frames", &self.frames)
            .finish_non_exhaustive()
    }
}

/// The scale of a 16-bit sample: `i16::MIN` is −1.0.
const I16_SCALE: f32 = 1.0 / 32_768.0;

impl StemSet {
    /// Checks that each stem holds `frames` stereo frames.
    pub fn new(
        vocals: Box<[i16]>,
        bass: Box<[i16]>,
        rhythm: Box<[i16]>,
        frames: usize,
    ) -> Result<Self, StemError> {
        let expected = frames * 2;
        for (stem, samples) in [("vocals", &vocals), ("bass", &bass), ("rhythm", &rhythm)] {
            if samples.len() != expected {
                return Err(StemError::Length {
                    stem,
                    samples: samples.len(),
                    expected,
                    frames,
                });
            }
        }
        Ok(Self {
            stems: [vocals, bass, rhythm],
            frames,
        })
    }

    pub fn frames(&self) -> usize {
        self.frames
    }

    /// The four stems of frame `index`, `[left, right]` each, in [`Stem::ALL`] order;
    /// instruments is what the other three leave of `mix`.
    ///
    /// # Panics
    ///
    /// If `index` is outside `self` or `mix`.
    pub(crate) fn frame(&self, mix: &TrackBuffer, index: usize) -> [[f32; 2]; 4] {
        let [vocals, bass, rhythm] = self
            .stems
            .each_ref()
            .map(|s| [s[2 * index], s[2 * index + 1]].map(|v| f32::from(v) * I16_SCALE));
        let mix = mix.frame(index);
        let instruments = [0, 1].map(|ch| mix[ch] - (vocals[ch] + bass[ch] + rhythm[ch]));
        [vocals, instruments, bass, rhythm]
    }

    /// `Σ gains[s] × stem[s]` of frame `index`.
    #[inline]
    pub(crate) fn mixed(&self, mix: &TrackBuffer, index: usize, gains: [f32; 4]) -> [f32; 2] {
        let stems = self.frame(mix, index);
        let mut out = [0.0; 2];
        for (stem, gain) in stems.iter().zip(gains) {
            out[0] += stem[0] * gain;
            out[1] += stem[1] * gain;
        }
        out
    }
}

/// The stem gains for the EQ knob positions `low`, `mid` and `high` (each in `[0, 1]`,
/// 0.5 the middle), in [`Stem::ALL`] order. HI controls vocals, MID instruments and LOW
/// bass; rhythm has no knob.
///
/// Each knob's own level is `min(1, 2p)` and its isolation `max(0, 2p − 1)`; with `I` the
/// largest isolation, a knob stem whose isolation is above 0 keeps its own level (1), the
/// other knob stems get `own × (1 − I)` and rhythm gets `1 − I`.
pub fn knob_law(low: f32, mid: f32, high: f32) -> [f32; 4] {
    // In `Stem::ALL` order, without rhythm.
    let knobs = [high, mid, low].map(|p| p.clamp(0.0, 1.0));
    let own = knobs.map(|p| (2.0 * p).min(1.0));
    let iso = knobs.map(|p| (2.0 * p - 1.0).max(0.0));
    let isolation = iso.into_iter().fold(0.0, f32::max);
    let knob = |k: usize| {
        if iso[k] > 0.0 {
            own[k]
        } else {
            own[k] * (1.0 - isolation)
        }
    };
    [knob(0), knob(1), knob(2), 1.0 - isolation]
}

/// What the renderer hands back to the controller to free off the audio thread.
#[derive(Debug)]
#[expect(dead_code, reason = "held only to be dropped off the audio thread")]
pub(crate) enum Garbage {
    Track(Arc<TrackBuffer>),
    Stems(Arc<StemSet>),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_four_stems_sum_to_the_mix() {
        let mix = TrackBuffer::new(44_100, vec![0.5, -0.25, 0.125, 0.75]).unwrap();
        let set = StemSet::new(
            vec![1000, -2000, 3000, 4000].into(),
            vec![-16_384, 5, 32_767, -32_768].into(),
            vec![7, 8, 9, 10].into(),
            2,
        )
        .unwrap();
        for index in 0..2 {
            let stems = set.frame(&mix, index);
            for ch in 0..2 {
                let sum: f32 = stems.iter().map(|s| s[ch]).sum();
                assert!((sum - mix.frame(index)[ch]).abs() < 1e-6);
            }
        }
        assert_eq!(set.frame(&mix, 0)[2][0], -0.5);
        assert_eq!(set.mixed(&mix, 1, [1.0; 4]), {
            let s = set.frame(&mix, 1);
            [0, 1].map(|ch| s[0][ch] + s[1][ch] + s[2][ch] + s[3][ch])
        });
    }

    #[test]
    fn stem_indices_follow_all() {
        for (i, stem) in Stem::ALL.into_iter().enumerate() {
            assert_eq!(stem.index(), i);
        }
    }
}
