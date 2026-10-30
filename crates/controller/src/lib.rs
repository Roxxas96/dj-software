//! MIDI controller support: message parsing and port access.

mod midi;
mod ports;

pub use midi::{MidiMessage, TimedMidi};
pub use ports::{
    InputPort, MidiOut, OutputPort, PortError, input_port_names, open_input, open_output,
    output_port_names,
};
