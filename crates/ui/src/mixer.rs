//! The mixer: what the controller set for each side's active deck (gain, EQ, filter,
//! fader, headphone button, level) and the crossfader and master level between them. It
//! shows `ControlState` and never changes it. A hollow marker on a knob or fader is where
//! the physical control waits to pick its value up after a deck switch (soft takeover).

use std::time::{Duration, Instant};

use actions::Side;
use egui::{
    Color32, CornerRadius, Label, Pos2, Rect, RichText, Stroke, StrokeKind, Ui, pos2, vec2,
};
use engine::{DeckId, EngineMonitor, EqMode, StemKnobs};

use crate::layout::{Sections, mixer_order};
use crate::model::{
    ControlFrame, GHOST_EQ_HIGH, GHOST_EQ_LOW, GHOST_EQ_MID, GHOST_FADER, GHOST_FILTER, GHOST_TRIM,
};
use crate::theme;

/// How long a channel meter's peak marker stays at the highest peak.
const PEAK_HOLD: Duration = Duration::from_secs(1);
/// The height of the crossfader and master row.
const BOTTOM_HEIGHT: f32 = 56.0;
const KNOB_RADIUS: f32 = 15.0;
const KNOB_STEP: f32 = 42.0;
const HEADER_HEIGHT: f32 = 24.0;
/// The headphone button's width when the strip has room for it.
const PFL_WIDTH: f32 = 40.0;
/// Between the deck's name and the headphone button.
const HEADER_GAP: f32 = 4.0;
const CAPTION_FONT: f32 = 10.0;
const CAPTION_HEIGHT: f32 = 10.0;
const FADER_WIDTH: f32 = 14.0;
const METER_WIDTH: f32 = 8.0;
/// A knob's travel, either side of straight up, in radians.
const KNOB_SWEEP: f32 = 0.75 * std::f32::consts::PI;
const METER_HOT: f32 = 0.95;

/// The labels of the three EQ knobs, top to bottom. The stems' names show only where the
/// knobs control the stems: in stems mode, on a deck whose stems are ready.
pub fn eq_labels(eq_mode: EqMode, stems_active: bool, knobs: StemKnobs) -> [&'static str; 3] {
    match (eq_mode, stems_active, knobs) {
        (EqMode::Stems, true, StemKnobs::Vdj) => ["HIHAT", "MEL/VOX", "KICK"],
        (EqMode::Stems, true, StemKnobs::Raw) => ["VOCALS", "INSTRU", "BASS"],
        _ => ["HIGH", "MID", "LOW"],
    }
}

/// Where a control's pickup marker goes: `at` maps a value (0 to 1) to a point on the
/// control. `None` when the control has nothing to pick up.
pub fn ghost_marker(ghost: Option<f32>, at: impl Fn(f32) -> Pos2) -> Option<Pos2> {
    ghost.map(|v| at(v.clamp(0.0, 1.0)))
}

/// A meter's highest recent peak.
#[derive(Debug, Default, Clone, Copy)]
struct Hold {
    peak: f32,
    since: Option<Instant>,
}

impl Hold {
    /// The peak to show for the level `now` measured at `at`.
    fn update(&mut self, level: f32, at: Instant) -> f32 {
        let expired = self
            .since
            .is_none_or(|s| at.saturating_duration_since(s) >= PEAK_HOLD);
        if level >= self.peak || expired {
            *self = Hold {
                peak: level,
                since: Some(at),
            };
        }
        self.peak
    }
}

/// What the mixer keeps from one frame to the next: the meters' held peaks.
#[derive(Debug, Default)]
pub struct Mixer {
    holds: [Hold; 4],
}

impl Mixer {
    /// Draws the mixer in `rect`. Without `monitor` the meters are empty and the EQ keeps
    /// its band names.
    pub fn show(
        &mut self,
        ui: &mut Ui,
        rect: Rect,
        frame: &ControlFrame,
        monitor: Option<&EngineMonitor>,
        sections: &Sections,
    ) {
        let now = Instant::now();
        let peaks = monitor.map(|m| m.meters().channel_peak).unwrap_or_default();
        let strips =
            Rect::from_min_max(rect.min, pos2(rect.right(), rect.bottom() - BOTTOM_HEIGHT));
        let bottom = Rect::from_min_max(pos2(rect.left(), strips.bottom()), rect.max);
        let active = Side::ALL.map(|side| frame.control.side(side).active_deck);
        // Four strips in VirtualDJ's order (3, 1, 2, 4) or one per side's active deck.
        let decks = if frame.view.four_decks {
            mixer_order().to_vec()
        } else {
            active.to_vec()
        };
        let width = strips.width() / decks.len() as f32;
        for (i, deck) in decks.into_iter().enumerate() {
            let strip_rect = Rect::from_min_size(
                pos2(strips.left() + i as f32 * width, strips.top()),
                vec2(width, strips.height()),
            );
            let stems_active = monitor.is_some_and(|m| m.deck(deck).stems_active);
            let level = peaks[deck.index()];
            let held = self.holds[deck.index()].update(level, now);
            ui.scope_builder(egui::UiBuilder::new().max_rect(strip_rect), |ui| {
                strip(
                    ui,
                    strip_rect,
                    deck,
                    frame,
                    stems_active,
                    (level, held),
                    sections,
                );
            });
        }
        centre(ui, bottom, frame, monitor.map(|m| m.meters().master_peak));
    }
}

fn split(rect: Rect, half: f32) -> [Rect; 2] {
    [
        Rect::from_min_size(rect.min, vec2(half, rect.height())),
        Rect::from_min_size(
            pos2(rect.left() + half, rect.top()),
            vec2(half, rect.height()),
        ),
    ]
}

/// A small caption centred in `rect`, cut short when it does not fit.
fn caption(ui: &mut Ui, rect: Rect, text: &str, colour: Color32) {
    ui.put(
        rect,
        Label::new(RichText::new(text).size(CAPTION_FONT).color(colour))
            .selectable(false)
            .truncate(),
    );
}

/// Where the parts of a channel strip go.
#[derive(Debug, Clone, PartialEq)]
pub struct StripLayout {
    /// The deck's name.
    pub name: Rect,
    /// The headphone (PFL) button.
    pub pfl: Rect,
    /// The knobs' centres, top to bottom.
    pub knobs: Vec<Pos2>,
    pub knob_radius: f32,
    /// Each knob's caption, under it.
    pub captions: Vec<Rect>,
    /// The fader's travel.
    pub fader: Rect,
    /// The level meter, when the strip has one.
    pub meter: Option<Rect>,
}

/// Lays out a strip of `knobs` knobs in `rect`, the knobs as large as the strip's height
/// and width let them be (at most [`KNOB_RADIUS`]).
pub fn strip_layout(rect: Rect, knobs: usize, meter: bool) -> StripLayout {
    let inner = rect.shrink2(vec2(6.0, 4.0));
    let header = Rect::from_min_size(inner.min, vec2(inner.width(), HEADER_HEIGHT));
    let pfl_width = PFL_WIDTH.min(header.width() / 2.0 - HEADER_GAP / 2.0);
    let pfl = Rect::from_min_max(pos2(header.right() - pfl_width, header.top()), header.max);
    let name = Rect::from_min_max(header.min, pos2(pfl.left() - HEADER_GAP, header.bottom()));
    let travel = (header.bottom() + 6.0)..=(rect.bottom() - 10.0);
    let fader_x = inner.right() - 6.0 - METER_WIDTH * 2.0 - FADER_WIDTH;
    let fader = Rect::from_x_y_ranges(fader_x..=fader_x + FADER_WIDTH, travel.clone());
    let meter = meter.then(|| {
        Rect::from_x_y_ranges(
            fader.right() + METER_WIDTH..=fader.right() + METER_WIDTH * 2.0,
            travel,
        )
    });
    // The knobs share the height under the header; each has its caption under it.
    let column = egui::Rangef::new(inner.left(), fader.left() - 4.0);
    let top = header.bottom() + 6.0;
    let room = (inner.bottom() - top).max(0.0);
    let step = (room / knobs.max(1) as f32).min(KNOB_STEP);
    let knob_radius = KNOB_RADIUS
        .min((step - CAPTION_HEIGHT - 4.0) / 2.0)
        .min(column.span() / 2.0)
        .max(0.0);
    let x = column.center();
    let knobs: Vec<Pos2> = (0..knobs)
        .map(|i| pos2(x, top + i as f32 * step + knob_radius))
        .collect();
    let captions = knobs
        .iter()
        .map(|centre| {
            Rect::from_min_size(
                pos2(column.min, centre.y + knob_radius + 2.0),
                vec2(column.span(), CAPTION_HEIGHT),
            )
        })
        .collect();
    StripLayout {
        name,
        pfl,
        knobs,
        knob_radius,
        captions,
        fader,
        meter,
    }
}

/// Draws one channel in `rect`: the deck's header with the headphone button, the knobs,
/// the fader and the meter. The meter's bar shows `level`, the live peak, and a line marks
/// `held`, the highest recent peak.
pub fn strip(
    ui: &mut Ui,
    rect: Rect,
    deck: DeckId,
    frame: &ControlFrame,
    stems_active: bool,
    (level, held): (f32, f32),
    sections: &Sections,
) {
    ui.set_clip_rect(rect.intersect(ui.clip_rect()));
    let p = theme::palette(ui.ctx());
    let accent = p.deck_accent(deck);
    let state = frame.control.deck(deck);
    let ghosts = &frame.ghosts[deck.index()];
    let painter = ui.painter().clone();
    painter.rect_filled(rect.shrink(2.0), CornerRadius::same(4), p.panel);

    let [high, mid, low] = eq_labels(frame.eq_mode, stems_active, frame.stem_knobs);
    let gain = ("GAIN", state.trim, ghosts[GHOST_TRIM]);
    let filter = ("FILTER", state.filter, ghosts[GHOST_FILTER]);
    let all = [
        gain,
        (high, state.eq_high, ghosts[GHOST_EQ_HIGH]),
        (mid, state.eq_mid, ghosts[GHOST_EQ_MID]),
        (low, state.eq_low, ghosts[GHOST_EQ_LOW]),
        filter,
    ];
    let without_eq = [gain, filter];
    let knobs: &[_] = if sections.mixer_eq { &all } else { &without_eq };
    let layout = strip_layout(rect, knobs.len(), sections.mixer_meters);

    caption(
        ui,
        layout.name,
        &format!("DECK {}", deck.index() + 1),
        accent,
    );
    painter.rect(
        layout.pfl,
        CornerRadius::same(3),
        if state.pfl { accent } else { p.background },
        Stroke::new(1.0, accent),
        StrokeKind::Inside,
    );
    let pfl_text = if state.pfl { p.on_accent } else { p.text };
    caption(ui, layout.pfl, "PFL", pfl_text);

    for (i, &(name, value, ghost)) in knobs.iter().enumerate() {
        let centre = layout.knobs[i];
        knob(&painter, centre, layout.knob_radius, value, ghost, accent);
        caption(ui, layout.captions[i], name, p.text_dim);
    }

    fader(
        &painter,
        layout.fader,
        state.fader,
        ghosts[GHOST_FADER],
        accent,
    );
    if let Some(meter) = layout.meter {
        level_bar(&painter, meter, level);
        peak_line(&painter, meter, held);
    }
}

/// A knob at `value` (0 to 1) with its pickup marker, if any.
fn knob(
    painter: &egui::Painter,
    centre: Pos2,
    radius: f32,
    value: f32,
    ghost: Option<f32>,
    accent: Color32,
) {
    let p = theme::palette(painter.ctx());
    let on_rim = |v: f32, inset: f32| {
        let angle = (v.clamp(0.0, 1.0) * 2.0 - 1.0) * KNOB_SWEEP;
        centre + vec2(angle.sin(), -angle.cos()) * (radius - inset).max(0.0)
    };
    painter.circle(centre, radius, p.background, Stroke::new(1.5, p.text_dim));
    painter.line_segment(
        [on_rim(value, radius * 0.66), on_rim(value, 1.0)],
        Stroke::new(2.5, accent),
    );
    if let Some(at) = ghost_marker(ghost, |v| on_rim(v, -3.0)) {
        painter.circle_stroke(at, 3.5, Stroke::new(1.5, p.cue));
    }
}

/// A vertical fader whose handle sits at `value` (0 at the bottom).
fn fader(painter: &egui::Painter, track: Rect, value: f32, ghost: Option<f32>, accent: Color32) {
    let p = theme::palette(painter.ctx());
    let slot = Rect::from_center_size(track.center(), vec2(4.0, track.height()));
    painter.rect_filled(slot, CornerRadius::same(2), p.background);
    let y = |v: f32| track.bottom() - v.clamp(0.0, 1.0) * track.height();
    let handle = Rect::from_center_size(pos2(track.center().x, y(value)), vec2(track.width(), 8.0));
    painter.rect_filled(handle, CornerRadius::same(2), accent);
    if let Some(at) = ghost_marker(ghost, |v| pos2(track.center().x, y(v))) {
        let wide = Rect::from_center_size(at, vec2(track.width() + 6.0, 8.0));
        painter.rect_stroke(
            wide,
            CornerRadius::same(2),
            Stroke::new(1.5, p.cue),
            StrokeKind::Outside,
        );
    }
}

/// A vertical bar filled to `level`.
fn level_bar(painter: &egui::Painter, bar: Rect, level: f32) {
    let p = theme::palette(painter.ctx());
    painter.rect_filled(bar, CornerRadius::ZERO, p.background);
    let level = level.clamp(0.0, 1.0);
    let filled = Rect::from_min_max(
        pos2(bar.left(), bar.bottom() - level * bar.height()),
        bar.max,
    );
    let colour = if level > METER_HOT {
        p.meter_clip
    } else {
        p.meter_ok
    };
    painter.rect_filled(filled, CornerRadius::ZERO, colour);
}

/// A line across `bar` at `peak`, the held peak over the live level.
fn peak_line(painter: &egui::Painter, bar: Rect, peak: f32) {
    let p = theme::palette(painter.ctx());
    let peak = peak.clamp(0.0, 1.0);
    if peak <= 0.0 {
        return;
    }
    let y = bar.bottom() - peak * bar.height();
    let colour = if peak > METER_HOT {
        p.meter_clip
    } else {
        p.meter_ok
    };
    painter.rect_filled(
        Rect::from_min_max(
            pos2(bar.left(), y),
            pos2(bar.right(), (y + 2.0).min(bar.bottom())),
        ),
        CornerRadius::ZERO,
        colour,
    );
}

/// The crossfader and the master level in `rect`: the crossfader is dimmed when the
/// configuration turned it off.
fn centre(ui: &mut Ui, rect: Rect, frame: &ControlFrame, master_peak: Option<[f32; 2]>) {
    let p = theme::palette(ui.ctx());
    let painter = ui.painter().clone();
    let dim = if frame.crossfader_enabled { 1.0 } else { 0.35 };
    let half = rect.width() / 2.0;
    let [cross, master] = split(rect.shrink2(vec2(10.0, 6.0)), half);

    let slot = Rect::from_center_size(cross.center(), vec2(cross.width() - 20.0, 4.0));
    painter.rect_filled(slot, CornerRadius::same(2), p.background);
    let x = slot.left() + frame.control.crossfader.clamp(0.0, 1.0) * slot.width();
    let handle = Rect::from_center_size(pos2(x, slot.center().y), vec2(10.0, 22.0));
    painter.rect_filled(handle, CornerRadius::same(2), p.text.gamma_multiply(dim));
    caption(
        ui,
        Rect::from_center_size(pos2(cross.center().x, cross.top() + 4.0), vec2(80.0, 10.0)),
        "CROSSFADER",
        p.text_dim.gamma_multiply(dim),
    );

    let bar = Rect::from_center_size(master.center(), vec2(master.width() - 20.0, 6.0));
    painter.rect_filled(bar, CornerRadius::ZERO, p.background);
    let level = frame.control.master_level.clamp(0.0, 1.0);
    painter.rect_filled(
        Rect::from_min_size(bar.min, vec2(bar.width() * level, bar.height())),
        CornerRadius::ZERO,
        p.text,
    );
    // The peaks ride on the bar's edge so the level and the signal read together.
    if let Some(peaks) = master_peak {
        let peak = peaks[0].max(peaks[1]).clamp(0.0, 1.0);
        let x = bar.left() + peak * bar.width();
        painter.line_segment(
            [pos2(x, bar.top() - 3.0), pos2(x, bar.bottom() + 3.0)],
            Stroke::new(
                1.5,
                if peak > METER_HOT {
                    p.meter_clip
                } else {
                    p.meter_ok
                },
            ),
        );
    }
    caption(
        ui,
        Rect::from_center_size(
            pos2(master.center().x, master.top() + 4.0),
            vec2(80.0, 10.0),
        ),
        "MASTER",
        p.text_dim,
    );
}

#[cfg(test)]
mod tests {
    use egui::Vec2;

    use super::*;

    #[test]
    fn eq_labels_follow_the_mode() {
        use EqMode::{Eq, Stems};
        use StemKnobs::{Raw, Vdj};
        assert_eq!(eq_labels(Eq, true, Vdj), ["HIGH", "MID", "LOW"]);
        assert_eq!(eq_labels(Stems, false, Vdj), ["HIGH", "MID", "LOW"]);
        assert_eq!(eq_labels(Stems, true, Vdj), ["HIHAT", "MEL/VOX", "KICK"]);
        assert_eq!(eq_labels(Stems, true, Raw), ["VOCALS", "INSTRU", "BASS"]);
    }

    #[test]
    fn a_ghost_marks_where_the_control_must_go() {
        let at = |v: f32| pos2(10.0, 100.0 - v * 100.0);
        assert_eq!(ghost_marker(Some(0.2), at), Some(pos2(10.0, 80.0)));
        assert_eq!(ghost_marker(Some(1.5), at), Some(pos2(10.0, 0.0)));
        assert_eq!(ghost_marker(None, at), None);
    }

    #[test]
    fn a_strip_keeps_its_knobs_inside() {
        // 4-Decks at 1024 wide and 700 or 900 high, two decks at 1024x700 (Pro and
        // Performance) and at 1440x900.
        let sizes = [
            (82.0, 224.0),
            (82.0, 290.0),
            (123.0, 248.0),
            (123.0, 246.0),
            (115.0, 290.0),
            (173.0, 286.0),
        ];
        for (width, height) in sizes {
            for knobs in [2, 5] {
                let rect = Rect::from_min_size(pos2(10.0, 20.0), vec2(width, height));
                let layout = strip_layout(rect, knobs, true);
                let r = layout.knob_radius;
                let at = format!("{knobs} knobs in {width}x{height}: {layout:?}");
                assert!(r >= 8.0, "{at}");
                assert!(!layout.name.intersects(layout.pfl), "{at}");
                assert!(rect.contains_rect(layout.name) && rect.contains_rect(layout.pfl));
                for (i, (centre, caption)) in layout.knobs.iter().zip(&layout.captions).enumerate()
                {
                    let circle = Rect::from_center_size(*centre, Vec2::splat(2.0 * r));
                    assert!(rect.contains_rect(circle), "{at}");
                    assert!(rect.contains_rect(*caption), "{at}");
                    assert!(circle.top() >= layout.pfl.bottom(), "{at}");
                    assert!(circle.right() < layout.fader.left(), "{at}");
                    assert!(caption.right() < layout.fader.left(), "{at}");
                    assert!(caption.top() >= circle.bottom(), "{at}");
                    if let Some(next) = layout.knobs.get(i + 1) {
                        assert!(caption.bottom() <= next.y - r, "{at}");
                    }
                }
                assert!(rect.contains_rect(layout.fader));
                assert!(rect.contains_rect(layout.meter.unwrap()));
            }
        }
    }

    #[test]
    fn a_peak_is_held_for_a_second() {
        let start = Instant::now();
        let mut hold = Hold::default();
        assert_eq!(hold.update(0.8, start), 0.8);
        assert_eq!(hold.update(0.2, start + Duration::from_millis(900)), 0.8);
        assert_eq!(hold.update(0.2, start + PEAK_HOLD), 0.2);
        assert_eq!(hold.update(0.5, start + PEAK_HOLD), 0.5);
    }
}
