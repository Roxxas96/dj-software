//! What an export reads from a temp library.

use std::fs;
use std::path::{Path, PathBuf};

use interop::export::{FileKind, gather};
use library::{GridEdit, HotcuePoint, Library, RawStatement, WriteOp, add_folder, scan_folder};
use rusqlite::types::Value;

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
    Setup {
        _dir: dir,
        lib,
        music,
    }
}

/// Writes a 16-bit mono 44.1 kHz WAV of silence; `seed` makes the fingerprint differ.
fn write_wav(path: &Path, seed: i16) {
    let frames = 44_100u32;
    let data_len = frames * 2;
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&44_100u32.to_le_bytes());
    bytes.extend_from_slice(&88_200u32.to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_len.to_le_bytes());
    for i in 0..frames {
        let v = if i % 100 < 50 { seed } else { -seed };
        bytes.extend_from_slice(&v.to_le_bytes());
    }
    fs::write(path, bytes).unwrap();
}

fn raw(lib: &Library, sql: &str, params: Vec<Value>) {
    lib.writer()
        .submit_wait(WriteOp::Raw(vec![RawStatement {
            sql: sql.into(),
            params,
        }]))
        .unwrap();
}

/// Adds `names` as WAV files and returns their track ids in the same order.
fn add_tracks(s: &Setup, names: &[&str]) -> Vec<i64> {
    for (i, name) in names.iter().enumerate() {
        write_wav(&s.music.join(name), 1000 + i as i16 * 500);
    }
    add_folder(&s.lib, &s.music).unwrap();
    scan_folder(&s.lib, &s.music, &mut |_| {}).unwrap();
    let reader = s.lib.reader().unwrap();
    names
        .iter()
        .map(|n| {
            reader
                .connection()
                .query_row(
                    "SELECT id FROM tracks WHERE path LIKE ?1",
                    [format!("%{n}")],
                    |r| r.get(0),
                )
                .unwrap()
        })
        .collect()
}

fn playlist(s: &Setup, name: &str, tracks: &[i64]) -> i64 {
    raw(
        &s.lib,
        "INSERT INTO playlists(name, position) VALUES (?1, 0)",
        vec![Value::Text(name.into())],
    );
    let id: i64 = s
        .lib
        .reader()
        .unwrap()
        .connection()
        .query_row("SELECT id FROM playlists WHERE name = ?1", [name], |r| {
            r.get(0)
        })
        .unwrap();
    for (pos, t) in tracks.iter().enumerate() {
        raw(
            &s.lib,
            "INSERT INTO playlist_tracks(playlist_id, position, track_id) VALUES (?1, ?2, ?3)",
            vec![
                Value::Integer(id),
                Value::Integer(pos as i64),
                Value::Integer(*t),
            ],
        );
    }
    id
}

#[test]
fn gather_keeps_order_and_duplicates() {
    let s = setup();
    let ids = add_tracks(&s, &["a.wav", "b.wav"]);
    let (a, b) = (ids[0], ids[1]);
    let p = playlist(&s, "P", &[a, b, a]);
    let set = gather(&s.lib.reader().unwrap(), &[p]).unwrap();
    assert_eq!(set.playlists.len(), 1);
    assert_eq!(set.playlists[0].name, "P");
    assert_eq!(set.playlists[0].tracks, vec![a, b, a]);
    assert_eq!(
        set.tracks.iter().map(|t| t.id).collect::<Vec<_>>(),
        vec![a, b]
    );
    assert!(set.skipped.is_empty());
}

#[test]
fn gather_skips_missing_tracks() {
    let s = setup();
    let ids = add_tracks(&s, &["a.wav", "b.wav"]);
    raw(
        &s.lib,
        "UPDATE tracks SET missing = 1 WHERE id = ?1",
        vec![Value::Integer(ids[1])],
    );
    let p = playlist(&s, "P", &ids);
    let set = gather(&s.lib.reader().unwrap(), &[p]).unwrap();
    assert_eq!(set.tracks.len(), 1);
    assert_eq!(set.playlists[0].tracks, vec![ids[0]]);
    assert_eq!(set.skipped.len(), 1);
    assert!(set.skipped[0].path.ends_with("b.wav"));
    assert_eq!(set.skipped[0].why, "missing");
}

#[test]
fn gather_reads_cues_grid_and_key() {
    let s = setup();
    let id = add_tracks(&s, &["a.wav"])[0];
    let hot = HotcuePoint {
        frames: 1000.0,
        loop_frames: None,
        color: Some(0xFF0000),
        name: Some("Drop".into()),
    };
    let looped = HotcuePoint {
        frames: 2000.0,
        loop_frames: Some(500.0),
        color: None,
        name: None,
    };
    for (slot, point) in [(1, hot.clone()), (2, looped.clone())] {
        s.lib
            .writer()
            .submit_wait(WriteOp::PutHotcue {
                track: id,
                slot,
                point,
            })
            .unwrap();
    }
    s.lib
        .writer()
        .submit_wait(WriteOp::SetCue {
            track: id,
            frames: 300.0,
        })
        .unwrap();
    s.lib.edit_grid(id, GridEdit::SetBpm(128.0), None).unwrap();
    raw(
        &s.lib,
        "UPDATE analysis SET key = 13 WHERE track_id = ?1",
        vec![Value::Integer(id)],
    );
    let p = playlist(&s, "P", &[id]);
    let set = gather(&s.lib.reader().unwrap(), &[p]).unwrap();
    let t = &set.tracks[0];
    assert_eq!(t.kind, FileKind::Wav);
    assert_eq!(t.sample_rate, 44_100);
    assert_eq!(t.hotcues[0], Some(hot));
    assert_eq!(t.hotcues[1], Some(looped));
    assert_eq!(t.cue_frames, Some(300.0));
    assert_eq!(t.bpm, Some(128.0));
    assert!(t.first_beat_frames.is_some());
    assert_eq!(t.key.as_deref(), Some("C#m"));
    assert_eq!(t.lead_in, decode::LeadIn::default());
}

#[test]
fn gather_refuses_an_unknown_playlist() {
    let s = setup();
    let err = gather(&s.lib.reader().unwrap(), &[999]).unwrap_err();
    assert_eq!(err.to_string(), "playlist 999 no longer exists");
}
