//! Builds the [`ControlFrame`] the window draws from the control loop's state.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use engine::{DeckId, SAMPLER_SLOTS};
use ui::{ControlFrame, DeckInfo, EVENTS_KEPT, ViewState};

use crate::config::Config;
use crate::library_mode::LibraryMode;
use crate::play::Pass;
use crate::printer::Line;

use super::detail::DetailJobs;
use super::view::ViewStore;

/// What the session knows besides the loop's state: the configuration's settings, what
/// the printer was asked to show, and the audio device.
pub(super) struct FrameParts {
    crossfader_enabled: bool,
    fx_slots: [engine::EffectKind; 3],
    eq_mode: engine::EqMode,
    stem_knobs: engine::StemKnobs,
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
}

impl FrameParts {
    pub fn new(config: &Config) -> Self {
        Self {
            crossfader_enabled: config.crossfader,
            fx_slots: config.fx_slots,
            eq_mode: config.stems.startup_eq_mode(),
            stem_knobs: config.stems.knobs,
            view: ViewStore::load(None, default_view(config), config.ui.zoom_range()),
            samples_loaded: [false; SAMPLER_SLOTS],
            status: Vec::new(),
            events: VecDeque::new(),
            audio: Err("opening the audio device".to_owned()),
            output_latency: Duration::ZERO,
            detail: None,
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
            })
        });
        ControlFrame {
            control: pass.state.clone(),
            ghosts: pass.ghosts,
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
