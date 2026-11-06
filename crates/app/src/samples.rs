//! Loads the sampler pads' samples at startup, off the audio thread.

use std::fmt;
use std::path::Path;
use std::sync::Arc;

use engine::TrackBuffer;
use library::Reader;

use crate::config::{PadSource, SamplerConfig};
use crate::format::safe;

/// The longest sample a pad accepts, in seconds.
pub const MAX_SAMPLE_SECONDS: f64 = 60.0;

/// Why a pad's sample could not be loaded.
#[derive(Debug)]
pub enum SampleError {
    /// The file does not exist.
    MissingFile,
    /// The file could not be decoded.
    Decode(String),
    /// The library has no track with this id.
    UnknownTrack,
    /// A track id was configured but there is no library to look it up in.
    NoLibrary,
    /// Looking the track up in the library failed.
    Library(String),
    /// The sample is longer than [`MAX_SAMPLE_SECONDS`].
    TooLong(f64),
}

impl fmt::Display for SampleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingFile => write!(f, "file not found"),
            Self::Decode(reason) => write!(f, "{}", safe(reason)),
            Self::UnknownTrack => write!(f, "no such track in the library"),
            Self::NoLibrary => write!(f, "no library to look the track up in"),
            Self::Library(reason) => write!(f, "library error: {}", safe(reason)),
            Self::TooLong(seconds) => write!(
                f,
                "{seconds:.1} s is too long, samples are at most {MAX_SAMPLE_SECONDS} s"
            ),
        }
    }
}

/// Decodes every configured pad; a result's number is the pad's 0-based index. A `track` is
/// read through `library`.
pub fn load(
    config: &SamplerConfig,
    library: Option<&Reader>,
) -> Vec<(usize, Result<Arc<TrackBuffer>, SampleError>)> {
    config
        .pads
        .iter()
        .enumerate()
        .map(|(index, source)| {
            let loaded = match source {
                PadSource::File(path) => load_file(path),
                PadSource::Track(id) => load_track(*id, library),
            };
            (index, loaded)
        })
        .collect()
}

/// The report for a pad that failed: `sampler pad N (<file or track>): <reason>`.
pub fn failure_line(index: usize, source: &PadSource, error: &SampleError) -> String {
    let what = match source {
        PadSource::File(path) => safe(path.display()),
        PadSource::Track(id) => format!("track {id}"),
    };
    format!("sampler pad {} ({what}): {error}", index + 1)
}

fn load_track(id: i64, library: Option<&Reader>) -> Result<Arc<TrackBuffer>, SampleError> {
    let library = library.ok_or(SampleError::NoLibrary)?;
    let details = library
        .track(id)
        .map_err(|e| SampleError::Library(e.to_string()))?
        .ok_or(SampleError::UnknownTrack)?;
    load_file(&details.path)
}

fn load_file(path: &Path) -> Result<Arc<TrackBuffer>, SampleError> {
    if !path.is_file() {
        return Err(SampleError::MissingFile);
    }
    let decoded = decode::decode_file(path).map_err(|e| SampleError::Decode(e.to_string()))?;
    let seconds = decoded.track.frames() as f64 / f64::from(decoded.track.sample_rate());
    if seconds > MAX_SAMPLE_SECONDS {
        return Err(SampleError::TooLong(seconds));
    }
    Ok(Arc::new(decoded.track))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Writes a 16-bit mono PCM WAV of `seconds` of a quiet tone at 8 kHz.
    fn write_wav(path: &Path, seconds: f64) {
        const SR: u32 = 8_000;
        let samples = (seconds * f64::from(SR)) as usize;
        let data_len = u32::try_from(samples * 2).unwrap();
        let mut bytes = Vec::with_capacity(44 + samples * 2);
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes()); // PCM
        bytes.extend_from_slice(&1u16.to_le_bytes()); // channels
        bytes.extend_from_slice(&SR.to_le_bytes());
        bytes.extend_from_slice(&(SR * 2).to_le_bytes()); // byte rate
        bytes.extend_from_slice(&2u16.to_le_bytes()); // block align
        bytes.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&data_len.to_le_bytes());
        for i in 0..samples {
            let s: i16 = if (i / 20) % 2 == 0 { 3000 } else { -3000 };
            bytes.extend_from_slice(&s.to_le_bytes());
        }
        std::fs::write(path, bytes).unwrap();
    }

    #[test]
    fn bad_samples_are_reported_and_silent() {
        let dir = tempfile::tempdir().unwrap();
        let good = dir.path().join("good.wav");
        let long = dir.path().join("long.wav");
        write_wav(&good, 0.25);
        write_wav(&long, 61.0);
        let lib = library::Library::open(&dir.path().join("library.sqlite3")).unwrap();
        let reader = lib.reader().unwrap();

        let config = SamplerConfig {
            volume: 1.0,
            pads: vec![
                PadSource::File(good),
                PadSource::File(dir.path().join("missing.wav")),
                PadSource::File(long),
                PadSource::Track(999),
            ],
        };
        let loaded = load(&config, Some(&reader));
        assert_eq!(loaded.len(), 4);
        let buffer = loaded[0].1.as_ref().unwrap();
        assert_eq!(buffer.frames(), 2_000);
        assert_eq!(buffer.sample_rate(), 8_000);

        let lines: Vec<String> = loaded
            .iter()
            .filter_map(|(index, result)| {
                let error = result.as_ref().err()?;
                Some(failure_line(*index, &config.pads[*index], error))
            })
            .collect();
        assert_eq!(lines.len(), 3, "{lines:?}");
        assert!(lines[0].starts_with("sampler pad 2 ("), "{lines:?}");
        assert!(lines[0].ends_with("): file not found"), "{lines:?}");
        assert!(lines[1].starts_with("sampler pad 3 ("), "{lines:?}");
        assert!(lines[1].contains("61.0 s is too long"), "{lines:?}");
        assert!(
            lines[2].ends_with("(track 999): no such track in the library"),
            "{lines:?}"
        );

        // A track id with no library at all.
        let no_library = load(&config, None);
        assert!(matches!(no_library[3].1, Err(SampleError::NoLibrary)));
    }
}
