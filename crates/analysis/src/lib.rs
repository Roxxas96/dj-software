//! Offline track analysis: waveform overview, BPM and beatgrid, and (later) key detection.

mod onset;
mod tempo;
mod waveform;

pub use tempo::{Beatgrid, BpmRange, detect_beatgrid};
pub use waveform::{DEFAULT_FRAMES_PER_BUCKET, Waveform, WaveformError, waveform};
