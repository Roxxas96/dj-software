//! What every stick format reads and writes; the export run itself comes later.

use super::{ExportError, ExportPlan};

/// What every format reads; each `PlannedTrack` carries its `stick_path`.
pub struct StickContext<'a> {
    pub plan: &'a ExportPlan,
    /// The time to record as "added", in Unix seconds.
    pub now_unix: u32,
}

/// What a format wrote, for the manifest.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FormatOutput {
    /// Names of the files written that belong to Open Jockey alone, for the manifest.
    pub files: Vec<String>,
    /// Things worth telling the user.
    pub notes: Vec<String>,
}

/// One app's library files on the stick.
pub trait StickFormat {
    /// The app's name, as shown to the user.
    fn name(&self) -> &'static str;

    /// Writes this app's library files for the whole plan (copies are already in place).
    ///
    /// # Errors
    ///
    /// [`ExportError`] when a file cannot be read or written.
    fn write_library(&self, cx: &StickContext<'_>) -> Result<FormatOutput, ExportError>;
}
