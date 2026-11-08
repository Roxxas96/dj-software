//! The computer's drives: the roots of the browser's folder tree.

use std::path::PathBuf;
#[cfg(not(windows))]
use std::{fs, path::Path};

#[cfg(not(windows))]
use crate::scan::is_hidden;

/// A drive at the top of the folder tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Drive {
    /// What the tree shows, such as "Macintosh HD".
    pub name: String,
    pub path: PathBuf,
}

/// The computer's drives, the system drive first.
///
/// - macOS: `/`, named after the `/Volumes` link to it ("Macintosh HD" if there is none),
///   then every other entry of `/Volumes`.
/// - Windows: every drive letter that exists.
/// - Linux and others: `/` ("Computer"), then the entries of `/media/$USER` and `/mnt`.
///
/// Hidden (`.`-prefixed) entries are skipped. Nothing below a drive is read.
#[must_use]
pub fn drives() -> Vec<Drive> {
    #[cfg(target_os = "macos")]
    {
        mac_drives(Path::new("/Volumes"))
    }
    #[cfg(windows)]
    {
        (b'A'..=b'Z')
            .map(char::from)
            .filter_map(|letter| {
                let path = PathBuf::from(format!("{letter}:\\"));
                path.exists().then(|| Drive {
                    name: format!("{letter}:"),
                    path,
                })
            })
            .collect()
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let mut drives = vec![Drive {
            name: "Computer".to_owned(),
            path: PathBuf::from("/"),
        }];
        if let Some(user) = std::env::var_os("USER") {
            drives.extend(entries(&Path::new("/media").join(user)));
        }
        drives.extend(entries(Path::new("/mnt")));
        drives
    }
}

/// `/` named after its link in `volumes`, then `volumes`' other entries.
#[cfg(target_os = "macos")]
fn mac_drives(volumes: &Path) -> Vec<Drive> {
    let mut root_name = None;
    let mut others = Vec::new();
    for drive in entries(volumes) {
        // Only the link itself is read, never what is inside the volume.
        match fs::read_link(&drive.path) {
            Ok(target) if target == Path::new("/") => {
                root_name.get_or_insert(drive.name);
            }
            _ => others.push(drive),
        }
    }
    let root = Drive {
        name: root_name.unwrap_or_else(|| "Macintosh HD".to_owned()),
        path: PathBuf::from("/"),
    };
    std::iter::once(root).chain(others).collect()
}

/// The visible entries of `dir` as drives, sorted by name (none if it can't be read).
#[cfg(not(windows))]
fn entries(dir: &Path) -> Vec<Drive> {
    let Ok(read) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut drives: Vec<Drive> = read
        .flatten()
        .filter(|e| !is_hidden(&e.file_name()))
        .map(|e| Drive {
            name: e.file_name().to_string_lossy().into_owned(),
            path: e.path(),
        })
        .collect();
    drives.sort_by_cached_key(|d| d.name.to_lowercase());
    drives
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn the_link_to_root_names_the_system_drive() {
        let volumes = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink("/", volumes.path().join("System Disk")).unwrap();
        fs::create_dir(volumes.path().join("Music")).unwrap();
        fs::create_dir(volumes.path().join(".timemachine")).unwrap();
        std::os::unix::fs::symlink("/tmp", volumes.path().join("Elsewhere")).unwrap();
        let names: Vec<_> = mac_drives(volumes.path())
            .into_iter()
            .map(|d| (d.name, d.path))
            .collect();
        assert_eq!(
            names,
            [
                ("System Disk".to_owned(), PathBuf::from("/")),
                ("Elsewhere".to_owned(), volumes.path().join("Elsewhere")),
                ("Music".to_owned(), volumes.path().join("Music")),
            ]
        );

        // No link to `/`: the default name.
        let empty = tempfile::tempdir().unwrap();
        assert_eq!(mac_drives(empty.path())[0].name, "Macintosh HD");
    }
}
