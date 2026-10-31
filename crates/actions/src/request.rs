//! Work an action asks the app to do because it needs more than the engine: the library,
//! the loaded track's sample rate or the playhead.

use engine::DeckId;

/// A request [`apply`](crate::apply) returns for the app to perform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum AppRequest {
    /// Edit the beatgrid of the track loaded on `deck`.
    Grid { deck: DeckId, edit: GridEditRequest },
}

/// A beatgrid edit as an action asks for it, before the app knows the track: the app turns
/// it into the library's grid edit, adding the track's sample rate, the playhead and the
/// tap time.
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
    /// A tap-tempo tap, timed by the app when it performs the request.
    Tap,
    /// Lock or unlock the grid.
    ToggleLock,
}
