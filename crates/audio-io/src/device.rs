//! Output device discovery and the cpal stream that drives the engine.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{BufferSize, Device, SampleFormat, StreamConfig, SupportedStreamConfigRange};
use engine::{EngineConfig, EngineRenderer};
use thiserror::Error;

use crate::errors::{StreamError, StreamErrors, stream_errors};
use crate::routing::{
    OutputRouting, RoutingError, Scratch, fill_device_buffer, fill_device_buffer_i16,
};

/// Largest block the engine renders per call when driven by an [`OutputStream`].
const MAX_BLOCK_FRAMES: usize = 1024;

/// Why an output device could not be listed, opened or started.
#[derive(Debug, Error)]
pub enum AudioError {
    #[error("no default audio output device")]
    NoDefaultDevice,
    #[error("no audio output device matches \"{query}\" (available: {})", available.join(", "))]
    DeviceNotFound {
        query: String,
        available: Vec<String>,
    },
    #[error("output device \"{device}\" offers no f32 or i16 format at {sample_rate} Hz")]
    NoSupportedFormat { device: String, sample_rate: u32 },
    #[error("routing does not fit output device \"{device}\": {source}")]
    Routing {
        device: String,
        #[source]
        source: RoutingError,
    },
    #[error("audio backend error: {0}")]
    Backend(#[from] cpal::Error),
}

/// An output device as reported by the system.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputDeviceInfo {
    pub name: String,
    /// Most channels any of its output configurations offers.
    pub max_channels: u16,
    pub default_sample_rate: u32,
}

/// Lists the default host's output devices. Devices whose configuration cannot be read
/// (for example, unplugged while listing) are skipped.
pub fn list_output_devices() -> Result<Vec<OutputDeviceInfo>, AudioError> {
    let host = cpal::default_host();
    let mut devices = Vec::new();
    for device in host.output_devices()? {
        let Ok(name) = device_name(&device) else {
            continue;
        };
        let Ok(default) = device.default_output_config() else {
            continue;
        };
        let Ok(configs) = device.supported_output_configs() else {
            continue;
        };
        let max_channels = configs
            .map(|c| c.channels())
            .max()
            .unwrap_or(default.channels());
        devices.push(OutputDeviceInfo {
            name,
            max_channels,
            default_sample_rate: default.sample_rate(),
        });
    }
    Ok(devices)
}

/// An opened output device with a chosen configuration, ready to [`start`](Self::start).
pub struct PendingOutput {
    device: Device,
    name: String,
    config: StreamConfig,
    sample_format: SampleFormat,
    routing: OutputRouting,
}

/// Finds an output device and a configuration that fits `routing`.
///
/// `device_name` matches case-insensitively, an exact name first, then the first name
/// containing it; `None` picks the system default. The configuration runs at the device's
/// default sample rate with `f32` (preferred) or `i16` samples and the fewest channels that
/// cover `routing`.
pub fn open_output(
    device_name: Option<&str>,
    routing: OutputRouting,
) -> Result<PendingOutput, AudioError> {
    let host = cpal::default_host();
    let (device, name) = match device_name {
        None => {
            let device = host
                .default_output_device()
                .ok_or(AudioError::NoDefaultDevice)?;
            let name = self::device_name(&device)?;
            (device, name)
        }
        Some(query) => find_device(&host, query)?,
    };

    let sample_rate = device.default_output_config()?.sample_rate();
    let candidates: Vec<SupportedStreamConfigRange> = device
        .supported_output_configs()?
        .filter(|c| is_usable_format(c.sample_format()) && c.contains_rate(sample_rate))
        .collect();
    let device_channels = candidates
        .iter()
        .map(|c| usize::from(c.channels()))
        .max()
        .ok_or_else(|| AudioError::NoSupportedFormat {
            device: name.clone(),
            sample_rate,
        })?;
    routing
        .validate(device_channels)
        .map_err(|source| AudioError::Routing {
            device: name.clone(),
            source,
        })?;

    let needed = routing.required_channels();
    let chosen = candidates
        .iter()
        .filter(|c| usize::from(c.channels()) >= needed)
        .min_by_key(|c| (c.channels(), c.sample_format() != SampleFormat::F32))
        .expect("validation guarantees a config with enough channels")
        .with_sample_rate(sample_rate);
    let config = StreamConfig {
        channels: chosen.channels(),
        sample_rate,
        buffer_size: BufferSize::Default,
    };
    Ok(PendingOutput {
        device,
        name,
        config,
        sample_format: chosen.sample_format(),
        routing,
    })
}

impl PendingOutput {
    /// The engine configuration to build the renderer passed to [`start`](Self::start) with.
    pub fn engine_config(&self) -> EngineConfig {
        EngineConfig {
            sample_rate: self.config.sample_rate,
            max_block_frames: MAX_BLOCK_FRAMES,
        }
    }

    pub fn device_name(&self) -> &str {
        &self.name
    }

    /// Channel count of the stream that [`start`](Self::start) opens.
    pub fn channels(&self) -> u16 {
        self.config.channels
    }

    /// Builds the stream around `renderer` and starts playback.
    pub fn start(self, mut renderer: EngineRenderer) -> Result<OutputStream, AudioError> {
        let channels = usize::from(self.config.channels);
        let routing = self.routing;
        let mut scratch = Scratch::new(renderer.max_block_frames());
        let (mut sink, errors) = stream_errors();
        // Real-time: bumps a counter and moves cpal's error into a bounded lock-free queue
        // (dropped when full). Nothing is allocated, locked or printed here.
        let on_error = move |error: cpal::Error| sink.report(error);

        let stream = match self.sample_format {
            SampleFormat::I16 => {
                let mut convert = vec![0.0; renderer.max_block_frames() * channels];
                self.device.build_output_stream::<i16, _, _>(
                    self.config,
                    move |out, _| {
                        fill_device_buffer_i16(
                            &mut renderer,
                            &mut scratch,
                            &mut convert,
                            out,
                            channels,
                            &routing,
                        );
                    },
                    on_error,
                    None,
                )?
            }
            _ => self.device.build_output_stream::<f32, _, _>(
                self.config,
                move |out, _| {
                    fill_device_buffer(&mut renderer, &mut scratch, out, channels, &routing);
                },
                on_error,
                None,
            )?,
        };
        stream.play()?;
        Ok(OutputStream {
            _stream: stream,
            errors,
        })
    }
}

/// A playing output stream. Audio stops when it is dropped.
pub struct OutputStream {
    _stream: cpal::Stream,
    errors: StreamErrors,
}

impl OutputStream {
    /// How many errors cpal has reported for this stream so far.
    pub fn errors(&self) -> u64 {
        self.errors.count()
    }

    /// The errors cpal reported since the last call, oldest first, with their kind and
    /// message. At most [`ERROR_QUEUE_CAPACITY`](crate::ERROR_QUEUE_CAPACITY) wait between
    /// calls; later ones are only counted in [`errors`](Self::errors).
    pub fn take_errors(&mut self) -> Vec<StreamError> {
        self.errors.take()
    }
}

fn is_usable_format(format: SampleFormat) -> bool {
    matches!(format, SampleFormat::F32 | SampleFormat::I16)
}

/// The device's name. Uses `description()` rather than `Display`, whose failure would
/// panic inside `to_string`.
fn device_name(device: &Device) -> Result<String, AudioError> {
    Ok(device.description()?.name().to_owned())
}

fn find_device(host: &cpal::Host, query: &str) -> Result<(Device, String), AudioError> {
    let mut named: Vec<(Device, String)> = host
        .output_devices()?
        .filter_map(|device| device_name(&device).ok().map(|name| (device, name)))
        .collect();
    let query_lower = query.to_lowercase();
    let position = named
        .iter()
        .position(|(_, name)| name.to_lowercase() == query_lower)
        .or_else(|| {
            named
                .iter()
                .position(|(_, name)| name.to_lowercase().contains(&query_lower))
        });
    match position {
        Some(index) => Ok(named.swap_remove(index)),
        None => Err(AudioError::DeviceNotFound {
            query: query.to_owned(),
            available: named.into_iter().map(|(_, name)| name).collect(),
        }),
    }
}
