//! Guided capture of a controller's MIDI messages: a checklist of steps, a pure
//! session state machine that turns incoming messages into per-control records,
//! and the TOML fixture those records are saved to.
//!
//! Everything here is pure (no ports, no stdin); the CLI drives it.

mod checklist;
mod fixture;
mod probe;
mod session;

pub use checklist::{Checklist, ChecklistError, Step, StepKind, load_checklist, parse_checklist};
pub use fixture::{Cc14, Control, Fixture, FixtureError, MessageRange, Meta, parse_fixture};
pub use probe::{LedProbe, LedSink, PROBE_VELOCITIES, ProbeAnswer, Question, run_led_probe};
pub use session::{CaptureSession, StepStatus};
