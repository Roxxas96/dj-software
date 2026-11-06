//! Beat effects: echo, reverb, flanger and a sweeping low-pass filter.
//!
//! One [`Effect`] holds the state of every kind, all allocated up front, so switching the
//! kind on the audio thread never allocates. [`Effect::process`] returns the wet signal
//! only; the channel mixes it with the dry input by the effect amount.

mod echo;
mod filter;
mod flanger;
mod reverb;

/// Beat period used when the deck has no usable grid: 120 BPM.
const FALLBACK_BEAT_SECONDS: f64 = 0.5;

/// Which effect an [`Effect`] runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EffectKind {
    #[default]
    Echo,
    Reverb,
    Flanger,
    Filter,
}

impl EffectKind {
    /// Stable identifier used in mapping files and settings.
    #[must_use]
    pub fn id(self) -> &'static str {
        match self {
            Self::Echo => "echo",
            Self::Reverb => "reverb",
            Self::Flanger => "flanger",
            Self::Filter => "filter",
        }
    }

    /// The kind with identifier `id`, if any.
    #[must_use]
    pub fn from_id(id: &str) -> Option<Self> {
        Some(match id {
            "echo" => Self::Echo,
            "reverb" => Self::Reverb,
            "flanger" => Self::Flanger,
            "filter" => Self::Filter,
            _ => return None,
        })
    }
}

/// A stereo effect with the state of every kind.
#[derive(Debug, Clone)]
pub(crate) struct Effect {
    kind: EffectKind,
    echo: echo::Echo,
    reverb: reverb::Reverb,
    flanger: flanger::Flanger,
    filter: filter::SweepFilter,
}

impl Effect {
    /// Allocates every buffer; call off the audio thread.
    pub(crate) fn new(sample_rate: u32) -> Self {
        Self {
            kind: EffectKind::default(),
            echo: echo::Echo::new(sample_rate),
            reverb: reverb::Reverb::new(sample_rate),
            flanger: flanger::Flanger::new(sample_rate),
            filter: filter::SweepFilter::new(sample_rate),
        }
    }

    /// Switches kind and clears all state.
    pub(crate) fn set_kind(&mut self, kind: EffectKind) {
        self.kind = kind;
        self.echo.clear();
        self.reverb.clear();
        self.flanger.clear();
        self.filter.clear();
    }

    /// Clears the state of the running kind only, leaving the others' buffers alone: the
    /// cheap way to start a slot that was off over.
    pub(crate) fn clear_active(&mut self) {
        match self.kind {
            EffectKind::Echo => self.echo.clear(),
            EffectKind::Reverb => self.reverb.clear(),
            EffectKind::Flanger => self.flanger.clear(),
            EffectKind::Filter => self.filter.clear(),
        }
    }

    /// The Filter kind's cutoff follows `amount` (0 to 1, a non-finite value counts as 0);
    /// the other kinds ignore it. Recomputes coefficients: call when the amount changes.
    pub(crate) fn set_amount(&mut self, amount: f32) {
        self.filter.set_amount(amount);
    }

    /// Wet output for input `x` on channel `ch` (0 or 1); `beat_frames` is the beat period
    /// in output frames. Call both channels of a frame in order.
    #[inline]
    pub(crate) fn process(&mut self, ch: usize, x: f32, beat_frames: f64) -> f32 {
        let beat = if beat_frames.is_finite() && beat_frames >= 1.0 {
            beat_frames
        } else {
            FALLBACK_BEAT_SECONDS * self.echo.sample_rate()
        };
        match self.kind {
            EffectKind::Echo => self.echo.process(ch, x, beat),
            EffectKind::Reverb => self.reverb.process(ch, x),
            EffectKind::Flanger => self.flanger.process(ch, x, beat),
            EffectKind::Filter => self.filter.process(ch, x),
        }
    }

    /// Call once per block.
    pub(crate) fn flush_denormals(&mut self) {
        self.reverb.flush_denormals();
        self.filter.flush_denormals();
    }
}

/// Test hook: drives one effect from the integration tests.
#[doc(hidden)]
#[derive(Debug, Clone)]
pub struct EffectForTests(Effect);

impl EffectForTests {
    pub fn set_kind(&mut self, kind: EffectKind) {
        self.0.set_kind(kind);
    }

    pub fn set_amount(&mut self, amount: f32) {
        self.0.set_amount(amount);
    }

    pub fn process(&mut self, ch: usize, x: f32, beat_frames: f64) -> f32 {
        self.0.process(ch, x, beat_frames)
    }

    pub fn flush_denormals(&mut self) {
        self.0.flush_denormals();
    }
}

/// Builds an effect of `kind` for the integration tests.
#[doc(hidden)]
#[must_use]
pub fn effect_for_tests(sample_rate: u32, kind: EffectKind) -> EffectForTests {
    let mut effect = Effect::new(sample_rate);
    effect.set_kind(kind);
    EffectForTests(effect)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: u32 = 48_000;
    const BEAT: f64 = 24_000.0;

    fn feed(effect: &mut Effect, x: f32, frames: usize) -> f32 {
        let mut energy = 0.0;
        for _ in 0..frames {
            energy += effect.process(0, x, BEAT).abs();
            effect.process(1, x, BEAT);
        }
        energy
    }

    #[test]
    fn clear_active_clears_only_the_running_kind() {
        let mut effect = Effect::new(SR);
        effect.set_kind(EffectKind::Echo);
        feed(&mut effect, 1.0, 5_000);
        // Switched without a clear, as only this test can: the echo keeps its buffers.
        effect.kind = EffectKind::Reverb;
        feed(&mut effect, 1.0, 5_000);
        effect.clear_active();
        assert_eq!(feed(&mut effect, 0.0, 30_000), 0.0, "reverb cleared");
        effect.kind = EffectKind::Echo;
        assert!(feed(&mut effect, 0.0, 30_000) > 0.0, "echo kept");
    }
}
