//! Reading tags. Files are only ever read, never written.

use std::path::Path;

use lofty::file::{AudioFile, TaggedFileExt};
use lofty::tag::Accessor;

/// What the scanner stores about a file besides its fingerprint.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TrackTags {
    /// The tagged title, or the file stem when there is none.
    pub title: String,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub genre: Option<String>,
    /// Length in frames at `sample_rate`, from the file's properties.
    pub duration_frames: Option<u64>,
    pub sample_rate: Option<u32>,
}

/// Reads `path`'s tags and properties with lofty. Missing or unreadable tags give
/// defaults (the title falls back to the file stem); this never fails.
#[must_use]
pub fn read(path: &Path) -> TrackTags {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let Ok(file) = lofty::read_from_path(path) else {
        return TrackTags {
            title: stem,
            ..TrackTags::default()
        };
    };
    let tag = file.primary_tag().or_else(|| file.first_tag());
    let text = |value: Option<std::borrow::Cow<'_, str>>| {
        value.map(|v| v.trim().to_owned()).filter(|v| !v.is_empty())
    };
    let properties = file.properties();
    let sample_rate = properties.sample_rate().filter(|&r| r > 0);
    let duration = properties.duration();
    let duration_frames = sample_rate
        .filter(|_| !duration.is_zero())
        .map(|rate| frames(duration.as_secs_f64(), rate));
    TrackTags {
        title: text(tag.and_then(Accessor::title)).unwrap_or(stem),
        artist: text(tag.and_then(Accessor::artist)),
        album: text(tag.and_then(Accessor::album)),
        genre: text(tag.and_then(Accessor::genre)),
        duration_frames,
        sample_rate,
    }
}

/// Non-negative and far below `u64::MAX` for any real file; the cast saturates anyway.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn frames(seconds: f64, rate: u32) -> u64 {
    (seconds * f64::from(rate)).round() as u64
}
