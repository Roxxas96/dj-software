//! Stems separation: the [`Separator`] trait, a synthetic separator for tests and the
//! htdemucs model separator (cargo feature `model`), plus `stems setup`, which fetches the
//! model's weights from their author and fills the committed weight-free graph.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use engine::{StemError, StemSet, TrackBuffer};
use thiserror::Error;

// The model's chunking and DSP; without the feature only their tests build them.
#[cfg(any(feature = "model", test))]
mod chunked;
#[cfg(any(feature = "model", test))]
mod dsp;
#[cfg(feature = "model")]
mod model;
#[cfg(any(feature = "model", test))]
mod resample;
mod setup;
mod synthetic;

#[cfg(feature = "model")]
pub use model::{ModelOptions, ModelSeparator};
pub use setup::{
    GRAPH_SHA256, MODEL_ID, SetupError, SetupEvent, WEIGHTS_SHA256, WEIGHTS_URL, setup,
};
pub use synthetic::SyntheticSeparator;

/// The longest track separated: 20 minutes. Longer ones (DJ mixes) get no stems.
pub const MAX_SECONDS: u64 = 20 * 60;

/// Splits a track into its stored stems (vocals, bass, rhythm); instruments is the residual.
pub trait Separator: Send {
    /// Separates `track`, calling `progress` with the done fraction (0 to 1) after each
    /// chunk. Returns [`SeparateError::Cancelled`] once `cancel` is set; the flag is checked
    /// at least once per chunk.
    fn separate(
        &mut self,
        track: &TrackBuffer,
        progress: &dyn Fn(f32),
        cancel: &AtomicBool,
    ) -> Result<StemSet, SeparateError>;
}

/// Why a separation gave no stems.
#[derive(Debug, Error)]
pub enum SeparateError {
    #[error("cancelled")]
    Cancelled,
    #[error("the track is {minutes} minutes long; stems stop at 20 minutes")]
    TooLong { minutes: u64 },
    #[error("the model failed: {0}")]
    Model(String),
    #[error(transparent)]
    Stems(#[from] StemError),
}

/// Refuses tracks longer than [`MAX_SECONDS`].
pub fn check_length(frames: usize, sample_rate: u32) -> Result<(), SeparateError> {
    let rate = u64::from(sample_rate.max(1));
    let frames = frames as u64;
    if frames > MAX_SECONDS * rate {
        return Err(SeparateError::TooLong {
            minutes: frames.div_ceil(rate * 60),
        });
    }
    Ok(())
}

/// A sample in `[-1, 1]` as 16-bit, as [`StemSet`] stores it (clamped, rounded).
pub(crate) fn to_i16(value: f32) -> i16 {
    // `as` saturates, and maps NaN to 0.
    (value * 32_768.0).round().clamp(-32_768.0, 32_767.0) as i16
}

/// The file name of the filled model.
pub const MODEL_NAME: &str = "htdemucs-filled.onnx";

/// The per-user data folder (`ProjectDirs` "open-jockey"), or `open-jockey-data` in the
/// current folder on a system with no home folder.
fn data_dir() -> PathBuf {
    directories::ProjectDirs::from("", "", "open-jockey")
        .map(|dirs| dirs.data_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from("open-jockey-data"))
}

/// Where `stems setup` writes the filled model: `<data dir>/models/htdemucs-filled.onnx`.
pub fn model_path() -> PathBuf {
    data_dir().join("models").join(MODEL_NAME)
}

/// The CoreML compile cache (macOS): `<data dir>/stems/coreml-cache`, about 0.5 GB,
/// outside the stem cache's budget and safe to delete.
pub fn coreml_cache_dir() -> PathBuf {
    data_dir().join("stems").join("coreml-cache")
}
