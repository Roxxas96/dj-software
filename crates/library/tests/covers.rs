//! Covers kept for tracks: found, missed and removed.

use std::path::PathBuf;

use library::{COVERS_RETRY_MISSED, CoverState, Library, LibraryError, RawStatement, WriteOp};
use rusqlite::{Connection, types::Value};

fn add_track(lib: &Library, id: i64, artist: Option<&str>, title: Option<&str>) {
    let text = |s: Option<&str>| s.map_or(Value::Null, |s| Value::Text(s.to_owned()));
    lib.writer()
        .submit_wait(WriteOp::Raw(vec![RawStatement {
            sql: "INSERT INTO tracks(id, path, fingerprint, artist, title, added_at) \
                  VALUES (?1, ?2, ?3, ?4, ?5, 0)"
                .into(),
            params: vec![
                Value::Integer(id),
                Value::Text(format!("/music/Folder/song {id}.wav")),
                Value::Blob(vec![0; 32]),
                text(artist),
                text(title),
            ],
        }]))
        .unwrap();
}

fn open(dir: &tempfile::TempDir) -> Library {
    Library::open(&dir.path().join("lib").join("library.sqlite3")).unwrap()
}

fn needing(lib: &Library) -> Vec<i64> {
    lib.reader()
        .unwrap()
        .tracks_needing_covers(100)
        .unwrap()
        .iter()
        .map(|q| q.track)
        .collect()
}

#[test]
fn set_and_read_a_cover() {
    let dir = tempfile::tempdir().unwrap();
    let lib = open(&dir);
    add_track(&lib, 1, Some("A"), Some("B"));
    assert_eq!(lib.reader().unwrap().cover(1).unwrap(), None);
    let file = PathBuf::from("/somewhere/1.jpg");
    lib.writer()
        .submit_wait(WriteOp::SetCover {
            track: 1,
            file: file.clone(),
        })
        .unwrap();
    let row = lib.reader().unwrap().cover(1).unwrap().unwrap();
    assert_eq!(row.state, CoverState::Found);
    assert_eq!(row.file, Some(file));
    let err = lib
        .writer()
        .submit_wait(WriteOp::SetCover {
            track: 9,
            file: "/x.jpg".into(),
        })
        .unwrap_err();
    assert!(matches!(err, LibraryError::NoSuchTrack(9)));
}

#[test]
fn a_miss_is_remembered() {
    let dir = tempfile::tempdir().unwrap();
    let lib = open(&dir);
    add_track(&lib, 1, None, None);
    lib.writer()
        .submit_wait(WriteOp::MissCover { track: 1 })
        .unwrap();
    let row = lib.reader().unwrap().cover(1).unwrap().unwrap();
    assert_eq!(row.state, CoverState::Missed);
    assert_eq!(row.file, None);
    assert!(needing(&lib).is_empty());
}

#[test]
fn removed_is_never_looked_up_again() {
    let dir = tempfile::tempdir().unwrap();
    let lib = open(&dir);
    add_track(&lib, 1, None, None);
    let covers = dir.path().join("lib").join("covers");
    std::fs::create_dir_all(&covers).unwrap();
    let own = covers.join("1.jpg");
    let other = covers.join("2.jpg");
    std::fs::write(&own, b"jpg").unwrap();
    std::fs::write(&other, b"jpg").unwrap();
    // A stored path elsewhere is never what gets deleted.
    let outside = dir.path().join("precious.jpg");
    std::fs::write(&outside, b"mine").unwrap();
    lib.writer()
        .submit_wait(WriteOp::SetCover {
            track: 1,
            file: outside.clone(),
        })
        .unwrap();
    lib.writer()
        .submit_wait(WriteOp::ClearCover { track: 1 })
        .unwrap();
    assert!(!own.exists());
    assert!(other.exists());
    assert!(outside.exists());
    let row = lib.reader().unwrap().cover(1).unwrap().unwrap();
    assert_eq!(row.state, CoverState::Removed);
    assert_eq!(row.file, None);
    // Not looked up, even with the retry flag, and a late miss does not undo it.
    lib.writer()
        .submit_wait(WriteOp::SetSetting {
            key: COVERS_RETRY_MISSED.into(),
            value: Some("1".into()),
        })
        .unwrap();
    lib.writer()
        .submit_wait(WriteOp::MissCover { track: 1 })
        .unwrap();
    assert!(needing(&lib).is_empty());
    assert_eq!(
        lib.reader().unwrap().cover(1).unwrap().unwrap().state,
        CoverState::Removed
    );
    // A missing file is fine.
    lib.writer()
        .submit_wait(WriteOp::ClearCover { track: 1 })
        .unwrap();
}

#[test]
fn tracks_needing_covers_skips_found_and_missed() {
    let dir = tempfile::tempdir().unwrap();
    let lib = open(&dir);
    for id in 1..=4 {
        add_track(&lib, id, Some("Artist"), Some("Title"));
    }
    add_track(&lib, 5, None, None);
    let w = lib.writer();
    w.submit_wait(WriteOp::SetCover {
        track: 1,
        file: "/c/1.jpg".into(),
    })
    .unwrap();
    w.submit_wait(WriteOp::MissCover { track: 2 }).unwrap();
    w.submit_wait(WriteOp::MarkMissing(vec![4])).unwrap();
    assert_eq!(needing(&lib), vec![3, 5]);
    let queries = lib.reader().unwrap().tracks_needing_covers(1).unwrap();
    assert_eq!(queries.len(), 1);
    assert_eq!(queries[0].track, 3);
    assert_eq!(queries[0].artist.as_deref(), Some("Artist"));
    assert_eq!(queries[0].title.as_deref(), Some("Title"));
    assert_eq!(queries[0].file_stem, "song 3");
    // With the retry flag, missed tracks are asked about again; found ones never.
    w.submit_wait(WriteOp::SetSetting {
        key: COVERS_RETRY_MISSED.into(),
        value: Some("1".into()),
    })
    .unwrap();
    assert_eq!(needing(&lib), vec![2, 3, 5]);
}

#[test]
fn a_version_3_library_opens_and_gains_the_table() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("library.sqlite3");
    {
        let conn = Connection::open(&path).unwrap();
        for sql in &library::schema::MIGRATIONS[..3] {
            conn.execute_batch(sql).unwrap();
        }
        conn.pragma_update(None, "user_version", 3).unwrap();
        conn.execute(
            "INSERT INTO tracks(id, path, fingerprint, added_at) VALUES (1, '/a.wav', x'00', 0)",
            [],
        )
        .unwrap();
    }
    let lib = Library::open(&path).unwrap();
    assert_eq!(lib.reader().unwrap().cover(1).unwrap(), None);
    assert_eq!(needing(&lib), vec![1]);
}
