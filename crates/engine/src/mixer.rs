//! Four-channel DJ mixer: channel strips, crossfader, master and headphone buses.
//!
//! Signal flow per channel: deck → trim → EQ → filter → PFL tap → fader → crossfader →
//! master sum → master level → clamp to `[-1, 1]`. Headphones get
//! `lerp(cue_sum, master_pre_clamp, mix) * headphone_level`, clamped, where `cue_sum` is the
//! sum of the PFL taps. Every gain change is ramped linearly across one block.
//!
//! With all three EQ knobs at 0.5 the isolator is bypassed (bit-exact dry signal); the same
//! goes for the filter inside its center deadzone. Engaging or releasing either crossfades
//! between dry and processed signal across one block.
//!
//! The curve functions are public so the UI can show the same values the engine applies.

use std::f32::consts::FRAC_PI_2;

use crate::dsp::filter::Filter;
use crate::dsp::isolator::Isolator;
use crate::dsp::smooth::Ramp;
use crate::{ChannelCommand, DeckId, MasterCommand, Meters, XfaderAssign};

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

#[derive(Debug)]
struct ChannelStrip {
    trim: Ramp,
    /// Low, mid and high knob positions.
    eq: [f32; 3],
    /// Low, mid and high band gains.
    eq_gain: [Ramp; 3],
    /// 1.0 when the isolator is engaged, 0.0 when every knob is neutral.
    eq_wet: Ramp,
    isolator: Isolator,
    filter: Filter,
    fader: Ramp,
    /// 1.0 when PFL is on, 0.0 otherwise; ramped so toggling does not click.
    pfl: Ramp,
    assign: XfaderAssign,
    /// Crossfader gain for this channel's side.
    xfade: Ramp,
}

impl ChannelStrip {
    fn new(assign: XfaderAssign, crossfader: f32, sample_rate: u32) -> Self {
        Self {
            trim: Ramp::new(trim_gain(0.5)),
            eq: [0.5; 3],
            eq_gain: [Ramp::new(eq_gain(0.5)); 3],
            eq_wet: Ramp::new(0.0),
            isolator: Isolator::new(sample_rate),
            filter: Filter::default(),
            fader: Ramp::new(fader_gain(1.0)),
            pfl: Ramp::new(0.0),
            assign,
            xfade: Ramp::new(side_gain(assign, crossfader)),
        }
    }

    fn set_eq(&mut self, band: usize, pos: f32) {
        let pos = pos.clamp(0.0, 1.0);
        self.eq[band] = pos;
        self.eq_gain[band].target = eq_gain(pos);
        let neutral = self.eq.iter().all(|&p| p == 0.5);
        self.eq_wet.target = if neutral { 0.0 } else { 1.0 };
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
            master_level: Ramp::new(level_gain(0.8)),
            headphone_level: Ramp::new(level_gain(0.8)),
            headphone_mix: Ramp::new(0.0),
        }
    }

    // Non-finite positions would poison the buses (and later the filter state): drop them.
    pub(crate) fn apply_channel(&mut self, id: DeckId, cmd: ChannelCommand) {
        let crossfader = self.crossfader;
        let strip = &mut self.channels[id.index()];
        match cmd {
            ChannelCommand::Trim(v) if v.is_finite() => strip.trim.target = trim_gain(v),
            ChannelCommand::EqLow(v) if v.is_finite() => strip.set_eq(0, v),
            ChannelCommand::EqMid(v) if v.is_finite() => strip.set_eq(1, v),
            ChannelCommand::EqHigh(v) if v.is_finite() => strip.set_eq(2, v),
            ChannelCommand::Filter(v) if v.is_finite() => strip.filter.set_position(v),
            ChannelCommand::Fader(v) if v.is_finite() => strip.fader.target = fader_gain(v),
            ChannelCommand::Pfl(on) => strip.pfl.target = if on { 1.0 } else { 0.0 },
            ChannelCommand::XfaderAssign(assign) => {
                strip.assign = assign;
                strip.xfade.target = side_gain(assign, crossfader);
            }
            ChannelCommand::Trim(_)
            | ChannelCommand::EqLow(_)
            | ChannelCommand::EqMid(_)
            | ChannelCommand::EqHigh(_)
            | ChannelCommand::Filter(_)
            | ChannelCommand::Fader(_) => {}
        }
    }

    pub(crate) fn apply_master(&mut self, cmd: MasterCommand) {
        match cmd {
            MasterCommand::Crossfader(v) if v.is_finite() => {
                self.crossfader = v.clamp(0.0, 1.0);
                for strip in &mut self.channels {
                    strip.xfade.target = side_gain(strip.assign, self.crossfader);
                }
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
            MasterCommand::Crossfader(_)
            | MasterCommand::MasterLevel(_)
            | MasterCommand::HeadphoneLevel(_)
            | MasterCommand::HeadphoneMix(_) => {}
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
    /// stereo `master` and `headphones` buffers (overwritten) and returns the block's peaks.
    pub(crate) fn process(&mut self, master: &mut [f32], headphones: &mut [f32]) -> Meters {
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
            let eq_wet = strip.eq_wet.block(frames);
            let eq_engaged = !eq_wet.is_zero();
            let filter_wet = strip.filter.block(frames, self.sample_rate);
            let fader = strip.fader.block(frames);
            let pfl = strip.pfl.block(frames);
            let xfade = strip.xfade.block(frames);
            let input = input[..frames * 2].as_chunks::<2>().0;
            for (i, ((x, m), h)) in input
                .iter()
                .zip(master_frames.iter_mut())
                .zip(cue_frames.iter_mut())
                .enumerate()
            {
                let (trim, fader, pfl, xfade) = (trim.at(i), fader.at(i), pfl.at(i), xfade.at(i));
                for ch in 0..2 {
                    let mut pre_fader = x[ch] * trim;
                    // The isolator always runs so its state is warm when it engages.
                    let bands = strip.isolator.split(ch, pre_fader);
                    if eq_engaged {
                        let eq =
                            bands[0] * low.at(i) + bands[1] * mid.at(i) + bands[2] * high.at(i);
                        pre_fader += (eq - pre_fader) * eq_wet.at(i);
                    }
                    if let Some(wet) = &filter_wet {
                        let filtered = strip.filter.process(ch, pre_fader);
                        pre_fader += (filtered - pre_fader) * wet.at(i);
                    }
                    h[ch] += pre_fader * pfl;
                    let post_fader = pre_fader * fader;
                    *peak = peak.max(post_fader.abs());
                    m[ch] += post_fader * xfade;
                }
            }
            strip.isolator.flush_denormals();
            strip.filter.flush_denormals();
        }

        let master_level = self.master_level.block(frames);
        let headphone_level = self.headphone_level.block(frames);
        let headphone_mix = self.headphone_mix.block(frames);
        let mut master_peak = [0.0_f32; 2];
        for (i, (m, h)) in master_frames.iter_mut().zip(cue_frames).enumerate() {
            let (level, hp_level, mix) = (
                master_level.at(i),
                headphone_level.at(i),
                headphone_mix.at(i),
            );
            for ch in 0..2 {
                let pre_clamp = m[ch] * level;
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
