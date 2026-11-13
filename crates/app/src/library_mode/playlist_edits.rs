//! The window's playlist edits (drops into a playlist, reorders, removals) as writer
//! batches: each edit is one [`WriteOp::Batch`] of the playlist ops the writer has, so it
//! commits in one transaction, all of it or none. A move or a removal names each entry by
//! its position and the track the window saw there, and its batch checks them first: an
//! edit made from an older view of the playlist writes nothing.

use library::WriteOp;

/// Appends `tracks` to `playlist`, in their order.
pub(super) fn append(playlist: i64, tracks: &[i64]) -> WriteOp {
    let ops = tracks.iter().map(|&track| WriteOp::AddToPlaylist {
        playlist,
        track,
        at: None,
    });
    WriteOp::Batch(ops.collect())
}

/// The check that `entries` (position, track) still hold in `playlist`.
fn expect(playlist: i64, entries: &[(usize, i64)]) -> WriteOp {
    WriteOp::ExpectPlaylistEntries {
        playlist,
        entries: entries.to_vec(),
    }
}

/// Moves the entries `from` (position, track) of `playlist` together, in playlist order, to
/// the gap before position `to` (counted before the move), once they are checked. One move
/// per entry that moves.
pub(super) fn move_to(playlist: i64, from: &[(usize, i64)], to: usize) -> WriteOp {
    let check = expect(playlist, from);
    let mut from: Vec<usize> = from.iter().map(|&(position, _)| position).collect();
    from.sort_unstable();
    from.dedup();
    let (before, after) = from.split_at(from.partition_point(|&f| f < to));
    let mut ops = Vec::with_capacity(from.len() + 1);
    ops.push(check);
    let mut add = |from: usize, to: usize| {
        if from != to {
            ops.push(WriteOp::MoveInPlaylist { playlist, from, to });
        }
    };
    // The entries before the gap, first to last, each to just before it: every one moved
    // pulls the later ones back by one.
    for (moved, &f) in before.iter().enumerate() {
        add(f - moved, to - 1);
    }
    // Then those after it, each just after the one before: taking one out and putting it
    // back before the next leaves the next where it was.
    for (moved, &f) in after.iter().enumerate() {
        add(f, to + moved);
    }
    WriteOp::Batch(ops)
}

/// Removes the entries `entries` (position, track) of `playlist`, once they are checked.
pub(super) fn remove(playlist: i64, entries: &[(usize, i64)]) -> WriteOp {
    let mut positions: Vec<usize> = entries.iter().map(|&(position, _)| position).collect();
    // Last first, so each position still names its entry.
    positions.sort_unstable_by(|a, b| b.cmp(a));
    positions.dedup();
    let removals = positions
        .into_iter()
        .map(|position| WriteOp::RemoveFromPlaylist { playlist, position });
    WriteOp::Batch(
        std::iter::once(expect(playlist, entries))
            .chain(removals)
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `entries` after `op`, as the writer applies each playlist op of the batch.
    fn applied<T: Clone>(mut entries: Vec<T>, op: &WriteOp) -> Vec<T> {
        let WriteOp::Batch(ops) = op else {
            panic!("not one batch: {op:?}");
        };
        for op in ops {
            match *op {
                WriteOp::MoveInPlaylist { from, to, .. } => {
                    let moved = entries.remove(from);
                    entries.insert(to.min(entries.len()), moved);
                }
                WriteOp::RemoveFromPlaylist { position, .. } => {
                    entries.remove(position);
                }
                // Checked by the writer; here every entry holds.
                WriteOp::ExpectPlaylistEntries { .. } => {}
                ref other => panic!("unexpected op {other:?}"),
            }
        }
        entries
    }

    #[test]
    fn moving_a_duplicate_moves_that_entry() {
        // [a, b, a]: the a at 2 moves to the top, ahead of the a that was at 0.
        let moved = applied(vec!["a@0", "b@1", "a@2"], &move_to(1, &[(2, 0)], 0));
        assert_eq!(moved, ["a@2", "a@0", "b@1"]);
    }

    #[test]
    fn moves_keep_the_selection_in_playlist_order() {
        let list: Vec<usize> = (0..6).collect();
        // Up, down, around the gap, to the end, and nowhere.
        assert_eq!(
            applied(list.clone(), &move_to(1, &[(4, 0), (1, 0)], 0)),
            [1, 4, 0, 2, 3, 5]
        );
        assert_eq!(
            applied(list.clone(), &move_to(1, &[(0, 0), (2, 0)], 5)),
            [1, 3, 4, 0, 2, 5]
        );
        assert_eq!(
            applied(list.clone(), &move_to(1, &[(1, 0), (4, 0)], 3)),
            [0, 2, 1, 4, 3, 5]
        );
        assert_eq!(
            applied(list.clone(), &move_to(1, &[(0, 0), (5, 0)], 6)),
            [1, 2, 3, 4, 0, 5]
        );
        assert_eq!(
            applied(list.clone(), &move_to(1, &[(2, 0), (3, 0)], 3)),
            list
        );
        // One op per entry moved, at most.
        let WriteOp::Batch(ops) = move_to(1, &[(1, 0), (4, 0)], 3) else {
            unreachable!()
        };
        // The check, then one op per entry moved at most.
        assert!(ops.len() <= 3, "{ops:?}");
        assert!(matches!(ops[0], WriteOp::ExpectPlaylistEntries { .. }));
    }

    #[test]
    fn removal_takes_exactly_those_positions() {
        let removed = applied(
            vec!["a@0", "b@1", "a@2", "c@3"],
            &remove(1, &[(2, 0), (0, 0), (2, 0)]),
        );
        assert_eq!(removed, ["b@1", "c@3"]);
    }

    #[test]
    fn appending_adds_each_track_at_the_end_in_one_batch() {
        let op = append(3, &[9, 7, 9]);
        let expected: Vec<WriteOp> = [9, 7, 9]
            .map(|track| WriteOp::AddToPlaylist {
                playlist: 3,
                track,
                at: None,
            })
            .into();
        assert_eq!(op, WriteOp::Batch(expected));
    }
}
