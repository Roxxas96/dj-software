//! Three-band isolator: Linkwitz-Riley 4th-order crossovers split the signal into low, mid
//! and high bands that sum back to an allpass (flat magnitude), so each band can be scaled
//! from a true kill to a boost.

use super::biquad::{Coeffs, State};

const LOW_CROSSOVER_HZ: f32 = 250.0;
const HIGH_CROSSOVER_HZ: f32 = 2500.0;
/// Butterworth Q: two cascaded sections make one Linkwitz-Riley 4th-order filter.
const BUTTERWORTH_Q: f32 = std::f32::consts::FRAC_1_SQRT_2;

/// Crossover coefficients, shared by both channels.
#[derive(Debug, Clone, Copy)]
struct Crossovers {
    low_lp: Coeffs,
    low_hp: Coeffs,
    high_lp: Coeffs,
    high_hp: Coeffs,
    /// LR4 low pass + high pass at the high crossover sum to this allpass: the low band
    /// goes through it so all three bands share the same phase.
    high_ap: Coeffs,
}

/// One channel's crossover memory.
#[derive(Debug, Clone, Copy, Default)]
struct ChannelState {
    low_lp: [State; 2],
    low_hp: [State; 2],
    high_lp: [State; 2],
    high_hp: [State; 2],
    high_ap: State,
}

impl ChannelState {
    fn states_mut(&mut self) -> impl Iterator<Item = &mut State> {
        self.low_lp
            .iter_mut()
            .chain(&mut self.low_hp)
            .chain(&mut self.high_lp)
            .chain(&mut self.high_hp)
            .chain(std::iter::once(&mut self.high_ap))
    }
}

/// Runs a sample through two cascaded sections sharing `c` (one LR4 filter).
#[inline]
fn lr4(states: &mut [State; 2], c: &Coeffs, x: f32) -> f32 {
    let y = states[0].process(c, x);
    states[1].process(c, y)
}

/// A stereo three-band splitter.
#[derive(Debug, Clone)]
pub(crate) struct Isolator {
    crossovers: Crossovers,
    channels: [ChannelState; 2],
}

impl Isolator {
    pub(crate) fn new(sample_rate: u32) -> Self {
        let q = BUTTERWORTH_Q;
        Self {
            crossovers: Crossovers {
                low_lp: Coeffs::low_pass(LOW_CROSSOVER_HZ, q, sample_rate),
                low_hp: Coeffs::high_pass(LOW_CROSSOVER_HZ, q, sample_rate),
                high_lp: Coeffs::low_pass(HIGH_CROSSOVER_HZ, q, sample_rate),
                high_hp: Coeffs::high_pass(HIGH_CROSSOVER_HZ, q, sample_rate),
                high_ap: Coeffs::all_pass(HIGH_CROSSOVER_HZ, q, sample_rate),
            },
            channels: Default::default(),
        }
    }

    /// Splits one sample of channel `ch` (0 left, 1 right) into `[low, mid, high]`.
    #[inline]
    pub(crate) fn split(&mut self, ch: usize, x: f32) -> [f32; 3] {
        let c = &self.crossovers;
        let s = &mut self.channels[ch];
        let low = lr4(&mut s.low_lp, &c.low_lp, x);
        let low = s.high_ap.process(&c.high_ap, low);
        let rest = lr4(&mut s.low_hp, &c.low_hp, x);
        let mid = lr4(&mut s.high_lp, &c.high_lp, rest);
        let high = lr4(&mut s.high_hp, &c.high_hp, rest);
        [low, mid, high]
    }

    /// Call once per block.
    pub(crate) fn flush_denormals(&mut self) {
        for state in self.channels.iter_mut().flat_map(ChannelState::states_mut) {
            state.flush_denormals();
        }
    }
}
