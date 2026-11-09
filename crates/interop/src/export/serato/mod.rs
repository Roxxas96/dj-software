//! Serato's file formats: crates, `database V2` and the cue tags in the audio files.

pub mod markers;
pub mod tags;
pub mod tlv;

use std::collections::{HashMap, HashSet};
use std::path::Path;

use super::run::{FormatOutput, StickContext, StickFormat};
use super::{ExportError, files, safe_component};

/// The folder on the stick that Serato reads.
const SERATO_DIR: &str = "_Serato_";
const SUBCRATES: &str = "Subcrates";
const DATABASE: &str = "database V2";

/// Writes the library files Serato reads from a stick: one crate per playlist, and our tracks
/// merged into `database V2`.
#[derive(Debug, Clone, Copy, Default)]
pub struct SeratoFormat;

fn exists(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok()
}

impl StickFormat for SeratoFormat {
    fn name(&self) -> &'static str {
        "Serato"
    }

    fn write_library(&self, cx: &StickContext<'_>) -> Result<FormatOutput, ExportError> {
        let plan = cx.plan;
        let stick = plan.stick.as_path();
        let serato = stick.join(SERATO_DIR);
        let subcrates = serato.join(SUBCRATES);
        let paths: HashMap<i64, &str> = plan
            .tracks
            .iter()
            .map(|t| (t.track.id, t.stick_path.as_str()))
            .collect();
        let ours: HashSet<&str> = plan
            .previous
            .iter()
            .flat_map(|m| m.crates.iter().map(String::as_str))
            .collect();

        let mut output = FormatOutput::default();
        let mut taken = HashSet::new();
        for playlist in &plan.playlists {
            let base = safe_component(&playlist.name.replace("%%", "_"), 120);
            let file = unique_crate_name(&base, &subcrates, &ours, &taken);
            taken.insert(file.to_lowercase());
            let listed: Vec<&str> = playlist
                .tracks
                .iter()
                .filter_map(|id| paths.get(id).copied())
                .collect();
            files::write_atomic(stick, &subcrates.join(&file), &tlv::crate_file(&listed))?;
            output.files.push(file);
        }

        let database = serato.join(DATABASE);
        let existing = match std::fs::symlink_metadata(&database) {
            Ok(meta) if meta.file_type().is_file() => {
                Some(std::fs::read(&database).map_err(|source| ExportError::Io {
                    path: database.clone(),
                    source,
                })?)
            }
            Ok(_) => {
                return Err(ExportError::Refused(format!(
                    "{} is not a regular file; Open Jockey will not touch it",
                    database.display()
                )));
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(source) => {
                return Err(ExportError::Io {
                    path: database,
                    source,
                });
            }
        };
        let records = plan
            .tracks
            .iter()
            .map(|t| tlv::database_record(&t.track, &t.stick_path, cx.now_unix))
            .collect();
        let merged = tlv::merge_database(existing.as_deref(), records).map_err(|why| {
            ExportError::Format {
                path: database.clone(),
                why,
            }
        })?;
        files::write_atomic(stick, &database, &merged)?;
        Ok(output)
    }
}

/// `<base>.crate`, unless a crate of that name is on the stick and is not ours or is already
/// used by another playlist of this export; then ` (Open Jockey)` is added, and a number.
fn unique_crate_name(
    base: &str,
    subcrates: &Path,
    ours: &HashSet<&str>,
    taken: &HashSet<String>,
) -> String {
    let free = |file: &str| {
        !taken.contains(&file.to_lowercase())
            && (ours.contains(file) || !exists(&subcrates.join(file)))
    };
    let plain = format!("{base}.crate");
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
        let name = format!("{cut}{suffix}.crate");
        if free(&name) {
            return name;
        }
        n += 1;
    }
}
