//! The deck panels between the waveforms and the browser: the left side's active deck on
//! the left, the right side's on the right, each drawn from a [`DeckView`].

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use actions::{ControlState, HardwarePadMode, PadMode, Side, SideControl};
use analysis::{Key, Waveform};
use egui::{
    Align, Color32, CornerRadius, Label, Layout, Mesh, Rect, RichText, Sense, Shape, Stroke,
    TextureHandle, Ui, UiBuilder, Vec2, pos2, vec2,
};
use engine::{
    ChannelSnapshot, DeckId, DeckSnapshot, EffectKind, EngineMonitor, SAMPLER_SLOTS,
    SamplerSnapshot,
};

use crate::cover::{self, Covers};
use crate::model::ControlFrame;
use crate::theme;
use crate::waveform::{Playhead, colour};

/// A record's speed: the jog's marker turns at this at the original tempo.
const JOG_RPM: f64 = 100.0 / 3.0;
/// The share of the panels' width each deck panel takes; the mixer has the rest.
const PANEL_SHARE: f32 = 0.38;
const PANEL_MARGIN: i8 = 6;
const COVER_SIDE: f32 = 64.0;
/// The right of the header: BPM, key and times.
const TIMES_WIDTH: f32 = 130.0;
const OVERVIEW_HEIGHT: f32 = 32.0;
const MIDDLE_HEIGHT: f32 = 104.0;
const FX_WIDTH: f32 = 96.0;
const SLOT_HEIGHT: f32 = 22.0;
const FX_LEVEL_HEIGHT: f32 = 6.0;
const JOG_SIDE: f32 = 104.0;
const JOG_STROKE: f32 = 2.0;
const JOG_MARKER: f32 = 3.0;
const PITCH_WIDTH: f32 = 54.0;
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
/// PLAY while playing.
const PLAY_LIT: Color32 = Color32::from_rgb(0x4c, 0xc2, 0x6a);
/// An indicator that is off.
const OFF_TEXT: Color32 = theme::TEXT_DIM;

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
    /// In the library's notation.
    pub key: Option<String>,
    /// `M:SS.d`, cut (not rounded) to the tenth; empty without a track.
    pub elapsed: String,
    pub remaining: String,
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
    pub fx: [(String, bool); 3],
    pub fx_level: f32,
    /// What the pads do, as [`SideControl::pad_mode_label`] names it.
    pub pad_mode: &'static str,
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
            key: info
                .and_then(|i| i.row.key)
                .and_then(Key::new)
                .map(|key| key.to_notation(frame.key_notation)),
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
                let label = effect_label(frame.fx_slots[slot]).to_owned();
                (label, channel.fx_enabled[slot])
            }),
            fx_level: channel.fx_amount,
            pad_mode: side.pad_mode_label(),
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
fn split_remix(title: &str) -> (String, Option<String>) {
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

/// `frames` at `rate` as `M:SS.d`, cut to the tenth.
fn clock(frames: f64, rate: u32) -> String {
    // The tiny allowance keeps a time that is exactly on a tenth from reading one less
    // through rounding error.
    let tenths = (frames.max(0.0) * 10.0 / f64::from(rate) + 1e-6).floor() as u64;
    format!("{}:{:02}.{}", tenths / 600, tenths / 10 % 60, tenths % 10)
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
    ) -> Rect {
        let now = Instant::now();
        let (area, _) = ui.allocate_exact_size(ui.available_size(), Sense::hover());
        let width = area.width() * PANEL_SHARE;
        let left = Rect::from_min_size(area.min, vec2(width, area.height()));
        let right = Rect::from_min_size(
            pos2(area.right() - width, area.top()),
            vec2(width, area.height()),
        );
        let sampler = monitor.map(EngineMonitor::sampler).unwrap_or_default();
        for (side, rect) in Side::ALL.into_iter().zip([left, right]) {
            let deck = frame.control.side(side).active_deck;
            let mut snapshot = monitor.map(|m| m.deck(deck)).unwrap_or_default();
            let channel = monitor.map(|m| m.channel(deck)).unwrap_or_default();
            let head = &mut self.playheads[deck.index()];
            head.observe(&snapshot, now);
            snapshot.position_frames = head.position(now, frame.output_latency);
            let view = DeckView::new(deck, &snapshot, &channel, frame, &sampler);
            let cover = view
                .path
                .as_deref()
                .and_then(|path| covers.get(ui.ctx(), path));
            ui.scope_builder(UiBuilder::new().max_rect(rect), |ui| {
                panel(ui, &view, cover.as_ref());
            });
        }
        Rect::from_min_max(
            pos2(left.right(), area.top()),
            pos2(right.left(), area.bottom()),
        )
    }
}

/// Draws `view` filling `ui`, with `cover` (the plain tile without one).
pub fn panel(ui: &mut Ui, view: &DeckView, cover: Option<&TextureHandle>) {
    let accent = theme::deck_accent(view.deck);
    egui::Frame::new()
        .fill(theme::PANEL)
        .inner_margin(PANEL_MARGIN)
        .show(ui, |ui| {
            ui.set_min_size(ui.available_size());
            ui.spacing_mut().item_spacing = vec2(6.0, 4.0);
            header(ui, view, cover, accent);
            overview(ui, view);
            ui.horizontal(|ui| {
                ui.set_height(MIDDLE_HEIGHT);
                fx(ui, view, accent);
                jog(ui, view, accent);
                pitch(ui, view, accent);
                states(ui, view, accent);
            });
            ui.horizontal_top(|ui| {
                transport(ui, view);
                pads_grid(ui, view, accent);
            });
        });
}

/// The cover, title, artist, remix and stems, then BPM, key and times on the right.
fn header(ui: &mut Ui, view: &DeckView, cover: Option<&TextureHandle>, accent: Color32) {
    ui.horizontal_top(|ui| {
        cover::tile(ui, cover, COVER_SIDE);
        let text_width = (ui.available_width() - TIMES_WIDTH).max(0.0);
        let column = vec2(text_width, COVER_SIDE);
        ui.allocate_ui_with_layout(column, Layout::top_down(Align::Min), |ui| {
            ui.set_min_size(column);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(&view.tag)
                        .color(accent)
                        .strong()
                        .size(TITLE_FONT),
                );
                ui.add(Label::new(RichText::new(&view.title).strong().size(TITLE_FONT)).truncate());
            });
            ui.add(Label::new(RichText::new(&view.artist).color(theme::TEXT_DIM)).truncate());
            if let Some(remix) = &view.remix {
                ui.add(Label::new(RichText::new(remix).color(theme::TEXT_DIM)).truncate());
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
            if let Some(key) = &view.key {
                ui.label(RichText::new(key).color(accent));
            }
            if !view.elapsed.is_empty() {
                ui.label(&view.elapsed);
                ui.label(RichText::new(format!("−{}", view.remaining)).color(theme::TEXT_DIM));
            }
        });
    });
}

/// The whole track's waveform with the playhead.
fn overview(ui: &mut Ui, view: &DeckView) {
    let size = vec2(ui.available_width(), OVERVIEW_HEIGHT);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, CornerRadius::ZERO, theme::LANE);
    let mut mesh = Mesh::default();
    if let Some(waveform) = view.overview.as_deref() {
        overview_columns(&mut mesh, rect, waveform);
    }
    if let Some(progress) = view.progress {
        let x = rect.left() + rect.width() * progress.clamp(0.0, 1.0);
        let head = Rect::from_x_y_ranges(x - 1.0..=x + 1.0, rect.y_range());
        mesh.add_colored_rect(head, theme::PLAYHEAD);
    }
    painter.add(Shape::mesh(mesh));
}

/// One column per point, mirrored around the middle, as tall as the loudest bucket under
/// it and in its colour.
fn overview_columns(mesh: &mut Mesh, rect: Rect, waveform: &Waveform) {
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
        mesh.add_colored_rect(bar, colour(*loudest));
    }
}

/// The three FX slots, each lit while on, over the effect level.
fn fx(ui: &mut Ui, view: &DeckView, accent: Color32) {
    ui.vertical(|ui| {
        ui.set_width(FX_WIDTH);
        for (label, on) in &view.fx {
            indicator(ui, vec2(FX_WIDTH, SLOT_HEIGHT), label, on.then_some(accent));
        }
        let (rect, _) = ui.allocate_exact_size(vec2(FX_WIDTH, FX_LEVEL_HEIGHT), Sense::hover());
        let painter = ui.painter();
        painter.rect_filled(rect, CornerRadius::ZERO, theme::LANE);
        let level = rect.width() * view.fx_level.clamp(0.0, 1.0);
        let filled = Rect::from_min_size(rect.min, vec2(level, rect.height()));
        painter.rect_filled(filled, CornerRadius::ZERO, accent);
        ui.label(
            RichText::new(format!("FX {:.0}%", view.fx_level * 100.0))
                .color(theme::TEXT_DIM)
                .size(SMALL_FONT),
        );
    });
}

/// The platter: a circle with a marker that turns with the playhead.
fn jog(ui: &mut Ui, view: &DeckView, accent: Color32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(JOG_SIDE), Sense::hover());
    let painter = ui.painter();
    let centre = rect.center();
    let radius = JOG_SIDE / 2.0 - JOG_STROKE;
    painter.circle(
        centre,
        radius,
        theme::LANE,
        Stroke::new(JOG_STROKE, theme::TEXT_DIM),
    );
    let angle = view.jog_turn * std::f32::consts::TAU;
    let direction = vec2(angle.sin(), -angle.cos());
    let colour = if view.playing { accent } else { theme::TEXT };
    painter.line_segment(
        [
            centre + direction * radius * 0.55,
            centre + direction * radius,
        ],
        Stroke::new(JOG_MARKER, colour),
    );
}

/// The tempo slider (faster toward the bottom, as on the SB3) with the pitch in percent.
fn pitch(ui: &mut Ui, view: &DeckView, accent: Color32) {
    ui.vertical_centered(|ui| {
        ui.set_width(PITCH_WIDTH);
        let track_height = MIDDLE_HEIGHT - PITCH_LABEL_HEIGHT;
        let (rect, _) = ui.allocate_exact_size(vec2(PITCH_WIDTH, track_height), Sense::hover());
        let painter = ui.painter();
        let x = rect.center().x;
        let track = Rect::from_x_y_ranges(
            x - PITCH_TRACK / 2.0..=x + PITCH_TRACK / 2.0,
            rect.top() + PITCH_HANDLE.y / 2.0..=rect.bottom() - PITCH_HANDLE.y / 2.0,
        );
        painter.rect_filled(track, CornerRadius::ZERO, theme::LANE);
        let centre_mark =
            Rect::from_center_size(pos2(x, track.center().y), vec2(PITCH_HANDLE.x * 0.6, 1.0));
        painter.rect_filled(centre_mark, CornerRadius::ZERO, theme::TEXT_DIM);
        let y = track.top() + track.height() * view.pitch_slider.clamp(0.0, 1.0);
        let handle = Rect::from_center_size(pos2(x, y), PITCH_HANDLE);
        painter.rect_filled(handle, CornerRadius::same(1), accent);
        ui.label(RichText::new(format!("{:+.1}%", view.pitch_percent)).size(SMALL_FONT));
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
    ui.vertical(|ui| {
        let size = vec2(TRANSPORT_WIDTH, BUTTON_HEIGHT);
        // Line up with the pads, below the mode's name.
        ui.add_space(SMALL_FONT + 4.0);
        indicator(ui, size, "CUE", view.cueing.then_some(theme::CUE));
        indicator(ui, size, "PLAY", view.playing.then_some(PLAY_LIT));
        indicator(ui, size, "SYNC", None);
    });
}

/// The pad mode's name over the 2×4 pads, then the loop: its size, IN and OUT.
fn pads_grid(ui: &mut Ui, view: &DeckView, accent: Color32) {
    let pads_width = (ui.available_width() - LOOP_WIDTH - PAD_GAP * 2.0).max(0.0);
    ui.vertical(|ui| {
        ui.set_width(pads_width);
        ui.label(RichText::new(view.pad_mode).color(accent).size(SMALL_FONT));
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
    ui.vertical(|ui| {
        let size = vec2(LOOP_WIDTH, BUTTON_HEIGHT);
        ui.label(
            RichText::new("LOOP")
                .color(theme::TEXT_DIM)
                .size(SMALL_FONT),
        );
        let looping = view.loop_active.then_some(theme::HOTCUE);
        indicator(ui, size, &view.loop_beats, looping);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = PAD_GAP;
            let half = vec2((LOOP_WIDTH - PAD_GAP) / 2.0, BUTTON_HEIGHT);
            indicator(ui, half, "IN", looping);
            indicator(ui, half, "OUT", looping);
        });
    });
}

/// A box of `size` with `text`, filled with `lit`'s colour when lit.
fn indicator(ui: &mut Ui, size: Vec2, text: &str, lit: Option<Color32>) {
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let (fill, colour) = match lit {
        Some(colour) => (colour, theme::BACKGROUND),
        None => (theme::LANE, OFF_TEXT),
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

    use engine::Grid;
    use library::TrackRow;

    use super::*;
    use crate::model::DeckInfo;

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
        snapshot.hotcues[0] = Some(1000.0);
        snapshot.hotcues[2] = Some(5000.0);
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
