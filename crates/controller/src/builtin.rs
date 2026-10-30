//! Mappings shipped with the app, and finding the mapping to load: a file path, a user
//! override in the config directory, or a built-in.

use std::path::{Path, PathBuf};

use directories::ProjectDirs;

use crate::mapping::{Mapping, MappingError, parse_mapping};

/// Built-in mappings by name.
const BUILTINS: &[(&str, &str)] = &[(
    "ddj-sb3",
    include_str!("../../../controllers/ddj-sb3/mapping.toml"),
)];

/// The names of the built-in mappings.
pub fn builtin_names() -> impl Iterator<Item = &'static str> {
    BUILTINS.iter().map(|(name, _)| *name)
}

/// The text of the built-in mapping `name`.
pub fn builtin_mapping(name: &str) -> Option<&'static str> {
    BUILTINS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, text)| *text)
}

/// The app's configuration directory, where `controllers/<name>/mapping.toml` overrides a
/// built-in mapping.
pub fn config_dir() -> Option<PathBuf> {
    ProjectDirs::from("", "", "dj-software").map(|dirs| dirs.config_dir().to_path_buf())
}

/// Loads a mapping. An existing path is loaded as a file. Otherwise `name_or_path` names
/// a mapping: the user's override in [`config_dir`] wins over the built-in.
pub fn load_mapping(name_or_path: &str) -> Result<Mapping, MappingError> {
    let path = Path::new(name_or_path);
    if path.exists() {
        return load_file(path);
    }
    load_mapping_from(name_or_path, config_dir().as_deref())
}

/// [`load_mapping`] for a name, with the configuration directory given (for tests).
#[doc(hidden)]
pub fn load_mapping_from(name: &str, config_dir: Option<&Path>) -> Result<Mapping, MappingError> {
    let user = config_dir.map(|dir| dir.join("controllers").join(name).join("mapping.toml"));
    if let Some(path) = &user
        && path.is_file()
    {
        return load_file(path);
    }
    if let Some(text) = builtin_mapping(name) {
        return parse_mapping(&format!("built-in mapping {name}"), text);
    }
    let looked_in = user
        .map(|path| format!(" (no file {})", path.display()))
        .unwrap_or_default();
    Err(MappingError {
        file: name.to_owned(),
        message: format!(
            "no such mapping file or mapping name{looked_in}; built-in mappings: {}",
            builtin_names().collect::<Vec<_>>().join(", ")
        ),
    })
}

fn load_file(path: &Path) -> Result<Mapping, MappingError> {
    let name = path.display().to_string();
    let text = std::fs::read_to_string(path).map_err(|e| MappingError {
        file: name.clone(),
        message: e.to_string(),
    })?;
    parse_mapping(&name, &text)
}
