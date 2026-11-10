//! VirtualDJ's drive data: `VirtualDJ/database.xml` with our tracks merged in, and a
//! `.vdjfolder` for each exported playlist.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::path::Path;

use quick_xml::events::{BytesStart, Event};
use quick_xml::{Reader, XmlVersion};

use super::run::{FormatOutput, StickContext, StickFormat};
use super::timing::{App, app_seconds};
use super::{ExportError, ExportTrack, PlannedTrack, files, safe_component};

/// The drive's database, relative to the stick's root.
pub const DB_PATH: &str = "VirtualDJ/database.xml";
/// Where VirtualDJ keeps a drive's playlists, relative to the stick's root.
pub const LISTS_DIR: &str = "VirtualDJ/MyLists";

/// What VirtualDJ writes for a drive's own root.
const DRIVE: &str = "D:/";
/// Songs under this prefix are ours; every other element of an existing database is kept.
const OURS: &str = "D:/Open Jockey/";
const ROOT: &str = "VirtualDJ_Database";
const DECLARATION: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>";
const LIST_EXT: &str = ".vdjfolder";

/// Writes `VirtualDJ/database.xml` with our tracks merged in, and one playlist file each.
#[derive(Debug, Clone, Copy, Default)]
pub struct VirtualDjFormat;

impl StickFormat for VirtualDjFormat {
    fn name(&self) -> &'static str {
        "VirtualDJ"
    }

    fn write_library(&self, cx: &StickContext<'_>) -> Result<FormatOutput, ExportError> {
        let plan = cx.plan;
        let stick = plan.stick.as_path();
        let lists = join(stick, LISTS_DIR);
        let ours: HashSet<&str> = plan
            .previous
            .iter()
            .flat_map(|m| m.vdj_lists.iter().map(String::as_str))
            .collect();
        let by_id: HashMap<i64, &PlannedTrack> =
            plan.tracks.iter().map(|t| (t.track.id, t)).collect();

        let mut output = FormatOutput::default();
        let mut taken = HashSet::new();
        for playlist in &plan.playlists {
            let base = safe_component(&playlist.name, 120);
            let file = files::unique_name(&base, LIST_EXT, &lists, &ours, &taken);
            taken.insert(file.to_lowercase());
            let entries: Vec<&PlannedTrack> = playlist
                .tracks
                .iter()
                .filter_map(|id| by_id.get(id).copied())
                .collect();
            files::write_atomic(stick, &lists.join(&file), list(&entries, stick).as_bytes())?;
            output.files.push(file);
        }

        let path = join(stick, DB_PATH);
        let existing = files::read_existing(&path)?;
        let (bytes, left_out) =
            database(existing.as_deref(), cx).map_err(|why| ExportError::Format {
                path: path.clone(),
                why,
            })?;
        files::write_atomic(stick, &path, &bytes)?;
        if left_out > 0 {
            output.notes.push(format!(
                "{left_out} saved loops left out of VirtualDJ: their tracks have no BPM"
            ));
        }
        Ok(output)
    }
}

/// Refuses a stick whose `database.xml` is there but is not a regular file or is not one
/// [`database`] can merge into, so an export never starts on a database it would fail on.
pub(crate) fn check_database(stick: &Path) -> Result<(), ExportError> {
    let path = join(stick, DB_PATH);
    let Some(bytes) = files::read_existing(&path)? else {
        return Ok(());
    };
    foreign_elements(&bytes).map(drop).map_err(|why| {
        ExportError::Refused(format!(
            "{} cannot be read as VirtualDJ's database ({why}); Open Jockey will not change it",
            path.display()
        ))
    })
}

/// `relative` (with `/` separators) under `root`, one component at a time.
fn join(root: &Path, relative: &str) -> std::path::PathBuf {
    relative
        .split('/')
        .fold(root.to_path_buf(), |p, part| p.join(part))
}

/// An attribute value: XML-escaped, with whitespace kept as character references and
/// characters XML cannot hold dropped.
fn esc(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            '\t' | '\n' | '\r' => {
                let _ = write!(out, "&#{};", u32::from(c));
            }
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

/// The copy's size in bytes: what is on the stick, else what the library says.
fn copy_size(stick: &Path, planned: &PlannedTrack) -> u64 {
    std::fs::symlink_metadata(join(stick, &planned.stick_path))
        .ok()
        .filter(|m| m.file_type().is_file())
        .map_or(planned.track.size, |m| m.len())
}

/// The track's length in seconds, if known.
fn length(t: &ExportTrack) -> Option<f64> {
    let rate = f64::from(t.sample_rate);
    t.duration_frames
        .filter(|_| rate > 0.0)
        .map(|frames| frames as f64 / rate)
}

fn usable_bpm(t: &ExportTrack) -> Option<f64> {
    t.bpm.filter(|b| b.is_finite() && *b > 0.0)
}

/// The new `database.xml`: every element of `existing` that is not ours as its source bytes,
/// then a song for each track of the plan. Also the number of saved loops left out because
/// their track has no BPM, so their length in beats is unknown.
///
/// # Errors
///
/// A message when `existing` is not XML or its root is not `VirtualDJ_Database`.
pub fn database(
    existing: Option<&[u8]>,
    cx: &StickContext<'_>,
) -> Result<(Vec<u8>, usize), String> {
    let foreign = match existing {
        Some(bytes) => foreign_elements(bytes)?,
        None => Vec::new(),
    };
    let mut out: Vec<u8> = Vec::new();
    out.extend_from_slice(DECLARATION.as_bytes());
    out.extend_from_slice(format!("\n<{ROOT} Version=\"2025\">\n").as_bytes());
    for element in foreign {
        out.extend_from_slice(b" ");
        out.extend_from_slice(element);
        out.extend_from_slice(b"\n");
    }
    let mut left_out = 0;
    let mut songs = String::new();
    for planned in &cx.plan.tracks {
        left_out += song(&mut songs, planned, &cx.plan.stick);
    }
    out.extend_from_slice(songs.as_bytes());
    out.extend_from_slice(format!("</{ROOT}>\n").as_bytes());
    Ok((out, left_out))
}

/// The source bytes of each child of the root that is not one of our songs.
fn foreign_elements(bytes: &[u8]) -> Result<Vec<&[u8]>, String> {
    let is_ours = |e: &BytesStart<'_>| {
        e.attributes()
            .flatten()
            .find(|a| a.key.as_ref() == "FilePath")
            .and_then(|a| a.normalized_value(XmlVersion::Implicit1_0).ok())
            .is_some_and(|v| v.starts_with(OURS))
    };
    let not_a_database = || "not a VirtualDJ database".to_string();
    let is_root = |e: &BytesStart<'_>| {
        if e.name().as_ref() == ROOT {
            Ok(())
        } else {
            Err(not_a_database())
        }
    };
    let mut reader = Reader::from_reader(bytes);
    let mut buf = Vec::new();
    let mut out = Vec::new();
    // 0 outside the root, 1 directly inside it, deeper inside a child.
    let mut depth = 0usize;
    let mut saw_root = false;
    // Where the child being read starts, and whether it is kept.
    let mut child = (0usize, false);
    loop {
        let before = usize::try_from(reader.buffer_position()).unwrap_or(usize::MAX);
        let event = reader
            .read_event_into(&mut buf)
            .map_err(|e| format!("not valid XML at byte {}: {e}", reader.error_position()))?;
        let after = usize::try_from(reader.buffer_position()).unwrap_or(usize::MAX);
        match event {
            Event::Start(e) if depth == 0 && !saw_root => {
                is_root(&e)?;
                saw_root = true;
                depth = 1;
            }
            // An empty root has no End event.
            Event::Empty(e) if depth == 0 && !saw_root => {
                is_root(&e)?;
                saw_root = true;
            }
            Event::Start(e) if depth == 1 => {
                child = (before, !is_ours(&e));
                depth = 2;
            }
            Event::Start(_) if depth > 1 => depth += 1,
            Event::Empty(e) if depth == 1 && !is_ours(&e) => out.push(&bytes[before..after]),
            Event::End(_) if depth > 0 => {
                depth -= 1;
                if depth == 1 && child.1 {
                    out.push(&bytes[child.0..after]);
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }
    if !saw_root {
        return Err(not_a_database());
    }
    if depth != 0 {
        return Err("the file ends inside an element".into());
    }
    Ok(out)
}

/// Appends the `Song` element of `planned` to `out`; returns the loops left out.
fn song(out: &mut String, planned: &PlannedTrack, stick: &Path) -> usize {
    let t = &planned.track;
    let _ = writeln!(
        out,
        " <Song FilePath=\"{DRIVE}{}\" FileSize=\"{}\">",
        esc(&planned.stick_path),
        copy_size(stick, planned)
    );
    let mut tags = String::new();
    for (key, value) in [
        ("Author", t.artist.as_deref()),
        ("Title", Some(t.title.as_str())),
        ("Album", t.album.as_deref()),
        ("Genre", t.genre.as_deref()),
        ("Key", t.key.as_deref()),
    ] {
        if let Some(v) = value.filter(|v| !v.is_empty()) {
            let _ = write!(tags, " {key}=\"{}\"", esc(v));
        }
    }
    let _ = writeln!(out, "  <Tags{tags}/>");
    if let Some(seconds) = length(t) {
        let _ = writeln!(out, "  <Infos SongLength=\"{seconds:.6}\"/>");
    }
    let mut left_out = 0;
    for (slot, point) in t.hotcues.iter().enumerate() {
        let Some(point) = point else { continue };
        let pos = app_seconds(App::VirtualDj, point.frames, t).max(0.0);
        let mut attrs = format!("Pos=\"{pos:.6}\" Num=\"{}\"", slot + 1);
        if let Some(rgb) = point.color {
            let _ = write!(
                attrs,
                " Color=\"{}\"",
                0xFF00_0000_u32 | (rgb & 0x00FF_FFFF)
            );
        }
        if let Some(name) = point.name.as_deref().filter(|n| !n.is_empty()) {
            let _ = write!(attrs, " Name=\"{}\"", esc(name));
        }
        match point.loop_frames {
            None => {
                let _ = writeln!(out, "  <Poi {attrs} Type=\"cue\"/>");
            }
            Some(frames) => match usable_bpm(t) {
                Some(bpm) => {
                    let beats = frames / (f64::from(t.sample_rate) * 60.0 / bpm);
                    let _ = writeln!(out, "  <Poi {attrs} Type=\"loop\" Size=\"{beats:.3}\"/>");
                }
                None => left_out += 1,
            },
        }
    }
    out.push_str(" </Song>\n");
    left_out
}

/// One `.vdjfolder`: the entries in order, as absolute paths on the stick.
fn list(entries: &[&PlannedTrack], stick: &Path) -> String {
    let mut out = format!("{DECLARATION}\n<VirtualFolder noDuplicates=\"no\">\n");
    for (idx, planned) in entries.iter().enumerate() {
        let t = &planned.track;
        let full = join(stick, &planned.stick_path)
            .to_string_lossy()
            .into_owned();
        // A backslash is a separator on Windows only; elsewhere it is part of a name.
        let full = if cfg!(windows) {
            full.replace('\\', "/")
        } else {
            full
        };
        let mut attrs = format!(
            "path=\"{}\" size=\"{}\"",
            esc(&full),
            copy_size(stick, planned)
        );
        if let Some(seconds) = length(t) {
            let _ = write!(attrs, " songlength=\"{seconds:.1}\"");
        }
        if let Some(bpm) = usable_bpm(t) {
            // Plain BPM here; only database.xml keeps seconds per beat.
            let _ = write!(attrs, " bpm=\"{bpm:.3}\"");
        }
        if let Some(key) = t.key.as_deref().filter(|k| !k.is_empty()) {
            let _ = write!(attrs, " key=\"{}\"", esc(key));
        }
        let _ = write!(attrs, " title=\"{}\" idx=\"{idx}\"", esc(&t.title));
        let _ = writeln!(out, " <song {attrs}/>");
    }
    out.push_str("</VirtualFolder>\n");
    out
}
