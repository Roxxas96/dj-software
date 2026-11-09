//! VirtualDJ's drive database and lists written for an export.

use std::fs;
use std::path::{Path, PathBuf};

use decode::LeadIn;
use interop::export::virtualdj::{DB_PATH, LISTS_DIR, VirtualDjFormat, database};
use interop::export::{
    ExportPlan, ExportPlaylist, ExportTrack, FileKind, PlannedTrack, StickContext, StickFormat,
    TrackAction,
};
use interop::{VdjPoi, read_database, read_playlists};
use library::HotcuePoint;
use tempfile::TempDir;

fn track(id: i64) -> ExportTrack {
    ExportTrack {
        id,
        path: format!("/m/{id}.mp3").into(),
        fingerprint: [0; 32],
        size: 1,
        title: format!("Track {id}"),
        artist: Some("Artist".into()),
        album: Some("Album".into()),
        genre: None,
        sample_rate: 44_100,
        duration_frames: Some(44_100 * 61),
        bpm: Some(120.0),
        first_beat_frames: Some(0.0),
        key: Some("C#m".into()),
        cue_frames: None,
        hotcues: Default::default(),
        kind: FileKind::Mp3,
        lead_in: LeadIn::default(),
    }
}

fn with_cues(mut t: ExportTrack) -> ExportTrack {
    t.hotcues[0] = Some(HotcuePoint {
        frames: 44_100.0 * 2.5,
        loop_frames: None,
        color: Some(0x00FF_8000),
        name: Some("Drop <1> & \"co\"".into()),
    });
    // Four beats at 120 BPM is two seconds.
    t.hotcues[1] = Some(HotcuePoint {
        frames: 44_100.0 * 10.0,
        loop_frames: Some(44_100.0 * 2.0),
        color: None,
        name: None,
    });
    t
}

fn planned(t: ExportTrack) -> PlannedTrack {
    let stick_path = format!("Open Jockey/Music/{}.mp3", t.id);
    PlannedTrack {
        track: t,
        stick_path,
        action: TrackAction::Copy,
    }
}

fn plan(stick: &Path, tracks: Vec<ExportTrack>, names: &[&str]) -> ExportPlan {
    let ids: Vec<i64> = tracks.iter().map(|t| t.id).collect();
    for t in &tracks {
        let path = stick
            .join("Open Jockey")
            .join("Music")
            .join(format!("{}.mp3", t.id));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, vec![0u8; 1234]).unwrap();
    }
    ExportPlan {
        stick: stick.to_path_buf(),
        tracks: tracks.into_iter().map(planned).collect(),
        playlists: names
            .iter()
            .map(|n| ExportPlaylist {
                name: (*n).into(),
                tracks: ids.clone(),
            })
            .collect(),
        skipped: Vec::new(),
        copy_bytes: 0,
        free_bytes: 0,
        without_serato_cues: 0,
        stale_tracks: Vec::new(),
        previous: None,
    }
}

fn cx(plan: &ExportPlan) -> StickContext<'_> {
    StickContext {
        plan,
        now_unix: 1_700_000_000,
    }
}

#[test]
fn round_trips_through_the_11a_reader() {
    let stick = TempDir::new().unwrap();
    let p = plan(
        stick.path(),
        vec![with_cues(track(1)), with_cues(track(2))],
        &["A list", "B list"],
    );
    let out = VirtualDjFormat.write_library(&cx(&p)).unwrap();
    assert_eq!(out.files, vec!["A list.vdjfolder", "B list.vdjfolder"]);
    assert!(out.notes.is_empty());

    let db = stick.path().join("VirtualDJ").join("database.xml");
    let songs = read_database(&db, Some(stick.path())).unwrap();
    assert_eq!(songs.len(), 2);
    for (i, song) in songs.iter().enumerate() {
        assert_eq!(
            song.path,
            stick
                .path()
                .join("Open Jockey")
                .join("Music")
                .join(format!("{}.mp3", i + 1))
        );
        assert_eq!(song.size, Some(1234));
        assert_eq!(song.pois.len(), 2);
        match &song.pois[0] {
            VdjPoi::Hotcue {
                num,
                seconds,
                name,
                color,
            } => {
                assert_eq!(*num, 1);
                assert!((seconds - 2.5).abs() < 1e-6);
                assert_eq!(name.as_deref(), Some("Drop <1> & \"co\""));
                assert_eq!(*color, Some(0x00FF_8000));
            }
            other => panic!("{other:?}"),
        }
        match &song.pois[1] {
            VdjPoi::Loop {
                num,
                seconds,
                beats,
                name,
                ..
            } => {
                assert_eq!(*num, 2);
                assert!((seconds - 10.0).abs() < 1e-6);
                assert!((beats - 4.0).abs() < 1e-9);
                assert_eq!(*name, None);
            }
            other => panic!("{other:?}"),
        }
    }

    let lists = stick.path().join("VirtualDJ").join("MyLists");
    let (playlists, errors) = read_playlists(&[lists], None).unwrap();
    assert!(errors.is_empty());
    let names: Vec<&str> = playlists.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(names, ["A list", "B list"]);
    let copies: Vec<PathBuf> = (1..=2)
        .map(|i| {
            stick
                .path()
                .join("Open Jockey")
                .join("Music")
                .join(format!("{i}.mp3"))
        })
        .collect();
    assert_eq!(playlists[0].paths, copies);
    assert_eq!(DB_PATH, "VirtualDJ/database.xml");
    assert_eq!(LISTS_DIR, "VirtualDJ/MyLists");
}

#[test]
fn keeps_foreign_songs_byte_for_byte() {
    let stick = TempDir::new().unwrap();
    let p = plan(stick.path(), vec![track(1)], &[]);
    let friend = "<Song FilePath=\"D:/friend.mp3\" FileSize=\"7\">\n  <Tags Title=\"F\"   />\n\t<Poi Pos=\"1.5\" Num=\"1\" Type=\"cue\"/>\n </Song>";
    let lone = "<FavoriteFolder   Path=\"x\"/>";
    let old = "<Song FilePath=\"D:/Open Jockey/Music/old.mp3\" FileSize=\"1\"><Tags Title=\"Old\"/></Song>";
    let existing = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<VirtualDJ_Database Version=\"2024\">\n {friend}\n {lone}\n{old}\n</VirtualDJ_Database>\n"
    );
    let (bytes, left_out) = database(Some(existing.as_bytes()), &cx(&p)).unwrap();
    let text = String::from_utf8(bytes).unwrap();
    assert_eq!(left_out, 0);
    assert!(text.contains(friend), "{text}");
    assert!(text.contains(lone), "{text}");
    assert!(!text.contains("old.mp3"), "{text}");
    assert!(text.contains("Open Jockey/Music/1.mp3"));
    assert!(text.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>"));
    assert!(text.contains("<VirtualDJ_Database Version=\"2025\">"));
    let db = TempDir::new().unwrap();
    fs::write(db.path().join("d.xml"), &text).unwrap();
    let songs = read_database(&db.path().join("d.xml"), Some(Path::new("/s"))).unwrap();
    assert_eq!(songs.len(), 2);
    assert_eq!(songs[0].pois.len(), 1);
}

#[test]
fn loop_without_bpm_is_left_out_and_counted() {
    let stick = TempDir::new().unwrap();
    let mut t = with_cues(track(1));
    t.bpm = None;
    let p = plan(stick.path(), vec![t], &["L"]);
    let (bytes, left_out) = database(None, &cx(&p)).unwrap();
    assert_eq!(left_out, 1);
    let text = String::from_utf8(bytes).unwrap();
    assert!(!text.contains("loop"), "{text}");
    assert!(text.contains("Type=\"cue\""));
    let out = VirtualDjFormat.write_library(&cx(&p)).unwrap();
    assert_eq!(
        out.notes,
        ["1 saved loops left out of VirtualDJ: their tracks have no BPM"]
    );
    let list = fs::read_to_string(stick.path().join("VirtualDJ/MyLists/L.vdjfolder")).unwrap();
    assert!(!list.contains("bpm="), "{list}");
}

#[test]
fn refuses_a_foreign_root() {
    let stick = TempDir::new().unwrap();
    let p = plan(stick.path(), vec![track(1)], &[]);
    assert!(database(Some(b"<foo/>"), &cx(&p)).is_err());
    assert!(database(Some(b"not xml <"), &cx(&p)).is_err());
}

#[test]
fn a_playlist_clash_gets_the_suffix_and_our_own_is_replaced() {
    let stick = TempDir::new().unwrap();
    let lists = stick.path().join("VirtualDJ").join("MyLists");
    fs::create_dir_all(&lists).unwrap();
    fs::write(lists.join("Mine.vdjfolder"), "friend's").unwrap();
    let p = plan(stick.path(), vec![track(1)], &["Mine"]);
    let out = VirtualDjFormat.write_library(&cx(&p)).unwrap();
    assert_eq!(out.files, vec!["Mine (Open Jockey).vdjfolder"]);
    assert_eq!(
        fs::read_to_string(lists.join("Mine.vdjfolder")).unwrap(),
        "friend's"
    );
}

#[test]
fn a_database_that_is_not_a_regular_file_is_refused() {
    let stick = TempDir::new().unwrap();
    fs::create_dir_all(stick.path().join("VirtualDJ").join("database.xml")).unwrap();
    let p = plan(stick.path(), vec![track(1)], &[]);
    assert!(VirtualDjFormat.write_library(&cx(&p)).is_err());
}
