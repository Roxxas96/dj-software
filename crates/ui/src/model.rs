//! What the control thread hands the window: [`ControlFrame`], published through
//! [`Published`].

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use actions::ControlState;
use analysis::{KeyNotation, Waveform};
use engine::{EffectKind, EqMode, SAMPLER_SLOTS, StemKnobs};
use library::{BrowserView, TrackRow};

use crate::view::ViewState;

/// The soft-takeover targets of a deck that can show a pickup value.
pub const GHOSTS: usize = 7;
// Each target's place in `ControlFrame::ghosts`.
pub(crate) const GHOST_TRIM: usize = 0;
pub(crate) const GHOST_EQ_HIGH: usize = 1;
pub(crate) const GHOST_EQ_MID: usize = 2;
pub(crate) const GHOST_EQ_LOW: usize = 3;
pub(crate) const GHOST_FILTER: usize = 4;
pub(crate) const GHOST_FADER: usize = 5;
pub(crate) const GHOST_TEMPO: usize = 6;
const _: () = assert!(GHOSTS > GHOST_TEMPO);
/// How many events a frame keeps, newest last.
pub const EVENTS_KEPT: usize = 20;
/// How long an event shows in the top bar.
pub const EVENT_SHOWN: Duration = Duration::from_secs(8);

/// A value one thread publishes and others read. Publishing swaps a pointer and reading
/// clones one: the lock is held for nothing else, so neither side waits on the other's
/// work.
#[derive(Debug)]
pub struct Published<T> {
    latest: Arc<Mutex<Arc<T>>>,
}

impl<T> Clone for Published<T> {
    fn clone(&self) -> Self {
        Self {
            latest: Arc::clone(&self.latest),
        }
    }
}

impl<T> Published<T> {
    pub fn new(value: T) -> Self {
        Self {
            latest: Arc::new(Mutex::new(Arc::new(value))),
        }
    }

    /// Replaces the value every reader sees from now on.
    pub fn publish(&self, value: T) {
        let value = Arc::new(value);
        // The old value is dropped after the lock is released.
        let _old = std::mem::replace(&mut *self.lock(), value);
    }

    /// The value last published.
    pub fn latest(&self) -> Arc<T> {
        Arc::clone(&self.lock())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Arc<T>> {
        // A panic while holding the lock cannot leave the pointer half-written.
        self.latest.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The library track on a deck.
#[derive(Debug, Clone)]
pub struct DeckInfo {
    /// The deck's `load_seq` from which the engine plays this track: until its snapshot
    /// reaches it, the deck still holds the previous one.
    pub load_seq: u64,
    pub row: TrackRow,
    pub path: PathBuf,
    pub overview: Option<Arc<Waveform>>,
    pub detail: Option<Arc<Waveform>>,
    /// The stems' state (`stems`, `stems 40%`, ...), when there is one to show.
    pub stems: Option<String>,
}

impl PartialEq for DeckInfo {
    /// Waveforms compare by pointer: a frame is rebuilt often, and a waveform only ever
    /// changes by being replaced.
    fn eq(&self, other: &Self) -> bool {
        fn same(a: &Option<Arc<Waveform>>, b: &Option<Arc<Waveform>>) -> bool {
            match (a, b) {
                (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                (None, None) => true,
                _ => false,
            }
        }
        self.load_seq == other.load_seq
            && self.row == other.row
            && self.path == other.path
            && same(&self.overview, &other.overview)
            && same(&self.detail, &other.detail)
            && self.stems == other.stems
    }
}

/// Everything the window shows that is not read from the engine's snapshots.
#[derive(Debug, Clone, PartialEq)]
pub struct ControlFrame {
    pub control: ControlState,
    /// Per deck and soft-takeover target, the value the control must reach to pick it up
    /// (`None`: it has).
    pub ghosts: [[Option<f32>; GHOSTS]; 4],
    /// How long a rendered block takes to be heard.
    pub output_latency: Duration,
    pub crossfader_enabled: bool,
    pub decks: [Option<DeckInfo>; 4],
    pub browser: Option<BrowserView>,
    pub status: Vec<String>,
    /// The last [`EVENTS_KEPT`] events with when they happened, oldest first.
    pub events: VecDeque<(Instant, String)>,
    /// Per deck, the effect in each FX slot.
    pub fx_slots: [[EffectKind; 3]; 4],
    pub eq_mode: EqMode,
    pub stem_knobs: StemKnobs,
    pub samples_loaded: [bool; SAMPLER_SLOTS],
    /// The connected controller's port name.
    pub controller: Option<String>,
    /// The audio device's name, or why there is none.
    pub audio: Result<String, String>,
    /// How the window is arranged and drawn, the zoom included.
    pub view: ViewState,
    /// How keys are shown: the library's `key_notation` setting.
    pub key_notation: KeyNotation,
    /// Why the session stopped (a bad configuration or mapping, say), shown until the
    /// window is closed.
    pub error: Option<String>,
}

impl Default for ControlFrame {
    fn default() -> Self {
        Self {
            control: ControlState::default(),
            ghosts: [[None; GHOSTS]; 4],
            output_latency: Duration::ZERO,
            crossfader_enabled: true,
            decks: [None, None, None, None],
            browser: None,
            status: Vec::new(),
            events: VecDeque::new(),
            fx_slots: [[EffectKind::Echo, EffectKind::Reverb, EffectKind::Flanger]; 4],
            eq_mode: EqMode::default(),
            stem_knobs: StemKnobs::default(),
            samples_loaded: [false; SAMPLER_SLOTS],
            controller: None,
            audio: Err("no audio device yet".to_owned()),
            view: ViewState::default(),
            key_notation: KeyNotation::default(),
            error: None,
        }
    }
}

impl ControlFrame {
    /// The top bar's status: the newest event while it shows, else the first status line.
    pub fn status_line(&self, now: Instant) -> &str {
        self.events
            .back()
            .filter(|(at, _)| now.saturating_duration_since(*at) < EVENT_SHOWN)
            .map(|(_, text)| text.as_str())
            .or_else(|| self.status.first().map(String::as_str))
            .unwrap_or("")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_status_line_shows_an_event_for_eight_seconds() {
        let start = Instant::now();
        let mut frame = ControlFrame {
            status: vec!["▸ Music  1/3".into(), "Deck 1".into()],
            ..ControlFrame::default()
        };
        assert_eq!(frame.status_line(start), "▸ Music  1/3");
        frame.events.push_back((start, "Deck 1 ◂ A – B".into()));
        assert_eq!(frame.status_line(start), "Deck 1 ◂ A – B");
        let later = start + EVENT_SHOWN;
        assert_eq!(frame.status_line(later), "▸ Music  1/3");
        assert_eq!(ControlFrame::default().status_line(later), "");
    }
}
