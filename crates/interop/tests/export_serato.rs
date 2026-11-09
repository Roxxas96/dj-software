//! Serato tags on copies, and the Serato crates and database on the stick.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use decode::LeadIn;
use interop::export::serato::SeratoFormat;
use interop::export::serato::markers::{beatgrid_geob, markers2_geob};
use interop::export::serato::tags::{needs_tagging, wanted_frames, write_tags};
use interop::export::serato::tlv::{Body, Record, decode};
use interop::export::{
    ExportPlan, ExportPlaylist, ExportTrack, FileKind, Manifest, PlannedTrack, StickContext,
    StickFormat, TrackAction, copy_atomic,
};
use library::HotcuePoint;
use lofty::TextEncoding;
use lofty::config::{ParseOptions, WriteOptions};
use lofty::file::AudioFile;
use lofty::id3::v2::{
    AttachedPictureFrame, BinaryFrame, Frame, FrameId, GeneralEncapsulatedObject, Id3v2Tag,
};
use lofty::iff::wav::WavFile;
use lofty::mpeg::MpegFile;
use lofty::picture::{MimeType, Picture, PictureType};

fn write_wav(path: &Path) {
    let frames = 4_410u32;
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
        let v: i16 = if i % 100 < 50 { 1000 } else { -1000 };
        bytes.extend_from_slice(&v.to_le_bytes());
    }
    fs::write(path, bytes).unwrap();
}

/// An MP3 made by ffmpeg, or `None` (with a note) when ffmpeg is not installed.
fn write_mp3(path: &Path) -> bool {
    let ok = Command::new("ffmpeg")
        .args(["-v", "quiet", "-y", "-f", "lavfi", "-i"])
        .arg("sine=frequency=440:duration=1")
        .arg(path)
        .status()
        .is_ok_and(|s| s.success());
    if !ok {
        eprintln!("skipped: ffmpeg is not available to make an MP3");
    }
    ok
}

fn track(id: i64, path: &Path, kind: FileKind) -> ExportTrack {
    let mut hotcues: [Option<HotcuePoint>; 8] = Default::default();
    hotcues[0] = Some(HotcuePoint {
        frames: 22_050.0,
        loop_frames: None,
        color: Some(0xCC0000),
        name: Some("Drop".into()),
    });
    ExportTrack {
        id,
        path: path.into(),
        fingerprint: [0; 32],
        size: 1,
        title: format!("Track {id}"),
        artist: Some("Artist".into()),
        album: None,
        genre: None,
        sample_rate: 44_100,
        duration_frames: None,
        bpm: Some(128.0),
        first_beat_frames: Some(1_000.0),
        key: None,
        cue_frames: None,
        hotcues,
        kind,
        lead_in: LeadIn::default(),
    }
}

fn id3_of(path: &Path, kind: &FileKind) -> Option<Id3v2Tag> {
    let mut file = fs::File::open(path).unwrap();
    match kind {
        FileKind::Mp3 => MpegFile::read_from(&mut file, ParseOptions::new())
            .unwrap()
            .id3v2()
            .cloned(),
        _ => WavFile::read_from(&mut file, ParseOptions::new())
            .unwrap()
            .id3v2()
            .cloned(),
    }
}

/// MIME type, file name and data of a GEOB frame.
type Geob = (Option<String>, Option<String>, Vec<u8>);

/// Every GEOB frame: description to (MIME, file name, data).
fn geobs(path: &Path, kind: &FileKind) -> BTreeMap<String, Geob> {
    let Some(tag) = id3_of(path, kind) else {
        return BTreeMap::new();
    };
    tag.iter()
        .filter_map(|f| match f {
            Frame::Binary(b) if b.id().as_str() == "GEOB" => {
                let g = GeneralEncapsulatedObject::parse(&b.data, b.flags()).unwrap();
                Some((
                    g.descriptor.unwrap_or_default(),
                    (g.mime_type, g.file_name, g.data),
                ))
            }
            _ => None,
        })
        .collect()
}

fn geob_frame(description: &str, data: Vec<u8>) -> Frame<'static> {
    let g = GeneralEncapsulatedObject::new(
        TextEncoding::Latin1,
        Some("application/octet-stream".into()),
        Some(String::new()),
        Some(description.into()),
        data,
    );
    Frame::Binary(BinaryFrame::new(
        FrameId::new("GEOB").unwrap(),
        g.as_bytes(WriteOptions::default()).unwrap(),
    ))
}

#[test]
fn tags_written_to_wav_copy_only() {
    let dir = tempfile::tempdir().unwrap();
    let original = dir.path().join("a.wav");
    write_wav(&original);
    let snapshot = fs::read(&original).unwrap();
    let copy = dir.path().join("copy.wav");
    fs::copy(&original, &copy).unwrap();
    let t = track(1, &original, FileKind::Wav);
    write_tags(&copy, &t).unwrap();
    let found = geobs(&copy, &FileKind::Wav);
    let (mime, name, data) = &found["Serato Markers2"];
    assert_eq!(data, &markers2_geob(&t));
    assert_eq!(mime.as_deref(), Some("application/octet-stream"));
    assert_eq!(name, &None);
    assert_eq!(found["Serato BeatGrid"].2, beatgrid_geob(&t).unwrap());
    assert_eq!(fs::read(&original).unwrap(), snapshot);
}

#[test]
fn tags_written_to_mp3_copy() {
    let dir = tempfile::tempdir().unwrap();
    let original = dir.path().join("a.mp3");
    if !write_mp3(&original) {
        return;
    }
    let snapshot = fs::read(&original).unwrap();
    let copy = dir.path().join("copy.mp3");
    fs::copy(&original, &copy).unwrap();
    let t = track(1, &original, FileKind::Mp3);
    write_tags(&copy, &t).unwrap();
    let found = geobs(&copy, &FileKind::Mp3);
    assert_eq!(found["Serato Markers2"].2, markers2_geob(&t));
    assert_eq!(found["Serato BeatGrid"].2, beatgrid_geob(&t).unwrap());
    assert!(!needs_tagging(&copy, &t));
    assert_eq!(fs::read(&original).unwrap(), snapshot);
}

#[test]
fn old_serato_frames_are_replaced() {
    let dir = tempfile::tempdir().unwrap();
    let copy = dir.path().join("copy.wav");
    write_wav(&copy);
    let mut tag = Id3v2Tag::new();
    tag.insert(geob_frame("Serato Markers_", vec![1, 2, 3]));
    tag.insert(geob_frame("Other Tool", vec![9, 9]));
    tag.insert(Frame::Picture(AttachedPictureFrame::new(
        TextEncoding::Latin1,
        Picture::unchecked(vec![0x89, b'P', b'N', b'G'])
            .pic_type(PictureType::CoverFront)
            .mime_type(MimeType::Png)
            .build(),
    )));
    let mut file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&copy)
        .unwrap();
    let mut wav = WavFile::read_from(&mut file, ParseOptions::new()).unwrap();
    wav.set_id3v2(tag);
    wav.save_to(&mut file, WriteOptions::default()).unwrap();
    drop(file);

    let t = track(1, &copy, FileKind::Wav);
    write_tags(&copy, &t).unwrap();
    let found = geobs(&copy, &FileKind::Wav);
    assert!(!found.contains_key("Serato Markers_"));
    assert!(found.contains_key("Serato Markers2"));
    assert_eq!(found["Other Tool"].2, vec![9, 9]);
    let tag = id3_of(&copy, &FileKind::Wav).unwrap();
    assert!(tag.iter().any(|f| matches!(f, Frame::Picture(_))));
}

#[test]
fn needs_tagging_only_when_cues_change() {
    let dir = tempfile::tempdir().unwrap();
    let copy = dir.path().join("copy.wav");
    write_wav(&copy);
    let mut t = track(1, &copy, FileKind::Wav);
    assert!(needs_tagging(&copy, &t));
    write_tags(&copy, &t).unwrap();
    assert!(!needs_tagging(&copy, &t));
    t.hotcues[0].as_mut().unwrap().frames = 30_000.0;
    assert!(needs_tagging(&copy, &t));
    // A different set of frames also counts.
    t.hotcues[0].as_mut().unwrap().frames = 22_050.0;
    t.bpm = None;
    assert!(needs_tagging(&copy, &t));
}

#[test]
fn flac_is_left_alone() {
    let dir = tempfile::tempdir().unwrap();
    let copy = dir.path().join("copy.flac");
    fs::write(&copy, b"fLaC not really").unwrap();
    let t = track(1, &copy, FileKind::Other("flac".into()));
    assert!(wanted_frames(&t).is_empty());
    assert!(!needs_tagging(&copy, &t));
    write_tags(&copy, &t).unwrap();
    assert_eq!(fs::read(&copy).unwrap(), b"fLaC not really");
}

#[test]
fn read_only_source_copies_and_tags_and_stays_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let stick = dir.path().join("stick");
    fs::create_dir(&stick).unwrap();
    let original = dir.path().join("a.wav");
    write_wav(&original);
    let snapshot = fs::read(&original).unwrap();
    let mut perms = fs::metadata(&original).unwrap().permissions();
    perms.set_readonly(true);
    fs::set_permissions(&original, perms).unwrap();
    let t = track(1, &original, FileKind::Wav);
    let to = stick.join("Open Jockey").join("Music").join("a.wav");
    copy_atomic(&stick, &original, &to, |part| write_tags(part, &t)).unwrap();
    assert!(geobs(&to, &FileKind::Wav).contains_key("Serato Markers2"));
    assert!(fs::metadata(&original).unwrap().permissions().readonly());
    assert_eq!(fs::read(&original).unwrap(), snapshot);
}

fn utf16(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_be_bytes(*c))
        .collect();
    String::from_utf16(&units).unwrap()
}

fn field<'a>(record: &'a Record, tag: &[u8; 4]) -> Option<&'a Record> {
    match &record.body {
        Body::Records(fields) => fields.iter().find(|f| &f.tag == tag),
        Body::Bytes(_) => None,
    }
}

fn texts(bytes: &[u8], container: &[u8; 4], tag: &[u8; 4]) -> Vec<String> {
    decode(bytes)
        .unwrap()
        .iter()
        .filter(|r| &r.tag == container)
        .filter_map(|r| field(r, tag))
        .map(|f| match &f.body {
            Body::Bytes(b) => utf16(b),
            Body::Records(_) => panic!("text field"),
        })
        .collect()
}

fn plan(stick: &Path, names: &[(&str, &[i64])], previous: Option<Manifest>) -> ExportPlan {
    let planned = |id: i64, name: &str| PlannedTrack {
        track: track(id, Path::new(name), FileKind::Wav),
        stick_path: format!("Open Jockey/Music/Artist/{name}"),
        action: TrackAction::Copy,
    };
    ExportPlan {
        stick: stick.to_path_buf(),
        tracks: vec![planned(1, "a.wav"), planned(2, "b.wav")],
        playlists: names
            .iter()
            .map(|(n, ids)| ExportPlaylist {
                name: (*n).into(),
                tracks: ids.to_vec(),
            })
            .collect(),
        skipped: Vec::new(),
        copy_bytes: 0,
        free_bytes: 0,
        without_serato_cues: 0,
        stale_tracks: Vec::new(),
        previous,
    }
}

fn stick() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let stick = dir.path().join("stick");
    fs::create_dir(&stick).unwrap();
    (dir, stick)
}

#[test]
fn serato_format_writes_crates_in_order() {
    let (_dir, stick) = stick();
    let plan = plan(&stick, &[("Set A", &[1, 2, 1]), ("B/C", &[2])], None);
    let format = SeratoFormat;
    assert_eq!(format.name(), "Serato");
    let out = format
        .write_library(&StickContext {
            plan: &plan,
            now_unix: 1_700_000_000,
        })
        .unwrap();
    assert_eq!(out.files, vec!["Set A.crate", "B_C.crate"]);
    let subcrates = stick.join("_Serato_").join("Subcrates");
    let set_a = fs::read(subcrates.join("Set A.crate")).unwrap();
    assert_eq!(
        texts(&set_a, b"otrk", b"ptrk"),
        [
            "Open Jockey/Music/Artist/a.wav",
            "Open Jockey/Music/Artist/b.wav",
            "Open Jockey/Music/Artist/a.wav"
        ]
    );
    assert!(subcrates.join("B_C.crate").is_file());
    let db = fs::read(stick.join("_Serato_").join("database V2")).unwrap();
    assert_eq!(
        decode(&db)
            .unwrap()
            .iter()
            .filter(|r| &r.tag == b"otrk")
            .count(),
        2
    );
}

#[test]
fn foreign_crate_name_is_kept() {
    let (_dir, stick) = stick();
    let subcrates = stick.join("_Serato_").join("Subcrates");
    fs::create_dir_all(&subcrates).unwrap();
    fs::write(subcrates.join("Set A.crate"), b"friend's crate").unwrap();
    let plan = plan(&stick, &[("Set A", &[1])], None);
    let out = SeratoFormat
        .write_library(&StickContext {
            plan: &plan,
            now_unix: 1,
        })
        .unwrap();
    assert_eq!(out.files, vec!["Set A (Open Jockey).crate"]);
    assert_eq!(
        fs::read(subcrates.join("Set A.crate")).unwrap(),
        b"friend's crate"
    );
    assert!(subcrates.join("Set A (Open Jockey).crate").is_file());
}

#[test]
fn own_crate_from_the_manifest_is_rewritten_and_foreign_database_records_stay() {
    let (_dir, stick) = stick();
    let serato = stick.join("_Serato_");
    fs::create_dir_all(serato.join("Subcrates")).unwrap();
    fs::write(serato.join("Subcrates").join("Set A.crate"), b"old").unwrap();
    let theirs = interop::export::serato::tlv::encode(&[
        interop::export::serato::tlv::text("vrsn", "2.0/Serato Scratch LIVE Database"),
        interop::export::serato::tlv::decode(&interop::export::serato::tlv::crate_file(&["x.mp3"]))
            .unwrap()
            .pop()
            .unwrap(),
    ]);
    fs::write(serato.join("database V2"), &theirs).unwrap();
    let previous = Manifest {
        version: 1,
        playlists: vec!["Set A".into()],
        crates: vec!["Set A.crate".into()],
        vdj_lists: vec![],
        tracks: vec![],
    };
    let plan = plan(&stick, &[("Set A", &[1])], Some(previous));
    let out = SeratoFormat
        .write_library(&StickContext {
            plan: &plan,
            now_unix: 1,
        })
        .unwrap();
    assert_eq!(out.files, vec!["Set A.crate"]);
    assert_ne!(
        fs::read(serato.join("Subcrates").join("Set A.crate")).unwrap(),
        b"old"
    );
    let db = decode(&fs::read(serato.join("database V2")).unwrap()).unwrap();
    // version, their record, and our two tracks
    assert_eq!(db.len(), 4);
}
