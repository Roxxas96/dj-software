//! Audio file decoding into engine-ready sample buffers.

mod mp4_trim;

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

/// Frames a decode may fall short of the container's declared length before it counts as
/// truncated (gapless trimming is not exact across encoders).
const DECLARED_SLACK_FRAMES: usize = 2_048;

/// A shortfall against the declared length only counts past this fraction of it.
const DECLARED_SHORTFALL_DIVISOR: usize = 10;

/// A decoded track plus whether decoding ended early.
#[derive(Debug, Clone, PartialEq)]
pub struct Decoded {
    /// The decoded audio.
    pub track: TrackBuffer,
    /// A read or decode error, or a shortfall against the container's declared length,
    /// ended the track after some audio, so it may be incomplete.
    pub truncated: bool,
}

/// Decodes `path` at its native sample rate into interleaved stereo.
///
/// Mono is duplicated to both channels; with more than two channels the first two are kept.
/// Gapless mode is on, so encoder delay and padding are trimmed. A packet that fails to decode
/// is skipped; a stream that ends early yields the audio decoded so far and sets
/// [`Decoded::truncated`]. Besides read/decode errors, truncation is inferred when the
/// decode falls over 10% short of the length the container declares. That declared length
/// is ignored for MP3s without a Xing/Info/LAME header, where it is only a bitrate
/// estimate. A file cut cleanly at a packet boundary without a declared length is not
/// detected.
///
/// # Errors
///
/// Returns [`DecodeError`] if the file cannot be read, is not a supported audio format, or
/// contains no decodable audio.
pub fn decode_file(path: &Path) -> Result<Decoded, DecodeError> {
    decode(path, None)
}

/// Like [`decode_file`], but stops once `max_frames` frames have been decoded, returning
/// at most that many (fewer if the file is shorter). Used to fingerprint a file without
/// decoding all of it.
///
/// # Errors
///
/// As [`decode_file`].
pub fn decode_prefix(path: &Path, max_frames: usize) -> Result<TrackBuffer, DecodeError> {
    decode(path, Some(max_frames)).map(|d| d.track)
}

/// The decode loop shared by [`decode_file`] and [`decode_prefix`]; `limit` caps the
/// number of frames returned.
fn decode(path: &Path, limit: Option<usize>) -> Result<Decoded, DecodeError> {
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
    let mut declared_frames = track.num_frames;
    let has_gapless_header = track.delay.is_some();
    let track_delay_missing = track.delay.is_none();
    let params = track
        .codec_params
        .as_ref()
        .and_then(|p| p.audio())
        .ok_or(DecodeError::NoAudioTrack)?;

    // Without a Xing/Info/LAME header symphonia estimates an MP3's length from its bitrate,
    // which trailing tags or a quiet VBR intro inflate; only a header's count is reliable.
    if params.codec == symphonia::core::codecs::audio::well_known::CODEC_ID_MP3
        && !has_gapless_header
    {
        declared_frames = None;
    }

    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(params, &AudioDecoderOptions::default().gapless(true))
        .map_err(map_error)?;

    let mut sample_rate = params.sample_rate;
    let mut out: Vec<f32> = Vec::new();
    let mut scratch: Vec<f32> = Vec::new();
    let mut last_error: Option<SymphoniaError> = None;
    let mut truncated = false;
    // True while the most recent packet failed to decode; if the stream then ends, the
    // tail of the file was lost.
    let mut trailing_error = false;
    // With a limit, decoding stops once it is reached. An MP4 edit list's encoder delay is
    // trimmed from the front afterwards, so decode that much more; it is read once the
    // decoded sample rate is known.
    let mut stop_after: Option<usize> = None;

    loop {
        if let Some(frames) = limit
            && stop_after.is_none()
            && let Some(rate) = sample_rate.filter(|_| !out.is_empty())
        {
            let delay = if track_delay_missing {
                mp4_trim::read_trim(path, track_id, rate).map_or(0, |t| t.delay)
            } else {
                0
            };
            stop_after = Some(frames.saturating_add(delay).saturating_mul(2));
        }
        if stop_after.is_some_and(|samples| out.len() >= samples) {
            break;
        }
        let packet = match format.next_packet() {
            Ok(Some(p)) => p,
            // A read error after some audio is an early end of stream (e.g. truncation).
            Ok(None) => {
                truncated |= trailing_error;
                break;
            }
            Err(_) if !out.is_empty() => {
                truncated = true;
                break;
            }
            Err(e) => return Err(map_error(e)),
        };
        if packet.track_id != track_id {
            continue;
        }
        match decoder.decode(&packet) {
            Ok(buf) => {
                trailing_error = false;
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
                trailing_error = true;
                last_error = Some(e);
            }
            Err(e) => {
                if out.is_empty() {
                    return Err(map_error(e));
                }
                truncated = true;
                break;
            }
        }
    }

    // A stream cut at a packet boundary ends cleanly, so also compare with the length the
    // container declares; a shortfall of over 10% (and more than a packet or two) means
    // data is missing. Declared lengths are only used where the container states them.
    if limit.is_none()
        && let Some(declared) = declared_frames
        && usize::try_from(declared).is_ok_and(|d| {
            let shortfall = d.saturating_sub(out.len() / 2);
            shortfall > DECLARED_SLACK_FRAMES.max(d / DECLARED_SHORTFALL_DIVISOR)
        })
    {
        truncated = true;
    }
    if track_delay_missing
        && let Some(rate) = sample_rate
        && let Some(trim) = mp4_trim::read_trim(path, track_id, rate)
    {
        let start = trim.delay.saturating_mul(2).min(out.len());
        out.drain(..start);
        // A zero edit duration means "to the end of the media", not "empty".
        if trim.length > 0 {
            out.truncate(trim.length.saturating_mul(2));
        }
    }
    if let Some(frames) = limit {
        out.truncate(frames.saturating_mul(2));
    }
    if out.is_empty() {
        return Err(last_error.map_or(DecodeError::NoAudioTrack, map_error));
    }
    let rate = sample_rate.ok_or_else(|| DecodeError::Corrupt("unknown sample rate".into()))?;
    let track = TrackBuffer::new(rate, out).map_err(|e| DecodeError::Corrupt(e.to_string()))?;
    Ok(Decoded { track, truncated })
}
