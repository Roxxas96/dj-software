//! What an export would do, worked out without writing anything.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use super::layout::{Manifest, OJ_DIR, SPACE_MARGIN, stick_paths};
use super::{ExportError, ExportPlaylist, ExportSet, ExportTrack, FileKind, Skipped, free_space};

/// What happens to one track's file on the stick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackAction {
    /// Copy it: it is not on the stick, or what is there differs.
    Copy,
    /// Leave it: the same audio is already there.
    Keep,
}

/// One track with its place on the stick.
#[derive(Debug, Clone, PartialEq)]
pub struct PlannedTrack {
    pub track: ExportTrack,
    /// Relative to the stick's root, with `/` separators.
    pub stick_path: String,
    pub action: TrackAction,
}

/// Everything an export would do, and what it needs.
#[derive(Debug, Clone, PartialEq)]
pub struct ExportPlan {
    pub stick: PathBuf,
    pub tracks: Vec<PlannedTrack>,
    pub playlists: Vec<ExportPlaylist>,
    pub skipped: Vec<Skipped>,
    /// Bytes the copies will take.
    pub copy_bytes: u64,
    /// Free bytes on the stick; 0 when they cannot be read.
    pub free_bytes: u64,
    /// Tracks copied without Serato cues because their type carries no Serato tags.
    pub without_serato_cues: usize,
    /// Files an earlier export wrote that this one no longer uses, relative to `Open Jockey/`.
    pub stale_tracks: Vec<String>,
    /// The manifest an earlier export left on the stick.
    pub previous: Option<Manifest>,
}

impl ExportPlan {
    /// Whether the stick cannot hold the copies and the safety margin.
    #[must_use]
    pub fn too_small(&self) -> bool {
        self.copy_bytes + SPACE_MARGIN > self.free_bytes
    }
}

/// Works out what exporting `set` to `stick` would do. Writes nothing.
///
/// # Errors
///
/// [`ExportError::Refused`] when `stick` holds or is inside a library folder or the manifest
/// on it is damaged, [`ExportError::Io`] when `stick` cannot be read.
pub fn plan_export(
    set: ExportSet,
    stick: &Path,
    library_folders: &[PathBuf],
) -> Result<ExportPlan, ExportError> {
    refuse_library_folders(stick, library_folders)?;
    let previous = Manifest::read(stick)?;
    let paths = stick_paths(&set.tracks);
    let mut tracks = Vec::with_capacity(set.tracks.len());
    let mut copy_bytes = 0;
    let mut without_serato_cues = 0;
    for track in set.tracks {
        let stick_path = paths[&track.id].clone();
        let on_stick = stick_path
            .split('/')
            .fold(stick.to_path_buf(), |p, part| p.join(part));
        let unchanged = on_stick.is_file()
            && library::fingerprint::of_file(&on_stick).is_ok_and(|f| f == track.fingerprint);
        let action = if unchanged {
            TrackAction::Keep
        } else {
            copy_bytes += track.size;
            TrackAction::Copy
        };
        if matches!(track.kind, FileKind::Other(_)) {
            without_serato_cues += 1;
        }
        tracks.push(PlannedTrack {
            track,
            stick_path,
            action,
        });
    }
    let used: HashSet<&str> = tracks
        .iter()
        .map(|t| {
            t.stick_path
                .strip_prefix(OJ_DIR)
                .map_or(t.stick_path.as_str(), |s| s.trim_start_matches('/'))
        })
        .collect();
    let stale_tracks = previous
        .iter()
        .flat_map(|m| m.tracks.iter())
        .filter(|t| !used.contains(t.as_str()))
        .cloned()
        .collect();
    Ok(ExportPlan {
        stick: stick.to_path_buf(),
        tracks,
        playlists: set.playlists,
        skipped: set.skipped,
        copy_bytes,
        free_bytes: free_space(stick).unwrap_or(0),
        without_serato_cues,
        stale_tracks,
        previous,
    })
}

/// Refuses a stick that holds, is, or is inside a library folder.
fn refuse_library_folders(stick: &Path, folders: &[PathBuf]) -> Result<(), ExportError> {
    let canon_stick = dunce::canonicalize(stick).map_err(|source| ExportError::Io {
        path: stick.to_path_buf(),
        source,
    })?;
    for folder in folders {
        let Ok(canon) = dunce::canonicalize(folder) else {
            continue;
        };
        let why = if canon.starts_with(&canon_stick) {
            "holds"
        } else if canon_stick.starts_with(&canon) {
            "is inside"
        } else {
            continue;
        };
        return Err(ExportError::Refused(format!(
            "{} {why} the library folder {}: export to a USB stick, not the music drive",
            stick.display(),
            folder.display()
        )));
    }
    Ok(())
}
