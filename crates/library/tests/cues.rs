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
