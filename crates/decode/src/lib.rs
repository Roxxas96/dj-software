//! Audio file decoding into engine-ready sample buffers.

use std::fs::File;
use std::path::Path;

use engine::TrackBuffer;
use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;

/// Why a file could not be turned into a [`TrackBuffer`].
#[derive(Debug, thiserror::Error)]
pub enum DecodeError {
    /// The file could not be opened or read.
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
    /// The container or codec is not recognised or not supported.
    #[error("unsupported audio file: {0}")]
    Unsupported(String),
    /// The file was recognised but its audio data is unusable.
    #[error("corrupt audio file: {0}")]
    Corrupt(String),
    /// The file has no audio track.
    #[error("file contains no audio track")]
    NoAudioTrack,
}

fn map_error(err: SymphoniaError) -> DecodeError {
    match err {
        SymphoniaError::IoError(e) => DecodeError::Io(e),
        SymphoniaError::Unsupported(m) => DecodeError::Unsupported(m.to_string()),
        other => DecodeError::Corrupt(other.to_string()),
    }
}

/// Gapless trim for an MP4 audio track, in frames at the track's sample rate.
struct Trim {
    delay: usize,
    length: usize,
}

fn be32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(
        b.get(at..at.checked_add(4)?)?.try_into().ok()?,
    ))
}

fn be64(b: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_be_bytes(
        b.get(at..at.checked_add(8)?)?.try_into().ok()?,
    ))
}

/// Iterates the ISO base media boxes in `data` as `(type, payload)`.
fn boxes(data: &[u8]) -> impl Iterator<Item = (&[u8], &[u8])> {
    let mut at = 0usize;
    std::iter::from_fn(move || {
        let size = be32(data, at)? as usize;
        let kind = data.get(at + 4..at + 8)?;
        let (header, size) = match size {
            0 => (8, data.len() - at),
            1 => (16, usize::try_from(be64(data, at + 8)?).ok()?),
            n => (8, n),
        };
        let payload = data.get(at + header..at.checked_add(size)?)?;
        at += size;
        Some((kind, payload))
    })
}

/// symphonia 0.6 does not apply MP4 edit lists, so AAC encoder delay and padding are read
/// from the first real `elst` entry here.
fn mp4_trim(data: &[u8], rate: u32) -> Option<Trim> {
    if data.get(4..8)? != b"ftyp" {
        return None;
    }
    let moov = boxes(data).find(|(k, _)| *k == b"moov")?.1;
    let mvhd = boxes(moov).find(|(k, _)| *k == b"mvhd")?.1;
    let movie_scale = u64::from(if *mvhd.first()? == 1 {
        be32(mvhd, 20)?
    } else {
        be32(mvhd, 12)?
    });
    if movie_scale == 0 {
        return None;
    }
    let elst = boxes(moov)
        .filter(|(k, _)| *k == b"trak")
        .filter_map(|(_, trak)| boxes(trak).find(|(k, _)| *k == b"edts"))
        .find_map(|(_, edts)| boxes(edts).find(|(k, _)| *k == b"elst"))?
        .1;
    let v1 = *elst.first()? == 1;
    let (mut at, count) = (8usize, be32(elst, 4)?);
    for _ in 0..count {
        let (duration, media_time, next) = if v1 {
            (be64(elst, at)?, be64(elst, at + 8)? as i64, at + 20)
        } else {
            (
                u64::from(be32(elst, at)?),
                i64::from(be32(elst, at + 4)? as i32),
                at + 12,
            )
        };
        at = next;
        if media_time >= 0 {
            let length = duration.checked_mul(u64::from(rate))? / movie_scale;
            return Some(Trim {
                delay: media_time as usize,
                length: usize::try_from(length).ok()?,
            });
        }
    }
    None
}

/// Decodes `path` at its native sample rate into interleaved stereo.
///
/// Mono is duplicated to both channels; with more than two channels the first two are kept.
/// Gapless mode is on, so encoder delay and padding are trimmed. A packet that fails to decode
/// is skipped; a stream that ends early yields the audio decoded so far.
///
/// # Errors
///
/// Returns [`DecodeError`] if the file cannot be read, is not a supported audio format, or
/// contains no decodable audio.
pub fn decode_file(path: &Path) -> Result<TrackBuffer, DecodeError> {
    let file = File::open(path)?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());

    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }

    let mut format = symphonia::default::get_probe()
        .probe(
            &hint,
            mss,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .map_err(|e| match e {
            // The probe reports "no format matched" as Unsupported; anything else is real.
            SymphoniaError::Unsupported(m) => DecodeError::Unsupported(m.to_string()),
            other => map_error(other),
        })?;

    let track = format
        .default_track(TrackType::Audio)
        .ok_or(DecodeError::NoAudioTrack)?;
    let track_id = track.id;
    let track_delay_missing = track.delay.is_none();
    let params = track
        .codec_params
        .as_ref()
        .and_then(|p| p.audio())
        .ok_or(DecodeError::NoAudioTrack)?;

    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(params, &AudioDecoderOptions::default().gapless(true))
        .map_err(map_error)?;

    let mut sample_rate = params.sample_rate;
    let mut out: Vec<f32> = Vec::new();
    let mut scratch: Vec<f32> = Vec::new();
    let mut last_error: Option<SymphoniaError> = None;

    loop {
        let packet = match format.next_packet() {
            Ok(Some(p)) => p,
            // A read error after some audio is an early end of stream (e.g. truncation).
            Ok(None) => break,
            Err(_) if !out.is_empty() => break,
            Err(e) => return Err(map_error(e)),
        };
        if packet.track_id != track_id {
            continue;
        }
        match decoder.decode(&packet) {
            Ok(buf) => {
                let spec = buf.spec();
                sample_rate = Some(spec.rate());
                let channels = spec.channels().count();
                if channels == 0 || buf.frames() == 0 {
                    continue;
                }
                buf.copy_to_vec_interleaved(&mut scratch);
                for frame in scratch.chunks_exact(channels) {
                    let left = frame[0];
                    let right = if channels > 1 { frame[1] } else { left };
                    out.push(left);
                    out.push(right);
                }
            }
            Err(e @ (SymphoniaError::DecodeError(_) | SymphoniaError::IoError(_))) => {
                last_error = Some(e);
            }
            Err(e) => {
                if out.is_empty() {
                    return Err(map_error(e));
                }
                break;
            }
        }
    }

    if track_delay_missing
        && let Some(trim) = std::fs::read(path)
            .ok()
            .and_then(|d| mp4_trim(&d, sample_rate?))
    {
        let start = trim.delay.saturating_mul(2).min(out.len());
        out.drain(..start);
        out.truncate(trim.length.saturating_mul(2));
    }
    if out.is_empty() {
        return Err(last_error.map_or(DecodeError::NoAudioTrack, map_error));
    }
    let rate = sample_rate.ok_or_else(|| DecodeError::Corrupt("unknown sample rate".into()))?;
    TrackBuffer::new(rate, out).map_err(|e| DecodeError::Corrupt(e.to_string()))
}
