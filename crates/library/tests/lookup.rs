//! Single-track lookups the app needs: a track with its file, its beatgrid, and which
//! tracks still need analysis.

use library::{ANALYZER_VERSION, GridEdit, Library, RawStatement, WriteOp};
use rusqlite::types::Value;

fn raw(lib: &Library, sql: &str, params: Vec<Value>) {
    lib.writer()
        .submit_wait(WriteOp::Raw(vec![RawStatement {
            sql: sql.into(),
            params,
        }]))
        .unwrap();
}

fn add_track(lib: &Library, id: i64, missing: bool) {
    raw(
        lib,
        "INSERT INTO tracks(id, path, fingerprint, added_at, missing, title, artist, \
         sample_rate, duration_frames) VALUES (?1, ?2, ?3, 0, ?4, 'T', 'A', 44100, 441000)",
        vec![
            Value::Integer(id),
            Value::Text(format!("/music/{id}.wav")),
            Value::Blob(vec![0; 32]),
            Value::Integer(i64::from(missing)),
        ],
    );
}

fn set_analysis(lib: &Library, id: i64, version: i64) {
    raw(
        lib,
        "INSERT INTO analysis(track_id, analyzer_version, bpm, first_beat_frames, key) \
         VALUES (?1, ?2, 128.0, 100.0, 9)",
        vec![Value::Integer(id), Value::Integer(version)],
    );
}

fn open() -> (tempfile::TempDir, Library) {
    let dir = tempfile::tempdir().unwrap();
    let lib = Library::open(&dir.path().join("library.sqlite3")).unwrap();
    (dir, lib)
}

#[test]
fn track_details_include_path_and_analysis() {
    let (_dir, lib) = open();
    add_track(&lib, 1, false);
    set_analysis(&lib, 1, 1);
    let reader = lib.reader().unwrap();
    let details = reader.track(1).unwrap().unwrap();
    assert_eq!(details.path, std::path::PathBuf::from("/music/1.wav"));
    assert_eq!(details.row.id, 1);
    assert_eq!(details.row.title.as_deref(), Some("T"));
    assert_eq!(details.row.bpm, Some(128.0));
    assert_eq!(details.row.key, Some(9));
    assert_eq!(details.row.sample_rate, Some(44_100));
    assert!(!details.row.missing);
    assert_eq!(reader.track(2).unwrap(), None);
}

#[test]
fn beatgrid_reads_grid_columns() {
    let (_dir, lib) = open();
    add_track(&lib, 1, false);
    add_track(&lib, 2, false);
    set_analysis(&lib, 1, 1);
    let reader = lib.reader().unwrap();
    let grid = reader.beatgrid(1).unwrap().unwrap();
    assert_eq!(grid.bpm, Some(128.0));
    assert_eq!(grid.first_beat_frames, Some(100.0));
    assert!(!grid.grid_locked);
    assert_eq!(reader.beatgrid(2).unwrap(), None);
    lib.edit_grid(1, GridEdit::ToggleLock, None).unwrap();
    assert!(reader.beatgrid(1).unwrap().unwrap().grid_locked);
}

#[test]
fn track_ids_to_analyze_skips_current_and_missing() {
    let (_dir, lib) = open();
    for id in 1..=4 {
        add_track(&lib, id, id == 4);
    }
    set_analysis(&lib, 1, i64::from(ANALYZER_VERSION));
    set_analysis(&lib, 2, 0);
    let reader = lib.reader().unwrap();
    assert_eq!(reader.track_ids_to_analyze(false).unwrap(), vec![2, 3]);
    assert_eq!(reader.track_ids_to_analyze(true).unwrap(), vec![1, 2, 3]);
}

#[test]
fn track_position_follows_the_sort() {
    use library::{TrackFilter, TrackSort, TrackSortKey};
    let (_dir, lib) = open();
    // Artists: 1 "b", 2 NULL, 3 "A", 4 "b", 5 "c" (missing, filtered out below).
    for (id, artist) in [
        (1, Some("b")),
        (2, None),
        (3, Some("A")),
        (4, Some("b")),
        (5, Some("c")),
    ] {
        add_track(&lib, id, id == 5);
        raw(
            &lib,
            "UPDATE tracks SET artist = ?2, title = ?3 WHERE id = ?1",
            vec![
                Value::Integer(id),
                artist.map_or(Value::Null, |a| Value::Text(a.into())),
                Value::Text(format!("t{}", 6 - id)),
            ],
        );
    }
    let reader = lib.reader().unwrap();
    let all = TrackFilter::default();
    for key in [
        TrackSortKey::Artist,
        TrackSortKey::Title,
        TrackSortKey::Bpm,
        TrackSortKey::Added,
    ] {
        for descending in [false, true] {
            let sort = TrackSort { key, descending };
            let rows = reader.tracks(&all, sort, 0, 100).unwrap();
            for (i, row) in rows.iter().enumerate() {
                assert_eq!(
                    reader.track_position(&all, sort, row.id).unwrap(),
                    Some(i),
                    "{key:?} desc={descending} track {}",
                    row.id
                );
            }
        }
    }
    let present = TrackFilter {
        missing: Some(false),
        ..TrackFilter::default()
    };
    assert_eq!(
        reader
            .track_position(&present, TrackSort::default(), 5)
            .unwrap(),
        None
    );
    assert_eq!(
        reader
            .track_position(&all, TrackSort::default(), 99)
            .unwrap(),
        None
    );
}
