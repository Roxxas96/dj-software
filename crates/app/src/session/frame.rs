//! Builds the [`ControlFrame`] the window draws from the control loop's state.

use std::collections::VecDeque;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use engine::{DeckId, SAMPLER_SLOTS};
use ui::{ControlFrame, DeckInfo, EVENTS_KEPT, SettingRow, ViewState};

use crate::config::Config;
use crate::library_mode::LibraryMode;
use crate::play::Pass;
use crate::printer::Line;

use super::detail::DetailJobs;
use super::export::Export;
use super::import::Import;
use super::settings::Settings;
use super::view::ViewStore;

/// What the session knows besides the loop's state: the configuration's settings, what
/// the printer was asked to show, and the audio device.
pub(super) struct FrameParts {
    /// The engine's and the controller's settings as they were sent: the configuration's
    /// at launch, then the window's live changes (the next-launch keys keep their launch
    /// values).
    pub applied: Config,
    /// A configuration saved from the window whose live changes are still to be sent:
    /// they wait for the control loop to run.
    pub unapplied: Option<Config>,
    /// The configuration file's settings; `None` when there is no file to keep them in.
    pub settings: Option<Settings>,
    settings_rows: Arc<Vec<SettingRow>>,
    /// The audio output devices' names, filled by the thread [`FrameParts::list_devices`]
    /// starts.
    devices: Arc<OnceLock<Vec<String>>>,
    pub crossfader_enabled: bool,
    /// Per deck, the effects the engine was sent: the configuration's, then the window's
    /// choices.
    pub fx_slots: [[engine::EffectKind; 3]; 4],
    pub eq_mode: engine::EqMode,
    pub stem_knobs: engine::StemKnobs,
    /// The window's view state, the zoom included; saved once it stays put and when the
    /// session ends.
    pub view: ViewStore,
    pub samples_loaded: [bool; SAMPLER_SLOTS],
    pub status: Vec<String>,
    pub events: VecDeque<(Instant, String)>,
    pub audio: Result<String, String>,
    pub output_latency: Duration,
    /// Computes the decks' waveforms; none until the library is open.
    pub detail: Option<DetailJobs>,
    /// The import from VirtualDJ; none until the library is open.
    pub import: Option<Import>,
    /// The export to a USB stick; none until the library is open.
    pub export: Option<Export>,
}

impl FrameParts {
    pub fn new(config: &Config) -> Self {
        Self {
            applied: config.clone(),
            unapplied: None,
            settings: None,
            settings_rows: Arc::default(),
            devices: Arc::default(),
            crossfader_enabled: config.crossfader,
            fx_slots: DeckId::ALL.map(|deck| config.fx_slots_for(deck)),
            eq_mode: config.stems.startup_eq_mode(),
            stem_knobs: config.stems.knobs,
            view: ViewStore::load(None, default_view(config), config.ui.zoom_range()),
            samples_loaded: [false; SAMPLER_SLOTS],
            status: Vec::new(),
            events: VecDeque::new(),
            audio: Err("opening the audio device".to_owned()),
            output_latency: Duration::ZERO,
            detail: None,
            import: None,
            export: None,
        }
    }

    /// Lists the audio output devices on a thread of its own, once: the system's answer
    /// can take a while, and the control loop never waits for it. The window shows the
    /// list from the first frame after it arrives.
    pub fn list_devices(&self) {
        let devices = Arc::clone(&self.devices);
        let spawned = std::thread::Builder::new()
            .name("devices".to_owned())
            .spawn(move || {
                let names = audio_io::list_output_devices()
                    .map(|found| found.into_iter().map(|d| d.name).collect())
                    .unwrap_or_default();
                let _ = devices.set(names);
            });
        // Without the thread the window just has no list.
        drop(spawned);
    }

    /// Reads the configuration file's settings from `settings`, for the window to list.
    pub fn set_settings(&mut self, settings: Settings) {
        self.settings = Some(settings);
        self.refresh_settings();
    }

    /// Shows the settings' rows as they are now; the frame's rows are replaced only when
    /// they changed.
    pub fn refresh_settings(&mut self) {
        if let Some(settings) = &self.settings {
            let rows = settings.rows();
            if *self.settings_rows != rows {
                self.settings_rows = Arc::new(rows);
            }
        }
    }

    /// Takes what the printer was asked to show.
    pub fn take(&mut self, line: Line) {
        match line {
            Line::Status(lines) => self.status = lines,
            Line::Event(text) => self.event(text),
        }
    }

    pub fn event(&mut self, text: String) {
        if self.events.len() == EVENTS_KEPT {
            self.events.pop_front();
        }
        self.events.push_back((Instant::now(), text));
    }

    /// Applies the zoom and layout actions' counts once the loop runs (`None` before it
    /// does), and saves the view once it has stayed put.
    pub fn settle_view(&mut self, counts: Option<(i32, i32)>) {
        let now = Instant::now();
        self.view.zoom(0, counts.map(|(zoom, _)| zoom), now);
        if let Some((_, layout)) = counts {
            self.view.layout(layout, now);
        }
        self.view.save_if_due(now);
    }

    /// Reads the hotcue marks of the decks' tracks from the library again.
    pub fn refresh_marks(&mut self) {
        if let Some(detail) = self.detail.as_mut() {
            detail.refresh_marks();
        }
    }

    /// Shows `mark` as the colour and name of `track`'s hotcue `slot` on the decks holding
    /// it, from the next frame on.
    pub fn show_mark(&mut self, track: i64, slot: engine::HotcueSlot, mark: ui::HotcueMark) {
        if let Some(detail) = self.detail.as_mut() {
            detail.set_mark(track, slot, mark);
        }
    }

    /// Stops showing the colour and name of `track`'s hotcue `slot`, whose label failed to
    /// save, and reads the decks' marks again for what the library holds.
    pub fn forget_failed_mark(&mut self, track: i64, slot: engine::HotcueSlot) {
        if let Some(detail) = self.detail.as_mut() {
            detail.forget_mark(track, slot);
            detail.refresh_marks();
        }
    }

    /// Stops showing the colour and name of the hotcues `library` deleted in its latest
    /// round. Called after every pass, published or not, so none is missed.
    pub fn forget_deleted_marks(&mut self, library: Option<&LibraryMode>) {
        if let (Some(detail), Some(library)) = (self.detail.as_mut(), library) {
            for &(track, slot) in library.deleted_hotcues() {
                detail.forget_mark(track, slot);
            }
        }
    }

    /// The frame before the control loop runs: no controller, decks or browser yet.
    pub fn idle(&self) -> ControlFrame {
        ControlFrame {
            crossfader_enabled: self.crossfader_enabled,
            fx_slots: self.fx_slots,
            eq_mode: self.eq_mode,
            stem_knobs: self.stem_knobs,
            view: self.view.state().clone(),
            samples_loaded: self.samples_loaded,
            status: self.status.clone(),
            events: self.events.clone(),
            audio: self.audio.clone(),
            output_latency: self.output_latency,
            settings: Arc::clone(&self.settings_rows),
            devices: self.devices.get().cloned().unwrap_or_default(),
            mappings: controller::builtin::builtin_names()
                .map(str::to_owned)
                .collect(),
            import: self
                .import
                .as_ref()
                .map(|import| import.view().clone())
                .unwrap_or_default(),
            export: self
                .export
                .as_ref()
                .map(|export| export.view().clone())
                .unwrap_or_default(),
            ..ControlFrame::default()
        }
    }

    /// The frame for the loop's state after a pass.
    pub fn build(&mut self, pass: &Pass<'_>) -> ControlFrame {
        if let Some(detail) = self.detail.as_mut() {
            detail.collect();
        }
        let decks = DeckId::ALL.map(|deck| {
            let i = deck.index();
            let snapshot = pass.engine.deck(deck);
            let view = pass
                .library
                .and_then(|library| library.deck_view(deck, &snapshot));
            let Some(view) = view else {
                if let Some(detail) = self.detail.as_mut() {
                    detail.empty(i);
                }
                return None;
            };
            let (overview, detail) = match self.detail.as_mut() {
                Some(jobs) => {
                    // Once the engine plays the load: until then its waveform is not
                    // the one on the deck.
                    if snapshot.loaded && snapshot.load_seq == view.load_seq {
                        jobs.observe(i, view.load_seq, view.row.id, view.audio);
                    }
                    jobs.waveforms(i, view.load_seq)
                }
                None => (None, None),
            };
            Some(DeckInfo {
                load_seq: view.load_seq,
                row: view.row.clone(),
                path: view.path.to_owned(),
                overview,
                detail,
                stems: view.stems,
                hotcue_marks: self
                    .detail
                    .as_ref()
                    .map(|jobs| jobs.marks(i, view.load_seq))
                    .unwrap_or_default(),
            })
        });
        ControlFrame {
            control: pass.state.clone(),
            ghosts: pass.ghosts,
            global_ghosts: pass.global_ghosts,
            decks,
            browser: pass.library.and_then(|l| l.browser().cloned()),
            controller: pass.controller.map(str::to_owned),
            key_notation: pass.library.map(LibraryMode::notation).unwrap_or_default(),
            ..self.idle()
        }
    }
}

/// The view before `ui-state.toml` holds one: the defaults, zoomed to `ui.waveform_beats`.
pub(super) fn default_view(config: &Config) -> ViewState {
    ViewState {
        waveform_beats: config.ui.waveform_beats,
        ..ViewState::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_keep_the_last_twenty() {
        let mut parts = FrameParts::new(&Config::default());
        for n in 0..25 {
            parts.take(Line::Event(format!("event {n}")));
        }
        parts.take(Line::Status(vec!["▸ Music".into()]));
        let frame = parts.idle();
        assert_eq!(frame.events.len(), EVENTS_KEPT);
        assert_eq!(frame.events.front().unwrap().1, "event 5");
        assert_eq!(frame.events.back().unwrap().1, "event 24");
        assert_eq!(frame.status, ["▸ Music"]);
        assert_eq!(frame.view.waveform_beats, 16.0);
    }
}
