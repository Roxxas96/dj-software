//! The scrolling waveform lanes across the top of the window: one per deck on a side, and
//! one more per other deck that plays, each centred on what is being heard.

use std::sync::Arc;
use std::time::Instant;

use actions::{Action, Input, Side};
use analysis::Waveform;
use egui::{
    Align2, Color32, FontId, Mesh, Painter, PointerButton, Pos2, Rect, Shape, Ui, pos2, vec2,
};
use engine::{DeckId, DeckSnapshot, Hotcue, HotcueSlot, PLATTER_SECONDS_PER_TURN};

use super::colour::band_colours;
use super::styles::{self, MAX_BARS};
use super::{Playhead, ZoomGesture, ticks};
use crate::command::UiCommand;
use crate::controls::{Sink, carry_steps, drag_path};
use crate::deck::DeckReads;
use crate::hotcue_menu::{self, HotcueTarget};
use crate::model::{ControlFrame, HotcueMark};
use crate::theme::{self, Palette};
use crate::view::{WaveformPalette, WaveformStyle};

/// The beat period, in seconds, of a lane whose deck has no grid: 120 BPM.
const DEFAULT_BEAT_SECONDS: f64 = 0.5;
/// How tall a beat tick is, and a bar's.
const BEAT_TICK_HEIGHT: f32 = 4.0;
const BAR_TICK_HEIGHT: f32 = 8.0;
const TICK_WIDTH: f32 = 1.0;
const PLAYHEAD_WIDTH: f32 = 2.0;
/// The cue triangle's width and height.
const CUE_SIZE: f32 = 8.0;
/// A hotcue flag's side, and its pole's width.
const FLAG_SIZE: f32 = 12.0;
const FLAG_POLE: f32 = 1.0;
/// The space between two lanes.
const LANE_GAP: f32 = 1.0;
/// Where the deck number sits from the lane's top left.
const LABEL_OFFSET: f32 = 4.0;
const LABEL_FONT: f32 = 14.0;
const FLAG_FONT: f32 = 10.0;
/// The space between a flag and its name, and how many characters of a name show.
const NAME_GAP: f32 = 3.0;
const NAME_CHARS: usize = 14;
/// How opaque a saved loop's band is.
const SAVED_LOOP_ALPHA: u8 = 0x50;
/// The zoom buttons' side, and their gap from the area's top right corner.
const ZOOM_BUTTON: f32 = 18.0;
const ZOOM_MARGIN: f32 = 4.0;

/// The decks that get a lane, top to bottom: the left side's active deck, the right's,
/// then every other deck that plays, in deck order. No deck twice.
pub fn lanes(active: [DeckId; 2], playing: [bool; 4]) -> Vec<DeckId> {
    let mut shown: Vec<DeckId> = Vec::with_capacity(4);
    for deck in active {
        if !shown.contains(&deck) {
            shown.push(deck);
        }
    }
    for deck in DeckId::ALL {
        if playing[deck.index()] && !shown.contains(&deck) {
            shown.push(deck);
        }
    }
    shown
}

/// What the lanes keep from frame to frame: each deck's playhead, and the mesh its lane
/// is drawn with, whose buffers are reused once egui has painted it.
#[derive(Debug, Default)]
pub struct Lanes {
    playheads: [Playhead; 4],
    meshes: [Arc<Mesh>; 4],
    gesture: ZoomGesture,
}

impl Lanes {
    /// Draws the lanes in the space `ui` has left, sharing its height equally, the decks
    /// as `reads` has them. A scroll or pinch over the lanes, or the zoom
    /// buttons, send zoom steps through `sink` (positive zooms in) for the session to apply.
    /// Dragging a lane scrubs its deck like the jog: the waveform follows the pointer.
    pub fn show(
        &mut self,
        ui: &mut Ui,
        frame: &ControlFrame,
        reads: &DeckReads,
        sink: &mut Sink<'_>,
    ) {
        let now = Instant::now();
        for (head, (snapshot, rendered_at)) in self.playheads.iter_mut().zip(&reads.decks) {
            head.observe(snapshot, *rendered_at);
        }
        let snapshots = reads.decks.each_ref().map(|(snapshot, _)| snapshot);
        // The 4-Decks view shows every deck, playing or not.
        let shown = if frame.view.four_decks {
            DeckId::ALL.to_vec()
        } else {
            lanes(
                Side::ALL.map(|side| frame.control.sides[side.index()].active_deck),
                snapshots.each_ref().map(|s| s.playing),
            )
        };
        let (area, response) =
            ui.allocate_exact_size(ui.available_size(), egui::Sense::click_and_drag());
        // A right-click on a hotcue flag opens its editor.
        let right_click = response
            .secondary_clicked()
            .then(|| response.interact_pointer_pos())
            .flatten();
        let mut flag = None;
        // Where each lane with a track is, and how many jog steps a point dragged across it
        // is worth.
        let mut scrubbable: [Option<(DeckId, Rect, f64)>; 4] = [None; 4];
        // Overlaid, the first two decks share the first lane.
        let overlaid = frame.view.waveform_style == WaveformStyle::Overlaid && shown.len() >= 2;
        let rows = shown.len() - usize::from(overlaid);
        let height = area.height() / rows as f32;
        for (index, deck) in shown.into_iter().enumerate() {
            let row = index.saturating_sub(usize::from(overlaid));
            let top = area.top() + row as f32 * height;
            let rect = Rect::from_min_max(
                pos2(area.left(), top),
                pos2(area.right(), top + height - LANE_GAP),
            );
            let snapshot = &snapshots[deck.index()];
            // Until the engine plays the track the frame names, its waveform is not heard.
            let info = frame.decks[deck.index()]
                .as_ref()
                .filter(|info| snapshot.loaded && info.load_seq == snapshot.load_seq);
            let waveform = info.and_then(|info| info.detail.as_ref().or(info.overview.as_ref()));
            let lane = Lane {
                deck,
                snapshot,
                waveform: waveform.map(Arc::as_ref),
                position_frames: self.playheads[deck.index()].position(now, frame.output_latency),
                beats: frame.view.waveform_beats,
                style: frame.view.waveform_style,
                palette: frame.view.waveform_palette,
                overlay: overlaid && index == 1,
                marks: info.map(|info| &*info.hotcue_marks),
            };
            lane.draw(&ui.painter_at(rect), rect, &mut self.meshes[deck.index()]);
            // The frames a point spans are the lane's zoom; a step is a jog step's worth of
            // the platter's turn.
            scrubbable[index] = lane.scale(rect, 1.0).map(|scale| {
                let per_step = PLATTER_SECONDS_PER_TURN * f64::from(snapshot.track_sample_rate)
                    / f64::from(frame.control.jog_steps_per_turn.max(1));
                (deck, rect, scale.frames_per_point / per_step)
            });
            // An overlay is drawn over the lane below: its flags are on top.
            if let Some(pos) = right_click.filter(|&pos| rect.contains(pos))
                && let Some(slot) = lane.flag_at(rect, pos)
            {
                let mark = lane.marks.and_then(|marks| marks[slot.index()].as_ref());
                let track = info.map(|info| info.row.id);
                flag = Some((HotcueTarget::new(deck, slot, track, true, mark), pos));
            }
        }
        if let Some((target, pos)) = flag {
            hotcue_menu::open(ui.ctx(), target, pos);
        }
        Self::scrub(ui, &response, &scrubbable, sink);
        let mut steps = 0;
        if ui.rect_contains_pointer(area) {
            let (scroll, pinch) = ui.input(|i| (i.smooth_scroll_delta().y, i.zoom_delta()));
            steps += self.gesture.steps(scroll, pinch);
        } else {
            // A scroll elsewhere never finishes a zoom step begun over the lanes.
            self.gesture = ZoomGesture::default();
        }
        for (label, step, offset) in [("+", 1, 1.0), ("-", -1, 2.0)] {
            let at = pos2(
                area.right() - offset * (ZOOM_BUTTON + ZOOM_MARGIN),
                area.top() + ZOOM_MARGIN,
            );
            let button = egui::Button::new(label).min_size(vec2(ZOOM_BUTTON, ZOOM_BUTTON));
            if ui
                .put(
                    Rect::from_min_size(at, vec2(ZOOM_BUTTON, ZOOM_BUTTON)),
                    button,
                )
                .clicked()
            {
                steps += step;
            }
        }
        if steps != 0 {
            sink.send(UiCommand::Zoom(steps));
        }
    }

    /// Dragging a lane touches the jog of the deck under the pointer when the press began
    /// (the first of the decks sharing an overlaid lane), scratches it by the distance
    /// dragged and lets go. The waveform follows the pointer, so a drag to the left goes
    /// forward. A lane with no track sends nothing.
    fn scrub(
        ui: &Ui,
        response: &egui::Response,
        lanes: &[Option<(DeckId, Rect, f64)>; 4],
        sink: &mut Sink<'_>,
    ) {
        // Called every frame, so it forgets the drag when the pointer lets go.
        let path = drag_path(response);
        let remembered = response.id.with("scrub_deck");
        let down = response.is_pointer_button_down_on() && ui.input(|i| i.pointer.primary_down());
        if !down {
            ui.data_mut(|d| d.remove::<DeckId>(remembered));
            carry_steps(ui.ctx(), response.id, 0.0, false);
            // A late release still goes to the deck touched.
            if let Some(deck) = lanes.iter().flatten().next().map(|l| l.0) {
                sink.button(response, Action::JogTouch(deck));
            }
            return;
        }
        // Only a drag touches the platter: a click would hold a playing deck. Its deck is
        // the one under the press, and stays so for the whole drag.
        let dragging = response.dragged_by(PointerButton::Primary);
        if !dragging {
            return;
        }
        let deck = ui.data(|d| d.get_temp::<DeckId>(remembered)).or_else(|| {
            let origin = ui.input(|i| i.pointer.press_origin())?;
            lanes
                .iter()
                .flatten()
                .find(|(_, rect, _)| rect.contains(origin))
                .map(|&(deck, ..)| deck)
        });
        let Some(deck) = deck else { return };
        ui.data_mut(|d| d.insert_temp(remembered, deck));
        let mut amount = 0.0;
        let lane = lanes.iter().flatten().find(|l| l.0 == deck);
        if let (Some(&(_, _, steps_per_point)), Some((from, to))) = (lane, path) {
            amount = (f64::from(from.x - to.x) * steps_per_point) as f32;
        }
        // The touch comes first on the frame the drag starts.
        sink.button(response, Action::JogTouch(deck));
        let steps = carry_steps(ui.ctx(), response.id, amount, dragging);
        if steps != 0 {
            sink.send(UiCommand::Action(
                Action::JogScratch(deck),
                Input::Relative(steps),
            ));
        }
    }
}

/// One deck's lane, as it is at the moment drawn.
#[derive(Debug, Clone, Copy)]
pub struct Lane<'a> {
    pub deck: DeckId,
    pub snapshot: &'a DeckSnapshot,
    /// The waveform of the track the engine plays, if there is one.
    pub waveform: Option<&'a Waveform>,
    /// Where the playhead is, in track frames: the lane's centre.
    pub position_frames: f64,
    /// How many beats the lane spans.
    pub beats: f64,
    pub style: WaveformStyle,
    /// The colours of the low, mid and high bands.
    pub palette: WaveformPalette,
    /// Drawn over another deck's lane in the same rect: no background of its own.
    pub overlay: bool,
    /// The hotcues' colours and names, when the library has them for the track.
    pub marks: Option<&'a [Option<HotcueMark>; 8]>,
}

impl Lane<'_> {
    /// Draws the lane in `rect` with `painter`, filling `mesh` (its buffers are kept when
    /// nothing else holds it any more).
    pub fn draw(&self, painter: &Painter, rect: Rect, mesh: &mut Arc<Mesh>) {
        let p = theme::palette(painter.ctx());
        // An overlay's number sits beside the number below it.
        let label_shift = if self.overlay { LABEL_FONT * 1.5 } else { 0.0 };
        if !self.overlay {
            painter.rect_filled(rect, 0.0, p.wave_background);
        }
        if let Some(scale) = self.scale(rect, painter.pixels_per_point()) {
            let m = Arc::make_mut(mesh);
            // `Mesh::clear` frees the vertices; clear by hand to keep them.
            m.indices.clear();
            m.vertices.clear();
            self.fill(m, rect, &scale, &p);
            painter.add(Shape::mesh(Arc::clone(mesh)));
            self.hotcue_numbers(painter, rect, &scale, &p);
        }
        painter.text(
            rect.left_top() + vec2(LABEL_OFFSET + label_shift, LABEL_OFFSET),
            Align2::LEFT_TOP,
            self.deck.index() + 1,
            FontId::proportional(LABEL_FONT),
            p.deck_accent(self.deck),
        );
    }

    /// The hotcue whose flag is at `pos` in the lane drawn in `rect`; of flags drawn over
    /// each other, the one on top.
    pub fn flag_at(&self, rect: Rect, pos: Pos2) -> Option<HotcueSlot> {
        let scale = self.scale(rect, 1.0)?;
        HotcueSlot::ALL.into_iter().rev().find(|slot| {
            self.snapshot.hotcues[slot.index()].is_some_and(|hotcue| {
                let x = scale.x(hotcue.frames);
                Rect::from_min_size(pos2(x, rect.top()), vec2(FLAG_SIZE, FLAG_SIZE)).contains(pos)
            })
        })
    }

    /// How track frames map onto the lane, or `None` for an empty deck.
    fn scale(&self, rect: Rect, pixels_per_point: f32) -> Option<Scale> {
        let snapshot = self.snapshot;
        if !snapshot.loaded || snapshot.track_sample_rate == 0 || rect.width() <= 0.0 {
            return None;
        }
        let beat_frames = snapshot
            .grid
            .map(|g| g.beat_period_frames)
            .filter(|p| p.is_finite() && *p > 0.0)
            .unwrap_or(DEFAULT_BEAT_SECONDS * f64::from(snapshot.track_sample_rate));
        let span = self.beats * beat_frames;
        if !(span.is_finite() && span > 0.0 && self.position_frames.is_finite()) {
            return None;
        }
        let columns = f64::from(rect.width() * pixels_per_point);
        Some(Scale {
            centre_x: rect.center().x,
            centre_frame: self.position_frames,
            frames_per_column: span / columns,
            frames_per_point: span / f64::from(rect.width()),
            half_span: span / 2.0,
        })
    }

    /// Everything but text: the columns, the loop, the ticks, the markers and the
    /// playhead, in that order from the back.
    fn fill(&self, mesh: &mut Mesh, rect: Rect, scale: &Scale, p: &Palette) {
        let snapshot = self.snapshot;
        if let Some(waveform) = self.waveform {
            let bands = band_colours(self.palette, p);
            columns(
                mesh,
                rect,
                scale,
                waveform,
                (self.style, self.deck),
                (p, bands),
            );
        }
        if let (true, Some(start), Some(end)) =
            (snapshot.loop_active, snapshot.loop_in, snapshot.loop_out)
        {
            let band = Rect::from_x_y_ranges(scale.x(start)..=scale.x(end), rect.y_range());
            mesh.add_colored_rect(band, p.loop_band);
        }
        for (slot, hotcue) in snapshot.hotcues.iter().enumerate() {
            let Some(Hotcue {
                frames,
                loop_frames: Some(length),
            }) = *hotcue
            else {
                continue;
            };
            let band =
                Rect::from_x_y_ranges(scale.x(frames)..=scale.x(frames + length), rect.y_range());
            mesh.add_colored_rect(
                band,
                with_alpha(self.hotcue_colour(slot, p), SAVED_LOOP_ALPHA),
            );
        }
        let from = scale.centre_frame - scale.half_span;
        let to = scale.centre_frame + scale.half_span;
        for tick in ticks(snapshot.grid, from, to) {
            let (height, colour) = if tick.bar {
                (BAR_TICK_HEIGHT, p.bar_tick)
            } else {
                (BEAT_TICK_HEIGHT, p.beat_tick)
            };
            let x = scale.x(tick.frame);
            let xs = x - TICK_WIDTH / 2.0..=x + TICK_WIDTH / 2.0;
            let top = rect.top()..=rect.top() + height;
            let bottom = rect.bottom() - height..=rect.bottom();
            mesh.add_colored_rect(Rect::from_x_y_ranges(xs.clone(), top), colour);
            mesh.add_colored_rect(Rect::from_x_y_ranges(xs, bottom), colour);
        }
        for (slot, hotcue) in snapshot.hotcues.iter().enumerate() {
            let Some(hotcue) = hotcue else { continue };
            let colour = self.hotcue_colour(slot, p);
            let x = scale.x(hotcue.frames);
            let pole = x..=x + FLAG_POLE;
            mesh.add_colored_rect(Rect::from_x_y_ranges(pole, rect.y_range()), colour);
            let flag = Rect::from_min_size(pos2(x, rect.top()), vec2(FLAG_SIZE, FLAG_SIZE));
            mesh.add_colored_rect(flag, colour);
        }
        let cue = scale.x(snapshot.cue_frames);
        triangle(
            mesh,
            [
                pos2(cue, rect.bottom() - CUE_SIZE),
                pos2(cue - CUE_SIZE / 2.0, rect.bottom()),
                pos2(cue + CUE_SIZE / 2.0, rect.bottom()),
            ],
            p.cue,
        );
        let centre = rect.center().x;
        let head = centre - PLAYHEAD_WIDTH / 2.0..=centre + PLAYHEAD_WIDTH / 2.0;
        mesh.add_colored_rect(Rect::from_x_y_ranges(head, rect.y_range()), p.playhead);
    }

    /// Hotcue `slot`'s colour: its saved one, or the palette's.
    fn hotcue_colour(&self, slot: usize, p: &Palette) -> Color32 {
        let saved = self
            .marks
            .and_then(|m| m[slot].as_ref())
            .and_then(|m| m.color);
        p.wave_mark(saved)
    }

    /// The numbers on the hotcue flags in sight, and the names beside them.
    fn hotcue_numbers(&self, painter: &Painter, rect: Rect, scale: &Scale, p: &Palette) {
        for (slot, hotcue) in self.snapshot.hotcues.iter().enumerate() {
            let Some(hotcue) = hotcue else { continue };
            let x = scale.x(hotcue.frames);
            if x + FLAG_SIZE < rect.left() || x > rect.right() {
                continue;
            }
            let number = flag_number(self.hotcue_colour(slot, p));
            painter.text(
                pos2(x + FLAG_SIZE / 2.0, rect.top() + FLAG_SIZE / 2.0),
                Align2::CENTER_CENTER,
                slot + 1,
                FontId::proportional(FLAG_FONT),
                number,
            );
            let name = self
                .marks
                .and_then(|m| m[slot].as_ref())
                .and_then(|m| m.name.as_deref());
            if let Some(name) = name.map(flag_name).filter(|name| !name.is_empty()) {
                painter.text(
                    pos2(
                        x + FLAG_SIZE + FLAG_POLE + NAME_GAP,
                        rect.top() + FLAG_SIZE / 2.0,
                    ),
                    Align2::LEFT_CENTER,
                    name,
                    FontId::proportional(FLAG_FONT),
                    p.wave_text,
                );
            }
        }
    }
}

/// The colour of the number on a hotcue flag of colour `flag`, from the flag alone: the
/// scheme's background could be as bright as the flag.
fn flag_number(flag: Color32) -> Color32 {
    theme::text_on(flag)
}

/// `name` cut to the length a flag has room for (a slice: nothing is allocated).
fn flag_name(name: &str) -> &str {
    let name = name.trim();
    match name.char_indices().nth(NAME_CHARS) {
        Some((end, _)) => name[..end].trim_end(),
        None => name,
    }
}

/// `colour` with `alpha` (0 to 255) as its opacity.
fn with_alpha(colour: Color32, alpha: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(colour.r(), colour.g(), colour.b(), alpha)
}

/// How a lane maps track frames onto its width.
#[derive(Debug, Clone, Copy)]
struct Scale {
    centre_x: f32,
    centre_frame: f64,
    /// Track frames under one physical pixel column.
    frames_per_column: f64,
    frames_per_point: f64,
    /// Frames from the centre to either edge.
    half_span: f64,
}

impl Scale {
    /// Where `frame` falls across the lane, in points.
    fn x(&self, frame: f64) -> f32 {
        self.centre_x + ((frame - self.centre_frame) / self.frames_per_point) as f32
    }
}

/// One column per physical pixel, mirrored around the lane's middle, each band as tall as
/// its loudest under it ([`styles::loudest`]) and in its colour. Columns are cut at fixed
/// track frames and only their place moves, so a column keeps its shape as the track
/// scrolls.
fn columns(
    mesh: &mut Mesh,
    rect: Rect,
    scale: &Scale,
    waveform: &Waveform,
    (style, deck): (WaveformStyle, DeckId),
    (p, bands): (&Palette, [Color32; 3]),
) {
    let per_bucket = f64::from(waveform.frames_per_bucket.max(1));
    let buckets = &waveform.buckets;
    let fpc = scale.frames_per_column;
    let first = ((scale.centre_frame - scale.half_span) / fpc).floor() as i64;
    let last = ((scale.centre_frame + scale.half_span) / fpc).ceil() as i64;
    let per_column = if matches!(style, WaveformStyle::Bands | WaveformStyle::Frequency) {
        MAX_BARS
    } else {
        1
    };
    let count = (last - first + 1).max(0) as usize * per_column;
    mesh.reserve_triangles(2 * count);
    mesh.reserve_vertices(4 * count);
    for column in first..=last {
        let start = column as f64 * fpc;
        let end = start + fpc;
        if end <= 0.0 {
            continue;
        }
        let from = (start / per_bucket).floor().max(0.0) as usize;
        if from >= buckets.len() {
            break;
        }
        let to = ((end / per_bucket).floor() as usize).clamp(from + 1, buckets.len());
        let Some(loudest) = styles::loudest(&buckets[from..to]) else {
            continue;
        };
        let (bars, used) = styles::bars(style, loudest, deck, p, bands);
        for bar in &bars[..used] {
            let rect = Rect::from_min_max(
                pos2(scale.x(start), rect.top() + rect.height() * bar.from),
                pos2(scale.x(end), rect.top() + rect.height() * bar.to),
            );
            mesh.add_colored_rect(rect, bar.colour);
        }
    }
}

fn triangle(mesh: &mut Mesh, corners: [Pos2; 3], colour: Color32) {
    let base = mesh.vertices.len() as u32;
    for corner in corners {
        mesh.colored_vertex(corner, colour);
    }
    mesh.add_triangle(base, base + 1, base + 2);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decks(numbers: &[usize]) -> Vec<DeckId> {
        numbers
            .iter()
            .map(|n| DeckId::new(n - 1).unwrap())
            .collect()
    }

    fn deck(number: usize) -> DeckId {
        DeckId::new(number - 1).unwrap()
    }

    #[test]
    fn flag_numbers_read_on_any_colour_in_both_schemes() {
        let saved = [
            Some(Color32::from_rgb(0x00, 0xff, 0x00)),
            Some(Color32::from_rgb(0xff, 0x00, 0x00)),
            Some(Color32::from_rgb(0x00, 0x00, 0xff)),
            None,
        ];
        for p in [Palette::DARK, Palette::LIGHT] {
            for colour in saved {
                for flag in [p.mark(colour), p.wave_mark(colour)] {
                    let ratio = theme::contrast(flag_number(flag), flag);
                    assert!(ratio >= 4.5, "{colour:?} on {:?}: {ratio:.2}", p.background);
                }
            }
        }
    }

    #[test]
    fn lanes_follow_the_sides() {
        assert_eq!(lanes([deck(1), deck(2)], [false; 4]), decks(&[1, 2]));
    }

    #[test]
    fn lanes_keep_playing_decks() {
        let playing = [true, false, false, false];
        assert_eq!(lanes([deck(3), deck(2)], playing), decks(&[3, 2, 1]));
    }

    #[test]
    fn lanes_never_repeat() {
        let playing = [true, true, false, false];
        assert_eq!(lanes([deck(1), deck(2)], playing), decks(&[1, 2]));
    }

    #[test]
    fn a_column_keeps_its_place_on_the_track() {
        let waveform = Waveform {
            frames_per_bucket: 100,
            buckets: (0..100).map(|i| [i as u8, 255, 0, 0]).collect(),
        };
        let rect = Rect::from_min_size(Pos2::ZERO, vec2(100.0, 50.0));
        let scale = |centre_frame| Scale {
            centre_x: 50.0,
            centre_frame,
            frames_per_column: 100.0,
            frames_per_point: 100.0,
            half_span: 5000.0,
        };
        let mut a = Mesh::default();
        let deck = DeckId::new(0).unwrap();
        columns(
            &mut a,
            rect,
            &scale(5000.0),
            &waveform,
            (WaveformStyle::Frequency, deck),
            (&Palette::DARK, [Color32::RED; 3]),
        );
        let mut b = Mesh::default();
        columns(
            &mut b,
            rect,
            &scale(5025.0),
            &waveform,
            (WaveformStyle::Frequency, deck),
            (&Palette::DARK, [Color32::RED; 3]),
        );
        // Moving a quarter column shifts every column a quarter point, unchanged.
        let shifted: Vec<f32> = b.vertices.iter().map(|v| v.pos.x + 0.25).collect();
        let (first_a, first_b) = (a.vertices[0].pos, b.vertices[0].pos);
        assert_eq!(first_a.y, first_b.y);
        assert!((shifted[0] - first_a.x).abs() < 1e-3, "{shifted:?}");
    }

    #[test]
    fn a_column_shows_every_band_peak() {
        // Zoomed out: two buckets under each column, the kick's and a bright one.
        let waveform = Waveform {
            frames_per_bucket: 50,
            buckets: vec![[255, 200, 10, 10], [100, 20, 150, 120]],
        };
        let rect = Rect::from_min_size(Pos2::ZERO, vec2(100.0, 40.0));
        let scale = Scale {
            centre_x: 0.0,
            centre_frame: 0.0,
            frames_per_column: 100.0,
            frames_per_point: 100.0,
            half_span: 50.0,
        };
        let mut mesh = Mesh::default();
        columns(
            &mut mesh,
            rect,
            &scale,
            &waveform,
            (WaveformStyle::Frequency, deck(1)),
            (&Palette::DARK, [Color32::RED; 3]),
        );
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

    fn marked_lane<'a>(
        snapshot: &'a DeckSnapshot,
        marks: Option<&'a [Option<HotcueMark>; 8]>,
    ) -> Lane<'a> {
        Lane {
            deck: deck(1),
            snapshot,
            waveform: None,
            position_frames: 0.0,
            beats: 16.0,
            style: WaveformStyle::Frequency,
            palette: WaveformPalette::default(),
            overlay: false,
            marks,
        }
    }

    /// The colours of the rectangles `lane` fills, once each.
    fn filled_colours(lane: &Lane, p: &Palette) -> Vec<Color32> {
        let rect = Rect::from_min_size(Pos2::ZERO, vec2(400.0, 50.0));
        let scale = Scale {
            centre_x: 200.0,
            centre_frame: 0.0,
            frames_per_column: 100.0,
            frames_per_point: 100.0,
            half_span: 20_000.0,
        };
        let mut mesh = Mesh::default();
        lane.fill(&mut mesh, rect, &scale, p);
        mesh.vertices.iter().map(|v| v.color).collect()
    }

    fn snapshot_with(slot: usize, hotcue: Hotcue) -> DeckSnapshot {
        let mut snapshot = DeckSnapshot {
            loaded: true,
            track_sample_rate: 44_100,
            ..DeckSnapshot::default()
        };
        snapshot.hotcues[slot] = Some(hotcue);
        snapshot
    }

    fn red_mark() -> [Option<HotcueMark>; 8] {
        let mut marks: [Option<HotcueMark>; 8] = Default::default();
        marks[3] = Some(HotcueMark::new(Some(0xff0000), Some("Drop".into())));
        marks
    }

    #[test]
    fn a_loop_slot_draws_a_band_from_start_to_end_in_its_colour() {
        let p = Palette::DARK;
        let snapshot = snapshot_with(
            3,
            Hotcue {
                frames: 1000.0,
                loop_frames: Some(4000.0),
            },
        );
        let marks = red_mark();
        let colours = filled_colours(&marked_lane(&snapshot, Some(&marks)), &p);
        let band = with_alpha(Color32::from_rgb(0xff, 0, 0), SAVED_LOOP_ALPHA);
        assert_eq!(colours.iter().filter(|c| **c == band).count(), 4);
        // Without a colour the band takes the hotcue colour.
        let colours = filled_colours(&marked_lane(&snapshot, None), &p);
        assert!(colours.contains(&with_alpha(p.hotcue, SAVED_LOOP_ALPHA)));
        // A plain hotcue has no band.
        let plain = snapshot_with(3, Hotcue::point(1000.0));
        let colours = filled_colours(&marked_lane(&plain, Some(&marks)), &p);
        assert!(!colours.contains(&band));
    }

    #[test]
    fn hotcue_colours_tint_flags() {
        let p = Palette::DARK;
        let snapshot = snapshot_with(3, Hotcue::point(1000.0));
        let marks = red_mark();
        let red = Color32::from_rgb(0xff, 0, 0);
        let colours = filled_colours(&marked_lane(&snapshot, Some(&marks)), &p);
        assert!(colours.contains(&red) && !colours.contains(&p.hotcue));
        let colours = filled_colours(&marked_lane(&snapshot, None), &p);
        assert!(colours.contains(&p.hotcue) && !colours.contains(&red));
    }

    #[test]
    fn pale_colours_are_darkened_on_the_light_palette() {
        let pale = Color32::from_rgb(0xff, 0xff, 0x80);
        assert_eq!(Palette::DARK.mark(Some(pale)), pale);
        assert_ne!(Palette::LIGHT.mark(Some(pale)), pale);
        let red = Color32::from_rgb(0xff, 0, 0);
        assert_eq!(Palette::LIGHT.mark(Some(red)), red);
        assert_eq!(Palette::LIGHT.mark(None), Palette::LIGHT.hotcue);
    }

    #[test]
    fn flags_keep_pale_colours_on_the_dark_waveform_in_both_schemes() {
        let pale = Color32::from_rgb(0xff, 0xff, 0x80);
        for p in [Palette::DARK, Palette::LIGHT] {
            assert_eq!(p.wave_mark(Some(pale)), pale);
            assert_eq!(p.wave_mark(None), p.hotcue);
        }
    }

    #[test]
    fn long_names_are_cut_for_the_flag() {
        assert_eq!(flag_name(" Drop "), "Drop");
        assert_eq!(flag_name("A very long hotcue name"), "A very long ho");
        assert_eq!(flag_name("ééééééééééééééééé"), "éééééééééééééé");
    }
}
