//! MIDI controller support: message parsing, port access, guided capture, mappings and
//! the runtime that applies them.

pub mod builtin;
pub mod capture;
pub mod clock;
pub mod feedback;
pub mod mapping;
mod midi;
mod ports;
mod runtime;

pub use feedback::Feedback;
pub use midi::{MidiMessage, TimedMidi};
pub use ports::{
    InputPort, MidiOut, OutputPort, PortError, input_port_names, open_input, open_output,
    output_port_names,
};
pub use runtime::{BatchStats, Connection, PresenceChange, Runtime};
