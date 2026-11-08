//! The data behind the scrolling waveforms: how a bucket is coloured, where the beat
//! ticks fall and where the playhead is drawn. Drawing is elsewhere.

mod colour;
mod playhead;
mod ticks;

pub use colour::colour;
pub use playhead::Playhead;
pub use ticks::{Tick, ticks};
