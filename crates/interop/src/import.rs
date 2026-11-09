//! What an import brings into the library, how it meets what is already there, and the
//! one transaction that writes it.

use std::path::PathBuf;

use library::{HotcuePoint, Library, WriteOp};

use crate::InteropError;

/// Everything an import would write, worked out before anything is written.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportPlan {
    /// The imported songs found in the library, in the order they were read.
    pub tracks: Vec<TrackImport>,
    pub playlists: Vec<PlaylistImport>,
    /// Songs and playlist entries not in the library (or without a known sample rate),
    /// each listed once; they are not added.
    pub unmatched: Vec<PathBuf>,
    /// Hotcues and loops that found no free slot, or lost theirs to an earlier one.
    pub dropped: usize,
    /// Markers that are not hotcues or loops, not imported.
    pub other_markers: usize,
}

/// One library track's hotcue slots: what the import brings and what is there now.
#[derive(Debug, Clone, PartialEq)]
pub struct TrackImport {
    pub track: i64,
    /// The song's path in the other software.
    pub path: PathBuf,
    /// Indexed by slot minus 1, like [`library::Reader::hotcue_points`].
    pub incoming: [Option<HotcuePoint>; 8],
    pub existing: [Option<HotcuePoint>; 8],
}

impl TrackImport {
    /// The track already has hotcues and the import brings some: the user must choose.
    #[must_use]
    pub fn conflicts(&self) -> bool {
        self.existing.iter().any(Option::is_some) && self.incoming.iter().any(Option::is_some)
    }
}

/// A playlist to create, under its final name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaylistImport {
    /// Suffixed (`Name (2)`) when a different playlist already has the name.
    pub name: String,
    pub tracks: Vec<i64>,
    pub action: PlaylistAction,
}

/// What happens to an imported playlist.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaylistAction {
    Create,
    /// A playlist with this name and these tracks is already there.
    SkipSame,
}

/// What to do with a track whose hotcues conflict with the incoming ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolution {
    /// Keep Open Jockey's hotcues; import nothing for the track.
    Keep,
    /// Make the track's slots exactly the incoming ones.
    Replace,
    /// Keep Open Jockey's slots and fill the empty ones with the incoming hotcues.
    Merge,
}

/// What [`apply`] wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportReport {
    /// The library backup taken just before writing.
    pub backup: PathBuf,
    pub tracks_changed: usize,
    pub playlists_created: usize,
}

impl ImportPlan {
    /// The whole import as one [`WriteOp::Batch`], with `resolve` choosing for each
    /// conflicting track; a track without conflict takes its incoming slots. Slots that
    /// would not change are left out, so importing the same data again is an empty batch.
    #[must_use]
    pub fn batch(&self, resolve: impl Fn(&TrackImport) -> Resolution) -> WriteOp {
        self.resolved(resolve).0
    }

    /// [`ImportPlan::batch`] and how many tracks it changes, asking `resolve` once per
    /// conflicting track.
    fn resolved(&self, resolve: impl Fn(&TrackImport) -> Resolution) -> (WriteOp, usize) {
        let mut ops = Vec::new();
        let mut tracks_changed = 0;
        for t in &self.tracks {
            let before = ops.len();
            track_ops(t, &resolve, &mut ops);
            tracks_changed += usize::from(ops.len() > before);
        }
        ops.extend(
            self.playlists
                .iter()
                .filter(|p| p.action == PlaylistAction::Create)
                .map(|p| WriteOp::CreatePlaylistWith {
                    name: p.name.clone(),
                    tracks: p.tracks.clone(),
                }),
        );
        (WriteOp::Batch(ops), tracks_changed)
    }
}

/// The ops that bring `t`'s slots to what `resolve` chose.
fn track_ops(
    t: &TrackImport,
    resolve: impl Fn(&TrackImport) -> Resolution,
    ops: &mut Vec<WriteOp>,
) {
    let resolution = if t.conflicts() {
        resolve(t)
    } else {
        Resolution::Merge
    };
    for (slot, (incoming, existing)) in (1u8..).zip(t.incoming.iter().zip(&t.existing)) {
        let put = |point: &HotcuePoint| WriteOp::PutHotcue {
            track: t.track,
            slot,
            point: point.clone(),
        };
        match (resolution, incoming, existing) {
            (Resolution::Keep, ..) => {}
            (Resolution::Replace, Some(new), old) if old.as_ref() != Some(new) => {
                ops.push(put(new));
            }
            (Resolution::Replace, None, Some(_)) => ops.push(WriteOp::DeleteHotcue {
                track: t.track,
                slot,
            }),
            (Resolution::Merge, Some(new), None) => ops.push(put(new)),
            _ => {}
        }
    }
}

/// Backs the library up, then writes `plan` (resolved by `resolve`) in one transaction:
/// all of it or, on any failure, none of it.
///
/// # Errors
///
/// [`InteropError::Library`] when the backup or the write fails; nothing is written then.
pub fn apply(
    lib: &Library,
    plan: &ImportPlan,
    resolve: impl Fn(&TrackImport) -> Resolution,
) -> Result<ImportReport, InteropError> {
    let backup = lib.backup_now()?;
    let (batch, tracks_changed) = plan.resolved(resolve);
    let playlists_created = plan
        .playlists
        .iter()
        .filter(|p| p.action == PlaylistAction::Create)
        .count();
    lib.writer().submit_wait(batch)?;
    Ok(ImportReport {
        backup,
        tracks_changed,
        playlists_created,
    })
}
