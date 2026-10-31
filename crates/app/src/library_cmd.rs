//! The `library` command: folders, scans, track lists, analysis, settings, playlists and
//! beatgrids. Results go to stdout, progress lines to stderr.

use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use library::{
    Analyzer, BPM_RANGE, GridEdit, GridError, KEY_NOTATION, Library, PlaylistSummary, Reader,
    ScanProgress, ScanReport, TrackFilter, TrackRow, TrackSort, TrackSortKey, WriteOp,
    WriteOutcome, default_threads, format_bpm_range, validate_bpm_range,
};

use crate::format::{
    bpm_text, grid_text, is_long_mix, key_text, length_text, notation_from_setting,
    notation_setting, truncate,
};

/// `println!` with control characters in the text (library tags, file names) replaced,
/// see [`crate::format::sanitize_lines`].
macro_rules! out {
    ($($arg:tt)*) => {
        println!("{}", crate::format::sanitize_lines(&format!($($arg)*)))
    };
}

/// [`out!`] on stderr.
macro_rules! err_out {
    ($($arg:tt)*) => {
        eprintln!("{}", crate::format::sanitize_lines(&format!($($arg)*)))
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
        /// Every track (those already analyzed are skipped unless --force)
        #[arg(long, conflicts_with = "tracks")]
        all: bool,
        /// Only these track ids
        #[arg(long = "track", value_name = "ID", num_args = 1..)]
        tracks: Vec<i64>,
        /// Analyze again even if up to date (a locked beatgrid is kept)
        #[arg(long)]
        force: bool,
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

#[derive(Debug, PartialEq, clap::Subcommand)]
pub enum SetCommand {
    /// The tempo range detected BPMs are folded into (default 90-180). A tempo outside is
    /// doubled or halved until it fits: with 90-180, 85 BPM reads as 170 and 185 as
    /// 92.5. The range must span an octave (MAX >= 2 x MIN) within 30-480. Ranges below
    /// ~70 BPM let half-time tracks (dubstep) read as half tempo. Applies to new
    /// analyses; run `library analyze --all --force` to redo analyzed tracks
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
    let dirs = directories::ProjectDirs::from("", "", "dj-software")
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
        LibraryCommand::Set(set) => set_setting(&lib, set),
        LibraryCommand::Playlist(cmd) => playlist(&lib, cmd),
        LibraryCommand::Grid { track, action } => grid(&lib, *track, *action),
    };
    for failed in lib.take_failed_writes() {
        err_out!("warning: a library change was not saved: {}", failed.error);
    }
    result
}

fn add_folder(lib: &Library, dir: &Path) -> anyhow::Result<()> {
    library::add_folder(lib, dir).with_context(|| format!("adding {}", dir.display()))?;
    out!("added folder {}", dir.display());
    scan_one(lib, dir)
}

fn scan_all(lib: &Library) -> anyhow::Result<()> {
    let folders = lib.reader()?.folders()?;
    if folders.is_empty() {
        out!("no library folders; add one with `dj-software library add-folder <DIR>`");
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
                crate::format::sanitize_for_terminal(&dir.display().to_string()),
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
    out!("{}", scan_report_text(&report).trim_end());
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
            text.push_str(&format!("    {}: {why}\n", path.display()));
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
                missing: None,
            };
            let sort = TrackSort {
                key: args.sort.map_or(TrackSortKey::Artist, SortArg::key),
                descending: false,
            };
            let rows = reader.tracks(&filter, sort, 0, args.limit)?;
            (rows, reader.track_count(&filter)?, None)
        }
    };
    out!(
        "{}",
        track_table(&rows, positions.as_deref(), notation).trim_end()
    );
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
                 (`library analyze --all --force` redoes analyzed tracks)"
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
}
