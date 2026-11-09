//! The Rekordbox XML written for the exported playlists.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use decode::LeadIn;
use interop::export::rekordbox::{RekordboxFormat, XML_PATH, xml};
use interop::export::{
    ExportPlan, ExportPlaylist, ExportTrack, FileKind, PlannedTrack, StickContext, StickFormat,
    TrackAction,
};
use library::HotcuePoint;
use quick_xml::Reader;
use quick_xml::events::Event;

fn track(id: i64, kind: FileKind) -> ExportTrack {
    ExportTrack {
        id,
        path: format!("/m/{id}").into(),
        fingerprint: [0; 32],
        size: 1,
        title: format!("Track {id}"),
        artist: Some("Artist".into()),
        album: None,
        genre: None,
        sample_rate: 44_100,
        duration_frames: Some(44_100 * 61 + 5),
        bpm: None,
        first_beat_frames: None,
        key: None,
        cue_frames: None,
        hotcues: Default::default(),
        kind,
        lead_in: LeadIn::default(),
    }
}

fn planned(t: ExportTrack, stick_path: &str) -> PlannedTrack {
    PlannedTrack {
        track: t,
        stick_path: stick_path.into(),
        action: TrackAction::Copy,
    }
}

fn plan(stick: &Path, tracks: Vec<PlannedTrack>, playlists: Vec<ExportPlaylist>) -> ExportPlan {
    ExportPlan {
        stick: stick.to_path_buf(),
        tracks,
        playlists,
        skipped: Vec::new(),
        copy_bytes: 0,
        free_bytes: 0,
        without_serato_cues: 0,
        stale_tracks: Vec::new(),
        previous: None,
    }
}

/// Creates the copy a plan entry points at, with `len` bytes.
fn make_copy(stick: &Path, stick_path: &str, len: usize) {
    let path = stick_path
        .split('/')
        .fold(stick.to_path_buf(), |p, part| p.join(part));
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, vec![0u8; len]).unwrap();
}

type Attrs = HashMap<String, String>;

/// Every start or empty element in document order: name and unescaped attributes.
fn elements(text: &str) -> Vec<(String, Attrs)> {
    let mut reader = Reader::from_str(text);
    let mut out = Vec::new();
    loop {
        match reader.read_event().unwrap() {
            Event::Start(e) | Event::Empty(e) => {
                let attrs = e
                    .attributes()
                    .map(|a| {
                        let a = a.unwrap();
                        (
                            a.key.as_ref().to_string(),
                            a.normalized_value(quick_xml::XmlVersion::Implicit1_0)
                                .unwrap()
                                .into_owned(),
                        )
                    })
                    .collect();
                out.push((e.name().as_ref().to_string(), attrs));
            }
            Event::Eof => break,
            _ => {}
        }
    }
    out
}

fn render(p: &ExportPlan) -> String {
    xml(&StickContext {
        plan: p,
        now_unix: 0,
    })
}

#[test]
fn collection_and_playlist_order() {
    let stick = tempfile::tempdir().unwrap();
    let p = plan(
        stick.path(),
        vec![
            planned(track(10, FileKind::Mp3), "Open Jockey/Music/A/a.mp3"),
            planned(track(20, FileKind::Wav), "Open Jockey/Music/A/b.wav"),
        ],
        vec![ExportPlaylist {
            name: "Set".into(),
            tracks: vec![20, 10, 20],
        }],
    );
    make_copy(stick.path(), "Open Jockey/Music/A/a.mp3", 123);
    make_copy(stick.path(), "Open Jockey/Music/A/b.wav", 7);
    let els = elements(&render(&p));
    let root = &els[0];
    assert_eq!(root.0, "DJ_PLAYLISTS");
    assert_eq!(root.1["Version"], "1.0.0");
    let collection = els.iter().find(|e| e.0 == "COLLECTION").unwrap();
    assert_eq!(collection.1["Entries"], "2");
    let tracks: Vec<&Attrs> = els
        .iter()
        .filter(|e| e.0 == "TRACK" && e.1.contains_key("TrackID"))
        .map(|e| &e.1)
        .collect();
    assert_eq!(tracks[0]["TrackID"], "1");
    assert_eq!(tracks[0]["Kind"], "MP3 File");
    assert_eq!(tracks[0]["Size"], "123");
    assert_eq!(tracks[0]["TotalTime"], "61");
    assert_eq!(tracks[1]["TrackID"], "2");
    assert_eq!(tracks[1]["Kind"], "WAV File");
    let node = els
        .iter()
        .find(|e| e.0 == "NODE" && e.1.get("Name").is_some_and(|n| n == "Set"))
        .unwrap();
    assert_eq!(node.1["Entries"], "3");
    assert_eq!(node.1["Type"], "1");
    let keys: Vec<&str> = els
        .iter()
        .filter(|e| e.0 == "TRACK" && e.1.contains_key("Key"))
        .map(|e| e.1["Key"].as_str())
        .collect();
    assert_eq!(keys, ["2", "1", "2"]);
}

#[test]
fn marks_have_types_and_numbers() {
    let stick = tempfile::tempdir().unwrap();
    let mut t = track(1, FileKind::Wav);
    t.cue_frames = Some(0.25 * 44_100.0);
    t.hotcues[0] = Some(HotcuePoint {
        frames: 1.5 * 44_100.0,
        loop_frames: None,
        color: Some(0x102030),
        name: Some("Drop".into()),
    });
    t.hotcues[2] = Some(HotcuePoint {
        frames: 4.0 * 44_100.0,
        loop_frames: Some(2.0 * 44_100.0),
        color: None,
        name: None,
    });
    let p = plan(
        stick.path(),
        vec![planned(t, "Open Jockey/Music/A/a.wav")],
        Vec::new(),
    );
    let els = elements(&render(&p));
    let marks: Vec<&Attrs> = els
        .iter()
        .filter(|e| e.0 == "POSITION_MARK")
        .map(|e| &e.1)
        .collect();
    assert_eq!(marks.len(), 3);
    let cue = marks.iter().find(|m| m["Num"] == "-1").unwrap();
    assert_eq!(
        (cue["Type"].as_str(), cue["Start"].as_str()),
        ("0", "0.250")
    );
    let hot = marks.iter().find(|m| m["Num"] == "0").unwrap();
    assert_eq!(hot["Type"], "0");
    assert_eq!(hot["Start"], "1.500");
    assert_eq!(hot["Name"], "Drop");
    assert_eq!(
        (
            hot["Red"].as_str(),
            hot["Green"].as_str(),
            hot["Blue"].as_str()
        ),
        ("16", "32", "48")
    );
    let lp = marks.iter().find(|m| m["Num"] == "2").unwrap();
    assert_eq!(lp["Type"], "4");
    assert_eq!(lp["Start"], "4.000");
    assert_eq!(lp["End"], "6.000");
    // No colour of its own: slot 3's Serato default, 0x0000CC.
    assert_eq!(
        (
            lp["Red"].as_str(),
            lp["Green"].as_str(),
            lp["Blue"].as_str()
        ),
        ("0", "0", "204")
    );
}

#[test]
fn tempo_from_grid() {
    let stick = tempfile::tempdir().unwrap();
    let mut with = track(1, FileKind::Mp3);
    with.bpm = Some(128.0);
    with.first_beat_frames = Some(0.1 * 44_100.0);
    let without = track(2, FileKind::Mp3);
    let p = plan(
        stick.path(),
        vec![
            planned(with, "Open Jockey/Music/A/a.mp3"),
            planned(without, "Open Jockey/Music/A/b.mp3"),
        ],
        Vec::new(),
    );
    let text = render(&p);
    assert!(
        text.contains(r#"<TEMPO Inizio="0.100" Bpm="128.00" Metro="4/4" Battito="1"/>"#),
        "{text}"
    );
    assert_eq!(text.matches("<TEMPO").count(), 1);
    let els = elements(&text);
    let first = els.iter().find(|e| e.0 == "TRACK").unwrap();
    assert_eq!(first.1["AverageBpm"], "128.00");
}

#[test]
fn escapes_and_encodes_names() {
    let stick = tempfile::tempdir().unwrap();
    let mut t = track(1, FileKind::Mp3);
    t.artist = Some("Tom & \"Jerry\" <3".into());
    let sp = "Open Jockey/Music/Tom & _Jerry_ _3/Café 🎵.mp3";
    let p = plan(stick.path(), vec![planned(t, sp)], Vec::new());
    let els = elements(&render(&p));
    let track = els.iter().find(|e| e.0 == "TRACK").unwrap();
    assert_eq!(track.1["Artist"], "Tom & \"Jerry\" <3");
    assert!(
        track.1["Location"].ends_with(
            "/Open%20Jockey/Music/Tom%20%26%20_Jerry_%20_3/Caf%C3%A9%20%F0%9F%8E%B5.mp3"
        ),
        "{}",
        track.1["Location"]
    );
    assert!(track.1["Location"].starts_with("file://localhost/"));
}

#[cfg(windows)]
#[test]
fn windows_location() {
    let loc = interop::export::rekordbox::location(Path::new(r"E:\"), "Open Jockey/a b.mp3");
    assert_eq!(loc, "file://localhost/E:/Open%20Jockey/a%20b.mp3");
}

#[test]
fn writes_the_file_and_lists_it() {
    let stick = tempfile::tempdir().unwrap();
    let p = plan(
        stick.path(),
        vec![planned(
            track(1, FileKind::Mp3),
            "Open Jockey/Music/A/a.mp3",
        )],
        Vec::new(),
    );
    let out = RekordboxFormat
        .write_library(&StickContext {
            plan: &p,
            now_unix: 0,
        })
        .unwrap();
    assert_eq!(RekordboxFormat.name(), "Rekordbox");
    assert_eq!(out.files, [XML_PATH]);
    let written =
        fs::read_to_string(stick.path().join("Open Jockey").join("rekordbox.xml")).unwrap();
    assert!(written.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>"));
    assert!(written.contains(r#"Name="Open Jockey""#));
}
