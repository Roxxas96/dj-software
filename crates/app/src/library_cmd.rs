//! The `library` command: folders, scans, track lists, analysis, settings, playlists,
//! beatgrids and stems prepared ahead. Results go to stdout, progress lines to stderr.

use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use interop::export::{REKORDBOX_STEPS, TrackAction};
use interop::{ExportPlan, ExportProgress};
use library::{
    Analyzer, BPM_RANGE, GridEdit, GridError, KEY_NOTATION, Library, PlaylistSummary, Reader,
    ScanProgress, ScanReport, StemCache, StemKey, TrackFilter, TrackRow, TrackSort, TrackSortKey,
    WriteOp, WriteOutcome, default_threads, format_bpm_range, validate_bpm_range,
};
use stems::Separator;
use ui::ExportSummary;

use crate::format::{
    bpm_text, grid_text, is_long_mix, key_text, length_text, megabytes, notation_from_setting,
    notation_setting, truncate,
};
use crate::sticks::{Mounts, refuse_library_drive};
use crate::vdj::{Places, Scan, Scanned};

/// `println!` of one line with every control character in it (from library tags, file
/// names, error messages) replaced, line breaks included, see [`crate::format::safe`].
/// Multi-line text is printed a line at a time.
macro_rules! out {
    ($($arg:tt)*) => {
        println!("{}", crate::format::safe(format!($($arg)*)))
    };
}

/// [`out!`] on stderr.
macro_rules! err_out {
    ($($arg:tt)*) => {
        eprintln!("{}", crate::format::safe(format!($($arg)*)))
    };
}

/// How many skipped files a scan report lists.
const SKIPPED_SHOWN: usize = 20;
/// How often progress lines are redrawn.
const PROGRESS_EVERY: Duration = Duration::from_millis(200);
/// Playlist entries read per query.
const PLAYLIST_PAGE: usize = 500;

#[derive(Debug, clap::Args)]
pub struct LibraryArgs {
    /// Library database file (default: library.sqlite3 in the per-user data directory)
    #[arg(long, value_name = "PATH", global = true)]
    pub library_db: Option<PathBuf>,
    #[command(subcommand)]
    pub command: LibraryCommand,
}

#[derive(Debug, clap::Subcommand)]
pub enum LibraryCommand {
    /// Add a music folder to the library, then scan it
    AddFolder { dir: PathBuf },
    /// Remove a music folder from the library (its tracks and cues are kept)
    RemoveFolder { dir: PathBuf },
    /// Scan every library folder for new, changed, moved and missing files
    Scan,
    /// List tracks: id, artist, title, BPM, key, length, • for cues, and missing files.
    /// Tracks over 30 minutes (DJ mixes) get one constant beatgrid, which may not fit them
    List(ListArgs),
    /// Detect BPM, beatgrid, key and waveform. Without --all or --track, analyzes every
    /// track not analyzed yet
    Analyze {
        /// Same as the default (up-to-date tracks are skipped); use --force to redo
        /// every track
        #[arg(long, conflicts_with = "tracks")]
        all: bool,
        /// Only these track ids
        #[arg(long = "track", value_name = "ID", num_args = 1..)]
        tracks: Vec<i64>,
        /// Analyze again even if up to date (a locked beatgrid is kept)
        #[arg(long)]
        force: bool,
    },
    /// Import hotcues, saved loops and playlists from VirtualDJ. Without --apply, only
    /// prints what would be imported
    ImportVdj {
        /// Write the import; for a track that has hotcues here and in VirtualDJ, keep
        /// ours, replace them with VirtualDJ's, or merge (ours stay, VirtualDJ's fill
        /// the empty slots). Takes a library backup first
        #[arg(long, value_enum, value_name = "HOW")]
        apply: Option<ResolutionArg>,
        /// A VirtualDJ data folder (database.xml, MyLists); repeatable. Replaces the
        /// default folders. Library drives are always searched for VirtualDJ/database.xml
        #[arg(long = "vdj-dir", value_name = "DIR")]
        vdj_dir: Vec<PathBuf>,
    },
    /// Export playlists to a USB stick for Serato, Rekordbox and VirtualDJ. Copies the
    /// tracks (Serato cues written into the copies of MP3 and WAV files) and writes each
    /// program's library files under the stick's own folders. Without --write, only prints
    /// what would be done
    Export {
        /// The stick (its mount point or drive); never a drive holding a library folder
        #[arg(long, value_name = "DIR")]
        to: PathBuf,
        /// A playlist to export, by exact name; repeatable
        #[arg(long = "playlist", value_name = "NAME", num_args = 1.., required = true)]
        playlists: Vec<String>,
        /// Write the export. Exporting again copies only new or changed tracks
        #[arg(long)]
        write: bool,
    },
    /// Change a library setting
    #[command(subcommand)]
    Set(SetCommand),
    /// Create and edit playlists (positions are 1-based)
    #[command(subcommand)]
    Playlist(PlaylistCommand),
    /// Show or correct a track's beatgrid (every change locks it against re-analysis)
    Grid {
        /// Track id
        track: i64,
        #[command(subcommand)]
        action: GridAction,
    },
    /// Separate tracks into stems ahead of time and keep them in the stems cache, where
    /// they count as prepared (kept longest). Needs the model: run `open-jockey stems
    /// setup` once. Tracks over 20 minutes are skipped
    Stems {
        /// Every track of this playlist
        #[arg(
            long,
            value_name = "NAME",
            conflicts_with = "tracks",
            required_unless_present = "tracks"
        )]
        playlist: Option<String>,
        /// These track ids
        #[arg(long = "track", value_name = "ID", num_args = 1..)]
        tracks: Vec<i64>,
        /// Configuration file, for the cache budget `stems.cache_gb` (default: config.toml
        /// in the per-user configuration folder)
        #[arg(long, value_name = "PATH")]
        config: Option<PathBuf>,
    },
}

#[derive(Debug, clap::Args)]
pub struct ListArgs {
    /// Only the tracks of this playlist, in playlist order (unless --sort)
    #[arg(long, value_name = "NAME", conflicts_with = "folder")]
    pub playlist: Option<String>,
    /// Only the tracks under this folder
    #[arg(long, value_name = "DIR")]
    pub folder: Option<PathBuf>,
    /// Only tracks whose title or artist contains this text (case-insensitive)
    #[arg(long, value_name = "TEXT")]
    pub search: Option<String>,
    /// Sort order (default: artist; playlist order for --playlist)
    #[arg(long, value_enum)]
    pub sort: Option<SortArg>,
    /// Show at most this many tracks
    #[arg(long, value_name = "N", default_value_t = 100)]
    pub limit: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum SortArg {
    Title,
    Artist,
    Bpm,
    Key,
    Added,
}

impl SortArg {
    fn key(self) -> TrackSortKey {
        match self {
            Self::Title => TrackSortKey::Title,
            Self::Artist => TrackSortKey::Artist,
            Self::Bpm => TrackSortKey::Bpm,
            Self::Key => TrackSortKey::Key,
            Self::Added => TrackSortKey::Added,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum ResolutionArg {
    Keep,
    Replace,
    Merge,
}

impl ResolutionArg {
    fn resolution(self) -> interop::Resolution {
        match self {
            Self::Keep => interop::Resolution::Keep,
            Self::Replace => interop::Resolution::Replace,
            Self::Merge => interop::Resolution::Merge,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Keep => "keep",
            Self::Replace => "replace",
            Self::Merge => "merge",
        }
    }
}

#[derive(Debug, PartialEq, clap::Subcommand)]
pub enum SetCommand {
    /// The tempo range detected BPMs are folded into (default 90-180). A tempo outside is
    /// doubled or halved until it fits: with 90-180, 85 BPM reads as 170 and 185 as
    /// 92.5. The range must span an octave (MAX >= 2 x MIN) within 30-480. Ranges below
    /// ~70 BPM let half-time tracks (dubstep) read as half tempo. Applies to new
    /// analyses; run `library analyze --force` to redo analyzed tracks
    BpmRange { min: f64, max: f64 },
    /// How keys are shown: camelot (8A), standard (Am) or open-key (1m)
    KeyNotation {
        #[arg(value_enum)]
        notation: NotationArg,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum NotationArg {
    Camelot,
    Standard,
    OpenKey,
}

impl NotationArg {
    fn notation(self) -> analysis::KeyNotation {
        match self {
            Self::Camelot => analysis::KeyNotation::Camelot,
            Self::Standard => analysis::KeyNotation::Standard,
            Self::OpenKey => analysis::KeyNotation::OpenKey,
        }
    }
}

#[derive(Debug, PartialEq, Eq, clap::Subcommand)]
pub enum PlaylistCommand {
    /// Create an empty playlist
    Create { name: String },
    /// Rename a playlist
    Rename { name: String, new_name: String },
    /// Delete a playlist (its tracks stay in the library)
    Delete { name: String },
    /// Add tracks (by id) to a playlist, at the end or at a position
    Add {
        name: String,
        #[arg(value_name = "TRACK", required = true)]
        tracks: Vec<i64>,
        /// 1-based position to insert at
        #[arg(long, value_name = "POS")]
        at: Option<usize>,
    },
    /// Remove the entry at a 1-based position
    Remove { name: String, position: usize },
    /// Move an entry from one 1-based position to another
    Move {
        name: String,
        from: usize,
        to: usize,
    },
    /// List the playlists, or the tracks of one
    List { name: Option<String> },
}

#[derive(Debug, Clone, Copy, PartialEq, clap::Subcommand)]
pub enum GridAction {
    /// Print the beatgrid
    Show,
    /// Move every beat by this many milliseconds (negative is earlier)
    Shift {
        #[arg(allow_negative_numbers = true)]
        ms: f64,
    },
    /// Set the BPM (40 to 300), keeping the first beat
    Bpm { bpm: f64 },
    /// Halve the BPM
    Halve,
    /// Double the BPM
    Double,
    /// Lock the grid so re-analysis keeps it
    Lock,
    /// Unlock the grid so re-analysis may replace it
    Unlock,
}

/// The library file: `--library-db`, or library.sqlite3 in the per-user data directory.
pub fn db_path(explicit: Option<&Path>) -> anyhow::Result<PathBuf> {
    if let Some(path) = explicit {
        return Ok(path.to_owned());
    }
    crate::legacy_dirs::migrate();
    let dirs = directories::ProjectDirs::from("", "", "open-jockey")
        .context("finding the per-user data directory (use --library-db)")?;
    Ok(dirs.data_dir().join("library.sqlite3"))
}

/// Opens the library at `explicit` or the default path.
pub fn open_library(explicit: Option<&Path>) -> anyhow::Result<Library> {
    let path = db_path(explicit)?;
    Library::open(&path).with_context(|| format!("opening the library {}", path.display()))
}

pub fn run(args: &LibraryArgs) -> anyhow::Result<()> {
    let lib = open_library(args.library_db.as_deref())?;
    let result = match &args.command {
        LibraryCommand::AddFolder { dir } => add_folder(&lib, dir),
        LibraryCommand::RemoveFolder { dir } => {
            library::remove_folder(&lib, dir)
                .with_context(|| format!("removing {}", dir.display()))?;
            out!(
                "removed {} from the library folders (its tracks and cues are kept)",
                dir.display()
            );
            Ok(())
        }
        LibraryCommand::Scan => scan_all(&lib),
        LibraryCommand::List(list) => list_tracks(&lib, list),
        LibraryCommand::Analyze { tracks, force, .. } => analyze(&lib, tracks, *force),
        LibraryCommand::ImportVdj { apply, vdj_dir } => import_vdj(&lib, *apply, vdj_dir),
        LibraryCommand::Export {
            to,
            playlists,
            write,
        } => export(&lib, to, playlists, *write),
        LibraryCommand::Set(set) => set_setting(&lib, set),
        LibraryCommand::Playlist(cmd) => playlist(&lib, cmd),
        LibraryCommand::Grid { track, action } => grid(&lib, *track, *action),
        LibraryCommand::Stems {
            playlist,
            tracks,
            config,
        } => crate::config::load(config.as_deref()).and_then(|config| {
            prepare_stems(
                &lib,
                &StemsRequest {
                    playlist: playlist.as_deref(),
                    tracks,
                    model: &stems::model_path(),
                    budget_bytes: gb_to_bytes(config.stems.cache_gb),
                },
            )
        }),
    };
    for failed in lib.take_failed_writes() {
        err_out!("warning: a library change was not saved: {}", failed.error);
    }
    result
}

/// Exports the playlists named `names` to the stick `to`, printing the plan and, with
/// `write`, the result.
fn export(lib: &Library, to: &Path, names: &[String], write: bool) -> anyhow::Result<()> {
    let tty = std::io::stderr().is_terminal();
    let progress = |p: ExportProgress| {
        if tty {
            eprint!("\rexporting {}/{}", p.done, p.total);
        }
    };
    let result = export_lines(lib, to, names, write, &Mounts::usual(), &progress);
    if tty {
        eprintln!();
    }
    for line in result? {
        out!("{line}");
    }
    Ok(())
}

/// The playlist ids for `names`, each name matched exactly and to one playlist.
fn export_playlist_ids(reader: &Reader, names: &[String]) -> anyhow::Result<Vec<i64>> {
    let all = reader.playlists()?;
    let mut ids = Vec::new();
    for name in names {
        let matching: Vec<&PlaylistSummary> = all.iter().filter(|p| &p.name == name).collect();
        match matching.as_slice() {
            [] => {
                let known: Vec<&str> = all.iter().map(|p| p.name.as_str()).collect();
                bail!(
                    "Unknown playlist \"{name}\". Playlists: {}",
                    known.join(", ")
                );
            }
            [one] => {
                if !ids.contains(&one.id) {
                    ids.push(one.id);
                }
            }
            several => {
                let listed: Vec<String> = several.iter().map(|p| format!("id {}", p.id)).collect();
                bail!(
                    "Several playlists are named \"{name}\" ({}); rename one first",
                    listed.join(", ")
                );
            }
        }
    }
    Ok(ids)
}

/// Plans (and with `write`, runs) the export of the playlists `names` to `to`, refusing a
/// `to` on the drive of a library folder (drives are found under `mounts`), and returns
/// the lines to print. A stick that is too small, or whose free space is unknown, is an
/// error, and nothing is written.
fn export_lines(
    lib: &Library,
    to: &Path,
    names: &[String],
    write: bool,
    mounts: &Mounts,
    progress: &dyn Fn(ExportProgress),
) -> anyhow::Result<Vec<String>> {
    let reader = lib.reader()?;
    let ids = export_playlist_ids(&reader, names)?;
    let folders = reader.folders()?;
    refuse_library_drive(to, &folders, mounts)?;
    let set = interop::gather(&reader, &ids)?;
    let plan = interop::plan_export(set, to, &folders)?;
    let free = plan.free_bytes.map_or_else(|| "?".to_owned(), megabytes);
    let summary = export_summary(to.display().to_string(), &plan, free);
    if let Some(why) = summary.refusal() {
        bail!(why);
    }
    let mut lines = summary.lines().to_vec();
    if !write {
        lines.push("Nothing written. Run again with --write to export.".to_owned());
        return Ok(lines);
    }
    let report = interop::run(&plan, progress)?;
    lines.push(format!(
        "Exported: {} copied, {} kept, {} re-tagged, {} removed.",
        report.copied, report.kept, report.retagged, report.removed
    ));
    lines.push(
        "To open them in Rekordbox (also in IMPORT TO REKORDBOX.txt on the stick):".to_owned(),
    );
    lines.extend(REKORDBOX_STEPS.lines().map(str::to_owned));
    lines.extend(report.notes);
    Ok(lines)
}

/// What exporting `plan` to the stick called `stick` would do, as `library export` and the
/// window show it; `free` is the stick's free space as shown.
pub(crate) fn export_summary(stick: String, plan: &ExportPlan, free: String) -> ExportSummary {
    let to_copy = plan
        .tracks
        .iter()
        .filter(|t| t.action == TrackAction::Copy)
        .count();
    ExportSummary {
        stick,
        playlists: plan.playlists.len(),
        tracks: plan.tracks.len(),
        to_copy,
        copy_size: megabytes(plan.copy_bytes),
        kept: plan.tracks.len() - to_copy,
        free,
        without_serato_cues: plan.without_serato_cues,
        skipped: plan.skipped.len(),
        to_remove: plan.stale_tracks.len(),
        too_small: plan.too_small(),
        free_unknown: plan.free_bytes.is_none(),
    }
}

/// How many unmatched songs the import summary lists.
const UNMATCHED_SHOWN: usize = 20;

fn import_vdj(
    lib: &Library,
    apply: Option<ResolutionArg>,
    vdj_dirs: &[PathBuf],
) -> anyhow::Result<()> {
    let places = if vdj_dirs.is_empty() {
        Places::usual()
    } else {
        Places::with_homes(vdj_dirs.to_vec())
    };
    for line in vdj_import(lib, apply, &places)? {
        out!("{line}");
    }
    Ok(())
}

/// Plans (and with `apply`, writes) the VirtualDJ import from `places`, returning the
/// summary lines.
fn vdj_import(
    lib: &Library,
    apply: Option<ResolutionArg>,
    places: &Places,
) -> anyhow::Result<Vec<String>> {
    let reader = lib.reader()?;
    let scanned = match crate::vdj::scan(&reader, places)? {
        Scan::Found(scanned) => scanned,
        Scan::Nothing { searched } => {
            let searched = if searched.is_empty() {
                "nowhere (pass --vdj-dir)".to_owned()
            } else {
                let names: Vec<String> = searched.iter().map(|d| d.display().to_string()).collect();
                names.join(", ")
            };
            return Ok(vec![format!("no VirtualDJ data found in: {searched}")]);
        }
    };
    let mut lines = vdj_summary(&scanned, apply);
    if let Some(how) = apply {
        let resolution = how.resolution();
        let report = interop::apply(lib, &scanned.plan, |_| resolution)?;
        lines.push(format!("  backup: {}", report.backup.display()));
        lines.push(format!(
            "  written: {} tracks, {} playlists",
            report.tracks_changed, report.playlists_created
        ));
    }
    Ok(lines)
}

/// The import summary printed before anything is written.
fn vdj_summary(scanned: &Scanned, apply: Option<ResolutionArg>) -> Vec<String> {
    let counts = scanned.summary();
    let mut lines = vec![match apply {
        None => {
            "VirtualDJ import (dry run: nothing written; add --apply keep|replace|merge)".to_owned()
        }
        Some(how) => format!(
            "VirtualDJ import ({} for tracks with cues here)",
            how.name()
        ),
    }];
    lines.push(format!("  databases: {}", counts.databases));
    lines.extend(
        scanned
            .sources
            .databases
            .iter()
            .map(|d| format!("    {}", d.display())),
    );
    lines.push(format!(
        "  tracks matched: {}, with hotcues or loops: {}, conflicting with cues here: {}",
        counts.tracks, counts.with_marks, counts.conflicting
    ));
    lines.push(format!(
        "  hotcues: {}, saved loops: {}, dropped over 8 slots: {}, other markers not imported: {}",
        counts.hotcues, counts.loops, counts.dropped, counts.other_markers
    ));
    lines.push(format!(
        "  playlists: {} to create, {} already here",
        counts.playlists_new, counts.playlists_here
    ));
    if !scanned.skipped.is_empty() {
        lines.push(format!("  playlists skipped: {}", counts.playlists_skipped));
        lines.extend(scanned.skipped.iter().map(|e| format!("    {e}")));
    }
    let unmatched = &scanned.plan.unmatched;
    lines.push(format!("  not in the library: {}", counts.unmatched));
    lines.extend(
        unmatched
            .iter()
            .take(UNMATCHED_SHOWN)
            .map(|p| format!("    {}", p.display())),
    );
    if unmatched.len() > UNMATCHED_SHOWN {
        lines.push(format!(
            "    ... and {} more",
            unmatched.len() - UNMATCHED_SHOWN
        ));
    }
    lines
}

fn add_folder(lib: &Library, dir: &Path) -> anyhow::Result<()> {
    library::add_folder(lib, dir).with_context(|| format!("adding {}", dir.display()))?;
    out!("added folder {}", dir.display());
    scan_one(lib, dir)
}

fn scan_all(lib: &Library) -> anyhow::Result<()> {
    let folders = lib.reader()?.folders()?;
    if folders.is_empty() {
        out!("no library folders; add one with `open-jockey library add-folder <DIR>`");
    }
    for folder in folders {
        if !folder.exists() {
            out!(
                "{}: not found (unplugged drive?), skipped; its tracks are kept",
                folder.display()
            );
            continue;
        }
        scan_one(lib, &folder)?;
    }
    Ok(())
}

fn scan_one(lib: &Library, dir: &Path) -> anyhow::Result<()> {
    let tty = std::io::stderr().is_terminal();
    let mut last = Instant::now();
    let mut progress = |p: ScanProgress| {
        if tty && last.elapsed() >= PROGRESS_EVERY {
            last = Instant::now();
            eprint!(
                "\rscanning {}: {} files",
                crate::format::safe(dir.display()),
                p.scanned
            );
            let _ = std::io::stderr().flush();
        }
    };
    let report = library::scan_folder(lib, dir, &mut progress)
        .with_context(|| format!("scanning {}", dir.display()))?;
    if tty {
        eprint!("\r\x1b[2K");
    }
    out!("{}:", dir.display());
    for line in scan_report_text(&report).lines() {
        out!("{line}");
    }
    Ok(())
}

/// A scan report: the counts, then up to [`SKIPPED_SHOWN`] skipped files.
fn scan_report_text(report: &ScanReport) -> String {
    let mut text = format!(
        "  added {}, updated {}, audio changed {}, relinked {}, missing {}, \
         ignored {} non-audio file(s)\n",
        report.added,
        report.updated,
        report.audio_changed,
        report.relinked,
        report.missing,
        report.ignored
    );
    if !report.skipped.is_empty() {
        text.push_str(&format!("  skipped {} file(s):\n", report.skipped.len()));
        for (path, why) in report.skipped.iter().take(SKIPPED_SHOWN) {
            text.push_str(&format!(
                "    {}: {}\n",
                crate::format::safe(path.display()),
                crate::format::safe(why)
            ));
        }
        if report.skipped.len() > SKIPPED_SHOWN {
            text.push_str(&format!(
                "    … and {} more\n",
                report.skipped.len() - SKIPPED_SHOWN
            ));
        }
    }
    text
}

fn list_tracks(lib: &Library, args: &ListArgs) -> anyhow::Result<()> {
    let reader = lib.reader()?;
    let notation = notation_from_setting(reader.setting(KEY_NOTATION)?.as_deref());
    let (rows, total, positions) = match &args.playlist {
        Some(name) => {
            let playlist = find_playlist(&reader, name)?;
            let mut entries: Vec<(usize, TrackRow)> = playlist_rows(&reader, &playlist)?
                .into_iter()
                .enumerate()
                .map(|(i, r)| (i + 1, r))
                .filter(|(_, r)| matches_search(r, args.search.as_deref()))
                .collect();
            if let Some(sort) = args.sort {
                entries.sort_by(|a, b| compare(&a.1, &b.1, sort.key()));
            }
            let total = entries.len();
            entries.truncate(args.limit);
            let (positions, rows): (Vec<usize>, Vec<TrackRow>) = entries.into_iter().unzip();
            (rows, total, Some(positions))
        }
        None => {
            let folder = args
                .folder
                .as_deref()
                .map(|f| dunce::canonicalize(f).with_context(|| format!("{}", f.display())))
                .transpose()?;
            let filter = TrackFilter {
                folder,
                text: args.search.clone(),
                ..TrackFilter::default()
            };
            let sort = TrackSort {
                key: args.sort.map_or(TrackSortKey::Artist, SortArg::key),
                descending: false,
            };
            let rows = reader.tracks(&filter, sort, 0, args.limit)?;
            (rows, reader.track_count(&filter)?, None)
        }
    };
    for line in track_table(&rows, positions.as_deref(), notation).lines() {
        out!("{line}");
    }
    if rows.len() < total {
        out!(
            "showing {} of {total} tracks (use --limit to see more)",
            rows.len()
        );
    } else {
        out!("{total} track(s)");
    }
    if rows.iter().any(is_long_mix) {
        out!(
            "note: tracks over 30 minutes (DJ mixes) get one constant beatgrid, which may \
             not fit them"
        );
    }
    Ok(())
}

/// Every entry of `playlist`, read a page at a time.
fn playlist_rows(reader: &Reader, playlist: &PlaylistSummary) -> anyhow::Result<Vec<TrackRow>> {
    let mut rows = Vec::new();
    loop {
        let page = reader.playlist_tracks(playlist.id, rows.len(), PLAYLIST_PAGE)?;
        let done = page.len() < PLAYLIST_PAGE;
        rows.extend(page);
        if done {
            return Ok(rows);
        }
    }
}

fn matches_search(row: &TrackRow, search: Option<&str>) -> bool {
    let Some(text) = search.filter(|t| !t.is_empty()) else {
        return true;
    };
    let text = text.to_lowercase();
    [&row.title, &row.artist].iter().any(|field| {
        field
            .as_deref()
            .is_some_and(|f| f.to_lowercase().contains(&text))
    })
}

/// Orders playlist rows like the library sorts (unknown values first, ties by id; added
/// follows the id, which grows as tracks are added).
fn compare(a: &TrackRow, b: &TrackRow, key: TrackSortKey) -> std::cmp::Ordering {
    let lower = |s: &Option<String>| s.as_deref().map(str::to_lowercase);
    let order = match key {
        TrackSortKey::Title => lower(&a.title).cmp(&lower(&b.title)),
        TrackSortKey::Artist => lower(&a.artist).cmp(&lower(&b.artist)),
        TrackSortKey::Bpm => a
            .bpm
            .partial_cmp(&b.bpm)
            .unwrap_or(std::cmp::Ordering::Equal),
        TrackSortKey::Key => a.key.cmp(&b.key),
        TrackSortKey::Added => std::cmp::Ordering::Equal,
    };
    order.then(a.id.cmp(&b.id))
}

/// The track table, with a 1-based position column for playlists.
fn track_table(
    rows: &[TrackRow],
    positions: Option<&[usize]>,
    notation: analysis::KeyNotation,
) -> String {
    let mut text = String::new();
    if positions.is_some() {
        text.push_str("   # ");
    }
    text.push_str(&format!(
        "{:>6}  {:<24}  {:<32}  {:>7}  {:>4}  {:>7}  CUE\n",
        "ID", "ARTIST", "TITLE", "BPM", "KEY", "LENGTH"
    ));
    for (i, row) in rows.iter().enumerate() {
        if let Some(positions) = positions {
            text.push_str(&format!("{:>4} ", positions.get(i).copied().unwrap_or(0)));
        }
        let artist = truncate(row.artist.as_deref().unwrap_or(""), 24);
        let title = truncate(row.title.as_deref().unwrap_or("(untitled)"), 32);
        let line = format!(
            "{:>6}  {artist:<24}  {title:<32}  {:>7}  {:>4}  {:>7}  {:<3}{}",
            row.id,
            row.bpm.map(bpm_text).unwrap_or_default(),
            key_text(row.key, notation),
            length_text(row),
            if row.has_cues { " •" } else { "" },
            if row.missing { "  missing" } else { "" },
        );
        text.push_str(line.trim_end());
        text.push('\n');
    }
    text
}

fn analyze(lib: &Library, tracks: &[i64], force: bool) -> anyhow::Result<()> {
    let ids = if tracks.is_empty() {
        lib.reader()?.track_ids_to_analyze(force)?
    } else {
        tracks.to_vec()
    };
    if ids.is_empty() {
        out!("nothing to analyze: every track is up to date (--force analyzes again)");
        return Ok(());
    }
    let analyzer = Analyzer::start(lib, default_threads()).context("starting the analyzer")?;
    analyzer.enqueue(&ids, force);
    let total = ids.len();
    let tty = std::io::stderr().is_terminal();
    loop {
        let p = analyzer.progress();
        let finished = p.done + p.skipped + p.failed_count;
        if tty {
            eprint!("\r{}", progress_bar(finished.min(total), total));
            let _ = std::io::stderr().flush();
        }
        if p.queued == 0 {
            break;
        }
        std::thread::sleep(PROGRESS_EVERY);
    }
    if tty {
        eprintln!();
    }
    let p = analyzer.progress();
    out!(
        "analyzed {}, already up to date {}, failed {}",
        p.done,
        p.skipped,
        p.failed_count
    );
    for (id, why) in analyzer.take_failures() {
        out!("  track {id}: {why}");
    }
    for warning in analyzer.take_warnings() {
        out!("  warning: {warning}");
    }
    Ok(())
}

fn progress_bar(done: usize, total: usize) -> String {
    const WIDTH: usize = 24;
    let filled = (done * WIDTH).checked_div(total).unwrap_or(WIDTH);
    format!(
        "analyzing [{}{}] {done}/{total}",
        "#".repeat(filled),
        "-".repeat(WIDTH - filled)
    )
}

fn set_setting(lib: &Library, set: &SetCommand) -> anyhow::Result<()> {
    let (key, value, message) = match set {
        SetCommand::BpmRange { min, max } => {
            let (range, warning) = validate_bpm_range(&format!("{min}-{max}"))
                .map_err(|e| anyhow::anyhow!("invalid BPM range {min}-{max}: {e}"))?;
            if let Some(warning) = warning {
                err_out!("warning: {warning}");
            }
            let value = format_bpm_range(range);
            let message = format!(
                "BPM range set to {value}; it applies to new analyses \
                 (`library analyze --force` redoes analyzed tracks)"
            );
            (BPM_RANGE, value, message)
        }
        SetCommand::KeyNotation { notation } => {
            let value = notation_setting(notation.notation()).to_owned();
            let message = format!("keys are now shown in {value} notation");
            (KEY_NOTATION, value, message)
        }
    };
    lib.writer().submit_wait(WriteOp::SetSetting {
        key: key.to_owned(),
        value: Some(value),
    })?;
    out!("{message}");
    Ok(())
}

/// The playlist named `name` (or, if no playlist has that name, with that id).
fn find_playlist(reader: &Reader, name: &str) -> anyhow::Result<PlaylistSummary> {
    let all = reader.playlists()?;
    let named: Vec<&PlaylistSummary> = all.iter().filter(|p| p.name == name).collect();
    match named.as_slice() {
        [one] => Ok((*one).clone()),
        [] => name
            .parse::<i64>()
            .ok()
            .and_then(|id| all.iter().find(|p| p.id == id).cloned())
            .with_context(|| format!("no playlist named \"{name}\" (see `library playlist list`)")),
        several => {
            let ids: Vec<String> = several.iter().map(|p| p.id.to_string()).collect();
            bail!(
                "several playlists are named \"{name}\" (ids {}); use the id instead",
                ids.join(", ")
            )
        }
    }
}

/// A 1-based position within `len` entries as 0-based.
fn position(pos: usize, len: usize, allow_end: bool) -> anyhow::Result<usize> {
    let max = if allow_end { len + 1 } else { len };
    if pos == 0 || pos > max {
        bail!(
            "position {pos} is out of range: the playlist has {len} entries (positions start at 1)"
        );
    }
    Ok(pos - 1)
}

fn playlist(lib: &Library, cmd: &PlaylistCommand) -> anyhow::Result<()> {
    let reader = lib.reader()?;
    let writer = lib.writer();
    match cmd {
        PlaylistCommand::Create { name } => {
            if reader.playlists()?.iter().any(|p| &p.name == name) {
                bail!("a playlist named \"{name}\" already exists");
            }
            let outcome = writer.submit_wait(WriteOp::CreatePlaylist { name: name.clone() })?;
            match outcome {
                WriteOutcome::Created { id } => out!("created playlist \"{name}\" (id {id})"),
                _ => out!("created playlist \"{name}\""),
            }
        }
        PlaylistCommand::Rename { name, new_name } => {
            let p = find_playlist(&reader, name)?;
            writer.submit_wait(WriteOp::RenamePlaylist {
                id: p.id,
                name: new_name.clone(),
            })?;
            out!("renamed \"{}\" to \"{new_name}\"", p.name);
        }
        PlaylistCommand::Delete { name } => {
            let p = find_playlist(&reader, name)?;
            writer.submit_wait(WriteOp::DeletePlaylist(p.id))?;
            out!(
                "deleted playlist \"{}\" ({} entries; the tracks stay in the library)",
                p.name,
                p.track_count
            );
        }
        PlaylistCommand::Add { name, tracks, at } => {
            let p = find_playlist(&reader, name)?;
            let len = p.track_count as usize;
            let start = at.map(|pos| position(pos, len, true)).transpose()?;
            let ops = tracks
                .iter()
                .enumerate()
                .map(|(i, &track)| WriteOp::AddToPlaylist {
                    playlist: p.id,
                    track,
                    at: start.map(|s| s + i),
                })
                .collect();
            writer.submit_wait(WriteOp::Batch(ops))?;
            out!("added {} track(s) to \"{}\"", tracks.len(), p.name);
        }
        PlaylistCommand::Remove {
            name,
            position: pos,
        } => {
            let p = find_playlist(&reader, name)?;
            let at = position(*pos, p.track_count as usize, false)?;
            writer.submit_wait(WriteOp::RemoveFromPlaylist {
                playlist: p.id,
                position: at,
            })?;
            out!("removed entry {pos} from \"{}\"", p.name);
        }
        PlaylistCommand::Move { name, from, to } => {
            let p = find_playlist(&reader, name)?;
            let len = p.track_count as usize;
            let (from0, to0) = (position(*from, len, false)?, position(*to, len, false)?);
            writer.submit_wait(WriteOp::MoveInPlaylist {
                playlist: p.id,
                from: from0,
                to: to0,
            })?;
            out!("moved entry {from} to {to} in \"{}\"", p.name);
        }
        PlaylistCommand::List { name: None } => {
            let all = reader.playlists()?;
            if all.is_empty() {
                out!("no playlists; create one with `library playlist create <NAME>`");
            }
            for p in all {
                out!(
                    "{:>4}  {}  ({} tracks, {} without cues)",
                    p.id,
                    p.name,
                    p.track_count,
                    p.unprepared_count
                );
            }
        }
        PlaylistCommand::List { name: Some(name) } => {
            return list_tracks(
                lib,
                &ListArgs {
                    playlist: Some(name.clone()),
                    folder: None,
                    search: None,
                    sort: None,
                    limit: usize::MAX,
                },
            );
        }
    }
    Ok(())
}

fn grid(lib: &Library, track: i64, action: GridAction) -> anyhow::Result<()> {
    let reader = lib.reader()?;
    let details = reader
        .track(track)?
        .with_context(|| format!("track {track} does not exist"))?;
    let name = crate::format::track_name(&details.row);
    let rate = details.row.sample_rate;
    let edit = match action {
        GridAction::Show => {
            let text = reader
                .beatgrid(track)?
                .map_or_else(|| "not analyzed yet".to_owned(), |g| grid_text(&g, rate));
            out!("{track}  {name}: {text}");
            return Ok(());
        }
        GridAction::Shift { ms } => {
            let rate = rate.ok_or(library::LibraryError::Grid(GridError::NoSampleRate))?;
            GridEdit::Shift {
                frames: ms / 1000.0 * f64::from(rate),
            }
        }
        GridAction::Bpm { bpm } => GridEdit::SetBpm(bpm),
        GridAction::Halve => GridEdit::Halve,
        GridAction::Double => GridEdit::Double,
        GridAction::Lock | GridAction::Unlock => {
            let want = action == GridAction::Lock;
            let locked = reader.beatgrid(track)?.is_some_and(|g| g.grid_locked);
            if locked == want {
                out!(
                    "{track}  {name}: already {}",
                    if want { "locked" } else { "unlocked" }
                );
                return Ok(());
            }
            GridEdit::ToggleLock
        }
    };
    let grid = lib
        .edit_grid(track, edit, None)
        .with_context(|| format!("editing the beatgrid of track {track}"))?;
    out!("{track}  {name}: {}", grid_text(&grid, rate));
    Ok(())
}

/// The stems model could not be opened: `library stems` stops there.
#[derive(Debug)]
struct ModelOpenFailed(stems::SeparateError);

impl std::fmt::Display for ModelOpenFailed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "opening the stems model: {}", self.0)
    }
}

impl std::error::Error for ModelOpenFailed {}

/// What `library stems` prepares, and with what.
struct StemsRequest<'a> {
    playlist: Option<&'a str>,
    tracks: &'a [i64],
    /// The filled model (`stems setup` writes it).
    model: &'a Path,
    budget_bytes: u64,
}

/// `cache_gb` gigabytes (10^9 bytes) as bytes.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub(crate) fn gb_to_bytes(gb: f64) -> u64 {
    // `as` saturates; the config keeps `cache_gb` within 0.5-1000.
    (gb * 1e9) as u64
}

/// The stems cache folder for the library at `db`: `stems` beside it, which is
/// `<data dir>/stems` for the default library.
pub(crate) fn stems_dir(db: &Path) -> PathBuf {
    db.parent().unwrap_or_else(|| Path::new(".")).join("stems")
}

/// How one track of `library stems` went.
enum Prepared {
    Separated,
    AlreadyCached,
    Skipped(String),
}

/// `library stems`: separates the requested tracks one at a time and stores them in the
/// cache as prepared; tracks already cached are only marked prepared. Ctrl-C stops after
/// the current chunk. Fails without a model, or if any track failed.
fn prepare_stems(lib: &Library, request: &StemsRequest<'_>) -> anyhow::Result<()> {
    if !request.model.is_file() {
        bail!(
            "no stems model at {}: run `open-jockey stems setup` once to download and \
             install it",
            request.model.display()
        );
    }
    let reader = lib.reader()?;
    let ids = match request.playlist {
        Some(name) => {
            let playlist = find_playlist(&reader, name)?;
            let mut ids: Vec<i64> = Vec::new();
            for row in playlist_rows(&reader, &playlist)? {
                if !ids.contains(&row.id) {
                    ids.push(row.id);
                }
            }
            ids
        }
        None => request.tracks.to_vec(),
    };
    if ids.is_empty() {
        out!("no tracks to prepare");
        return Ok(());
    }
    let mut cache = StemCache::new(lib, stems_dir(lib.path()), request.budget_bytes)
        .context("opening the stems cache")?;
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let cancel = Arc::clone(&cancel);
        // Only fails if a handler is already set; Ctrl-C then just ends the process (a
        // half-written entry is swept the next time the cache opens).
        let _ = ctrlc::set_handler(move || cancel.store(true, Ordering::Relaxed));
    }
    let mut separator: Option<stems::ModelSeparator> = None;
    let (mut separated, mut cached, mut skipped, mut failed) = (0, 0, 0, 0);
    for (i, &id) in ids.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            out!("stopped: {} track(s) not prepared", ids.len() - i);
            break;
        }
        let prefix = format!("[{}/{}] track {id}", i + 1, ids.len());
        let result = prepare_one(
            &reader,
            &mut cache,
            id,
            &prefix,
            &cancel,
            &mut |separator: &mut Option<stems::ModelSeparator>| {
                if separator.is_none() {
                    err_out!("opening the stems model (the first time takes about a minute)");
                    *separator = Some(
                        stems::ModelSeparator::new(request.model, &stems::ModelOptions::default())
                            .map_err(ModelOpenFailed)?,
                    );
                }
                Ok(())
            },
            &mut separator,
        );
        match result {
            Ok(Prepared::Separated) => {
                separated += 1;
                out!("{prefix}: stems ready");
            }
            Ok(Prepared::AlreadyCached) => {
                cached += 1;
                out!("{prefix}: already cached, marked prepared");
            }
            Ok(Prepared::Skipped(why)) => {
                skipped += 1;
                out!("{prefix}: skipped, {why}");
            }
            // The model will not open for the next track either.
            Err(e) if e.is::<ModelOpenFailed>() => return Err(e),
            Err(e) if cancel.load(Ordering::Relaxed) => {
                out!("{prefix}: stopped ({e:#})");
                break;
            }
            Err(e) => {
                failed += 1;
                out!("{prefix}: failed: {e:#}");
            }
        }
    }
    out!(
        "stems: separated {separated}, already cached {cached}, skipped {skipped}, failed \
         {failed}; cache {:.1} of {:.1} GB",
        cache.bytes()? as f64 / 1e9,
        request.budget_bytes as f64 / 1e9
    );
    if failed > 0 {
        bail!("{failed} track(s) could not be prepared");
    }
    Ok(())
}

/// Opens the separator into its slot if it is not open yet.
type OpenSeparator<'a, S> = dyn FnMut(&mut Option<S>) -> anyhow::Result<()> + 'a;

/// Prepares one track: decodes it, keeps an existing entry, or separates and stores it,
/// opening the separator (`open` into `separator`) only when one is needed.
fn prepare_one<S: Separator>(
    reader: &Reader,
    cache: &mut StemCache,
    id: i64,
    prefix: &str,
    cancel: &AtomicBool,
    open: &mut OpenSeparator<'_, S>,
    separator: &mut Option<S>,
) -> anyhow::Result<Prepared> {
    let Some(details) = reader.track(id)? else {
        return Ok(Prepared::Skipped("no such track".into()));
    };
    if details.row.missing {
        return Ok(Prepared::Skipped(format!(
            "its file is missing ({})",
            details.path.display()
        )));
    }
    let name = crate::format::track_name(&details.row);
    let fingerprint = cache
        .fingerprint(id)?
        .with_context(|| format!("track {id} has no fingerprint"))?;
    if let (Some(frames), Some(rate)) = (details.row.duration_frames, details.row.sample_rate)
        && let Err(e) = stems::check_length(usize::try_from(frames).unwrap_or(usize::MAX), rate)
    {
        return Ok(Prepared::Skipped(format!("{name}: {e}")));
    }
    let decoded = decode::decode_file(&details.path)
        .with_context(|| format!("decoding {}", details.path.display()))?;
    let track = decoded.track;
    if let Err(e) = stems::check_length(track.frames(), track.sample_rate()) {
        return Ok(Prepared::Skipped(format!("{name}: {e}")));
    }
    let key = StemKey {
        track: id,
        fingerprint: &fingerprint,
        sample_rate: track.sample_rate(),
        frames: track.frames(),
    };
    if cache.contains(&key) {
        cache.mark_prepared(id)?;
        return Ok(Prepared::AlreadyCached);
    }
    open(separator)?;
    let separator = separator.as_mut().context("no stems separator")?;
    let tty = std::io::stderr().is_terminal();
    let shown = std::cell::Cell::new(None);
    let progress = |done: f32| {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let percent = (done.clamp(0.0, 1.0) * 100.0) as u32;
        if tty && shown.replace(Some(percent)) != Some(percent) {
            eprint!(
                "\r{prefix} {}: separating {percent:3} %",
                truncate(&name, 40)
            );
            let _ = std::io::stderr().flush();
        }
    };
    let result = separator.separate(&track, &progress, cancel);
    if tty {
        eprint!("\r\x1b[2K");
    }
    let set = result?;
    let eviction = cache.put(&key, &set, true, &[])?;
    if let Some(notice) = eviction.notice {
        err_out!("{notice}");
    }
    if eviction.failed > 0 {
        err_out!(
            "stems cache: {} entry(ies) could not be removed; they are retried later",
            eviction.failed
        );
    }
    if !eviction.removed.is_empty() {
        err_out!(
            "stems cache: evicted {} track(s) to stay within the budget",
            eviction.removed.len()
        );
    }
    Ok(Prepared::Separated)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: i64, bpm: Option<f64>) -> TrackRow {
        TrackRow {
            id,
            title: Some(format!("Song {id}")),
            artist: Some("Artist".into()),
            bpm,
            key: Some(21),
            duration_frames: Some(44_100 * 200),
            sample_rate: Some(44_100),
            has_cues: id == 1,
            missing: id == 2,
        }
    }

    #[test]
    fn table_shows_bpm_key_cues_and_missing() {
        let text = track_table(
            &[row(1, Some(128.0)), row(2, None)],
            None,
            analysis::KeyNotation::Camelot,
        );
        let lines: Vec<&str> = text.lines().collect();
        assert!(lines[0].contains("BPM") && lines[0].contains("KEY"));
        assert!(lines[1].contains("128.00") && lines[1].contains("8A"));
        assert!(lines[1].contains("3:20") && lines[1].ends_with('•'));
        assert!(lines[2].ends_with("missing") && !lines[2].contains('•'));
    }

    #[test]
    fn scan_report_lists_twenty_skipped_files() {
        let report = ScanReport {
            added: 3,
            skipped: (0..25)
                .map(|i| (PathBuf::from(format!("/m/{i}.mp3")), "bad".to_owned()))
                .collect(),
            ..ScanReport::default()
        };
        let text = scan_report_text(&report);
        assert!(text.contains("added 3"));
        assert!(text.contains("skipped 25 file(s)"));
        assert!(text.contains("/m/19.mp3") && !text.contains("/m/20.mp3"));
        assert!(text.contains("… and 5 more"));
    }

    #[test]
    fn stems_without_a_model_explain_setup() {
        let dir = tempfile::tempdir().unwrap();
        let lib = Library::open(&dir.path().join("library.sqlite3")).unwrap();
        let err = prepare_stems(
            &lib,
            &StemsRequest {
                playlist: None,
                tracks: &[1],
                model: &dir.path().join("no-model.onnx"),
                budget_bytes: u64::MAX,
            },
        )
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("open-jockey stems setup"),
            "{err:#}"
        );
        assert!(
            !dir.path().join("stems").exists(),
            "nothing done without a model"
        );
    }

    /// Writes a 16-bit stereo PCM WAV of `seconds` of a tone at 44.1 kHz.
    fn write_wav(path: &Path, seconds: f64) {
        const SR: u32 = 44_100;
        let frames = (seconds * f64::from(SR)) as usize;
        let data_len = u32::try_from(frames * 4).unwrap();
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes()); // PCM
        bytes.extend_from_slice(&2u16.to_le_bytes()); // channels
        bytes.extend_from_slice(&SR.to_le_bytes());
        bytes.extend_from_slice(&(SR * 4).to_le_bytes()); // byte rate
        bytes.extend_from_slice(&4u16.to_le_bytes()); // block align
        bytes.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&data_len.to_le_bytes());
        for i in 0..frames {
            let s = (3000.0 * (i as f64 * 0.05).sin()) as i16;
            bytes.extend_from_slice(&s.to_le_bytes());
            bytes.extend_from_slice(&s.to_le_bytes());
        }
        std::fs::write(path, bytes).unwrap();
    }

    #[test]
    fn a_model_that_will_not_open_stops_the_run() {
        let dir = tempfile::tempdir().unwrap();
        let music = dir.path().join("music");
        std::fs::create_dir_all(&music).unwrap();
        write_wav(&music.join("a.wav"), 0.5);
        write_wav(&music.join("b.wav"), 0.6);
        let lib = Library::open(&dir.path().join("library.sqlite3")).unwrap();
        library::add_folder(&lib, &music).unwrap();
        library::scan_folder(&lib, &music, &mut |_| {}).unwrap();
        let model = dir.path().join("broken.onnx");
        std::fs::write(&model, b"not a model").unwrap();
        let err = prepare_stems(
            &lib,
            &StemsRequest {
                playlist: None,
                tracks: &[1, 2],
                model: &model,
                budget_bytes: u64::MAX,
            },
        )
        .unwrap_err();
        assert!(err.is::<ModelOpenFailed>(), "{err:#}");
    }

    #[test]
    fn stems_are_prepared_once_then_marked() {
        let dir = tempfile::tempdir().unwrap();
        let music = dir.path().join("music");
        std::fs::create_dir_all(&music).unwrap();
        write_wav(&music.join("a.wav"), 1.0);
        let lib = Library::open(&dir.path().join("library.sqlite3")).unwrap();
        library::add_folder(&lib, &music).unwrap();
        library::scan_folder(&lib, &music, &mut |_| {}).unwrap();
        let reader = lib.reader().unwrap();
        let mut cache = StemCache::new(&lib, stems_dir(lib.path()), u64::MAX).unwrap();
        let cancel = AtomicBool::new(false);
        let mut opened = 0;
        let mut open = |slot: &mut Option<stems::SyntheticSeparator>| {
            if slot.is_none() {
                opened += 1;
                *slot = Some(stems::SyntheticSeparator::new());
            }
            Ok(())
        };
        let mut separator = None;
        let mut run = |id| {
            prepare_one(
                &reader,
                &mut cache,
                id,
                "",
                &cancel,
                &mut open,
                &mut separator,
            )
            .unwrap()
        };
        assert!(matches!(run(1), Prepared::Separated));
        assert!(matches!(run(1), Prepared::AlreadyCached));
        assert!(matches!(run(2), Prepared::Skipped(why) if why.contains("no such track")));
        drop(run);
        assert_eq!(opened, 1);
        let prepared: bool = reader
            .connection()
            .query_row("SELECT prepared FROM stems WHERE track_id = 1", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert!(prepared);
        assert!(dir.path().join("stems").is_dir());
    }

    #[test]
    fn positions_are_one_based() {
        assert_eq!(position(1, 3, false).unwrap(), 0);
        assert!(position(0, 3, false).is_err());
        assert!(position(4, 3, false).is_err());
        assert_eq!(position(4, 3, true).unwrap(), 3);
        assert_eq!(
            progress_bar(0, 0),
            format!("analyzing [{}] 0/0", "#".repeat(24))
        );
        assert!(progress_bar(12, 24).contains("############------------] 12/24"));
    }

    #[test]
    fn playlist_sort_and_search() {
        let mut rows = [row(3, Some(130.0)), row(1, None), row(2, Some(120.0))];
        rows.sort_by(|a, b| compare(a, b, TrackSortKey::Bpm));
        assert_eq!(rows.map(|r| r.id), [1, 2, 3]);
        assert!(matches_search(&row(1, None), Some("song 1")));
        assert!(matches_search(&row(1, None), Some("ARTIST")));
        assert!(!matches_search(&row(1, None), Some("nope")));
    }

    mod export {
        use super::*;

        /// A library with one scanned wav in `<temp>/music`, in the playlist "Mix", and an
        /// empty stick `<temp>/volumes/STICK`.
        fn export_fixture() -> (tempfile::TempDir, Library, PathBuf) {
            export_fixture_in(&["music"])
        }

        /// [`export_fixture`] with the music in `<temp>/<music...>`.
        fn export_fixture_in(music: &[&str]) -> (tempfile::TempDir, Library, PathBuf) {
            let dir = tempfile::tempdir().unwrap();
            let music = music
                .iter()
                .fold(dir.path().to_path_buf(), |p, c| p.join(c));
            let stick = dir.path().join("volumes").join("STICK");
            std::fs::create_dir_all(&music).unwrap();
            std::fs::create_dir_all(&stick).unwrap();
            write_wav(&music.join("a.wav"), 1.0);
            let lib = Library::open(&dir.path().join("library.sqlite3")).unwrap();
            library::add_folder(&lib, &music).unwrap();
            library::scan_folder(&lib, &music, &mut |_| {}).unwrap();
            let id = match lib
                .writer()
                .submit_wait(WriteOp::CreatePlaylist { name: "Mix".into() })
                .unwrap()
            {
                WriteOutcome::Created { id } => id,
                other => panic!("{other:?}"),
            };
            let track: i64 = lib
                .reader()
                .unwrap()
                .connection()
                .query_row("SELECT id FROM tracks", [], |r| r.get(0))
                .unwrap();
            lib.writer()
                .submit_wait(WriteOp::AddToPlaylist {
                    playlist: id,
                    track,
                    at: None,
                })
                .unwrap();
            (dir, lib, stick)
        }

        /// Asserts that the stick the export touches is inside `root`.
        fn assert_inside(stick: &Path, root: &Path) {
            assert!(
                dunce::canonicalize(stick)
                    .unwrap()
                    .starts_with(dunce::canonicalize(root).unwrap()),
                "{} is outside the temp dir",
                stick.display()
            );
        }

        /// `<temp>/volumes` stands for `/Volumes`: no real drive is looked at.
        fn mounts(dir: &tempfile::TempDir) -> Mounts {
            Mounts {
                dirs: vec![dunce::canonicalize(dir.path().join("volumes")).unwrap()],
                drive_letters: false,
            }
        }

        #[test]
        fn export_refuses_the_drive_that_holds_the_library() {
            let (dir, lib, _) = export_fixture_in(&["volumes", "LIB", "music"]);
            let to = dir.path().join("volumes").join("LIB").join("out");
            std::fs::create_dir_all(&to).unwrap();
            assert_inside(&to, dir.path());
            let err = export_lines(&lib, &to, &mix(), true, &mounts(&dir), &|_| {}).unwrap_err();
            let folder = lib.reader().unwrap().folders().unwrap().remove(0);
            assert_eq!(
                format!("{err:#}"),
                format!(
                    "{} is on the drive that holds the library folder {}: export to a USB \
                     stick, not the music drive",
                    to.display(),
                    folder.display()
                )
            );
            assert_eq!(std::fs::read_dir(&to).unwrap().count(), 0);
        }

        #[test]
        fn export_refuses_a_folder_on_the_system_drive_with_the_library() {
            let (dir, lib, _) = export_fixture();
            let to = dir.path().join("elsewhere");
            std::fs::create_dir_all(&to).unwrap();
            let err = export_lines(&lib, &to, &mix(), true, &mounts(&dir), &|_| {}).unwrap_err();
            assert!(
                format!("{err:#}").contains("is on the drive that holds the library folder"),
                "{err:#}"
            );
            assert_eq!(std::fs::read_dir(&to).unwrap().count(), 0);
        }

        fn mix() -> Vec<String> {
            vec!["Mix".to_owned()]
        }

        #[test]
        fn export_dry_run_writes_nothing() {
            let (dir, lib, stick) = export_fixture();
            assert_inside(&stick, dir.path());
            let lines = export_lines(&lib, &stick, &mix(), false, &mounts(&dir), &|_| {}).unwrap();
            assert_eq!(lines[0], format!("Stick: {}", stick.display()));
            assert_eq!(lines[1], "Playlists: 1, tracks: 1");
            assert!(
                lines[2].starts_with("To copy: 1 track (")
                    && lines[2].ends_with("), already on the stick: 0"),
                "{lines:?}"
            );
            assert!(lines[3].starts_with("Free on the stick: "), "{lines:?}");
            assert_eq!(lines[4], "Copied without Serato cues (not MP3 or WAV): 0");
            assert_eq!(lines[5], "Skipped (missing or unreadable): 0");
            assert_eq!(lines[6], "Removed from the stick (no longer exported): 0");
            assert_eq!(
                lines[7],
                "Nothing written. Run again with --write to export."
            );
            assert_eq!(lines.len(), 8);
            assert_eq!(std::fs::read_dir(&stick).unwrap().count(), 0);
        }

        #[test]
        fn export_write_makes_the_stick() {
            let (dir, lib, stick) = export_fixture();
            assert_inside(&stick, dir.path());
            let lines = export_lines(&lib, &stick, &mix(), true, &mounts(&dir), &|_| {}).unwrap();
            assert!(stick.join("Open Jockey").join("export.toml").is_file());
            assert!(
                lines.contains(&"Exported: 1 copied, 0 kept, 0 re-tagged, 0 removed.".to_owned()),
                "{lines:?}"
            );
            let heading =
                "To open them in Rekordbox (also in IMPORT TO REKORDBOX.txt on the stick):";
            let at = lines.iter().position(|l| l == heading).unwrap();
            assert!(lines[at - 1].starts_with("Exported: "), "{lines:?}");
            assert!(lines[at + 1].starts_with("1. In Rekordbox,"), "{lines:?}");
            // Again: the track is already there.
            let again = export_lines(&lib, &stick, &mix(), true, &mounts(&dir), &|_| {}).unwrap();
            assert!(again[2].ends_with("already on the stick: 1"), "{again:?}");
        }

        #[test]
        fn export_refuses_a_library_folder() {
            let (dir, lib, _) = export_fixture();
            let music = dir.path().join("music");
            assert_inside(&music, dir.path());
            let err = export_lines(&lib, &music, &mix(), true, &mounts(&dir), &|_| {}).unwrap_err();
            assert!(!format!("{err:#}").is_empty());
            assert_eq!(
                std::fs::read_dir(&music).unwrap().count(),
                1,
                "nothing added"
            );
        }

        #[test]
        fn unknown_free_space_is_refused_with_its_own_reason() {
            let (dir, lib, stick) = export_fixture();
            assert_inside(&stick, dir.path());
            let reader = lib.reader().unwrap();
            let set = interop::gather(&reader, &[1]).unwrap();
            let mut plan = interop::plan_export(set, &stick, &reader.folders().unwrap()).unwrap();
            plan.free_bytes = None;
            let summary = export_summary("S".to_owned(), &plan, "?".to_owned());
            assert!(!summary.too_small && summary.free_unknown, "{summary:?}");
            assert_eq!(
                summary.refusal().as_deref(),
                Some("Open Jockey cannot read the stick's free space.")
            );
            // The window and the CLI say what the export run itself refuses with.
            assert_eq!(
                summary.refusal().as_deref(),
                Some(interop::export::FREE_SPACE_UNKNOWN)
            );
            plan.free_bytes = Some(0);
            let summary = export_summary("S".to_owned(), &plan, "0.0 MB".to_owned());
            assert_eq!(summary.refusal(), Some(summary.too_small_line()));
            plan.free_bytes = Some(u64::MAX);
            let summary = export_summary("S".to_owned(), &plan, "lots".to_owned());
            assert_eq!(summary.refusal(), None);
        }

        #[test]
        fn export_unknown_playlist_lists_names() {
            let (dir, lib, stick) = export_fixture();
            assert_inside(&stick, dir.path());
            let err = export_lines(
                &lib,
                &stick,
                &["Nope".to_owned()],
                true,
                &mounts(&dir),
                &|_| {},
            )
            .unwrap_err();
            assert_eq!(
                format!("{err:#}"),
                "Unknown playlist \"Nope\". Playlists: Mix"
            );
            assert_eq!(std::fs::read_dir(&stick).unwrap().count(), 0);
        }
    }

    mod import_vdj {
        use super::*;

        /// A library with one scanned wav, and a VirtualDJ folder whose database gives that
        /// track a hotcue and a loop.
        fn vdj_fixture() -> (tempfile::TempDir, Library, PathBuf, PathBuf) {
            let dir = tempfile::tempdir().unwrap();
            let music = dir.path().join("music");
            std::fs::create_dir_all(&music).unwrap();
            write_wav(&music.join("a.wav"), 1.0);
            let lib = Library::open(&dir.path().join("library.sqlite3")).unwrap();
            library::add_folder(&lib, &music).unwrap();
            library::scan_folder(&lib, &music, &mut |_| {}).unwrap();
            let vdj = dir.path().join("vdj");
            std::fs::create_dir_all(&vdj).unwrap();
            let xml = format!(
                "<VirtualDJ_Database Version=\"2025\">\n\
                 <Song FilePath=\"{}\" FileSize=\"1\">\n\
                 <Scan Bpm=\"0.5\" />\n\
                 <Poi Pos=\"0.25\" Num=\"1\" Type=\"cue\" />\n\
                 <Poi Pos=\"0.5\" Num=\"2\" Type=\"loop\" Size=\"0.25\" />\n\
                 </Song>\n\
                 <Song FilePath=\"{}\" FileSize=\"1\" />\n\
                 </VirtualDJ_Database>",
                music.join("a.wav").display(),
                music.join("gone.wav").display()
            );
            std::fs::write(vdj.join("database.xml"), xml).unwrap();
            (dir, lib, vdj, music)
        }

        /// Looks in `vdj`, and for drives under `mounts`. Canonical, as library folders are
        /// stored (/private/var on macOS): otherwise a library folder in the temp dir is on
        /// the filesystem root's drive, and the real `/VirtualDJ` would be read.
        fn places(vdj: &Path, mounts: &Path) -> Places {
            Places {
                homes: vec![vdj.to_path_buf()],
                mounts: mounts.canonicalize().unwrap(),
            }
        }

        /// Asserts that every place `places` makes the import look in is inside `root`.
        fn assert_looks_only_in(lib: &Library, places: &Places, root: &Path) {
            let root = root.canonicalize().unwrap();
            let volumes = crate::vdj::volumes(&lib.reader().unwrap(), places).unwrap();
            assert!(!volumes.is_empty());
            for place in places.homes.iter().chain(&volumes) {
                let place = place.canonicalize().unwrap_or_else(|_| place.clone());
                assert!(
                    place.starts_with(&root),
                    "{} is outside the temp dir",
                    place.display()
                );
            }
        }

        fn hotcues_of(lib: &Library) -> [Option<library::HotcuePoint>; 8] {
            lib.reader().unwrap().hotcue_points(1).unwrap()
        }

        #[test]
        fn dry_run_prints_the_summary_and_writes_nothing() {
            let (dir, lib, vdj, music) = vdj_fixture();
            let places = places(&vdj, dir.path());
            assert_looks_only_in(&lib, &places, dir.path());
            let lines = vdj_import(&lib, None, &places).unwrap();
            assert_eq!(
                lines[0],
                "VirtualDJ import (dry run: nothing written; add --apply keep|replace|merge)"
            );
            assert_eq!(lines[1], "  databases: 1");
            assert_eq!(
                lines[2],
                format!("    {}", vdj.join("database.xml").display())
            );
            assert_eq!(
                lines[3],
                "  tracks matched: 1, with hotcues or loops: 1, conflicting with cues here: 0"
            );
            assert_eq!(
                lines[4],
                "  hotcues: 1, saved loops: 1, dropped over 8 slots: 0, other markers not imported: 0"
            );
            assert_eq!(lines[5], "  playlists: 0 to create, 0 already here");
            assert_eq!(lines[6], "  not in the library: 1");
            assert_eq!(
                lines[7],
                format!("    {}", music.join("gone.wav").display())
            );
            assert_eq!(lines.len(), 8);
            assert!(hotcues_of(&lib).iter().all(Option::is_none));
            assert!(
                !dir.path().join("backups").exists(),
                "no backup for a dry run"
            );
        }

        #[test]
        fn apply_merge_writes_and_reports_the_backup() {
            let (dir, lib, vdj, _) = vdj_fixture();
            let places = places(&vdj, dir.path());
            assert_looks_only_in(&lib, &places, dir.path());
            let lines = vdj_import(&lib, Some(ResolutionArg::Merge), &places).unwrap();
            assert_eq!(
                lines[0],
                "VirtualDJ import (merge for tracks with cues here)"
            );
            let n = lines.len();
            assert!(lines[n - 2].starts_with("  backup: "), "{lines:?}");
            assert!(dir.path().join("backups").is_dir());
            assert_eq!(lines[n - 1], "  written: 1 tracks, 0 playlists");
            let slots = hotcues_of(&lib);
            assert!(slots[0].is_some() && slots[1].as_ref().unwrap().loop_frames.is_some());
        }

        #[test]
        fn a_playlist_that_cannot_be_read_is_listed_as_skipped() {
            let (dir, lib, vdj, _) = vdj_fixture();
            let lists = vdj.join("MyLists");
            std::fs::create_dir_all(&lists).unwrap();
            let broken = lists.join("Broken.vdjfolder");
            std::fs::write(&broken, "<VirtualFolder><song path=").unwrap();
            let places = places(&vdj, dir.path());
            assert_looks_only_in(&lib, &places, dir.path());
            let lines = vdj_import(&lib, None, &places).unwrap();
            let at = lines
                .iter()
                .position(|l| l == "  playlists skipped: 1")
                .unwrap_or_else(|| panic!("{lines:?}"));
            assert!(lines[at - 1].starts_with("  playlists: "), "{lines:?}");
            assert!(
                lines[at + 1].starts_with("    ")
                    && lines[at + 1].contains(&broken.display().to_string()),
                "{lines:?}"
            );
            assert!(
                lines[at + 2].starts_with("  not in the library: "),
                "{lines:?}"
            );
        }

        #[test]
        fn no_virtualdj_data_says_so() {
            let (dir, lib, _, _) = vdj_fixture();
            let empty = dir.path().join("empty");
            std::fs::create_dir_all(&empty).unwrap();
            // Library folders are stored canonical (/private/var on macOS).
            let mounts = dunce::canonicalize(dir.path()).unwrap();
            let lines = vdj_import(&lib, None, &places(&empty, &mounts)).unwrap();
            let music_volume = mounts.join("music").join("VirtualDJ");
            assert_eq!(
                lines,
                [format!(
                    "no VirtualDJ data found in: {}, {}",
                    empty.display(),
                    music_volume.display()
                )]
            );
        }
    }
}
