//! Reads VirtualDJ's `.vdjfolder` and `.m3u` playlists.

use std::path::{Path, PathBuf};

use quick_xml::events::Event;
use quick_xml::{Reader, XmlVersion};

use super::{InteropError, read_file};

/// A playlist; `name` is `Folder / Name` when nested.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VdjPlaylist {
    pub name: String,
    pub paths: Vec<PathBuf>,
}

/// How many folder levels below a list dir are searched.
const MAX_DEPTH: usize = 8;

/// Reads every playlist under the dirs, in `order`'s sequence and then by name.
///
/// Symlinks are not followed. A file or entry that cannot be read is returned in the second
/// list and the rest are still read; `Err` only when no list dir can be opened.
pub fn read_playlists(
    list_dirs: &[PathBuf],
    order: Option<&Path>,
) -> Result<(Vec<VdjPlaylist>, Vec<InteropError>), InteropError> {
    let mut playlists = Vec::new();
    let mut errors = Vec::new();
    let mut first_open_failure = None;
    let mut opened = 0;
    for dir in list_dirs {
        match collect(dir, dir, 0, &mut playlists, &mut errors) {
            Ok(()) => opened += 1,
            Err(e) => first_open_failure = first_open_failure.or(Some(e)),
        }
    }
    if opened == 0
        && let Some(e) = first_open_failure
    {
        return Err(e);
    }
    errors.extend(first_open_failure);
    let order_lines: Vec<String> = match order.map(read_file) {
        Some(Ok(bytes)) => String::from_utf8_lossy(&bytes)
            .lines()
            .map(|l| l.trim().replace('\\', "/"))
            .filter(|l| !l.is_empty())
            .collect(),
        Some(Err(e)) => {
            errors.push(e);
            Vec::new()
        }
        None => Vec::new(),
    };
    let rank = |p: &VdjPlaylist| {
        let slashed = p.name.replace(" / ", "/");
        order_lines
            .iter()
            .position(|l| *l == slashed || *l == p.name)
            .unwrap_or(usize::MAX)
    };
    playlists.sort_by(|a, b| rank(a).cmp(&rank(b)).then_with(|| a.name.cmp(&b.name)));
    Ok((playlists, errors))
}

/// Fails only when `dir` itself cannot be listed; problems below it go to `errors`.
fn collect(
    root: &Path,
    dir: &Path,
    depth: usize,
    out: &mut Vec<VdjPlaylist>,
    errors: &mut Vec<InteropError>,
) -> Result<(), InteropError> {
    let io = |source| InteropError::Io {
        path: dir.to_path_buf(),
        source,
    };
    let mut entries = Vec::new();
    for entry in std::fs::read_dir(dir).map_err(io)? {
        match entry.and_then(|e| e.file_type().map(|t| (e.path(), t))) {
            Ok((path, kind)) => entries.push((path, kind)),
            Err(source) => errors.push(InteropError::Io {
                path: dir.to_path_buf(),
                source,
            }),
        }
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    for (path, kind) in entries {
        if kind.is_symlink() {
            continue;
        }
        if kind.is_dir() {
            if depth < MAX_DEPTH
                && let Err(e) = collect(root, &path, depth + 1, out, errors)
            {
                errors.push(e);
            }
            continue;
        }
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase);
        let read = match ext.as_deref() {
            Some("vdjfolder") => read_vdjfolder(&path),
            Some("m3u") => read_m3u(&path),
            _ => continue,
        };
        match read {
            Ok(paths) => out.push(VdjPlaylist {
                name: playlist_name(root, &path),
                paths,
            }),
            Err(e) => errors.push(e),
        }
    }
    Ok(())
}

fn playlist_name(root: &Path, file: &Path) -> String {
    let relative = file.strip_prefix(root).unwrap_or(file).with_extension("");
    relative
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(" / ")
}

fn read_vdjfolder(path: &Path) -> Result<Vec<PathBuf>, InteropError> {
    let bytes = read_file(path)?;
    let format = |why: String| InteropError::Format {
        path: path.to_path_buf(),
        why,
    };
    let mut reader = Reader::from_reader(bytes.as_slice());
    let mut buf = Vec::new();
    let mut paths = Vec::new();
    let mut saw_root = false;
    loop {
        let event = reader
            .read_event_into(&mut buf)
            .map_err(|e| format(format!("at byte {}: {e}", reader.error_position())))?;
        match event {
            Event::Start(e) | Event::Empty(e) => match e.name().as_ref() {
                "VirtualFolder" => saw_root = true,
                "song" => {
                    let song_path = e
                        .attributes()
                        .flatten()
                        .find(|a| a.key.as_ref() == "path")
                        .and_then(|a| {
                            a.normalized_value(XmlVersion::Implicit1_0)
                                .ok()
                                .map(|v| v.into_owned())
                        });
                    paths.extend(song_path.map(PathBuf::from));
                }
                _ => {}
            },
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }
    if !saw_root {
        return Err(format("not a VirtualDJ playlist".into()));
    }
    Ok(paths)
}

fn read_m3u(path: &Path) -> Result<Vec<PathBuf>, InteropError> {
    let text = String::from_utf8_lossy(&read_file(path)?).into_owned();
    let base = path.parent().unwrap_or(Path::new(""));
    Ok(text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| base.join(l))
        .collect())
}
