//! Serato's cue and beat grid tags, written into the copies on the stick.

use std::collections::BTreeMap;
use std::fs::File;
use std::path::Path;

use lofty::TextEncoding;
use lofty::config::{ParseOptions, WriteOptions};
use lofty::file::AudioFile;
use lofty::id3::v2::{BinaryFrame, Frame, FrameId, GeneralEncapsulatedObject, Id3v2Tag};
use lofty::iff::wav::WavFile;
use lofty::mpeg::MpegFile;

use super::super::{ExportError, ExportTrack, FileKind};
use super::markers::{beatgrid_geob, markers2_geob};

const GEOB: FrameId<'static> = FrameId::Valid(std::borrow::Cow::Borrowed("GEOB"));
const PREFIX: &str = "Serato ";

/// The `Serato *` GEOB frames `t` should carry, as description and data; none for a file type
/// that holds no Serato tags.
#[must_use]
pub fn wanted_frames(t: &ExportTrack) -> Vec<(&'static str, Vec<u8>)> {
    if matches!(t.kind, FileKind::Other(_)) {
        return Vec::new();
    }
    let mut frames = vec![("Serato Markers2", markers2_geob(t))];
    if let Some(grid) = beatgrid_geob(t) {
        frames.push(("Serato BeatGrid", grid));
    }
    frames
}

fn format_error(path: &Path, why: impl ToString) -> ExportError {
    ExportError::Format {
        path: path.to_path_buf(),
        why: why.to_string(),
    }
}

fn read_id3(copy: &Path, kind: &FileKind) -> Result<Option<Id3v2Tag>, ExportError> {
    let mut file = File::open(copy).map_err(|source| ExportError::Io {
        path: copy.to_path_buf(),
        source,
    })?;
    let opts = ParseOptions::new();
    match kind {
        FileKind::Mp3 => MpegFile::read_from(&mut file, opts)
            .map(|f| f.id3v2().cloned())
            .map_err(|e| format_error(copy, e)),
        FileKind::Wav => WavFile::read_from(&mut file, opts)
            .map(|f| f.id3v2().cloned())
            .map_err(|e| format_error(copy, e)),
        FileKind::Other(_) => Ok(None),
    }
}

/// The `Serato *` GEOB frames in `tag`, description to data.
fn serato_frames(tag: &Id3v2Tag) -> BTreeMap<String, Vec<u8>> {
    tag.iter()
        .filter_map(|frame| {
            let Frame::Binary(bin) = frame else {
                return None;
            };
            if bin.id() != &GEOB {
                return None;
            }
            let geob = GeneralEncapsulatedObject::parse(&bin.data, bin.flags()).ok()?;
            let description = geob.descriptor?;
            description
                .starts_with(PREFIX)
                .then_some((description, geob.data))
        })
        .collect()
}

/// Whether the Serato tags in `copy` differ from the ones `t` should carry, in content or in
/// which frames there are. A file that cannot be read needs tagging; a type that holds no
/// Serato tags never does.
#[must_use]
pub fn needs_tagging(copy: &Path, t: &ExportTrack) -> bool {
    if matches!(t.kind, FileKind::Other(_)) {
        return false;
    }
    let wanted: BTreeMap<String, Vec<u8>> = wanted_frames(t)
        .into_iter()
        .map(|(d, data)| (d.to_string(), data))
        .collect();
    match read_id3(copy, &t.kind) {
        Ok(Some(tag)) => serato_frames(&tag) != wanted,
        Ok(None) => !wanted.is_empty(),
        Err(_) => true,
    }
}

/// Replaces the `Serato *` GEOB frames in `copy` with the ones `t` should carry, keeping every
/// other frame; creates the ID3v2 tag when there is none. Does nothing for a type that holds
/// no Serato tags. Only ever call this on a copy.
///
/// # Errors
///
/// [`ExportError::Format`] when the file cannot be parsed or the tag cannot be written,
/// [`ExportError::Io`] when it cannot be opened.
pub fn write_tags(copy: &Path, t: &ExportTrack) -> Result<(), ExportError> {
    if matches!(t.kind, FileKind::Other(_)) {
        return Ok(());
    }
    let mut tag = read_id3(copy, &t.kind)?.unwrap_or_default();
    tag.retain(|frame| match frame {
        Frame::Binary(bin) if bin.id() == &GEOB => {
            GeneralEncapsulatedObject::parse(&bin.data, bin.flags())
                .ok()
                .and_then(|g| g.descriptor)
                .is_none_or(|d| !d.starts_with(PREFIX))
        }
        _ => true,
    });
    let options = WriteOptions::default();
    for (description, data) in wanted_frames(t) {
        let geob = GeneralEncapsulatedObject::new(
            TextEncoding::Latin1,
            Some("application/octet-stream".into()),
            Some(String::new()),
            Some(description.into()),
            data,
        );
        let bytes = geob.as_bytes(options).map_err(|e| format_error(copy, e))?;
        tag.insert(Frame::Binary(BinaryFrame::new(GEOB, bytes)));
    }
    let io = |source| ExportError::Io {
        path: copy.to_path_buf(),
        source,
    };
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(copy)
        .map_err(io)?;
    match &t.kind {
        FileKind::Mp3 => {
            let mut mpeg = MpegFile::read_from(&mut file, ParseOptions::new())
                .map_err(|e| format_error(copy, e))?;
            mpeg.set_id3v2(tag);
            mpeg.save_to(&mut file, options)
        }
        FileKind::Wav => {
            let mut wav = WavFile::read_from(&mut file, ParseOptions::new())
                .map_err(|e| format_error(copy, e))?;
            wav.set_id3v2(tag);
            wav.save_to(&mut file, options)
        }
        FileKind::Other(_) => Ok(()),
    }
    .map_err(|e| format_error(copy, e))
}
