//! The Rekordbox XML: our tracks, their cues and a playlist for each exported playlist.

use std::collections::HashMap;
use std::io::Cursor;
use std::path::Path;

use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use quick_xml::Writer;
use quick_xml::events::{BytesDecl, Event};

use super::run::{FormatOutput, StickContext, StickFormat};
use super::serato::markers::SLOT_DEFAULT_COLORS;
use super::timing::{App, app_seconds};
use super::{ExportError, ExportTrack, FileKind, files};
use library::HotcuePoint;

/// Where the XML goes on the stick, relative to its root.
pub const XML_PATH: &str = "Open Jockey/rekordbox.xml";

/// What stays as is in a path: `/ - . _ ~` and letters and digits.
const PATH_SET: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'/')
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

/// Writes `Open Jockey/rekordbox.xml`, which Rekordbox imports from its "rekordbox xml" view.
#[derive(Debug, Clone, Copy, Default)]
pub struct RekordboxFormat;

impl StickFormat for RekordboxFormat {
    fn name(&self) -> &'static str {
        "Rekordbox"
    }

    fn write_library(&self, cx: &StickContext<'_>) -> Result<FormatOutput, ExportError> {
        let stick = cx.plan.stick.as_path();
        let path = XML_PATH
            .split('/')
            .fold(stick.to_path_buf(), |p, part| p.join(part));
        files::write_atomic(stick, &path, xml(cx).as_bytes())?;
        Ok(FormatOutput {
            files: vec![XML_PATH.to_string()],
            notes: Vec::new(),
        })
    }
}

/// The `Location` of a file on the stick: a `file://localhost` URL with the absolute path,
/// `/` separators and percent-encoding (on Windows the path starts with the drive letter).
#[must_use]
pub fn location(stick: &Path, stick_path: &str) -> String {
    let full = stick_path
        .split('/')
        .fold(stick.to_path_buf(), |p, part| p.join(part));
    let text = full.to_string_lossy();
    // A backslash is a separator on Windows only; elsewhere it is part of a name.
    let text = if cfg!(windows) {
        text.replace('\\', "/")
    } else {
        text.into_owned()
    };
    let encoded: Vec<String> = text
        .split('/')
        .enumerate()
        .map(|(i, part)| {
            if i <= 1 && is_drive(part) {
                part.to_string()
            } else {
                utf8_percent_encode(part, PATH_SET).to_string()
            }
        })
        .collect();
    let path = encoded.join("/");
    if path.starts_with('/') {
        format!("file://localhost{path}")
    } else {
        format!("file://localhost/{path}")
    }
}

/// Whether `part` is a Windows drive such as `E:`.
fn is_drive(part: &str) -> bool {
    let b = part.as_bytes();
    b.len() == 2 && b[0].is_ascii_alphabetic() && b[1] == b':'
}

fn secs(s: f64) -> String {
    format!("{:.3}", s.max(0.0))
}

fn kind(k: &FileKind) -> String {
    match k {
        FileKind::Mp3 => "MP3 File".into(),
        FileKind::Wav => "WAV File".into(),
        FileKind::Other(ext) if ext.is_empty() => "File".into(),
        FileKind::Other(ext) => format!("{} File", ext.to_uppercase()),
    }
}

/// The hotcue's RGB, or its slot's default colour.
fn rgb(point: &HotcuePoint, slot: usize) -> [String; 3] {
    let c = point.color.unwrap_or(SLOT_DEFAULT_COLORS[slot]);
    [(c >> 16) & 0xFF, (c >> 8) & 0xFF, c & 0xFF].map(|v| v.to_string())
}

type Attrs<'a> = Vec<(&'a str, String)>;
type XmlWriter = Writer<Cursor<Vec<u8>>>;

/// The Rekordbox XML for `cx.plan`: a collection in plan order, and the playlists.
///
/// # Panics
///
/// Never in practice: it writes to memory.
#[must_use]
pub fn xml(cx: &StickContext<'_>) -> String {
    let plan = cx.plan;
    let mut w = Writer::new_with_indent(Cursor::new(Vec::new()), b' ', 2);
    w.write_event(Event::Decl(BytesDecl::new("1.0", Some("UTF-8"), None)))
        .expect("writing to memory cannot fail");

    let ids: HashMap<i64, usize> = plan
        .tracks
        .iter()
        .enumerate()
        .map(|(i, t)| (t.track.id, i + 1))
        .collect();
    let element = |w: &mut XmlWriter, name: &str, attrs: &Attrs<'_>| {
        w.create_element(name)
            .with_attributes(attrs.iter().map(|(k, v)| (*k, v.as_str())))
            .write_empty()
            .expect("writing to memory cannot fail");
    };
    let open = |w: &mut XmlWriter,
                name: &str,
                attrs: &Attrs<'_>,
                inner: &mut dyn FnMut(&mut XmlWriter)| {
        w.create_element(name)
            .with_attributes(attrs.iter().map(|(k, v)| (*k, v.as_str())))
            .write_inner_content(|w| {
                inner(w);
                Ok::<(), std::io::Error>(())
            })
            .expect("writing to memory cannot fail");
    };

    open(
        &mut w,
        "DJ_PLAYLISTS",
        &vec![("Version", "1.0.0".into())],
        &mut |w| {
            element(
                w,
                "PRODUCT",
                &vec![
                    ("Name", "Open Jockey".into()),
                    ("Version", env!("CARGO_PKG_VERSION").into()),
                    ("Company", String::new()),
                ],
            );
            open(
                w,
                "COLLECTION",
                &vec![("Entries", plan.tracks.len().to_string())],
                &mut |w| {
                    for (i, planned) in plan.tracks.iter().enumerate() {
                        let size = std::fs::symlink_metadata(
                            planned
                                .stick_path
                                .split('/')
                                .fold(plan.stick.clone(), |p, part| p.join(part)),
                        )
                        .ok()
                        .map(|m| m.len());
                        let attrs = track_attrs(i + 1, planned, &plan.stick, size);
                        open(w, "TRACK", &attrs, &mut |w| {
                            for (name, attrs) in track_children(&planned.track) {
                                element(w, name, &attrs);
                            }
                        });
                    }
                },
            );
            open(w, "PLAYLISTS", &Vec::new(), &mut |w| {
                open(
                    w,
                    "NODE",
                    &vec![
                        ("Type", "0".into()),
                        ("Name", "ROOT".into()),
                        ("Count", plan.playlists.len().to_string()),
                    ],
                    &mut |w| {
                        for playlist in &plan.playlists {
                            let keys: Vec<usize> = playlist
                                .tracks
                                .iter()
                                .filter_map(|id| ids.get(id).copied())
                                .collect();
                            let attrs = vec![
                                ("Name", playlist.name.clone()),
                                ("Type", "1".into()),
                                ("KeyType", "0".into()),
                                ("Entries", keys.len().to_string()),
                            ];
                            open(w, "NODE", &attrs, &mut |w| {
                                for key in &keys {
                                    element(w, "TRACK", &vec![("Key", key.to_string())]);
                                }
                            });
                        }
                    },
                );
            });
        },
    );
    let mut text = String::from_utf8(w.into_inner().into_inner()).unwrap_or_default();
    text.push('\n');
    text
}

fn track_attrs<'a>(
    id: usize,
    planned: &super::PlannedTrack,
    stick: &Path,
    size: Option<u64>,
) -> Attrs<'a> {
    let t = &planned.track;
    let mut a: Attrs<'a> = vec![("TrackID", id.to_string()), ("Name", t.title.clone())];
    let optional = [
        ("Artist", &t.artist),
        ("Album", &t.album),
        ("Genre", &t.genre),
    ];
    for (key, value) in optional {
        if let Some(v) = value {
            a.push((key, v.clone()));
        }
    }
    a.push(("Kind", kind(&t.kind)));
    if let Some(size) = size {
        a.push(("Size", size.to_string()));
    }
    if let (Some(frames), rate) = (t.duration_frames, u64::from(t.sample_rate))
        && rate > 0
    {
        a.push(("TotalTime", (frames / rate).to_string()));
    }
    if let Some(bpm) = t.bpm.filter(|b| b.is_finite()) {
        a.push(("AverageBpm", format!("{bpm:.2}")));
    }
    if t.sample_rate > 0 {
        a.push(("SampleRate", t.sample_rate.to_string()));
    }
    if let Some(key) = &t.key {
        a.push(("Tonality", key.clone()));
    }
    a.push(("Location", location(stick, &planned.stick_path)));
    a
}

/// The `TEMPO` and `POSITION_MARK` elements of one track.
fn track_children(t: &ExportTrack) -> Vec<(&'static str, Attrs<'static>)> {
    let at = |frames: f64| secs(app_seconds(App::Rekordbox, frames, t));
    let mut out = Vec::new();
    if let (Some(bpm), Some(first)) = (t.bpm.filter(|b| b.is_finite()), t.first_beat_frames) {
        out.push((
            "TEMPO",
            vec![
                ("Inizio", at(first)),
                ("Bpm", format!("{bpm:.2}")),
                ("Metro", "4/4".into()),
                ("Battito", "1".into()),
            ],
        ));
    }
    if let Some(cue) = t.cue_frames {
        out.push((
            "POSITION_MARK",
            vec![
                ("Name", String::new()),
                ("Type", "0".into()),
                ("Start", at(cue)),
                ("Num", "-1".into()),
            ],
        ));
    }
    for (slot, point) in t.hotcues.iter().enumerate() {
        let Some(point) = point else { continue };
        let mut a: Attrs<'static> = vec![
            ("Name", point.name.clone().unwrap_or_default()),
            (
                "Type",
                if point.loop_frames.is_some() {
                    "4"
                } else {
                    "0"
                }
                .into(),
            ),
            ("Start", at(point.frames)),
        ];
        if let Some(len) = point.loop_frames {
            a.push(("End", at(point.frames + len)));
        }
        a.push(("Num", slot.to_string()));
        let [r, g, b] = rgb(point, slot);
        a.push(("Red", r));
        a.push(("Green", g));
        a.push(("Blue", b));
        out.push(("POSITION_MARK", a));
    }
    out
}
