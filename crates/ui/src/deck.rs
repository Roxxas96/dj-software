//! The deck panels between the waveforms and the browser: the left side's active deck on
//! the left, the right side's on the right, each drawn from a [`DeckView`].

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::Sender;
use std::time::Instant;

use actions::{Action, ControlState, HardwarePadMode, Input, PadMode, Side, SideControl};
use analysis::{Key, KeyNotation, Waveform};
use egui::{
    Align, Color32, CornerRadius, Label, Layout, Mesh, Rect, RichText, Sense, Shape, Stroke,
    StrokeKind, TextureHandle, Ui, UiBuilder, Vec2, pos2, vec2,
};
use engine::{
    ChannelSnapshot, DeckId, DeckSnapshot, EffectKind, EngineMonitor, Grid, SAMPLER_SLOTS,
    SamplerSnapshot,
};

use crate::command::{UiCommand, ViewChange};
use crate::cover::{self, Covers};
use crate::layout::{Sections, deck_slots};
use crate::mixer;
use crate::model::{ControlFrame, GHOST_TEMPO};
use crate::theme;
use crate::view::{JogDisplay, TimeMode};
use crate::waveform::{Playhead, colour};

/// A record's speed: the jog's marker turns at this at the original tempo.
const JOG_RPM: f64 = 100.0 / 3.0;
/// The share of the panels' width each deck panel takes; the mixer has the rest.
const PANEL_SHARE: f32 = 0.38;
/// The same in the 4-Decks view, where the mixer has four strips to fit.
const FOUR_PANEL_SHARE: f32 = 0.34;
/// The outline of a side's active deck in the 4-Decks view.
const ACTIVE_BORDER: f32 = 2.0;
const PANEL_MARGIN: i8 = 6;
const COVER_SIDE: f32 = 64.0;
/// The right of the header: BPM, key and times.
const TIMES_WIDTH: f32 = 130.0;
/// A line of the header's small text.
const HEADER_LINE: f32 = 16.0;
const OVERVIEW_HEIGHT: f32 = 32.0;
/// The space between a panel's rows, and between the controls of a column.
const ROW_GAP: f32 = 4.0;
const FX_WIDTH: f32 = 96.0;
const SLOT_HEIGHT: f32 = 22.0;
const FX_LEVEL_HEIGHT: f32 = 6.0;
/// The effect level's caption under its bar.
const FX_CAPTION_HEIGHT: f32 = 14.0;
/// The pad mode dropdown's width.
const PAD_MODE_WIDTH: f32 = 120.0;
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
    /// What the pads do, as [`SideControl::pad_mode_label`] names it.
    pub pad_mode: &'static str,
    /// The pad mode the app switches to, [`PadMode::Hardware`] while the SB3's own mode
    /// buttons drive the pads.
    pub pad_mode_choice: PadMode,
    pub pads: [PadView; 8],
    /// The stems' state ("stems", "stems 40%").
    pub stems: Option<String>,
    /// The track's file, for its cover.
    pub path: Option<PathBuf>,
    pub overview: Option<Arc<Waveform>>,
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
            pad_mode: side.pad_mode_label(),
            pad_mode_choice: side.pad_mode,
            pads: pads(
                side,
                &frame.control,
                snapshot,
                &frame.samples_loaded,
                sampler,
            ),
            stems: info.and_then(|i| i.stems.clone()),
            path: info.map(|i| i.path.clone()),
            overview: info.and_then(|i| i.overview.clone()),
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
    samples_loaded: &[bool; SAMPLER_SLOTS],
    sampler: &SamplerSnapshot,
) -> [PadView; 8] {
    std::array::from_fn(|pad| {
        let number = pad + 1;
        let (label, lit) = match side.pad_mode {
            PadMode::Hardware => match side.hardware_mode {
                Some(HardwarePadMode::Sampler | HardwarePadMode::Trans) => (
                    format!("S {number}"),
                    // A sounding slot is loaded even if the frame has not caught up.
                    samples_loaded[pad] || sampler.playing[pad],
                ),
                _ => (number.to_string(), snapshot.hotcues[pad].is_some()),
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
        PadView { label, lit }
    })
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

/// What the engine reports for every deck, as the panels read it. Without a monitor every
/// deck is empty.
#[derive(Debug, Default)]
pub struct DeckReads {
    /// Each deck's snapshot and the time its block began rendering (0: unknown).
    pub decks: [(DeckSnapshot, u64); 4],
    pub channels: [ChannelSnapshot; 4],
    pub sampler: SamplerSnapshot,
}

impl DeckReads {
    pub fn read(monitor: Option<&EngineMonitor>) -> Self {
        let Some(m) = monitor else {
            return Self::default();
        };
        Self {
            decks: DeckId::ALL.map(|id| m.deck_rendered_at(id)),
            channels: DeckId::ALL.map(|id| m.channel(id)),
            sampler: m.sampler(),
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
    /// right side's at its right, and returns the space between them, for the mixer.
    /// Without `monitor` every deck is empty.
    pub fn show(
        &mut self,
        ui: &mut Ui,
        frame: &ControlFrame,
        monitor: Option<&EngineMonitor>,
        covers: &mut Covers,
        sections: &Sections,
        commands: &Sender<UiCommand>,
    ) -> Rect {
        let reads = DeckReads::read(monitor);
        self.show_reads(ui, frame, &reads, covers, sections, commands)
    }

    /// [`DeckPanels::show`] from what was already read from the engine.
    pub fn show_reads(
        &mut self,
        ui: &mut Ui,
        frame: &ControlFrame,
        reads: &DeckReads,
        covers: &mut Covers,
        sections: &Sections,
        commands: &Sender<UiCommand>,
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
        // Two decks: one panel a side. Four: two stacked, the side's first deck on top.
        let rects: Vec<Rect> = if four {
            [left, right]
                .into_iter()
                .flat_map(|side| {
                    let half = side.height() / 2.0;
                    [
                        Rect::from_min_size(side.min, vec2(side.width(), half)),
                        Rect::from_min_size(
                            pos2(side.left(), side.top() + half),
                            vec2(side.width(), half),
                        ),
                    ]
                })
                .collect()
        } else {
            vec![left, right]
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
            ui.scope_builder(UiBuilder::new().max_rect(rect), |ui| {
                // Nothing a panel draws reaches its neighbours.
                ui.set_clip_rect(rect.intersect(ui.clip_rect()));
                panel(ui, &view, cover.as_ref(), sections, commands);
            });
            // With four panels, the ones the controller plays are the ones outlined.
            if four && active.contains(&deck) {
                let accent = theme::palette(ui.ctx()).deck_accent(deck);
                ui.painter().rect_stroke(
                    rect,
                    CornerRadius::ZERO,
                    Stroke::new(ACTIVE_BORDER, accent),
                    StrokeKind::Inside,
                );
            }
        }
        Rect::from_min_max(
            pos2(left.right(), area.top()),
            pos2(right.left(), area.bottom()),
        )
    }
}

/// The height a deck panel needs to show every area of `sections`.
pub fn panel_height(sections: &Sections) -> f32 {
    let mut height = 2.0 * f32::from(PANEL_MARGIN) + COVER_SIDE;
    if sections.overview {
        height += ROW_GAP + OVERVIEW_HEIGHT;
    }
    if sections.compact {
        return height + ROW_GAP + BUTTON_HEIGHT;
    }
    height + ROW_GAP + middle_height(sections) + ROW_GAP + bottom_height()
}

/// The height the deck panels' row needs for `sections`: one panel, or two stacked in the
/// 4-Decks view.
pub fn row_height(sections: &Sections) -> f32 {
    let panels = if sections.compact { 2.0 } else { 1.0 };
    panels * panel_height(sections)
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

/// The bottom row's height: three buttons, as tall as the pad mode over two rows of pads.
fn bottom_height() -> f32 {
    3.0 * BUTTON_HEIGHT + 2.0 * ROW_GAP
}

/// Draws `view` filling `ui`, with `cover` (the plain tile without one), showing the
/// `sections` the layout has. A click on the time asks `commands` to cycle it.
pub fn panel(
    ui: &mut Ui,
    view: &DeckView,
    cover: Option<&TextureHandle>,
    sections: &Sections,
    commands: &Sender<UiCommand>,
) {
    let p = theme::palette(ui.ctx());
    let accent = p.deck_accent(view.deck);
    egui::Frame::new()
        .fill(p.panel)
        .inner_margin(PANEL_MARGIN)
        .show(ui, |ui| {
            ui.set_min_size(ui.available_size());
            ui.spacing_mut().item_spacing = vec2(6.0, ROW_GAP);
            header(ui, view, cover, accent, commands);
            if sections.overview {
                overview(ui, view);
            }
            if sections.compact {
                compact_row(ui, view, accent, sections.loop_controls);
                return;
            }
            let middle = middle_height(sections);
            ui.horizontal(|ui| {
                ui.set_height(middle);
                if sections.fx {
                    fx(ui, view, accent, sections.fx_level, commands);
                }
                if sections.jog {
                    jog(ui, view, accent);
                }
                if sections.pitch {
                    let width = if sections.jog {
                        PITCH_WIDTH
                    } else {
                        PITCH_COMPACT_WIDTH
                    };
                    pitch(ui, view, accent, width, middle);
                }
                states(ui, view, accent);
            });
            ui.horizontal_top(|ui| {
                transport(ui, view);
                pads_grid(ui, view, accent, sections, commands);
            });
        });
}

/// CUE, PLAY, SYNC, the three states and (when `loop_controls`) the loop's size, IN and
/// OUT in one row, for the compact panel; narrower than they like when the row is short.
fn compact_row(ui: &mut Ui, view: &DeckView, accent: Color32, loop_controls: bool) {
    let p = theme::palette(ui.ctx());
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
        indicator(ui, size(transport), "CUE", view.cueing.then_some(p.cue));
        indicator(
            ui,
            size(transport),
            "PLAY",
            view.playing.then_some(p.play_lit),
        );
        indicator(ui, size(transport), "SYNC", None);
        indicator(ui, size(state), "VINYL", view.vinyl.then_some(accent));
        indicator(ui, size(state), "SLIP", view.slip.then_some(accent));
        indicator(ui, size(state), "KEYLOCK", view.keylock.then_some(accent));
        if loop_controls {
            let lit = view.loop_active.then_some(p.hotcue);
            indicator(ui, size(looping), &view.loop_beats, lit);
            indicator(ui, size(looping), "IN", lit);
            indicator(ui, size(looping), "OUT", lit);
        }
    });
}

/// The cover, title, artist, remix and stems, then BPM, key and times on the right, all
/// within the cover's height.
fn header(
    ui: &mut Ui,
    view: &DeckView,
    cover: Option<&TextureHandle>,
    accent: Color32,
    commands: &Sender<UiCommand>,
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
    let column = vec2(text_width, COVER_SIDE);
    ui.allocate_ui_with_layout(column, Layout::top_down(Align::Min), |ui| {
        ui.set_min_size(column);
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
        ui.add(Label::new(RichText::new(&view.artist).color(p.text_dim)).truncate());
        if let Some(remix) = &view.remix {
            ui.add(Label::new(RichText::new(remix).color(p.text_dim)).truncate());
        }
        if let Some(stems) = &view.stems {
            ui.label(RichText::new(stems).color(accent).size(SMALL_FONT));
        }
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
        if view.key.is_some() || view.beat_counter.is_some() {
            ui.allocate_ui_with_layout(line, row, |ui| {
                if let Some(counter) = &view.beat_counter {
                    ui.label(RichText::new(counter).color(p.text_dim));
                }
                if let Some(key) = &view.key {
                    ui.label(RichText::new(key).color(accent));
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
                    let _ = commands.send(UiCommand::View(ViewChange::CycleTime(view.deck)));
                }
            });
        }
    });
}

/// The whole track's waveform with the playhead.
fn overview(ui: &mut Ui, view: &DeckView) {
    let p = theme::palette(ui.ctx());
    let size = vec2(ui.available_width(), OVERVIEW_HEIGHT);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, CornerRadius::ZERO, p.lane);
    let mut mesh = Mesh::default();
    if let Some(waveform) = view.overview.as_deref() {
        overview_columns(&mut mesh, rect, waveform, p.text_dim, p.wave_high);
    }
    if let Some(progress) = view.progress {
        let x = rect.left() + rect.width() * progress.clamp(0.0, 1.0);
        let head = Rect::from_x_y_ranges(x - 1.0..=x + 1.0, rect.y_range());
        mesh.add_colored_rect(head, p.playhead);
    }
    painter.add(Shape::mesh(mesh));
}

/// One column per point, mirrored around the middle, as tall as the loudest bucket under
/// it and in its colour.
fn overview_columns(
    mesh: &mut Mesh,
    rect: Rect,
    waveform: &Waveform,
    silent: Color32,
    high: Color32,
) {
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
        let Some(loudest) = buckets[from..to].iter().max_by_key(|b| b[0]) else {
            continue;
        };
        let reach = half_height * f32::from(loudest[0]) / 255.0;
        let x = rect.left() + column as f32;
        let bar = Rect::from_min_max(pos2(x, middle - reach), pos2(x + 1.0, middle + reach));
        mesh.add_colored_rect(bar, colour(*loudest, silent, high));
    }
}

/// The three FX slots, each lit while on, over the effect level (when `level`). Each
/// slot's effect is a dropdown that asks `commands` to change it.
fn fx(ui: &mut Ui, view: &DeckView, accent: Color32, level: bool, commands: &Sender<UiCommand>) {
    let p = theme::palette(ui.ctx());
    ui.vertical(|ui| {
        ui.set_width(FX_WIDTH);
        for (slot, &(current, on)) in view.fx.iter().enumerate() {
            let size = vec2(FX_WIDTH, SLOT_HEIGHT);
            let lit = on.then_some(accent);
            let id = ("fx slot", view.deck, slot);
            let name = format!("FX {}", slot + 1);
            dropdown(ui, &name, id, size, effect_label(current), lit, |ui| {
                for kind in EffectKind::ALL {
                    let chosen = ui.selectable_label(kind == current, effect_label(kind));
                    if chosen.clicked() && kind != current {
                        let deck = view.deck;
                        let _ = commands.send(UiCommand::FxSlot { deck, slot, kind });
                    }
                }
            });
        }
        if !level {
            return;
        }
        let (rect, _) = ui.allocate_exact_size(vec2(FX_WIDTH, FX_LEVEL_HEIGHT), Sense::hover());
        let painter = ui.painter();
        painter.rect_filled(rect, CornerRadius::ZERO, p.lane);
        let level = rect.width() * view.fx_level.clamp(0.0, 1.0);
        let filled = Rect::from_min_size(rect.min, vec2(level, rect.height()));
        painter.rect_filled(filled, CornerRadius::ZERO, accent);
        let caption = RichText::new(format!("FX {:.0}%", view.fx_level * 100.0))
            .color(p.text_dim)
            .size(SMALL_FONT);
        ui.add_sized(vec2(FX_WIDTH, FX_CAPTION_HEIGHT), Label::new(caption));
    });
}

/// The platter: a circle with a marker that turns with the playhead.
fn jog(ui: &mut Ui, view: &DeckView, accent: Color32) {
    let p = theme::palette(ui.ctx());
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(JOG_SIDE), Sense::hover());
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

/// The tempo slider (faster toward the bottom, as on the SB3) with the pitch in percent.
/// `height` is the middle row's.
fn pitch(ui: &mut Ui, view: &DeckView, accent: Color32, width: f32, height: f32) {
    let p = theme::palette(ui.ctx());
    let (column, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    let layout = Layout::top_down(Align::Center);
    ui.scope_builder(UiBuilder::new().max_rect(column).layout(layout), |ui| {
        let track_height = height - PITCH_LABEL_HEIGHT;
        let (rect, _) = ui.allocate_exact_size(vec2(width, track_height), Sense::hover());
        let painter = ui.painter();
        let x = rect.center().x;
        let track = Rect::from_x_y_ranges(
            x - PITCH_TRACK / 2.0..=x + PITCH_TRACK / 2.0,
            rect.top() + PITCH_HANDLE.y / 2.0..=rect.bottom() - PITCH_HANDLE.y / 2.0,
        );
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

/// VINYL, SLIP and KEYLOCK, each lit while on.
fn states(ui: &mut Ui, view: &DeckView, accent: Color32) {
    ui.vertical(|ui| {
        let size = vec2(STATE_WIDTH, SLOT_HEIGHT);
        indicator(ui, size, "VINYL", view.vinyl.then_some(accent));
        indicator(ui, size, "SLIP", view.slip.then_some(accent));
        indicator(ui, size, "KEYLOCK", view.keylock.then_some(accent));
    });
}

/// CUE (lit while previewing), PLAY (lit while playing) and SYNC (one-shot: never lit).
fn transport(ui: &mut Ui, view: &DeckView) {
    let p = theme::palette(ui.ctx());
    ui.vertical(|ui| {
        // Each in line with the pad mode and the two rows of pads beside it.
        let size = vec2(TRANSPORT_WIDTH, BUTTON_HEIGHT);
        indicator(ui, size, "CUE", view.cueing.then_some(p.cue));
        indicator(ui, size, "PLAY", view.playing.then_some(p.play_lit));
        indicator(ui, size, "SYNC", None);
    });
}

/// The pads and the loop, whichever the layout shows.
fn pads_grid(
    ui: &mut Ui,
    view: &DeckView,
    accent: Color32,
    sections: &Sections,
    commands: &Sender<UiCommand>,
) {
    let loop_width = if sections.loop_controls {
        LOOP_WIDTH + PAD_GAP * 2.0
    } else {
        0.0
    };
    let pads_width = (ui.available_width() - loop_width).max(0.0);
    if sections.pads {
        pads_column(ui, view, accent, pads_width, commands);
    }
    if sections.loop_controls {
        loop_column(ui, view);
    }
}

/// The pad mode's dropdown over the 2×4 pads; a choice asks `commands` to switch.
fn pads_column(
    ui: &mut Ui,
    view: &DeckView,
    accent: Color32,
    pads_width: f32,
    commands: &Sender<UiCommand>,
) {
    ui.vertical(|ui| {
        ui.set_width(pads_width);
        let p = theme::palette(ui.ctx());
        let size = vec2(PAD_MODE_WIDTH.min(pads_width), BUTTON_HEIGHT);
        let id = ("pad mode", view.deck);
        let text = RichText::new(view.pad_mode).color(accent);
        dropdown_text(ui, "pad mode", id, size, text, p.lane, |ui| {
            for (mode, label) in PAD_MODES {
                let chosen = ui.selectable_label(mode == view.pad_mode_choice, label);
                if chosen.clicked() && mode != view.pad_mode_choice {
                    let action = Action::PadMode(Side::of(view.deck), mode);
                    let _ = commands.send(UiCommand::Action(action, Input::Press));
                }
            }
        });
        let pad = vec2((pads_width - PAD_GAP * 3.0) / 4.0, BUTTON_HEIGHT);
        for row in view.pads.chunks(4) {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = PAD_GAP;
                for p in row {
                    indicator(ui, pad, &p.label, p.lit.then_some(accent));
                }
            });
        }
    });
}

/// The loop: its size, IN and OUT.
fn loop_column(ui: &mut Ui, view: &DeckView) {
    let p = theme::palette(ui.ctx());
    ui.vertical(|ui| {
        let size = vec2(LOOP_WIDTH, BUTTON_HEIGHT);
        ui.label(RichText::new("LOOP").color(p.text_dim).size(SMALL_FONT));
        let looping = view.loop_active.then_some(p.hotcue);
        indicator(ui, size, &view.loop_beats, looping);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = PAD_GAP;
            let half = vec2((LOOP_WIDTH - PAD_GAP) / 2.0, BUTTON_HEIGHT);
            indicator(ui, half, "IN", looping);
            indicator(ui, half, "OUT", looping);
        });
    });
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

/// A box of `size` with `text`, filled with `lit`'s colour when lit.
fn indicator(ui: &mut Ui, size: Vec2, text: &str, lit: Option<Color32>) {
    let p = theme::palette(ui.ctx());
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let (fill, colour) = match lit {
        Some(colour) => (colour, p.on_accent),
        None => (p.lane, p.text_dim),
    };
    ui.painter().rect_filled(rect, CornerRadius::same(2), fill);
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
}
