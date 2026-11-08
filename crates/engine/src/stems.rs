//! Stems: a track split into vocals, instruments, bass and rhythm, rhythm split again into
//! kicks and hats, and the knob law that turns the EQ knobs into the gains of those parts.
//!
//! A [`StemSet`] stores three stems as 16-bit samples at the track's own rate and frame
//! count, plus kicks, which it computes from rhythm when it is built (off the audio
//! thread). Instruments is the residual `mix − (vocals + bass + rhythm)` taken from the
//! deck's track and hats is `rhythm − kicks`, both per frame, so the five parts a deck
//! mixes always sum to the mix exactly.

use std::f32::consts::FRAC_1_SQRT_2;
use std::fmt;
use std::sync::Arc;

use thiserror::Error;

use crate::TrackBuffer;
use crate::dsp::biquad::{Coeffs, State};
use crate::mixer::eq_gain;

/// The parts a deck mixes: vocals, instruments, bass, kicks and hats, in that order.
pub(crate) const PARTS: usize = 5;

/// The default frequency, in Hz, where kicks hand over to hats (`[stems] kicks_hz`). Kicks
/// and hats are each at half level (−6 dB) there. The normal EQ's 250 Hz low crossover left
/// too much kick in the hats on the owner's tracks, so the default sits a little higher.
pub const DEFAULT_KICKS_HZ: f32 = 300.0;

/// Frames of rhythm the kicks' backward pass filters at a time.
const KICKS_CHUNK: usize = 1 << 16;

/// A stem pad: one of a track's four stems, or one of the knob parts made of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Stem {
    Vocals,
    /// What the other three stems leave of the mix.
    Instruments,
    Bass,
    Rhythm,
    /// The low end of rhythm, below the split (see [`StemSet::with_split`]).
    Kicks,
    /// Vocals, instruments and bass together.
    Melody,
    /// What kicks leave of rhythm: hi-hats, and snares and other percussion too.
    Hats,
}

impl Stem {
    /// Every pad, in the order of [`pad_gains`]' flags.
    pub const ALL: [Stem; 7] = [
        Stem::Vocals,
        Stem::Instruments,
        Stem::Bass,
        Stem::Rhythm,
        Stem::Kicks,
        Stem::Melody,
        Stem::Hats,
    ];

    /// This pad's place in [`Stem::ALL`].
    pub fn index(self) -> usize {
        match self {
            Stem::Vocals => 0,
            Stem::Instruments => 1,
            Stem::Bass => 2,
            Stem::Rhythm => 3,
            Stem::Kicks => 4,
            Stem::Melody => 5,
            Stem::Hats => 6,
        }
    }
}

/// What a channel's EQ knobs control.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EqMode {
    /// The frequency bands, always.
    #[default]
    Eq,
    /// The stems, on a deck whose stems are ready (the bands otherwise), in the
    /// [`StemKnobs`] layout with the [`knob_law`].
    Stems,
}

/// Which parts the EQ knobs control in [`EqMode::Stems`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StemKnobs {
    /// LOW the kicks, MID the melody (vocals, instruments and bass), HI the hats: all three
    /// fully left is silence.
    #[default]
    Vdj,
    /// HI the vocals, MID the instruments, LOW the bass; rhythm has no knob.
    Raw,
}

/// Why a [`StemSet`] could not be built.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum StemError {
    #[error("the {stem} stem has {samples} samples, expected {expected} ({frames} stereo frames)")]
    Length {
        stem: &'static str,
        samples: usize,
        expected: usize,
        frames: usize,
    },
    #[error("the stems' sample rate is 0")]
    SampleRate,
    #[error("the kicks split of {hz} Hz is not between 0 and half the {sample_rate} Hz rate")]
    Split { hz: f32, sample_rate: u32 },
}

/// The stored stems of a track, each interleaved stereo 16-bit at the track's rate: the
/// three from the separation, and kicks.
#[derive(Clone, PartialEq, Eq)]
pub struct StemSet {
    /// Vocals, bass, rhythm and kicks.
    stems: [Box<[i16]>; 4],
    frames: usize,
    sample_rate: u32,
}

impl fmt::Debug for StemSet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StemSet")
            .field("frames", &self.frames)
            .field("sample_rate", &self.sample_rate)
            .finish_non_exhaustive()
    }
}

/// The scale of a 16-bit sample: `i16::MIN` is −1.0.
const I16_SCALE: f32 = 1.0 / 32_768.0;

fn to_i16(x: f32) -> i16 {
    // In range after the clamp: the cast only drops the fraction `round` already removed.
    (x * 32_768.0).round().clamp(-32_768.0, 32_767.0) as i16
}

impl StemSet {
    /// [`StemSet::with_split`] at [`DEFAULT_KICKS_HZ`].
    pub fn new(
        vocals: Box<[i16]>,
        bass: Box<[i16]>,
        rhythm: Box<[i16]>,
        frames: usize,
        sample_rate: u32,
    ) -> Result<Self, StemError> {
        Self::with_split(vocals, bass, rhythm, frames, sample_rate, DEFAULT_KICKS_HZ)
    }

    /// Checks that each stem holds `frames` stereo frames, then computes kicks from rhythm
    /// at `sample_rate`, the track's rate, splitting at `kicks_hz`. That takes about 0.6 s
    /// for a 5-minute track (on an M1 Pro, release build), so call it off the audio thread.
    pub fn with_split(
        vocals: Box<[i16]>,
        bass: Box<[i16]>,
        rhythm: Box<[i16]>,
        frames: usize,
        sample_rate: u32,
        kicks_hz: f32,
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
        if sample_rate == 0 {
            return Err(StemError::SampleRate);
        }
        if !(kicks_hz > 0.0 && kicks_hz < sample_rate as f32 / 2.0) {
            return Err(StemError::Split {
                hz: kicks_hz,
                sample_rate,
            });
        }
        let kicks = kicks(&rhythm, frames, sample_rate, kicks_hz, KICKS_CHUNK);
        Ok(Self {
            stems: [vocals, bass, rhythm, kicks],
            frames,
            sample_rate,
        })
    }

    pub fn frames(&self) -> usize {
        self.frames
    }

    /// The rate kicks were computed at: the track's.
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// The stored samples of `stem`, interleaved stereo: vocals, bass, rhythm and kicks.
    /// `None` for the parts computed per frame (instruments and hats) and for melody.
    pub fn stored(&self, stem: Stem) -> Option<&[i16]> {
        match stem {
            Stem::Vocals => Some(&self.stems[0]),
            Stem::Bass => Some(&self.stems[1]),
            Stem::Rhythm => Some(&self.stems[2]),
            Stem::Kicks => Some(&self.stems[3]),
            Stem::Instruments | Stem::Melody | Stem::Hats => None,
        }
    }

    /// The five parts of frame `index`, `[left, right]` each, in [`knob_law`] order:
    /// instruments is what the three stems leave of `mix`, hats what kicks leave of
    /// rhythm.
    ///
    /// # Panics
    ///
    /// If `index` is outside `self` or `mix`.
    pub(crate) fn frame(&self, mix: &TrackBuffer, index: usize) -> [[f32; 2]; PARTS] {
        let [vocals, bass, rhythm, kicks] = self
            .stems
            .each_ref()
            .map(|s| [s[2 * index], s[2 * index + 1]].map(|v| f32::from(v) * I16_SCALE));
        let mix = mix.frame(index);
        let instruments = [0, 1].map(|ch| mix[ch] - (vocals[ch] + bass[ch] + rhythm[ch]));
        let hats = [0, 1].map(|ch| rhythm[ch] - kicks[ch]);
        [vocals, instruments, bass, kicks, hats]
    }

    /// `Σ gains[p] × part[p]` of frame `index`.
    #[inline]
    pub(crate) fn mixed(&self, mix: &TrackBuffer, index: usize, gains: [f32; PARTS]) -> [f32; 2] {
        let parts = self.frame(mix, index);
        let mut out = [0.0; 2];
        for (part, gain) in parts.iter().zip(gains) {
            out[0] += part[0] * gain;
            out[1] += part[1] * gain;
        }
        out
    }
}

/// Kicks: rhythm through a zero-phase low pass at `kicks_hz`, a Butterworth biquad run
/// forward and then backward (an exact −6 dB at the split, falling 24 dB per octave, with
/// no phase shift, so hats is the matching high pass).
///
/// Both passes start settled on the signal's first sample, so the track's edges do not
/// ring. The backward pass runs over `chunk` frames at a time, each started a tenth of a
/// second past its end (where the filter's memory has died out), so only one chunk of
/// float samples is held.
fn kicks(
    rhythm: &[i16],
    frames: usize,
    sample_rate: u32,
    kicks_hz: f32,
    chunk: usize,
) -> Box<[i16]> {
    const FLUSH_EVERY: usize = 4096;
    let mut kicks = vec![0_i16; frames * 2].into_boxed_slice();
    if frames == 0 {
        return kicks;
    }
    let c = Coeffs::low_pass(kicks_hz, FRAC_1_SQRT_2, sample_rate);
    let overlap = (sample_rate as usize / 10).max(1);
    let mut scratch = Vec::with_capacity((chunk + overlap).min(frames));
    let mut starts = Vec::with_capacity(frames.div_ceil(chunk));
    for ch in 0..2 {
        let sample = |i: usize| f32::from(rhythm[2 * i + ch]) * I16_SCALE;
        // Forward, keeping the filter's memory at each chunk's start.
        starts.clear();
        let mut state = State::settled(&c, sample(0));
        for i in 0..frames {
            if i.is_multiple_of(chunk) {
                state.flush_denormals();
                starts.push(state);
            }
            state.process(&c, sample(i));
        }
        for (k, &start_state) in starts.iter().enumerate() {
            let start = k * chunk;
            let end = (start + chunk).min(frames);
            let past = (end + overlap).min(frames);
            let mut forward = start_state;
            scratch.clear();
            scratch.extend((start..past).map(|i| {
                if (i - start).is_multiple_of(FLUSH_EVERY) {
                    forward.flush_denormals();
                }
                forward.process(&c, sample(i))
            }));
            let mut backward = State::settled(&c, scratch[scratch.len() - 1]);
            for (j, &y) in scratch.iter().enumerate().rev() {
                if j.is_multiple_of(FLUSH_EVERY) {
                    backward.flush_denormals();
                }
                let z = backward.process(&c, y);
                if start + j < end {
                    kicks[2 * (start + j) + ch] = to_i16(z);
                }
            }
        }
    }
    kicks
}

/// The part gains for the EQ knob positions `[low, mid, high]` (each in `[0, 1]`, 0.5 the
/// middle) in the `layout`, in part order: vocals, instruments, bass, kicks, hats.
///
/// Each knob gives its parts [`eq_gain`]: silent fully left, a squared law up to the part's
/// own level at the centre, then up to +6 dB fully right. A knob only changes its own
/// parts. In [`StemKnobs::Vdj`], LOW is kicks, MID vocals, instruments and bass, and HI
/// hats; in [`StemKnobs::Raw`], HI is vocals, MID instruments and LOW bass, and kicks and
/// hats stay at 1.
pub fn knob_law(layout: StemKnobs, [low, mid, high]: [f32; 3]) -> [f32; PARTS] {
    match layout {
        StemKnobs::Vdj => {
            let melody = eq_gain(mid);
            [melody, melody, melody, eq_gain(low), eq_gain(high)]
        }
        StemKnobs::Raw => [eq_gain(high), eq_gain(mid), eq_gain(low), 1.0, 1.0],
    }
}

/// The part gains for the stem pads `on`, in [`Stem::ALL`] order: each part plays only if
/// every pad covering it is on. Melody covers vocals, instruments and bass; rhythm covers
/// kicks and hats.
pub fn pad_gains(on: [bool; 7]) -> [f32; PARTS] {
    let [vocals, instruments, bass, rhythm, kicks, melody, hats] =
        on.map(|on| if on { 1.0 } else { 0.0 });
    [
        vocals * melody,
        instruments * melody,
        bass * melody,
        rhythm * kicks,
        rhythm * hats,
    ]
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
    fn the_five_parts_sum_to_the_mix() {
        let mix = TrackBuffer::new(44_100, vec![0.5, -0.25, 0.125, 0.75]).unwrap();
        let set = StemSet::new(
            vec![1000, -2000, 3000, 4000].into(),
            vec![-16_384, 5, 32_767, -32_768].into(),
            vec![7, 8, 9, 10].into(),
            2,
            44_100,
        )
        .unwrap();
        for index in 0..2 {
            let parts = set.frame(&mix, index);
            for ch in 0..2 {
                let sum: f32 = parts.iter().map(|s| s[ch]).sum();
                assert!((sum - mix.frame(index)[ch]).abs() < 1e-6);
                // Kicks and hats sum back to rhythm.
                let rhythm = f32::from(set.stems[2][2 * index + ch]) * I16_SCALE;
                assert_eq!(parts[3][ch] + parts[4][ch], rhythm);
            }
        }
        assert_eq!(set.frame(&mix, 0)[2][0], -0.5);
        assert_eq!(set.mixed(&mix, 1, [1.0; PARTS]), {
            let s = set.frame(&mix, 1);
            [0, 1].map(|ch| s[0][ch] + s[1][ch] + s[2][ch] + s[3][ch] + s[4][ch])
        });
    }

    #[test]
    fn stem_indices_follow_all() {
        for (i, stem) in Stem::ALL.into_iter().enumerate() {
            assert_eq!(stem.index(), i);
        }
    }

    /// Kicks filtered over the whole track in one piece.
    fn kicks_in_one_piece(rhythm: &[i16], frames: usize, rate: u32) -> Vec<i16> {
        let c = Coeffs::low_pass(DEFAULT_KICKS_HZ, FRAC_1_SQRT_2, rate);
        let mut out = vec![0; frames * 2];
        for ch in 0..2 {
            let x: Vec<f32> = (0..frames)
                .map(|i| f32::from(rhythm[2 * i + ch]) * I16_SCALE)
                .collect();
            let mut state = State::settled(&c, x[0]);
            let y: Vec<f32> = x.iter().map(|&x| state.process(&c, x)).collect();
            let mut state = State::settled(&c, y[frames - 1]);
            for i in (0..frames).rev() {
                out[2 * i + ch] = to_i16(state.process(&c, y[i]));
            }
        }
        out
    }

    #[test]
    fn kicks_in_chunks_match_one_piece() {
        let frames = 30_000;
        // A kick-ish 50 Hz tone and noise, different per channel.
        let mut seed = 1_u32;
        let rhythm: Vec<i16> = (0..frames * 2)
            .map(|i| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let noise = (seed >> 16) as f32 / 65_536.0 - 0.5;
                let t = (i / 2) as f32 / 44_100.0;
                let tone = 0.5 * (std::f32::consts::TAU * 50.0 * t + (i % 2) as f32).sin();
                to_i16(tone + 0.3 * noise)
            })
            .collect();
        let whole = kicks_in_one_piece(&rhythm, frames, 44_100);
        for chunk in [1000, 7_777, KICKS_CHUNK] {
            let chunked = kicks(&rhythm, frames, 44_100, DEFAULT_KICKS_HZ, chunk);
            let off = whole
                .iter()
                .zip(chunked.iter())
                .map(|(a, b)| (i32::from(*a) - i32::from(*b)).abs())
                .max()
                .unwrap();
            assert!(off <= 1, "chunks of {chunk}: off by {off} steps");
        }
        // The edges do not ring: a constant rhythm gives constant kicks.
        let constant = vec![8000_i16; 2000];
        assert!(
            kicks(&constant, 1000, 44_100, DEFAULT_KICKS_HZ, 300)
                .iter()
                .all(|&k| (k - 8000).abs() <= 1)
        );
        assert!(kicks(&[], 0, 44_100, DEFAULT_KICKS_HZ, 300).is_empty());
    }

    #[test]
    fn layouts_kill_everything_or_leave_rhythm() {
        assert_eq!(knob_law(StemKnobs::Vdj, [0.0; 3]), [0.0; PARTS]);
        assert_eq!(
            knob_law(StemKnobs::Raw, [0.0; 3]),
            [0.0, 0.0, 0.0, 1.0, 1.0]
        );
        assert_eq!(pad_gains([true; 7]), [1.0; PARTS]);
    }
}
