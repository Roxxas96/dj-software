//! Writes that never leave a half-written file under a real name.

use std::collections::HashSet;
use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};

use super::{ExportError, PART_SUFFIX, safe_component};

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

/// Checks that no existing folder between `stick` and the parent of `path` is a link, creates
/// the missing ones, and checks that the parent really lies inside the stick.
fn prepare_parent(stick: &Path, path: &Path) -> Result<(), ExportError> {
    let outside = || {
        ExportError::Refused(format!(
            "{} is not inside the stick {}",
            path.display(),
            stick.display()
        ))
    };
    let Some(parent) = path.parent() else {
        return Err(outside());
    };
    let relative = parent.strip_prefix(stick).map_err(|_| outside())?;
    let mut at = stick.to_path_buf();
    for part in relative.components() {
        if !matches!(part, Component::Normal(_)) {
            return Err(outside());
        }
        at.push(part);
        match fs::symlink_metadata(&at) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(ExportError::Refused(format!(
                    "{} is a link on the stick; Open Jockey does not write through links",
                    at.display()
                )));
            }
            Ok(meta) if meta.is_dir() => {}
            Ok(_) => {
                return Err(io(&at)(std::io::Error::other("not a folder")));
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(&at).map_err(io(&at))?;
            }
            Err(e) => return Err(io(&at)(e)),
        }
    }
    let canon_stick = dunce::canonicalize(stick).map_err(io(stick))?;
    let canon_parent = dunce::canonicalize(parent).map_err(io(parent))?;
    if canon_parent.starts_with(&canon_stick) {
        Ok(())
    } else {
        Err(outside())
    }
}

/// Opens `part` for writing as a new file; a regular file or link left there is removed first,
/// a folder makes the write fail.
fn create_part(part: &Path) -> Result<fs::File, ExportError> {
    match fs::symlink_metadata(part) {
        Ok(meta) if meta.is_dir() => {
            return Err(io(part)(std::io::Error::other("a folder is in the way")));
        }
        Ok(_) => fs::remove_file(part).map_err(io(part))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(io(part)(e)),
    }
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(part)
        .map_err(io(part))
}

fn sync(path: &Path) -> Result<(), ExportError> {
    fs::OpenOptions::new()
        .write(true)
        .open(path)
        .and_then(|f| f.sync_all())
        .map_err(io(path))
}

/// Writes `bytes` to `path` through `<path>.ojpart`, creating parent folders. `path` must lie
/// inside `stick`, and no folder on the way may be a link.
///
/// # Errors
///
/// [`ExportError::Refused`] when `path` is outside `stick` or reached through a link,
/// [`ExportError::Io`] when a folder or file cannot be written; the temporary file is removed.
pub fn write_atomic(stick: &Path, path: &Path, bytes: &[u8]) -> Result<(), ExportError> {
    prepare_parent(stick, path)?;
    let part = part_path(path);
    let mut file = create_part(&part)?;
    let result = file
        .write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(io(&part))
        .and_then(|()| {
            drop(file);
            fs::rename(&part, path).map_err(io(path))
        });
    if result.is_err() {
        let _ = fs::remove_file(&part);
    }
    result
}

/// `<base><ext>`, unless a file of that name is in `dir` and is not ours or is already used
/// by another playlist of this export (`taken`, lowercase); then ` (Open Jockey)` is added to
/// the base, and a number.
pub(super) fn unique_name(
    base: &str,
    ext: &str,
    dir: &Path,
    ours: &HashSet<&str>,
    taken: &HashSet<String>,
) -> String {
    let free = |file: &str| {
        !taken.contains(&file.to_lowercase())
            && (ours.contains(file) || fs::symlink_metadata(dir.join(file)).is_err())
    };
    let plain = format!("{base}{ext}");
    if free(&plain) {
        return plain;
    }
    let mut n = 1;
    loop {
        let suffix = if n == 1 {
            " (Open Jockey)".to_string()
        } else {
            format!(" (Open Jockey {n})")
        };
        // Cut the base, not the suffix, so every candidate differs.
        let cut = safe_component(base, 120usize.saturating_sub(suffix.chars().count()).max(1));
        let name = format!("{cut}{suffix}{ext}");
        if free(&name) {
            return name;
        }
        n += 1;
    }
}

/// Copies `from` to `to` through `<to>.ojpart`, runs `then` on the temporary file (to tag it),
/// and renames it into place. `to` must lie inside `stick`, and no folder on the way may be a
/// link. The temporary file is never read-only, whatever `from` is.
///
/// # Errors
///
/// [`ExportError::Refused`] when `to` is outside `stick` or reached through a link,
/// [`ExportError::Io`] when the copy fails, or whatever `then` returns; the temporary file is
/// removed.
pub fn copy_atomic(
    stick: &Path,
    from: &Path,
    to: &Path,
    then: impl FnOnce(&Path) -> Result<(), ExportError>,
) -> Result<(), ExportError> {
    prepare_parent(stick, to)?;
    let part = part_path(to);
    let mut source = fs::File::open(from).map_err(io(from))?;
    let mut file = create_part(&part)?;
    let result = std::io::copy(&mut source, &mut file)
        .map_err(io(from))
        .and_then(|_| {
            drop(file);
            // The handle was created writable; this keeps that true if a platform copies flags.
            let mut perms = fs::metadata(&part).map_err(io(&part))?.permissions();
            #[allow(clippy::permissions_set_readonly_false)]
            perms.set_readonly(false);
            fs::set_permissions(&part, perms).map_err(io(&part))
        })
        .and_then(|()| then(&part))
        .and_then(|()| sync(&part))
        .and_then(|()| fs::rename(&part, to).map_err(io(to)));
    if result.is_err() {
        let _ = fs::remove_file(&part);
    }
    result
}

/// Deletes `path` if it is a regular file, without following links; a link, a folder or a
/// missing path is left alone. Says whether it deleted anything.
///
/// # Errors
///
/// [`ExportError::Io`] when the file cannot be removed.
pub fn remove_regular_file(path: &Path) -> Result<bool, ExportError> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_file() => {
            fs::remove_file(path).map_err(io(path))?;
            Ok(true)
        }
        Ok(_) => Ok(false),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(io(path)(e)),
    }
}

/// Deletes every `*.ojpart` regular file or link below `dir`, without following links (a link's
/// target is untouched), and says how many it removed. Folders are never deleted. A missing `dir` has none.
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
        } else if (kind.is_file() || kind.is_symlink())
            && path.to_string_lossy().ends_with(PART_SUFFIX)
        {
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
        write_atomic(dir.path(), &path, b"hi").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"hi");
        assert!(!dir.path().join("a").join("b.txt.ojpart").exists());
    }

    #[test]
    fn copy_atomic_runs_then_on_the_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let from = dir.path().join("from.bin");
        std::fs::write(&from, b"abc").unwrap();
        let to = dir.path().join("x").join("to.bin");
        copy_atomic(dir.path(), &from, &to, |part| {
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

    #[test]
    fn read_only_source_copies_and_stays_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let from = dir.path().join("from.bin");
        std::fs::write(&from, b"abc").unwrap();
        let mut perms = std::fs::metadata(&from).unwrap().permissions();
        perms.set_readonly(true);
        std::fs::set_permissions(&from, perms).unwrap();
        let to = dir.path().join("x").join("to.bin");
        copy_atomic(dir.path(), &from, &to, |part| {
            assert!(!std::fs::metadata(part).unwrap().permissions().readonly());
            sync(part)
        })
        .unwrap();
        assert_eq!(std::fs::read(&to).unwrap(), b"abc");
        assert!(std::fs::metadata(&from).unwrap().permissions().readonly());
        assert_eq!(std::fs::read(&from).unwrap(), b"abc");
        assert!(!std::fs::metadata(&to).unwrap().permissions().readonly());
    }

    #[test]
    fn write_outside_the_stick_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let stick = dir.path().join("stick");
        std::fs::create_dir(&stick).unwrap();
        let err = write_atomic(&stick, &dir.path().join("o.txt"), b"x").unwrap_err();
        assert!(matches!(err, ExportError::Refused(_)));
        assert!(!dir.path().join("o.txt").exists());
    }

    #[cfg(unix)]
    #[test]
    fn write_through_a_linked_folder_fails_and_leaves_nothing_outside() {
        let dir = tempfile::tempdir().unwrap();
        let stick = dir.path().join("stick");
        let outside = dir.path().join("outside");
        std::fs::create_dir_all(&stick).unwrap();
        std::fs::create_dir(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, stick.join("Open Jockey")).unwrap();
        let target = stick.join("Open Jockey").join("export.toml");
        assert!(write_atomic(&stick, &target, b"x").is_err());
        let from = dir.path().join("f.bin");
        std::fs::write(&from, b"abc").unwrap();
        assert!(copy_atomic(&stick, &from, &target, |_| Ok(())).is_err());
        assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
    }

    #[cfg(unix)]
    #[test]
    fn a_linked_part_file_is_replaced_not_followed() {
        let dir = tempfile::tempdir().unwrap();
        let stick = dir.path().join("stick");
        std::fs::create_dir_all(&stick).unwrap();
        let victim = dir.path().join("victim.txt");
        std::fs::write(&victim, b"keep").unwrap();
        std::os::unix::fs::symlink(&victim, stick.join("a.txt.ojpart")).unwrap();
        write_atomic(&stick, &stick.join("a.txt"), b"new").unwrap();
        assert_eq!(std::fs::read(&victim).unwrap(), b"keep");
        assert_eq!(std::fs::read(stick.join("a.txt")).unwrap(), b"new");
    }

    #[cfg(unix)]
    #[test]
    fn remove_leftovers_removes_a_part_link_not_its_target() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target.txt");
        std::fs::write(&target, b"keep").unwrap();
        let stick = dir.path().join("stick");
        std::fs::create_dir(&stick).unwrap();
        std::os::unix::fs::symlink(&target, stick.join("t.mp3.ojpart")).unwrap();
        assert_eq!(remove_leftovers(&stick).unwrap(), 1);
        assert!(std::fs::symlink_metadata(stick.join("t.mp3.ojpart")).is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"keep");
    }

    #[cfg(unix)]
    #[test]
    fn remove_regular_file_skips_links() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target.txt");
        std::fs::write(&target, b"keep").unwrap();
        let link = dir.path().join("l.txt");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(!remove_regular_file(&link).unwrap());
        assert!(std::fs::symlink_metadata(&link).is_ok());
        assert!(remove_regular_file(&target).unwrap());
        assert!(!remove_regular_file(&target).unwrap());
    }
}
