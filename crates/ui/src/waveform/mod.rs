//! The scrolling waveforms: how a bucket is coloured, where the beat ticks fall, where
//! the playhead is drawn, and the lanes that draw them.

mod colour;
mod lane;
mod playhead;
mod ticks;

pub use colour::colour;
pub use lane::{Lane, Lanes, lanes};
pub use playhead::Playhead;
pub use ticks::{Tick, ticks};
