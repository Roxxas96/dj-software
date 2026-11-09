//! Reads other DJ software's data so it can be imported; nothing here writes to it.

pub mod virtualdj;

pub use virtualdj::{
    InteropError, VdjPlaylist, VdjPoi, VdjSong, VdjSources, find_sources, read_database,
    read_playlists,
};
