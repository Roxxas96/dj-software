//! Watching folders: debounced events become per-path scans.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use library::{
    DEBOUNCE, Library, RawStatement, Watcher, WriteOp, scan_folder, watch, watch_with_debounce,
};
use rusqlite::types::Value;

/// Longest any test waits for the watcher.
const CAP: Duration = Duration::from_secs(10);

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../decode/tests/fixtures")
        .join(name)
}

struct Setup {
    _dir: tempfile::TempDir,
    lib: Library,
    music: PathBuf,
}

fn setup() -> Setup {
    let dir = tempfile::tempdir().unwrap();
    let lib = Library::open(&dir.path().join("db").join("library.sqlite3")).unwrap();
    let music = dir.path().join("music");
    fs::create_dir_all(&music).unwrap();
    let music = dunce::canonicalize(&music).unwrap();
    Setup {
        _dir: dir,
        lib,
        music,
    }
}

/// `(id, path, missing)` of every track, by path.
fn tracks(lib: &Library) -> Vec<(i64, String, bool)> {
    let reader = lib.reader().unwrap();
    let mut stmt = reader
        .connection()
        .prepare("SELECT id, path, missing FROM tracks ORDER BY path")
        .unwrap();
    stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

/// Polls until `done` holds, failing after [`CAP`].
fn wait_for(what: &str, mut done: impl FnMut() -> bool) {
    let start = Instant::now();
    while !done() {
        assert!(start.elapsed() < CAP, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn assert_quiet(w: &Watcher) {
    assert!(w.errors().try_recv().is_err(), "watcher reported an error");
}

#[test]
fn new_file_appears_after_debounce() {
    let s = setup();
    let w = watch(&s.lib, std::slice::from_ref(&s.music)).unwrap();
    let new = s.music.join("new.wav");
    let start = Instant::now();
    fs::copy(fixture("sine440.wav"), &new).unwrap();
    wait_for("the new track", || !tracks(&s.lib).is_empty());
    assert!(start.elapsed() >= DEBOUNCE, "scanned before the debounce");
    let rows = tracks(&s.lib);
    assert_eq!(rows.len(), 1);
    assert_eq!(Path::new(&rows[0].1), new);
    assert!(!rows[0].2);
    assert_quiet(&w);
}

#[test]
fn removed_file_becomes_missing() {
    let s = setup();
    let path = s.music.join("gone.wav");
    fs::copy(fixture("sine440.wav"), &path).unwrap();
    scan_folder(&s.lib, &s.music, &mut |_| {}).unwrap();
    let id = tracks(&s.lib)[0].0;
    let w = watch_with_debounce(
        &s.lib,
        std::slice::from_ref(&s.music),
        Duration::from_millis(300),
    )
    .unwrap();
    fs::remove_file(&path).unwrap();
    wait_for("the track to be flagged missing", || tracks(&s.lib)[0].2);
    let rows = tracks(&s.lib);
    assert_eq!(rows.len(), 1, "tracks are never deleted");
    assert_eq!(rows[0].0, id);
    assert_quiet(&w);
}

#[test]
fn burst_of_events_scans_once() {
    let s = setup();
    let w = watch_with_debounce(
        &s.lib,
        std::slice::from_ref(&s.music),
        Duration::from_millis(500),
    )
    .unwrap();
    let path = s.music.join("burst.wav");
    let bytes = fs::read(fixture("sine440.wav")).unwrap();
    for _ in 0..20 {
        let mut f = fs::File::create(&path).unwrap();
        f.write_all(&bytes).unwrap();
    }
    wait_for("the track", || !tracks(&s.lib).is_empty());
    // Anything late would arrive within a debounce or two.
    let settle = Instant::now();
    while settle.elapsed() < Duration::from_secs(1) {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(w.scan_count(), 1);
    assert_eq!(tracks(&s.lib).len(), 1);
    assert_quiet(&w);
}

#[test]
fn rename_within_watched_folder_keeps_id() {
    let s = setup();
    let old = s.music.join("old.wav");
    fs::copy(fixture("sine440.wav"), &old).unwrap();
    scan_folder(&s.lib, &s.music, &mut |_| {}).unwrap();
    let id = tracks(&s.lib)[0].0;
    s.lib
        .writer()
        .submit_wait(WriteOp::Raw(vec![RawStatement {
            sql: "INSERT INTO cues(track_id, cue_frames) VALUES (?1, 1234.0)".into(),
            params: vec![Value::Integer(id)],
        }]))
        .unwrap();

    let w = watch_with_debounce(
        &s.lib,
        std::slice::from_ref(&s.music),
        Duration::from_millis(300),
    )
    .unwrap();
    let new = s.music.join("sub").join("renamed.wav");
    fs::create_dir_all(new.parent().unwrap()).unwrap();
    fs::rename(&old, &new).unwrap();
    wait_for("the track to follow the rename", || {
        let rows = tracks(&s.lib);
        rows.len() == 1 && Path::new(&rows[0].1) == new && !rows[0].2
    });
    assert_eq!(tracks(&s.lib)[0].0, id, "same id after the rename");
    let reader = s.lib.reader().unwrap();
    let cues: i64 = reader
        .connection()
        .query_row("SELECT COUNT(*) FROM cues WHERE track_id = ?1", [id], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(cues, 1);
    assert_quiet(&w);
}

#[test]
fn dropping_the_watcher_stops_it() {
    let s = setup();
    let w = watch_with_debounce(
        &s.lib,
        std::slice::from_ref(&s.music),
        Duration::from_millis(100),
    )
    .unwrap();
    drop(w);
    fs::copy(fixture("sine440.wav"), s.music.join("late.wav")).unwrap();
    std::thread::sleep(Duration::from_millis(600));
    assert!(tracks(&s.lib).is_empty());
}
