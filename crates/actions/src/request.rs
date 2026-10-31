//! Work an action asks the app to do because it needs more than the engine: the library,
//! the loaded track's sample rate, the playhead or the browser.

use engine::DeckId;

/// A request [`apply`](crate::apply) returns for the app to perform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum AppRequest {
    /// Edit the beatgrid of the track loaded on `deck`.
    Grid { deck: DeckId, edit: GridEditRequest },
    /// Move through the library browser.
    Browser(BrowserRequest),
    /// Load the track selected in the library browser onto `deck`.
    LoadSelected { deck: DeckId },
}

/// A move in the library browser (the BROWSE knob).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowserRequest {
    /// Move the cursor of the focused pane by this many rows (positive is down the list).
    Scroll(i32),
    /// Switch focus between the tree (playlists, folders, all tracks) and the track list.
    ToggleFocus,
}

/// A beatgrid edit as an action asks for it, before the app knows the track: the app turns
/// it into the library's grid edit, adding the track's sample rate and the playhead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GridEditRequest {
    /// Move every beat by this many milliseconds (negative moves earlier).
    Shift { millis: i32 },
    /// Put a beat exactly at the playhead.
    SetDownbeat,
    /// Halve the BPM, keeping the beat nearest the playhead.
    Halve,
    /// Double the BPM, keeping the beat nearest the playhead.
    Double,
    /// A tap-tempo tap at `at_micros` on the controller clock (the MIDI message's time,
    /// see `controller::clock::now_micros`).
    Tap { at_micros: u64 },
    /// Lock or unlock the grid.
    ToggleLock,
}
