//! Finding the USB sticks an export can go to, and which drive a folder is on.

// The window's export (a later task) is the other caller of the stick list.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

use library::Reader;

/// A mounted volume an export can be written to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Stick {
    /// The mount point or drive root.
    pub root: PathBuf,
    /// What to call it: the volume's name, or its drive letter.
    pub name: String,
    /// Free bytes, when the system says.
    pub free_bytes: Option<u64>,
}

/// The folders whose subfolders are mounted volumes.
#[derive(Debug, Clone, Default)]
pub(crate) struct Mounts {
    pub dirs: Vec<PathBuf>,
}

impl Mounts {
    /// Where this system mounts removable drives: `/Volumes` on macOS; `/media/<user>`,
    /// `/run/media/<user>` and `/media` on Linux (the first two only when `$USER` is set);
    /// none on Windows, whose drives come from letters.
    pub fn usual() -> Self {
        let dirs = if cfg!(target_os = "macos") {
            vec![PathBuf::from("/Volumes")]
        } else if cfg!(windows) {
            Vec::new()
        } else {
            let mut dirs = Vec::new();
            if let Some(user) = std::env::var_os("USER").filter(|u| !u.is_empty()) {
                dirs.push(Path::new("/media").join(&user));
                dirs.push(Path::new("/run/media").join(user));
            }
            dirs.push(PathBuf::from("/media"));
            dirs
        };
        Self { dirs }
    }
}

/// The drive holding `folder`: `<mounts>/<name>` when it is under `mounts`, else the
/// filesystem root. Both are compared by canonical path, so a short or symlinked spelling
/// still matches; the drive comes back in canonical form.
pub(crate) fn volume_of(folder: &Path, mounts: &Path) -> PathBuf {
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

/// The drive letters `D:` to `Z:` that exist, other than the system drive (`system_drive`,
/// like `C:`). `exists` says whether a root is there.
fn letter_roots(system_drive: &str, exists: impl Fn(&Path) -> bool) -> Vec<PathBuf> {
    ('D'..='Z')
        .filter(|letter| !system_drive.eq_ignore_ascii_case(&format!("{letter}:")))
        .map(|letter| PathBuf::from(format!("{letter}:\\")))
        .filter(|root| exists(root))
        .collect()
}

/// The directories directly under `dirs` that are not hidden and are not the filesystem
/// root reached through a link.
fn mounted_dirs(dirs: &[PathBuf]) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let hidden = entry.file_name().to_string_lossy().starts_with('.');
            if !hidden && path.is_dir() {
                found.push(path);
            }
        }
    }
    found
}

/// The candidates that are not a library drive, sorted by name. A candidate whose canonical
/// path has no parent is the filesystem root: never a stick.
fn sticks_among(candidates: Vec<PathBuf>, library_volumes: &[PathBuf]) -> Vec<Stick> {
    let mut sticks: Vec<Stick> = Vec::new();
    for candidate in candidates {
        let root = dunce::canonicalize(&candidate).unwrap_or(candidate);
        if root.parent().is_none() || library_volumes.contains(&root) {
            continue;
        }
        if sticks.iter().any(|s| s.root == root) {
            continue;
        }
        let name = root.file_name().map_or_else(
            || root.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        );
        let free_bytes = interop::free_space(&root);
        sticks.push(Stick {
            root,
            name,
            free_bytes,
        });
    }
    sticks.sort_by_cached_key(|s| (s.name.to_lowercase(), s.root.clone()));
    sticks
}

/// The mounted volumes that can take an export: each folder directly under a mount dir (not
/// hidden, not the filesystem root through a link), and on Windows each existing drive
/// letter `D:` to `Z:` besides the system drive. A volume holding a library folder is left
/// out. Sorted by name.
pub(crate) fn list_sticks(reader: &Reader, mounts: &Mounts) -> anyhow::Result<Vec<Stick>> {
    let mut library_volumes = Vec::new();
    for folder in reader.folders()? {
        for mount in &mounts.dirs {
            library_volumes.push(volume_of(&folder, mount));
        }
        let canonical = dunce::canonicalize(&folder).unwrap_or(folder);
        library_volumes.extend(canonical.ancestors().last().map(Path::to_path_buf));
    }
    let mut candidates = mounted_dirs(&mounts.dirs);
    if cfg!(windows) {
        let system = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".to_owned());
        candidates.extend(letter_roots(&system, |root| root.exists()));
    }
    Ok(sticks_among(candidates, &library_volumes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use library::Library;

    /// A library with one folder, `<temp>/<volume>/music`.
    fn library_on(temp: &Path, volume: &str) -> Library {
        let folder = temp.join(volume).join("music");
        std::fs::create_dir_all(&folder).unwrap();
        let lib = Library::open(&temp.join("library.sqlite3")).unwrap();
        library::add_folder(&lib, &folder).unwrap();
        lib
    }

    fn mounts_at(temp: &Path) -> Mounts {
        Mounts {
            dirs: vec![dunce::canonicalize(temp).unwrap()],
        }
    }

    #[test]
    fn lists_mounted_dirs_but_not_the_library_drive() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("A")).unwrap();
        let lib = library_on(temp.path(), "B");
        let mounts = mounts_at(temp.path());
        assert!(
            mounts
                .dirs
                .iter()
                .all(|d| d.starts_with(dunce::canonicalize(temp.path()).unwrap()))
        );
        let sticks = list_sticks(&lib.reader().unwrap(), &mounts).unwrap();
        let names: Vec<&str> = sticks.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["A"]);
        assert_eq!(sticks[0].root, mounts.dirs[0].join("A"));
    }

    #[test]
    fn sticks_are_sorted_by_name() {
        let temp = tempfile::tempdir().unwrap();
        for name in ["zed", "Alpha", "mid"] {
            std::fs::create_dir_all(temp.path().join(name)).unwrap();
        }
        let lib = library_on(temp.path(), "lib");
        let sticks = list_sticks(&lib.reader().unwrap(), &mounts_at(temp.path())).unwrap();
        let names: Vec<&str> = sticks.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["Alpha", "mid", "zed"]);
    }

    #[test]
    fn skips_hidden_and_root_links() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join(".Trashes")).unwrap();
        std::fs::create_dir_all(temp.path().join("Stick")).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("/", temp.path().join("Macintosh HD")).unwrap();
        let lib = library_on(temp.path(), "lib");
        let sticks = list_sticks(&lib.reader().unwrap(), &mounts_at(temp.path())).unwrap();
        let names: Vec<&str> = sticks.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["Stick"]);
    }

    #[test]
    fn a_missing_mount_dir_lists_nothing() {
        let temp = tempfile::tempdir().unwrap();
        let lib = library_on(temp.path(), "lib");
        let mounts = Mounts {
            dirs: vec![temp.path().join("nowhere")],
        };
        assert!(
            list_sticks(&lib.reader().unwrap(), &mounts)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn letter_roots_skip_the_system_drive_and_missing_ones() {
        let present = |root: &Path| ["C:\\", "D:\\", "F:\\"].contains(&root.to_str().unwrap());
        assert_eq!(
            letter_roots("C:", present),
            [PathBuf::from("D:\\"), PathBuf::from("F:\\")]
        );
        assert_eq!(letter_roots("d:", present), [PathBuf::from("F:\\")]);
    }

    #[test]
    fn a_root_candidate_is_never_a_stick() {
        let root = Path::new("/").to_path_buf();
        let found = sticks_among(vec![root], &[]);
        if cfg!(unix) {
            assert!(found.is_empty());
        }
    }

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
