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
    /// A step of the import from VirtualDJ.
    Import(ImportStep),
    /// A step of the export to a USB stick.
    Export(ExportStep),
}

/// What the window asks of the import from VirtualDJ; the session answers with the
/// frame's [`ImportView`](crate::ImportView).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportStep {
    /// Reads VirtualDJ's data and shows what the import would do.
    Scan,
    /// Answers the question about `track`, and with `rest_too` every question left.
    Resolve {
        track: i64,
        resolution: Resolution,
        rest_too: bool,
    },
    /// Imports what the summary showed, asking first about each conflicting track.
    Apply,
    /// Stops before anything is written, or puts a finished import's result away.
    Cancel,
}

/// What the window asks of the export to a USB stick; the session answers with the
/// frame's [`ExportView`](crate::ExportView).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExportStep {
    /// Lists the sticks and the playlists again.
    Open,
    /// Works out what exporting `playlists` (library ids) to stick `stick`, an index in
    /// the choice's sticks, would do; writes nothing.
    Check { stick: usize, playlists: Vec<i64> },
    /// Writes what the summary showed.
    Write,
    /// Goes from the summary back to the choice.
    Back,
    /// Puts the choice, or a finished export's result, away.
    Close,
}

/// What to do with a track that has hotcues both here and in VirtualDJ.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolution {
    /// Keep Open Jockey's hotcues.
    Keep,
    /// Use VirtualDJ's hotcues instead.
    Replace,
    /// Keep Open Jockey's hotcues and fill the empty slots with VirtualDJ's.
    Merge,
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
