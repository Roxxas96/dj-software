//! The deck panels between the waveforms and the browser: the left side's active deck on
//! the left, the right side's on the right, each drawn from a [`DeckView`].

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use actions::{
    Action, ControlState, HardwarePadMode, Input, PadMode, Side, SideControl, window_pad_action,
};
use analysis::{Key, KeyNotation, Waveform};
use egui::{
    Align, Color32, CornerRadius, Id, Label, Layout, Margin, Mesh, Rect, RichText, Role, Sense,
    Shape, Stroke, StrokeKind, TextureHandle, Ui, UiBuilder, Vec2, WidgetInfo, pos2, vec2,
};
use engine::{
    ChannelSnapshot, DeckId, DeckSnapshot, EffectKind, EngineMonitor, Grid, Hotcue, HotcueSlot,
    Meters, SAMPLER_SLOTS, SamplerSnapshot,
};

use crate::command::{LibraryStep, UiCommand, ViewChange};
use crate::controls::{
    Handle, Sink, carry_steps, context_press, drag_path, fader_input, grab_fader, moved_drag,
};
use crate::cover::{self, Covers};
use crate::hotcue_menu::{self, HotcueTarget};
use crate::layout::{Sections, deck_slots};
use crate::mixer;
use crate::model::{ControlFrame, GHOST_FX_LEVEL, GHOST_TEMPO, HotcueMark};
use crate::theme::{self, ButtonLook, EDGE_LABEL_WIDTH, SECTION_GAP, edge_label};
use crate::view::{JogDisplay, TimeMode, WaveformPalette};
use crate::waveform::{Playhead, band_colours, layers, loudest};

/// What each deck's panel is called for screen readers, by deck index.
const PANEL_NAMES: [&str; 4] = [
    "deck panel 1",
    "deck panel 2",
    "deck panel 3",
    "deck panel 4",
];

/// The brightness below which a lit box takes white text instead of dark.
const DARK_FILL: f32 = 0.35;
/// Points from the jog's centre within which a drag does not turn it.
const JOG_DEAD_ZONE: f32 = 8.0;
/// A record's speed: the jog's marker turns at this at the original tempo.
const JOG_RPM: f64 = 100.0 / 3.0;
/// The share of the panels' width each deck panel takes; the mixer has the rest.
const PANEL_SHARE: f32 = 0.38;
/// The same in the 4-Decks view, where the mixer has four strips to fit.
const FOUR_PANEL_SHARE: f32 = 0.34;
/// The width of the accent tab on the outer edge of a side's active deck.
const ACCENT_TAB: f32 = 4.0;
/// A panel's margin at its sides: room for the accent tab and a little more.
const PANEL_MARGIN_X: i8 = 8;
/// A panel's margin at its top and bottom.
const PANEL_MARGIN_Y: i8 = 4;
const COVER_SIDE: f32 = 64.0;
/// The right of the header: BPM, key and times.
const TIMES_WIDTH: f32 = 130.0;
/// A line of the header's small text.
const HEADER_LINE: f32 = 16.0;
/// The overview, under the title in the header.
const OVERVIEW_HEIGHT: f32 = 24.0;
/// The space between a panel's rows, and between the controls of a column.
const ROW_GAP: f32 = 4.0;
const FX_WIDTH: f32 = 96.0;
const SLOT_HEIGHT: f32 = 22.0;
const FX_LEVEL_HEIGHT: f32 = 6.0;
/// The effect level's caption under its bar.
const FX_CAPTION_HEIGHT: f32 = 14.0;
/// The pad mode dropdown's width.
const PAD_MODE_WIDTH: f32 = 120.0;
/// The arrows either side of the pad mode's dropdown.
const PAD_ARROW_WIDTH: f32 = 16.0;
/// An FX slot's arrow, which opens its list of effects, and the gap before it.
const FX_ARROW: f32 = 18.0;
const FX_ARROW_GAP: f32 = 2.0;
/// Each FX slot's name, for screen readers: a static table, so no frame formats it.
const FX_NAMES: [&str; 3] = ["FX 1", "FX 2", "FX 3"];
/// Where a double-click puts the tempo slider: the original tempo.
const PITCH_RESET: f32 = 0.5;
/// The pad modes the dropdown offers: only the app's own. The SB3 switches its hardware
/// modes itself and the host cannot set them, so the hardware entry hands the pads back to
/// whichever the controller last chose.
const PAD_MODES: [(PadMode, &str); 5] = [
    (PadMode::Hardware, "pads (controller)"),
    (PadMode::BeatJump, "BEAT JUMP"),
    (PadMode::LoopRoll, "LOOP ROLL"),
    (PadMode::Slicer, "SLICER"),
    (PadMode::PadScratch, "PAD SCRATCH"),
];
const JOG_SIDE: f32 = 104.0;
const JOG_STROKE: f32 = 2.0;
const JOG_MARKER: f32 = 3.0;
const PITCH_WIDTH: f32 = 54.0;
/// The tempo slider's width when the layout has no jog wheel.
const PITCH_COMPACT_WIDTH: f32 = 40.0;
const PITCH_TRACK: f32 = 4.0;
const PITCH_HANDLE: Vec2 = Vec2::new(24.0, 6.0);
/// Below the tempo slider, for the pitch in percent.
const PITCH_LABEL_HEIGHT: f32 = 22.0;
const STATE_WIDTH: f32 = 72.0;
const BUTTON_HEIGHT: f32 = 24.0;
const TRANSPORT_WIDTH: f32 = 60.0;
const LOOP_WIDTH: f32 = 60.0;
const PAD_GAP: f32 = 4.0;
const SMALL_FONT: f32 = 11.0;
const TITLE_FONT: f32 = 16.0;
const BPM_FONT: f32 = 22.0;

/// One performance pad as the panel shows it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PadView {
    pub label: String,
    pub lit: bool,
    /// The saved colour of the hotcue the pad shows, if it has one.
    pub colour: Option<Color32>,
}

/// Everything a deck panel shows, worked out from the engine's snapshots and the frame so
/// it can be checked without a screen.
///
/// There is no SYNC or MASTER state: the engine's sync is one-shot, so the panel never
/// shows one lit.
#[derive(Debug, Clone)]
pub struct DeckView {
    pub deck: DeckId,
    /// The deck's number.
    pub tag: String,
    /// The title without its remix, else the file name; empty without a track.
    pub title: String,
    pub artist: String,
    /// The mix name in the title's last brackets.
    pub remix: Option<String>,
    /// The tempo playing: the grid's BPM times the tempo ratio.
    pub bpm: Option<f64>,
    pub pitch_percent: f64,
    /// The tempo slider, 0 to 1: 0.5 is the original tempo, 1 the fastest.
    pub pitch_slider: f32,
    /// Where the physical tempo slider waits to pick the tempo up (soft takeover).
    pub pitch_ghost: Option<f32>,
    /// In the library's notation.
    pub key: Option<String>,
    /// `M:SS.d`, cut (not rounded) to the tenth; empty without a track.
    pub elapsed: String,
    pub remaining: String,
    /// The track's length, like the times; empty without a track.
    pub total: String,
    /// Which of the times the header shows.
    pub time_mode: TimeMode,
    /// `bar.beat` for the header, when the view asks for it and the track has a grid.
    pub beat_counter: Option<String>,
    pub jog_display: JogDisplay,
    /// Relative steps one turn of the jog sends (the controller's resolution).
    pub jog_steps_per_turn: u16,
    /// The playhead's place in the track, 0 to 1; `None` without a track.
    pub progress: Option<f32>,
    /// How far the jog's marker has turned, 0 to 1 clockwise from the top.
    pub jog_turn: f32,
    pub keylock: bool,
    pub slip: bool,
    pub vinyl: bool,
    pub playing: bool,
    /// CUE is held and the deck previews from the cue point.
    pub cueing: bool,
    /// The next auto loop's length in beats ("1/2", "4").
    pub loop_beats: String,
    pub loop_active: bool,
    /// Each FX slot's effect and whether it is on.
    pub fx: [(EffectKind, bool); 3],
    pub fx_level: f32,
    /// Where the controller's FX level waits to pick the level up.
    pub fx_level_ghost: Option<f32>,
    /// What the pads do, as [`SideControl::pad_mode_label`] names it.
    pub pad_mode: &'static str,
    /// The pad mode the app switches to, [`PadMode::Hardware`] while the SB3's own mode
    /// buttons drive the pads.
    pub pad_mode_choice: PadMode,
    /// The SB3's last hardware mode button, which decides what the pads show and send
    /// while [`PadMode::Hardware`].
    pub pad_hardware_mode: Option<HardwarePadMode>,
    pub pads: [PadView; 8],
    /// The stems' state ("stems", "stems 40%").
    pub stems: Option<String>,
    /// The track's file, for its cover.
    pub path: Option<PathBuf>,
    pub overview: Option<Arc<Waveform>>,
    /// The colours the overview draws its bands in.
    pub waveform_palette: WaveformPalette,
    /// The library track the deck plays, for the hotcue editor.
    pub track: Option<i64>,
    /// The hotcues' colours and names, when the library has them.
    pub hotcue_marks: Option<Arc<[Option<HotcueMark>; 8]>>,
}

impl DeckView {
    /// The view of deck `id` from its snapshots and `frame`. The frame's track is only used
    /// once the engine plays it (`snapshot.load_seq` has reached it). `snapshot`'s position
    /// is what is shown: pass the playhead's.
    pub fn new(
        id: DeckId,
        snapshot: &DeckSnapshot,
        channel: &ChannelSnapshot,
        frame: &ControlFrame,
        sampler: &SamplerSnapshot,
    ) -> DeckView {
        let info = frame.decks[id.index()]
            .as_ref()
            .filter(|info| snapshot.loaded && info.load_seq == snapshot.load_seq);
        let rate = snapshot.track_sample_rate;
        let loaded = snapshot.loaded && rate > 0;
        let duration = snapshot.duration_frames as f64;
        let position = snapshot.position_frames.clamp(0.0, duration);
        let (title, remix) = match info {
            Some(info) => {
                let file_name = || {
                    info.path
                        .file_stem()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default()
                };
                match info.row.title.as_deref().filter(|t| !t.trim().is_empty()) {
                    Some(title) => split_remix(title),
                    None => (file_name(), None),
                }
            }
            None => (String::new(), None),
        };
        let grid_bpm = snapshot
            .grid
            .map(|g| g.beat_period_frames)
            .filter(|p| loaded && p.is_finite() && *p > 0.0)
            .map(|period| 60.0 * f64::from(rate) / period);
        let side = frame.control.side(Side::of(id));
        DeckView {
            deck: id,
            tag: (id.index() + 1).to_string(),
            title,
            artist: info.and_then(|i| i.row.artist.clone()).unwrap_or_default(),
            remix,
            bpm: grid_bpm
                .or(info.and_then(|i| i.row.bpm))
                .map(|bpm| bpm * snapshot.tempo_ratio),
            pitch_percent: (snapshot.tempo_ratio - 1.0) * 100.0,
            pitch_slider: frame.control.deck(id).tempo,
            pitch_ghost: frame.ghosts[id.index()][GHOST_TEMPO],
            key: info
                .and_then(|i| i.row.key)
                .and_then(|key| key_text(key, frame.key_notation)),
            elapsed: if loaded {
                clock(position, rate)
            } else {
                String::new()
            },
            remaining: if loaded {
                clock(duration - position, rate)
            } else {
                String::new()
            },
            total: if loaded {
                clock(duration, rate)
            } else {
                String::new()
            },
            time_mode: frame.view.time_mode(id),
            beat_counter: snapshot
                .grid
                .filter(|_| loaded && frame.view.beat_counter)
                .and_then(|grid| beat_counter(Some(grid), position)),
            jog_display: frame.view.jog_display,
            jog_steps_per_turn: frame.control.jog_steps_per_turn,
            progress: (loaded && duration > 0.0).then(|| (position / duration) as f32),
            jog_turn: if loaded {
                (position / f64::from(rate) * JOG_RPM / 60.0).fract() as f32
            } else {
                0.0
            },
            keylock: snapshot.keylock,
            slip: snapshot.slip,
            vinyl: snapshot.vinyl,
            playing: snapshot.playing,
            cueing: snapshot.previewing,
            loop_beats: beats_text(snapshot.loop_beats),
            loop_active: snapshot.loop_active,
            fx: std::array::from_fn(|slot| {
                (frame.fx_slots[id.index()][slot], channel.fx_enabled[slot])
            }),
            fx_level: channel.fx_amount,
            fx_level_ghost: frame.ghosts[id.index()][GHOST_FX_LEVEL],
            pad_mode: side.pad_mode_label(),
            pad_mode_choice: side.pad_mode,
            pad_hardware_mode: side.hardware_mode,
            pads: pads(
                side,
                &frame.control,
                snapshot,
                info.map(|i| &*i.hotcue_marks),
                &frame.samples_loaded,
                sampler,
            ),
            stems: info.and_then(|i| i.stems.clone()),
            path: info.map(|i| i.path.clone()),
            overview: info.and_then(|i| i.overview.clone()),
            waveform_palette: frame.view.waveform_palette,
            track: info.map(|i| i.row.id),
            hotcue_marks: info.map(|i| Arc::clone(&i.hotcue_marks)),
        }
    }
}

/// The eight pads for `side`'s mode. In the hardware modes the SB3 lights its own pads; the
/// panel shows what they act on: the hotcues (also before any mode button and in FX FADE)
/// or, in SAMPLER and TRANS, the sampler slots.
fn pads(
    side: &SideControl,
    control: &ControlState,
    snapshot: &DeckSnapshot,
    marks: Option<&[Option<HotcueMark>; 8]>,
    samples_loaded: &[bool; SAMPLER_SLOTS],
    sampler: &SamplerSnapshot,
) -> [PadView; 8] {
    std::array::from_fn(|pad| {
        let number = pad + 1;
        let mut colour = None;
        let (label, lit) = match side.pad_mode {
            PadMode::Hardware => match side.hardware_mode {
                Some(HardwarePadMode::Sampler | HardwarePadMode::Trans) => (
                    format!("S {number}"),
                    // A sounding slot is loaded even if the frame has not caught up.
                    samples_loaded[pad] || sampler.playing[pad],
                ),
                _ => {
                    let mark = marks.and_then(|m| m[pad].as_ref());
                    colour = mark
                        .and_then(|m| m.color)
                        .filter(|_| snapshot.hotcues[pad].is_some());
                    (
                        hotcue_label(number, snapshot.hotcues[pad], mark),
                        snapshot.hotcues[pad].is_some(),
                    )
                }
            },
            PadMode::BeatJump => {
                // SHIFT switches the pads to their second set of sizes.
                let sizes = if side.shift {
                    &control.beat_jump_shift_pads
                } else {
                    &control.beat_jump_pads
                };
                (jump_text(sizes[pad]), false)
            }
            PadMode::LoopRoll => (beats_text(control.loop_roll_pads[pad]), false),
            PadMode::Slicer => (number.to_string(), false),
            PadMode::PadScratch => (String::new(), false),
        };
        PadView { label, lit, colour }
    })
}

/// A hotcue pad's text: its number, then its name, or `loop` for a saved loop without
/// one. An empty slot shows the number alone.
fn hotcue_label(number: usize, hotcue: Option<Hotcue>, mark: Option<&HotcueMark>) -> String {
    let Some(hotcue) = hotcue else {
        return number.to_string();
    };
    let name = mark
        .and_then(|m| m.name.as_deref())
        .map(str::trim)
        .filter(|name| !name.is_empty());
    match (name, hotcue.loop_frames) {
        (Some(name), _) => format!("{number} {name}"),
        (None, Some(_)) => format!("{number} loop"),
        (None, None) => number.to_string(),
    }
}

/// `title` without the remix in its last brackets, and the remix ([`library::remix_of`]).
/// A title that is only the brackets is kept whole.
pub(crate) fn split_remix(title: &str) -> (String, Option<String>) {
    let title = title.trim();
    let Some(remix) = library::remix_of(title) else {
        return (title.to_owned(), None);
    };
    // `remix` lies inside `title`; the bracket that opens it is the last one before it.
    let inner = remix.as_ptr() as usize - title.as_ptr() as usize;
    let stem = title[..inner]
        .rfind(['(', '['])
        .map_or(title, |open| title[..open].trim_end());
    if stem.is_empty() {
        (title.to_owned(), Some(remix.to_owned()))
    } else {
        (stem.to_owned(), Some(remix.to_owned()))
    }
}

fn effect_label(kind: EffectKind) -> &'static str {
    match kind {
        EffectKind::Echo => "Echo",
        EffectKind::Reverb => "Reverb",
        EffectKind::Flanger => "Flanger",
        EffectKind::Filter => "Filter",
    }
}

/// The stored key `raw` (0..=23) in `notation`; `None` when it is out of range.
pub(crate) fn key_text(raw: u8, notation: KeyNotation) -> Option<String> {
    Key::new(raw).map(|key| key.to_notation(notation))
}

/// `frames` at `rate` as `M:SS.d`, cut to the tenth.
fn clock(frames: f64, rate: u32) -> String {
    // The tiny allowance keeps a time that is exactly on a tenth from reading one less
    // through rounding error.
    let tenths = (frames.max(0.0) * 10.0 / f64::from(rate) + 1e-6).floor() as u64;
    format!("{}:{:02}.{}", tenths / 600, tenths / 10 % 60, tenths % 10)
}

/// The `bar.beat` the playhead is at, both counted from 1 from the grid's first beat, 4
/// beats to a bar. Before the first beat the bars count backwards from 0: three beats
/// before it is "0.2". `None` without a grid.
pub fn beat_counter(grid: Option<Grid>, position_frames: f64) -> Option<String> {
    let grid = grid.filter(|g| g.beat_period_frames.is_finite() && g.beat_period_frames > 0.0)?;
    let beats = ((position_frames - grid.first_beat_frames) / grid.beat_period_frames).floor();
    if !beats.is_finite() {
        return None;
    }
    let beats = beats as i64;
    Some(format!(
        "{}.{}",
        beats.div_euclid(4) + 1,
        beats.rem_euclid(4) + 1
    ))
}

/// The pitch as "+0.0%"; a pitch that rounds to nothing never reads "-0.0%".
fn pitch_text(percent: f64) -> String {
    let rounded = (percent * 10.0).round() / 10.0;
    format!("{:+.1}%", if rounded == 0.0 { 0.0 } else { rounded })
}

/// A beat count as a pad or loop shows it: "1/16", "1/2", "4".
fn beats_text(beats: f64) -> String {
    if beats > 0.0 && beats < 1.0 {
        let denominator = (1.0 / beats).round();
        if (denominator * beats - 1.0).abs() < 1e-9 {
            return format!("1/{denominator}");
        }
    }
    format!("{beats}")
}

/// A beat jump as its pad shows it: "−4", "+1/2".
fn jump_text(beats: f64) -> String {
    let sign = if beats < 0.0 { '−' } else { '+' };
    format!("{sign}{}", beats_text(beats.abs()))
}

/// What the engine reports, as the window reads it once a frame: every deck, the meters
/// and the load. Without a monitor every deck is empty and the meters are silent.
#[derive(Debug, Default)]
pub struct DeckReads {
    /// Each deck's snapshot and the time its block began rendering (0: unknown).
    pub decks: [(DeckSnapshot, u64); 4],
    pub channels: [ChannelSnapshot; 4],
    pub sampler: SamplerSnapshot,
    /// The last block's peaks; `None` until the audio device opens.
    pub meters: Option<Meters>,
    /// The last block's DSP load; `None` until the audio device opens.
    pub dsp_load: Option<f32>,
}

impl DeckReads {
    /// Reads everything the window shows from `monitor`; `None` reads as empty.
    pub fn read(monitor: Option<&EngineMonitor>) -> Self {
        let Some(m) = monitor else {
            return Self::default();
        };
        Self {
            decks: DeckId::ALL.map(|id| m.deck_rendered_at(id)),
            channels: DeckId::ALL.map(|id| m.channel(id)),
            sampler: m.sampler(),
            meters: Some(m.meters()),
            dsp_load: Some(m.dsp_load()),
        }
    }
}

/// What the deck panels keep from frame to frame: each deck's playhead.
#[derive(Debug, Default)]
pub struct DeckPanels {
    playheads: [Playhead; 4],
}

impl DeckPanels {
    /// Draws the left side's active deck at the left of the space `ui` has left and the
    /// right side's at its right, each as `reads` has it, and returns the space between
    /// them, for the mixer.
    pub fn show_reads(
        &mut self,
        ui: &mut Ui,
        frame: &ControlFrame,
        reads: &DeckReads,
        covers: &mut Covers,
        sections: &Sections,
        sink: &mut Sink<'_>,
    ) -> Rect {
        let now = Instant::now();
        let (area, _) = ui.allocate_exact_size(ui.available_size(), Sense::hover());
        let four = frame.view.four_decks;
        let share = if four { FOUR_PANEL_SHARE } else { PANEL_SHARE };
        let width = area.width() * share;
        let left = Rect::from_min_size(area.min, vec2(width, area.height()));
        let right = Rect::from_min_size(
            pos2(area.right() - width, area.top()),
            vec2(width, area.height()),
        );
        let active = Side::ALL.map(|side| frame.control.side(side).active_deck);
        let slots = deck_slots(four, active);
        // Half the gap to the mixer on each panel's inner side; the mixer keeps the other.
        let half_gap = SECTION_GAP / 2.0;
        let columns = [
            Rect::from_min_max(left.min, pos2(left.right() - half_gap, left.bottom())),
            Rect::from_min_max(pos2(right.left() + half_gap, right.top()), right.max),
        ];
        // Two decks: one panel a side. Four: two stacked, the side's first deck on top,
        // the gap between them.
        let rects: Vec<Rect> = if four {
            columns
                .into_iter()
                .flat_map(|side| {
                    let half = (side.height() - SECTION_GAP) / 2.0;
                    [
                        Rect::from_min_size(side.min, vec2(side.width(), half)),
                        Rect::from_min_max(pos2(side.left(), side.bottom() - half), side.max),
                    ]
                })
                .collect()
        } else {
            columns.to_vec()
        };
        for (deck, rect) in slots.into_iter().zip(rects) {
            let (mut snapshot, rendered_at) = reads.decks[deck.index()];
            let channel = &reads.channels[deck.index()];
            let head = &mut self.playheads[deck.index()];
            head.observe(&snapshot, rendered_at);
            snapshot.position_frames = head.position(now, frame.output_latency);
            let view = DeckView::new(deck, &snapshot, channel, frame, &reads.sampler);
            let cover = view
                .path
                .as_deref()
                .and_then(|path| covers.get(ui.ctx(), path));
            let area = crate::landmark_rect(ui, rect, PANEL_NAMES[deck.index()]);
            ui.scope_builder(UiBuilder::new().max_rect(rect), |ui| {
                // Nothing a panel draws reaches its neighbours.
                ui.set_clip_rect(rect.intersect(ui.clip_rect()));
                panel(ui, &view, cover.as_ref(), sections, sink);
            });
            drop_load(ui, &area, rect, deck, sink);
            // The decks the controller plays have a tab in their side's accent on the outer
            // edge (with two decks, both panels).
            if active.contains(&deck) {
                let accent = theme::palette(ui.ctx()).deck_accent(deck);
                let x = match Side::of(deck) {
                    Side::Left => rect.left(),
                    Side::Right => rect.right() - ACCENT_TAB,
                };
                let tab = Rect::from_x_y_ranges(x..=x + ACCENT_TAB, rect.y_range());
                ui.painter().rect_filled(tab, CornerRadius::ZERO, accent);
            }
        }
        Rect::from_min_max(
            pos2(left.right(), area.top()),
            pos2(right.left(), area.bottom()),
        )
    }
}

/// Loads on `deck` the first of the tracks dropped on `response` (a deck panel or its lane,
/// at `rect`); the session refuses it while the deck plays.
pub(crate) fn drop_load(
    ui: &Ui,
    response: &egui::Response,
    rect: Rect,
    deck: DeckId,
    sink: &mut Sink<'_>,
) {
    let accent = theme::palette(ui.ctx()).deck_accent(deck);
    let dropped = crate::browser::track_drop(ui, response, rect, accent);
    if let Some(&track) = dropped.as_ref().and_then(|drag| drag.tracks.first()) {
        sink.send(UiCommand::Library(LibraryStep::Load { deck, track }));
    }
}

/// The height a deck panel needs to show every area of `sections`: the header (with the
/// overview in it), then the middle and bottom rows, or the compact panel's one row.
pub fn panel_height(sections: &Sections) -> f32 {
    let height = 2.0 * f32::from(PANEL_MARGIN_Y) + COVER_SIDE;
    if sections.compact {
        return height + ROW_GAP + BUTTON_HEIGHT;
    }
    height + ROW_GAP + middle_height(sections) + ROW_GAP + bottom_height()
}

/// The height the deck panels' row needs for `sections`: one panel, or two stacked in the
/// 4-Decks view, each with the [`SECTION_GAP`] under it.
pub fn row_height(sections: &Sections) -> f32 {
    let panels = if sections.compact { 2.0 } else { 1.0 };
    panels * (panel_height(sections) + SECTION_GAP)
}

/// The middle row's height: the jog's when the layout has one, else the FX slots' (or the
/// states', without them).
fn middle_height(sections: &Sections) -> f32 {
    let states = 3.0 * SLOT_HEIGHT + 2.0 * ROW_GAP;
    let fx = if sections.fx {
        fx_height(sections.fx_level)
    } else {
        0.0
    };
    let jog = if sections.jog { JOG_SIDE } else { 0.0 };
    states.max(fx).max(jog)
}

/// The FX column's height, with the effect level under the slots when `level`.
fn fx_height(level: bool) -> f32 {
    let slots = 3.0 * SLOT_HEIGHT + 2.0 * ROW_GAP;
    if level {
        slots + ROW_GAP + FX_LEVEL_HEIGHT + ROW_GAP + FX_CAPTION_HEIGHT
    } else {
        slots
    }
}

/// The bottom row's height: three buttons, as tall as the pad mode over two rows of pads
/// and as CUE, PLAY and SYNC.
fn bottom_height() -> f32 {
    3.0 * BUTTON_HEIGHT + 2.0 * ROW_GAP
}

/// Draws `view` filling `ui`, with `cover` (the plain tile without one), showing the
/// `sections` the layout has. A click on the time asks `sink` to cycle it.
pub fn panel(
    ui: &mut Ui,
    view: &DeckView,
    cover: Option<&TextureHandle>,
    sections: &Sections,
    sink: &mut Sink<'_>,
) {
    let p = theme::palette(ui.ctx());
    let accent = p.deck_accent(view.deck);
    egui::Frame::new()
        .fill(p.panel)
        .inner_margin(Margin::symmetric(PANEL_MARGIN_X, PANEL_MARGIN_Y))
        .show(ui, |ui| {
            ui.set_min_size(ui.available_size());
            ui.spacing_mut().item_spacing = vec2(6.0, ROW_GAP);
            header(ui, view, cover, accent, sections.overview, sink);
            let mirrored = Side::of(view.deck) == Side::Right;
            if sections.compact {
                compact_row(ui, view, accent, sections.loop_controls, mirrored, sink);
                return;
            }
            ui.horizontal(|ui| {
                middle_row(ui, view, accent, sections, mirrored, sink);
            });
            ui.horizontal_top(|ui| {
                bottom_row(ui, view, accent, sections, mirrored, sink);
            });
        });
}

/// What the middle and bottom rows hold, outer edge first on a left deck.
#[derive(Debug, Clone, Copy)]
enum Part {
    Label(&'static str),
    Fx,
    States,
    /// What the row has to spare, between the outer and the inner parts.
    Spare,
    Jog,
    Pitch,
    Pads,
    Loop,
    Transport,
}

/// `parts` in order on a left deck, reversed on a right one (`mirrored`): VirtualDJ
/// mirrors the right decks, their sections' labels on the outer edge.
fn ordered<const N: usize>(mut parts: [Option<Part>; N], mirrored: bool) -> [Option<Part>; N] {
    if mirrored {
        parts.reverse();
    }
    parts
}

/// The FX slots and the states on the outer side, the jog and the tempo slider on the
/// inner side over CUE, PLAY and SYNC.
fn middle_row(
    ui: &mut Ui,
    view: &DeckView,
    accent: Color32,
    sections: &Sections,
    mirrored: bool,
    sink: &mut Sink<'_>,
) {
    let middle = middle_height(sections);
    ui.set_height(middle);
    let pitch_width = if sections.jog {
        PITCH_WIDTH
    } else {
        PITCH_COMPACT_WIDTH
    };
    let parts = ordered(
        [
            sections.fx.then_some(Part::Label("FX")),
            sections.fx.then_some(Part::Fx),
            Some(Part::States),
            Some(Part::Spare),
            sections.jog.then_some(Part::Jog),
            sections.pitch.then_some(Part::Pitch),
        ],
        mirrored,
    );
    let width = |part: Part| match part {
        Part::Label(_) => EDGE_LABEL_WIDTH,
        Part::Fx => FX_WIDTH,
        Part::States => STATE_WIDTH,
        Part::Jog => JOG_SIDE,
        Part::Pitch => pitch_width,
        _ => 0.0,
    };
    let spare = spare_width(ui, &parts, width);
    for part in parts.into_iter().flatten() {
        match part {
            Part::Label(text) => edge_strip(ui, middle, text),
            Part::Fx => fx(ui, view, accent, sections.fx_level, sink),
            Part::States => states(ui, view, accent, sink),
            Part::Spare => ui.add_space(spare),
            Part::Jog => jog(ui, view, accent, sink),
            Part::Pitch => pitch(ui, view, accent, (pitch_width, middle), sink),
            _ => {}
        }
    }
}

/// The room left in `ui`'s row once each of `parts` takes its `width` and the spacing
/// after it (none after the last).
fn spare_width(ui: &Ui, parts: &[Option<Part>], width: impl Fn(Part) -> f32) -> f32 {
    let spacing = ui.spacing().item_spacing.x;
    let used: f32 = parts
        .iter()
        .flatten()
        .filter(|part| !matches!(part, Part::Spare))
        .map(|&part| width(part) + spacing)
        .sum();
    (ui.available_width() - used + spacing).max(0.0)
}

/// The pads and the loop on the outer side, CUE, PLAY and SYNC on the inner side, under
/// the jog. The pads take the width the others leave.
fn bottom_row(
    ui: &mut Ui,
    view: &DeckView,
    accent: Color32,
    sections: &Sections,
    mirrored: bool,
    sink: &mut Sink<'_>,
) {
    let height = bottom_height();
    let parts = ordered(
        [
            sections.pads.then_some(Part::Label("PADS")),
            sections.pads.then_some(Part::Pads),
            sections.loop_controls.then_some(Part::Label("LOOP")),
            sections.loop_controls.then_some(Part::Loop),
            Some(Part::Spare),
            Some(Part::Transport),
        ],
        mirrored,
    );
    let width = |part: Part| match part {
        Part::Label(_) => EDGE_LABEL_WIDTH,
        Part::Loop => LOOP_WIDTH,
        Part::Transport => TRANSPORT_WIDTH,
        _ => 0.0,
    };
    // The pads count as nothing here: they take the spare room, with what is left over
    // between them and the transport.
    let pads_width = if sections.pads {
        let spacing = ui.spacing().item_spacing.x;
        (spare_width(ui, &parts, width) - spacing).max(0.0)
    } else {
        0.0
    };
    let spare = spare_width(ui, &parts, |part| match part {
        Part::Pads => pads_width,
        other => width(other),
    });
    for part in parts.into_iter().flatten() {
        match part {
            Part::Label(text) => edge_strip(ui, height, text),
            Part::Pads => pads_column(ui, view, accent, pads_width, sink),
            Part::Loop => loop_column(ui, view, sink),
            Part::Spare => ui.add_space(spare),
            Part::Transport => transport(ui, view, sink),
            _ => {}
        }
    }
}

/// A strip [`EDGE_LABEL_WIDTH`] wide and `height` tall at the left of what follows in
/// `ui`, named `text` with an [`edge_label`].
fn edge_strip(ui: &mut Ui, height: f32, text: &str) {
    let (strip, _) = ui.allocate_exact_size(vec2(EDGE_LABEL_WIDTH, height), Sense::hover());
    edge_label(ui, strip, text);
}

/// CUE, PLAY, SYNC, the three states and (when `loop_controls`) the loop's size, IN and
/// OUT in one row, for the compact panel; narrower than they like when the row is short.
/// On a right deck (`mirrored`) the three groups come in the other order. The mouse
/// presses them through `sink`.
fn compact_row(
    ui: &mut Ui,
    view: &DeckView,
    accent: Color32,
    loop_controls: bool,
    mirrored: bool,
    sink: &mut Sink<'_>,
) {
    let p = theme::palette(ui.ctx());
    let deck = view.deck;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = PAD_GAP;
        let transport = TRANSPORT_WIDTH / 1.5;
        let state = STATE_WIDTH * 0.75;
        let looping = LOOP_WIDTH / 2.0;
        let (boxes, natural) = if loop_controls {
            (9.0, 3.0 * (transport + state + looping))
        } else {
            (6.0, 3.0 * (transport + state))
        };
        let room = (ui.available_width() - (boxes - 1.0) * PAD_GAP).max(0.0);
        let scale = (room / natural).min(1.0);
        let size = |width: f32| vec2(width * scale, BUTTON_HEIGHT);
        let groups = ordered(
            [
                Some(Part::Transport),
                Some(Part::States),
                loop_controls.then_some(Part::Loop),
            ],
            mirrored,
        );
        for part in groups.into_iter().flatten() {
            match part {
                Part::Transport => {
                    let cue = view.cueing.then_some(p.cue);
                    button(ui, sink, size(transport), "CUE", cue, Action::Cue(deck));
                    let play = view.playing.then_some(p.play_lit);
                    let play_pause = Action::PlayPause(deck);
                    button(ui, sink, size(transport), "PLAY", play, play_pause);
                    button(ui, sink, size(transport), "SYNC", None, Action::Sync(deck));
                }
                Part::States => {
                    for (text, on, action) in state_buttons(view) {
                        button(ui, sink, size(state), text, on.then_some(accent), action);
                    }
                }
                Part::Loop => {
                    let lit = view.loop_active.then_some(p.hotcue);
                    let beats = &view.loop_beats;
                    button(ui, sink, size(looping), beats, lit, Action::AutoLoop(deck));
                    button(ui, sink, size(looping), "IN", lit, Action::LoopIn(deck));
                    button(ui, sink, size(looping), "OUT", lit, Action::LoopOut(deck));
                }
                _ => {}
            }
        }
    });
}

/// The cover, the title over the artist and remix, the overview under them (when
/// `overview`), then BPM, key, stems and times on the right, all within the cover's height.
fn header(
    ui: &mut Ui,
    view: &DeckView,
    cover: Option<&TextureHandle>,
    accent: Color32,
    overview: bool,
    sink: &mut Sink<'_>,
) {
    let p = theme::palette(ui.ctx());
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), COVER_SIDE), Sense::hover());
    let mut ui = ui.new_child(
        UiBuilder::new()
            .max_rect(rect)
            .layout(Layout::left_to_right(Align::Min)),
    );
    ui.set_clip_rect(rect.intersect(ui.clip_rect()));
    cover::tile(&mut ui, cover, COVER_SIDE);
    let text_width = (ui.available_width() - TIMES_WIDTH).max(0.0);
    let (column, _) = ui.allocate_exact_size(vec2(text_width, COVER_SIDE), Sense::hover());
    // The overview along the bottom of the column, the text above it.
    let text = if overview {
        let (text, wave) = column.split_top_bottom_at_y(column.bottom() - OVERVIEW_HEIGHT);
        overview_strip(&ui, view, wave, sink);
        text
    } else {
        column
    };
    let layout = Layout::top_down(Align::Min);
    ui.scope_builder(UiBuilder::new().max_rect(text).layout(layout), |ui| {
        ui.set_clip_rect(text.intersect(ui.clip_rect()));
        ui.spacing_mut().item_spacing.y = 0.0;
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(&view.tag)
                    .color(accent)
                    .strong()
                    .size(TITLE_FONT),
            );
            ui.add(Label::new(RichText::new(&view.title).strong().size(TITLE_FONT)).truncate());
        });
        ui.horizontal(|ui| {
            ui.add(Label::new(RichText::new(&view.artist).color(p.text_dim)).truncate());
            if let Some(remix) = &view.remix {
                ui.add(Label::new(RichText::new(remix).color(p.text_dim)).truncate());
            }
        });
    });
    let times = vec2(ui.available_width(), COVER_SIDE);
    ui.allocate_ui_with_layout(times, Layout::top_down(Align::Max), |ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        let bpm = view
            .bpm
            .map_or_else(|| "–".to_owned(), |bpm| format!("{bpm:.2}"));
        ui.label(RichText::new(bpm).strong().size(BPM_FONT));
        // Right to left: the last added is the leftmost.
        let row = Layout::right_to_left(Align::Center);
        let line = vec2(ui.available_width(), HEADER_LINE);
        if view.key.is_some() || view.beat_counter.is_some() || view.stems.is_some() {
            ui.allocate_ui_with_layout(line, row, |ui| {
                if let Some(counter) = &view.beat_counter {
                    ui.label(RichText::new(counter).color(p.text_dim));
                }
                if let Some(key) = &view.key {
                    ui.label(RichText::new(key).color(accent));
                }
                if let Some(stems) = &view.stems {
                    ui.label(RichText::new(stems).color(accent).size(SMALL_FONT));
                }
            });
        }
        if !view.elapsed.is_empty() {
            let remaining = format!("−{}", view.remaining);
            let (time, other) = match view.time_mode {
                TimeMode::Elapsed => (view.elapsed.clone(), remaining),
                TimeMode::Remaining => (remaining, view.elapsed.clone()),
                TimeMode::Total => (view.total.clone(), view.elapsed.clone()),
            };
            ui.allocate_ui_with_layout(line, row, |ui| {
                ui.label(RichText::new(other).color(p.text_dim));
                let time = ui.add(Label::new(time).selectable(false).sense(Sense::click()));
                if time.clicked() {
                    sink.send(UiCommand::View(ViewChange::CycleTime(view.deck)));
                }
            });
        }
    });
}

/// The whole track's waveform with the playhead, in `rect`.
fn overview_strip(ui: &Ui, view: &DeckView, rect: Rect, sink: &mut Sink<'_>) {
    let p = theme::palette(ui.ctx());
    let response = ui.interact(
        rect,
        ui.scope_id().with(("overview", view.deck)),
        Sense::click_and_drag(),
    );
    // A click seeks to the point, a drag follows the pointer; an empty deck has no track.
    if view.progress.is_some() {
        sink.act(
            Action::Seek(view.deck),
            fader_input(&response, rect, false, None),
        );
    }
    response.widget_info(|| WidgetInfo::labeled(Role::Image, true, "overview"));
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, CornerRadius::ZERO, p.wave_background);
    let mut mesh = Mesh::default();
    if let Some(waveform) = view.overview.as_deref() {
        overview_columns(
            &mut mesh,
            rect,
            waveform,
            band_colours(view.waveform_palette, &p),
        );
    }
    if let Some(progress) = view.progress {
        let x = rect.left() + rect.width() * progress.clamp(0.0, 1.0);
        let head = Rect::from_x_y_ranges(x - 1.0..=x + 1.0, rect.y_range());
        mesh.add_colored_rect(head, p.playhead);
    }
    painter.add(Shape::mesh(mesh));
}

/// One column per point, mirrored around the middle, in the three layers of the lane:
/// low, mid and high in `bands`, back to front, each as tall as its loudest under the
/// column ([`loudest`]) and blended as the lane blends them ([`layers`]).
fn overview_columns(mesh: &mut Mesh, rect: Rect, waveform: &Waveform, bands: [Color32; 3]) {
    let bands = layers(bands);
    let buckets = &waveform.buckets;
    let columns = rect.width().floor().max(0.0) as usize;
    if buckets.is_empty() || columns == 0 {
        return;
    }
    let middle = rect.center().y;
    let half_height = rect.height() / 2.0;
    for column in 0..columns {
        let from = column * buckets.len() / columns;
        let to = ((column + 1) * buckets.len() / columns).clamp(from + 1, buckets.len());
        let Some([_, low, mid, high]) = loudest(&buckets[from..to]) else {
            continue;
        };
        let x = rect.left() + column as f32;
        for (level, colour) in [(low, bands[0]), (mid, bands[1]), (high, bands[2])] {
            if level == 0 {
                continue;
            }
            let reach = half_height * f32::from(level) / 255.0;
            let bar = Rect::from_min_max(pos2(x, middle - reach), pos2(x + 1.0, middle + reach));
            mesh.add_colored_rect(bar, colour);
        }
    }
}

/// The three FX slots, each lit while on, over the effect level (when `level`). A slot's
/// effect turns it on and off; its arrow is a dropdown that changes the effect. The mouse
/// plays them and the level through `sink`.
fn fx(ui: &mut Ui, view: &DeckView, accent: Color32, level: bool, sink: &mut Sink<'_>) {
    let p = theme::palette(ui.ctx());
    let deck = view.deck;
    ui.vertical(|ui| {
        ui.set_width(FX_WIDTH);
        for (slot, &(current, on)) in view.fx.iter().enumerate() {
            let lit = on.then_some(accent);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = FX_ARROW_GAP;
                let toggle = vec2(FX_WIDTH - FX_ARROW - FX_ARROW_GAP, SLOT_HEIGHT);
                let action = Action::FxToggle(deck, slot as u8);
                button(ui, sink, toggle, effect_label(current), lit, action);
                let id = ("fx slot", deck, slot);
                let size = vec2(FX_ARROW, SLOT_HEIGHT);
                dropdown(ui, FX_NAMES[slot], id, size, "", lit, |ui| {
                    for kind in EffectKind::ALL {
                        let chosen = ui.selectable_label(kind == current, effect_label(kind));
                        if chosen.clicked() && kind != current {
                            sink.send(UiCommand::FxSlot { deck, slot, kind });
                        }
                    }
                });
            });
        }
        if !level {
            return;
        }
        let (rect, _) = ui.allocate_exact_size(vec2(FX_WIDTH, FX_LEVEL_HEIGHT), Sense::hover());
        let action = Action::FxLevel(deck);
        // A little above and below the thin bar takes a click too.
        let grab = rect.expand2(vec2(0.0, ROW_GAP / 2.0));
        let response = ui.interact(grab, control_id(action), Sense::click_and_drag());
        sink.act(action, fader_input(&response, rect, false, None));
        let painter = ui.painter();
        painter.rect_filled(rect, CornerRadius::ZERO, p.lane);
        let level = rect.width() * view.fx_level.clamp(0.0, 1.0);
        let filled = Rect::from_min_size(rect.min, vec2(level, rect.height()));
        painter.rect_filled(filled, CornerRadius::ZERO, accent);
        let ghost_at = |v: f32| pos2(rect.left() + rect.width() * v, rect.center().y);
        if let Some(at) = mixer::ghost_marker(view.fx_level_ghost, ghost_at) {
            let marker = Rect::from_center_size(at, vec2(6.0, rect.height() + 6.0));
            painter.rect_stroke(
                marker,
                CornerRadius::same(1),
                Stroke::new(1.5, p.cue),
                StrokeKind::Outside,
            );
        }
        let caption = RichText::new(format!("FX {:.0}%", view.fx_level * 100.0))
            .color(p.text_dim)
            .size(SMALL_FONT);
        ui.add_sized(vec2(FX_WIDTH, FX_CAPTION_HEIGHT), Label::new(caption));
    });
}

/// The platter: a circle with a marker that turns with the playhead. Dragging it round
/// a paused deck touches, scratches (clockwise goes forward) and lets go, as the
/// controller's jog does; round a playing deck it turns the ring, which nudges the pitch,
/// without a touch. The deck's state when the drag starts holds for the whole drag.
fn jog(ui: &mut Ui, view: &DeckView, accent: Color32, sink: &mut Sink<'_>) {
    let p = theme::palette(ui.ctx());
    let (_, rect) = ui.allocate_space(Vec2::splat(JOG_SIDE));
    let id = ui.make_persistent_id(("jog", view.deck));
    let response = ui.interact(rect, id, Sense::click_and_drag());
    response.widget_info(|| WidgetInfo::labeled(Role::Image, true, "jog"));
    // An empty deck has nothing to scratch.
    if view.progress.is_some() {
        let mut amount = 0.0;
        if let Some((from, to)) = drag_path(&response) {
            let (from, to) = (from - rect.center(), to - rect.center());
            // Round the hub the angle swings wildly: moves there are ignored.
            let turned = if from.length().min(to.length()) < JOG_DEAD_ZONE {
                0.0
            } else {
                (from.x * to.y - from.y * to.x).atan2(from.dot(to))
            };
            amount = turned / std::f32::consts::TAU * f32::from(view.jog_steps_per_turn);
        }
        // Only a drag that moved does anything: a click, even a slow one, would hold a
        // playing deck.
        let dragging = moved_drag(&response);
        let nudging = id.with("nudging");
        let nudge = if dragging {
            ui.data_mut(|d| *d.get_temp_mut_or_insert_with(nudging, || view.playing))
        } else {
            ui.data_mut(|d| d.remove::<bool>(nudging));
            false
        };
        let touch = Action::JogTouch(view.deck);
        if dragging && !nudge {
            sink.button(&response, touch);
        } else {
            // A release always reaches the press.
            sink.release(&response, touch);
        }
        let steps = carry_steps(ui.ctx(), id, amount, dragging);
        if steps != 0 {
            let action = if nudge {
                Action::JogRing(view.deck)
            } else {
                Action::JogScratch(view.deck)
            };
            sink.send(UiCommand::Action(action, Input::Relative(steps)));
        }
    }
    let painter = ui.painter();
    let centre = rect.center();
    let radius = JOG_SIDE / 2.0 - JOG_STROKE;
    painter.circle(centre, radius, p.lane, Stroke::new(JOG_STROKE, p.text_dim));
    let angle = view.jog_turn * std::f32::consts::TAU;
    let direction = vec2(angle.sin(), -angle.cos());
    let colour = if view.playing { accent } else { p.text };
    let info = view.jog_display == JogDisplay::Info;
    // The info jog keeps the marker on the rim, out of the text.
    let inner = if info { 0.85 } else { 0.55 };
    painter.line_segment(
        [
            centre + direction * radius * inner,
            centre + direction * radius,
        ],
        Stroke::new(JOG_MARKER, colour),
    );
    if info {
        jog_info(
            ui,
            view,
            Rect::from_center_size(centre, Vec2::splat(radius * 1.4)),
        );
    }
}

/// VirtualDJ's info jog: the BPM, pitch and both times inside the wheel.
fn jog_info(ui: &mut Ui, view: &DeckView, rect: Rect) {
    let p = theme::palette(ui.ctx());
    let bpm = view
        .bpm
        .map_or_else(|| "–".to_owned(), |bpm| format!("{bpm:.2}"));
    ui.scope_builder(UiBuilder::new().max_rect(rect), |ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        ui.vertical_centered(|ui| {
            ui.label(RichText::new(bpm).strong());
            ui.label(RichText::new(pitch_text(view.pitch_percent)).size(SMALL_FONT));
            if !view.elapsed.is_empty() {
                ui.label(RichText::new(&view.elapsed).size(SMALL_FONT));
                ui.label(
                    RichText::new(format!("−{}", view.remaining))
                        .color(p.text_dim)
                        .size(SMALL_FONT),
                );
            }
        });
    });
}

/// The tempo slider (faster toward the bottom, as on the SB3) with the pitch in percent,
/// `width` by `height` (the middle row's). The mouse sets it through `sink`, and a
/// double-click puts it back to the original tempo.
fn pitch(
    ui: &mut Ui,
    view: &DeckView,
    accent: Color32,
    (width, height): (f32, f32),
    sink: &Sink<'_>,
) {
    let p = theme::palette(ui.ctx());
    let (column, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    let layout = Layout::top_down(Align::Center);
    ui.scope_builder(UiBuilder::new().max_rect(column).layout(layout), |ui| {
        let track_height = height - PITCH_LABEL_HEIGHT;
        let (rect, _) = ui.allocate_exact_size(vec2(width, track_height), Sense::hover());
        let x = rect.center().x;
        let track = Rect::from_x_y_ranges(
            x - PITCH_TRACK / 2.0..=x + PITCH_TRACK / 2.0,
            rect.top() + PITCH_HANDLE.y / 2.0..=rect.bottom() - PITCH_HANDLE.y / 2.0,
        );
        let action = Action::Tempo(view.deck);
        let response = ui.interact(rect, control_id(action), Sense::click_and_drag());
        response.widget_info(|| WidgetInfo::labeled(Role::Slider, true, "pitch"));
        // The slider's 0 is at the top: the other way round from a fader. A press on the
        // handle takes it where it is.
        let handle = Handle {
            value: 1.0 - view.pitch_slider,
            reach: PITCH_HANDLE.y / 2.0 + 1.0,
        };
        let input = match grab_fader(&response, track, true, Some(PITCH_RESET), handle) {
            Some(Input::Absolute(v)) => Some(Input::Absolute(1.0 - v)),
            other => other,
        };
        sink.act(action, input);
        let painter = ui.painter();
        painter.rect_filled(track, CornerRadius::ZERO, p.lane);
        let centre_mark =
            Rect::from_center_size(pos2(x, track.center().y), vec2(PITCH_HANDLE.x * 0.6, 1.0));
        painter.rect_filled(centre_mark, CornerRadius::ZERO, p.text_dim);
        let y = track.top() + track.height() * view.pitch_slider.clamp(0.0, 1.0);
        let handle = Rect::from_center_size(pos2(x, y), PITCH_HANDLE);
        painter.rect_filled(handle, CornerRadius::same(1), accent);
        let ghost_at = |v: f32| pos2(x, track.top() + track.height() * v);
        if let Some(at) = mixer::ghost_marker(view.pitch_ghost, ghost_at) {
            let marker = Rect::from_center_size(at, PITCH_HANDLE + vec2(4.0, 4.0));
            painter.rect_stroke(
                marker,
                CornerRadius::same(1),
                Stroke::new(1.5, p.cue),
                StrokeKind::Outside,
            );
        }
        ui.label(RichText::new(pitch_text(view.pitch_percent)).size(SMALL_FONT));
    });
}

/// VINYL, SLIP and KEYLOCK: each one's text, whether it is on and what it toggles.
fn state_buttons(view: &DeckView) -> [(&'static str, bool, Action); 3] {
    let deck = view.deck;
    [
        ("VINYL", view.vinyl, Action::VinylToggle(deck)),
        ("SLIP", view.slip, Action::SlipToggle(deck)),
        ("KEYLOCK", view.keylock, Action::KeylockToggle(deck)),
    ]
}

/// VINYL, SLIP and KEYLOCK, each lit while on; the mouse toggles them through `sink`.
fn states(ui: &mut Ui, view: &DeckView, accent: Color32, sink: &mut Sink<'_>) {
    ui.vertical(|ui| {
        let size = vec2(STATE_WIDTH, SLOT_HEIGHT);
        for (text, on, action) in state_buttons(view) {
            button(ui, sink, size, text, on.then_some(accent), action);
        }
    });
}

/// CUE (lit while previewing), PLAY (lit while playing) and SYNC (one-shot: never lit);
/// the mouse presses them through `sink`.
fn transport(ui: &mut Ui, view: &DeckView, sink: &mut Sink<'_>) {
    let p = theme::palette(ui.ctx());
    let deck = view.deck;
    ui.vertical(|ui| {
        // Each in line with the pad mode and the two rows of pads beside it.
        let size = vec2(TRANSPORT_WIDTH, BUTTON_HEIGHT);
        let cue = view.cueing.then_some(p.cue);
        button(ui, sink, size, "CUE", cue, Action::Cue(deck));
        let play = view.playing.then_some(p.play_lit);
        button(ui, sink, size, "PLAY", play, Action::PlayPause(deck));
        button(ui, sink, size, "SYNC", None, Action::Sync(deck));
    });
}

/// The pad mode's dropdown over the 2×4 pads; a choice asks `sink` to switch.
fn pads_column(
    ui: &mut Ui,
    view: &DeckView,
    accent: Color32,
    pads_width: f32,
    sink: &mut Sink<'_>,
) {
    ui.vertical(|ui| {
        ui.set_width(pads_width);
        let p = theme::palette(ui.ctx());
        let arrows = 2.0 * (PAD_ARROW_WIDTH + PAD_GAP);
        let size = vec2(PAD_MODE_WIDTH.min(pads_width - arrows), BUTTON_HEIGHT);
        let id = ("pad mode", view.deck);
        let text = RichText::new(view.pad_mode).color(accent);
        let side = Side::of(view.deck);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = PAD_GAP;
            pad_mode_arrow(ui, view, -1, sink);
            dropdown_text(ui, "pad mode", id, size, text, p.lane, |ui| {
                for (mode, label) in PAD_MODES {
                    let chosen = ui.selectable_label(mode == view.pad_mode_choice, label);
                    if chosen.clicked() && mode != view.pad_mode_choice {
                        let action = Action::PadMode(side, mode);
                        sink.send(UiCommand::Action(action, Input::Press));
                    }
                }
            });
            pad_mode_arrow(ui, view, 1, sink);
        });
        let pad = vec2((pads_width - PAD_GAP * 3.0) / 4.0, BUTTON_HEIGHT);
        // The window's SHIFT, as it is when a pad goes down, picks the second beat jumps.
        let shift = ui.input(|i| i.modifiers.shift);
        for (row, pads) in view.pads.chunks(4).enumerate() {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = PAD_GAP;
                for (column, pad_view) in pads.iter().enumerate() {
                    let fill = pad_view.colour.map_or(accent, |c| p.mark(Some(c)));
                    let lit = pad_view.lit.then_some(fill);
                    let (rect, _) = ui.allocate_exact_size(pad, Sense::hover());
                    let mut look = ButtonLook::Idle;
                    let slot = HotcueSlot::ALL[row * 4 + column];
                    if let Some(action) = window_pad_action(
                        view.pad_mode_choice,
                        view.pad_hardware_mode,
                        side,
                        view.deck,
                        slot,
                        shift,
                    ) {
                        let id = ("pad", view.deck, slot);
                        let response =
                            ui.interact(rect, Id::NULL.with(id), Sense::click_and_drag());
                        let hotcue = matches!(action, Action::Hotcue(..));
                        let press = context_press(&response);
                        // On macOS Ctrl + click is a right-click: it presses no hotcue.
                        if hotcue && press.right_click {
                            sink.release(&response, action);
                            // It presses nothing, so it does not look pressed.
                            look = if response.hovered() {
                                ButtonLook::Hovered
                            } else {
                                ButtonLook::Idle
                            };
                        } else {
                            sink.button(&response, action);
                            look = ButtonLook::of(&response);
                        }
                        // A hotcue pad's right-click opens its editor.
                        if hotcue && press.opened {
                            open_hotcue_menu(ui, view, slot, pad_view.lit, rect);
                        }
                    }
                    indicator(ui, rect, &pad_view.label, lit, look);
                }
            });
        }
    });
}

/// Opens the hotcue editor on `view`'s hotcue `slot` (`set` when it holds one), where the
/// pointer is, else under the pad at `rect`.
fn open_hotcue_menu(ui: &Ui, view: &DeckView, slot: HotcueSlot, set: bool, rect: Rect) {
    let mark = view
        .hotcue_marks
        .as_ref()
        .and_then(|marks| marks[slot.index()].as_ref());
    let target = HotcueTarget::new(view.deck, slot, view.track, set, mark);
    let at = ui
        .ctx()
        .pointer_interact_pos()
        .unwrap_or(rect.left_bottom());
    hotcue_menu::open(ui.ctx(), target, at);
}

/// The loop: < and > (halve and double it) in line with the pad mode, then its size (an
/// auto loop of that size) over IN and OUT, beside the two rows of pads. The mouse presses
/// them through `sink`.
fn loop_column(ui: &mut Ui, view: &DeckView, sink: &mut Sink<'_>) {
    let p = theme::palette(ui.ctx());
    let deck = view.deck;
    ui.vertical(|ui| {
        let size = vec2(LOOP_WIDTH, BUTTON_HEIGHT);
        let half = vec2((LOOP_WIDTH - PAD_GAP) / 2.0, BUTTON_HEIGHT);
        let looping = view.loop_active.then_some(p.hotcue);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = PAD_GAP;
            button(ui, sink, half, "<", None, Action::LoopHalve(deck));
            button(ui, sink, half, ">", None, Action::LoopDouble(deck));
        });
        let beats = &view.loop_beats;
        button(ui, sink, size, beats, looping, Action::AutoLoop(deck));
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = PAD_GAP;
            button(ui, sink, half, "IN", looping, Action::LoopIn(deck));
            button(ui, sink, half, "OUT", looping, Action::LoopOut(deck));
        });
    });
}

/// The pad mode `step` (−1 or 1) along [`PAD_MODES`] from `view`'s, wrapping round.
fn stepped_pad_mode(view: &DeckView, step: isize) -> PadMode {
    let count = PAD_MODES.len() as isize;
    let at = PAD_MODES
        .iter()
        .position(|&(mode, _)| mode == view.pad_mode_choice)
        .unwrap_or(0) as isize;
    PAD_MODES[(at + step).rem_euclid(count) as usize].0
}

/// The arrow beside the pad mode's dropdown that steps it back (`step` −1, ◀) or on (1,
/// ▶) through the dropdown's modes; the mouse presses it through `sink`.
fn pad_mode_arrow(ui: &mut Ui, view: &DeckView, step: isize, sink: &mut Sink<'_>) {
    let p = theme::palette(ui.ctx());
    let size = vec2(PAD_ARROW_WIDTH, BUTTON_HEIGHT);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let name = if step < 0 {
        "previous pad mode"
    } else {
        "next pad mode"
    };
    let id = Id::NULL.with(("pad mode arrow", view.deck, step));
    let response = ui.interact(rect, id, Sense::click_and_drag());
    response.widget_info(|| WidgetInfo::labeled(Role::Button, true, name));
    let action = Action::PadMode(Side::of(view.deck), stepped_pad_mode(view, step));
    sink.button(&response, action);
    let look = ButtonLook::of(&response);
    ui.painter()
        .rect_filled(look.rect(rect), CornerRadius::same(2), look.fill(p.lane));
    // A triangle pointing the way it steps.
    let c = rect.center();
    let (w, h) = (3.5 * step as f32, 4.5);
    let points = vec![c + vec2(w, 0.0), c + vec2(-w, -h), c + vec2(-w, h)];
    ui.painter()
        .add(Shape::convex_polygon(points, p.text_dim, Stroke::NONE));
}

/// A dropdown named `name` of `size` drawn like an [`indicator`]: `text`, filled with
/// `lit`'s colour when lit; `contents` fills its list.
fn dropdown(
    ui: &mut Ui,
    name: &str,
    id: impl std::hash::Hash + std::fmt::Debug,
    size: Vec2,
    text: &str,
    lit: Option<Color32>,
    contents: impl FnOnce(&mut Ui),
) {
    let p = theme::palette(ui.ctx());
    let (fill, colour) = match lit {
        Some(colour) => (colour, p.on_accent),
        None => (p.lane, p.text_dim),
    };
    let text = RichText::new(text).color(colour);
    dropdown_text(ui, name, id, size, text, fill, contents);
}

/// A dropdown named `name` of `size` showing `text` on `fill`; `contents` fills its list.
fn dropdown_text(
    ui: &mut Ui,
    name: &str,
    id: impl std::hash::Hash + std::fmt::Debug,
    size: Vec2,
    text: RichText,
    fill: Color32,
    contents: impl FnOnce(&mut Ui),
) {
    ui.scope(|ui| {
        let idle = &mut ui.visuals_mut().widgets.inactive;
        idle.weak_bg_fill = fill;
        idle.bg_stroke = Stroke::NONE;
        ui.spacing_mut().interact_size.y = size.y;
        if text.is_empty() {
            // An arrow alone: no room kept for text.
            let spacing = ui.spacing_mut();
            spacing.button_padding.x = 0.0;
            spacing.icon_spacing = 0.0;
            spacing.item_spacing.x = 0.0;
        }
        let response = egui::ComboBox::from_id_salt(id)
            .width(size.x)
            .selected_text(text.size(SMALL_FONT))
            .truncate()
            .show_ui(ui, contents)
            .response;
        // The list's name for screen readers; the button shows the choice.
        ui.ctx()
            .accesskit_node_builder(response.id, |node| node.set_label(name));
    });
}

/// The id of the deck's control that sends `action`.
fn control_id(action: Action) -> Id {
    Id::NULL.with(("deck", action))
}

/// An [`indicator`] of `size` that the mouse presses: it sends `action` through `sink`,
/// and looks pressed while held.
fn button(
    ui: &mut Ui,
    sink: &mut Sink<'_>,
    size: Vec2,
    text: &str,
    lit: Option<Color32>,
    action: Action,
) {
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let response = ui.interact(rect, control_id(action), Sense::click_and_drag());
    sink.button(&response, action);
    indicator(ui, rect, text, lit, ButtonLook::of(&response));
}

/// A box at `rect` with `text`, filled with `lit`'s colour when lit, in `look`.
fn indicator(ui: &mut Ui, rect: Rect, text: &str, lit: Option<Color32>, look: ButtonLook) {
    let p = theme::palette(ui.ctx());
    let (fill, colour) = match lit {
        // A dark fill (a hotcue's saved colour) takes light text.
        Some(colour) if theme::luminance(colour) < DARK_FILL => (colour, Color32::WHITE),
        Some(colour) => (colour, p.on_accent),
        None => (p.lane, p.text_dim),
    };
    ui.painter()
        .rect_filled(look.rect(rect), CornerRadius::same(2), look.fill(fill));
    if !text.is_empty() {
        let label = Label::new(RichText::new(text).color(colour).size(SMALL_FONT))
            .selectable(false)
            .truncate();
        ui.place(rect, label);
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use engine::{Grid, Hotcue};
    use library::TrackRow;

    use super::*;
    use crate::model::DeckInfo;

    fn grid(first_beat_frames: f64) -> Option<Grid> {
        Some(Grid {
            beat_period_frames: 100.0,
            first_beat_frames,
        })
    }

    #[test]
    fn beat_counter_counts_bars() {
        let at = |first, position| beat_counter(grid(first), position);
        assert_eq!(at(0.0, 0.0).as_deref(), Some("1.1"));
        assert_eq!(at(0.0, 150.0).as_deref(), Some("1.2"));
        assert_eq!(at(0.0, 400.0).as_deref(), Some("2.1"));
        // Three beats before the first beat: bar 0 counts backwards.
        assert_eq!(at(300.0, 0.0).as_deref(), Some("0.2"));
        assert_eq!(beat_counter(None, 0.0), None);
    }

    #[test]
    fn pitch_reads_with_a_sign_and_never_minus_zero() {
        assert_eq!(pitch_text(0.0), "+0.0%");
        assert_eq!(pitch_text(-0.0), "+0.0%");
        assert_eq!(pitch_text(-0.04), "+0.0%");
        assert_eq!(pitch_text(1.25), "+1.3%");
        assert_eq!(pitch_text(-2.0), "-2.0%");
    }

    fn deck(number: usize) -> DeckId {
        DeckId::new(number - 1).unwrap()
    }

    /// A 5:29 track at 126 BPM on deck 1, 57.1 s in.
    fn loaded() -> (DeckSnapshot, ControlFrame) {
        let snapshot = DeckSnapshot {
            loaded: true,
            playing: true,
            // 57.1 s and 329.0 s at 44.1 kHz.
            position_frames: 2_518_110.0,
            duration_frames: 14_508_900,
            track_sample_rate: 44_100,
            tempo_ratio: 1.0,
            grid: Grid::from_bpm(126.0, 0.0, 44_100),
            load_seq: 3,
            ..DeckSnapshot::default()
        };
        let mut frame = ControlFrame::default();
        frame.decks[0] = Some(DeckInfo {
            load_seq: 3,
            row: TrackRow {
                id: 1,
                title: Some("Just Us (Extended Mix)".into()),
                artist: Some("Someone".into()),
                bpm: Some(126.0),
                key: Some(21),
                duration_frames: Some(14_508_900),
                sample_rate: Some(44_100),
                has_cues: true,
                missing: false,
            },
            path: PathBuf::from("/music/just-us.mp3"),
            overview: None,
            detail: None,
            stems: None,
            hotcue_marks: Default::default(),
        });
        (snapshot, frame)
    }

    fn view(snapshot: &DeckSnapshot, frame: &ControlFrame) -> DeckView {
        DeckView::new(
            deck(1),
            snapshot,
            &ChannelSnapshot::default(),
            frame,
            &SamplerSnapshot::default(),
        )
    }

    #[test]
    fn deck_view_formats_like_vdj() {
        let (snapshot, frame) = loaded();
        let view = view(&snapshot, &frame);
        assert_eq!(view.elapsed, "0:57.1");
        // 329.0 s − 57.1 s = 271.9 s (ruling R1); times are cut, not rounded (R2).
        assert_eq!(view.remaining, "4:31.9");
        assert_eq!(view.bpm, Some(126.0));
        assert_eq!(view.tag, "1");
        assert_eq!(view.title, "Just Us");
        assert_eq!(view.remix.as_deref(), Some("Extended Mix"));
        assert_eq!(view.artist, "Someone");
        assert_eq!(view.key.as_deref(), Some("8A"));
        assert_eq!(view.loop_beats, "4");
        assert!(view.playing && !view.cueing);
    }

    #[test]
    fn overview_layers_blend() {
        // One bucket in a one-column overview: low, mid and high, back to front.
        let waveform = Waveform {
            frames_per_bucket: 256,
            buckets: vec![[255, 200, 100, 50]],
        };
        let mut mesh = Mesh::default();
        let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(1.0, 40.0));
        let bands = band_colours(WaveformPalette::Rgb, &theme::Palette::DARK);
        overview_columns(&mut mesh, rect, &waveform, bands);
        // Four corners a layer.
        let alphas: Vec<u8> = mesh
            .vertices
            .iter()
            .step_by(4)
            .map(|v| v.color.a())
            .collect();
        assert_eq!(alphas.len(), 3, "{alphas:?}");
        assert_eq!(alphas[0], 255, "{alphas:?}");
        assert!(alphas[1] < 255 && alphas[2] < 255, "{alphas:?}");
    }

    #[test]
    fn an_overview_column_shows_every_band_peak() {
        // Two buckets under one column: the kick's and a bright one.
        let waveform = Waveform {
            frames_per_bucket: 256,
            buckets: vec![[255, 200, 10, 10], [100, 20, 150, 120]],
        };
        let mut mesh = Mesh::default();
        let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(1.0, 40.0));
        let bands = band_colours(WaveformPalette::Rgb, &theme::Palette::DARK);
        overview_columns(&mut mesh, rect, &waveform, bands);
        // Each layer's half-height, from its four corners' top and bottom.
        let reaches: Vec<f32> = mesh
            .vertices
            .chunks(4)
            .map(|corners| {
                let top = corners.iter().map(|v| v.pos.y).fold(f32::MAX, f32::min);
                let bottom = corners.iter().map(|v| v.pos.y).fold(f32::MIN, f32::max);
                (bottom - top) / 2.0
            })
            .collect();
        let expected = [200.0, 150.0, 120.0].map(|level| 20.0 * level / 255.0);
        assert_eq!(reaches.len(), 3, "{reaches:?}");
        for (reach, want) in reaches.iter().zip(expected) {
            assert!(
                (reach - want).abs() < 1e-3,
                "{reaches:?} against {expected:?}"
            );
        }
    }

    #[test]
    fn times_are_cut_and_bpm_follows_the_tempo() {
        let (mut snapshot, frame) = loaded();
        // 61.99 s: shows 1:01.9, not 1:02.0.
        snapshot.position_frames = 61.99 * 44_100.0;
        snapshot.tempo_ratio = 1.08;
        let view = view(&snapshot, &frame);
        assert_eq!(view.elapsed, "1:01.9");
        let bpm = view.bpm.unwrap();
        assert!((bpm - 136.08).abs() < 1e-9, "{bpm}");
        assert!((view.pitch_percent - 8.0).abs() < 1e-9);
    }

    #[test]
    fn a_track_the_engine_does_not_play_yet_is_not_shown() {
        let (mut snapshot, frame) = loaded();
        snapshot.load_seq = 2;
        let view = view(&snapshot, &frame);
        assert_eq!(view.title, "");
        assert_eq!(view.key, None);
        assert!(view.path.is_none());
        let empty = super::tests::view(&DeckSnapshot::default(), &ControlFrame::default());
        assert_eq!(empty.elapsed, "");
        assert_eq!(empty.bpm, None);
        assert_eq!(empty.progress, None);
    }

    #[test]
    fn pads_follow_the_mode() {
        let (mut snapshot, mut frame) = loaded();
        snapshot.hotcues[0] = Some(Hotcue::point(1000.0));
        snapshot.hotcues[2] = Some(Hotcue::point(5000.0));
        frame.control.sides[0].hardware_mode = Some(HardwarePadMode::HotCue);
        let pads = view(&snapshot, &frame).pads;
        assert!(pads[0].lit && !pads[1].lit && pads[2].lit);
        assert_eq!(pads[0].label, "1");

        frame.control.sides[0].pad_mode = PadMode::BeatJump;
        let pads = view(&snapshot, &frame).pads;
        assert_eq!(pads[0].label, "−4");
        assert_eq!(pads[1].label, "+4");

        frame.control.sides[0].pad_mode = PadMode::LoopRoll;
        assert_eq!(view(&snapshot, &frame).pads[0].label, "1/16");
        frame.control.sides[0].pad_mode = PadMode::Slicer;
        assert_eq!(view(&snapshot, &frame).pads[7].label, "8");
        frame.control.sides[0].pad_mode = PadMode::PadScratch;
        assert_eq!(view(&snapshot, &frame).pads[0], PadView::default());

        frame.control.sides[0].pad_mode = PadMode::Hardware;
        frame.control.sides[0].hardware_mode = Some(HardwarePadMode::Sampler);
        frame.samples_loaded[1] = true;
        let pads = view(&snapshot, &frame).pads;
        assert_eq!(pads[1].label, "S 2");
        assert!(!pads[0].lit && pads[1].lit);
    }

    #[test]
    fn pad_mode_name_is_pads_until_a_button() {
        let view = view(&DeckSnapshot::default(), &ControlFrame::default());
        assert_eq!(view.pad_mode, "pads");
        // Before any mode button the pads show the hotcues.
        assert_eq!(view.pads[0].label, "1");
    }

    #[test]
    fn no_sync_badge() {
        // `DeckView` has no SYNC or MASTER field: the engine's sync is one-shot, so there
        // is no lasting state to show. That is checked at compile time by the struct
        // itself; this test only documents it.
        let (snapshot, frame) = loaded();
        let view = view(&snapshot, &frame);
        assert!(view.playing);
    }

    #[test]
    fn beats_read_as_fractions_or_whole_numbers() {
        assert_eq!(beats_text(0.0625), "1/16");
        assert_eq!(beats_text(0.5), "1/2");
        assert_eq!(beats_text(4.0), "4");
        assert_eq!(beats_text(1.5), "1.5");
        assert_eq!(clock(0.0, 44_100), "0:00.0");
        assert_eq!(clock(-10.0, 44_100), "0:00.0");
        assert_eq!(clock(600.0 * 44_100.0, 44_100), "10:00.0");
    }

    fn loop_hotcue(frames: f64) -> Hotcue {
        Hotcue {
            frames,
            loop_frames: Some(500.0),
        }
    }

    fn mark(color: Option<u32>, name: Option<&str>) -> Option<HotcueMark> {
        Some(HotcueMark::new(color, name.map(str::to_owned)))
    }

    #[test]
    fn a_loop_slot_shows_as_a_loop_on_its_pad() {
        let (mut snapshot, frame) = loaded();
        snapshot.hotcues[3] = Some(loop_hotcue(1000.0));
        snapshot.hotcues[4] = Some(Hotcue::point(2000.0));
        let pads = view(&snapshot, &frame).pads;
        assert_eq!(pads[3].label, "4 loop");
        assert!(pads[3].lit);
        assert_eq!(pads[4].label, "5");
        assert_eq!(pads[5].label, "6");
    }

    #[test]
    fn a_named_hotcue_shows_its_name() {
        let (mut snapshot, mut frame) = loaded();
        snapshot.hotcues[1] = Some(Hotcue::point(1000.0));
        snapshot.hotcues[3] = Some(loop_hotcue(2000.0));
        let info = frame.decks[0].as_mut().unwrap();
        Arc::make_mut(&mut info.hotcue_marks)[1] = mark(None, Some("Drop"));
        Arc::make_mut(&mut info.hotcue_marks)[3] = mark(None, Some(" Break "));
        // A name on an empty slot shows nothing.
        Arc::make_mut(&mut info.hotcue_marks)[5] = mark(None, Some("Ghost"));
        let pads = view(&snapshot, &frame).pads;
        assert_eq!(pads[1].label, "2 Drop");
        assert_eq!(pads[3].label, "4 Break");
        assert_eq!(pads[5].label, "6");
    }

    #[test]
    fn hotcue_colours_tint_pads() {
        let (mut snapshot, mut frame) = loaded();
        snapshot.hotcues[0] = Some(Hotcue::point(1000.0));
        snapshot.hotcues[1] = Some(Hotcue::point(2000.0));
        let info = frame.decks[0].as_mut().unwrap();
        Arc::make_mut(&mut info.hotcue_marks)[0] = mark(Some(0xff0000), None);
        let pads = view(&snapshot, &frame).pads;
        assert_eq!(pads[0].colour, Some(Color32::from_rgb(0xff, 0, 0)));
        // No colour: the pad keeps the deck's accent.
        assert_eq!(pads[1].colour, None);
    }
}
