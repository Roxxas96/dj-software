//! Single-knob DJ filter: low pass below the center, high pass above, bypassed in a small
//! deadzone around the center.

use super::biquad::{Coeffs, State};
use super::smooth::{Ramp, Segment};

const CENTER: f32 = 0.5;
/// Half-width of the bypass deadzone around the center.
const DEADZONE: f32 = 0.02;
/// Low-pass cutoff at the deadzone edge and at full left.
const LOW_PASS_HZ: (f32, f32) = (20_000.0, 40.0);
/// High-pass cutoff at the deadzone edge and at full right.
const HIGH_PASS_HZ: (f32, f32) = (20.0, 8_000.0);
const Q: f32 = 1.0;

/// Exponential sweep from `from` (t = 0) to `to` (t = 1).
fn sweep((from, to): (f32, f32), t: f32) -> f32 {
    from * (to / from).powf(t)
}

/// Knob position to filter coefficients; `None` inside the deadzone.
fn coeffs(pos: f32, sample_rate: u32) -> Option<Coeffs> {
    let offset = pos - CENTER;
    // The epsilon keeps the literal edges (0.48, 0.52) inside despite f32 rounding.
    if offset.abs() <= DEADZONE + f32::EPSILON {
        return None;
    }
    // 0 at the deadzone edge, 1 at the end of the knob.
    let t = ((offset.abs() - DEADZONE) / (CENTER - DEADZONE)).clamp(0.0, 1.0);
    Some(if offset < 0.0 {
        Coeffs::low_pass(sweep(LOW_PASS_HZ, t), Q, sample_rate)
    } else {
        Coeffs::high_pass(sweep(HIGH_PASS_HZ, t), Q, sample_rate)
    })
}

/// A stereo filter driven by one knob. Entering and leaving the deadzone crossfades
/// between dry and filtered signal across one block, so it does not click.
#[derive(Debug, Clone)]
pub(crate) struct Filter {
    pos: f32,
    /// Coefficients need recomputing for `pos`.
    dirty: bool,
    coeffs: Coeffs,
    channels: [State; 2],
    /// 1.0 when filtering, 0.0 when bypassed.
    wet: Ramp,
}

impl Default for Filter {
    fn default() -> Self {
        Self {
            pos: CENTER,
            dirty: false,
            coeffs: Coeffs::IDENTITY,
            channels: Default::default(),
            wet: Ramp::new(0.0),
        }
    }
}

impl Filter {
    /// Sets the knob position (clamped to `[0, 1]`); coefficients follow on the next block.
    pub(crate) fn set_position(&mut self, pos: f32) {
        self.pos = pos.clamp(0.0, 1.0);
        self.dirty = true;
    }

    /// Starts a block of `frames` frames: recomputes the coefficients if the knob moved and
    /// returns the dry/wet mix, or `None` when fully bypassed.
    pub(crate) fn block(&mut self, frames: usize, sample_rate: u32) -> Option<Segment> {
        if self.dirty {
            self.dirty = false;
            match coeffs(self.pos, sample_rate) {
                Some(c) => {
                    self.coeffs = c;
                    self.wet.target = 1.0;
                }
                // Keep the last coefficients so the fade out still sounds like the filter.
                None => self.wet.target = 0.0,
            }
        }
        let wet = self.wet.block(frames);
        if wet.is_zero() {
            // Idle: start from silence next time it engages.
            self.channels = Default::default();
            None
        } else {
            Some(wet)
        }
    }

    #[inline]
    pub(crate) fn process(&mut self, ch: usize, x: f32) -> f32 {
        self.channels[ch].process(&self.coeffs, x)
    }

    /// Call once per block.
    pub(crate) fn flush_denormals(&mut self) {
        for state in &mut self.channels {
            state.flush_denormals();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deadzone_is_bypass_and_ends_hit_the_range() {
        assert!(coeffs(0.5, 44_100).is_none());
        assert!(coeffs(0.48, 44_100).is_none());
        assert!(coeffs(0.52, 44_100).is_none());
        assert!(coeffs(0.47, 44_100).is_some());
        assert!(coeffs(0.53, 44_100).is_some());
        assert!((sweep(LOW_PASS_HZ, 1.0) - 40.0).abs() < 1e-2);
        assert!((sweep(HIGH_PASS_HZ, 0.0) - 20.0).abs() < 1e-4);
        assert!((sweep(LOW_PASS_HZ, 0.0) - 20_000.0).abs() < 1e-2);
        assert!((sweep(HIGH_PASS_HZ, 1.0) - 8000.0).abs() < 1e-2);
    }
}
