//! Second-order IIR sections: RBJ cookbook coefficients, transposed direct form II.

use std::f64::consts::TAU;

/// State magnitudes below this are flushed to zero once per block, so a decaying filter
/// never lingers in (slow, on some CPUs) subnormal arithmetic.
const FLUSH_BELOW: f32 = 1e-20;

/// Normalized biquad coefficients (`a0 == 1`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Coeffs {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
}

impl Coeffs {
    /// Passes the input through unchanged.
    pub(crate) const IDENTITY: Self = Self {
        b0: 1.0,
        b1: 0.0,
        b2: 0.0,
        a1: 0.0,
        a2: 0.0,
    };

    pub(crate) fn low_pass(freq: f32, q: f32, sample_rate: u32) -> Self {
        let w = Omega::new(freq, q, sample_rate);
        let b = (1.0 - w.cos) / 2.0;
        w.normalize(b, 2.0 * b, b)
    }

    pub(crate) fn high_pass(freq: f32, q: f32, sample_rate: u32) -> Self {
        let w = Omega::new(freq, q, sample_rate);
        let b = (1.0 + w.cos) / 2.0;
        w.normalize(b, -2.0 * b, b)
    }

    pub(crate) fn all_pass(freq: f32, q: f32, sample_rate: u32) -> Self {
        let w = Omega::new(freq, q, sample_rate);
        w.normalize(1.0 - w.alpha, -2.0 * w.cos, 1.0 + w.alpha)
    }
}

/// The shared RBJ intermediates, computed in `f64` so low cutoffs keep their precision.
struct Omega {
    cos: f64,
    alpha: f64,
}

impl Omega {
    fn new(freq: f32, q: f32, sample_rate: u32) -> Self {
        let sample_rate = f64::from(sample_rate);
        // Keep the cutoff strictly inside (0, Nyquist), where the formulas are stable.
        let freq = f64::from(freq).clamp(1.0, 0.49 * sample_rate);
        let w0 = TAU * freq / sample_rate;
        Self {
            cos: w0.cos(),
            alpha: w0.sin() / (2.0 * f64::from(q)),
        }
    }

    fn normalize(&self, b0: f64, b1: f64, b2: f64) -> Coeffs {
        let a0 = 1.0 + self.alpha;
        Coeffs {
            b0: (b0 / a0) as f32,
            b1: (b1 / a0) as f32,
            b2: (b2 / a0) as f32,
            a1: (-2.0 * self.cos / a0) as f32,
            a2: ((1.0 - self.alpha) / a0) as f32,
        }
    }
}

/// One channel's filter memory.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct State {
    s1: f32,
    s2: f32,
}

impl State {
    #[inline]
    pub(crate) fn process(&mut self, c: &Coeffs, x: f32) -> f32 {
        let y = c.b0 * x + self.s1;
        self.s1 = c.b1 * x - c.a1 * y + self.s2;
        self.s2 = c.b2 * x - c.a2 * y;
        y
    }

    /// Zeroes negligible state; call once per block.
    pub(crate) fn flush_denormals(&mut self) {
        for s in [&mut self.s1, &mut self.s2] {
            if s.abs() < FLUSH_BELOW {
                *s = 0.0;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Steady-state gain at `freq` from a sine through `c`.
    fn gain(c: &Coeffs, freq: f64, sample_rate: u32) -> f32 {
        let mut state = State::default();
        let mut peak = 0.0_f32;
        for i in 0..20_000 {
            let x = (TAU * freq * f64::from(i) / f64::from(sample_rate)).sin() as f32;
            let y = state.process(c, x);
            if i >= 10_000 {
                peak = peak.max(y.abs());
            }
        }
        peak
    }

    #[test]
    fn cutoff_is_minus_3_db_for_butterworth_q() {
        let q = std::f32::consts::FRAC_1_SQRT_2;
        let lp = Coeffs::low_pass(1000.0, q, 48_000);
        let hp = Coeffs::high_pass(1000.0, q, 48_000);
        for c in [lp, hp] {
            let db = 20.0 * gain(&c, 1000.0, 48_000).log10();
            assert!((db + 3.01).abs() < 0.05, "{db}");
        }
        assert!(gain(&lp, 50.0, 48_000) > 0.999);
        assert!(gain(&hp, 15_000.0, 48_000) > 0.99);
    }

    #[test]
    fn all_pass_is_flat() {
        let ap = Coeffs::all_pass(2500.0, std::f32::consts::FRAC_1_SQRT_2, 44_100);
        for freq in [100.0, 2500.0, 10_000.0] {
            assert!((gain(&ap, freq, 44_100) - 1.0).abs() < 1e-3);
        }
    }

    #[test]
    fn identity_passes_through_and_flush_zeroes_tiny_state() {
        let mut state = State::default();
        assert_eq!(state.process(&Coeffs::IDENTITY, 0.3), 0.3);
        state.s1 = 1e-30;
        state.s2 = 0.5;
        state.flush_denormals();
        assert_eq!(state, State { s1: 0.0, s2: 0.5 });
    }
}
