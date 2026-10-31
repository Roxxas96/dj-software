//! Key/value settings stored in the library, and the BPM range setting's format.
//!
//! Settings are written with [`crate::WriteOp::SetSetting`] and read with
//! [`Reader::setting`]. Keys in use:
//! - [`BPM_RANGE`]: the tempo range detected BPMs are folded into, as `"MIN-MAX"`
//!   (default `"90-180"`).
//! - [`KEY_NOTATION`]: how keys are shown (`"camelot"` by default).

use analysis::BpmRange;
use rusqlite::OptionalExtension;

use crate::{Library, LibraryError, Reader};

/// Setting key for the BPM range, stored as `"MIN-MAX"` such as `"90-180"`.
pub const BPM_RANGE: &str = "bpm_range";
/// Setting key for the key notation, such as `"camelot"`.
pub const KEY_NOTATION: &str = "key_notation";

/// Bounds a BPM range must lie within (the beatgrid detector rejects anything wider).
const MIN_BPM: f64 = 30.0;
const MAX_BPM: f64 = 480.0;
/// Below this minimum, [`validate_bpm_range`] warns.
const LOW_MIN_BPM: f64 = 70.0;

/// The warning [`validate_bpm_range`] returns for a minimum below 70 BPM.
pub const LOW_BPM_RANGE_WARNING: &str =
    "ranges below ~70 BPM let half-time tracks (dubstep) read as half tempo";

/// Why a BPM range string was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum BpmRangeError {
    #[error("expected MIN-MAX, such as 90-180")]
    Format,
    #[error("BPM range bounds must lie within 30-480")]
    OutOfBounds,
    #[error("the minimum BPM must be below the maximum")]
    NotAscending,
    #[error(
        "the BPM range must span at least an octave (the maximum at least twice the \
         minimum), or some tempos cannot be folded into it"
    )]
    TooNarrow,
}

/// Parses `"MIN-MAX"` (decimals allowed, spaces around the numbers ignored). Both bounds
/// must lie within 30–480, `MIN < MAX`, and `MAX >= 2 * MIN` so every tempo has an octave
/// inside the range.
///
/// # Errors
///
/// The first rule `text` breaks.
pub fn parse_bpm_range(text: &str) -> Result<BpmRange, BpmRangeError> {
    let (min, max) = text.split_once('-').ok_or(BpmRangeError::Format)?;
    let parse = |s: &str| {
        s.trim()
            .parse::<f64>()
            .ok()
            .filter(|v| v.is_finite())
            .ok_or(BpmRangeError::Format)
    };
    let (min, max) = (parse(min)?, parse(max)?);
    let bounds = MIN_BPM..=MAX_BPM;
    if !bounds.contains(&min) || !bounds.contains(&max) {
        return Err(BpmRangeError::OutOfBounds);
    }
    if min >= max {
        return Err(BpmRangeError::NotAscending);
    }
    if max < 2.0 * min {
        return Err(BpmRangeError::TooNarrow);
    }
    Ok(BpmRange { min, max })
}

/// Formats a range the way [`parse_bpm_range`] reads it, such as `"90-180"`.
#[must_use]
pub fn format_bpm_range(range: BpmRange) -> String {
    format!("{}-{}", range.min, range.max)
}

/// Validates a range the user typed: [`parse_bpm_range`], plus
/// [`LOW_BPM_RANGE_WARNING`] when the minimum is below 70 BPM, for the caller to show.
///
/// # Errors
///
/// As [`parse_bpm_range`].
pub fn validate_bpm_range(text: &str) -> Result<(BpmRange, Option<&'static str>), BpmRangeError> {
    let range = parse_bpm_range(text)?;
    let warning = (range.min < LOW_MIN_BPM).then_some(LOW_BPM_RANGE_WARNING);
    Ok((range, warning))
}

/// A stored setting value that could not be used, so the default applies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidBpmRange {
    pub value: String,
    pub error: BpmRangeError,
}

impl Reader {
    /// The value of setting `key`, if set.
    ///
    /// # Errors
    ///
    /// Any database error.
    pub fn setting(&self, key: &str) -> Result<Option<String>, LibraryError> {
        Ok(self
            .connection()
            .prepare_cached("SELECT value FROM settings WHERE key = ?1")?
            .query_row([key], |r| r.get::<_, Option<String>>(0))
            .optional()?
            .flatten())
    }

    /// The BPM range to analyze with: the [`BPM_RANGE`] setting, or the 90–180 default
    /// when it is unset. A stored value that does not parse also yields the default, and
    /// is returned alongside so the caller can report it; it never fails the analysis.
    ///
    /// # Errors
    ///
    /// Any database error.
    pub fn bpm_range(&self) -> Result<(BpmRange, Option<InvalidBpmRange>), LibraryError> {
        let Some(value) = self.setting(BPM_RANGE)? else {
            return Ok((BpmRange::default(), None));
        };
        Ok(match parse_bpm_range(&value) {
            Ok(range) => (range, None),
            Err(error) => (BpmRange::default(), Some(InvalidBpmRange { value, error })),
        })
    }
}

impl Library {
    /// [`Reader::setting`] on a fresh reader.
    ///
    /// # Errors
    ///
    /// Any database error.
    pub fn setting(&self, key: &str) -> Result<Option<String>, LibraryError> {
        self.reader()?.setting(key)
    }
}
