//! What every stick format reads and writes, and the export run that ties them together.

use std::collections::{BTreeSet, HashSet};
use std::path::Path;

use super::layout::{Manifest, OJ_DIR, name_key};
use super::plan::TrackAction;
use super::rekordbox::RekordboxFormat;
use super::serato::tags::{needs_tagging, write_tags};
use super::serato::{SERATO_DIR, SUBCRATES, SeratoFormat, crate_names};
use super::virtualdj::{LISTS_DIR, VirtualDjFormat, list_names};
use super::{ExportError, ExportPlan, files};

/// What every format reads; each `PlannedTrack` carries its `stick_path`.
pub struct StickContext<'a> {
    pub plan: &'a ExportPlan,
    /// The time to record as "added", in Unix seconds.
    pub now_unix: u32,
}

/// What a format wrote, for the manifest.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FormatOutput {
    /// Names of the files written that belong to Open Jockey alone, for the manifest.
    pub files: Vec<String>,
    /// Things worth telling the user.
    pub notes: Vec<String>,
}

/// One app's library files on the stick.
pub trait StickFormat {
    /// The app's name, as shown to the user.
    fn name(&self) -> &'static str;

    /// Writes this app's library files for the whole plan (copies are already in place).
    ///
    /// # Errors
    ///
    /// [`ExportError`] when a file cannot be read or written.
    fn write_library(&self, cx: &StickContext<'_>) -> Result<FormatOutput, ExportError>;
}

/// What the stick's `Open Jockey/README.txt` says.
pub const README_TEXT: &str = "\
This stick was made by Open Jockey. Your playlists are on it for Serato, Rekordbox and VirtualDJ.

Serato DJ: plug the stick in. The playlists show as crates under the stick's name.
Rekordbox: open Preferences > Advanced > Database > rekordbox xml, choose
  Open Jockey/rekordbox.xml on this stick, then find the playlists under \"rekordbox xml\" in the
  browser and import them. On a computer where the stick has another name or drive letter,
  Rekordbox shows the tracks as missing: use Relocate on the Open Jockey/Music folder.
VirtualDJ: plug the stick in. The playlists show under the stick's Lists folder. If VirtualDJ
  says the drive's database is corrupted and offers to fix it, say yes: it only updates the
  drive letter, and your playlists and cues stay.

The music is in Open Jockey/Music. Exporting again from Open Jockey updates the stick.
";

/// Why an export to a stick whose free space the system will not tell is refused.
pub const FREE_SPACE_UNKNOWN: &str = "Open Jockey cannot read the stick's free space.";

/// How far an export has got.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExportProgress {
    /// Steps finished: one per track, then one for the library files.
    pub done: usize,
    /// The number of tracks plus 1.
    pub total: usize,
}

/// What an export did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExportReport {
    /// Tracks copied onto the stick.
    pub copied: usize,
    /// Tracks already on the stick and left as they were.
    pub kept: usize,
    /// Tracks already on the stick whose Serato tags were rewritten.
    pub retagged: usize,
    /// Copies from an earlier export that this one no longer uses, deleted.
    pub removed: usize,
    /// Playlists written.
    pub playlists: usize,
    /// Things worth telling the user.
    pub notes: Vec<String>,
}

/// `n` and the noun for it: `one` when `n` is 1, else `many`.
pub(super) fn count(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// Bytes as megabytes, for the user.
fn megabytes(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / 1_000_000.0)
}

/// Runs `plan`: copies the tracks, writes Serato's, Rekordbox's and VirtualDJ's library files,
/// deletes what an earlier export wrote and this one no longer uses, and writes the manifest
/// last. Each step leaves the stick usable, so an export that stops halfway leaves the earlier
/// one in place, and the next export picks up from there. `progress` is called after each
/// track and once at the end.
///
/// # Errors
///
/// [`ExportError::Refused`] when the stick is too small or its free space is unknown (nothing
/// is written) or a write would go through a link, [`ExportError::Io`] or
/// [`ExportError::Format`] when a file cannot be read or written.
pub fn run(
    plan: &ExportPlan,
    progress: &dyn Fn(ExportProgress),
) -> Result<ExportReport, ExportError> {
    let Some(free) = plan.free_bytes else {
        return Err(ExportError::Refused(FREE_SPACE_UNKNOWN.to_owned()));
    };
    if plan.too_small() {
        return Err(ExportError::Refused(format!(
            "the stick is too small: {} to copy, {} free",
            megabytes(plan.copy_bytes),
            megabytes(free)
        )));
    }
    let stick = plan.stick.as_path();
    // Only a real folder reached without links is cleaned: a linked one is not the stick's.
    if let Some((dir, meta)) = files::reach(stick, OJ_DIR)
        && meta.is_dir()
    {
        files::remove_leftovers(&dir)?;
    }

    // Before anything else is written, the manifest claims what this export is about to
    // write, besides what earlier ones did: an export that stops halfway then leaves no crate
    // or list that the next one takes for another app's, and no copy it cannot clean up.
    let tracks = manifest_tracks(plan);
    let previous = plan.previous.as_ref();
    let earlier = |field: fn(&Manifest) -> &Vec<String>| previous.map_or(&[][..], |m| field(m));
    Manifest {
        version: 1,
        playlists: union(earlier(|m| &m.playlists), &playlist_names(plan)),
        crates: union(earlier(|m| &m.crates), &crate_names(plan)),
        vdj_lists: union(earlier(|m| &m.vdj_lists), &list_names(plan)),
        tracks: union(earlier(|m| &m.tracks), &tracks),
    }
    .write(stick)?;

    let mut report = ExportReport::default();
    let total = plan.tracks.len() + 1;
    for (i, planned) in plan.tracks.iter().enumerate() {
        let track = &planned.track;
        let to = join(stick, &planned.stick_path);
        // A copy is read only as a regular file reached without links; anything else is
        // replaced by a fresh copy.
        let on_stick = match planned.action {
            TrackAction::Keep => files::regular_file(stick, &planned.stick_path),
            TrackAction::Copy => None,
        };
        match on_stick {
            Some(copy) if needs_tagging(&copy, track) => {
                files::copy_atomic(stick, &copy, &to, |part| write_tags(part, track))?;
                report.retagged += 1;
            }
            Some(_) => report.kept += 1,
            None => {
                files::copy_atomic(stick, &track.path, &to, |part| write_tags(part, track))?;
                report.copied += 1;
            }
        }
        progress(ExportProgress { done: i + 1, total });
    }

    let cx = StickContext {
        plan,
        now_unix: now_unix(),
    };
    let crates = SeratoFormat.write_library(&cx)?;
    let rekordbox = RekordboxFormat.write_library(&cx)?;
    let lists = VirtualDjFormat.write_library(&cx)?;

    if let Some(previous) = &plan.previous {
        let subcrates = format!("{SERATO_DIR}/{SUBCRATES}");
        for (dir, old, new) in [
            (subcrates.as_str(), &previous.crates, &crates.files),
            (LISTS_DIR, &previous.vdj_lists, &lists.files),
        ] {
            // A name that differs only in case is the file just written on most sticks.
            let new: HashSet<String> = new.iter().map(|n| name_key(n)).collect();
            for name in old.iter().filter(|n| !new.contains(&name_key(n))) {
                remove(stick, &format!("{dir}/{name}"))?;
            }
        }
    }

    let readme = stick.join(OJ_DIR).join("README.txt");
    files::write_atomic(stick, &readme, README_TEXT.as_bytes())?;

    let mut folders = BTreeSet::new();
    for stale in &plan.stale_tracks {
        if remove(stick, &format!("{OJ_DIR}/{stale}"))? {
            report.removed += 1;
        }
        // Every folder below Music that held the file; deleted deepest first below.
        let mut parts: Vec<&str> = stale.split('/').collect();
        parts.pop();
        while parts.len() > 1 {
            folders.insert(format!("{OJ_DIR}/{}", parts.join("/")));
            parts.pop();
        }
    }
    for folder in folders.iter().rev() {
        remove_empty_folder(stick, folder);
    }

    Manifest {
        version: 1,
        playlists: playlist_names(plan),
        crates: crates.files,
        vdj_lists: lists.files,
        tracks,
    }
    .write(stick)?;

    report.playlists = plan.playlists.len();
    report.notes = [crates.notes, rekordbox.notes, lists.notes].concat();
    if plan.without_serato_cues > 0 {
        report.notes.push(format!(
            "{} copied without Serato cues (not MP3 or WAV)",
            count(plan.without_serato_cues, "track", "tracks")
        ));
    }
    progress(ExportProgress { done: total, total });
    Ok(report)
}

/// The names of the playlists of `plan`, for the manifest.
fn playlist_names(plan: &ExportPlan) -> Vec<String> {
    plan.playlists.iter().map(|p| p.name.clone()).collect()
}

/// The copies of `plan`, relative to `Open Jockey/`, for the manifest.
fn manifest_tracks(plan: &ExportPlan) -> Vec<String> {
    plan.tracks
        .iter()
        .map(|t| {
            t.stick_path
                .strip_prefix(OJ_DIR)
                .map_or(t.stick_path.as_str(), |s| s.trim_start_matches('/'))
                .to_string()
        })
        .collect()
}

/// `old`, then each name of `new` not in it.
fn union(old: &[String], new: &[String]) -> Vec<String> {
    let mut all = old.to_vec();
    for name in new {
        if !all.contains(name) {
            all.push(name.clone());
        }
    }
    all
}

/// `relative` (with `/` separators) under `root`, one component at a time.
fn join(root: &Path, relative: &str) -> std::path::PathBuf {
    relative
        .split('/')
        .fold(root.to_path_buf(), |p, part| p.join(part))
}

fn now_unix() -> u32 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| u32::try_from(d.as_secs()).unwrap_or(u32::MAX))
}

/// Deletes the regular file at `relative` under `stick`, unless a link is on the way or it is
/// not a regular file. Says whether it deleted anything.
fn remove(stick: &Path, relative: &str) -> Result<bool, ExportError> {
    match files::reach(stick, relative) {
        Some((path, meta)) if meta.file_type().is_file() => files::remove_regular_file(&path),
        _ => Ok(false),
    }
}

/// Deletes the folder at `relative` under `stick` if it is empty and a real folder reached
/// without links. A folder that still holds something stays; so does one that cannot go.
fn remove_empty_folder(stick: &Path, relative: &str) {
    if let Some((path, meta)) = files::reach(stick, relative)
        && meta.is_dir()
    {
        let _ = std::fs::remove_dir(path);
    }
}
