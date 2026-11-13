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
pub const GHOSTS: usize = 8;
// Each target's place in `ControlFrame::ghosts`.
pub(crate) const GHOST_TRIM: usize = 0;
pub(crate) const GHOST_EQ_HIGH: usize = 1;
pub(crate) const GHOST_EQ_MID: usize = 2;
pub(crate) const GHOST_EQ_LOW: usize = 3;
pub(crate) const GHOST_FILTER: usize = 4;
pub(crate) const GHOST_FADER: usize = 5;
pub(crate) const GHOST_TEMPO: usize = 6;
pub(crate) const GHOST_FX_LEVEL: usize = 7;
const _: () = assert!(GHOSTS > GHOST_FX_LEVEL);
/// The soft-takeover targets of no deck that can show a pickup value.
pub const GLOBAL_GHOSTS: usize = 4;
// Each target's place in `ControlFrame::global_ghosts`.
pub(crate) const GHOST_CROSSFADER: usize = 0;
pub(crate) const GHOST_MASTER: usize = 1;
pub(crate) const GHOST_HEADPHONE: usize = 2;
pub(crate) const GHOST_HEADPHONE_MIX: usize = 3;
const _: () = assert!(GLOBAL_GHOSTS > GHOST_HEADPHONE_MIX);
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

/// A hotcue's colour and name in the library. Where it is and whether it is a loop come
/// from the engine's snapshot.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HotcueMark {
    pub color: Option<egui::Color32>,
    pub name: Option<String>,
}

impl HotcueMark {
    /// A mark with `color` as `0xRRGGBB`.
    pub fn new(color: Option<u32>, name: Option<String>) -> Self {
        Self {
            color: color.map(|rgb| {
                let [_, r, g, b] = rgb.to_be_bytes();
                egui::Color32::from_rgb(r, g, b)
            }),
            name,
        }
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
    /// Per hotcue slot, its colour and name; shared, like the waveforms, so a frame is
    /// built without copying the names.
    pub hotcue_marks: Arc<[Option<HotcueMark>; 8]>,
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
            && (Arc::ptr_eq(&self.hotcue_marks, &other.hotcue_marks)
                || self.hotcue_marks == other.hotcue_marks)
    }
}

/// One `config.toml` setting, as the settings window lists it.
#[derive(Debug, Clone, PartialEq)]
pub struct SettingRow {
    /// The key, `stems.knobs` say.
    pub key: String,
    /// The group it is listed under: "Audio", "Controller", "Mixer", "Effects", "Sampler",
    /// "Pads", "Stems" or "Interface".
    pub category: &'static str,
    /// The current value as TOML text (`"vdj"`, `300`, `[3, 4]`); the default when the file
    /// leaves it out.
    pub value: String,
    /// The default as TOML text; empty when the default is to leave it unset.
    pub default: String,
    /// What it does, in one line.
    pub help: &'static str,
    /// Whether a change applies at once; otherwise it applies at the next launch.
    pub live: bool,
    /// A saved change that applies at the next launch.
    pub pending: bool,
    /// Why the last change to it was refused.
    pub error: Option<String>,
}

/// Where the import from VirtualDJ is, as the settings window shows it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum ImportView {
    #[default]
    Idle,
    Scanning,
    /// What the import would do, before anything is written.
    Summary(ImportSummary),
    /// What to do with `track`, which has `existing` hotcues here and `incoming` in
    /// VirtualDJ; `left` tracks are still to choose for, this one included.
    Ask {
        track: TrackLabel,
        existing: usize,
        incoming: usize,
        left: usize,
    },
    Applying,
    /// What was written, and where the backup taken before is.
    Done(String),
    /// Why the import stopped; nothing was written.
    Failed(String),
    /// Refused: a deck holds a track.
    DecksBusy,
}

/// The counts the import shows before it writes anything.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportSummary {
    /// VirtualDJ databases read.
    pub databases: usize,
    /// VirtualDJ songs found in the library.
    pub tracks: usize,
    /// Of those, the ones with hotcues or saved loops.
    pub with_marks: usize,
    /// Of those, the ones that have hotcues here too.
    pub conflicting: usize,
    pub hotcues: usize,
    pub loops: usize,
    /// Hotcues and loops that found no free slot.
    pub dropped: usize,
    /// Markers that are not hotcues or loops.
    pub other_markers: usize,
    pub playlists_new: usize,
    /// Playlists already here with the same tracks.
    pub playlists_here: usize,
    /// Playlists that could not be read.
    pub playlists_skipped: usize,
    /// Songs and playlist entries not in the library.
    pub unmatched: usize,
}

/// Where the export to a USB stick is, as the settings window shows it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum ExportView {
    #[default]
    Idle,
    /// Looking for sticks and reading the playlists.
    Listing,
    /// The sticks and playlists to choose from.
    Choose(ExportChoice),
    /// Working out what the export would do.
    Checking,
    /// What the export would do, before anything is written.
    Summary(ExportSummary),
    /// `done` of `total` steps written.
    Writing { done: usize, total: usize },
    /// What was written.
    Done(String),
    /// Why the export stopped.
    Failed(String),
}

/// The sticks an export can go to, and the library's playlists.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExportChoice {
    pub sticks: Vec<StickLabel>,
    pub playlists: Vec<PlaylistLabel>,
}

/// A USB stick, as the export's choice names it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StickLabel {
    pub name: String,
    /// Free space, like "512.0 MB", or "?" when the system does not say.
    pub free: String,
    /// The library playlists already on the stick, by id: those whose names are in the
    /// manifest an earlier export left there.
    pub exported: Vec<i64>,
}

/// A library playlist, as the export's choice names it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlaylistLabel {
    pub id: i64,
    pub name: String,
    pub tracks: u32,
}

/// Why an export is refused when the system will not say how much is free on the stick.
/// The same words as `interop::export::FREE_SPACE_UNKNOWN`, which the export run refuses with.
const FREE_SPACE_UNKNOWN: &str = "Open Jockey cannot read the stick's free space.";

/// The counts the export shows before it writes anything.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExportSummary {
    /// The stick's name, or its path from the command line.
    pub stick: String,
    pub playlists: usize,
    /// Tracks in the playlists, each counted once.
    pub tracks: usize,
    /// Tracks to copy, and their size like "12.3 MB".
    pub to_copy: usize,
    pub copy_size: String,
    /// Tracks already on the stick.
    pub kept: usize,
    /// Free space on the stick, like "512.0 MB".
    pub free: String,
    /// Tracks copied without Serato cues: not MP3 or WAV.
    pub without_serato_cues: usize,
    /// Tracks missing or unreadable, left out.
    pub skipped: usize,
    /// Copies from an earlier export no longer exported, to delete.
    pub to_remove: usize,
    /// The stick cannot hold the copies: nothing will be written.
    pub too_small: bool,
    /// The system will not say how much is free on the stick: nothing will be written.
    pub free_unknown: bool,
}

impl ExportSummary {
    /// The summary, a count per line, as both the window and `library export` show it.
    #[must_use]
    pub fn lines(&self) -> [String; 7] {
        [
            format!("Stick: {}", self.stick),
            format!("Playlists: {}, tracks: {}", self.playlists, self.tracks),
            format!(
                "To copy: {} {} ({}), already on the stick: {}",
                self.to_copy,
                if self.to_copy == 1 { "track" } else { "tracks" },
                self.copy_size,
                self.kept
            ),
            format!("Free on the stick: {}", self.free),
            format!(
                "Copied without Serato cues (not MP3 or WAV): {}",
                self.without_serato_cues
            ),
            format!("Skipped (missing or unreadable): {}", self.skipped),
            format!(
                "Removed from the stick (no longer exported): {}",
                self.to_remove
            ),
        ]
    }

    /// Why a stick that is [`too_small`](Self::too_small) is refused.
    #[must_use]
    pub fn too_small_line(&self) -> String {
        format!(
            "The stick is too small: {} to copy, {} free.",
            self.copy_size, self.free
        )
    }

    /// Why this export is refused, if it is: the stick's free space is unknown, or it is
    /// too small. Nothing is written then.
    #[must_use]
    pub fn refusal(&self) -> Option<String> {
        if self.free_unknown {
            Some(FREE_SPACE_UNKNOWN.to_owned())
        } else if self.too_small {
            Some(self.too_small_line())
        } else {
            None
        }
    }
}

/// A library track, as a question names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackLabel {
    pub id: i64,
    pub name: String,
}

/// Everything the window shows that is not read from the engine's snapshots.
#[derive(Debug, Clone, PartialEq)]
pub struct ControlFrame {
    pub control: ControlState,
    /// Per deck and soft-takeover target, the value the control must reach to pick it up
    /// (`None`: it has).
    pub ghosts: [[Option<f32>; GHOSTS]; 4],
    /// The same for the crossfader, the master level and the headphones' level and mix.
    pub global_ghosts: [Option<f32>; GLOBAL_GHOSTS],
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
    /// Every `config.toml` setting, in `docs/config.md`'s order; empty until the session
    /// has read the file.
    pub settings: Arc<Vec<SettingRow>>,
    /// The names of the audio output devices, listed once when the session starts; empty
    /// until then.
    pub devices: Vec<String>,
    /// The names of the built-in controller mappings.
    pub mappings: Vec<String>,
    /// Where the import from VirtualDJ is.
    pub import: ImportView,
    /// Where the export to a USB stick is.
    pub export: ExportView,
}

impl Default for ControlFrame {
    fn default() -> Self {
        Self {
            control: ControlState::default(),
            ghosts: [[None; GHOSTS]; 4],
            global_ghosts: [None; GLOBAL_GHOSTS],
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
            settings: Arc::default(),
            devices: Vec::new(),
            mappings: Vec::new(),
            import: ImportView::default(),
            export: ExportView::default(),
        }
    }
}

impl ControlFrame {
    /// The top bar's status: the newest event while it shows, else the first status line.
    pub fn status_line(&self, now: Instant) -> &str {
        self.status_line_with(now, None)
    }

    /// [`ControlFrame::status_line`] with the window's own latest event `own` (when it
    /// happened, and its text) among the session's.
    pub fn status_line_with<'a>(
        &'a self,
        now: Instant,
        own: Option<(Instant, &'a str)>,
    ) -> &'a str {
        let session = self.events.back().map(|(at, text)| (*at, text.as_str()));
        let newest = match (session, own) {
            (Some(s), Some(o)) => Some(if o.0 > s.0 { o } else { s }),
            (s, o) => s.or(o),
        };
        newest
            .filter(|(at, _)| now.saturating_duration_since(*at) < EVENT_SHOWN)
            .map(|(_, text)| text)
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

    #[test]
    fn the_windows_own_event_shows_when_it_is_the_newest() {
        let start = Instant::now();
        let mut frame = ControlFrame::default();
        let own = Some((start, "Every deck is playing"));
        assert_eq!(frame.status_line_with(start, own), "Every deck is playing");
        let later = start + Duration::from_secs(1);
        frame.events.push_back((later, "Deck 1 ◂ A – B".into()));
        assert_eq!(frame.status_line_with(later, own), "Deck 1 ◂ A – B");
        let own = Some((later + Duration::from_secs(1), "Every deck is playing"));
        assert_eq!(frame.status_line_with(later, own), "Every deck is playing");
        assert_eq!(frame.status_line_with(later + EVENT_SHOWN * 2, own), "");
    }
}
