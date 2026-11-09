//! What an export to a USB stick reads from the library, and how each app times a position.

mod files;
mod layout;
mod plan;
pub mod timing;

pub use files::{copy_atomic, remove_leftovers, write_atomic};
pub use layout::{
    MANIFEST, MUSIC_DIR, Manifest, OJ_DIR, PART_SUFFIX, SPACE_MARGIN, safe_component, stick_paths,
};
pub use plan::{ExportPlan, PlannedTrack, TrackAction, plan_export};

use std::collections::{HashMap, HashSet};
use std::io;
use std::path::PathBuf;

use analysis::Key;
use decode::LeadIn;
use library::{HotcuePoint, Reader};

/// Free bytes on the volume that holds `path`, or `None` when the system will not say.
#[must_use]
pub fn free_space(path: &std::path::Path) -> Option<u64> {
    fs4::available_space(path).ok()
}

/// Why an export could not go ahead.
#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    /// A file or folder could not be read or written.
    #[error("{}: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    /// The library could not be read.
    #[error(transparent)]
    Library(#[from] library::LibraryError),
    /// The export is refused; the reason is shown to the user as is.
    #[error("{0}")]
    Refused(String),
    /// A file on the stick, or a tag in a track, is not in the expected format.
    #[error("{}: {why}", path.display())]
    Format { path: PathBuf, why: String },
}

/// The kind of audio file, by its lowercase extension.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileKind {
    Mp3,
    Wav,
    /// Any other extension, lowercase and without the dot.
    Other(String),
}

impl FileKind {
    fn of(path: &std::path::Path) -> Self {
        let ext = path
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        match ext.as_str() {
            "mp3" => Self::Mp3,
            "wav" => Self::Wav,
            _ => Self::Other(ext),
        }
    }
}

/// One track with everything an export writes about it.
#[derive(Debug, Clone, PartialEq)]
pub struct ExportTrack {
    pub id: i64,
    pub path: PathBuf,
    pub fingerprint: [u8; 32],
    pub size: u64,
    /// The tag, or the file name without its extension when there is none.
    pub title: String,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub genre: Option<String>,
    pub sample_rate: u32,
    pub duration_frames: Option<u64>,
    pub bpm: Option<f64>,
    /// In frames at the native sample rate.
    pub first_beat_frames: Option<f64>,
    /// Standard notation with `#` for sharps ("C#m").
    pub key: Option<String>,
    pub cue_frames: Option<f64>,
    /// Indexed by slot minus 1.
    pub hotcues: [Option<HotcuePoint>; 8],
    pub kind: FileKind,
    pub lead_in: LeadIn,
}

/// A playlist to export: its tracks by id, in order, duplicates kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportPlaylist {
    pub name: String,
    pub tracks: Vec<i64>,
}

/// A track that cannot be exported, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skipped {
    pub path: PathBuf,
    pub why: String,
}

/// Everything an export writes.
#[derive(Debug, Clone, PartialEq)]
pub struct ExportSet {
    /// Unique, in order of first appearance.
    pub tracks: Vec<ExportTrack>,
    pub playlists: Vec<ExportPlaylist>,
    pub skipped: Vec<Skipped>,
}

/// Reads playlists `playlists` (library ids) and every track in them from the library.
///
/// A track that is flagged missing, is gone from disk or has an unknown sample rate goes to
/// [`ExportSet::skipped`] and is left out of the playlists.
///
/// # Errors
///
/// [`ExportError::Refused`] when a playlist no longer exists, [`ExportError::Library`] for a
/// database error.
pub fn gather(reader: &Reader, playlists: &[i64]) -> Result<ExportSet, ExportError> {
    let names: HashMap<i64, String> = reader
        .playlists()?
        .into_iter()
        .map(|p| (p.id, p.name))
        .collect();
    let mut set = ExportSet {
        tracks: Vec::new(),
        playlists: Vec::new(),
        skipped: Vec::new(),
    };
    let mut seen = HashSet::new();
    let mut usable = HashSet::new();
    for &id in playlists {
        let name = names
            .get(&id)
            .ok_or_else(|| ExportError::Refused(format!("playlist {id} no longer exists")))?;
        let mut tracks = Vec::new();
        for row in reader.playlist_tracks(id, 0, usize::MAX)? {
            let track_id = row.id;
            if seen.insert(track_id) {
                match read_track(reader, row)? {
                    Ok(track) => {
                        usable.insert(track_id);
                        set.tracks.push(track);
                    }
                    Err(skipped) => set.skipped.push(skipped),
                }
            }
            if usable.contains(&track_id) {
                tracks.push(track_id);
            }
        }
        set.playlists.push(ExportPlaylist {
            name: name.clone(),
            tracks,
        });
    }
    Ok(set)
}

/// Reads one track, or says why it cannot be exported.
fn read_track(
    reader: &Reader,
    row: library::TrackRow,
) -> Result<Result<ExportTrack, Skipped>, ExportError> {
    let Some(source) = reader.track_source(row.id)? else {
        return Ok(Err(Skipped {
            path: PathBuf::new(),
            why: "missing".into(),
        }));
    };
    let skip = |why: &str| {
        Ok(Err(Skipped {
            path: source.path.clone(),
            why: why.into(),
        }))
    };
    if row.missing || !source.path.is_file() {
        return skip("missing");
    }
    let Some(sample_rate) = row.sample_rate.filter(|&r| r > 0) else {
        return skip("unknown sample rate");
    };
    let grid = reader.beatgrid(row.id)?;
    let kind = FileKind::of(&source.path);
    // Only an MP3's header shifts anything; a file that fails to probe has no lead-in.
    let lead_in = if kind == FileKind::Mp3 {
        decode::lead_in(&source.path).unwrap_or_default()
    } else {
        LeadIn::default()
    };
    let title = row
        .title
        .clone()
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| {
            source
                .path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default()
        });
    Ok(Ok(ExportTrack {
        id: row.id,
        fingerprint: source.fingerprint,
        size: source.size,
        title,
        artist: row.artist,
        album: source.album,
        genre: source.genre,
        sample_rate,
        duration_frames: row.duration_frames,
        bpm: grid.and_then(|g| g.bpm).or(row.bpm),
        first_beat_frames: grid.and_then(|g| g.first_beat_frames),
        key: row
            .key
            .and_then(Key::new)
            .map(|k| k.standard().replace('\u{266F}', "#")),
        cue_frames: reader.cue(row.id)?,
        hotcues: reader.hotcue_points(row.id)?,
        kind,
        lead_in,
        path: source.path,
    }))
}
