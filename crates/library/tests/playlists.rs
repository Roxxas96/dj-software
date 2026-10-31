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
#[test]
fn queries_stay_fast_with_50k_tracks() {
    let (_d, lib) = open();
    // One op, one transaction.
    let stmts: Vec<RawStatement> = (0..50_000)
        .map(|i| {
            insert_track_stmt(
                &format!("/m/{i}.flac"),
                &format!("title {i}"),
                &format!("artist {:05}", (i * 7919) % 50_000),
            )
        })
        .collect();
    lib.writer().submit_wait(WriteOp::Raw(stmts)).unwrap();

    let reader = lib.reader().unwrap();
    let sort = TrackSort {
        key: TrackSortKey::Artist,
        descending: false,
    };
    let start = Instant::now();
    let rows = reader
        .tracks(&TrackFilter::default(), sort, 25_000, 100)
        .unwrap();
    let elapsed = start.elapsed();
    assert_eq!(rows.len(), 100);
    assert_eq!(rows[0].artist.as_deref(), Some("artist 25000"));
    assert!(
        elapsed < Duration::from_millis(250),
        "paged artist query took {elapsed:?}"
    );
}
