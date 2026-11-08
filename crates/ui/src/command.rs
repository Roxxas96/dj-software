//! What the window asks the session to do. The window sends a [`UiCommand`] and never
//! waits: the session applies it on the control thread and publishes the result in the
//! next frame.

use engine::{DeckId, EffectKind};

use crate::view::{ColourScheme, JogDisplay, Layout, PanelSizes, WaveformPosition, WaveformStyle};

/// A request from the window to the session.
#[derive(Debug, Clone, PartialEq)]
pub enum UiCommand {
    /// Changes how the window is arranged or drawn.
    View(ViewChange),
    /// Zooms the waveforms by this many steps; positive zooms in.
    Zoom(i32),
    /// Performs an action as if a controller input bound to it sent `Input` (a dropdown
    /// pressing `pads.mode_beat_jump`, say).
    Action(actions::Action, actions::Input),
    /// Puts effect `kind` in `deck`'s FX `slot`.
    FxSlot {
        deck: DeckId,
        slot: usize,
        kind: EffectKind,
    },
    /// Sets the configuration key `key` (`fx.slots`, say) to `value`, written as in
    /// `config.toml`.
    SetSetting { key: String, value: String },
    /// Puts every setting back to its default.
    ResetSettings,
}

/// One change to the [`ViewState`](crate::ViewState), carrying the new value. The zoom
/// changes by [`UiCommand::Zoom`] steps instead.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ViewChange {
    SetLayout(Layout),
    SetColourScheme(ColourScheme),
    SetWaveformStyle(WaveformStyle),
    SetWaveformPosition(WaveformPosition),
    SetJogDisplay(JogDisplay),
    SetBeatCounter(bool),
    SetFourDecks(bool),
    SetOverview(bool),
    SetInfoPanel(bool),
    SetSideview(bool),
    /// Moves the deck's time to the next [`TimeMode`](crate::TimeMode).
    CycleTime(DeckId),
    SetSizes(PanelSizes),
}
