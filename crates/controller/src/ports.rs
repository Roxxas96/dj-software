//! Thin wrappers over midir: port listing, substring matching and connection.
//!
//! The input callback only parses and forwards over a channel; it never touches the engine.

use std::sync::mpsc::Sender;

use midir::{Ignore, MidiInput, MidiInputConnection, MidiOutput, MidiOutputConnection};

use crate::clock;
use crate::midi::{MidiMessage, TimedMidi};

const CLIENT_NAME: &str = "dj-software";

#[derive(Debug, thiserror::Error)]
pub enum PortError {
    #[error("MIDI backend error: {0}")]
    Backend(String),
    #[error("no MIDI {kind} port matching {wanted:?}; available: {}", format_list(.available))]
    NotFound {
        kind: &'static str,
        wanted: String,
        available: Vec<String>,
    },
    #[error("failed to send MIDI message: {0}")]
    Send(String),
}

fn format_list(names: &[String]) -> String {
    if names.is_empty() {
        "none".to_owned()
    } else {
        names.join(", ")
    }
}

fn backend<E: std::fmt::Display>(e: E) -> PortError {
    PortError::Backend(e.to_string())
}

/// Index of the first name containing `wanted`, ignoring case.
pub(crate) fn find_match(names: &[String], wanted: &str) -> Option<usize> {
    let wanted = wanted.to_lowercase();
    names
        .iter()
        .position(|n| n.to_lowercase().contains(&wanted))
}

pub fn input_port_names() -> Result<Vec<String>, PortError> {
    let input = MidiInput::new(CLIENT_NAME).map_err(backend)?;
    input
        .ports()
        .iter()
        .map(|p| input.port_name(p).map_err(backend))
        .collect()
}

pub fn output_port_names() -> Result<Vec<String>, PortError> {
    let output = MidiOutput::new(CLIENT_NAME).map_err(backend)?;
    output
        .ports()
        .iter()
        .map(|p| output.port_name(p).map_err(backend))
        .collect()
}

/// An open MIDI input; messages stop flowing when it is dropped.
pub struct InputPort {
    _connection: MidiInputConnection<()>,
    name: String,
}

impl InputPort {
    pub fn name(&self) -> &str {
        &self.name
    }
}

/// Opens the first input whose name contains `name_contains` (case-insensitive).
/// Parsed messages are sent to `tx`; unparsable ones are dropped.
pub fn open_input(name_contains: &str, tx: Sender<TimedMidi>) -> Result<InputPort, PortError> {
    let mut input = MidiInput::new(CLIENT_NAME).map_err(backend)?;
    input.ignore(Ignore::All);
    let ports = input.ports();
    let names = ports
        .iter()
        .map(|p| input.port_name(p).map_err(backend))
        .collect::<Result<Vec<_>, _>>()?;
    let Some(i) = find_match(&names, name_contains) else {
        return Err(PortError::NotFound {
            kind: "input",
            wanted: name_contains.to_owned(),
            available: names,
        });
    };
    let name = names[i].clone();
    let connection = input
        .connect(
            &ports[i],
            "dj-software-in",
            move |_backend_micros, bytes, _| {
                if let Some(message) = MidiMessage::parse(bytes) {
                    // The receiver may be gone during shutdown; nothing to do then.
                    let micros = clock::now_micros();
                    let _ = tx.send(TimedMidi { micros, message });
                }
            },
            (),
        )
        .map_err(backend)?;
    Ok(InputPort {
        _connection: connection,
        name,
    })
}

/// An open MIDI output.
pub struct OutputPort {
    connection: MidiOutputConnection,
    name: String,
    send_errors: u64,
}

impl OutputPort {
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Number of sends that failed through [`MidiOut::send`].
    pub fn send_errors(&self) -> u64 {
        self.send_errors
    }

    pub fn send(&mut self, msg: MidiMessage) -> Result<(), PortError> {
        self.connection
            .send(&msg.encode())
            .map_err(|e| PortError::Send(e.to_string()))
    }
}

/// Opens the first output whose name contains `name_contains` (case-insensitive).
pub fn open_output(name_contains: &str) -> Result<OutputPort, PortError> {
    let output = MidiOutput::new(CLIENT_NAME).map_err(backend)?;
    let ports = output.ports();
    let names = ports
        .iter()
        .map(|p| output.port_name(p).map_err(backend))
        .collect::<Result<Vec<_>, _>>()?;
    let Some(i) = find_match(&names, name_contains) else {
        return Err(PortError::NotFound {
            kind: "output",
            wanted: name_contains.to_owned(),
            available: names,
        });
    };
    let name = names[i].clone();
    let connection = output
        .connect(&ports[i], "dj-software-out")
        .map_err(backend)?;
    Ok(OutputPort {
        connection,
        name,
        send_errors: 0,
    })
}

/// Sink for outgoing MIDI, so feedback code can be tested with a recorder.
pub trait MidiOut {
    fn send(&mut self, msg: MidiMessage);
}

impl MidiOut for OutputPort {
    fn send(&mut self, msg: MidiMessage) {
        if OutputPort::send(self, msg).is_err() {
            self.send_errors += 1;
        }
    }
}
