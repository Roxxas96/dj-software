//! The waveform zoom the window shows: changed by the window (a scroll, a pinch, the zoom
//! buttons) and by the `ui.waveform_zoom_in` / `ui.waveform_zoom_out` actions, and kept in
//! `ui-state.toml` next to `config.toml`, so the next launch starts where this one ended.
//! `ui.waveform_beats` in `config.toml` is only the zoom before that file exists.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// The file the zoom is kept in, next to `config.toml`.
pub(super) const STATE_FILE: &str = "ui-state.toml";
/// How long the zoom must stay put before it is saved, so a scroll saves once.
const SAVE_AFTER: Duration = Duration::from_secs(2);

/// The zoom and where it is saved.
#[derive(Debug)]
pub(super) struct Zoom {
    path: Option<PathBuf>,
    beats: f64,
    /// The fewest and most beats the zoom can show (`ui.waveform_min_beats` and
    /// `ui.waveform_max_beats`).
    range: (f64, f64),
    /// The actions' zoom count when last seen, `None` before the control loop runs.
    seen: Option<i32>,
    /// When the zoom last changed without being saved since.
    unsaved: Option<Instant>,
}

impl Zoom {
    /// The zoom saved at `path`, else `default`, brought within `range` (a zoom saved
    /// before the range was narrowed comes back at its nearest end).
    pub fn load(path: Option<PathBuf>, default: f64, range: (f64, f64)) -> Self {
        let beats = path
            .as_deref()
            .and_then(read)
            .unwrap_or(default)
            .clamp(range.0, range.1);
        Self {
            path,
            beats,
            range,
            seen: None,
            unsaved: None,
        }
    }

    pub fn beats(&self) -> f64 {
        self.beats
    }

    /// Applies the window's `steps` and the change in the actions' `count` since the last
    /// call (the first count seen only sets the base). Returns whether the zoom changed.
    pub fn take(&mut self, steps: i32, count: Option<i32>, now: Instant) -> bool {
        let mut steps = steps;
        if let Some(count) = count {
            if let Some(seen) = self.seen {
                steps = steps.saturating_add(count.wrapping_sub(seen));
            }
            self.seen = Some(count);
        }
        if steps == 0 {
            return false;
        }
        let beats = ui::waveform::zoomed(self.beats, steps, self.range);
        if beats == self.beats {
            return false;
        }
        self.beats = beats;
        self.unsaved = Some(now);
        true
    }

    /// Saves the zoom once it has stayed put for a while, on a thread of its own so the
    /// control loop never waits on the disk.
    pub fn save_if_due(&mut self, now: Instant) {
        if self
            .unsaved
            .is_some_and(|at| now.saturating_duration_since(at) >= SAVE_AFTER)
        {
            self.unsaved = None;
            if let Some(path) = self.path.clone() {
                let beats = self.beats;
                let _ = std::thread::Builder::new()
                    .name("ui state".to_owned())
                    .spawn(move || write(&path, beats));
            }
        }
    }

    /// Saves an unsaved zoom now: the session is ending.
    pub fn save_now(&mut self) {
        if self.unsaved.take().is_some()
            && let Some(path) = &self.path
        {
            write(path, self.beats);
        }
    }
}

impl Drop for Zoom {
    fn drop(&mut self) {
        self.save_now();
    }
}

/// The saved zoom, if the file holds a usable one.
fn read(path: &Path) -> Option<f64> {
    let text = std::fs::read_to_string(path).ok()?;
    let table: toml::Table = text.parse().ok()?;
    let beats = match table.get("waveform_beats")? {
        toml::Value::Float(f) => *f,
        toml::Value::Integer(i) => *i as f64,
        _ => return None,
    };
    (beats.is_finite() && beats > 0.0).then_some(beats)
}

/// Best effort: a zoom that cannot be saved is simply not remembered. The file is written
/// beside its place and renamed over it, so it is never left half written.
fn write(path: &Path, beats: f64) {
    let text = format!(
        "# The window's state, saved by Open Jockey. Delete it to start from config.toml.\n\
         waveform_beats = {beats}\n"
    );
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let partial = path.with_extension("toml.partial");
    if std::fs::write(&partial, text).is_ok() && std::fs::rename(&partial, path).is_err() {
        let _ = std::fs::remove_file(&partial);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RANGE: (f64, f64) = (4.0, 64.0);

    #[test]
    fn the_zoom_is_saved_and_loaded_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(STATE_FILE);
        let now = Instant::now();
        let mut zoom = Zoom::load(Some(path.clone()), 16.0, RANGE);
        assert_eq!(zoom.beats(), 16.0);
        // The first count seen is only the base.
        assert!(!zoom.take(0, Some(7), now));
        assert!(zoom.take(0, Some(8), now));
        assert!((zoom.beats() - 12.8).abs() < 1e-9);
        assert!(zoom.take(-2, Some(8), now));
        assert!((zoom.beats() - 20.0).abs() < 1e-9);
        // Not saved until it has stayed put.
        zoom.save_if_due(now);
        assert!(!path.exists());
        zoom.save_now();
        assert_eq!(Zoom::load(Some(path.clone()), 16.0, RANGE).beats(), 20.0);
    }

    #[test]
    fn a_bad_or_missing_file_falls_back_to_the_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(STATE_FILE);
        assert_eq!(Zoom::load(Some(path.clone()), 24.0, RANGE).beats(), 24.0);
        for text in [
            "waveform_beats = -2",
            "waveform_beats = \"wide\"",
            "not toml =",
        ] {
            std::fs::write(&path, text).unwrap();
            assert_eq!(
                Zoom::load(Some(path.clone()), 24.0, RANGE).beats(),
                24.0,
                "{text}"
            );
        }
        std::fs::write(&path, "waveform_beats = 8").unwrap();
        assert_eq!(Zoom::load(Some(path.clone()), 24.0, RANGE).beats(), 8.0);
        // A zoom saved outside a narrowed range comes back at its nearest end.
        assert_eq!(Zoom::load(Some(path), 24.0, (12.0, 32.0)).beats(), 12.0);
    }

    #[test]
    fn the_zoom_stays_in_range() {
        let now = Instant::now();
        let mut zoom = Zoom::load(None, 4.0, RANGE);
        assert!(!zoom.take(3, None, now));
        assert_eq!(zoom.beats(), 4.0);
        // Nowhere to save: nothing happens.
        zoom.save_now();
    }
}
