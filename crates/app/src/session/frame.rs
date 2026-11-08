//! Builds the [`ControlFrame`] the window draws from the control loop's state.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use engine::{DeckId, SAMPLER_SLOTS};
use ui::{ControlFrame, DeckInfo, EVENTS_KEPT};

use crate::config::Config;
use crate::play::Pass;
use crate::printer::Line;

/// What the session knows besides the loop's state: the configuration's settings, what
/// the printer was asked to show, and the audio device.
pub(super) struct FrameParts {
    crossfader_enabled: bool,
    fx_slots: [engine::EffectKind; 3],
    eq_mode: engine::EqMode,
    stem_knobs: engine::StemKnobs,
    waveform_beats: f64,
    pub samples_loaded: [bool; SAMPLER_SLOTS],
    pub status: Vec<String>,
    pub events: VecDeque<(Instant, String)>,
    pub audio: Result<String, String>,
    pub output_latency: Duration,
}

impl FrameParts {
    pub fn new(config: &Config) -> Self {
        Self {
            crossfader_enabled: config.crossfader,
            fx_slots: config.fx_slots,
            eq_mode: config.stems.startup_eq_mode(),
            stem_knobs: config.stems.knobs,
            waveform_beats: config.ui.waveform_beats,
            samples_loaded: [false; SAMPLER_SLOTS],
            status: Vec::new(),
            events: VecDeque::new(),
            audio: Err("opening the audio device".to_owned()),
            output_latency: Duration::ZERO,
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

    /// The frame before the control loop runs: no controller, decks or browser yet.
    pub fn idle(&self) -> ControlFrame {
        ControlFrame {
            crossfader_enabled: self.crossfader_enabled,
            fx_slots: self.fx_slots,
            eq_mode: self.eq_mode,
            stem_knobs: self.stem_knobs,
            waveform_beats: self.waveform_beats,
            samples_loaded: self.samples_loaded,
            status: self.status.clone(),
            events: self.events.clone(),
            audio: self.audio.clone(),
            output_latency: self.output_latency,
            ..ControlFrame::default()
        }
    }

    /// The frame for the loop's state after a pass.
    pub fn build(&self, pass: &Pass<'_>) -> ControlFrame {
        let decks = DeckId::ALL.map(|deck| {
            let library = pass.library?;
            let view = library.deck_view(deck, &pass.engine.deck(deck))?;
            Some(DeckInfo {
                load_seq: view.load_seq,
                row: view.row.clone(),
                path: view.path.to_owned(),
                overview: None,
                detail: None,
                stems: view.stems,
            })
        });
        ControlFrame {
            control: pass.state.clone(),
            decks,
            browser: pass.library.and_then(|l| l.browser().cloned()),
            controller: pass.controller.map(str::to_owned),
            ..self.idle()
        }
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
        assert_eq!(frame.waveform_beats, 16.0);
    }
}
