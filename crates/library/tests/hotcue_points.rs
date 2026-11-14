//! Hotcue slots with a loop length, a colour and a name; track lookups; forced backups.

use library::{HotcuePoint, Library, LibraryError, RawStatement, WriteOp};
use rusqlite::Connection;
use rusqlite::types::Value;

fn add_track(lib: &Library, id: i64, path: &str, fingerprint: u8, size: i64) {
    lib.writer()
        .submit_wait(WriteOp::Raw(vec![RawStatement {
            sql: "INSERT INTO tracks(id, path, fingerprint, size, added_at) \
                  VALUES (?1, ?2, ?3, ?4, 0)"
                .into(),
            params: vec![
                Value::Integer(id),
                Value::Text(path.into()),
                Value::Blob(vec![fingerprint; 32]),
                Value::Integer(size),
            ],
        }]))
        .unwrap();
}

fn open_with_track() -> (tempfile::TempDir, Library) {
    let dir = tempfile::tempdir().unwrap();
    let lib = Library::open(&dir.path().join("library.sqlite3")).unwrap();
    add_track(&lib, 1, "/music/a.mp3", 1, 1000);
    (dir, lib)
}

fn drop_point() -> HotcuePoint {
    HotcuePoint {
        frames: 5_664_000.0,
        loop_frames: Some(219_000.0),
        color: Some(0xFF0000),
        name: Some("Drop".into()),
    }
}

fn put_drop(lib: &Library) {
    lib.writer()
        .submit_wait(WriteOp::PutHotcue {
            track: 1,
            slot: 4,
            point: drop_point(),
        })
        .unwrap();
}

#[test]
fn put_hotcue_round_trips_every_field() {
    let (_dir, lib) = open_with_track();
    put_drop(&lib);
    let reader = lib.reader().unwrap();
    assert_eq!(reader.hotcue_points(1).unwrap()[3], Some(drop_point()));
    assert_eq!(reader.hotcues(1).unwrap()[3], Some(5_664_000.0));
}

#[test]
fn set_hotcue_keeps_colour_and_name_and_sets_the_loop() {
    let (_dir, lib) = open_with_track();
    put_drop(&lib);
    lib.writer()
        .submit_wait(WriteOp::SetHotcue {
            track: 1,
            slot: 4,
            frames: 100.0,
            loop_frames: None,
        })
        .unwrap();
    assert_eq!(
        lib.reader().unwrap().hotcue_points(1).unwrap()[3],
        Some(HotcuePoint {
            frames: 100.0,
            loop_frames: None,
            color: Some(0xFF0000),
            name: Some("Drop".into()),
        })
    );
    lib.writer()
        .submit_wait(WriteOp::SetHotcue {
            track: 1,
            slot: 4,
            frames: 100.0,
            loop_frames: Some(50.0),
        })
        .unwrap();
    let points = lib.reader().unwrap().hotcue_points(1).unwrap();
    assert_eq!(points[3].as_ref().unwrap().loop_frames, Some(50.0));
}

#[test]
fn bad_loops_are_refused() {
    let (_dir, lib) = open_with_track();
    for bad in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        let set = lib.writer().submit_wait(WriteOp::SetHotcue {
            track: 1,
            slot: 2,
            frames: 10.0,
            loop_frames: Some(bad),
        });
        assert!(matches!(set, Err(LibraryError::InvalidLoop(_))), "{bad}");
        let put = lib.writer().submit_wait(WriteOp::PutHotcue {
            track: 1,
            slot: 2,
            point: HotcuePoint {
                frames: 10.0,
                loop_frames: Some(bad),
                color: None,
                name: None,
            },
        });
        assert!(matches!(put, Err(LibraryError::InvalidLoop(_))), "{bad}");
    }
    assert_eq!(
        lib.reader().unwrap().hotcue_points(1).unwrap(),
        [const { None }; 8]
    );
}

#[test]
fn a_v2_database_migrates_keeping_hotcues() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("library.sqlite3");
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE tracks (
                id INTEGER PRIMARY KEY, path TEXT UNIQUE NOT NULL, fingerprint BLOB NOT NULL,
                size INTEGER, mtime INTEGER, missing INTEGER NOT NULL DEFAULT 0, title TEXT,
                artist TEXT, album TEXT, genre TEXT, duration_frames INTEGER,
                sample_rate INTEGER, added_at INTEGER NOT NULL,
                last_loaded_at INTEGER, stems_prepared INTEGER NOT NULL DEFAULT 0);
             CREATE TABLE hotcues (
                track_id INTEGER NOT NULL REFERENCES tracks(id),
                slot INTEGER NOT NULL CHECK (slot BETWEEN 1 AND 8),
                frames REAL NOT NULL, color INTEGER, name TEXT,
                PRIMARY KEY (track_id, slot));
             INSERT INTO tracks(id, path, fingerprint, added_at) VALUES (1, '/m/a.wav', x'00', 0);
             INSERT INTO hotcues(track_id, slot, frames, color, name)
                 VALUES (1, 2, 321.0, 255, 'x');
             PRAGMA user_version = 2;",
        )
        .unwrap();
    }
    let lib = Library::open(&path).unwrap();
    let version: i64 = lib
        .reader()
        .unwrap()
        .connection()
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .unwrap();
    assert_eq!(version, library::schema::MIGRATIONS.len() as i64);
    assert_eq!(
        lib.reader().unwrap().hotcue_points(1).unwrap()[1],
        Some(HotcuePoint {
            frames: 321.0,
            loop_frames: None,
            color: Some(255),
            name: Some("x".into()),
        })
    );
}

#[test]
fn tracks_are_found_by_path_fingerprint_and_name_size() {
    let (_dir, lib) = open_with_track();
    add_track(&lib, 2, "/other/b.mp3", 2, 2000);
    add_track(&lib, 3, "/third/B.mp3", 3, 2000);
    let reader = lib.reader().unwrap();
    let path = std::path::Path::new;
    assert_eq!(
        reader.track_id_by_path(path("/music/a.mp3")).unwrap(),
        Some(1)
    );
    assert_eq!(reader.track_id_by_path(path("/MUSIC/a.mp3")).unwrap(), None);
    assert_eq!(reader.track_ids_by_fingerprint(&[2; 32]).unwrap(), vec![2]);
    assert_eq!(
        reader.track_ids_by_name_and_size("b.mp3", 2000).unwrap(),
        vec![2]
    );
    assert!(
        reader
            .track_ids_by_name_and_size("b.mp3", 1)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn backup_now_writes_a_backup_even_right_after_another() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("library.sqlite3");
    drop(Library::open(&path).unwrap());
    let lib = Library::open(&path).unwrap();
    let count = || {
        std::fs::read_dir(dir.path().join("backups"))
            .unwrap()
            .count()
    };
    assert_eq!(count(), 1);
    let backup = lib.backup_now().unwrap();
    assert!(backup.exists());
    assert_eq!(count(), 2);
}

#[test]
fn set_hotcue_label_updates_only_name_and_colour() {
    let (_dir, lib) = open_with_track();
    put_drop(&lib);
    lib.writer()
        .submit_wait(WriteOp::SetHotcueLabel {
            track: 1,
            slot: 4,
            name: Some("Break".into()),
            color: Some(0x00CC00),
        })
        .unwrap();
    let relabelled = HotcuePoint {
        color: Some(0x00CC00),
        name: Some("Break".into()),
        ..drop_point()
    };
    let reader = lib.reader().unwrap();
    assert_eq!(reader.hotcue_points(1).unwrap()[3], Some(relabelled));
    // No name and no colour clear both; the position and the loop stay.
    lib.writer()
        .submit_wait(WriteOp::SetHotcueLabel {
            track: 1,
            slot: 4,
            name: None,
            color: None,
        })
        .unwrap();
    let cleared = HotcuePoint {
        color: None,
        name: None,
        ..drop_point()
    };
    assert_eq!(reader.hotcue_points(1).unwrap()[3], Some(cleared));
}

#[test]
fn label_on_an_empty_slot_is_refused() {
    let (_dir, lib) = open_with_track();
    let label = |track, slot| WriteOp::SetHotcueLabel {
        track,
        slot,
        name: Some("Intro".into()),
        color: Some(0xCC0000),
    };
    let error = lib.writer().submit_wait(label(1, 2)).unwrap_err();
    assert!(
        matches!(error, LibraryError::NoSuchHotcue { track: 1, slot: 2 }),
        "{error:?}"
    );
    assert_eq!(lib.reader().unwrap().hotcue_points(1).unwrap()[1], None);
    let error = lib.writer().submit_wait(label(1, 9)).unwrap_err();
    assert!(
        matches!(error, LibraryError::InvalidHotcueSlot(9)),
        "{error:?}"
    );
    let error = lib.writer().submit_wait(label(5, 2)).unwrap_err();
    assert!(matches!(error, LibraryError::NoSuchTrack(5)), "{error:?}");
}
