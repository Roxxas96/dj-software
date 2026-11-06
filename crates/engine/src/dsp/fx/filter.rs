//! Resonant low-pass whose cutoff sweeps down with the effect amount.

use crate::dsp::biquad::{Coeffs, State};

const TOP_HZ: f32 = 20_000.0;
const BOTTOM_HZ: f32 = 200.0;
const Q: f32 = 2.0;

#[derive(Debug, Clone)]
pub(super) struct SweepFilter {
    sample_rate: u32,
    coeffs: Coeffs,
    channels: [State; 2],
}

impl SweepFilter {
    pub(super) fn new(sample_rate: u32) -> Self {
        let mut f = Self {
            sample_rate,
            coeffs: Coeffs::IDENTITY,
            channels: Default::default(),
        };
        f.set_amount(0.0);
        f
    }

    /// Exponential sweep from 20 kHz (amount 0) to 200 Hz (amount 1).
    pub(super) fn set_amount(&mut self, amount: f32) {
        let t = amount.clamp(0.0, 1.0);
        let hz = TOP_HZ * (BOTTOM_HZ / TOP_HZ).powf(t);
        self.coeffs = Coeffs::low_pass(hz, Q, self.sample_rate);
    }

    pub(super) fn clear(&mut self) {
        self.channels = Default::default();
    }

    #[inline]
    pub(super) fn process(&mut self, ch: usize, x: f32) -> f32 {
        self.channels[ch].process(&self.coeffs, x)
    }

    pub(super) fn flush_denormals(&mut self) {
        for s in &mut self.channels {
            s.flush_denormals();
        }
    }
}
