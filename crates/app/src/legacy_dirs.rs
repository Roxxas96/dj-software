//! Moves the per-user folders of the app's former name (`dj-software`) to the current
//! ones, so a library and mapping overrides made before the rename are kept. The move
//! runs only when the app is about to use those folders, never for an explicit
//! `--library-db` or mapping file.

use std::io;
use std::path::Path;

use controller::mapping::{Mapping, MappingError};
use directories::ProjectDirs;

/// The app's name before it became Open Jockey.
const LEGACY_NAME: &str = "dj-software";
/// The app's name in per-user folder paths.
const NAME: &str = "open-jockey";

/// Moves the legacy data and configuration folders to the current ones, once: a
/// folder is only moved when the current one does not exist yet. Failures are
/// reported and do not stop the app.
pub fn migrate() {
    let (Some(old), Some(new)) = (
        ProjectDirs::from("", "", LEGACY_NAME),
        ProjectDirs::from("", "", NAME),
    ) else {
        return;
    };
    // On macOS both are the same folder: the first move takes it all, the second
    // finds the current folder already there.
    for (old, new) in [
        (old.data_dir(), new.data_dir()),
        (old.config_dir(), new.config_dir()),
    ] {
        match move_dir(old, new) {
            Ok(true) => eprintln!("moved {} to {}", old.display(), new.display()),
            Ok(false) => {}
            Err(e) => eprintln!(
                "warning: could not move {} to {}: {e}",
                old.display(),
                new.display()
            ),
        }
    }
}

/// [`controller::builtin::load_mapping`], after [`migrate`] when `name_or_path` names a
/// mapping (whose override lives in the configuration folder) rather than a file.
pub fn load_mapping(name_or_path: &str) -> Result<Mapping, MappingError> {
    if !Path::new(name_or_path).is_file() {
        migrate();
    }
    controller::builtin::load_mapping(name_or_path)
}

/// Renames `old` to `new` if `old` is a folder and nothing exists at `new`.
/// Returns whether it moved.
fn move_dir(old: &Path, new: &Path) -> io::Result<bool> {
    if new.symlink_metadata().is_ok() || !old.is_dir() {
        return Ok(false);
    }
    if let Some(parent) = new.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::rename(old, new)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("open-jockey-legacy-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn moves_the_legacy_folder_with_its_contents() {
        let dir = scratch("move");
        let old = dir.join("dj-software");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join("library.sqlite3"), b"db").unwrap();
        let new = dir.join("nested").join("open-jockey");

        assert!(move_dir(&old, &new).unwrap());
        assert!(!old.exists());
        assert_eq!(std::fs::read(new.join("library.sqlite3")).unwrap(), b"db");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn keeps_an_existing_current_folder() {
        let dir = scratch("keep");
        let old = dir.join("dj-software");
        let new = dir.join("open-jockey");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::create_dir_all(&new).unwrap();
        std::fs::write(new.join("library.sqlite3"), b"current").unwrap();

        assert!(!move_dir(&old, &new).unwrap());
        assert!(old.exists());
        assert_eq!(
            std::fs::read(new.join("library.sqlite3")).unwrap(),
            b"current"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn does_nothing_without_a_legacy_folder() {
        let dir = scratch("none");
        let new = dir.join("open-jockey");
        assert!(!move_dir(&dir.join("dj-software"), &new).unwrap());
        assert!(!new.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
