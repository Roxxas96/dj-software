//! Library database: schema, migrations, startup backups and the writer thread.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use library::{LibraryError, RawStatement, WriteOp, WriteOutcome};
use rusqlite::{Connection, types::Value};

fn db_path(dir: &tempfile::TempDir) -> PathBuf {
    dir.path().join("nested").join("library.sqlite3")
}

fn user_version(conn: &Connection) -> i64 {
    conn.pragma_query_value(None, "user_version", |r| r.get(0))
        .unwrap()
}

fn table_exists(conn: &Connection, name: &str) -> bool {
    conn.query_row(
        "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
        [name],
        |r| r.get::<_, i64>(0),
    )
    .unwrap()
        == 1
}

fn count(conn: &Connection, table: &str) -> i64 {
    conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
        .unwrap()
}

fn insert_setting(key: &str, value: &str) -> WriteOp {
    WriteOp::Raw(vec![RawStatement {
        sql: "INSERT INTO settings(key, value) VALUES (?1, ?2)".into(),
        params: vec![Value::Text(key.into()), Value::Text(value.into())],
    }])
}

fn backups(path: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(path.parent().unwrap().join("backups")) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries.map(|e| e.unwrap().path()).collect();
    files.sort();
    files
}

/// The backup with the latest modification time.
fn newest_backup(path: &Path) -> PathBuf {
    backups(path)
        .into_iter()
        .max_by_key(|f| std::fs::metadata(f).unwrap().modified().unwrap())
        .unwrap()
}

/// Makes every existing backup look an hour old, so the next open takes a new one.
fn age_backups(path: &Path) {
    for f in backups(path) {
        std::fs::File::options()
            .write(true)
            .open(&f)
            .unwrap()
            .set_modified(SystemTime::now() - Duration::from_secs(3600))
            .unwrap();
    }
}

fn backup_count(path: &Path) -> usize {
    backups(path).len()
}

#[test]
fn open_creates_schema_v1() {
    let dir = tempfile::tempdir().unwrap();
    let path = db_path(&dir);
    let lib = library::Library::open(&path).unwrap();
    let reader = lib.reader().unwrap();
    let conn = reader.connection();
    assert_eq!(user_version(conn), 1);
    for table in [
        "tracks",
        "folders",
        "analysis",
        "cues",
        "hotcues",
        "playlists",
        "playlist_tracks",
        "settings",
    ] {
        assert!(table_exists(conn, table), "missing table {table}");
    }
    let journal: String = conn
        .pragma_query_value(None, "journal_mode", |r| r.get(0))
        .unwrap();
    assert_eq!(journal, "wal");
    // A brand-new library has nothing to back up.
    assert_eq!(backup_count(&path), 0);
}

#[test]
fn open_backs_up_existing_db_and_keeps_ten() {
    let dir = tempfile::tempdir().unwrap();
    let path = db_path(&dir);
    // Open k (1-based) backs up a file holding k - 1 settings rows.
    for k in 1..=12 {
        age_backups(&path);
        let lib = library::Library::open(&path).unwrap();
        lib.writer()
            .submit_wait(insert_setting(&format!("k{k}"), "v"))
            .unwrap();
    }
    let files = backups(&path);
    assert_eq!(files.len(), 10, "{files:?}");
    // Backups made within the same second get a counter suffix, so sort by contents to
    // check which ones survived: the 11 backups held 1..=11 rows, the oldest one is gone.
    let mut rows: Vec<i64> = files
        .iter()
        .map(|f| count(&Connection::open(f).unwrap(), "settings"))
        .collect();
    rows.sort_unstable();
    assert_eq!(rows, (2..=11).collect::<Vec<_>>());
}

#[test]
fn backup_names_are_utc_timestamps() {
    let dir = tempfile::tempdir().unwrap();
    let path = db_path(&dir);
    drop(library::Library::open(&path).unwrap());
    drop(library::Library::open(&path).unwrap());
    let files = backups(&path);
    assert_eq!(files.len(), 1);
    let name = files[0].file_name().unwrap().to_str().unwrap().to_owned();
    // library-yyyymmdd-hhmmss.sqlite3
    let stamp = name
        .strip_prefix("library-")
        .and_then(|s| s.strip_suffix(".sqlite3"))
        .unwrap();
    let (date, time) = stamp.split_once('-').unwrap();
    assert_eq!(date.len(), 8, "{name}");
    assert_eq!(time.len(), 6, "{name}");
    assert!(date.chars().chain(time.chars()).all(|c| c.is_ascii_digit()));
}

#[test]
fn failed_migration_leaves_previous_version() {
    let dir = tempfile::tempdir().unwrap();
    let path = db_path(&dir);
    {
        let lib = library::Library::open(&path).unwrap();
        lib.writer()
            .submit_wait(insert_setting("keep", "me"))
            .unwrap();
    }
    // A recent backup exists, which would normally make the next open skip its backup.
    drop(library::Library::open(&path).unwrap());
    assert_eq!(backup_count(&path), 1);
    let broken: &[&str] = &[
        library::schema::MIGRATIONS[0],
        "CREATE TABLE half_done(a); INSERT INTO no_such_table VALUES (1);",
    ];
    let err = library::Library::open_with_migrations(&path, broken)
        .expect_err("broken migration must fail");
    assert!(
        matches!(err, LibraryError::Migration { version: 2, .. }),
        "{err:?}"
    );

    let conn = Connection::open(&path).unwrap();
    assert_eq!(user_version(&conn), 1);
    assert!(!table_exists(&conn, "half_done"));
    let value: String = conn
        .query_row("SELECT value FROM settings WHERE key = 'keep'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(value, "me");

    // The pending migration forced a backup, taken before the migration ran.
    assert_eq!(backup_count(&path), 2);
    let backup = Connection::open(newest_backup(&path)).unwrap();
    assert_eq!(user_version(&backup), 1);
    assert!(!table_exists(&backup, "half_done"));
    assert_eq!(count(&backup, "settings"), 1);
}

#[test]
fn second_open_within_ten_minutes_makes_no_backup() {
    let dir = tempfile::tempdir().unwrap();
    let path = db_path(&dir);
    drop(library::Library::open(&path).unwrap());
    assert_eq!(backup_count(&path), 0, "nothing to back up yet");
    drop(library::Library::open(&path).unwrap());
    assert_eq!(backup_count(&path), 1);
    for _ in 0..5 {
        drop(library::Library::open(&path).unwrap());
    }
    assert_eq!(
        backup_count(&path),
        1,
        "recent backup: opens must not rotate"
    );
    age_backups(&path);
    drop(library::Library::open(&path).unwrap());
    assert_eq!(
        backup_count(&path),
        2,
        "older than ten minutes: back up again"
    );
}

#[test]
fn pending_migration_forces_a_backup_of_the_old_version() {
    let dir = tempfile::tempdir().unwrap();
    let path = db_path(&dir);
    {
        let lib = library::Library::open(&path).unwrap();
        lib.writer().submit_wait(insert_setting("a", "1")).unwrap();
    }
    drop(library::Library::open(&path).unwrap());
    assert_eq!(backup_count(&path), 1);

    let v2: &[&str] = &[library::schema::MIGRATIONS[0], "CREATE TABLE extra(a);"];
    let lib = library::Library::open_with_migrations(&path, v2).unwrap();
    assert_eq!(backup_count(&path), 2);
    let backup = Connection::open(newest_backup(&path)).unwrap();
    assert_eq!(user_version(&backup), 1);
    assert!(!table_exists(&backup, "extra"));
    assert_eq!(count(&backup, "settings"), 1);
    assert_eq!(user_version(lib.reader().unwrap().connection()), 2);
}

#[test]
fn backup_succeeds_while_another_connection_is_writing() {
    let dir = tempfile::tempdir().unwrap();
    let path = db_path(&dir);
    drop(library::Library::open(&path).unwrap());
    let other = Connection::open(&path).unwrap();
    other
        .execute_batch("BEGIN IMMEDIATE; INSERT INTO settings VALUES ('pending', 'x');")
        .unwrap();
    // The open takes a backup and starts while the other write transaction is open.
    let lib = library::Library::open(&path).unwrap();
    assert_eq!(backup_count(&path), 1);
    let backup = Connection::open(newest_backup(&path)).unwrap();
    assert_eq!(
        count(&backup, "settings"),
        0,
        "uncommitted data is not in the backup"
    );
    other.execute_batch("COMMIT").unwrap();
    assert_eq!(count(lib.reader().unwrap().connection(), "settings"), 1);
}

#[test]
fn failed_submit_is_reported() {
    let dir = tempfile::tempdir().unwrap();
    let lib = library::Library::open(&db_path(&dir)).unwrap();
    let writer = lib.writer();
    let bad = WriteOp::Raw(vec![RawStatement {
        sql: "INSERT INTO no_such_table VALUES (1)".into(),
        params: vec![],
    }]);
    writer.submit(bad.clone()).unwrap();
    writer.submit(insert_setting("ok", "1")).unwrap();
    // Waited ops report their own error and are not added to the failure list.
    assert!(writer.submit_wait(bad.clone()).is_err());

    assert_eq!(writer.failed_write_count(), 1);
    assert_eq!(lib.failed_write_count(), 1);
    let failed = lib.take_failed_writes();
    assert_eq!(failed.len(), 1);
    assert_eq!(failed[0].op, bad);
    assert!(
        matches!(failed[0].error, LibraryError::Sqlite(_)),
        "{:?}",
        failed[0].error
    );
    // Taken: the list is empty, the counter keeps counting.
    assert!(writer.take_failed_writes().is_empty());
    assert_eq!(writer.failed_write_count(), 1);

    // A panicking op queued without waiting is reported too.
    writer
        .submit(WriteOp::PanicAfter {
            statements: vec![],
            after: 0,
        })
        .unwrap();
    writer.submit_wait(insert_setting("ok2", "1")).unwrap();
    let failed = writer.take_failed_writes();
    assert_eq!(failed.len(), 1);
    assert!(matches!(failed[0].error, LibraryError::WriterPanicked));
    assert_eq!(writer.failed_write_count(), 2);
}

#[test]
fn submits_racing_shutdown_are_applied_or_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = db_path(&dir);
    let lib = library::Library::open(&path).unwrap();
    let workers: Vec<_> = (0..4)
        .map(|t| {
            let writer = lib.writer();
            std::thread::spawn(move || {
                let mut accepted = 0_i64;
                for i in 0.. {
                    match writer.submit(insert_setting(&format!("t{t}-{i}"), "v")) {
                        Ok(()) => accepted += 1,
                        Err(LibraryError::WriterGone) => break,
                        Err(e) => panic!("{e:?}"),
                    }
                }
                accepted
            })
        })
        .collect();
    std::thread::sleep(Duration::from_millis(50));
    drop(lib);
    let accepted: i64 = workers.into_iter().map(|w| w.join().unwrap()).sum();
    assert!(accepted > 0);
    let conn = Connection::open(&path).unwrap();
    assert_eq!(
        count(&conn, "settings"),
        accepted,
        "an accepted submit was lost"
    );
}

#[test]
fn newer_database_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = db_path(&dir);
    drop(library::Library::open(&path).unwrap());
    Connection::open(&path)
        .unwrap()
        .pragma_update(None, "user_version", 99)
        .unwrap();
    let err = library::Library::open(&path).unwrap_err();
    assert!(
        matches!(
            err,
            LibraryError::TooNew {
                found: 99,
                supported: 1
            }
        ),
        "{err:?}"
    );
    assert_eq!(user_version(&Connection::open(&path).unwrap()), 99);
}

#[test]
fn writer_applies_ops_in_order() {
    let dir = tempfile::tempdir().unwrap();
    let lib = library::Library::open(&db_path(&dir)).unwrap();
    let writer = lib.writer();
    for i in 0..200_i64 {
        writer
            .submit(WriteOp::Raw(vec![RawStatement {
                sql: "INSERT INTO playlists(name, position) VALUES (?1, ?2)".into(),
                params: vec![Value::Text(format!("p{i}")), Value::Integer(i)],
            }]))
            .unwrap();
    }
    // A waited op is queued behind the others, so once it commits they all have.
    writer.submit_wait(insert_setting("done", "1")).unwrap();

    let reader = lib.reader().unwrap();
    let mut stmt = reader
        .connection()
        .prepare("SELECT id, name, position FROM playlists ORDER BY id")
        .unwrap();
    let rows: Vec<(i64, String, i64)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(rows.len(), 200);
    for (n, (id, name, position)) in rows.into_iter().enumerate() {
        let n = i64::try_from(n).unwrap();
        assert_eq!(id, n + 1);
        assert_eq!(position, n);
        assert_eq!(name, format!("p{n}"));
    }
}

#[test]
fn submit_wait_reports_changes() {
    let dir = tempfile::tempdir().unwrap();
    let lib = library::Library::open(&db_path(&dir)).unwrap();
    let outcome = lib
        .writer()
        .submit_wait(WriteOp::SetSetting {
            key: "theme".into(),
            value: Some("dark".into()),
        })
        .unwrap();
    assert_eq!(outcome, WriteOutcome::Done);
    lib.writer()
        .submit_wait(WriteOp::SetSetting {
            key: "theme".into(),
            value: Some("light".into()),
        })
        .unwrap();
    let reader = lib.reader().unwrap();
    let value: String = reader
        .connection()
        .query_row("SELECT value FROM settings WHERE key = 'theme'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(value, "light");
    lib.writer()
        .submit_wait(WriteOp::SetSetting {
            key: "theme".into(),
            value: None,
        })
        .unwrap();
    assert_eq!(count(reader.connection(), "settings"), 0);
}

#[test]
fn failing_op_rolls_back_and_writer_keeps_going() {
    let dir = tempfile::tempdir().unwrap();
    let lib = library::Library::open(&db_path(&dir)).unwrap();
    let writer = lib.writer();
    let op = WriteOp::Raw(vec![
        RawStatement {
            sql: "INSERT INTO settings(key, value) VALUES ('a', '1')".into(),
            params: vec![],
        },
        RawStatement {
            sql: "INSERT INTO settings(key, value) VALUES ('a', 'duplicate')".into(),
            params: vec![],
        },
    ]);
    let err = writer.submit_wait(op).unwrap_err();
    assert!(matches!(err, LibraryError::Sqlite(_)), "{err:?}");
    writer.submit_wait(insert_setting("b", "2")).unwrap();
    let reader = lib.reader().unwrap();
    assert_eq!(count(reader.connection(), "settings"), 1);
}

#[test]
fn writes_are_atomic_across_crash() {
    let dir = tempfile::tempdir().unwrap();
    let path = db_path(&dir);
    {
        let lib = library::Library::open(&path).unwrap();
        let statements: Vec<RawStatement> = (0..5)
            .map(|i| RawStatement {
                sql: "INSERT INTO playlists(name, position) VALUES ('x', ?1)".into(),
                params: vec![Value::Integer(i)],
            })
            .collect();
        // The hook panics inside the transaction after three of the five inserts.
        let err = lib
            .writer()
            .submit_wait(WriteOp::PanicAfter {
                statements,
                after: 3,
            })
            .unwrap_err();
        assert!(matches!(err, LibraryError::WriterPanicked), "{err:?}");
        // The library is not wedged: the next op goes through.
        lib.writer()
            .submit_wait(insert_setting("after", "ok"))
            .unwrap();
    }
    let lib = library::Library::open(&path).unwrap();
    let reader = lib.reader().unwrap();
    let rows = count(reader.connection(), "playlists");
    assert!(rows == 0 || rows == 5, "partial write: {rows} rows");
    assert_eq!(rows, 0, "the panicking transaction must roll back");
    assert_eq!(count(reader.connection(), "settings"), 1);
}

#[test]
fn concurrent_reader_sees_committed_data() {
    let dir = tempfile::tempdir().unwrap();
    let lib = library::Library::open(&db_path(&dir)).unwrap();
    let reader = lib.reader().unwrap();
    assert_eq!(count(reader.connection(), "settings"), 0);

    lib.writer().submit_wait(insert_setting("a", "1")).unwrap();
    // The reader opened before the commit sees it on its next statement...
    assert_eq!(count(reader.connection(), "settings"), 1);
    // ...and so does a reader on another thread.
    std::thread::scope(|s| {
        s.spawn(|| {
            let other = lib.reader().unwrap();
            assert_eq!(count(other.connection(), "settings"), 1);
        });
    });
    // Readers are read-only.
    assert!(
        reader
            .connection()
            .execute("DELETE FROM settings", [])
            .is_err()
    );
}

#[test]
fn writer_outliving_library_reports_gone() {
    let dir = tempfile::tempdir().unwrap();
    let lib = library::Library::open(&db_path(&dir)).unwrap();
    let writer = lib.writer();
    writer.submit(insert_setting("queued", "1")).unwrap();
    drop(lib);
    let err = writer.submit(insert_setting("late", "1")).unwrap_err();
    assert!(matches!(err, LibraryError::WriterGone), "{err:?}");
    // Ops queued before the library closed were committed on shutdown.
    let conn = Connection::open(db_path(&dir)).unwrap();
    assert_eq!(count(&conn, "settings"), 1);
}

#[test]
fn cues_survive_track_delete_attempt() {
    let dir = tempfile::tempdir().unwrap();
    let lib = library::Library::open(&db_path(&dir)).unwrap();
    let writer = lib.writer();
    writer
        .submit_wait(WriteOp::Raw(vec![
            RawStatement {
                sql: "INSERT INTO tracks(id, path, fingerprint, added_at) VALUES (1, '/a.mp3', x'00', 0)"
                    .into(),
                params: vec![],
            },
            RawStatement {
                sql: "INSERT INTO cues(track_id, cue_frames) VALUES (1, 42.0)".into(),
                params: vec![],
            },
        ]))
        .unwrap();
    // No cascade: with foreign keys on, deleting a track that still has a cue fails.
    let err = writer
        .submit_wait(WriteOp::Raw(vec![RawStatement {
            sql: "DELETE FROM tracks WHERE id = 1".into(),
            params: vec![],
        }]))
        .unwrap_err();
    assert!(matches!(err, LibraryError::Sqlite(_)), "{err:?}");
    let reader = lib.reader().unwrap();
    assert_eq!(count(reader.connection(), "cues"), 1);
}
