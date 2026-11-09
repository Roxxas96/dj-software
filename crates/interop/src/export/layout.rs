//! Where an export puts its files on the stick, and the manifest that records them.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::{ExportError, ExportTrack, files};

/// The folder on the stick that holds everything Open Jockey writes.
pub const OJ_DIR: &str = "Open Jockey";
/// The folder inside [`OJ_DIR`] that holds the copied audio files.
pub const MUSIC_DIR: &str = "Music";
/// The manifest's file name inside [`OJ_DIR`].
pub const MANIFEST: &str = "export.toml";
/// The suffix of a file that is still being written.
pub const PART_SUFFIX: &str = ".ojpart";
/// Free space an export leaves untouched, for file system overhead and the databases.
pub const SPACE_MARGIN: u64 = 32 * 1024 * 1024;

const DEVICE_NAMES: [&str; 4] = ["CON", "PRN", "AUX", "NUL"];

/// Turns text into one file or folder name that every file system on a stick accepts.
///
/// Characters Windows forbids and control characters become `_`, trailing spaces and dots go,
/// a device name such as `CON` gets a `_`, and the result is cut to `max_chars` characters
/// (a name with an extension keeps the extension).
#[must_use]
pub fn safe_component(text: &str, max_chars: usize) -> String {
    let replaced: String = text
        .chars()
        .map(|c| {
            if matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();
    let trimmed = replaced.trim_end_matches([' ', '.']);
    if trimmed.is_empty() {
        return "_".into();
    }
    // A device name stays reserved with any extension: `con.mp3` is `con`.
    let (stem, ext) = split_extension(trimmed);
    let device_stem = stem.split('.').next().unwrap_or(stem);
    let is_device = DEVICE_NAMES
        .iter()
        .any(|d| device_stem.eq_ignore_ascii_case(d))
        || ["COM", "LPT"].iter().any(|p| {
            device_stem.len() == 4
                && device_stem[..3].eq_ignore_ascii_case(p)
                && matches!(device_stem.as_bytes()[3], b'1'..=b'9')
        });
    let stem = if is_device && !stem.contains('.') {
        format!("{stem}_")
    } else if is_device {
        let (first, rest) = stem.split_once('.').unwrap_or((stem, ""));
        format!("{first}_.{rest}")
    } else {
        stem.to_string()
    };
    let ext_chars = ext.chars().count();
    let keep = max_chars.saturating_sub(ext_chars);
    if stem.chars().count() + ext_chars <= max_chars {
        return format!("{stem}{ext}");
    }
    let cut: String = stem.chars().take(keep).collect();
    let cut = cut.trim_end_matches([' ', '.']);
    let cut = if cut.is_empty() { "_" } else { cut };
    format!("{cut}{ext}")
}

/// The components of a relative path made only of plain names, or why it is not one.
fn relative_parts(text: &str) -> Result<impl Iterator<Item = &std::ffi::OsStr>, String> {
    let path = Path::new(text);
    let plain = path
        .components()
        .all(|c| matches!(c, std::path::Component::Normal(_)));
    if text.is_empty() || !plain {
        return Err(format!("path {text:?} is not a plain relative path"));
    }
    Ok(path.iter())
}

/// Splits `name.ext` into `("name", ".ext")`; a leading dot or a very long "extension" is
/// part of the stem.
fn split_extension(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(i) if i > 0 && name[i..].chars().count() <= 12 => (&name[..i], &name[i..]),
        _ => (name, ""),
    }
}

/// Gives every track its path on the stick, relative to the stick's root with `/` separators:
/// `Open Jockey/Music/<artist>/<file name>`. Tracks are placed by ascending id, and a name
/// that clashes with an earlier one (ignoring case) gets ` (2)`, ` (3)` and so on.
#[must_use]
pub fn stick_paths(tracks: &[ExportTrack]) -> HashMap<i64, String> {
    let mut ordered: Vec<&ExportTrack> = tracks.iter().collect();
    ordered.sort_by_key(|t| t.id);
    let mut taken = HashSet::new();
    let mut paths = HashMap::new();
    for track in ordered {
        let artist = track
            .artist
            .as_deref()
            .filter(|a| !a.trim().is_empty())
            .unwrap_or("Unknown Artist");
        let artist = safe_component(artist, 60);
        let file = track
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let file = safe_component(&file, 120);
        let (stem, ext) = split_extension(&file);
        let mut n = 1;
        let path = loop {
            let name = if n == 1 {
                file.clone()
            } else {
                format!("{stem} ({n}){ext}")
            };
            let path = format!("{OJ_DIR}/{MUSIC_DIR}/{artist}/{name}");
            if taken.insert(path.to_lowercase()) {
                break path;
            }
            n += 1;
        };
        paths.insert(track.id, path);
    }
    paths
}

/// What Open Jockey wrote to a stick, so a later export knows which files are its own.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    /// Always 1.
    pub version: u32,
    /// Names of the Serato playlists (crates) and Rekordbox playlists written.
    pub playlists: Vec<String>,
    /// Names of the Serato crate files written.
    pub crates: Vec<String>,
    /// Names of the VirtualDJ playlist files written.
    pub vdj_lists: Vec<String>,
    /// Audio files written, relative to the `Open Jockey` folder, with `/` separators.
    pub tracks: Vec<String>,
}

impl Manifest {
    fn path(stick: &Path) -> std::path::PathBuf {
        stick.join(OJ_DIR).join(MANIFEST)
    }

    /// Reads the manifest on `stick`; `None` when there is none.
    ///
    /// # Errors
    ///
    /// [`ExportError::Refused`] when the file is damaged, [`ExportError::Io`] when it cannot
    /// be read.
    pub fn read(stick: &Path) -> Result<Option<Self>, ExportError> {
        let path = Self::path(stick);
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(source) => return Err(ExportError::Io { path, source }),
        };
        let damaged = |why: String| {
            ExportError::Refused(format!(
                "Open Jockey's manifest on the stick is damaged: {why}"
            ))
        };
        let manifest: Self = toml::from_str(&text).map_err(|e| damaged(e.to_string()))?;
        manifest.validate().map_err(damaged)?;
        Ok(Some(manifest))
    }

    /// The stick's contents are untrusted: every name must stay inside where it is joined.
    fn validate(&self) -> Result<(), String> {
        for track in &self.tracks {
            let mut parts = relative_parts(track)?;
            if parts.next().is_none_or(|first| first != MUSIC_DIR) {
                return Err(format!("track path {track:?} is not under {MUSIC_DIR}/"));
            }
        }
        for name in self.crates.iter().chain(&self.vdj_lists) {
            if relative_parts(name)?.count() != 1 {
                return Err(format!("file name {name:?} is not a single name"));
            }
        }
        Ok(())
    }

    /// Writes the manifest to `stick`, all or nothing.
    ///
    /// # Errors
    ///
    /// [`ExportError::Io`] when the file cannot be written.
    pub fn write(&self, stick: &Path) -> Result<(), ExportError> {
        let path = Self::path(stick);
        let text = toml::to_string(self).map_err(|e| ExportError::Format {
            path: path.clone(),
            why: e.to_string(),
        })?;
        files::write_atomic(stick, &path, text.as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use decode::LeadIn;

    fn track(id: i64, path: &str, artist: Option<&str>) -> crate::export::ExportTrack {
        crate::export::ExportTrack {
            id,
            path: path.into(),
            fingerprint: [0; 32],
            size: 1,
            title: String::new(),
            artist: artist.map(Into::into),
            album: None,
            genre: None,
            sample_rate: 44_100,
            duration_frames: None,
            bpm: None,
            first_beat_frames: None,
            key: None,
            cue_frames: None,
            hotcues: Default::default(),
            kind: crate::export::FileKind::Mp3,
            lead_in: LeadIn::default(),
        }
    }

    #[test]
    fn safe_component_replaces_and_trims() {
        assert_eq!(safe_component("AC/DC: Live?", 60), "AC_DC_ Live_");
        assert_eq!(safe_component("con.mp3", 120), "con_.mp3");
        assert_eq!(safe_component("Mix. ", 60), "Mix");
        assert_eq!(safe_component("", 60), "_");
        assert_eq!(safe_component("Béyoncé 🎵", 60), "Béyoncé 🎵");
        assert_eq!(safe_component("a\u{7}b", 60), "a_b");
        assert_eq!(safe_component("LPT3", 60), "LPT3_");
    }

    #[test]
    fn safe_component_cut_keeps_the_extension() {
        let name = format!("{}.mp3", "x".repeat(196));
        let cut = safe_component(&name, 120);
        assert!(cut.ends_with(".mp3"));
        assert_eq!(cut.chars().count(), 120);
    }

    #[test]
    fn stick_paths_resolve_clashes_by_track_id() {
        let tracks = [
            track(7, "/m/Artist/song.MP3", Some("Artist")),
            track(3, "/m/other/Song.mp3", Some("Artist")),
        ];
        let paths = stick_paths(&tracks);
        assert_eq!(paths[&3], "Open Jockey/Music/Artist/Song.mp3");
        assert_eq!(paths[&7], "Open Jockey/Music/Artist/song (2).MP3");
    }

    #[test]
    fn stick_paths_name_a_missing_artist() {
        let tracks = [track(1, "/m/a.mp3", None), track(2, "/m/b.mp3", Some(""))];
        let paths = stick_paths(&tracks);
        assert_eq!(paths[&1], "Open Jockey/Music/Unknown Artist/a.mp3");
        assert_eq!(paths[&2], "Open Jockey/Music/Unknown Artist/b.mp3");
    }

    #[test]
    fn manifest_round_trips_and_missing_is_none() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(Manifest::read(dir.path()).unwrap(), None);
        let manifest = Manifest {
            version: 1,
            playlists: vec!["a".into()],
            crates: vec![],
            vdj_lists: vec![],
            tracks: vec!["Music/A/x.mp3".into()],
        };
        manifest.write(dir.path()).unwrap();
        assert_eq!(Manifest::read(dir.path()).unwrap(), Some(manifest));
    }

    #[test]
    fn damaged_manifest_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let oj = dir.path().join(OJ_DIR);
        std::fs::create_dir_all(&oj).unwrap();
        std::fs::write(oj.join(MANIFEST), "not = [toml").unwrap();
        let err = Manifest::read(dir.path()).unwrap_err();
        assert!(
            matches!(&err, crate::export::ExportError::Refused(m) if m.starts_with("Open Jockey's manifest on the stick is damaged: "))
        );
    }

    #[test]
    fn manifest_with_escaping_entries_is_refused() {
        let bad = [
            ("tracks", "\"../x\""),
            ("tracks", "\"/etc/passwd\""),
            ("tracks", "\"Other/x.mp3\""),
            ("tracks", "\"Music/../../x\""),
            ("crates", "\"../x.crate\""),
            ("crates", "\"a/b.crate\""),
            ("vdj_lists", "\"/abs.vdjfolder\""),
            ("vdj_lists", "\"\""),
        ];
        for (field, value) in bad {
            let dir = tempfile::tempdir().unwrap();
            let oj = dir.path().join(OJ_DIR);
            std::fs::create_dir_all(&oj).unwrap();
            let mut lists = std::collections::BTreeMap::from([
                ("tracks", "[]"),
                ("crates", "[]"),
                ("vdj_lists", "[]"),
            ]);
            let value = format!("[{value}]");
            lists.insert(field, &value);
            let text = format!(
                "version = 1\nplaylists = []\ncrates = {}\nvdj_lists = {}\ntracks = {}\n",
                lists["crates"], lists["vdj_lists"], lists["tracks"]
            );
            std::fs::write(oj.join(MANIFEST), text).unwrap();
            let err = Manifest::read(dir.path()).unwrap_err();
            assert!(
                matches!(&err, ExportError::Refused(m) if m.starts_with("Open Jockey's manifest on the stick is damaged: ")),
                "{field} {value}: {err:?}"
            );
        }
    }
}
