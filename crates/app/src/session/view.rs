//! The window's view state (layout, styles, panel sizes, the waveform zoom), changed by
//! the window's commands and by the `ui.waveform_zoom_in` / `ui.waveform_zoom_out`
//! and `ui.layout` actions, and kept in `ui-state.toml` next to `config.toml`, so the next
//! launch starts where this one ended. `ui.waveform_beats` in `config.toml` is only the
//! zoom before that file holds one.
//!
//! The file is TOML: a `[view]` table with one key per [`ViewState`] field, enums by
//! their ids (`layout = "pro"`). A value that cannot be read keeps its default and the
//! others still load; plan 9's file, a lone top-level `waveform_beats`, loads too.
//!
//! Every save writes `version = 2` in `[view]`. A file from before (no version, or an
//! older one) wrote `jog_display = "plain"` whether or not it was chosen, so its jog
//! display is ignored once and the default (the info jog) applies.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use toml::{Table, Value};
use ui::{PanelSizes, ViewChange, ViewState};

/// The file the view is kept in, next to `config.toml`.
pub(super) const STATE_FILE: &str = "ui-state.toml";
/// The `[view]` version this build writes. From 2 on, `jog_display` is the user's choice.
const VERSION: i64 = 2;
/// How long the view must stay put before it is saved, so a scroll or a drag saves once.
const SAVE_AFTER: Duration = Duration::from_secs(2);

/// The view state and where it is saved.
#[derive(Debug)]
pub(super) struct ViewStore {
    path: Option<PathBuf>,
    state: ViewState,
    /// The fewest and most beats the zoom can show (`ui.waveform_min_beats` and
    /// `ui.waveform_max_beats`).
    zoom_range: (f64, f64),
    /// The actions' zoom count when last seen, `None` before the control loop runs.
    seen: Option<i32>,
    /// The actions' layout count when last seen, `None` before the control loop runs.
    seen_layout: Option<i32>,
    /// When the view last changed without being saved since.
    unsaved: Option<Instant>,
    /// The thread writing the last save, if it may still run.
    saving: Option<std::thread::JoinHandle<()>>,
}

impl ViewStore {
    /// The view saved at `path`, with `defaults` for what it does not hold (or cannot be
    /// read), brought within `zoom_range` and the panel limits (a zoom saved before the
    /// range was narrowed comes back at its nearest end).
    pub fn load(path: Option<PathBuf>, defaults: ViewState, zoom_range: (f64, f64)) -> Self {
        let state = match path.as_deref().and_then(read) {
            Some(table) => parse(&table, defaults),
            None => defaults,
        };
        Self {
            path,
            state: state.clamped(zoom_range),
            zoom_range,
            seen: None,
            seen_layout: None,
            unsaved: None,
            saving: None,
        }
    }

    pub fn state(&self) -> &ViewState {
        &self.state
    }

    /// Applies the window's `change`. Returns whether the view changed.
    pub fn apply(&mut self, change: ViewChange, now: Instant) -> bool {
        let mut next = self.state.clone();
        next.apply(change);
        self.replace(next.clamped(self.zoom_range), now)
    }

    /// Applies the window's zoom `steps` and the change in the actions' `actions_count`
    /// since the last call (the first count seen only sets the base). Returns whether the
    /// zoom changed.
    pub fn zoom(&mut self, steps: i32, actions_count: Option<i32>, now: Instant) -> bool {
        let mut steps = steps;
        if let Some(count) = actions_count {
            if let Some(seen) = self.seen {
                steps = steps.saturating_add(count.wrapping_sub(seen));
            }
            self.seen = Some(count);
        }
        if steps == 0 {
            return false;
        }
        let beats = ui::waveform::zoomed(self.state.waveform_beats, steps, self.zoom_range);
        let next = ViewState {
            waveform_beats: beats,
            ..self.state.clone()
        };
        self.replace(next, now)
    }

    /// Changes the zoom's range to `zoom_range` (`ui.waveform_min_beats` and
    /// `ui.waveform_max_beats` changed) and, when `ui.waveform_beats` changed, zooms to
    /// `beats`; the zoom is kept within the range. Returns whether the view changed.
    pub fn set_zoom_range(
        &mut self,
        zoom_range: (f64, f64),
        beats: Option<f64>,
        now: Instant,
    ) -> bool {
        self.zoom_range = zoom_range;
        let mut next = self.state.clone();
        if let Some(beats) = beats {
            next.waveform_beats = beats;
        }
        self.replace(next.clamped(zoom_range), now)
    }

    /// Moves to the next layout once for each press of `ui.layout` since the last call (the
    /// first count seen only sets the base). Returns whether the layout changed.
    pub fn layout(&mut self, actions_count: i32, now: Instant) -> bool {
        let seen = self.seen_layout.replace(actions_count);
        let presses = seen.map_or(0, |seen| actions_count.wrapping_sub(seen));
        let mut changed = false;
        // Four presses are a full turn: only the remainder shows.
        for _ in 0..presses.rem_euclid(ui::Layout::ALL.len() as i32) {
            let next = ui::Layout::next(self.state.layout);
            changed |= self.apply(ViewChange::SetLayout(next), now);
        }
        changed
    }

    fn replace(&mut self, next: ViewState, now: Instant) -> bool {
        if next == self.state {
            return false;
        }
        self.state = next;
        self.unsaved = Some(now);
        true
    }

    /// Saves the view once it has stayed put for a while, on a thread of its own so the
    /// control loop never waits on the disk.
    pub fn save_if_due(&mut self, now: Instant) {
        if self
            .unsaved
            .is_some_and(|at| now.saturating_duration_since(at) >= SAVE_AFTER)
        {
            let Some(path) = self.path.clone() else {
                self.unsaved = None;
                return;
            };
            let state = self.state.clone();
            let spawned = std::thread::Builder::new()
                .name("ui state".to_owned())
                .spawn(move || write(&path, &state));
            // Without a thread the view stays unsaved: the next call, or the end of the
            // session, tries again.
            if let Ok(handle) = spawned {
                self.unsaved = None;
                self.saving = Some(handle);
            }
        }
    }

    /// Saves an unsaved view now: the session is ending. A save still being written
    /// finishes first, so the two never share the file's `.partial`.
    pub fn save_now(&mut self) {
        if let Some(handle) = self.saving.take() {
            let _ = handle.join();
        }
        if self.unsaved.take().is_some()
            && let Some(path) = &self.path
        {
            write(path, &self.state);
        }
    }
}

impl Drop for ViewStore {
    fn drop(&mut self) {
        self.save_now();
    }
}

/// The saved file, if it is readable TOML.
fn read(path: &Path) -> Option<Table> {
    std::fs::read_to_string(path).ok()?.parse().ok()
}

/// `defaults` with every value of `file` that can be read. Unknown keys are ignored.
fn parse(file: &Table, defaults: ViewState) -> ViewState {
    let mut state = defaults;
    // Plan 9 kept the zoom at the top level.
    if let Some(beats) = file.get("waveform_beats").and_then(beats) {
        state.waveform_beats = beats;
    }
    let Some(view) = file.get("view").and_then(Value::as_table) else {
        return state;
    };
    let id = |key: &str| view.get(key).and_then(Value::as_str);
    let flag = |key: &str, value: &mut bool| {
        if let Some(on) = view.get(key).and_then(Value::as_bool) {
            *value = on;
        }
    };
    if let Some(layout) = id("layout").and_then(ui::Layout::from_id) {
        state.layout = layout;
    }
    if let Some(scheme) = id("colour_scheme").and_then(ui::ColourScheme::from_id) {
        state.colour_scheme = scheme;
    }
    if let Some(style) = id("waveform_style").and_then(ui::WaveformStyle::from_id) {
        state.waveform_style = style;
    }
    if let Some(palette) = id("waveform_palette").and_then(ui::WaveformPalette::from_id) {
        state.waveform_palette = palette;
    }
    if let Some(position) = id("waveform_position").and_then(ui::WaveformPosition::from_id) {
        state.waveform_position = position;
    }
    // Older files saved the plain jog without it being chosen: keep the default.
    let version = view.get("version").and_then(Value::as_integer).unwrap_or(1);
    if version >= VERSION
        && let Some(display) = id("jog_display").and_then(ui::JogDisplay::from_id)
    {
        state.jog_display = display;
    }
    flag("beat_counter", &mut state.beat_counter);
    flag("four_decks", &mut state.four_decks);
    flag("overview", &mut state.overview);
    flag("info_panel", &mut state.info_panel);
    flag("sideview", &mut state.sideview);
    if let Some(modes) = view.get("time_mode").and_then(Value::as_array) {
        for (mode, saved) in state.time_mode.iter_mut().zip(modes) {
            if let Some(saved) = saved.as_str().and_then(ui::TimeMode::from_id) {
                *mode = saved;
            }
        }
    }
    if let Some(sizes) = view.get("sizes").and_then(Value::as_table) {
        let s = &mut state.sizes;
        for (key, share) in [
            ("lanes", &mut s.lanes),
            ("decks", &mut s.decks),
            ("tree", &mut s.tree),
            ("info", &mut s.info),
            ("sideview", &mut s.sideview),
        ] {
            if let Some(saved) = sizes.get(key).and_then(number) {
                *share = saved as f32;
            }
        }
    }
    if let Some(beats) = view.get("waveform_beats").and_then(beats) {
        state.waveform_beats = beats;
    }
    state
}

fn number(value: &Value) -> Option<f64> {
    let number = match value {
        Value::Float(f) => *f,
        Value::Integer(i) => *i as f64,
        _ => return None,
    };
    number.is_finite().then_some(number)
}

/// A usable zoom: a positive number of beats.
fn beats(value: &Value) -> Option<f64> {
    number(value).filter(|beats| *beats > 0.0)
}

/// The file's text for `state`.
fn text(state: &ViewState) -> String {
    // Through its decimal text, so 0.22 is saved as 0.22 rather than 0.2199999988.
    let share = |share: f32| Value::Float(share.to_string().parse().unwrap_or(f64::from(share)));
    let PanelSizes {
        lanes,
        decks,
        tree,
        info,
        sideview,
    } = state.sizes;
    let mut sizes = Table::new();
    for (key, value) in [
        ("lanes", lanes),
        ("decks", decks),
        ("tree", tree),
        ("info", info),
        ("sideview", sideview),
    ] {
        sizes.insert(key.to_owned(), share(value));
    }
    let id = |id: &str| Value::String(id.to_owned());
    let mut view = Table::new();
    view.insert("version".to_owned(), Value::Integer(VERSION));
    view.insert("layout".to_owned(), id(state.layout.id()));
    view.insert("colour_scheme".to_owned(), id(state.colour_scheme.id()));
    view.insert("waveform_style".to_owned(), id(state.waveform_style.id()));
    view.insert(
        "waveform_palette".to_owned(),
        id(state.waveform_palette.id()),
    );
    view.insert(
        "waveform_position".to_owned(),
        id(state.waveform_position.id()),
    );
    view.insert("jog_display".to_owned(), id(state.jog_display.id()));
    for (key, on) in [
        ("beat_counter", state.beat_counter),
        ("four_decks", state.four_decks),
        ("overview", state.overview),
        ("info_panel", state.info_panel),
        ("sideview", state.sideview),
    ] {
        view.insert(key.to_owned(), Value::Boolean(on));
    }
    let modes = state.time_mode.iter().map(|mode| id(mode.id())).collect();
    view.insert("time_mode".to_owned(), Value::Array(modes));
    view.insert(
        "waveform_beats".to_owned(),
        Value::Float(state.waveform_beats),
    );
    view.insert("sizes".to_owned(), Value::Table(sizes));
    let mut file = Table::new();
    file.insert("view".to_owned(), Value::Table(view));
    format!(
        "# The window's state, saved by Open Jockey. Delete it to start from config.toml.\n\n{file}"
    )
}

/// Best effort: a view that cannot be saved is simply not remembered. The file is written
/// beside its place and renamed over it, so it is never left half written.
fn write(path: &Path, state: &ViewState) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let partial = path.with_extension("toml.partial");
    if std::fs::write(&partial, text(state)).is_ok() && std::fs::rename(&partial, path).is_err() {
        let _ = std::fs::remove_file(&partial);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::DeckId;
    use ui::{ColourScheme, Layout, PanelSizes, TimeMode, WaveformStyle};

    const RANGE: (f64, f64) = (4.0, 64.0);

    fn defaults() -> ViewState {
        ViewState::default()
    }

    #[test]
    fn view_state_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(STATE_FILE);
        let now = Instant::now();
        let mut store = ViewStore::load(Some(path.clone()), defaults(), RANGE);
        let deck2 = DeckId::new(1).unwrap();
        for change in [
            ViewChange::SetLayout(Layout::Performance),
            ViewChange::SetColourScheme(ColourScheme::Light),
            ViewChange::SetWaveformStyle(WaveformStyle::DeckColour),
            ViewChange::SetWaveformPalette(ui::WaveformPalette::OrangeYellowCyan),
            ViewChange::SetWaveformPosition(ui::WaveformPosition::Middle),
            ViewChange::SetJogDisplay(ui::JogDisplay::Plain),
            ViewChange::SetBeatCounter(true),
            ViewChange::SetFourDecks(true),
            ViewChange::SetOverview(false),
            ViewChange::SetInfoPanel(false),
            ViewChange::SetSideview(true),
            ViewChange::CycleTime(deck2),
            ViewChange::SetSizes(PanelSizes {
                lanes: 0.3,
                decks: 0.33,
                tree: 0.15,
                info: 0.25,
                sideview: 0.1,
            }),
        ] {
            assert!(store.apply(change, now), "{change:?}");
        }
        assert!(store.zoom(1, None, now));
        let saved = store.state().clone();
        assert_ne!(saved, defaults());
        assert_eq!(saved.time_mode[1], TimeMode::Total);
        // Not saved until it has stayed put.
        store.save_if_due(now);
        assert!(!path.exists());
        drop(store);
        let loaded = ViewStore::load(Some(path), defaults(), RANGE);
        assert_eq!(*loaded.state(), saved);
    }

    #[test]
    fn waveform_palette_is_saved_and_restored() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(STATE_FILE);
        let now = Instant::now();
        let mut store = ViewStore::load(Some(path.clone()), defaults(), RANGE);
        store.apply(
            ViewChange::SetWaveformPalette(ui::WaveformPalette::OrangeYellowCyan),
            now,
        );
        store.save_now();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("waveform_palette = \"orange_yellow_cyan\""),
            "{text}"
        );
        let loaded = ViewStore::load(Some(path), defaults(), RANGE);
        assert_eq!(
            loaded.state().waveform_palette,
            ui::WaveformPalette::OrangeYellowCyan
        );
    }

    #[test]
    fn the_last_save_is_the_one_kept() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(STATE_FILE);
        let now = Instant::now();
        let mut store = ViewStore::load(Some(path.clone()), defaults(), RANGE);
        store.apply(ViewChange::SetLayout(Layout::Starter), now);
        store.save_if_due(now + SAVE_AFTER);
        assert!(store.unsaved.is_none() && store.saving.is_some());
        // Changed again while that save may still be writing: the session ends.
        store.apply(ViewChange::SetLayout(Layout::Performance), now + SAVE_AFTER);
        store.save_now();
        assert!(store.saving.is_none());
        let loaded = ViewStore::load(Some(path.clone()), defaults(), RANGE);
        assert_eq!(loaded.state().layout, Layout::Performance);
        assert!(!dir.path().join("ui-state.toml.partial").exists());
    }

    #[test]
    fn an_unchanged_view_is_not_saved_again() {
        let now = Instant::now();
        let mut store = ViewStore::load(None, defaults(), RANGE);
        assert!(!store.apply(ViewChange::SetLayout(Layout::Pro), now));
        assert!(store.unsaved.is_none());
        // A share past the end is clamped: dragging further changes nothing.
        let wide = PanelSizes {
            lanes: 0.9,
            ..PanelSizes::default()
        };
        assert!(store.apply(ViewChange::SetSizes(wide), now));
        assert_eq!(store.state().sizes.lanes, ui::MAX_SHARE);
        assert!(!store.apply(ViewChange::SetSizes(wide), now));
    }

    #[test]
    fn a_bad_view_file_keeps_the_good_values() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(STATE_FILE);
        std::fs::write(
            &path,
            "[view]\nlayout = \"weird\"\nsizes = { lanes = 5.0, decks = \"big\" }\n\
             four_decks = true\noverview = 3\ntime_mode = [\"total\", \"later\"]\n\
             colour = \"pink\"\n[other]\nkey = 1\n",
        )
        .unwrap();
        let state = ViewStore::load(Some(path), defaults(), RANGE)
            .state()
            .clone();
        assert_eq!(state.layout, Layout::Pro);
        assert_eq!(state.sizes.lanes, ui::MAX_SHARE);
        assert_eq!(state.sizes.decks, PanelSizes::default().decks);
        assert!(state.four_decks);
        assert!(state.overview);
        assert_eq!(
            state.time_mode,
            [
                TimeMode::Total,
                TimeMode::Remaining,
                TimeMode::Remaining,
                TimeMode::Remaining
            ]
        );
    }

    #[test]
    fn pre_12a_file_gets_the_info_jog() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(STATE_FILE);
        for text in [
            "[view]\njog_display = \"plain\"\nfour_decks = true\n",
            "[view]\nversion = 1\njog_display = \"plain\"\nfour_decks = true\n",
        ] {
            std::fs::write(&path, text).unwrap();
            let state = ViewStore::load(Some(path.clone()), defaults(), RANGE)
                .state()
                .clone();
            assert_eq!(state.jog_display, ui::JogDisplay::Info, "{text}");
            // The rest of the file still loads.
            assert!(state.four_decks, "{text}");
        }
    }

    #[test]
    fn version_2_keeps_a_chosen_plain_jog() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(STATE_FILE);
        std::fs::write(&path, "[view]\nversion = 2\njog_display = \"plain\"\n").unwrap();
        let state = ViewStore::load(Some(path), defaults(), RANGE)
            .state()
            .clone();
        assert_eq!(state.jog_display, ui::JogDisplay::Plain);
    }

    #[test]
    fn saving_writes_version_2() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(STATE_FILE);
        let now = Instant::now();
        let mut store = ViewStore::load(Some(path.clone()), defaults(), RANGE);
        store.apply(ViewChange::SetJogDisplay(ui::JogDisplay::Plain), now);
        store.save_now();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("version = 2"), "{text}");
        let loaded = ViewStore::load(Some(path), defaults(), RANGE);
        assert_eq!(loaded.state().jog_display, ui::JogDisplay::Plain);
    }

    #[test]
    fn a_plan_9_file_still_loads() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(STATE_FILE);
        std::fs::write(&path, "waveform_beats = 8\n").unwrap();
        let state = ViewStore::load(Some(path), defaults(), RANGE)
            .state()
            .clone();
        assert_eq!(
            state,
            ViewState {
                waveform_beats: 8.0,
                ..defaults()
            }
        );
    }

    #[test]
    fn a_bad_or_missing_file_falls_back_to_the_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(STATE_FILE);
        let config = ViewState {
            waveform_beats: 24.0,
            ..defaults()
        };
        let beats = |path: &Path, range| {
            ViewStore::load(Some(path.to_owned()), config.clone(), range)
                .state()
                .waveform_beats
        };
        assert_eq!(beats(&path, RANGE), 24.0);
        for text in [
            "waveform_beats = -2",
            "waveform_beats = \"wide\"",
            "[view]\nwaveform_beats = inf",
            "not toml =",
        ] {
            std::fs::write(&path, text).unwrap();
            assert_eq!(beats(&path, RANGE), 24.0, "{text}");
        }
        // The `[view]` value wins over plan 9's.
        std::fs::write(&path, "waveform_beats = 8\n[view]\nwaveform_beats = 32").unwrap();
        assert_eq!(beats(&path, RANGE), 32.0);
        // A zoom saved outside a narrowed range comes back at its nearest end.
        std::fs::write(&path, "waveform_beats = 8").unwrap();
        assert_eq!(beats(&path, (12.0, 32.0)), 12.0);
    }

    #[test]
    fn ui_layout_presses_cycle_the_layout() {
        let now = Instant::now();
        let mut store = ViewStore::load(None, defaults(), RANGE);
        // The first count seen is only the base.
        assert!(!store.layout(5, now));
        assert!(!store.layout(5, now));
        assert!(store.layout(6, now));
        assert_eq!(store.state().layout, Layout::Performance);
        assert!(store.layout(7, now));
        assert_eq!(store.state().layout, Layout::Starter);
        // Two presses between looks move two layouts.
        assert!(store.layout(9, now));
        assert_eq!(store.state().layout, Layout::Pro);
    }

    #[test]
    fn the_zoom_follows_the_window_and_the_actions() {
        let now = Instant::now();
        let mut store = ViewStore::load(None, defaults(), RANGE);
        // The first count seen is only the base.
        assert!(!store.zoom(0, Some(7), now));
        assert!(store.zoom(0, Some(8), now));
        assert!((store.state().waveform_beats - 12.8).abs() < 1e-9);
        assert!(store.zoom(-2, Some(8), now));
        assert!((store.state().waveform_beats - 20.0).abs() < 1e-9);
        // In range only.
        assert!(store.zoom(-100, None, now));
        assert_eq!(store.state().waveform_beats, 64.0);
        assert!(!store.zoom(-1, None, now));
        // Nowhere to save: nothing happens.
        store.save_now();
    }
}
