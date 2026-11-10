//! Serato's file formats: crates, `database V2` and the cue tags in the audio files.

pub mod markers;
pub mod tags;
pub mod tlv;

use std::collections::{HashMap, HashSet};

use super::run::{FormatOutput, StickContext, StickFormat};
use super::{ExportError, ExportPlan, files, name_key, safe_component};

/// The folder on the stick that Serato reads.
pub(crate) const SERATO_DIR: &str = "_Serato_";
/// The folder inside [`SERATO_DIR`] that holds the crates.
pub(crate) const SUBCRATES: &str = "Subcrates";
const DATABASE: &str = "database V2";

/// Refuses a stick whose `database V2` is there but is not a regular file or does not parse,
/// so an export never starts on a database it would fail to merge into.
pub(crate) fn check_database(stick: &std::path::Path) -> Result<(), ExportError> {
    let path = stick.join(SERATO_DIR).join(DATABASE);
    let Some(bytes) = files::read_existing(&path)? else {
        return Ok(());
    };
    tlv::decode(&bytes).map(drop).map_err(|why| {
        ExportError::Refused(format!(
            "{} cannot be read as Serato's database ({why}); Open Jockey will not change it",
            path.display()
        ))
    })
}

/// The crate files an earlier export wrote, by exact name.
fn ours(plan: &ExportPlan) -> HashSet<&str> {
    plan.previous
        .iter()
        .flat_map(|m| m.crates.iter().map(String::as_str))
        .collect()
}

/// The crate file name of each playlist of `plan`, in order, as [`SeratoFormat`] writes them:
/// worked out before anything is written, so the manifest can claim them first.
pub(crate) fn crate_names(plan: &ExportPlan) -> Vec<String> {
    let subcrates = plan.stick.join(SERATO_DIR).join(SUBCRATES);
    let ours = ours(plan);
    let mut taken = HashSet::new();
    plan.playlists
        .iter()
        .map(|playlist| {
            let base = safe_component(&playlist.name.replace("%%", "_"), 120);
            let file = files::unique_name(&base, ".crate", &subcrates, &ours, &taken);
            taken.insert(name_key(&file));
            file
        })
        .collect()
}

/// Writes the library files Serato reads from a stick: one crate per playlist, and our tracks
/// merged into `database V2`.
#[derive(Debug, Clone, Copy, Default)]
pub struct SeratoFormat;

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
        let ours = ours(plan);

        let mut output = FormatOutput::default();
        for (playlist, file) in plan.playlists.iter().zip(crate_names(plan)) {
            let listed: Vec<&str> = playlist
                .tracks
                .iter()
                .filter_map(|id| paths.get(id).copied())
                .collect();
            files::write_atomic(stick, &subcrates.join(&file), &tlv::crate_file(&listed))?;
            files::settle_case(&subcrates, &file, &ours)?;
            output.files.push(file);
        }

        let database = serato.join(DATABASE);
        let existing = files::read_existing(&database)?;
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
