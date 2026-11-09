//! Reads songs, hotcues and saved loops from a VirtualDJ `database.xml`.

use std::path::{Path, PathBuf};

use quick_xml::events::{BytesStart, Event};
use quick_xml::{Reader, XmlVersion};

use super::{InteropError, read_file};

/// A song entry of the database.
#[derive(Debug, Clone, PartialEq)]
pub struct VdjSong {
    pub path: PathBuf,
    pub size: Option<u64>,
    /// VirtualDJ's scanned grid, as seconds per beat.
    pub seconds_per_beat: Option<f64>,
    pub pois: Vec<VdjPoi>,
    /// Markers that are not hotcues or loops (automix, remix, beatgrid).
    pub other_markers: usize,
    /// Read from a drive's own database (`<drive>/VirtualDJ/database.xml`), where VirtualDJ
    /// keeps the drive's tracks.
    pub from_drive: bool,
}

/// A hotcue or saved loop; `color` is `0xRRGGBB`.
#[derive(Debug, Clone, PartialEq)]
pub enum VdjPoi {
    Hotcue {
        num: u32,
        seconds: f64,
        name: Option<String>,
        color: Option<u32>,
    },
    Loop {
        num: u32,
        seconds: f64,
        beats: f64,
        name: Option<String>,
        color: Option<u32>,
    },
}

/// Reads every song of a database; `drive_root` maps a per-drive database's `X:/` paths.
pub fn read_database(path: &Path, drive_root: Option<&Path>) -> Result<Vec<VdjSong>, InteropError> {
    let bytes = read_file(path)?;
    let format = |why: String| InteropError::Format {
        path: path.to_path_buf(),
        why,
    };
    let mut reader = Reader::from_reader(bytes.as_slice());
    let mut buf = Vec::new();
    let mut songs = Vec::new();
    let mut current: Option<VdjSong> = None;
    let mut saw_root = false;
    loop {
        let event = reader
            .read_event_into(&mut buf)
            .map_err(|e| format(format!("at byte {}: {e}", reader.error_position())))?;
        let is_empty = matches!(event, Event::Empty(_));
        match event {
            Event::Start(e) | Event::Empty(e) => match e.name().as_ref() {
                "VirtualDJ_Database" => saw_root = true,
                _ if !saw_root => return Err(format("not a VirtualDJ database".into())),
                "Song" => {
                    let song = start_song(&e, drive_root).map_err(format)?;
                    if is_empty {
                        songs.push(song);
                    } else {
                        current = Some(song);
                    }
                }
                "Scan" => {
                    if let Some(song) = current.as_mut() {
                        song.seconds_per_beat = attr(&e, "Bpm")
                            .and_then(|v| v.parse().ok())
                            .filter(|v: &f64| *v > 0.0);
                    }
                }
                "Poi" => {
                    if let Some(song) = current.as_mut() {
                        match read_poi(&e) {
                            Some(poi) => song.pois.push(poi),
                            None => song.other_markers += 1,
                        }
                    }
                }
                _ => {}
            },
            Event::End(e) if e.name().as_ref() == "Song" => {
                songs.extend(current.take());
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }
    if !saw_root {
        return Err(format("not a VirtualDJ database".into()));
    }
    if current.is_some() {
        return Err(format("the file ends inside a song".into()));
    }
    Ok(songs)
}

fn attr(e: &BytesStart, key: &str) -> Option<String> {
    e.attributes()
        .flatten()
        .find(|a| a.key.as_ref() == key)
        .and_then(|a| a.normalized_value(XmlVersion::Implicit1_0).ok())
        .map(|v| v.into_owned())
}

fn start_song(e: &BytesStart, drive_root: Option<&Path>) -> Result<VdjSong, String> {
    let file_path = attr(e, "FilePath").ok_or("a song has no FilePath")?;
    Ok(VdjSong {
        path: map_path(&file_path, drive_root),
        size: attr(e, "FileSize").and_then(|v| v.parse().ok()),
        seconds_per_beat: None,
        pois: Vec::new(),
        other_markers: 0,
        from_drive: drive_root.is_some(),
    })
}

/// `D:/a/b.mp3` under a drive root becomes `<root>/a/b.mp3`.
fn map_path(file_path: &str, drive_root: Option<&Path>) -> PathBuf {
    let bytes = file_path.as_bytes();
    let has_drive = bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':';
    match drive_root {
        Some(root) if has_drive => root.join(file_path[2..].trim_start_matches(['/', '\\'])),
        _ => PathBuf::from(file_path),
    }
}

/// A hotcue or loop, or `None` for any other marker.
fn read_poi(e: &BytesStart) -> Option<VdjPoi> {
    let kind = attr(e, "Type")?;
    let num: u32 = attr(e, "Num")?.parse().ok()?;
    let seconds: f64 = attr(e, "Pos")?.parse().ok()?;
    let name = attr(e, "Name");
    let color = attr(e, "Color")
        .and_then(|v| v.parse::<u32>().ok())
        .map(|argb| argb & 0x00FF_FFFF);
    match kind.as_str() {
        "cue" => Some(VdjPoi::Hotcue {
            num,
            seconds,
            name,
            color,
        }),
        "loop" => Some(VdjPoi::Loop {
            num,
            seconds,
            beats: attr(e, "Size")?.parse().ok()?,
            name,
            color,
        }),
        _ => None,
    }
}
