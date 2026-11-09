//! How the window is arranged and drawn: the [`ViewState`] the session keeps, saves and
//! publishes in every frame. The window changes it only by asking the session (a
//! [`ViewChange`]).

use engine::DeckId;

use crate::command::ViewChange;

/// The beats a waveform lane spans when nothing else says: plan 9's default zoom.
const DEFAULT_BEATS: f64 = 16.0;
/// The smallest and largest share of the window a panel can take, so every area stays
/// reachable.
pub const MIN_SHARE: f32 = 0.05;
pub const MAX_SHARE: f32 = 0.7;

/// Gives each enum the list of its values and the id it is saved under.
macro_rules! ids {
    ($name:ident { $($variant:ident => $id:literal),+ $(,)? }) => {
        impl $name {
            pub const ALL: &[Self] = &[$(Self::$variant),+];

            /// The id this value is saved under in `ui-state.toml`.
            pub fn id(self) -> &'static str {
                match self {
                    $(Self::$variant => $id),+
                }
            }

            /// The value saved as `id`, if there is one.
            pub fn from_id(id: &str) -> Option<Self> {
                Self::ALL.iter().copied().find(|value| value.id() == id)
            }
        }
    };
}

/// Which sections the window shows and how big they are, VirtualDJ's way.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Layout {
    Starter,
    Essentials,
    #[default]
    Pro,
    Performance,
}
ids!(Layout {
    Starter => "starter",
    Essentials => "essentials",
    Pro => "pro",
    Performance => "performance",
});

impl Layout {
    /// The layout `ui.layout` moves to: the four in turn, then back to the first.
    pub fn next(self) -> Self {
        match self {
            Self::Starter => Self::Essentials,
            Self::Essentials => Self::Pro,
            Self::Pro => Self::Performance,
            Self::Performance => Self::Starter,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ColourScheme {
    #[default]
    Dark,
    Light,
}
ids!(ColourScheme { Dark => "dark", Light => "light" });

/// How a waveform lane colours its columns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WaveformStyle {
    /// Coloured by frequency content.
    #[default]
    Frequency,
    /// In the deck's colour.
    DeckColour,
    /// Low, mid and high drawn as separate bands.
    Bands,
    /// Both decks' waveforms over each other in one lane.
    Overlaid,
}
ids!(WaveformStyle {
    Frequency => "frequency",
    DeckColour => "deck_colour",
    Bands => "bands",
    Overlaid => "overlaid",
});

/// Where the waveform lanes sit: above the decks or between them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WaveformPosition {
    #[default]
    Top,
    Middle,
}
ids!(WaveformPosition { Top => "top", Middle => "middle" });

/// What the jog wheels show.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum JogDisplay {
    #[default]
    Plain,
    /// The deck's time and BPM inside the jog.
    Info,
}
ids!(JogDisplay { Plain => "plain", Info => "info" });

/// Which time a deck shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TimeMode {
    Elapsed,
    #[default]
    Remaining,
    Total,
}
ids!(TimeMode {
    Elapsed => "elapsed",
    Remaining => "remaining",
    Total => "total",
});

impl TimeMode {
    /// The mode a click on the time moves to.
    pub fn next(self) -> Self {
        match self {
            Self::Elapsed => Self::Remaining,
            Self::Remaining => Self::Total,
            Self::Total => Self::Elapsed,
        }
    }
}

/// The share of the window each resizable area takes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PanelSizes {
    /// The waveform lanes' share of the height.
    pub lanes: f32,
    /// The deck panels' and mixer's share of the height.
    pub decks: f32,
    /// The browser's folder tree's share of the width.
    pub tree: f32,
    /// The browser's info panel's share of the width.
    pub info: f32,
    /// The sideview's share of the width.
    pub sideview: f32,
}

impl Default for PanelSizes {
    fn default() -> Self {
        Self {
            lanes: 0.22,
            decks: 0.38,
            tree: 0.20,
            info: 0.18,
            sideview: 0.22,
        }
    }
}

impl PanelSizes {
    /// Each share brought within [`MIN_SHARE`] and [`MAX_SHARE`]; one that is not a
    /// number goes back to its default.
    fn clamped(self) -> Self {
        let defaults = Self::default();
        let share = |value: f32, default: f32| {
            if value.is_finite() {
                value.clamp(MIN_SHARE, MAX_SHARE)
            } else {
                default
            }
        };
        Self {
            lanes: share(self.lanes, defaults.lanes),
            decks: share(self.decks, defaults.decks),
            tree: share(self.tree, defaults.tree),
            info: share(self.info, defaults.info),
            sideview: share(self.sideview, defaults.sideview),
        }
    }
}

/// How the window is arranged and drawn.
#[derive(Debug, Clone, PartialEq)]
pub struct ViewState {
    pub layout: Layout,
    pub colour_scheme: ColourScheme,
    pub waveform_style: WaveformStyle,
    pub waveform_position: WaveformPosition,
    pub jog_display: JogDisplay,
    pub beat_counter: bool,
    /// Whether all four decks show, rather than the two the sides play.
    pub four_decks: bool,
    /// Whether the deck panels show the overview waveform.
    pub overview: bool,
    /// Whether the browser shows the selected track's info panel.
    pub info_panel: bool,
    pub sideview: bool,
    /// Per deck, which time it shows.
    pub time_mode: [TimeMode; 4],
    pub sizes: PanelSizes,
    /// How many beats a waveform lane spans (the zoom).
    pub waveform_beats: f64,
}

impl Default for ViewState {
    fn default() -> Self {
        Self {
            layout: Layout::default(),
            colour_scheme: ColourScheme::default(),
            waveform_style: WaveformStyle::default(),
            waveform_position: WaveformPosition::default(),
            jog_display: JogDisplay::default(),
            beat_counter: false,
            four_decks: false,
            overview: true,
            info_panel: true,
            sideview: false,
            time_mode: [TimeMode::default(); 4],
            sizes: PanelSizes::default(),
            waveform_beats: DEFAULT_BEATS,
        }
    }
}

impl ViewState {
    /// The state with each share within [`MIN_SHARE`] and [`MAX_SHARE`] and the beats
    /// within `zoom_range` (fewest beats, most beats).
    pub fn clamped(self, zoom_range: (f64, f64)) -> Self {
        let beats = if self.waveform_beats.is_finite() && self.waveform_beats > 0.0 {
            self.waveform_beats
        } else {
            DEFAULT_BEATS
        };
        Self {
            sizes: self.sizes.clamped(),
            waveform_beats: beats.clamp(zoom_range.0, zoom_range.1),
            ..self
        }
    }

    pub fn apply(&mut self, change: ViewChange) {
        match change {
            ViewChange::SetLayout(layout) => self.layout = layout,
            ViewChange::SetColourScheme(scheme) => self.colour_scheme = scheme,
            ViewChange::SetWaveformStyle(style) => self.waveform_style = style,
            ViewChange::SetWaveformPosition(position) => self.waveform_position = position,
            ViewChange::SetJogDisplay(display) => self.jog_display = display,
            ViewChange::SetBeatCounter(on) => self.beat_counter = on,
            ViewChange::SetFourDecks(on) => self.four_decks = on,
            ViewChange::SetOverview(on) => self.overview = on,
            ViewChange::SetInfoPanel(on) => self.info_panel = on,
            ViewChange::SetSideview(on) => self.sideview = on,
            ViewChange::CycleTime(deck) => {
                let mode = &mut self.time_mode[deck.index()];
                *mode = mode.next();
            }
            ViewChange::SetSizes(sizes) => self.sizes = sizes,
        }
    }

    /// Which time `deck` shows.
    pub fn time_mode(&self, deck: DeckId) -> TimeMode {
        self.time_mode[deck.index()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_mode_cycles() {
        assert_eq!(TimeMode::Total.next(), TimeMode::Elapsed);
        assert_eq!(TimeMode::Elapsed.next(), TimeMode::Remaining);
    }

    #[test]
    fn layout_next_cycles() {
        assert_eq!(Layout::Performance.next(), Layout::Starter);
        assert_eq!(Layout::Starter.next(), Layout::Essentials);
        assert_eq!(Layout::Pro.next(), Layout::Performance);
    }

    #[test]
    fn ids_name_every_value_once() {
        assert_eq!(Layout::from_id("pro"), Some(Layout::Pro));
        assert_eq!(
            WaveformStyle::from_id("deck_colour"),
            Some(WaveformStyle::DeckColour)
        );
        assert_eq!(Layout::from_id("Pro"), None);
        for mode in TimeMode::ALL {
            assert_eq!(TimeMode::from_id(mode.id()), Some(*mode));
        }
    }

    #[test]
    fn clamping_keeps_every_area_reachable() {
        let state = ViewState {
            sizes: PanelSizes {
                lanes: 5.0,
                decks: 0.0,
                tree: f32::NAN,
                ..PanelSizes::default()
            },
            waveform_beats: 1000.0,
            ..ViewState::default()
        }
        .clamped((4.0, 64.0));
        assert_eq!(state.sizes.lanes, MAX_SHARE);
        assert_eq!(state.sizes.decks, MIN_SHARE);
        assert_eq!(state.sizes.tree, PanelSizes::default().tree);
        assert_eq!(state.waveform_beats, 64.0);
        let lost = ViewState {
            waveform_beats: f64::NAN,
            ..ViewState::default()
        };
        assert_eq!(lost.clamped((4.0, 64.0)).waveform_beats, DEFAULT_BEATS);
    }

    #[test]
    fn a_click_on_the_time_cycles_its_mode() {
        let deck = DeckId::new(1).unwrap();
        let mut state = ViewState::default();
        let seen: Vec<TimeMode> = (0..3)
            .map(|_| {
                state.apply(ViewChange::CycleTime(deck));
                state.time_mode(deck)
            })
            .collect();
        assert_eq!(
            seen,
            [TimeMode::Total, TimeMode::Elapsed, TimeMode::Remaining]
        );
        assert_eq!(
            state.time_mode(DeckId::new(0).unwrap()),
            TimeMode::Remaining
        );
    }
}
