//! Planning and applying a VirtualDJ import against a temp library.

use std::fs;
use std::path::{Path, PathBuf};

use interop::{
    ImportPlan, PlaylistAction, Resolution, VdjPlaylist, VdjPoi, VdjSong, apply, plan_virtualdj,
};
use library::{HotcuePoint, Library, RawStatement, WriteOp};
use rusqlite::types::Value;

const RATE: f64 = 44_100.0;

fn open() -> (tempfile::TempDir, Library) {
    let dir = tempfile::tempdir().unwrap();
    let lib = Library::open(&dir.path().join("library.sqlite3")).unwrap();
    (dir, lib)
}

fn raw(lib: &Library, sql: &str, params: Vec<Value>) {
    lib.writer()
        .submit_wait(WriteOp::Raw(vec![RawStatement {
            sql: sql.into(),
            params,
        }]))
        .unwrap();
}

/// Adds a 44.1 kHz track and returns its id.
fn add_track(lib: &Library, path: &str, size: i64, fingerprint: &[u8]) -> i64 {
    raw(
        lib,
        "INSERT INTO tracks(path, fingerprint, size, added_at, sample_rate) \
         VALUES (?1, ?2, ?3, 0, 44100)",
        vec![
            Value::Text(path.into()),
            Value::Blob(fingerprint.to_vec()),
            Value::Integer(size),
        ],
    );
    lib.reader()
        .unwrap()
        .track_id_by_path(Path::new(path))
        .unwrap()
        .unwrap()
}

fn track(lib: &Library, path: &str) -> i64 {
    add_track(lib, path, 1000, &[0; 32])
}

fn hotcue(num: u32, seconds: f64) -> VdjPoi {
    VdjPoi::Hotcue {
        num,
        seconds,
        name: None,
        color: None,
    }
}

fn song(path: &str, pois: Vec<VdjPoi>) -> VdjSong {
    VdjSong {
        path: PathBuf::from(path),
        size: Some(1000),
        seconds_per_beat: Some(0.5),
        pois,
        other_markers: 0,
    }
}

fn point(frames: f64) -> HotcuePoint {
    HotcuePoint {
        frames,
        loop_frames: None,
        color: None,
        name: None,
    }
}

fn put(lib: &Library, track: i64, slot: u8, frames: f64) {
    lib.writer()
        .submit_wait(WriteOp::PutHotcue {
            track,
            slot,
            point: point(frames),
        })
        .unwrap();
}

fn slots(lib: &Library, track: i64) -> [Option<HotcuePoint>; 8] {
    lib.reader().unwrap().hotcue_points(track).unwrap()
}

fn plan(lib: &Library, songs: &[VdjSong], playlists: &[VdjPlaylist]) -> ImportPlan {
    plan_virtualdj(&lib.reader().unwrap(), songs, playlists).unwrap()
}

/// Every row of `table`, rendered, in rowid order.
fn rows(lib: &Library, table: &str) -> Vec<String> {
    let reader = lib.reader().unwrap();
    let mut stmt = reader
        .connection()
        .prepare(&format!("SELECT * FROM {table} ORDER BY rowid"))
        .unwrap();
    let n = stmt.column_count();
    stmt.query_map([], |r| {
        let values: Vec<String> = (0..n)
            .map(|i| format!("{:?}", r.get::<_, Value>(i).unwrap()))
            .collect();
        Ok(values.join("|"))
    })
    .unwrap()
    .collect::<Result<_, _>>()
    .unwrap()
}

/// Writes a 16-bit stereo 44.1 kHz WAV of a 440 Hz sine, `seconds` long.
fn write_wav(path: &Path, seconds: f64) {
    const SR: u32 = 44_100;
    let total = (seconds * f64::from(SR)) as usize;
    let data_len = u32::try_from(total * 4).unwrap();
    let mut bytes = Vec::with_capacity(44 + total * 4);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&SR.to_le_bytes());
    bytes.extend_from_slice(&(SR * 4).to_le_bytes());
    bytes.extend_from_slice(&4u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_len.to_le_bytes());
    for i in 0..total {
        let t = i as f64 / f64::from(SR);
        let v = ((std::f64::consts::TAU * 440.0 * t).sin() * 8000.0) as i16;
        bytes.extend_from_slice(&v.to_le_bytes());
        bytes.extend_from_slice(&v.to_le_bytes());
    }
    fs::write(path, bytes).unwrap();
}

#[test]
fn slots_positions_and_loops_convert() {
    let (_dir, lib) = open();
    let id = track(&lib, "/music/a.mp3");
    let mut s = song(
        "/music/a.mp3",
        vec![
            VdjPoi::Hotcue {
                num: 1,
                seconds: 6.765_714,
                name: Some("Drop".into()),
                color: Some(0xFF_0000),
            },
            VdjPoi::Loop {
                num: 4,
                seconds: 128.456_145,
                beats: 8.0,
                name: Some("Outro".into()),
                color: Some(0x00_FF00),
            },
        ],
    );
    s.seconds_per_beat = Some(0.620_703);
    s.other_markers = 3;
    let plan = plan(&lib, &[s], &[]);
    assert_eq!(plan.tracks.len(), 1);
    assert_eq!(plan.other_markers, 3);
    assert_eq!(plan.dropped, 0);
    let t = &plan.tracks[0];
    assert_eq!(t.track, id);
    assert_eq!(t.path, PathBuf::from("/music/a.mp3"));
    let cue = t.incoming[0].as_ref().unwrap();
    assert!((cue.frames - 298_368.0).abs() <= 0.5, "{}", cue.frames);
    assert_eq!(cue.loop_frames, None);
    assert_eq!(cue.color, Some(0xFF_0000));
    assert_eq!(cue.name.as_deref(), Some("Drop"));
    let lp = t.incoming[3].as_ref().unwrap();
    assert!((lp.frames - 128.456_145 * RATE).abs() <= 0.5);
    let len = lp.loop_frames.unwrap();
    assert!((len - 218_984.0).abs() <= 0.5, "{len}");
    assert_eq!(lp.color, Some(0x00_FF00));
    assert_eq!(lp.name.as_deref(), Some("Outro"));
    assert!(t.incoming.iter().filter(|p| p.is_some()).count() == 2);
    assert!(!t.conflicts());
}

#[test]
fn a_loop_without_a_grid_imports_as_a_hotcue() {
    let (_dir, lib) = open();
    track(&lib, "/music/a.mp3");
    let mut s = song(
        "/music/a.mp3",
        vec![VdjPoi::Loop {
            num: 2,
            seconds: 1.0,
            beats: 4.0,
            name: None,
            color: None,
        }],
    );
    s.seconds_per_beat = None;
    let plan = plan(&lib, &[s], &[]);
    assert_eq!(plan.tracks[0].incoming[1], Some(point(RATE)));
}

#[test]
fn num_over_8_fills_free_slots_then_drops() {
    let (_dir, lib) = open();
    track(&lib, "/music/a.mp3");
    let full: Vec<VdjPoi> = (1..=8)
        .map(|n| hotcue(n, f64::from(n)))
        .chain([hotcue(12, 20.0), hotcue(10, 10.0)])
        .collect();
    let p = plan(&lib, &[song("/music/a.mp3", full)], &[]);
    assert_eq!(p.dropped, 2);

    let some_free: Vec<VdjPoi> = (1..=8)
        .filter(|n| *n != 5)
        .map(|n| hotcue(n, f64::from(n)))
        .chain([hotcue(12, 20.0), hotcue(10, 10.0)])
        .collect();
    let p = plan(&lib, &[song("/music/a.mp3", some_free)], &[]);
    assert_eq!(p.dropped, 1);
    assert_eq!(p.tracks[0].incoming[4], Some(point(10.0 * RATE)));
}

#[test]
fn two_pois_with_the_same_num_keep_the_earlier() {
    let (_dir, lib) = open();
    track(&lib, "/music/a.mp3");
    let p = plan(
        &lib,
        &[song("/music/a.mp3", vec![hotcue(3, 9.0), hotcue(3, 2.0)])],
        &[],
    );
    assert_eq!(p.dropped, 1);
    assert_eq!(p.tracks[0].incoming[2], Some(point(2.0 * RATE)));
}

#[test]
fn matching_by_path_then_fingerprint_then_name_and_size() {
    let (dir, lib) = open();
    let by_path = track(&lib, "/music/path.mp3");

    let wav = dir.path().join("vdj-copy.wav");
    write_wav(&wav, 3.5);
    let fp = library::fingerprint::of_file(&wav).unwrap();
    let by_fp = add_track(&lib, "/music/moved.wav", 1, &fp);

    let by_name = add_track(&lib, "/music/named.mp3", 4321, &[1; 32]);
    add_track(&lib, "/one/twin.mp3", 777, &[2; 32]);
    add_track(&lib, "/two/twin.mp3", 777, &[3; 32]);

    let mut named = song("/old/drive/named.mp3", vec![]);
    named.size = Some(4321);
    let mut twin = song("/old/drive/twin.mp3", vec![]);
    twin.size = Some(777);
    let songs = [
        song("/music/path.mp3", vec![]),
        song(wav.to_str().unwrap(), vec![]),
        named,
        twin,
        song("/nowhere/unknown.mp3", vec![]),
    ];
    let p = plan(&lib, &songs, &[]);
    let matched: Vec<(i64, PathBuf)> = p.tracks.iter().map(|t| (t.track, t.path.clone())).collect();
    assert_eq!(
        matched,
        vec![
            (by_path, PathBuf::from("/music/path.mp3")),
            (by_fp, wav.clone()),
            (by_name, PathBuf::from("/old/drive/named.mp3")),
        ]
    );
    assert_eq!(
        p.unmatched,
        vec![
            PathBuf::from("/old/drive/twin.mp3"),
            PathBuf::from("/nowhere/unknown.mp3")
        ]
    );
}

#[test]
fn a_track_without_a_sample_rate_is_unmatched() {
    let (_dir, lib) = open();
    raw(
        &lib,
        "INSERT INTO tracks(path, fingerprint, size, added_at) VALUES ('/music/a.mp3', x'00', 1000, 0)",
        vec![],
    );
    let p = plan(&lib, &[song("/music/a.mp3", vec![hotcue(1, 1.0)])], &[]);
    assert!(p.tracks.is_empty());
    assert_eq!(p.unmatched, vec![PathBuf::from("/music/a.mp3")]);
}

#[test]
fn keep_replace_merge() {
    let incoming = || vec![hotcue(1, 10.0), hotcue(2, 20.0)];
    let setup = || {
        let (dir, lib) = open();
        let id = track(&lib, "/music/a.mp3");
        put(&lib, id, 1, 100.0);
        put(&lib, id, 3, 300.0);
        (dir, lib, id)
    };

    let (_d, lib, id) = setup();
    let p = plan(&lib, &[song("/music/a.mp3", incoming())], &[]);
    assert!(p.tracks[0].conflicts());
    let report = apply(&lib, &p, |_| Resolution::Keep).unwrap();
    assert_eq!(report.tracks_changed, 0);
    let s = slots(&lib, id);
    assert_eq!(s[0], Some(point(100.0)));
    assert_eq!(s[1], None);
    assert_eq!(s[2], Some(point(300.0)));

    let (_d, lib, id) = setup();
    let p = plan(&lib, &[song("/music/a.mp3", incoming())], &[]);
    let report = apply(&lib, &p, |_| Resolution::Replace).unwrap();
    assert_eq!(report.tracks_changed, 1);
    let s = slots(&lib, id);
    assert_eq!(s[0], Some(point(10.0 * RATE)));
    assert_eq!(s[1], Some(point(20.0 * RATE)));
    assert_eq!(s[2], None);

    let (_d, lib, id) = setup();
    let p = plan(&lib, &[song("/music/a.mp3", incoming())], &[]);
    apply(&lib, &p, |_| Resolution::Merge).unwrap();
    let s = slots(&lib, id);
    assert_eq!(s[0], Some(point(100.0)));
    assert_eq!(s[1], Some(point(20.0 * RATE)));
    assert_eq!(s[2], Some(point(300.0)));
}

#[test]
fn a_track_without_conflict_takes_its_incoming_slots() {
    let (_dir, lib) = open();
    let id = track(&lib, "/music/a.mp3");
    let other = track(&lib, "/music/b.mp3");
    put(&lib, other, 1, 5.0);
    let p = plan(
        &lib,
        &[
            song("/music/a.mp3", vec![hotcue(2, 1.0)]),
            song("/music/b.mp3", vec![]),
        ],
        &[],
    );
    assert!(!p.tracks[1].conflicts(), "nothing incoming is no conflict");
    apply(&lib, &p, |_| Resolution::Keep).unwrap();
    assert_eq!(slots(&lib, id)[1], Some(point(RATE)));
    assert_eq!(
        slots(&lib, other)[0],
        Some(point(5.0)),
        "empty incoming deletes nothing"
    );
}

#[test]
fn importing_twice_changes_nothing_the_second_time() {
    let (_dir, lib) = open();
    let a = track(&lib, "/music/a.mp3");
    let b = track(&lib, "/music/b.mp3");
    put(&lib, a, 7, 1.0);
    let songs = [
        song(
            "/music/a.mp3",
            vec![
                VdjPoi::Hotcue {
                    num: 1,
                    seconds: 6.765_714,
                    name: Some("Drop".into()),
                    color: Some(0xFF_0000),
                },
                VdjPoi::Loop {
                    num: 2,
                    seconds: 30.1,
                    beats: 8.0,
                    name: None,
                    color: None,
                },
            ],
        ),
        song("/music/b.mp3", vec![hotcue(3, 2.5)]),
    ];
    let lists = [VdjPlaylist {
        name: "Set".into(),
        paths: vec![PathBuf::from("/music/b.mp3"), PathBuf::from("/music/a.mp3")],
    }];
    let first = plan(&lib, &songs, &lists);
    let report = apply(&lib, &first, |_| Resolution::Replace).unwrap();
    assert_eq!(report.tracks_changed, 2);
    assert_eq!(report.playlists_created, 1);
    let playlists = lib.reader().unwrap().playlists().unwrap();
    assert_eq!(playlists.len(), 1);
    assert_eq!(playlists[0].name, "Set");
    let order: Vec<i64> = lib
        .reader()
        .unwrap()
        .playlist_tracks(playlists[0].id, 0, 10)
        .unwrap()
        .iter()
        .map(|r| r.id)
        .collect();
    assert_eq!(order, vec![b, a]);

    let hotcues = rows(&lib, "hotcues");
    let second = plan(&lib, &songs, &lists);
    assert!(
        second
            .playlists
            .iter()
            .all(|p| matches!(p.action, PlaylistAction::SkipSame))
    );
    assert!(second.tracks.iter().all(|t| t.incoming == t.existing));
    for resolution in [Resolution::Keep, Resolution::Replace, Resolution::Merge] {
        assert_eq!(second.batch(|_| resolution), WriteOp::Batch(vec![]));
    }
    let report = apply(&lib, &second, |_| Resolution::Replace).unwrap();
    assert_eq!((report.tracks_changed, report.playlists_created), (0, 0));
    assert_eq!(rows(&lib, "hotcues"), hotcues);
    assert_eq!(lib.reader().unwrap().playlists().unwrap().len(), 1);
}

#[test]
fn a_different_playlist_with_the_same_name_gets_a_suffix() {
    let (_dir, lib) = open();
    let a = track(&lib, "/music/a.mp3");
    track(&lib, "/music/b.mp3");
    track(&lib, "/music/c.mp3");
    lib.writer()
        .submit_wait(WriteOp::CreatePlaylistWith {
            name: "Set".into(),
            tracks: vec![a],
        })
        .unwrap();
    let songs = [
        song("/music/a.mp3", vec![]),
        song("/music/b.mp3", vec![]),
        song("/music/c.mp3", vec![]),
    ];
    let list = |path: &str| VdjPlaylist {
        name: "Set".into(),
        paths: vec![PathBuf::from(path)],
    };

    let p = plan(&lib, &songs, &[list("/music/b.mp3")]);
    assert_eq!(p.playlists[0].name, "Set (2)");
    assert!(matches!(p.playlists[0].action, PlaylistAction::Create));
    apply(&lib, &p, |_| Resolution::Merge).unwrap();

    let p = plan(&lib, &songs, &[list("/music/c.mp3")]);
    assert_eq!(p.playlists[0].name, "Set (3)");
    assert!(matches!(p.playlists[0].action, PlaylistAction::Create));

    let p = plan(&lib, &songs, &[list("/music/b.mp3"), list("/music/c.mp3")]);
    assert_eq!(p.playlists[0].name, "Set (2)");
    assert!(matches!(p.playlists[0].action, PlaylistAction::SkipSame));
    assert_eq!(p.playlists[1].name, "Set (3)");
    assert!(matches!(p.playlists[1].action, PlaylistAction::Create));
}

#[test]
fn playlist_entries_outside_the_library_are_unmatched() {
    let (_dir, lib) = open();
    let a = track(&lib, "/music/a.mp3");
    let p = plan(
        &lib,
        &[song("/music/a.mp3", vec![])],
        &[VdjPlaylist {
            name: "Set".into(),
            paths: vec![PathBuf::from("/gone/x.mp3"), PathBuf::from("/music/a.mp3")],
        }],
    );
    assert_eq!(p.playlists[0].tracks, vec![a]);
    assert_eq!(p.unmatched, vec![PathBuf::from("/gone/x.mp3")]);
}

#[test]
fn apply_backs_up_first_and_writes_all_or_nothing() {
    let (dir, lib) = open();
    let a = track(&lib, "/music/a.mp3");
    let b = track(&lib, "/music/b.mp3");
    let p = plan(
        &lib,
        &[
            song("/music/a.mp3", vec![hotcue(1, 1.0)]),
            song("/music/b.mp3", vec![hotcue(1, 2.0)]),
        ],
        &[VdjPlaylist {
            name: "Set".into(),
            paths: vec![PathBuf::from("/music/a.mp3")],
        }],
    );
    raw(
        &lib,
        "DELETE FROM tracks WHERE id = ?1",
        vec![Value::Integer(b)],
    );
    let backups = || {
        fs::read_dir(dir.path().join("backups"))
            .map(|d| d.count())
            .unwrap_or(0)
    };
    let before = backups();

    assert!(apply(&lib, &p, |_| Resolution::Replace).is_err());
    assert_eq!(backups(), before + 1, "the backup is taken before writing");
    assert_eq!(slots(&lib, a), [const { None }; 8]);
    assert!(lib.reader().unwrap().playlists().unwrap().is_empty());
}

#[test]
fn apply_reports_its_backup() {
    let (_dir, lib) = open();
    track(&lib, "/music/a.mp3");
    let p = plan(&lib, &[song("/music/a.mp3", vec![hotcue(1, 1.0)])], &[]);
    let report = apply(&lib, &p, |_| Resolution::Replace).unwrap();
    assert!(report.backup.is_file());
}

#[test]
fn grids_are_never_touched() {
    let (_dir, lib) = open();
    let id = track(&lib, "/music/a.mp3");
    raw(
        &lib,
        "INSERT INTO analysis(track_id, analyzer_version, bpm, first_beat_frames, grid_locked) \
         VALUES (?1, 3, 128.0, 512.0, 1)",
        vec![Value::Integer(id)],
    );
    let before = rows(&lib, "analysis");
    let mut s = song(
        "/music/a.mp3",
        vec![VdjPoi::Loop {
            num: 1,
            seconds: 1.0,
            beats: 4.0,
            name: None,
            color: None,
        }],
    );
    s.seconds_per_beat = Some(0.620_703);
    let p = plan(&lib, &[s], &[]);
    apply(&lib, &p, |_| Resolution::Replace).unwrap();
    assert_eq!(rows(&lib, "analysis"), before);
}
