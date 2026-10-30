use thiserror::Error;

/// Why a [`TrackBuffer`] could not be built.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum TrackError {
    #[error("interleaved stereo needs an even sample count, got {0}")]
    OddSampleCount(usize),
    #[error("sample rate must be positive")]
    ZeroSampleRate,
}

/// A fully decoded track held in memory as interleaved stereo `f32` samples.
#[derive(Debug, Clone, PartialEq)]
pub struct TrackBuffer {
    sample_rate: u32,
    samples: Box<[f32]>,
}

impl TrackBuffer {
    /// Non-finite samples (NaN, infinities) are replaced with silence so they can never
    /// reach the mixer, where a single one would poison the filter state for good.
    pub fn new(sample_rate: u32, mut interleaved_stereo: Vec<f32>) -> Result<Self, TrackError> {
        if sample_rate == 0 {
            return Err(TrackError::ZeroSampleRate);
        }
        if !interleaved_stereo.len().is_multiple_of(2) {
            return Err(TrackError::OddSampleCount(interleaved_stereo.len()));
        }
        for sample in &mut interleaved_stereo {
            if !sample.is_finite() {
                *sample = 0.0;
            }
        }
        Ok(Self {
            sample_rate,
            samples: interleaved_stereo.into_boxed_slice(),
        })
    }

    pub fn frames(&self) -> usize {
        self.samples.len() / 2
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Returns `[left, right]` of frame `index`.
    ///
    /// # Panics
    ///
    /// If `index >= self.frames()`.
    pub fn frame(&self, index: usize) -> [f32; 2] {
        [self.samples[2 * index], self.samples[2 * index + 1]]
    }
}
