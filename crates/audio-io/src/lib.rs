//! Audio output device access and the real-time callback glue.
//!
//! [`open_output`] picks a device and a configuration for an [`OutputRouting`];
//! [`PendingOutput::start`] then drives an [`engine::EngineRenderer`] from the device's
//! callback, writing the master and headphone buses to their routed channels.

mod device;
mod routing;

pub use device::{
    AudioError, OutputDeviceInfo, OutputStream, PendingOutput, list_output_devices, open_output,
};
pub use routing::{
    OutputRouting, RoutingError, Scratch, fill_device_buffer, fill_device_buffer_i16,
};
