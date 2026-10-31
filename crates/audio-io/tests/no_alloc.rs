//! The device callback path must not allocate. The global allocator is per test binary,
//! hence this separate file.

use std::sync::Arc;

use assert_no_alloc::{AllocDisabler, assert_no_alloc};
use audio_io::{
    ERROR_QUEUE_CAPACITY, OutputRouting, Scratch, fill_device_buffer, fill_device_buffer_i16,
    stream_errors,
};
use engine::{
    ChannelCommand, Command, DeckCommand, DeckId, EngineConfig, EngineController, EngineRenderer,
    MasterCommand, TrackBuffer, XfaderAssign, engine,
};

#[global_allocator]
static A: AllocDisabler = AllocDisabler;

const SR: u32 = 44_100;
const MAX_BLOCK_FRAMES: usize = 1024;
const DEVICE_FRAMES: usize = 4096;
const CHANNELS: usize = 4;
const SIGNAL: f32 = 0.5;
/// Frames after which the engine's always-on EQ isolator has settled.
const SETTLE_FRAMES: usize = 4096;
const SETTLED_EPSILON: f32 = 1e-4;

/// Deck 1 plays a constant `SIGNAL` track at unity gain to master and (via PFL) headphones.
fn playing_engine() -> (EngineController, EngineRenderer) {
    let (mut c, r) = engine(EngineConfig {
        sample_rate: SR,
        max_block_frames: MAX_BLOCK_FRAMES,
    });
    let id = DeckId::new(0).unwrap();
    let track = Arc::new(TrackBuffer::new(SR, vec![SIGNAL; 100_000 * 2]).unwrap());
    for cmd in [
        Command::Channel(id, ChannelCommand::XfaderAssign(XfaderAssign::Thru)),
        Command::Channel(id, ChannelCommand::Pfl(true)),
        Command::Master(MasterCommand::MasterLevel(1.0)),
        Command::Master(MasterCommand::HeadphoneLevel(1.0)),
        Command::Deck(id, DeckCommand::Load(track)),
        Command::Deck(id, DeckCommand::Play),
    ] {
        c.send(cmd).unwrap();
    }
    (c, r)
}

#[test]
fn fill_device_buffer_chunks_large_requests() {
    let (_c, mut r) = playing_engine();
    let mut scratch = Scratch::new(MAX_BLOCK_FRAMES);
    let mut settle = vec![0.0; SETTLE_FRAMES * CHANNELS];
    let mut out = vec![0.0; DEVICE_FRAMES * CHANNELS];
    assert_no_alloc(|| {
        fill_device_buffer(
            &mut r,
            &mut scratch,
            &mut settle,
            CHANNELS,
            &OutputRouting::SB3,
        );
        fill_device_buffer(
            &mut r,
            &mut scratch,
            &mut out,
            CHANNELS,
            &OutputRouting::SB3,
        );
    });
    for (frame, samples) in out.as_chunks::<CHANNELS>().0.iter().enumerate() {
        for (channel, &s) in samples.iter().enumerate() {
            assert!(
                (s - SIGNAL).abs() <= SETTLED_EPSILON,
                "frame {frame} channel {channel}: {s} != {SIGNAL}"
            );
        }
    }
}

#[test]
fn fill_device_buffer_i16_converts_without_allocating() {
    let (_c, mut r) = playing_engine();
    let mut scratch = Scratch::new(MAX_BLOCK_FRAMES);
    let mut convert = vec![0.0; MAX_BLOCK_FRAMES * CHANNELS];
    let mut settle = vec![0; SETTLE_FRAMES * CHANNELS];
    let mut out = vec![0; DEVICE_FRAMES * CHANNELS];
    assert_no_alloc(|| {
        for buffer in [&mut settle, &mut out] {
            fill_device_buffer_i16(
                &mut r,
                &mut scratch,
                &mut convert,
                buffer,
                CHANNELS,
                &OutputRouting::SB3,
            );
        }
    });
    let expected = (SIGNAL * f32::from(i16::MAX)).round() as i16;
    for (frame, samples) in out.as_chunks::<CHANNELS>().0.iter().enumerate() {
        for (channel, &s) in samples.iter().enumerate() {
            assert!(
                (i32::from(s) - i32::from(expected)).abs() <= 4,
                "frame {frame} channel {channel}: {s} != {expected}"
            );
        }
    }
}

#[test]
fn error_sink_reports_without_allocating() {
    let (mut sink, mut errors) = stream_errors();
    let with_message = cpal::Error::with_message(cpal::ErrorKind::DeviceNotAvailable, "gone");
    let plain: Vec<cpal::Error> = (0..ERROR_QUEUE_CAPACITY + 2)
        .map(|_| cpal::Error::new(cpal::ErrorKind::DeviceBusy))
        .collect();
    let mut plain = plain.into_iter();
    let first = plain.next().unwrap();
    assert_no_alloc(|| {
        // The owned error, message included, is moved into the queue.
        sink.report(with_message);
        sink.report(first);
    });
    // Overflowing the queue drops the extra errors (these own no heap memory).
    let rest: Vec<cpal::Error> = plain.collect();
    let mut rest = rest.into_iter();
    assert_no_alloc(|| {
        for error in rest.by_ref() {
            sink.report(error);
        }
    });
    drop(rest);
    assert_eq!(errors.count(), (ERROR_QUEUE_CAPACITY + 3) as u64);
    let messages = errors.take();
    assert_eq!(messages.len(), ERROR_QUEUE_CAPACITY);
    assert_eq!(messages[0], "gone");
}
