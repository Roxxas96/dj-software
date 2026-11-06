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

/// A loaded sample.
#[derive(Debug)]
pub struct Sample {
    pub buffer: Arc<TrackBuffer>,
    /// Decoding stopped early, so the sample may be incomplete.
    pub truncated: bool,
}

/// The longest sample's frame count at the highest sample rate checked before decoding
/// (192 kHz), plus one frame so a sample just over the limit is seen as such.
const MAX_PREFIX_FRAMES: usize = (MAX_SAMPLE_SECONDS as usize) * 192_000 + 1;

/// Decodes every configured pad that is not empty, with the pad's 0-based index and source.
/// A `track` is read through `library`: `Ok(None)` when there is none, `Err` with the reason
/// when it could not be opened or read (each `track` pad then fails with that reason). A
/// sample too long is refused without decoding all of it.
pub fn load<'a>(
    config: &'a SamplerConfig,
    library: Result<Option<&Reader>, &str>,
) -> Vec<(usize, &'a PadSource, Result<Sample, SampleError>)> {
    config
        .pads
        .iter()
        .enumerate()
        .filter_map(|(index, source)| Some((index, source.as_ref()?)))
        .map(|(index, source)| {
            let loaded = match source {
                PadSource::File(path) => load_file(path),
                PadSource::Track(id) => match library {
                    Ok(library) => load_track(*id, library),
                    Err(reason) => Err(SampleError::Library(reason.to_owned())),
                },
            };
            (index, source, loaded)
        })
        .collect()
}

/// The report for a pad whose sample was loaded but may be incomplete.
pub fn truncated_line(index: usize, source: &PadSource) -> String {
    format!(
        "{} (loaded anyway)",
        failure_line_with(index, source, "the file ended early")
    )
}

/// The report for a pad that failed: `sampler pad N (<file or track>): <reason>`.
pub fn failure_line(index: usize, source: &PadSource, error: &SampleError) -> String {
    failure_line_with(index, source, error)
}

fn failure_line_with(index: usize, source: &PadSource, reason: impl fmt::Display) -> String {
    let what = match source {
        PadSource::File(path) => safe(path.display()),
        PadSource::Track(id) => format!("track {id}"),
    };
    format!("sampler pad {} ({what}): {reason}", index + 1)
}

fn load_track(id: i64, library: Option<&Reader>) -> Result<Sample, SampleError> {
    let library = library.ok_or(SampleError::NoLibrary)?;
    let details = library
        .track(id)
        .map_err(|e| SampleError::Library(e.to_string()))?
        .ok_or(SampleError::UnknownTrack)?;
    // The stored length refuses a long track before any decoding.
    if let (Some(frames), Some(rate)) = (details.row.duration_frames, details.row.sample_rate)
        && rate > 0
    {
        too_long(frames as f64, rate)?;
    }
    load_file(&details.path)
}

/// Fails if `frames` at `rate` is longer than [`MAX_SAMPLE_SECONDS`].
fn too_long(frames: f64, rate: u32) -> Result<(), SampleError> {
    let seconds = frames / f64::from(rate);
    if seconds > MAX_SAMPLE_SECONDS {
        return Err(SampleError::TooLong(seconds));
    }
    Ok(())
}

fn load_file(path: &Path) -> Result<Sample, SampleError> {
    if !path.is_file() {
        return Err(SampleError::MissingFile);
    }
    let decoded = decode::decode_prefix_checked(path, MAX_PREFIX_FRAMES)
        .map_err(|e| SampleError::Decode(e.to_string()))?;
    too_long(decoded.track.frames() as f64, decoded.track.sample_rate())?;
    Ok(Sample {
        buffer: Arc::new(decoded.track),
        truncated: decoded.truncated,
    })
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
                Some(PadSource::File(good)),
                Some(PadSource::File(dir.path().join("missing.wav"))),
                Some(PadSource::File(long)),
                Some(PadSource::Track(999)),
            ],
        };
        let loaded = load(&config, Ok(Some(&reader)));
        assert_eq!(loaded.len(), 4);
        let buffer = &loaded[0].2.as_ref().unwrap().buffer;
        assert_eq!(buffer.frames(), 2_000);
        assert_eq!(buffer.sample_rate(), 8_000);

        let lines: Vec<String> = loaded
            .iter()
            .filter_map(|(index, source, result)| {
                let error = result.as_ref().err()?;
                Some(failure_line(*index, source, error))
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
        let no_library = load(&config, Ok(None));
        assert!(matches!(no_library[3].2, Err(SampleError::NoLibrary)));
    }

    #[test]
    fn empty_pads_are_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let config = SamplerConfig {
            volume: 1.0,
            pads: vec![None, Some(PadSource::File(dir.path().join("missing.wav")))],
        };
        let loaded = load(&config, Ok(None));
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].0, 1, "pad 2");
        assert!(matches!(loaded[0].2, Err(SampleError::MissingFile)));
    }

    #[test]
    fn a_library_failure_fails_only_the_track_pads() {
        let dir = tempfile::tempdir().unwrap();
        let config = SamplerConfig {
            volume: 1.0,
            pads: vec![
                Some(PadSource::Track(7)),
                Some(PadSource::File(dir.path().join("missing.wav"))),
            ],
        };
        let loaded = load(&config, Err("disk I/O error\u{1b}[2J"));
        let lines: Vec<String> = loaded
            .iter()
            .filter_map(|(index, source, result)| {
                Some(failure_line(*index, source, result.as_ref().err()?))
            })
            .collect();
        assert_eq!(
            lines[0], "sampler pad 1 (track 7): library error: disk I/O error\u{fffd}[2J",
            "{lines:?}"
        );
        assert!(lines[1].ends_with("): file not found"), "{lines:?}");
    }

    #[test]
    fn a_long_library_track_is_refused_without_decoding() {
        let dir = tempfile::tempdir().unwrap();
        let lib = library::Library::open(&dir.path().join("library.sqlite3")).unwrap();
        // The file does not exist: a decode (or even the file check) would say so instead.
        lib.writer()
            .submit_wait(library::WriteOp::Raw(vec![library::RawStatement {
                sql: "INSERT INTO tracks(id, path, fingerprint, duration_frames, sample_rate, \
                      added_at) VALUES (5, '/nowhere/mix.wav', x'00', 100000000, 44100, 0)"
                    .into(),
                params: vec![],
            }]))
            .unwrap();
        let reader = lib.reader().unwrap();
        let config = SamplerConfig {
            volume: 1.0,
            pads: vec![Some(PadSource::Track(5))],
        };
        let loaded = load(&config, Ok(Some(&reader)));
        let error = loaded[0].2.as_ref().unwrap_err();
        assert!(matches!(error, SampleError::TooLong(_)), "{error}");
        assert!(
            error.to_string().contains("2267.6 s is too long"),
            "{error}"
        );
    }

    #[test]
    fn a_truncated_sample_is_reported_but_loaded() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cut.wav");
        write_wav(&path, 1.0);
        // The header still declares 1 s of audio, but the file ends after half of it.
        let bytes = std::fs::read(&path).unwrap();
        std::fs::write(&path, &bytes[..44 + 8_000]).unwrap();
        let config = SamplerConfig {
            volume: 1.0,
            pads: vec![Some(PadSource::File(path))],
        };
        let loaded = load(&config, Ok(None));
        let sample = loaded[0].2.as_ref().unwrap();
        assert!(sample.truncated);
        assert!(sample.buffer.frames() > 0);
        let line = truncated_line(0, loaded[0].1);
        assert!(line.starts_with("sampler pad 1 ("), "{line}");
        assert!(line.contains("the file ended early"), "{line}");
    }
}
