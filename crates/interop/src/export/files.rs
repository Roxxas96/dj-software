//! Writes that never leave a half-written file under a real name.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use super::{ExportError, PART_SUFFIX};

fn io(path: &Path) -> impl FnOnce(std::io::Error) -> ExportError + '_ {
    move |source| ExportError::Io {
        path: path.to_path_buf(),
        source,
    }
}

fn part_path(path: &Path) -> PathBuf {
    let mut name = OsString::from(path.as_os_str());
    name.push(PART_SUFFIX);
    PathBuf::from(name)
}

fn create_parent(path: &Path) -> Result<(), ExportError> {
    match path.parent() {
        Some(parent) => fs::create_dir_all(parent).map_err(io(parent)),
        None => Ok(()),
    }
}

fn sync(path: &Path) -> Result<(), ExportError> {
    fs::OpenOptions::new()
        .write(true)
        .open(path)
        .and_then(|f| f.sync_all())
        .map_err(io(path))
}

/// Writes `bytes` to `path` through `<path>.ojpart`, creating parent folders.
///
/// # Errors
///
/// [`ExportError::Io`] when a folder or file cannot be written; the temporary file is removed.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), ExportError> {
    create_parent(path)?;
    let part = part_path(path);
    let result = fs::write(&part, bytes)
        .map_err(io(&part))
        .and_then(|()| sync(&part))
        .and_then(|()| fs::rename(&part, path).map_err(io(path)));
    if result.is_err() {
        let _ = fs::remove_file(&part);
    }
    result
}

/// Copies `from` to `to` through `<to>.ojpart`, runs `then` on the temporary file (to tag it),
/// and renames it into place.
///
/// # Errors
///
/// [`ExportError::Io`] when the copy fails, or whatever `then` returns; the temporary file is
/// removed.
pub fn copy_atomic(
    from: &Path,
    to: &Path,
    then: impl FnOnce(&Path) -> Result<(), ExportError>,
) -> Result<(), ExportError> {
    create_parent(to)?;
    let part = part_path(to);
    let result = fs::copy(from, &part)
        .map_err(io(from))
        .and_then(|_| then(&part))
        .and_then(|()| sync(&part))
        .and_then(|()| fs::rename(&part, to).map_err(io(to)));
    if result.is_err() {
        let _ = fs::remove_file(&part);
    }
    result
}

/// Deletes every regular `*.ojpart` file below `dir`, without following symlinks, and says how
/// many it removed. Folders are never deleted. A missing `dir` has none.
///
/// # Errors
///
/// [`ExportError::Io`] when a folder cannot be read or a file cannot be removed.
pub fn remove_leftovers(dir: &Path) -> Result<usize, ExportError> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(io(dir)(e)),
    };
    let mut removed = 0;
    for entry in entries {
        let entry = entry.map_err(io(dir))?;
        let path = entry.path();
        let kind = entry.file_type().map_err(io(&path))?;
        if kind.is_dir() {
            removed += remove_leftovers(&path)?;
        } else if kind.is_file() && path.to_string_lossy().ends_with(PART_SUFFIX) {
            fs::remove_file(&path).map_err(io(&path))?;
            removed += 1;
        }
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_atomic_leaves_no_part_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a").join("b.txt");
        write_atomic(&path, b"hi").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"hi");
        assert!(!dir.path().join("a").join("b.txt.ojpart").exists());
    }

    #[test]
    fn copy_atomic_runs_then_on_the_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let from = dir.path().join("from.bin");
        std::fs::write(&from, b"abc").unwrap();
        let to = dir.path().join("x").join("to.bin");
        copy_atomic(&from, &to, |part| {
            assert!(part.to_string_lossy().ends_with(".ojpart"));
            std::fs::OpenOptions::new()
                .append(true)
                .open(part)
                .and_then(|mut f| std::io::Write::write_all(&mut f, b"d"))
                .map_err(|source| crate::export::ExportError::Io {
                    path: part.into(),
                    source,
                })
        })
        .unwrap();
        assert_eq!(std::fs::read(&to).unwrap(), b"abcd");
        assert!(!dir.path().join("x").join("to.bin.ojpart").exists());
    }

    #[test]
    fn remove_leftovers_deletes_only_part_files() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("sub");
        std::fs::create_dir_all(sub.join("dir.ojpart")).unwrap();
        std::fs::write(sub.join("t.mp3.ojpart"), b"x").unwrap();
        std::fs::write(sub.join("a.mp3"), b"x").unwrap();
        assert_eq!(remove_leftovers(dir.path()).unwrap(), 1);
        assert!(!sub.join("t.mp3.ojpart").exists());
        assert!(sub.join("a.mp3").exists());
        assert!(sub.join("dir.ojpart").is_dir());
    }
}
