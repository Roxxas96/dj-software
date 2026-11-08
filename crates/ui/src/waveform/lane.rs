//! The scrolling waveform lanes across the top of the window: one per deck on a side, and
//! one more per other deck that plays, each centred on what is being heard.

use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::Instant;

use actions::Side;
use analysis::Waveform;
use egui::{Align2, Color32, FontId, Mesh, Painter, Pos2, Rect, Shape, Ui, pos2, vec2};
use engine::{DeckId, DeckSnapshot, EngineMonitor};

use super::{Playhead, ZoomGesture, colour, ticks};
use crate::model::ControlFrame;
use crate::theme;

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
    /// Draws the lanes in the space `ui` has left, sharing its height equally. Without
    /// `monitor` every deck is empty. A scroll or pinch over the lanes, or the zoom
    /// buttons, add zoom steps to `zoom` (positive zooms in) for the session to apply.
    pub fn show(
        &mut self,
        ui: &mut Ui,
        frame: &ControlFrame,
        monitor: Option<&EngineMonitor>,
        zoom: &AtomicI32,
    ) {
        let now = Instant::now();
        let read =
            DeckId::ALL.map(|id| monitor.map(|m| m.deck_rendered_at(id)).unwrap_or_default());
        for (head, (snapshot, rendered_at)) in self.playheads.iter_mut().zip(&read) {
            head.observe(snapshot, *rendered_at);
        }
        let snapshots = read.map(|(snapshot, _)| snapshot);
        let shown = lanes(
            Side::ALL.map(|side| frame.control.sides[side.index()].active_deck),
            snapshots.each_ref().map(|s| s.playing),
        );
        let (area, _) = ui.allocate_exact_size(ui.available_size(), egui::Sense::hover());
        let height = area.height() / shown.len() as f32;
        for (row, deck) in shown.into_iter().enumerate() {
            let top = area.top() + row as f32 * height;
            let rect = Rect::from_min_max(
                pos2(area.left(), top),
                pos2(area.right(), top + height - LANE_GAP),
            );
            let snapshot = &snapshots[deck.index()];
            // Until the engine plays the track the frame names, its waveform is not heard.
            let waveform = frame.decks[deck.index()]
                .as_ref()
                .filter(|info| snapshot.loaded && info.load_seq == snapshot.load_seq)
                .and_then(|info| info.detail.as_ref().or(info.overview.as_ref()));
            let lane = Lane {
                deck,
                snapshot,
                waveform: waveform.map(Arc::as_ref),
                position_frames: self.playheads[deck.index()].position(now, frame.output_latency),
                beats: frame.waveform_beats,
            };
            lane.draw(&ui.painter_at(rect), rect, &mut self.meshes[deck.index()]);
        }
        let mut steps = 0;
        if ui.rect_contains_pointer(area) {
            let (scroll, pinch) = ui.input(|i| (i.smooth_scroll_delta().y, i.zoom_delta()));
            steps += self.gesture.steps(scroll, pinch);
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
            zoom.fetch_add(steps, Ordering::Relaxed);
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
}

impl Lane<'_> {
    /// Draws the lane in `rect` with `painter`, filling `mesh` (its buffers are kept when
    /// nothing else holds it any more).
    pub fn draw(&self, painter: &Painter, rect: Rect, mesh: &mut Arc<Mesh>) {
        painter.rect_filled(rect, 0.0, theme::LANE);
        if let Some(scale) = self.scale(rect, painter.pixels_per_point()) {
            let m = Arc::make_mut(mesh);
            // `Mesh::clear` frees the vertices; clear by hand to keep them.
            m.indices.clear();
            m.vertices.clear();
            self.fill(m, rect, &scale);
            painter.add(Shape::mesh(Arc::clone(mesh)));
            self.hotcue_numbers(painter, rect, &scale);
        }
        painter.text(
            rect.left_top() + vec2(LABEL_OFFSET, LABEL_OFFSET),
            Align2::LEFT_TOP,
            self.deck.index() + 1,
            FontId::proportional(LABEL_FONT),
            theme::deck_accent(self.deck),
        );
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
    fn fill(&self, mesh: &mut Mesh, rect: Rect, scale: &Scale) {
        let snapshot = self.snapshot;
        if let Some(waveform) = self.waveform {
            columns(mesh, rect, scale, waveform);
        }
        if let (true, Some(start), Some(end)) =
            (snapshot.loop_active, snapshot.loop_in, snapshot.loop_out)
        {
            let band = Rect::from_x_y_ranges(scale.x(start)..=scale.x(end), rect.y_range());
            mesh.add_colored_rect(band, theme::LOOP_BAND);
        }
        let from = scale.centre_frame - scale.half_span;
        let to = scale.centre_frame + scale.half_span;
        for tick in ticks(snapshot.grid, from, to) {
            let (height, colour) = if tick.bar {
                (BAR_TICK_HEIGHT, theme::BAR_TICK)
            } else {
                (BEAT_TICK_HEIGHT, theme::BEAT_TICK)
            };
            let x = scale.x(tick.frame);
            let xs = x - TICK_WIDTH / 2.0..=x + TICK_WIDTH / 2.0;
            let top = rect.top()..=rect.top() + height;
            let bottom = rect.bottom() - height..=rect.bottom();
            mesh.add_colored_rect(Rect::from_x_y_ranges(xs.clone(), top), colour);
            mesh.add_colored_rect(Rect::from_x_y_ranges(xs, bottom), colour);
        }
        for frame in snapshot.hotcues.iter().flatten() {
            let x = scale.x(*frame);
            let pole = x..=x + FLAG_POLE;
            mesh.add_colored_rect(Rect::from_x_y_ranges(pole, rect.y_range()), theme::HOTCUE);
            let flag = Rect::from_min_size(pos2(x, rect.top()), vec2(FLAG_SIZE, FLAG_SIZE));
            mesh.add_colored_rect(flag, theme::HOTCUE);
        }
        let cue = scale.x(snapshot.cue_frames);
        triangle(
            mesh,
            [
                pos2(cue, rect.bottom() - CUE_SIZE),
                pos2(cue - CUE_SIZE / 2.0, rect.bottom()),
                pos2(cue + CUE_SIZE / 2.0, rect.bottom()),
            ],
            theme::CUE,
        );
        let centre = rect.center().x;
        let head = centre - PLAYHEAD_WIDTH / 2.0..=centre + PLAYHEAD_WIDTH / 2.0;
        mesh.add_colored_rect(Rect::from_x_y_ranges(head, rect.y_range()), theme::PLAYHEAD);
    }

    /// The numbers on the hotcue flags in sight.
    fn hotcue_numbers(&self, painter: &Painter, rect: Rect, scale: &Scale) {
        for (slot, frame) in self.snapshot.hotcues.iter().enumerate() {
            let Some(frame) = frame else { continue };
            let x = scale.x(*frame);
            if x + FLAG_SIZE < rect.left() || x > rect.right() {
                continue;
            }
            painter.text(
                pos2(x + FLAG_SIZE / 2.0, rect.top() + FLAG_SIZE / 2.0),
                Align2::CENTER_CENTER,
                slot + 1,
                FontId::proportional(FLAG_FONT),
                theme::BACKGROUND,
            );
        }
    }
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

/// One column per physical pixel, mirrored around the lane's middle, as tall as the
/// loudest bucket under it and in its colour. Columns are cut at fixed track frames and
/// only their place moves, so a column keeps its shape as the track scrolls.
fn columns(mesh: &mut Mesh, rect: Rect, scale: &Scale, waveform: &Waveform) {
    let per_bucket = f64::from(waveform.frames_per_bucket.max(1));
    let buckets = &waveform.buckets;
    let fpc = scale.frames_per_column;
    let first = ((scale.centre_frame - scale.half_span) / fpc).floor() as i64;
    let last = ((scale.centre_frame + scale.half_span) / fpc).ceil() as i64;
    let middle = rect.center().y;
    let half_height = rect.height() / 2.0;
    mesh.reserve_triangles(2 * (last - first + 1).max(0) as usize);
    mesh.reserve_vertices(4 * (last - first + 1).max(0) as usize);
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
        let Some(loudest) = buckets[from..to].iter().max_by_key(|b| b[0]) else {
            continue;
        };
        let reach = half_height * f32::from(loudest[0]) / 255.0;
        let bar = Rect::from_min_max(
            pos2(scale.x(start), middle - reach),
            pos2(scale.x(end), middle + reach),
        );
        mesh.add_colored_rect(bar, colour(*loudest));
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
        columns(&mut a, rect, &scale(5000.0), &waveform);
        let mut b = Mesh::default();
        columns(&mut b, rect, &scale(5025.0), &waveform);
        // Moving a quarter column shifts every column a quarter point, unchanged.
        let shifted: Vec<f32> = b.vertices.iter().map(|v| v.pos.x + 0.25).collect();
        let (first_a, first_b) = (a.vertices[0].pos, b.vertices[0].pos);
        assert_eq!(first_a.y, first_b.y);
        assert!((shifted[0] - first_a.x).abs() < 1e-3, "{shifted:?}");
    }
}
