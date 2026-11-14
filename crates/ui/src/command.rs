//! What the window asks the session to do. The window sends a [`UiCommand`] and never
//! waits: the session applies it on the control thread and publishes the result in the
//! next frame.

use engine::{DeckId, EffectKind, HotcueSlot};
use library::BrowserEdit;

use crate::view::{
    ColourScheme, JogDisplay, Layout, PanelSizes, WaveformPalette, WaveformPosition, WaveformStyle,
};

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
    /// A change to the library, which the session makes through library mode.
    Library(LibraryStep),
}

/// What the window asks of the library.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LibraryStep {
    /// Changes the browser as a click on it does.
    Browse(BrowserEdit),
    /// Loads library track `track` on `deck`, as the console's `load` does. Refused while
    /// `deck` plays.
    Load { deck: DeckId, track: i64 },
    /// Appends library tracks `tracks` to `playlist`, in their order, in one write.
    AddToPlaylist { playlist: i64, tracks: Vec<i64> },
    /// Appends every track of the browser's list to `playlist`, in list order, in one write.
    /// The library resolves the tracks when it makes the write: the window sends no ids for
    /// a list it selected whole ([`BrowserEdit::SelectAll`]).
    ///
    /// `list` is the list the window selected whole; the session adds nothing, with an
    /// event, unless the browser still shows it with every row selected, or when `playlist`
    /// is that list's own playlist.
    AddListToPlaylist {
        playlist: i64,
        list: library::ListIdentity,
    },
    /// Moves the entries `from` of `playlist`, each a position (from 0) and the track the
    /// window saw there, together, in playlist order, to the gap before position `to`
    /// (`0..=len`, counted before the move: `len` is the end), in one write. Nothing moves
    /// if any of them no longer holds its track.
    MoveInPlaylist {
        playlist: i64,
        from: Vec<(usize, i64)>,
        to: usize,
    },
    /// Removes the entries `positions` of `playlist`, each a position (from 0) and the
    /// track the window saw there, in one write; nothing is removed if any of them no
    /// longer holds its track. The tracks stay in the library.
    RemoveFromPlaylist {
        playlist: i64,
        positions: Vec<(usize, i64)>,
    },
    /// Makes a playlist called `name` and highlights it in the browser's tree. Refused,
    /// with an event, if the name is empty or a playlist already has it.
    CreatePlaylist { name: String },
    /// Renames `playlist` to `name`, refused as [`LibraryStep::CreatePlaylist`] is.
    RenamePlaylist { playlist: i64, name: String },
    /// Deletes `playlist`; its tracks stay in the library. The window asks first.
    DeletePlaylist { playlist: i64 },
    /// Asks what deleting `target` would do: the session answers with the frame's
    /// [`DeleteCheck`](crate::DeleteCheck) (the question to ask, or why it cannot be
    /// deleted). Nothing is deleted.
    AskDelete(DeleteTarget),
    /// Deletes `target`, once the window asked and the viewer said yes. The session checks
    /// again first: anything refused now is an event, and nothing is deleted.
    Delete(DeleteTarget),
    /// Names and colours (`0xRRGGBB`; `None` for the slot's default) hotcue `slot` of
    /// `track`, the track the window saw on `deck` when its editor opened. Saved only if
    /// `deck` still holds `track`.
    HotcueLabel {
        deck: DeckId,
        track: i64,
        slot: HotcueSlot,
        name: Option<String>,
        color: Option<u32>,
    },
    /// Looks `track`'s cover up online now, for one other than the cover it has, or after
    /// a lookup that found none. A cover found replaces the one it has.
    FindCover { track: i64 },
    /// Removes the cover found online for `track` and never looks it up again (until a
    /// [`LibraryStep::FindCover`]); the track shows its own picture or the plain tile.
    RemoveCover { track: i64 },
    /// Looks up again, once, the covers of the tracks whose lookup found none.
    RetryMissingCovers,
}

/// What a Delete in the browser deletes, from where the viewer is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeleteTarget {
    /// The entries of `playlist`, each a position (from 0) and the track the window saw
    /// there, taken out of it as [`LibraryStep::RemoveFromPlaylist`] does. The files stay.
    Entries {
        playlist: i64,
        entries: Vec<(usize, i64)>,
    },
    /// Every entry of the playlist list `list` the window selected whole: refused if the
    /// browser no longer shows it with every row selected.
    AllEntries { list: library::ListIdentity },
    /// The files of library tracks `tracks`, moved to the Trash.
    Files { tracks: Vec<i64> },
    /// Every file of the folder list `list` the window selected whole, moved to the Trash;
    /// refused as [`DeleteTarget::AllEntries`] is.
    AllFiles { list: library::ListIdentity },
    /// Playlist `id`, as [`LibraryStep::DeletePlaylist`] does; its tracks stay.
    Playlist { id: i64 },
    /// The folder `path` and everything in it, moved to the Trash.
    Folder { path: std::path::PathBuf },
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
    SetWaveformPalette(WaveformPalette),
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
