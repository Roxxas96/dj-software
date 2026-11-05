//! Remembered cue points.

use library::{Library, LibraryError, RawStatement, WriteOp};
use rusqlite::types::Value;

fn add_track(lib: &Library, id: i64) {
    lib.writer()
        .submit_wait(WriteOp::Raw(vec![RawStatement {
            sql: "INSERT INTO tracks(id, path, fingerprint, added_at) VALUES (?1, ?2, ?3, 0)"
                .into(),
            params: vec![
                Value::Integer(id),
                Value::Text(format!("/music/{id}.wav")),
                Value::Blob(vec![0; 32]),
            ],
        }]))
        .unwrap();
}

fn set(lib: &Library, track: i64, frames: f64) -> Result<(), LibraryError> {
    lib.writer()
        .submit_wait(WriteOp::SetCue { track, frames })
        .map(drop)
}

#[test]
fn cue_upsert_and_read() {
    let dir = tempfile::tempdir().unwrap();
    let lib = Library::open(&dir.path().join("library.sqlite3")).unwrap();
    add_track(&lib, 1);
    add_track(&lib, 2);
    assert_eq!(lib.cue(1).unwrap(), None);
    set(&lib, 1, 1234.5).unwrap();
    assert_eq!(lib.cue(1).unwrap(), Some(1234.5));
    set(&lib, 1, 0.0).unwrap();
    assert_eq!(lib.cue(1).unwrap(), Some(0.0));
    assert_eq!(lib.cue(2).unwrap(), None);
}

#[test]
fn cue_survives_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("library.sqlite3");
    {
        let lib = Library::open(&path).unwrap();
        add_track(&lib, 7);
        set(&lib, 7, 98_765.25).unwrap();
    }
    let lib = Library::open(&path).unwrap();
    assert_eq!(lib.cue(7).unwrap(), Some(98_765.25));
}

#[test]
fn invalid_cues_and_unknown_tracks_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let lib = Library::open(&dir.path().join("library.sqlite3")).unwrap();
    add_track(&lib, 1);
    set(&lib, 1, 10.0).unwrap();
    for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -1.0] {
        assert!(matches!(
            set(&lib, 1, bad),
            Err(LibraryError::InvalidCue(_))
        ));
    }
    assert!(matches!(
        set(&lib, 99, 5.0),
        Err(LibraryError::NoSuchTrack(99))
    ));
    assert_eq!(lib.cue(1).unwrap(), Some(10.0));
    assert_eq!(lib.cue(99).unwrap(), None);
}

fn hot_set(lib: &Library, track: i64, slot: u8, frames: f64) -> Result<(), LibraryError> {
    lib.writer()
        .submit_wait(WriteOp::SetHotcue {
            track,
            slot,
            frames,
        })
        .map(drop)
}

fn hot_delete(lib: &Library, track: i64, slot: u8) -> Result<(), LibraryError> {
    lib.writer()
        .submit_wait(WriteOp::DeleteHotcue { track, slot })
        .map(drop)
}

fn raw(lib: &Library, sql: &str) {
    lib.writer()
        .submit_wait(WriteOp::Raw(vec![RawStatement {
            sql: sql.into(),
            params: vec![],
        }]))
        .unwrap();
}

#[test]
fn hotcue_upsert_delete_and_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("library.sqlite3");
    {
        let lib = Library::open(&path).unwrap();
        add_track(&lib, 1);
        hot_set(&lib, 1, 1, 100.0).unwrap();
        hot_set(&lib, 1, 8, 800.0).unwrap();
        hot_set(&lib, 1, 1, 150.5).unwrap();
        hot_delete(&lib, 1, 8).unwrap();
        hot_delete(&lib, 1, 8).unwrap();
    }
    let lib = Library::open(&path).unwrap();
    let cues = lib.reader().unwrap().hotcues(1).unwrap();
    assert_eq!(cues[0], Some(150.5));
    assert_eq!(cues[7], None);
    assert!(cues[1..7].iter().all(Option::is_none));
}

#[test]
fn hotcue_update_keeps_color_and_name() {
    let dir = tempfile::tempdir().unwrap();
    let lib = Library::open(&dir.path().join("library.sqlite3")).unwrap();
    add_track(&lib, 1);
    raw(
        &lib,
        "INSERT INTO hotcues(track_id, slot, frames, color, name) VALUES (1, 3, 10, 255, 'drop')",
    );
    hot_set(&lib, 1, 3, 20.0).unwrap();
    let row: (f64, i64, String) = lib
        .reader()
        .unwrap()
        .connection()
        .query_row(
            "SELECT frames, color, name FROM hotcues WHERE track_id = 1 AND slot = 3",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(row, (20.0, 255, "drop".to_owned()));
}

#[test]
fn hotcue_rejects_bad_slot_and_frames() {
    let dir = tempfile::tempdir().unwrap();
    let lib = Library::open(&dir.path().join("library.sqlite3")).unwrap();
    add_track(&lib, 1);
    hot_set(&lib, 1, 2, 10.0).unwrap();
    for slot in [0, 9, 255] {
        assert!(matches!(
            hot_set(&lib, 1, slot, 5.0),
            Err(LibraryError::InvalidHotcueSlot(s)) if s == slot
        ));
        assert!(matches!(
            hot_delete(&lib, 1, slot),
            Err(LibraryError::InvalidHotcueSlot(s)) if s == slot
        ));
    }
    for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -1.0] {
        assert!(matches!(
            hot_set(&lib, 1, 2, bad),
            Err(LibraryError::InvalidCue(_))
        ));
    }
    assert!(matches!(
        hot_set(&lib, 99, 2, 5.0),
        Err(LibraryError::NoSuchTrack(99))
    ));
    assert!(matches!(
        hot_delete(&lib, 99, 2),
        Err(LibraryError::NoSuchTrack(99))
    ));
    let cues = lib.reader().unwrap().hotcues(1).unwrap();
    assert_eq!(cues.iter().flatten().count(), 1);
    assert_eq!(cues[1], Some(10.0));
    assert_eq!(lib.reader().unwrap().hotcues(99).unwrap(), [None; 8]);
}

#[test]
fn has_cues_counts_hotcues() {
    let dir = tempfile::tempdir().unwrap();
    let lib = Library::open(&dir.path().join("library.sqlite3")).unwrap();
    for id in 1..=4 {
        add_track(&lib, id);
    }
    set(&lib, 2, 0.0).unwrap();
    set(&lib, 3, 50.0).unwrap();
    hot_set(&lib, 4, 5, 70.0).unwrap();
    let rows = lib
        .reader()
        .unwrap()
        .tracks(&Default::default(), Default::default(), 0, 100)
        .unwrap();
    let mut has: Vec<(i64, bool)> = rows.iter().map(|r| (r.id, r.has_cues)).collect();
    has.sort();
    assert_eq!(has, [(1, false), (2, false), (3, true), (4, true)]);
}
