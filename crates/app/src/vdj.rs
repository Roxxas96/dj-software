//! Where VirtualDJ keeps its data, and reading it into an import plan: shared by
//! `library import-vdj` and the window's import.

use std::path::{Path, PathBuf};

use anyhow::Context;
use interop::{ImportPlan, InteropError, PlaylistAction, VdjSources};
use library::Reader;

/// Where mounted drives appear on macOS.
const VOLUMES_DIR: &str = "/Volumes";

/// Where to look for VirtualDJ's data.
#[derive(Debug, Clone)]
pub(crate) struct Places {
    /// VirtualDJ data folders (with `database.xml`, `MyLists`).
    pub homes: Vec<PathBuf>,
    /// Where drives are mounted, see [`volume_of`].
    pub mounts: PathBuf,
}

impl Places {
    /// VirtualDJ's own folders in the user's home, and the drives under `/Volumes`.
    pub fn usual() -> Self {
        Self::with_homes(default_vdj_dirs())
    }

    /// `homes`, and the drives under `/Volumes`.
    pub fn with_homes(homes: Vec<PathBuf>) -> Self {
        Self {
            homes,
            mounts: PathBuf::from(VOLUMES_DIR),
        }
    }
}

/// What a look for VirtualDJ's data found.
pub(crate) enum Scan {
    Found(Scanned),
    /// No database or playlist in any of these folders.
    Nothing {
        searched: Vec<PathBuf>,
    },
}

/// VirtualDJ's data, read and planned against the library.
pub(crate) struct Scanned {
    pub sources: VdjSources,
    /// The playlists that could not be read.
    pub skipped: Vec<InteropError>,
    pub plan: ImportPlan,
}

/// VirtualDJ's own folders in the user's home: where it keeps the local database.
fn default_vdj_dirs() -> Vec<PathBuf> {
    let Some(base) = directories::BaseDirs::new() else {
        return Vec::new();
    };
    let documents = directories::UserDirs::new()
        .and_then(|u| u.document_dir().map(Path::to_path_buf))
        .unwrap_or_else(|| base.home_dir().join("Documents"));
    if cfg!(target_os = "macos") {
        vec![
            base.home_dir()
                .join("Library/Application Support/VirtualDJ"),
            documents.join("VirtualDJ"),
        ]
    } else if cfg!(windows) {
        vec![documents.join("VirtualDJ")]
    } else {
        Vec::new()
    }
}

/// The drive holding `folder`: `<mounts>/<name>` when it is under `mounts`, else the
/// filesystem root. Both are compared by canonical path, so a short or symlinked spelling
/// still matches; the drive comes back in canonical form.
fn volume_of(folder: &Path, mounts: &Path) -> PathBuf {
    let canonical = |path: &Path| dunce::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let (folder, mounts) = (canonical(folder), canonical(mounts));
    if let Ok(rest) = folder.strip_prefix(&mounts)
        && let Some(name) = rest.components().next()
    {
        return mounts.join(name);
    }
    folder
        .ancestors()
        .last()
        .map_or_else(|| folder.clone(), Path::to_path_buf)
}

/// The drives of the library's folders, each once: where a per-drive database is looked
/// for, in `<drive>/VirtualDJ`.
pub(crate) fn volumes(reader: &Reader, places: &Places) -> anyhow::Result<Vec<PathBuf>> {
    let mut volumes: Vec<PathBuf> = Vec::new();
    for folder in reader.folders()? {
        let volume = volume_of(&folder, &places.mounts);
        if !volumes.contains(&volume) {
            volumes.push(volume);
        }
    }
    Ok(volumes)
}

/// Reads VirtualDJ's databases and playlists from `places` and the drives of the
/// library's folders, and plans the import against the library `reader` reads. Writes
/// nothing.
pub(crate) fn scan(reader: &Reader, places: &Places) -> anyhow::Result<Scan> {
    let volumes = volumes(reader, places)?;
    let sources = interop::find_sources(&places.homes, &volumes);
    if sources.databases.is_empty() && sources.list_dirs.is_empty() {
        let searched = places
            .homes
            .iter()
            .cloned()
            .chain(volumes.iter().map(|v| v.join("VirtualDJ")))
            .collect();
        return Ok(Scan::Nothing { searched });
    }
    let mut songs = Vec::new();
    for database in &sources.databases {
        // A per-drive database names its files from the drive's root.
        let drive_root = database
            .parent()
            .filter(|dir| dir.file_name().is_some_and(|n| n == "VirtualDJ"))
            .and_then(Path::parent)
            .filter(|root| volumes.iter().any(|v| v == root));
        songs.extend(
            interop::read_database(database, drive_root)
                .with_context(|| format!("reading {}", database.display()))?,
        );
    }
    let (playlists, skipped) =
        interop::read_playlists(&sources.list_dirs, sources.order.as_deref())
            .context("reading the VirtualDJ playlists")?;
    let plan = interop::plan_virtualdj(reader, &songs, &playlists)?;
    Ok(Scan::Found(Scanned {
        sources,
        skipped,
        plan,
    }))
}

impl Scanned {
    /// The counts shown before anything is written.
    pub fn summary(&self) -> ui::ImportSummary {
        let plan = &self.plan;
        let points = || plan.tracks.iter().flat_map(|t| &t.incoming).flatten();
        let loops = points().filter(|p| p.loop_frames.is_some()).count();
        let playlists_new = plan
            .playlists
            .iter()
            .filter(|p| p.action == PlaylistAction::Create)
            .count();
        ui::ImportSummary {
            databases: self.sources.databases.len(),
            tracks: plan.tracks.len(),
            with_marks: plan
                .tracks
                .iter()
                .filter(|t| t.incoming.iter().any(Option::is_some))
                .count(),
            conflicting: plan.tracks.iter().filter(|t| t.conflicts()).count(),
            hotcues: points().count() - loops,
            loops,
            dropped: plan.dropped,
            other_markers: plan.other_markers,
            playlists_new,
            playlists_here: plan.playlists.len() - playlists_new,
            playlists_skipped: self.skipped.len(),
            unmatched: plan.unmatched.len(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_drive_is_the_mount_holding_the_folder() {
        let mounts = Path::new("/Volumes");
        assert_eq!(
            volume_of(Path::new("/Volumes/Mix/music/a"), mounts),
            Path::new("/Volumes/Mix")
        );
        assert_eq!(
            volume_of(Path::new("/Users/me/music"), mounts),
            Path::new("/")
        );
    }

    #[test]
    fn a_folder_is_matched_to_its_drive_by_canonical_path() {
        let temp = tempfile::tempdir().unwrap();
        let folder = temp.path().join("Mix/music");
        std::fs::create_dir_all(&folder).unwrap();
        // The mounts are canonical; the folder keeps the temp dir's own spelling, which
        // differs on macOS (/var vs /private/var) and Windows (8.3 short names).
        let mounts = dunce::canonicalize(temp.path()).unwrap();
        assert_eq!(volume_of(&folder, &mounts), mounts.join("Mix"));
    }
}
