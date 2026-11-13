//! Playlists and set-prep queries.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use library::{
    Library, LibraryError, PlaylistSummary, RawStatement, TrackFilter, TrackRow, TrackSort,
    TrackSortKey, WriteOp, WriteOutcome,
};
use rusqlite::types::Value;

fn open() -> (tempfile::TempDir, Library) {
    let dir = tempfile::tempdir().unwrap();
    let lib = Library::open(&dir.path().join("library.sqlite3")).unwrap();
    (dir, lib)
}

fn raw(sql: &str, params: Vec<Value>) -> RawStatement {
    RawStatement {
        sql: sql.into(),
        params,
    }
}

fn insert_track_stmt(path: &str, title: &str, artist: &str) -> RawStatement {
    raw(
        "INSERT INTO tracks(path, fingerprint, title, artist, added_at) VALUES (?1, x'00', ?2, ?3, 0)",
        vec![
            Value::Text(path.into()),
            Value::Text(title.into()),
            Value::Text(artist.into()),
        ],
    )
}

/// Inserts tracks `(path, title, artist)` and returns their ids.
fn add_tracks(lib: &Library, tracks: &[(&str, &str, &str)]) -> Vec<i64> {
    let stmts = tracks
        .iter()
        .map(|(p, t, a)| insert_track_stmt(p, t, a))
        .collect();
    lib.writer().submit_wait(WriteOp::Raw(stmts)).unwrap();
    let reader = lib.reader().unwrap();
    tracks
        .iter()
        .map(|(p, _, _)| {
            reader
                .connection()
                .query_row("SELECT id FROM tracks WHERE path = ?1", [p], |r| r.get(0))
                .unwrap()
        })
        .collect()
}

fn n_tracks(lib: &Library, n: usize) -> Vec<i64> {
    let owned: Vec<(String, String, String)> = (0..n)
        .map(|i| (format!("/m/{i}.flac"), format!("t{i}"), "a".to_owned()))
        .collect();
    let refs: Vec<(&str, &str, &str)> = owned
        .iter()
        .map(|(p, t, a)| (p.as_str(), t.as_str(), a.as_str()))
        .collect();
    add_tracks(lib, &refs)
}

fn create(lib: &Library, name: &str) -> i64 {
    match lib
        .writer()
        .submit_wait(WriteOp::CreatePlaylist { name: name.into() })
        .unwrap()
    {
        WriteOutcome::Created { id } => id,
        other => panic!("unexpected outcome {other:?}"),
    }
}

fn submit(lib: &Library, op: WriteOp) {
    lib.writer().submit_wait(op).unwrap();
}

fn add(lib: &Library, playlist: i64, track: i64, at: Option<usize>) {
    submit(
        lib,
        WriteOp::AddToPlaylist {
            playlist,
            track,
            at,
        },
    );
}

fn order(lib: &Library, playlist: i64) -> Vec<i64> {
    lib.reader()
        .unwrap()
        .playlist_tracks(playlist, 0, 1000)
        .unwrap()
        .iter()
        .map(|r| r.id)
        .collect()
}

fn positions(lib: &Library, playlist: i64) -> Vec<i64> {
    let reader = lib.reader().unwrap();
    let mut stmt = reader
        .connection()
        .prepare("SELECT position FROM playlist_tracks WHERE playlist_id = ?1 ORDER BY position")
        .unwrap();
    stmt.query_map([playlist], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

fn playlists(lib: &Library) -> Vec<PlaylistSummary> {
    lib.reader().unwrap().playlists().unwrap()
}

fn query(lib: &Library, filter: &TrackFilter, sort: TrackSort) -> Vec<TrackRow> {
    lib.reader().unwrap().tracks(filter, sort, 0, 1000).unwrap()
}

#[test]
fn create_rename_delete_playlist() {
    let (_d, lib) = open();
    let a = create(&lib, "Warmup");
    let b = create(&lib, "Peak");
    let c = create(&lib, "Closing");
    let names =
        |lib: &Library| -> Vec<String> { playlists(lib).into_iter().map(|p| p.name).collect() };
    assert_eq!(names(&lib), ["Warmup", "Peak", "Closing"]);

    submit(
        &lib,
        WriteOp::RenamePlaylist {
            id: b,
            name: "Main room".into(),
        },
    );
    assert_eq!(names(&lib), ["Warmup", "Main room", "Closing"]);

    submit(
        &lib,
        WriteOp::MovePlaylist {
            id: c,
            to_position: 0,
        },
    );
    assert_eq!(names(&lib), ["Closing", "Warmup", "Main room"]);

    submit(&lib, WriteOp::DeletePlaylist(a));
    assert_eq!(names(&lib), ["Closing", "Main room"]);
    let reader = lib.reader().unwrap();
    let positions: Vec<i64> = reader
        .connection()
        .prepare("SELECT position FROM playlists ORDER BY position")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(positions, [0, 1]);

    let err = lib
        .writer()
        .submit_wait(WriteOp::RenamePlaylist {
            id: a,
            name: "x".into(),
        })
        .unwrap_err();
    assert!(
        matches!(err, LibraryError::NoSuchPlaylist(id) if id == a),
        "{err:?}"
    );
}

#[test]
fn add_move_remove_keep_positions_dense() {
    let (_d, lib) = open();
    let t = n_tracks(&lib, 4);
    let p = create(&lib, "Set");
    for id in &t {
        add(&lib, p, *id, None);
    }
    assert_eq!(order(&lib, p), t);

    // Insert in the middle, duplicates allowed.
    add(&lib, p, t[0], Some(2));
    assert_eq!(order(&lib, p), [t[0], t[1], t[0], t[2], t[3]]);
    assert_eq!(positions(&lib, p), [0, 1, 2, 3, 4]);

    // Past the end appends.
    add(&lib, p, t[3], Some(99));
    assert_eq!(order(&lib, p).len(), 6);

    // Forward and backward moves.
    submit(
        &lib,
        WriteOp::MoveInPlaylist {
            playlist: p,
            from: 0,
            to: 4,
        },
    );
    assert_eq!(order(&lib, p), [t[1], t[0], t[2], t[3], t[0], t[3]]);
    submit(
        &lib,
        WriteOp::MoveInPlaylist {
            playlist: p,
            from: 5,
            to: 0,
        },
    );
    assert_eq!(order(&lib, p), [t[3], t[1], t[0], t[2], t[3], t[0]]);
    assert_eq!(positions(&lib, p), [0, 1, 2, 3, 4, 5]);

    submit(
        &lib,
        WriteOp::RemoveFromPlaylist {
            playlist: p,
            position: 0,
        },
    );
    assert_eq!(order(&lib, p), [t[1], t[0], t[2], t[3], t[0]]);
    assert_eq!(positions(&lib, p), [0, 1, 2, 3, 4]);

    let err = lib
        .writer()
        .submit_wait(WriteOp::RemoveFromPlaylist {
            playlist: p,
            position: 5,
        })
        .unwrap_err();
    assert!(
        matches!(
            err,
            LibraryError::PositionOutOfRange {
                position: 5,
                len: 5
            }
        ),
        "{err:?}"
    );
    assert_eq!(order(&lib, p).len(), 5);
}

#[test]
fn deleting_playlist_keeps_tracks() {
    let (_d, lib) = open();
    let t = n_tracks(&lib, 3);
    let p = create(&lib, "Gone soon");
    for id in &t {
        add(&lib, p, *id, None);
    }
    lib.writer()
        .submit_wait(WriteOp::Raw(vec![
            raw(
                "INSERT INTO cues(track_id, cue_frames) VALUES (?1, 10.0)",
                vec![Value::Integer(t[0])],
            ),
            raw(
                "INSERT INTO hotcues(track_id, slot, frames) VALUES (?1, 1, 10.0)",
                vec![Value::Integer(t[0])],
            ),
        ]))
        .unwrap();
    submit(&lib, WriteOp::DeletePlaylist(p));
    let reader = lib.reader().unwrap();
    let conn = reader.connection();
    let count = |table: &str| -> i64 {
        conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
            .unwrap()
    };
    assert_eq!(count("tracks"), 3);
    assert_eq!(count("playlist_tracks"), 0);
    assert_eq!(count("playlists"), 0);
    assert_eq!(count("cues"), 1);
    assert_eq!(count("hotcues"), 1);
}

#[test]
fn unprepared_count_counts_tracks_without_cues() {
    let (_d, lib) = open();
    let t = n_tracks(&lib, 4);
    let p = create(&lib, "Prep");
    for id in &t {
        add(&lib, p, *id, None);
    }
    add(&lib, p, t[3], None); // a duplicate counts as its own entry
    let empty = create(&lib, "Empty");

    let summary = |lib: &Library, id: i64| playlists(lib).into_iter().find(|s| s.id == id).unwrap();
    assert_eq!(
        (
            summary(&lib, p).track_count,
            summary(&lib, p).unprepared_count
        ),
        (5, 5)
    );
    assert_eq!(
        (
            summary(&lib, empty).track_count,
            summary(&lib, empty).unprepared_count
        ),
        (0, 0)
    );

    // t[0] gets a cue, t[1] a hot cue, t[2] both: t[3] (twice) stays unprepared.
    lib.writer()
        .submit_wait(WriteOp::Raw(vec![
            raw(
                "INSERT INTO cues(track_id, cue_frames) VALUES (?1, 10.0)",
                vec![Value::Integer(t[0])],
            ),
            raw(
                "INSERT INTO hotcues(track_id, slot, frames) VALUES (?1, 1, 10.0)",
                vec![Value::Integer(t[1])],
            ),
            raw(
                "INSERT INTO cues(track_id, cue_frames) VALUES (?1, 10.0)",
                vec![Value::Integer(t[2])],
            ),
            raw(
                "INSERT INTO hotcues(track_id, slot, frames) VALUES (?1, 2, 10.0)",
                vec![Value::Integer(t[2])],
            ),
        ]))
        .unwrap();
    let s = summary(&lib, p);
    assert_eq!((s.track_count, s.unprepared_count), (5, 2));

    let rows = lib.reader().unwrap().playlist_tracks(p, 0, 10).unwrap();
    let has: Vec<bool> = rows.iter().map(|r| r.has_cues).collect();
    assert_eq!(has, [true, true, true, false, false]);
}

#[test]
fn paged_queries_return_stable_order() {
    let (_d, lib) = open();
    // Many ties on artist: paging must still never repeat or skip a row.
    let owned: Vec<(String, String, String)> = (0..25)
        .map(|i| {
            (
                format!("/m/{i:02}.flac"),
                format!("t{i}"),
                ["Bee", "ant", "Cat"][i % 3].to_owned(),
            )
        })
        .collect();
    let refs: Vec<(&str, &str, &str)> = owned
        .iter()
        .map(|(p, t, a)| (p.as_str(), t.as_str(), a.as_str()))
        .collect();
    let ids = add_tracks(&lib, &refs);
    let reader = lib.reader().unwrap();
    for descending in [false, true] {
        let sort = TrackSort {
            key: TrackSortKey::Artist,
            descending,
        };
        let all = reader
            .tracks(&TrackFilter::default(), sort, 0, 100)
            .unwrap();
        assert_eq!(all.len(), 25);
        let mut paged = Vec::new();
        for page in 0..5 {
            paged.extend(
                reader
                    .tracks(&TrackFilter::default(), sort, page * 6, 6)
                    .unwrap(),
            );
        }
        assert_eq!(paged, all);
        // Ties within an artist group break by id: ascending, or descending with `descending`.
        for group in ["ant", "bee", "cat"] {
            let ids_in_group: Vec<i64> = all
                .iter()
                .filter(|r| {
                    r.artist
                        .as_deref()
                        .is_some_and(|a| a.eq_ignore_ascii_case(group))
                })
                .map(|r| r.id)
                .collect();
            assert!(ids_in_group.len() >= 8);
            let mut sorted = ids_in_group.clone();
            sorted.sort_unstable();
            if descending {
                sorted.reverse();
            }
            assert_eq!(ids_in_group, sorted, "{group} descending={descending}");
        }
        // Artist sort is case-insensitive: "ant" < "Bee" < "Cat".
        let artists: Vec<_> = all.iter().filter_map(|r| r.artist.as_deref()).collect();
        let mut expected = artists.clone();
        expected.sort_by_key(|a| a.to_lowercase());
        if descending {
            expected.reverse();
        }
        assert_eq!(artists, expected);
    }
    // Playlist paging follows playlist order.
    let p = create(&lib, "Paged");
    for id in ids.iter().rev() {
        add(&lib, p, *id, None);
    }
    let r = lib.reader().unwrap();
    let mut paged = Vec::new();
    for page in 0..3 {
        paged.extend(
            r.playlist_tracks(p, page * 10, 10)
                .unwrap()
                .into_iter()
                .map(|t| t.id),
        );
    }
    assert_eq!(paged, ids.iter().rev().copied().collect::<Vec<_>>());
}

#[test]
fn text_search_is_case_insensitive() {
    let (_d, lib) = open();
    add_tracks(
        &lib,
        &[
            ("/m/1", "Blue Monday", "New Order"),
            ("/m/2", "100% Pure", "Someone"),
            ("/m/3", "Under_score", "Other"),
            ("/m/4", "Plain", "BLUE Artist"),
            ("/m/5", "Back\\slash", "Other"),
        ],
    );
    let titles = |text: &str| -> Vec<String> {
        query(
            &lib,
            &TrackFilter {
                text: Some(text.into()),
                ..TrackFilter::default()
            },
            TrackSort {
                key: TrackSortKey::Title,
                descending: false,
            },
        )
        .into_iter()
        .filter_map(|r| r.title)
        .collect()
    };
    assert_eq!(titles("blue"), ["Blue Monday", "Plain"]);
    assert_eq!(titles("NEW ORDER"), ["Blue Monday"]);
    // Wildcards in user text are literal.
    assert_eq!(titles("%"), ["100% Pure"]);
    assert_eq!(titles("_"), ["Under_score"]);
    assert_eq!(titles("r_s"), ["Under_score"]);
    assert_eq!(titles("\\"), ["Back\\slash"]);
    assert_eq!(titles("zzz"), Vec::<String>::new());
}

#[test]
fn folder_and_missing_filters() {
    let (_d, lib) = open();
    let sep = std::path::MAIN_SEPARATOR;
    let p = |s: &str| s.replace('/', &sep.to_string());
    let ids = add_tracks(
        &lib,
        &[
            (&p("/music/a/1.flac"), "one", "x"),
            (&p("/music/a/deep/2.flac"), "two", "x"),
            (&p("/music/ab/3.flac"), "three", "x"),
            (&p("/music/b/4.flac"), "four", "x"),
        ],
    );
    submit(&lib, WriteOp::MarkMissing(vec![ids[1]]));
    let sort = TrackSort {
        key: TrackSortKey::Title,
        descending: false,
    };
    let titles = |filter: TrackFilter| -> Vec<String> {
        query(&lib, &filter, sort)
            .into_iter()
            .filter_map(|r| r.title)
            .collect()
    };
    let folder = |f: &str| TrackFilter {
        folder: Some(PathBuf::from(p(f))),
        ..TrackFilter::default()
    };
    assert_eq!(titles(folder("/music/a")), ["one", "two"]);
    assert_eq!(titles(folder("/music/a/")), ["one", "two"]);
    assert_eq!(titles(folder("/music")), ["four", "one", "three", "two"]);
    // Only the folder's own files, not its subfolders'.
    let own = |f: &str| TrackFilter {
        folder_only: true,
        ..folder(f)
    };
    assert_eq!(titles(own("/music/a")), ["one"]);
    assert_eq!(titles(own("/music/a/")), ["one"]);
    assert_eq!(titles(own("/music/a/deep")), ["two"]);
    assert_eq!(titles(own("/music")), Vec::<String>::new());
    let missing = |m| TrackFilter {
        missing: Some(m),
        ..TrackFilter::default()
    };
    assert_eq!(titles(missing(true)), ["two"]);
    assert_eq!(titles(missing(false)), ["four", "one", "three"]);
    assert_eq!(titles(TrackFilter::default()).len(), 4);
}

#[test]
fn rows_carry_analysis_and_sort_by_bpm_key_added() {
    let (_d, lib) = open();
    let ids = add_tracks(
        &lib,
        &[("/m/1", "a", "x"), ("/m/2", "b", "x"), ("/m/3", "c", "x")],
    );
    lib.writer()
        .submit_wait(WriteOp::Raw(vec![
            raw("INSERT INTO analysis(track_id, analyzer_version, bpm, key) VALUES (?1, 1, 128.0, 5)", vec![Value::Integer(ids[0])]),
            raw("INSERT INTO analysis(track_id, analyzer_version, bpm, key) VALUES (?1, 1, 90.5, 23)", vec![Value::Integer(ids[1])]),
            raw("UPDATE tracks SET added_at = id * 10", vec![]),
        ]))
        .unwrap();
    let by = |key, descending| -> Vec<i64> {
        query(&lib, &TrackFilter::default(), TrackSort { key, descending })
            .into_iter()
            .map(|r| r.id)
            .collect()
    };
    // NULLs sort first ascending.
    assert_eq!(by(TrackSortKey::Bpm, false), [ids[2], ids[1], ids[0]]);
    assert_eq!(by(TrackSortKey::Key, true), [ids[1], ids[0], ids[2]]);
    assert_eq!(by(TrackSortKey::Added, true), [ids[2], ids[1], ids[0]]);
    let rows = query(&lib, &TrackFilter::default(), TrackSort::default());
    let first = rows.iter().find(|r| r.id == ids[0]).unwrap();
    assert_eq!((first.bpm, first.key), (Some(128.0), Some(5)));
    let none = rows.iter().find(|r| r.id == ids[2]).unwrap();
    assert_eq!(
        (none.bpm, none.key, none.missing, none.has_cues),
        (None, None, false, false)
    );
}

/// Guards against table scans: runs in every build, with a bound loose enough for debug.
/// Times the first and a deep page for every sort key.
#[test]
fn queries_stay_fast_with_50k_tracks() {
    const N: i64 = 50_000;
    let (_d, lib) = open();
    // Seeded in one op, one transaction: varied artists/titles/added_at, and analysis rows
    // (varied bpm and key) for four tracks in five.
    let mut stmts: Vec<RawStatement> = (0..N)
        .map(|i| {
            raw(
                "INSERT INTO tracks(path, fingerprint, title, artist, added_at) \
                 VALUES (?1, x'00', ?2, ?3, ?4)",
                vec![
                    Value::Text(format!("/m/{i}.flac")),
                    Value::Text(format!("Title {:05}", (i * 31) % N)),
                    Value::Text(format!("artist {:05}", (i * 7919) % N)),
                    Value::Integer((i * 104_729) % 1_000_003),
                ],
            )
        })
        .collect();
    stmts.push(raw(
        "INSERT INTO analysis(track_id, analyzer_version, bpm, key) \
         SELECT id, 1, 80.0 + (id * 37) % 9000 / 100.0, id % 24 FROM tracks WHERE id % 5 != 0",
        vec![],
    ));
    lib.writer().submit_wait(WriteOp::Raw(stmts)).unwrap();

    let reader = lib.reader().unwrap();
    let mut timings = Vec::new();
    for key in [
        TrackSortKey::Title,
        TrackSortKey::Artist,
        TrackSortKey::Bpm,
        TrackSortKey::Key,
        TrackSortKey::Added,
    ] {
        for offset in [0, 49_800] {
            let sort = TrackSort {
                key,
                descending: false,
            };
            let start = Instant::now();
            let rows = reader
                .tracks(&TrackFilter::default(), sort, offset, 100)
                .unwrap();
            let elapsed = start.elapsed();
            timings.push(format!("{key:?}@{offset}: {elapsed:?}"));
            assert_eq!(rows.len(), 100, "{key:?}@{offset}");
            // bpm and key sort in a temp b-tree (analysis is a separate table), which is
            // much slower in unoptimized SQLite builds and on shared CI runners: give debug
            // builds headroom (they take about 190 ms locally; release keeps 250 ms).
            let bound =
                if cfg!(debug_assertions) && matches!(key, TrackSortKey::Bpm | TrackSortKey::Key) {
                    Duration::from_millis(2000)
                } else {
                    Duration::from_millis(250)
                };
            assert!(
                elapsed < bound,
                "{key:?} page at offset {offset} took {elapsed:?} (bound {bound:?})"
            );
        }
    }
    println!("{}", timings.join("\n"));
}

/// Title, artist and added sorts pick their page through an index: no temp b-tree sort
/// of the whole table.
#[test]
fn title_artist_added_sorts_use_indexes() {
    let (_d, lib) = open();
    n_tracks(&lib, 10);
    let reader = lib.reader().unwrap();
    for key in [
        TrackSortKey::Title,
        TrackSortKey::Artist,
        TrackSortKey::Added,
    ] {
        for descending in [false, true] {
            let plan = reader
                .tracks_query_plan(&TrackFilter::default(), TrackSort { key, descending })
                .unwrap();
            // The page itself is picked through the index; only the outer SELECT
            // re-sorts that one page (after "SCAN p").
            let page = plan
                .iter()
                .position(|l| l == "SCAN p")
                .unwrap_or(plan.len());
            assert!(
                !plan[..page].iter().any(|l| l.contains("TEMP B-TREE")),
                "{key:?} desc={descending}: {plan:?}"
            );
            assert!(
                plan.iter().any(|l| l.contains("USING INDEX tracks_")
                    || l.contains("USING COVERING INDEX tracks_")),
                "{key:?} desc={descending}: {plan:?}"
            );
        }
    }
}

#[test]
fn adding_unknown_track_is_a_typed_error() {
    let (_d, lib) = open();
    let p = create(&lib, "P");
    let err = lib
        .writer()
        .submit_wait(WriteOp::AddToPlaylist {
            playlist: p,
            track: 999,
            at: None,
        })
        .unwrap_err();
    assert!(matches!(err, LibraryError::NoSuchTrack(999)), "{err:?}");
    assert!(order(&lib, p).is_empty());
}

#[test]
fn playlist_edit_boundaries() {
    let (_d, lib) = open();
    let t = n_tracks(&lib, 3);
    let p = create(&lib, "B");
    for id in &t {
        add(&lib, p, *id, None);
    }
    // Remove the last entry.
    submit(
        &lib,
        WriteOp::RemoveFromPlaylist {
            playlist: p,
            position: 2,
        },
    );
    assert_eq!(order(&lib, p), [t[0], t[1]]);
    assert_eq!(positions(&lib, p), [0, 1]);
    // from == to is a no-op.
    submit(
        &lib,
        WriteOp::MoveInPlaylist {
            playlist: p,
            from: 1,
            to: 1,
        },
    );
    assert_eq!(order(&lib, p), [t[0], t[1]]);
    // Move a duplicate to the end (and past the end).
    add(&lib, p, t[0], None);
    assert_eq!(order(&lib, p), [t[0], t[1], t[0]]);
    submit(
        &lib,
        WriteOp::MoveInPlaylist {
            playlist: p,
            from: 0,
            to: 2,
        },
    );
    assert_eq!(order(&lib, p), [t[1], t[0], t[0]]);
    submit(
        &lib,
        WriteOp::MoveInPlaylist {
            playlist: p,
            from: 0,
            to: 99,
        },
    );
    assert_eq!(order(&lib, p), [t[0], t[0], t[1]]);
    assert_eq!(positions(&lib, p), [0, 1, 2]);
    // Move from out of range fails and changes nothing.
    let err = lib
        .writer()
        .submit_wait(WriteOp::MoveInPlaylist {
            playlist: p,
            from: 3,
            to: 0,
        })
        .unwrap_err();
    assert!(
        matches!(
            err,
            LibraryError::PositionOutOfRange {
                position: 3,
                len: 3
            }
        ),
        "{err:?}"
    );
    assert_eq!(order(&lib, p), [t[0], t[0], t[1]]);

    // MovePlaylist: unknown id errors; a position past the end clamps to last.
    let q = create(&lib, "Q");
    let r = create(&lib, "R");
    let err = lib
        .writer()
        .submit_wait(WriteOp::MovePlaylist {
            id: 9999,
            to_position: 0,
        })
        .unwrap_err();
    assert!(matches!(err, LibraryError::NoSuchPlaylist(9999)), "{err:?}");
    submit(
        &lib,
        WriteOp::MovePlaylist {
            id: p,
            to_position: 99,
        },
    );
    let ids: Vec<i64> = playlists(&lib).into_iter().map(|s| s.id).collect();
    assert_eq!(ids, [q, r, p]);
}

#[test]
fn create_playlist_with_tracks_in_one_op() {
    let (_dir, lib) = open();
    let ids = n_tracks(&lib, 3);
    let outcome = lib
        .writer()
        .submit_wait(WriteOp::CreatePlaylistWith {
            name: "Set".into(),
            tracks: vec![ids[2], ids[0], ids[2]],
        })
        .unwrap();
    let WriteOutcome::Created { id } = outcome else {
        panic!("unexpected outcome {outcome:?}");
    };
    assert_eq!(order(&lib, id), vec![ids[2], ids[0], ids[2]]);
    assert_eq!(playlists(&lib)[0].name, "Set");

    let err = lib
        .writer()
        .submit_wait(WriteOp::CreatePlaylistWith {
            name: "Bad".into(),
            tracks: vec![ids[0], 9999],
        })
        .unwrap_err();
    assert!(matches!(err, LibraryError::NoSuchTrack(9999)), "{err:?}");
    assert_eq!(playlists(&lib).len(), 1, "a failed create leaves nothing");
}

#[test]
fn expected_entries_guard_the_edit_in_the_same_transaction() {
    let (_d, lib) = open();
    let t = n_tracks(&lib, 3);
    let p = create(&lib, "Set");
    for id in [t[0], t[1], t[0], t[2]] {
        add(&lib, p, id, None);
    }
    // The entries hold: the removal after the check goes through.
    submit(
        &lib,
        WriteOp::Batch(vec![
            WriteOp::ExpectPlaylistEntries {
                playlist: p,
                entries: vec![(2, t[0]), (1, t[1])],
            },
            WriteOp::RemoveFromPlaylist {
                playlist: p,
                position: 2,
            },
        ]),
    );
    assert_eq!(order(&lib, p), [t[0], t[1], t[2]]);
    // Position 2 now holds another track (or none): nothing is written.
    for entries in [vec![(2, t[0])], vec![(7, t[2])]] {
        let err = lib
            .writer()
            .submit_wait(WriteOp::Batch(vec![
                WriteOp::ExpectPlaylistEntries {
                    playlist: p,
                    entries,
                },
                WriteOp::RemoveFromPlaylist {
                    playlist: p,
                    position: 0,
                },
            ]))
            .unwrap_err();
        assert!(
            matches!(err, LibraryError::PlaylistChanged(id) if id == p),
            "{err:?}"
        );
        assert_eq!(order(&lib, p), [t[0], t[1], t[2]]);
    }
}
