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
            force: false,
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
            force: false,
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
            force: false,
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
        force: false,
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

#[test]
fn a_removed_cover_stays_removed_unless_forced() {
    let dir = tempfile::tempdir().unwrap();
    let lib = open(&dir);
    add_track(&lib, 1, None, None);
    let covers = dir.path().join("lib").join("covers");
    std::fs::create_dir_all(&covers).unwrap();
    let own = covers.join("1.jpg");
    let w = lib.writer();
    w.submit_wait(WriteOp::ClearCover { track: 1 }).unwrap();
    // A lookup that finished after the removal: refused, and its new file goes.
    std::fs::write(&own, b"jpg").unwrap();
    w.submit_wait(WriteOp::SetCover {
        track: 1,
        file: own.clone(),
        force: false,
    })
    .unwrap();
    let row = lib.reader().unwrap().cover(1).unwrap().unwrap();
    assert_eq!(row.state, CoverState::Removed);
    assert!(!own.exists());
    // A refused file elsewhere is left alone.
    let elsewhere = dir.path().join("keep.jpg");
    std::fs::write(&elsewhere, b"jpg").unwrap();
    w.submit_wait(WriteOp::SetCover {
        track: 1,
        file: elsewhere.clone(),
        force: false,
    })
    .unwrap();
    assert!(elsewhere.exists());
    assert_eq!(
        lib.reader().unwrap().cover(1).unwrap().unwrap().state,
        CoverState::Removed
    );
    // "Find another cover" forces it.
    std::fs::write(&own, b"jpg").unwrap();
    w.submit_wait(WriteOp::SetCover {
        track: 1,
        file: own.clone(),
        force: true,
    })
    .unwrap();
    let row = lib.reader().unwrap().cover(1).unwrap().unwrap();
    assert_eq!(row.state, CoverState::Found);
    assert_eq!(row.file, Some(own.clone()));
    assert!(own.exists());
    // Without force a found or missed cover is replaced as before.
    w.submit_wait(WriteOp::SetCover {
        track: 1,
        file: elsewhere.clone(),
        force: false,
    })
    .unwrap();
    assert_eq!(
        lib.reader().unwrap().cover(1).unwrap().unwrap().file,
        Some(elsewhere)
    );
}

#[test]
fn a_new_cover_takes_the_other_file_and_the_old_one_goes() {
    let dir = tempfile::tempdir().unwrap();
    let lib = open(&dir);
    add_track(&lib, 1, Some("A"), Some("T"));
    let [first, second] = library::cover_files(lib.path(), 1);
    assert_eq!(first, dir.path().join("lib").join("covers").join("1.jpg"));
    assert_eq!(
        second,
        dir.path().join("lib").join("covers").join("1-2.jpg")
    );
    std::fs::create_dir_all(first.parent().unwrap()).unwrap();
    std::fs::write(&first, b"one").unwrap();
    let w = lib.writer();
    w.submit_wait(WriteOp::SetCover {
        track: 1,
        file: first.clone(),
        force: false,
    })
    .unwrap();
    std::fs::write(&second, b"two").unwrap();
    w.submit_wait(WriteOp::SetCover {
        track: 1,
        file: second.clone(),
        force: true,
    })
    .unwrap();
    assert!(!first.exists());
    assert!(second.exists());
    // Removing deletes either file.
    w.submit_wait(WriteOp::ClearCover { track: 1 }).unwrap();
    assert!(!second.exists());
}

#[test]
fn lookups_walk_the_library_and_ask_for_one_track() {
    let dir = tempfile::tempdir().unwrap();
    let lib = open(&dir);
    for id in 1..=3 {
        add_track(&lib, id, Some("A"), Some("T"));
    }
    let reader = lib.reader().unwrap();
    let after: Vec<i64> = reader
        .tracks_needing_covers_after(1, 10)
        .unwrap()
        .iter()
        .map(|q| q.track)
        .collect();
    assert_eq!(after, [2, 3]);
    lib.writer()
        .submit_wait(WriteOp::ClearCover { track: 2 })
        .unwrap();
    // Asked for by id whatever its cover's state, with the file's stem only.
    let query = reader.cover_query(2).unwrap().unwrap();
    assert_eq!(query.file_stem, "song 2");
    assert_eq!(query.artist.as_deref(), Some("A"));
    assert_eq!(reader.cover_query(99).unwrap(), None);
}
