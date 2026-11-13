//! The window's playlist edits (drops into a playlist, reorders, removals) as writer
//! batches: each edit is one [`WriteOp::Batch`] of the playlist ops the writer has, so it
//! commits in one transaction, all of it or none.

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

/// Moves the entries at positions `from` of `playlist` together, in playlist order, to the
/// gap before position `to` (counted before the move). One move per entry that moves.
pub(super) fn move_to(playlist: i64, from: &[usize], to: usize) -> WriteOp {
    let mut from = from.to_vec();
    from.sort_unstable();
    from.dedup();
    let (before, after) = from.split_at(from.partition_point(|&f| f < to));
    let mut ops = Vec::with_capacity(from.len());
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

/// Removes the entries at `positions` of `playlist`.
pub(super) fn remove(playlist: i64, positions: &[usize]) -> WriteOp {
    let mut positions = positions.to_vec();
    // Last first, so each position still names its entry.
    positions.sort_unstable_by(|a, b| b.cmp(a));
    positions.dedup();
    let ops = positions
        .into_iter()
        .map(|position| WriteOp::RemoveFromPlaylist { playlist, position });
    WriteOp::Batch(ops.collect())
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
                ref other => panic!("unexpected op {other:?}"),
            }
        }
        entries
    }

    #[test]
    fn moving_a_duplicate_moves_that_entry() {
        // [a, b, a]: the a at 2 moves to the top, ahead of the a that was at 0.
        let moved = applied(vec!["a@0", "b@1", "a@2"], &move_to(1, &[2], 0));
        assert_eq!(moved, ["a@2", "a@0", "b@1"]);
    }

    #[test]
    fn moves_keep_the_selection_in_playlist_order() {
        let list: Vec<usize> = (0..6).collect();
        // Up, down, around the gap, to the end, and nowhere.
        assert_eq!(
            applied(list.clone(), &move_to(1, &[4, 1], 0)),
            [1, 4, 0, 2, 3, 5]
        );
        assert_eq!(
            applied(list.clone(), &move_to(1, &[0, 2], 5)),
            [1, 3, 4, 0, 2, 5]
        );
        assert_eq!(
            applied(list.clone(), &move_to(1, &[1, 4], 3)),
            [0, 2, 1, 4, 3, 5]
        );
        assert_eq!(
            applied(list.clone(), &move_to(1, &[0, 5], 6)),
            [1, 2, 3, 4, 0, 5]
        );
        assert_eq!(applied(list.clone(), &move_to(1, &[2, 3], 3)), list);
        // One op per entry moved, at most.
        let WriteOp::Batch(ops) = move_to(1, &[1, 4], 3) else {
            unreachable!()
        };
        assert!(ops.len() <= 2, "{ops:?}");
    }

    #[test]
    fn removal_takes_exactly_those_positions() {
        let removed = applied(vec!["a@0", "b@1", "a@2", "c@3"], &remove(1, &[2, 0, 2]));
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
