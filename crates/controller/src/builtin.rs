//! Mappings shipped with the app, and finding the mapping to load: a file path, a user
//! override in the config directory, or a built-in. Also loads the mappings an `extends`
//! names, refusing cycles.

use std::path::{Path, PathBuf};

use directories::ProjectDirs;

use crate::mapping::file::parse_with;
use crate::mapping::{Mapping, MappingError};

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
    ProjectDirs::from("", "", "open-jockey").map(|dirs| dirs.config_dir().to_path_buf())
}

/// Loads a mapping. An existing file is loaded as such (a directory is not). Otherwise `name_or_path` names
/// a mapping: the user's override in [`config_dir`] wins over the built-in.
pub fn load_mapping(name_or_path: &str) -> Result<Mapping, MappingError> {
    let path = Path::new(name_or_path);
    if path.is_file() {
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
        return Loader::default().builtin(name, text);
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
    Loader::default().file(path)
}

/// [`crate::mapping::parse_mapping`]: a mapping given as text, whose relative `extends`
/// paths are read from the working directory.
pub(crate) fn parse_text(name: &str, text: &str) -> Result<Mapping, MappingError> {
    Loader::default().text(name, Origin::Text, text, None)
}

/// Where a mapping's text came from, to recognize a mapping already being loaded.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Origin {
    Builtin(String),
    /// A canonical path.
    File(PathBuf),
    /// Text given directly, which nothing can extend.
    Text,
}

/// Loads a mapping and the chain of mappings it extends.
#[derive(Default)]
struct Loader {
    /// The mappings being loaded, outermost first: their origin and name for messages.
    chain: Vec<(Origin, String)>,
}

impl Loader {
    fn builtin(&mut self, name: &str, text: &str) -> Result<Mapping, MappingError> {
        let origin = Origin::Builtin(name.to_owned());
        self.text(&format!("built-in mapping {name}"), origin, text, None)
    }

    fn file(&mut self, path: &Path) -> Result<Mapping, MappingError> {
        let name = path.display().to_string();
        let error = |message: String| MappingError {
            file: name.clone(),
            message,
        };
        let text = std::fs::read_to_string(path).map_err(|e| error(e.to_string()))?;
        let canonical = path.canonicalize().map_err(|e| error(e.to_string()))?;
        let folder = canonical.parent().map(Path::to_path_buf);
        self.text(&name, Origin::File(canonical), &text, folder.as_deref())
    }

    /// Parses `text`, the mapping `name` from `origin`; a relative `extends` path is read
    /// from `folder` (the working directory when `None`).
    fn text(
        &mut self,
        name: &str,
        origin: Origin,
        text: &str,
        folder: Option<&Path>,
    ) -> Result<Mapping, MappingError> {
        self.chain.push((origin, name.to_owned()));
        let result = parse_with(name, text, &mut |parent| self.parent(name, parent, folder));
        self.chain.pop();
        result
    }

    /// Loads the mapping `parent` that the mapping `child` extends: a built-in name, or else
    /// a path, relative to `folder`.
    fn parent(
        &mut self,
        child: &str,
        parent: &str,
        folder: Option<&Path>,
    ) -> Result<Mapping, MappingError> {
        let error = |message: String| MappingError {
            file: child.to_owned(),
            message: format!("extends: {message}"),
        };
        let builtin = builtin_mapping(parent);
        let path = match folder {
            Some(folder) => folder.join(parent),
            None => PathBuf::from(parent),
        };
        let origin = match builtin {
            Some(_) => Origin::Builtin(parent.to_owned()),
            None if path.is_file() => {
                Origin::File(path.canonicalize().map_err(|e| error(e.to_string()))?)
            }
            None => {
                return Err(error(format!(
                    "`{parent}` is neither a built-in mapping ({}) nor a file (no file {})",
                    builtin_names().collect::<Vec<_>>().join(", "),
                    path.display()
                )));
            }
        };
        if let Some(at) = self.chain.iter().position(|(o, _)| *o == origin) {
            let mut names: Vec<&str> = self.chain[at..].iter().map(|(_, n)| n.as_str()).collect();
            names.push(&self.chain[at].1);
            return Err(error(format!(
                "`{parent}` makes a cycle: {}",
                names.join(" extends ")
            )));
        }
        match builtin {
            Some(text) => self.builtin(parent, text),
            None => self.file(&path),
        }
    }
}
