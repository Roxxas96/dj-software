//! Maps the engine's stereo master and headphone buses onto a multi-channel device buffer.

use engine::EngineRenderer;
use thiserror::Error;

/// Why an [`OutputRouting`] does not fit a device.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RoutingError {
    #[error(
        "routing uses output channel index {index}, but the device has only {device_channels} {}",
        plural_channels(*device_channels)
    )]
    ChannelOutOfRange {
        index: usize,
        device_channels: usize,
    },
    #[error("output channel index {index} is routed more than once")]
    Overlap { index: usize },
}

fn plural_channels(count: usize) -> &'static str {
    if count == 1 { "channel" } else { "channels" }
}

/// Which device channels (0-based) carry the master and headphone buses, left then right.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutputRouting {
    pub master: [usize; 2],
    pub headphones: Option<[usize; 2]>,
}

impl OutputRouting {
    /// Pioneer DDJ-SB3: master on outputs 1/2, headphones on outputs 3/4.
    pub const SB3: Self = Self {
        master: [0, 1],
        headphones: Some([2, 3]),
    };

    /// A plain stereo device: master only.
    pub const STEREO: Self = Self {
        master: [0, 1],
        headphones: None,
    };

    /// Every routed channel index, master first.
    fn channels(&self) -> impl Iterator<Item = usize> {
        self.master
            .into_iter()
            .chain(self.headphones.into_iter().flatten())
    }

    /// Number of device channels this routing needs (highest index + 1).
    pub fn required_channels(&self) -> usize {
        self.channels().max().map_or(0, |index| index + 1)
    }

    /// Checks that every routed channel exists on a device with `device_channels` channels
    /// and that no channel carries two signals.
    pub fn validate(&self, device_channels: usize) -> Result<(), RoutingError> {
        if let Some(index) = self.channels().find(|&index| index >= device_channels) {
            return Err(RoutingError::ChannelOutOfRange {
                index,
                device_channels,
            });
        }
        for (position, index) in self.channels().enumerate() {
            if self
                .channels()
                .take(position)
                .any(|earlier| earlier == index)
            {
                return Err(RoutingError::Overlap { index });
            }
        }
        Ok(())
    }
}

/// Preallocated stereo buffers the renderer draws into before routing.
pub struct Scratch {
    master: Vec<f32>,
    headphones: Vec<f32>,
}

impl Scratch {
    /// Allocates room for `max_block_frames` stereo frames per bus.
    pub fn new(max_block_frames: usize) -> Self {
        Self {
            master: vec![0.0; 2 * max_block_frames],
            headphones: vec![0.0; 2 * max_block_frames],
        }
    }
}

/// Renders `out.len() / device_channels` frames into the interleaved device buffer `out`,
/// in chunks of at most the scratch's `max_block_frames`.
///
/// Master and headphones go to the channels `routing` names; every other channel, and any
/// trailing partial frame, is zeroed. Never allocates, locks or panics: routed channels
/// beyond `device_channels` are skipped (validate the routing up front to catch that).
pub fn fill_device_buffer(
    renderer: &mut EngineRenderer,
    scratch: &mut Scratch,
    out: &mut [f32],
    device_channels: usize,
    routing: &OutputRouting,
) {
    let chunk_frames = scratch.master.len().min(scratch.headphones.len()) / 2;
    if device_channels == 0 || chunk_frames == 0 {
        out.fill(0.0);
        return;
    }
    let whole = out.len() / device_channels * device_channels;
    let (out, partial) = out.split_at_mut(whole);
    partial.fill(0.0);

    for chunk in out.chunks_mut(chunk_frames * device_channels) {
        let samples = chunk.len() / device_channels * 2;
        let master = &mut scratch.master[..samples];
        let headphones = &mut scratch.headphones[..samples];
        renderer.render(master, headphones);

        for ((frame, m), h) in chunk
            .chunks_exact_mut(device_channels)
            .zip(master.as_chunks::<2>().0)
            .zip(headphones.as_chunks::<2>().0)
        {
            frame.fill(0.0);
            write_pair(frame, routing.master, m);
            if let Some(channels) = routing.headphones {
                write_pair(frame, channels, h);
            }
        }
    }
}

fn write_pair(frame: &mut [f32], channels: [usize; 2], stereo: &[f32; 2]) {
    for (&channel, &sample) in channels.iter().zip(stereo) {
        if let Some(slot) = frame.get_mut(channel) {
            *slot = sample;
        }
    }
}

/// [`fill_device_buffer`] for devices that take `i16` samples: renders through the
/// preallocated `convert` buffer, `convert.len() / device_channels` frames at a time.
pub fn fill_device_buffer_i16(
    renderer: &mut EngineRenderer,
    scratch: &mut Scratch,
    convert: &mut [f32],
    out: &mut [i16],
    device_channels: usize,
    routing: &OutputRouting,
) {
    let chunk_frames = convert.len().checked_div(device_channels).unwrap_or(0);
    if chunk_frames == 0 {
        out.fill(0);
        return;
    }
    for chunk in out.chunks_mut(chunk_frames * device_channels) {
        let convert = &mut convert[..chunk.len()];
        fill_device_buffer(renderer, scratch, convert, device_channels, routing);
        for (dst, &src) in chunk.iter_mut().zip(convert.iter()) {
            *dst = f32_to_i16(src);
        }
    }
}

/// Full scale maps to ±`i16::MAX`; out-of-range samples clip and NaN becomes silence.
fn f32_to_i16(sample: f32) -> i16 {
    // `as` saturates and maps NaN to 0.
    (sample.clamp(-1.0, 1.0) * f32::from(i16::MAX)).round() as i16
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use engine::{
        ChannelCommand, Command, DeckCommand, DeckId, EngineConfig, EngineController,
        MasterCommand, TrackBuffer, XfaderAssign, engine,
    };

    use super::*;

    const SR: u32 = 44_100;
    const BLOCK: usize = 512;
    const SIGNAL: f32 = 0.5;
    /// Frames after which the engine's always-on EQ isolator has settled to well below
    /// [`SETTLED_EPSILON`] (see the engine's mixer tests).
    const SETTLE_FRAMES: usize = 4096;
    const SETTLED_EPSILON: f32 = 1e-4;
    /// Channel fader position 0.5 is a gain of 0.25, so master and headphones differ.
    const FADER: f32 = 0.5;
    const MASTER_VALUE: f32 = SIGNAL * 0.25;
    /// PFL is pre-fader and the headphone mix defaults to cue only.
    const HEADPHONE_VALUE: f32 = SIGNAL;

    /// Deck 1 plays a constant track, PFL on, unity master and headphone levels, the
    /// channel ignoring the crossfader. Renders until the output has settled.
    fn playing_engine() -> (EngineController, EngineRenderer, Scratch) {
        let (mut c, mut r) = engine(EngineConfig {
            sample_rate: SR,
            max_block_frames: BLOCK,
        });
        let id = DeckId::new(0).unwrap();
        let track = Arc::new(TrackBuffer::new(SR, vec![SIGNAL; 200_000 * 2]).unwrap());
        for cmd in [
            Command::Channel(id, ChannelCommand::XfaderAssign(XfaderAssign::Thru)),
            Command::Channel(id, ChannelCommand::Fader(FADER)),
            Command::Channel(id, ChannelCommand::Pfl(true)),
            Command::Master(MasterCommand::MasterLevel(1.0)),
            Command::Master(MasterCommand::HeadphoneLevel(1.0)),
            Command::Deck(id, DeckCommand::Load(track)),
            Command::Deck(id, DeckCommand::Play),
        ] {
            c.send(cmd).unwrap();
        }
        let mut scratch = Scratch::new(BLOCK);
        let mut settle = vec![0.0; SETTLE_FRAMES * 2];
        fill_device_buffer(&mut r, &mut scratch, &mut settle, 2, &OutputRouting::STEREO);
        (c, r, scratch)
    }

    fn assert_channel_near(out: &[f32], channels: usize, channel: usize, expected: f32) {
        for (frame, samples) in out.chunks_exact(channels).enumerate() {
            let s = samples[channel];
            assert!(
                (s - expected).abs() <= SETTLED_EPSILON,
                "frame {frame} channel {channel}: {s} != {expected}"
            );
        }
    }

    #[test]
    fn routing_rejects_channels_beyond_device() {
        let err = OutputRouting::SB3.validate(2).unwrap_err();
        assert_eq!(
            err,
            RoutingError::ChannelOutOfRange {
                index: 2,
                device_channels: 2
            }
        );
        let message = err.to_string();
        assert!(message.contains("2 channels"), "{message}");
        assert!(message.contains("index 2"), "{message}");
        assert_eq!(OutputRouting::SB3.validate(4), Ok(()));
        assert_eq!(OutputRouting::STEREO.validate(2), Ok(()));
        assert_eq!(OutputRouting::SB3.required_channels(), 4);
        assert_eq!(OutputRouting::STEREO.required_channels(), 2);
    }

    #[test]
    fn f32_to_i16_scales_clips_and_silences_nan() {
        assert_eq!(f32_to_i16(0.0), 0);
        assert_eq!(f32_to_i16(1.0), i16::MAX);
        assert_eq!(f32_to_i16(-1.0), -i16::MAX);
        assert_eq!(f32_to_i16(2.0), i16::MAX);
        assert_eq!(f32_to_i16(-2.0), -i16::MAX);
        assert_eq!(f32_to_i16(f32::NAN), 0);
    }

    #[test]
    fn routing_rejects_overlap() {
        let routing = OutputRouting {
            master: [0, 1],
            headphones: Some([1, 2]),
        };
        assert_eq!(routing.validate(4), Err(RoutingError::Overlap { index: 1 }));
    }

    #[test]
    fn fill_writes_master_and_headphones_to_their_channels() {
        let (_c, mut r, mut scratch) = playing_engine();
        let mut out = vec![f32::NAN; 2000 * 4];
        fill_device_buffer(&mut r, &mut scratch, &mut out, 4, &OutputRouting::SB3);
        assert_channel_near(&out, 4, 0, MASTER_VALUE);
        assert_channel_near(&out, 4, 1, MASTER_VALUE);
        assert_channel_near(&out, 4, 2, HEADPHONE_VALUE);
        assert_channel_near(&out, 4, 3, HEADPHONE_VALUE);
    }

    #[test]
    fn fill_zeroes_unrouted_channels() {
        let (_c, mut r, mut scratch) = playing_engine();
        let mut out = vec![f32::NAN; 2000 * 6];
        fill_device_buffer(&mut r, &mut scratch, &mut out, 6, &OutputRouting::SB3);
        assert_channel_near(&out, 6, 0, MASTER_VALUE);
        assert_channel_near(&out, 6, 2, HEADPHONE_VALUE);
        for samples in out.as_chunks::<6>().0 {
            assert_eq!(samples[4], 0.0);
            assert_eq!(samples[5], 0.0);
        }
    }
}
