//! The scrolling waveforms: how a bucket is coloured, where the beat ticks fall, where
//! the playhead is drawn, the lanes that draw them, and their zoom.

mod colour;
mod lane;
mod playhead;
mod ticks;
mod zoom;

pub use colour::colour;
pub use lane::{Lane, Lanes, lanes};
pub use playhead::Playhead;
pub use ticks::{Tick, ticks};
pub use zoom::{ZOOM_STEP, ZoomGesture, zoomed};
