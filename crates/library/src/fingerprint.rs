//! Audio fingerprints: what identifies a track across moves, renames and retags.
//!
//! The fingerprint hashes decoded audio rather than file bytes, so editing tags or moving
//! the file leaves it unchanged, while any change to the audio itself changes it.

use std::path::Path;

/// How much audio, from the start, the fingerprint covers.
pub const SECONDS: u32 = 3;

/// The sample rate decoded on the first try. Files at or below it need one decode; higher
/// rates need a second, longer one.
const FIRST_TRY_RATE: u32 = 48_000;

/// Why a file could not be fingerprinted.
#[derive(Debug, thiserror::Error)]
pub enum FingerprintError {
    #[error(transparent)]
    Decode(#[from] decode::DecodeError),
}

/// The blake3 hash of the first [`SECONDS`] seconds of `path`'s decoded audio at its
/// native rate: the sample rate (`u32` LE), the frame count (`u64` LE), then every
/// interleaved stereo sample quantized to `i16` LE.
///
/// # Errors
///
/// Fails if the file cannot be decoded.
pub fn of_file(path: &Path) -> Result<[u8; 32], FingerprintError> {
    let mut buffer = decode::decode_prefix(path, frames_for(FIRST_TRY_RATE))?;
    if buffer.sample_rate() > FIRST_TRY_RATE {
        buffer = decode::decode_prefix(path, frames_for(buffer.sample_rate()))?;
    }
    let frames = buffer.frames().min(frames_for(buffer.sample_rate()));

    let mut hasher = blake3::Hasher::new();
    hasher.update(&buffer.sample_rate().to_le_bytes());
    hasher.update(&(frames as u64).to_le_bytes());
    let mut chunk = Vec::with_capacity(4096);
    for i in 0..frames {
        for sample in buffer.frame(i) {
            chunk.extend_from_slice(&quantize(sample).to_le_bytes());
        }
        if chunk.len() >= 4092 {
            hasher.update(&chunk);
            chunk.clear();
        }
    }
    hasher.update(&chunk);
    Ok(*hasher.finalize().as_bytes())
}

fn frames_for(rate: u32) -> usize {
    usize::try_from(rate.saturating_mul(SECONDS)).unwrap_or(usize::MAX)
}

/// Full scale is 32767; the float-to-int cast saturates out-of-range values.
#[allow(clippy::cast_possible_truncation)]
fn quantize(sample: f32) -> i16 {
    (sample * f32::from(i16::MAX)).round() as i16
}
