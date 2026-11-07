//! Four-channel DJ mixer: channel strips, crossfader, master and headphone buses.
//!
//! Signal flow per channel: deck → trim → EQ → filter → effects → PFL tap → fader → crossfader →
//! master sum → master level → clamp to `[-1, 1]`. The sampler's output joins the master
//! sum after the channels. Headphones get
//! `lerp(cue_sum, master_pre_clamp, mix) * headphone_level`, clamped, where `cue_sum` is the
//! sum of the PFL taps. Every gain change is ramped linearly across one block.
//!
//! The EQ is an always-on isolator: at unity its bands sum to an allpass (flat magnitude,
//! DC passes at unity once settled), so the channel is never bit-exact dry.
//!
//! While a channel's deck plays its stems with the knobs ([`EqMode::Stems`](crate::EqMode)),
//! the EQ stage is bypassed: the knobs set the stem gains in the deck instead, and the
//! band gains ramp to unity, which leaves the isolator a flat allpass. It keeps running
//! so the stem path has the EQ path's phase: with the knobs centered the changeover is
//! then inaudible (the stems sum to the mix), where a dry bypass would switch between the
//! allpass's phase and none.
//!
//! Under keylock the deck's stem gains act before its time-stretcher, so they are heard
//! the stretcher's latency later (about 0.1 s): a stems changeover delays the bands'
//! change by that much to stay in step, and in stem mode the knobs themselves respond
//! with that latency. The filter is
//! bypassed inside its center deadzone; engaging or releasing it crossfades between dry and
//! filtered signal across one block.
//!
//! Each channel runs three effect slots in order after the filter. A slot that is off (and
//! done fading) is skipped entirely, so the channel is then bit-identical to one without
//! effects. An enabled slot mixes `out = x + (wet − x) × amount`; the Filter kind is always
//! fully wet and uses the amount as its cutoff. Switching a slot crossfades over one block.
//!
//! The curve functions are public so the UI can show the same values the engine applies.

use std::f32::consts::FRAC_PI_2;

use crate::dsp::filter::Filter;
use crate::dsp::fx::Effect;
use crate::dsp::isolator::Isolator;
use crate::dsp::smooth::Ramp;
use crate::{
    ChannelCommand, ChannelSnapshot, DeckId, EffectKind, MasterCommand, Meters, XfaderAssign,
};

const CHANNELS: usize = DeckId::ALL.len();

/// Trim knob position to linear gain: −12 dB at 0, unity at 0.5, +12 dB at 1.
pub fn trim_gain(pos: f32) -> f32 {
    let db = (pos.clamp(0.0, 1.0) - 0.5) * 24.0;
    10.0_f32.powf(db / 20.0)
}

/// EQ band knob position to linear gain: a true kill (0.0) at 0, unity at 0.5 (squared law
/// in between), up to +6 dB at 1.
pub fn eq_gain(pos: f32) -> f32 {
    let pos = pos.clamp(0.0, 1.0);
    if pos <= 0.5 {
        let x = pos / 0.5;
        x * x
    } else {
        let db = (pos - 0.5) / 0.5 * 6.0;
        10.0_f32.powf(db / 20.0)
    }
}

/// Channel fader position to linear gain (squared law).
pub fn fader_gain(pos: f32) -> f32 {
    let pos = pos.clamp(0.0, 1.0);
    pos * pos
}

/// Crossfader position to `(a, b)` side gains, constant power: 0 is full A, 1 full B.
pub fn crossfader_gains(pos: f32) -> (f32, f32) {
    let angle = pos.clamp(0.0, 1.0) * FRAC_PI_2;
    // `FRAC_PI_2` rounds up in f32, so its cosine is a tiny negative: snap it to zero.
    (angle.cos().max(0.0), angle.sin())
}

/// Master and headphone level position to linear gain (squared law).
pub fn level_gain(pos: f32) -> f32 {
    let pos = pos.clamp(0.0, 1.0);
    pos * pos
}

fn side_gain(assign: XfaderAssign, crossfader: f32) -> f32 {
    let (a, b) = crossfader_gains(crossfader);
    match assign {
        XfaderAssign::A => a,
        XfaderAssign::Thru => 1.0,
        XfaderAssign::B => b,
    }
}

/// A channel's crossfader gain; `None` is a disabled crossfader, which passes everything.
fn strip_gain(assign: XfaderAssign, crossfader: Option<f32>) -> f32 {
    crossfader.map_or(1.0, |position| side_gain(assign, position))
}

const FX_SLOTS: usize = 3;
const DEFAULT_FX_AMOUNT: f32 = 0.5;
/// The kinds the slots start with.
const DEFAULT_FX_KINDS: [EffectKind; FX_SLOTS] =
    [EffectKind::Echo, EffectKind::Reverb, EffectKind::Flanger];

/// A slot's output for dry input `x` and its effect's `wet` output, with the slot `on`
/// (0 to 1, ramped) and the channel's effect `amount` (0 to 1). Echo and reverb follow a
/// DJ mixer's dry/wet curve: the dry signal stays at unity up to mid amount while the wet
/// rises, then fades out as the wet holds at unity. Flanger crossfades by the amount;
/// Filter's amount is its cutoff, so only `on` mixes it.
#[inline]
fn fx_mix(kind: EffectKind, x: f32, wet: f32, on: f32, amount: f32) -> f32 {
    let full = match kind {
        EffectKind::Echo | EffectKind::Reverb => {
            let dry_gain = (2.0 * (1.0 - amount)).min(1.0);
            let wet_gain = (2.0 * amount).min(1.0);
            x * dry_gain + wet * wet_gain
        }
        EffectKind::Flanger => x + (wet - x) * amount,
        EffectKind::Filter => wet,
    };
    x + (full - x) * on
}

#[derive(Debug)]
struct FxSlot {
    effect: Effect,
    kind: EffectKind,
    /// 1.0 when the slot is on; ramped so toggling does not click.
    on: Ramp,
    /// The amount last given to the effect (only the Filter kind uses it); NaN when stale.
    applied_amount: f32,
}

impl FxSlot {
    fn new(kind: EffectKind, sample_rate: u32) -> Self {
        let mut effect = Effect::new(sample_rate);
        effect.set_kind(kind);
        Self {
            effect,
            kind,
            on: Ramp::new(0.0),
            applied_amount: f32::NAN,
        }
    }

    /// Whether the slot affects the signal in the coming block.
    fn active(&self) -> bool {
        self.on.current != 0.0 || self.on.target != 0.0
    }

    fn set_kind(&mut self, kind: EffectKind) {
        self.kind = kind;
        self.effect.set_kind(kind);
        self.applied_amount = f32::NAN;
    }

    fn set_enabled(&mut self, enabled: bool) {
        if enabled && !self.active() {
            // Off for a while: do not replay what was left in the buffers.
            self.effect.clear_active();
            self.applied_amount = f32::NAN;
        }
        self.on.target = if enabled { 1.0 } else { 0.0 };
    }
}

#[derive(Debug)]
struct ChannelStrip {
    trim: Ramp,
    /// Low, mid and high EQ knob positions.
    eq_pos: [f32; 3],
    /// The knobs control the deck's stems: the band gains are unity.
    stems_active: bool,
    /// What `stems_active` becomes once `stems_countdown` output frames have played: the
    /// deck's stem gains reach the output that late under keylock.
    stems_wanted: bool,
    stems_countdown: usize,
    /// Low, mid and high band gains.
    eq_gain: [Ramp; 3],
    isolator: Isolator,
    filter: Filter,
    fader: Ramp,
    /// 1.0 when PFL is on, 0.0 otherwise; ramped so toggling does not click.
    pfl: Ramp,
    assign: XfaderAssign,
    /// Crossfader gain for this channel's side.
    xfade: Ramp,
    fx: [FxSlot; FX_SLOTS],
    fx_amount: Ramp,
    /// One beat in output frames, 0 without a grid.
    beat_frames: f64,
}

impl ChannelStrip {
    fn new(assign: XfaderAssign, crossfader: f32, sample_rate: u32) -> Self {
        Self {
            trim: Ramp::new(trim_gain(0.5)),
            eq_pos: [0.5; 3],
            stems_active: false,
            stems_wanted: false,
            stems_countdown: 0,
            eq_gain: [Ramp::new(eq_gain(0.5)); 3],
            isolator: Isolator::new(sample_rate),
            filter: Filter::default(),
            fader: Ramp::new(fader_gain(1.0)),
            pfl: Ramp::new(0.0),
            assign,
            xfade: Ramp::new(strip_gain(assign, Some(crossfader))),
            fx: DEFAULT_FX_KINDS.map(|kind| FxSlot::new(kind, sample_rate)),
            fx_amount: Ramp::new(DEFAULT_FX_AMOUNT),
            beat_frames: 0.0,
        }
    }

    fn set_eq(&mut self, band: usize, pos: f32) {
        self.eq_pos[band] = pos.clamp(0.0, 1.0);
        self.retarget_eq();
    }

    fn retarget_eq(&mut self) {
        for (gain, pos) in self.eq_gain.iter_mut().zip(self.eq_pos) {
            gain.target = if self.stems_active { 1.0 } else { eq_gain(pos) };
        }
    }
}

/// Mixer state and scratch buffers. Owned by the renderer.
#[derive(Debug)]
pub(crate) struct Mixer {
    channels: [ChannelStrip; CHANNELS],
    /// Per-channel interleaved stereo input, preallocated to the largest block.
    inputs: [Box<[f32]>; CHANNELS],
    sample_rate: u32,
    crossfader: f32,
    crossfader_enabled: bool,
    master_level: Ramp,
    headphone_level: Ramp,
    headphone_mix: Ramp,
}

impl Mixer {
    pub(crate) fn new(sample_rate: u32, max_block_frames: usize) -> Self {
        const CROSSFADER: f32 = 0.5;
        let channels = DeckId::ALL.map(|id| {
            // Decks 1 and 3 sit on the A side, decks 2 and 4 on the B side.
            let assign = if id.index().is_multiple_of(2) {
                XfaderAssign::A
            } else {
                XfaderAssign::B
            };
            ChannelStrip::new(assign, CROSSFADER, sample_rate)
        });
        Self {
            channels,
            sample_rate,
            inputs: std::array::from_fn(|_| vec![0.0; max_block_frames * 2].into_boxed_slice()),
            crossfader: CROSSFADER,
            crossfader_enabled: true,
            master_level: Ramp::new(level_gain(0.8)),
            headphone_level: Ramp::new(level_gain(0.8)),
            headphone_mix: Ramp::new(0.0),
        }
    }

    // Non-finite positions would poison the buses (and later the filter state): drop them.
    pub(crate) fn apply_channel(&mut self, id: DeckId, cmd: ChannelCommand) {
        let crossfader = self.crossfader_enabled.then_some(self.crossfader);
        let strip = &mut self.channels[id.index()];
        match cmd {
            ChannelCommand::Trim(v) if v.is_finite() => strip.trim.target = trim_gain(v),
            ChannelCommand::EqLow(v) if v.is_finite() => strip.set_eq(0, v),
            ChannelCommand::EqMid(v) if v.is_finite() => strip.set_eq(1, v),
            ChannelCommand::EqHigh(v) if v.is_finite() => strip.set_eq(2, v),
            ChannelCommand::Filter(v) if v.is_finite() => strip.filter.set_position(v),
            ChannelCommand::Fader(v) if v.is_finite() => strip.fader.target = fader_gain(v),
            ChannelCommand::Pfl(on) => strip.pfl.target = if on { 1.0 } else { 0.0 },
            ChannelCommand::FxEnabled(slot, enabled) => {
                if let Some(slot) = strip.fx.get_mut(slot) {
                    slot.set_enabled(enabled);
                }
            }
            ChannelCommand::FxAmount(v) if v.is_finite() => {
                strip.fx_amount.target = v.clamp(0.0, 1.0);
            }
            ChannelCommand::XfaderAssign(assign) => {
                strip.assign = assign;
                strip.xfade.target = strip_gain(assign, crossfader);
            }
            ChannelCommand::Trim(_)
            | ChannelCommand::EqLow(_)
            | ChannelCommand::EqMid(_)
            | ChannelCommand::EqHigh(_)
            | ChannelCommand::Filter(_)
            | ChannelCommand::Fader(_)
            | ChannelCommand::FxAmount(_) => {}
        }
    }

    pub(crate) fn apply_master(&mut self, cmd: MasterCommand) {
        match cmd {
            MasterCommand::Crossfader(v) if v.is_finite() => {
                self.crossfader = v.clamp(0.0, 1.0);
                self.retarget_crossfader();
            }
            MasterCommand::CrossfaderEnabled(enabled) => {
                self.crossfader_enabled = enabled;
                self.retarget_crossfader();
            }
            MasterCommand::MasterLevel(v) if v.is_finite() => {
                self.master_level.target = level_gain(v);
            }
            MasterCommand::HeadphoneLevel(v) if v.is_finite() => {
                self.headphone_level.target = level_gain(v);
            }
            MasterCommand::HeadphoneMix(v) if v.is_finite() => {
                self.headphone_mix.target = v.clamp(0.0, 1.0);
            }
            MasterCommand::FxSlot(slot, kind) if slot < FX_SLOTS => {
                for strip in &mut self.channels {
                    strip.fx[slot].set_kind(kind);
                }
            }
            // The renderer applies it: the decks play the stems.
            MasterCommand::EqMode(_)
            | MasterCommand::FxSlot(..)
            | MasterCommand::Crossfader(_)
            | MasterCommand::MasterLevel(_)
            | MasterCommand::HeadphoneLevel(_)
            | MasterCommand::HeadphoneMix(_) => {}
        }
    }

    /// Sets the length of a beat of `deck`'s grid in output frames (0 without a grid), for
    /// the tempo-synced effects.
    pub(crate) fn set_beat_frames(&mut self, deck: DeckId, frames: f64) {
        self.channels[deck.index()].beat_frames = frames;
    }

    /// The channel's EQ knob positions, `[low, mid, high]`.
    pub(crate) fn eq_positions(&self, deck: DeckId) -> [f32; 3] {
        self.channels[deck.index()].eq_pos
    }

    /// Before a block of `frames`: bypasses the channel's EQ bands (unity gains, ramped
    /// over a block) while its deck's stems take the knobs. A change waits `delay` output
    /// frames, the time the deck's own change takes to be heard (its stretcher's latency
    /// under keylock), to the nearest block; a smaller `delay` later shortens the wait.
    pub(crate) fn set_stems_active(
        &mut self,
        deck: DeckId,
        active: bool,
        delay: usize,
        frames: usize,
    ) {
        let strip = &mut self.channels[deck.index()];
        if strip.stems_wanted != active {
            strip.stems_wanted = active;
            strip.stems_countdown = delay;
        } else {
            strip.stems_countdown = strip.stems_countdown.min(delay);
        }
        if strip.stems_active == strip.stems_wanted {
            return;
        }
        if strip.stems_countdown <= frames / 2 {
            strip.stems_countdown = 0;
            strip.stems_active = strip.stems_wanted;
            strip.retarget_eq();
        } else {
            strip.stems_countdown = strip.stems_countdown.saturating_sub(frames);
        }
    }

    /// The channel's state for display.
    pub(crate) fn channel_snapshot(&self, id: DeckId) -> ChannelSnapshot {
        let strip = &self.channels[id.index()];
        ChannelSnapshot {
            fx_enabled: strip.fx.each_ref().map(|slot| slot.on.target != 0.0),
            fx_amount: strip.fx_amount.target,
        }
    }

    fn retarget_crossfader(&mut self) {
        let position = self.crossfader_enabled.then_some(self.crossfader);
        for strip in &mut self.channels {
            strip.xfade.target = strip_gain(strip.assign, position);
        }
    }

    /// Zeroes the first `samples` samples of each channel input and returns them, in deck
    /// order, for the decks to render into.
    pub(crate) fn inputs(&mut self, samples: usize) -> [&mut [f32]; CHANNELS] {
        self.inputs.each_mut().map(|input| {
            let input = &mut input[..samples];
            input.fill(0.0);
            input
        })
    }

    /// Mixes the channel inputs filled since [`inputs`](Self::inputs) into the interleaved
    /// stereo `master` and `headphones` buffers (overwritten), adds the interleaved stereo
    /// `sampler` block to the master sum, and returns the block's peaks.
    pub(crate) fn process(
        &mut self,
        master: &mut [f32],
        headphones: &mut [f32],
        sampler: &[f32],
    ) -> Meters {
        let frames = master.len() / 2;
        master.fill(0.0);
        headphones.fill(0.0);
        let master_frames = master.as_chunks_mut::<2>().0;
        let cue_frames = headphones.as_chunks_mut::<2>().0;

        let mut channel_peak = [0.0_f32; CHANNELS];
        for ((strip, input), peak) in self
            .channels
            .iter_mut()
            .zip(&self.inputs)
            .zip(&mut channel_peak)
        {
            let trim = strip.trim.block(frames);
            let [low, mid, high] = strip.eq_gain.each_mut().map(|gain| gain.block(frames));
            let filter_wet = strip.filter.block(frames, self.sample_rate);
            let fader = strip.fader.block(frames);
            let pfl = strip.pfl.block(frames);
            let xfade = strip.xfade.block(frames);
            let beat_frames = strip.beat_frames;
            let fx_active = strip.fx.each_ref().map(FxSlot::active);
            let fx_amount_target = strip.fx_amount.target;
            for (slot, &active) in strip.fx.iter_mut().zip(&fx_active) {
                if active
                    && slot.kind == EffectKind::Filter
                    && slot.applied_amount != fx_amount_target
                {
                    slot.effect.set_amount(fx_amount_target);
                    slot.applied_amount = fx_amount_target;
                }
            }
            let fx_amount = strip.fx_amount.block(frames);
            let fx_on = strip.fx.each_mut().map(|slot| slot.on.block(frames));
            let input = input[..frames * 2].as_chunks::<2>().0;
            for (i, ((x, m), h)) in input
                .iter()
                .zip(master_frames.iter_mut())
                .zip(cue_frames.iter_mut())
                .enumerate()
            {
                let (trim, fader, pfl, xfade) = (trim.at(i), fader.at(i), pfl.at(i), xfade.at(i));
                for ch in 0..2 {
                    let [lo, mi, hi] = strip.isolator.split(ch, x[ch] * trim);
                    let mut pre_fader = lo * low.at(i) + mi * mid.at(i) + hi * high.at(i);
                    if let Some(wet) = &filter_wet {
                        let filtered = strip.filter.process(ch, pre_fader);
                        pre_fader += (filtered - pre_fader) * wet.at(i);
                    }
                    for (slot, (&active, on)) in
                        strip.fx.iter_mut().zip(fx_active.iter().zip(&fx_on))
                    {
                        if !active {
                            continue;
                        }
                        let wet = slot.effect.process(ch, pre_fader, beat_frames);
                        pre_fader = fx_mix(slot.kind, pre_fader, wet, on.at(i), fx_amount.at(i));
                    }
                    h[ch] += pre_fader * pfl;
                    let post_fader = pre_fader * fader;
                    *peak = peak.max(post_fader.abs());
                    m[ch] += post_fader * xfade;
                }
            }
            strip.isolator.flush_denormals();
            strip.filter.flush_denormals();
            for (slot, &active) in strip.fx.iter_mut().zip(&fx_active) {
                if active {
                    slot.effect.flush_denormals();
                }
            }
        }

        let master_level = self.master_level.block(frames);
        let headphone_level = self.headphone_level.block(frames);
        let headphone_mix = self.headphone_mix.block(frames);
        let mut master_peak = [0.0_f32; 2];
        let sampler = sampler.as_chunks::<2>().0;
        for (i, ((m, h), s)) in master_frames
            .iter_mut()
            .zip(cue_frames)
            .zip(sampler)
            .enumerate()
        {
            let (level, hp_level, mix) = (
                master_level.at(i),
                headphone_level.at(i),
                headphone_mix.at(i),
            );
            for ch in 0..2 {
                let pre_clamp = (m[ch] + s[ch]) * level;
                let cue = h[ch];
                h[ch] = ((cue + (pre_clamp - cue) * mix) * hp_level).clamp(-1.0, 1.0);
                m[ch] = pre_clamp.clamp(-1.0, 1.0);
                master_peak[ch] = master_peak[ch].max(m[ch].abs());
            }
        }

        Meters {
            channel_peak,
            master_peak,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn echo_and_reverb_keep_the_dry_signal_to_mid_amount() {
        for kind in [EffectKind::Echo, EffectKind::Reverb] {
            // Dry at unity and wet at unity at mid amount.
            assert_eq!(fx_mix(kind, 1.0, 0.0, 1.0, 0.5), 1.0, "{kind:?}");
            assert_eq!(fx_mix(kind, 0.0, 1.0, 1.0, 0.5), 1.0, "{kind:?}");
            // Full amount: wet only.
            assert_eq!(fx_mix(kind, 1.0, 0.0, 1.0, 1.0), 0.0, "{kind:?}");
            assert_eq!(fx_mix(kind, 0.0, 1.0, 1.0, 1.0), 1.0, "{kind:?}");
            // A quarter: dry at unity, wet at half.
            assert_eq!(fx_mix(kind, 1.0, 0.0, 1.0, 0.25), 1.0, "{kind:?}");
            assert_eq!(fx_mix(kind, 0.0, 1.0, 1.0, 0.25), 0.5, "{kind:?}");
            // A slot fading out goes back to the dry input.
            assert_eq!(fx_mix(kind, 0.3, 0.9, 0.0, 1.0), 0.3, "{kind:?}");
            assert!(
                (fx_mix(kind, 1.0, 0.0, 0.5, 1.0) - 0.5).abs() < 1e-6,
                "{kind:?}"
            );
        }
    }

    #[test]
    fn flanger_and_filter_crossfade() {
        assert_eq!(fx_mix(EffectKind::Flanger, 1.0, 0.0, 1.0, 0.5), 0.5);
        assert_eq!(fx_mix(EffectKind::Flanger, 0.0, 1.0, 1.0, 0.25), 0.25);
        // Filter's amount is its cutoff: on mixes it alone.
        assert_eq!(fx_mix(EffectKind::Filter, 1.0, 0.0, 1.0, 0.5), 0.0);
        assert_eq!(fx_mix(EffectKind::Filter, 1.0, 0.0, 0.5, 0.1), 0.5);
    }
}
