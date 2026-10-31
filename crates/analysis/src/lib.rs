//! Offline track analysis: waveform overview, and (later) BPM and key detection.

mod waveform;

pub use waveform::{DEFAULT_FRAMES_PER_BUCKET, Waveform, WaveformError, waveform};
