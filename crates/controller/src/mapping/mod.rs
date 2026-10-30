//! Controller mappings: the TOML file format, resolving MIDI messages into actions, and
//! soft takeover for absolute controls.

mod file;
mod resolve;
mod takeover;

pub use file::{
    ControllerInfo, InitAction, InputBinding, Mapping, MappingError, MidiSource, OutputBinding,
    OutputMidi, ValueKind, parse_mapping,
};
pub use resolve::{Resolved, Resolver};
pub use takeover::{TOLERANCE, Takeover};
