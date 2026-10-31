//! Offline track analysis: waveform overview, BPM and beatgrid, and musical key.

mod key;
mod notation;
mod onset;
mod tempo;
mod waveform;

pub use key::detect_key;
pub use notation::{Key, KeyNotation};
pub use tempo::{Beatgrid, BpmRange, detect_beatgrid};
pub use waveform::{DEFAULT_FRAMES_PER_BUCKET, Waveform, WaveformError, waveform};
