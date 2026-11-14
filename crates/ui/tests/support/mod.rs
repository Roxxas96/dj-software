//! What the window tests share: loaded decks to draw, and a render of the whole window.

use std::f64::consts::TAU;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use analysis::Waveform;
use egui::Vec2;
use egui_kittest::Harness;
use engine::{DeckSnapshot, Grid, Hotcue, TrackBuffer};
use library::{BrowserView, Focus, ListRow, TrackRow, TreeKind, TreeRow};
use ui::deck::DeckReads;
use ui::{ControlFrame, DeckInfo, HotcueMark, Layout};

/// Every deck's track: its length and rate, its tempo and where it plays.
const DURATION_FRAMES: u64 = 14_508_900;
const SAMPLE_RATE: u32 = 44_100;
const BPM: f64 = 126.0;
const POSITION_FRAMES: f64 = 2_518_110.0;
/// One beat at [`BPM`], in track frames.
const BEAT_FRAMES: f64 = 60.0 / BPM * SAMPLE_RATE as f64;
/// The waveforms' resolutions, as the library and the session analyse them.
const OVERVIEW_FRAMES_PER_BUCKET: u32 = 256;
const DETAIL_FRAMES_PER_BUCKET: u32 = 128;
/// How many tracks the browser's list shows.
const BROWSER_ROWS: usize = 30;

/// The library row of the browser's `n`th track; deck `n` holds track `n` for the first
/// four.
fn track_row(n: usize) -> TrackRow {
    TrackRow {
        id: n as i64 + 1,
        title: Some(format!("Track Number {} (Extended Mix)", n + 1)),
        artist: Some("Someone".into()),
        bpm: Some(if n < 4 { BPM } else { 118.0 + (n % 11) as f64 }),
        key: Some(if n < 4 { 21 } else { (n * 7 % 24) as u8 }),
        duration_frames: Some(DURATION_FRAMES + n as u64 * 441_000),
        sample_rate: Some(SAMPLE_RATE),
        has_cues: n.is_multiple_of(3),
        missing: false,
    }
}

/// The synthesized loop: eight bars at [`BPM`], a whole number of buckets at both
/// resolutions.
const LOOP_FRAMES: usize = 32 * BEAT_FRAMES as usize;
/// The kick: a 55 Hz sine that dies away over 120 ms.
const KICK_HZ: f64 = 55.0;
const KICK_SECS: f64 = 0.12;
/// A closed hat: 40 ms of noise.
const HAT_SECS: f64 = 0.04;
/// The mid layer's two detuned tones, a saw and a square, changing every two bars.
const MID_HZ: [f64; 4] = [440.0, 523.25, 587.33, 493.88];
const DETUNE: f64 = 1.5;

/// A fixed-seed noise source, so every render draws the same.
struct Noise(u64);

impl Noise {
    /// The next sample, from -1 to 1.
    fn next(&mut self) -> f64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 40) as f64 / f64::from(1u32 << 23) - 1.0
    }
}

/// `blocks` loops of synthesized stereo audio: a kick on every beat (unless `kick` says
/// no for that loop), closed hats on every quarter beat and two detuned mid tones.
fn audio(kick: &[bool]) -> Vec<f32> {
    let rate = f64::from(SAMPLE_RATE);
    let mut noise = Noise(0x5EED);
    let mut previous = 0.0;
    let mut out = Vec::with_capacity(2 * LOOP_FRAMES * kick.len());
    for (block, &kicks) in kick.iter().enumerate() {
        for frame in 0..LOOP_FRAMES {
            let t = (block * LOOP_FRAMES + frame) as f64 / rate;
            let beats = frame as f64 / BEAT_FRAMES;
            let since_beat = beats.fract() * BEAT_FRAMES / rate;
            let kick = if kicks && since_beat < KICK_SECS {
                0.9 * (TAU * KICK_HZ * since_beat).sin() * (-since_beat / 0.04).exp()
            } else {
                0.0
            };
            // White noise, its lows taken off by a first difference.
            let white = noise.next();
            let bright = white - previous;
            previous = white;
            let since_hat = (beats * 4.0).fract() * BEAT_FRAMES / 4.0 / rate;
            let hat = if since_hat < HAT_SECS {
                0.4 * bright * (-since_hat / 0.015).exp()
            } else {
                0.0
            };
            let hz = MID_HZ[(beats / 8.0) as usize % MID_HZ.len()];
            let saw = 2.0 * (t * hz).fract() - 1.0;
            let square = if (t * (hz + DETUNE)).fract() < 0.5 {
                1.0
            } else {
                -1.0
            };
            // Off-beat stabs over a quieter pad.
            let stab = if (beats + 0.5).fract() < 0.25 {
                1.0
            } else {
                0.6
            };
            let mid = 0.22 * stab * (saw + 0.6 * square);
            let sample = (kick + hat + mid) as f32;
            out.extend_from_slice(&[sample, sample]);
        }
    }
    out
}

/// The waveform of `frames_per_bucket` the analyser draws from a made-up 126 BPM track: a
/// kick on every beat, hats on every quarter beat, mids that change every two bars, and a
/// breakdown from 55 % to 65 % of the track with no kick. Three loops are analysed (with
/// the kick, without, with) and the track tiled from the last two, so the analyser's
/// filters have settled and both parts share one level.
fn waveform(frames_per_bucket: u32) -> Waveform {
    let track = TrackBuffer::new(SAMPLE_RATE, audio(&[true, false, true])).expect("the audio");
    let analysed = analysis::waveform(&track, frames_per_bucket);
    let per_loop = LOOP_FRAMES / frames_per_bucket as usize;
    let count = (DURATION_FRAMES / u64::from(frames_per_bucket)) as usize;
    let breakdown = count * 55 / 100..count * 65 / 100;
    let buckets = (0..count)
        .map(|i| {
            let block = if breakdown.contains(&i) { 1 } else { 2 };
            analysed.buckets[block * per_loop + i % per_loop]
        })
        .collect();
    Waveform {
        frames_per_bucket,
        buckets,
    }
}

/// The track's overview and detail waveforms, analysed once for every test of a binary.
fn waveforms() -> &'static (Arc<Waveform>, Arc<Waveform>) {
    static WAVEFORMS: OnceLock<(Arc<Waveform>, Arc<Waveform>)> = OnceLock::new();
    WAVEFORMS.get_or_init(|| {
        (
            Arc::new(waveform(OVERVIEW_FRAMES_PER_BUCKET)),
            Arc::new(waveform(DETAIL_FRAMES_PER_BUCKET)),
        )
    })
}

/// Deck `deck`'s track, with its waveforms and the colours of hotcues 1 to 3.
fn info(deck: usize, overview: &Arc<Waveform>, detail: &Arc<Waveform>) -> DeckInfo {
    let mut marks: [Option<HotcueMark>; 8] = Default::default();
    marks[0] = Some(HotcueMark::new(Some(0xE8_32_2A), None));
    marks[1] = Some(HotcueMark::new(Some(0x3D_8B_FF), None));
    marks[2] = Some(HotcueMark::new(Some(0xFF_D2_3D), None));
    DeckInfo {
        load_seq: 1,
        row: track_row(deck),
        path: PathBuf::from(format!("/music/track-{deck}.mp3")),
        cover_file: None,
        overview: Some(Arc::clone(overview)),
        detail: Some(Arc::clone(detail)),
        stems: Some("stems 40%".into()),
        hotcue_marks: Arc::new(marks),
    }
}

/// Hotcues 1 to 3 around the playhead; the second is a saved 4-beat loop.
fn hotcues() -> [Option<Hotcue>; 8] {
    let mut hotcues = [None; 8];
    hotcues[0] = Some(Hotcue {
        frames: POSITION_FRAMES - 8.0 * BEAT_FRAMES,
        loop_frames: None,
    });
    hotcues[1] = Some(Hotcue {
        frames: POSITION_FRAMES + 4.0 * BEAT_FRAMES,
        loop_frames: Some(4.0 * BEAT_FRAMES),
    });
    hotcues[2] = Some(Hotcue {
        frames: POSITION_FRAMES + 16.0 * BEAT_FRAMES,
        loop_frames: None,
    });
    hotcues
}

/// The library id of the tree's playlist at row `at`.
pub fn playlist_id(at: usize) -> i64 {
    10 + at as i64
}

fn tree_row(label: &str, depth: u8, kind: TreeKind, expanded: bool) -> TreeRow {
    TreeRow {
        label: label.into(),
        depth,
        kind,
        expanded,
        note: None,
        playlist: None,
        path: None,
    }
}

/// The browser on a playlist of 30 tracks, deck 1's first and selected; the tree shows
/// the drives and the playlists.
fn browser() -> BrowserView {
    let list: Vec<ListRow> = (0..BROWSER_ROWS)
        .map(|n| ListRow {
            path: PathBuf::from(format!("/music/track-{n}.mp3")),
            track: Some(track_row(n)),
            file_name: format!("track-{n}.mp3"),
            position: Some(n),
            cover_file: None,
        })
        .collect();
    let mut tree = vec![
        tree_row("Macintosh HD", 0, TreeKind::Drive, false),
        tree_row("USB STICK", 0, TreeKind::Drive, false),
        tree_row("Playlists", 0, TreeKind::Playlists, true),
        tree_row("Warm up", 1, TreeKind::Playlist, false),
        tree_row("Peak time", 1, TreeKind::Playlist, false),
        tree_row("Closing", 1, TreeKind::Playlist, false),
    ];
    for (at, row) in tree.iter_mut().enumerate().skip(3) {
        row.playlist = Some(playlist_id(at));
    }
    BrowserView {
        path: "Playlists ▸ Peak time".into(),
        index: 0,
        count: BROWSER_ROWS,
        track: Some(track_row(0)),
        focus: Focus::List,
        tree,
        tree_cursor: 4,
        list,
        list_offset: 0,
        list_count: BROWSER_ROWS,
        all_selected: false,
        list_cursor: 0,
        search: None,
        sort: None,
        seq: 0,
        playlist: Some(playlist_id(4)),
    }
}

/// Two decks (four with `four_decks`) loaded and playing at 126 BPM, with a key, a grid
/// (so the beat counter shows), hotcues 1 to 3 and both waveforms; no covers. The browser
/// shows a playlist with deck 1's track in it.
pub fn loaded(layout: Layout, four_decks: bool) -> (ControlFrame, DeckReads) {
    let mut frame = ControlFrame::default();
    frame.view.layout = layout;
    frame.view.four_decks = four_decks;
    frame.view.beat_counter = true;
    frame.browser = Some(browser());
    let (overview, detail) = waveforms();
    let mut reads = DeckReads::default();
    let decks = if four_decks { 4 } else { 2 };
    for deck in 0..decks {
        frame.decks[deck] = Some(info(deck, overview, detail));
        reads.decks[deck].0 = DeckSnapshot {
            loaded: true,
            playing: true,
            position_frames: POSITION_FRAMES,
            duration_frames: DURATION_FRAMES as usize,
            track_sample_rate: SAMPLE_RATE,
            tempo_ratio: 1.0,
            grid: Grid::from_bpm(BPM, 0.0, SAMPLE_RATE),
            hotcues: hotcues(),
            load_seq: 1,
            ..DeckSnapshot::default()
        };
    }
    (frame, reads)
}

/// The whole window drawn for `frame` and `reads` at `size`, on the GPU.
pub fn render(frame: ControlFrame, reads: DeckReads, size: Vec2) -> image::RgbaImage {
    let mut screen = ui::Screen::default();
    let mut harness = Harness::builder()
        .with_size(size)
        .wgpu()
        .with_context_setup(ui::install_fonts)
        .build_ui(move |ui| {
            // The harness keeps a margin around what it draws; the window has none.
            let window = ui.ctx().content_rect();
            ui.scope_builder(egui::UiBuilder::new().max_rect(window), |ui| {
                ui.set_clip_rect(window);
                ui::root_with_reads(ui, &frame, &reads, &mut screen);
            });
        });
    harness.run_steps(3);
    harness.render().expect("rendering the window")
}
