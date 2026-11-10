//! The export plan, against a temp dir standing in for the stick.

use std::fs;
use std::path::{Path, PathBuf};

use decode::LeadIn;
use interop::export::{
    ExportError, ExportPlan, ExportPlaylist, ExportSet, ExportTrack, FileKind, MANIFEST, Manifest,
    OJ_DIR, SPACE_MARGIN, TrackAction, plan_export,
};

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

fn export_track(id: i64, path: &Path) -> ExportTrack {
    ExportTrack {
        id,
        path: path.into(),
        fingerprint: library::fingerprint::of_file(path).unwrap(),
        size: fs::metadata(path).unwrap().len(),
        title: "t".into(),
        artist: Some("Artist".into()),
        album: None,
        genre: None,
        sample_rate: 44_100,
        duration_frames: None,
        bpm: None,
        first_beat_frames: None,
        key: None,
        cue_frames: None,
        hotcues: Default::default(),
        kind: FileKind::Wav,
        lead_in: LeadIn::default(),
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    stick: PathBuf,
    music: PathBuf,
    set: ExportSet,
}

fn fixture(names: &[&str]) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let stick = dir.path().join("stick");
    let music = dir.path().join("music");
    fs::create_dir_all(&stick).unwrap();
    fs::create_dir_all(&music).unwrap();
    let mut tracks = Vec::new();
    for (i, name) in names.iter().enumerate() {
        let path = music.join(name);
        write_wav(&path, 1000 + i as i16 * 500);
        tracks.push(export_track(i as i64 + 1, &path));
    }
    let ids = tracks.iter().map(|t| t.id).collect();
    Fixture {
        _dir: dir,
        stick,
        music,
        set: ExportSet {
            tracks,
            playlists: vec![ExportPlaylist {
                name: "p".into(),
                tracks: ids,
            }],
            skipped: vec![],
        },
    }
}

#[test]
fn first_export_copies_everything() {
    let f = fixture(&["a.wav", "b.wav"]);
    let sum: u64 = f.set.tracks.iter().map(|t| t.size).sum();
    let plan = plan_export(f.set, &f.stick, std::slice::from_ref(&f.music)).unwrap();
    assert_eq!(plan.tracks.len(), 2);
    assert!(plan.tracks.iter().all(|t| t.action == TrackAction::Copy));
    assert_eq!(plan.copy_bytes, sum);
    assert_eq!(plan.previous, None);
    assert!(plan.free_bytes.is_some_and(|f| f > 0));
    assert_eq!(plan.without_serato_cues, 0);
}

#[test]
fn unchanged_copy_is_kept() {
    let f = fixture(&["a.wav"]);
    let planned = plan_export(f.set.clone(), &f.stick, &[]).unwrap();
    let on_stick = f.stick.join(
        planned.tracks[0]
            .stick_path
            .replace('/', std::path::MAIN_SEPARATOR_STR),
    );
    fs::create_dir_all(on_stick.parent().unwrap()).unwrap();
    fs::copy(&f.set.tracks[0].path, &on_stick).unwrap();
    let plan = plan_export(f.set, &f.stick, &[]).unwrap();
    assert_eq!(plan.tracks[0].action, TrackAction::Keep);
    assert_eq!(plan.copy_bytes, 0);
}

#[test]
fn changed_audio_is_copied_again() {
    let f = fixture(&["a.wav"]);
    let planned = plan_export(f.set.clone(), &f.stick, &[]).unwrap();
    let on_stick = f.stick.join(
        planned.tracks[0]
            .stick_path
            .replace('/', std::path::MAIN_SEPARATOR_STR),
    );
    fs::create_dir_all(on_stick.parent().unwrap()).unwrap();
    write_wav(&on_stick, 9000);
    let plan = plan_export(f.set, &f.stick, &[]).unwrap();
    assert_eq!(plan.tracks[0].action, TrackAction::Copy);
    assert_eq!(plan.copy_bytes, plan.tracks[0].track.size);
}

#[test]
fn refuses_a_library_folder() {
    let f = fixture(&["a.wav"]);
    let inside = f.music.join("sub");
    fs::create_dir_all(&inside).unwrap();
    for (stick, folder, wording) in [
        (f.music.clone(), f.music.clone(), "holds the library folder"),
        (inside, f.music.clone(), "is inside the library folder"),
        (
            f.stick.parent().unwrap().to_path_buf(),
            f.music.clone(),
            "holds the library folder",
        ),
    ] {
        let err = plan_export(f.set.clone(), &stick, std::slice::from_ref(&folder)).unwrap_err();
        match err {
            ExportError::Refused(m) => {
                assert!(m.contains(wording), "{m}");
                assert!(m.contains(&folder.display().to_string()), "{m}");
            }
            other => panic!("{other:?}"),
        }
    }
}

#[test]
fn counts_types_without_serato_cues() {
    let mut f = fixture(&["a.wav", "b.wav"]);
    f.set.tracks[1].kind = FileKind::Other("flac".into());
    let plan = plan_export(f.set, &f.stick, &[]).unwrap();
    assert_eq!(plan.without_serato_cues, 1);
}

#[test]
fn stale_tracks_come_from_the_manifest() {
    let f = fixture(&["a.wav"]);
    let used = plan_export(f.set.clone(), &f.stick, &[]).unwrap().tracks[0]
        .stick_path
        .strip_prefix("Open Jockey/")
        .unwrap()
        .to_string();
    Manifest {
        version: 1,
        playlists: vec![],
        crates: vec![],
        vdj_lists: vec![],
        tracks: vec!["Music/Old/x.mp3".into(), used],
    }
    .write(&f.stick)
    .unwrap();
    fs::write(f.stick.join(OJ_DIR).join("stray.mp3"), b"x").unwrap();
    assert!(f.stick.join(OJ_DIR).join(MANIFEST).is_file());
    let plan = plan_export(f.set, &f.stick, &[]).unwrap();
    assert_eq!(plan.stale_tracks, vec!["Music/Old/x.mp3".to_string()]);
    assert!(plan.previous.is_some());
}

#[test]
fn too_small_when_free_space_is_short() {
    let plan = ExportPlan {
        stick: PathBuf::new(),
        tracks: vec![],
        playlists: vec![],
        skipped: vec![],
        copy_bytes: 10,
        free_bytes: Some(SPACE_MARGIN),
        without_serato_cues: 0,
        stale_tracks: vec![],
        previous: None,
    };
    assert!(plan.too_small());
}
