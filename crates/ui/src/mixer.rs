//! The mixer: what the controller set for each side's active deck (gain, EQ, filter,
//! fader, headphone button, level) and the crossfader and master level between them. It
//! shows `ControlState` and never changes it. A hollow marker on a knob or fader is where
//! the physical control waits to pick its value up after a deck switch (soft takeover).

use std::time::{Duration, Instant};

use actions::Side;
use egui::{
    Color32, CornerRadius, Label, Pos2, Rect, RichText, Stroke, StrokeKind, Ui, pos2, vec2,
};
use engine::{DeckId, EqMode, StemKnobs};

use crate::deck::DeckReads;
use crate::layout::{Sections, mixer_order};
use crate::model::{
    ControlFrame, GHOST_EQ_HIGH, GHOST_EQ_LOW, GHOST_EQ_MID, GHOST_FADER, GHOST_FILTER, GHOST_TRIM,
};
use crate::theme::{self, SECTION_GAP};

/// How long a channel meter's peak marker stays at the highest peak.
const PEAK_HOLD: Duration = Duration::from_secs(1);
/// The height of the top row: the decks' names and the master level.
const TOP_ROW: f32 = 18.0;
/// The height of the crossfader's row.
const BOTTOM_ROW: f32 = 24.0;
/// Between the rows.
const ROW_GAP: f32 = 4.0;
/// The share of the mixer's width the faders, the meter and the master level take, in
/// the middle.
const MIDDLE_SHARE: f32 = 0.4;
/// The crossfader's share of the mixer's width.
const CROSSFADER_SHARE: f32 = 0.4;
/// How far a fader sits from the centre, as a share of half the middle band.
const FADER_OFFSET: f32 = 0.6;
/// The headphone button's height under a fader.
const PFL_HEIGHT: f32 = 18.0;
/// Between the two bars of the centre meter.
const METER_GAP: f32 = 4.0;
/// The width of the master level's caption.
const MASTER_CAPTION: f32 = 44.0;
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
    /// Draws the mixer in `rect`, its meters from `reads`. Read without a monitor, the
    /// meters are empty and the EQ keeps its band names. Two decks get VirtualDJ's mixer
    /// ([`mixer_layout`]); four get a strip each, in [`mixer_order`].
    pub fn show(
        &mut self,
        ui: &mut Ui,
        rect: Rect,
        frame: &ControlFrame,
        reads: &DeckReads,
        sections: &Sections,
    ) {
        let now = Instant::now();
        let peaks = reads.meters.map(|m| m.channel_peak).unwrap_or_default();
        let mut levels = |deck: DeckId| {
            let level = peaks[deck.index()];
            (level, self.holds[deck.index()].update(level, now))
        };
        let p = theme::palette(ui.ctx());
        // One panel for the whole mixer, with half the gap to each deck panel.
        ui.painter().rect_filled(
            rect.shrink2(vec2(SECTION_GAP / 2.0, 0.0)),
            CornerRadius::ZERO,
            p.panel,
        );
        let master_peak = reads.meters.map(|m| m.master_peak);
        if frame.view.four_decks {
            let (top, _, bottom) = rows(rect);
            let (master_rect, crossfader_rect) = centre_parts(rect, top, bottom);
            let width = rect.width() / 4.0;
            // The strips keep their own margins: they take all the height between the rows.
            let height = bottom.top() - top.bottom();
            for (i, deck) in mixer_order().into_iter().enumerate() {
                let strip_rect = Rect::from_min_size(
                    pos2(rect.left() + i as f32 * width, top.bottom()),
                    vec2(width, height),
                );
                let stems_active = reads.decks[deck.index()].0.stems_active;
                let level = levels(deck);
                ui.scope_builder(egui::UiBuilder::new().max_rect(strip_rect), |ui| {
                    strip(ui, strip_rect, deck, frame, stems_active, level, sections);
                });
            }
            master(ui, master_rect, frame, master_peak);
            crossfader(ui, crossfader_rect, frame);
        } else {
            let layout = mixer_layout(rect, sections.mixer_eq, sections.mixer_meters);
            for (side, part) in Side::ALL.into_iter().zip(&layout.sides) {
                let deck = frame.control.side(side).active_deck;
                let channel = Channel {
                    deck,
                    frame,
                    stems_active: reads.decks[deck.index()].0.stems_active,
                    level: levels(deck),
                    eq: sections.mixer_eq,
                };
                side_strip(ui, part, layout.knob_radius, &channel);
            }
            master(ui, layout.master, frame, master_peak);
            crossfader(ui, layout.crossfader, frame);
        }
    }
}

/// What a channel draws: its deck, what the controller set, the stems' state, the meter's
/// live and held peaks, and whether the EQ shows.
struct Channel<'a> {
    deck: DeckId,
    frame: &'a ControlFrame,
    stems_active: bool,
    level: (f32, f32),
    eq: bool,
}

/// A knob's caption, value (0 to 1) and pickup marker.
type Knob = (&'static str, f32, Option<f32>);

/// A channel's knobs, top to bottom: the gain, the EQ's three (when it shows) and the
/// filter. Only the first of the count are shown.
fn knobs(channel: &Channel) -> ([Knob; 5], usize) {
    let state = channel.frame.control.deck(channel.deck);
    let ghosts = &channel.frame.ghosts[channel.deck.index()];
    let [high, mid, low] = eq_labels(
        channel.frame.eq_mode,
        channel.stems_active,
        channel.frame.stem_knobs,
    );
    let gain = ("GAIN", state.trim, ghosts[GHOST_TRIM]);
    let filter = ("FILTER", state.filter, ghosts[GHOST_FILTER]);
    if channel.eq {
        let all = [
            gain,
            (high, state.eq_high, ghosts[GHOST_EQ_HIGH]),
            (mid, state.eq_mid, ghosts[GHOST_EQ_MID]),
            (low, state.eq_low, ghosts[GHOST_EQ_LOW]),
            filter,
        ];
        (all, 5)
    } else {
        ([gain, filter, gain, gain, gain], 2)
    }
}

/// Draws one side of the two-deck mixer: its deck's name, knobs, fader, headphone button
/// and meter bar.
fn side_strip(ui: &mut Ui, layout: &SideLayout, knob_radius: f32, channel: &Channel) {
    let p = theme::palette(ui.ctx());
    let accent = p.deck_accent(channel.deck);
    let state = channel.frame.control.deck(channel.deck);
    let ghosts = &channel.frame.ghosts[channel.deck.index()];
    let painter = ui.painter().clone();
    caption(
        ui,
        layout.header,
        &format!("DECK {}", channel.deck.index() + 1),
        accent,
    );
    let (all, count) = knobs(channel);
    for (i, &(name, value, ghost)) in all[..count].iter().enumerate() {
        knob(&painter, layout.knobs[i], knob_radius, value, ghost, accent);
        caption(ui, layout.captions[i], name, p.text_dim);
    }
    fader(
        &painter,
        layout.fader,
        state.fader,
        ghosts[GHOST_FADER],
        accent,
    );
    pfl_button(ui, layout.pfl, state.pfl, accent);
    if let Some(meter) = layout.meter {
        let (level, held) = channel.level;
        level_bar(&painter, meter, level);
        peak_line(&painter, meter, held);
    }
}

/// The headphone button in `rect`, lit in the deck's `accent` when `on`.
fn pfl_button(ui: &mut Ui, rect: Rect, on: bool, accent: Color32) {
    let p = theme::palette(ui.ctx());
    ui.painter().rect(
        rect,
        CornerRadius::same(3),
        if on { accent } else { p.background },
        Stroke::new(1.0, accent),
        StrokeKind::Inside,
    );
    caption(ui, rect, "PFL", if on { p.on_accent } else { p.text });
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

/// One side's part of the two-deck mixer: the left side's knobs are left of its fader,
/// the right side's right of it.
#[derive(Debug, Clone, PartialEq)]
pub struct SideLayout {
    /// The knobs' centres, top to bottom: the gain, the EQ's three (when shown) and the
    /// filter.
    pub knobs: Vec<Pos2>,
    /// Each knob's caption, under it.
    pub captions: Vec<Rect>,
    /// The fader's travel.
    pub fader: Rect,
    /// The side's bar of the centre meter, when the mixer has meters.
    pub meter: Option<Rect>,
    /// The headphone (PFL) button, under the fader.
    pub pfl: Rect,
    /// The deck's name, in the top row.
    pub header: Rect,
}

/// Where the parts of the two-deck mixer go, as in VirtualDJ: the faders in the middle
/// with the meter between them, the knobs outside them, the master level on top and the
/// crossfader at the bottom.
#[derive(Debug, Clone, PartialEq)]
pub struct MixerLayout {
    /// The left side, then the right.
    pub sides: [SideLayout; 2],
    pub knob_radius: f32,
    /// Both sides' meters, between the faders.
    pub centre_meter: Option<Rect>,
    pub crossfader: Rect,
    /// The master level, in the top row's middle.
    pub master: Rect,
}

/// The mixer's rows: the top one (names and master), the body and the crossfader's.
fn rows(rect: Rect) -> (Rect, Rect, Rect) {
    let inner = rect.shrink2(vec2(6.0, 4.0));
    let top = Rect::from_min_size(inner.min, vec2(inner.width(), TOP_ROW));
    let bottom = Rect::from_min_max(pos2(inner.left(), inner.bottom() - BOTTOM_ROW), inner.max);
    let body = Rect::from_min_max(
        pos2(inner.left(), top.bottom() + ROW_GAP),
        pos2(inner.right(), bottom.top() - ROW_GAP),
    );
    (top, body, bottom)
}

/// The master level and the crossfader in the top and bottom rows, both centred.
fn centre_parts(rect: Rect, top: Rect, bottom: Rect) -> (Rect, Rect) {
    let x = rect.center().x;
    let master = Rect::from_x_y_ranges(
        x - rect.width() * MIDDLE_SHARE / 2.0..=x + rect.width() * MIDDLE_SHARE / 2.0,
        top.y_range(),
    );
    let half = rect.width() * CROSSFADER_SHARE / 2.0;
    let crossfader = Rect::from_x_y_ranges(x - half..=x + half, bottom.y_range());
    (master, crossfader)
}

/// Lays out the two-deck mixer in `rect`: five knobs a side with the `eq`, two without,
/// and the centre meter when there are `meters`. The faders keep their place either way.
pub fn mixer_layout(rect: Rect, eq: bool, meters: bool) -> MixerLayout {
    let (top, body, bottom) = rows(rect);
    let (master, crossfader) = centre_parts(rect, top, bottom);
    let centre = rect.center().x;
    // The middle band holds the faders, the meter between them and the headphone buttons.
    let band = rect.width() * MIDDLE_SHARE / 2.0;
    let offset = band * FADER_OFFSET;
    // The headphone buttons sit at the body's bottom, the faders and the meter above them.
    let pfl_y = egui::Rangef::new(body.bottom() - PFL_HEIGHT, body.bottom());
    let travel = (body.top() + 6.0)..=(pfl_y.min - 8.0);
    let pfl_width = PFL_WIDTH.min(2.0 * (band - offset));
    let knob_count = if eq { 5 } else { 2 };
    let step = (body.height() / knob_count as f32).min(KNOB_STEP);
    // Each side's knobs have the width from the body's edge to the middle band.
    let column_width = centre - band - 4.0 - body.left();
    let knob_radius = KNOB_RADIUS
        .min((step - CAPTION_HEIGHT - 4.0) / 2.0)
        .min(column_width / 2.0)
        .max(0.0);
    let side = |sign: f32| {
        let x = centre + sign * offset;
        let fader = Rect::from_x_y_ranges(
            x - FADER_WIDTH / 2.0..=x + FADER_WIDTH / 2.0,
            travel.clone(),
        );
        let meter = meters.then(|| {
            let near = centre + sign * METER_GAP / 2.0;
            let far = near + sign * METER_WIDTH;
            Rect::from_x_y_ranges(near.min(far)..=near.max(far), travel.clone())
        });
        let pfl = Rect::from_x_y_ranges(x - pfl_width / 2.0..=x + pfl_width / 2.0, pfl_y);
        // The knobs' column: from the mixer's edge to the middle band.
        let edge = centre + sign * (band + 4.0);
        let outer = if sign < 0.0 {
            body.left()
        } else {
            body.right()
        };
        let column = egui::Rangef::new(edge.min(outer), edge.max(outer));
        let header = Rect::from_x_y_ranges(column, top.y_range());
        let knobs: Vec<Pos2> = (0..knob_count)
            .map(|i| pos2(column.center(), body.top() + i as f32 * step + knob_radius))
            .collect();
        let captions = knobs
            .iter()
            .map(|k| {
                Rect::from_min_size(
                    pos2(column.min, k.y + knob_radius + 2.0),
                    vec2(column.span(), CAPTION_HEIGHT),
                )
            })
            .collect();
        SideLayout {
            knobs,
            captions,
            fader,
            meter,
            pfl,
            header,
        }
    };
    MixerLayout {
        sides: [side(-1.0), side(1.0)],
        knob_radius,
        centre_meter: meters.then(|| {
            Rect::from_x_y_ranges(
                centre - METER_GAP / 2.0 - METER_WIDTH..=centre + METER_GAP / 2.0 + METER_WIDTH,
                travel,
            )
        }),
        crossfader,
        master,
    }
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

/// Draws one channel of the 4-Decks mixer in `rect`: the deck's header with the headphone
/// button, the knobs, the fader and the meter. The meter's bar shows `level`, the live
/// peak, and a line marks `held`, the highest recent peak.
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
    let channel = Channel {
        deck,
        frame,
        stems_active,
        level: (level, held),
        eq: sections.mixer_eq,
    };
    let (all, count) = knobs(&channel);
    let layout = strip_layout(rect, count, sections.mixer_meters);

    caption(
        ui,
        layout.name,
        &format!("DECK {}", deck.index() + 1),
        accent,
    );
    pfl_button(ui, layout.pfl, state.pfl, accent);
    for (i, &(name, value, ghost)) in all[..count].iter().enumerate() {
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

/// The crossfader in `rect`, dimmed when the configuration turned it off.
fn crossfader(ui: &mut Ui, rect: Rect, frame: &ControlFrame) {
    let p = theme::palette(ui.ctx());
    let painter = ui.painter();
    let dim = if frame.crossfader_enabled { 1.0 } else { 0.35 };
    let slot = Rect::from_center_size(rect.center(), vec2(rect.width() - 10.0, 4.0));
    painter.rect_filled(slot, CornerRadius::same(2), p.background);
    let x = slot.left() + frame.control.crossfader.clamp(0.0, 1.0) * slot.width();
    let handle = Rect::from_center_size(pos2(x, slot.center().y), vec2(10.0, rect.height() - 4.0));
    painter.rect_filled(handle, CornerRadius::same(2), p.text.gamma_multiply(dim));
}

/// The master level in `rect`: its caption, then a bar with the master's peak on its
/// edge.
fn master(ui: &mut Ui, rect: Rect, frame: &ControlFrame, master_peak: Option<[f32; 2]>) {
    let p = theme::palette(ui.ctx());
    let painter = ui.painter().clone();
    let width = MASTER_CAPTION.min(rect.width() / 2.0);
    let (label, bar) = rect.split_left_right_at_x(rect.left() + width);
    caption(ui, label, "MASTER", p.text_dim);
    let bar = Rect::from_center_size(bar.center(), vec2(bar.width() - 4.0, 6.0));
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
    fn a_four_deck_strip_keeps_its_knobs_inside() {
        // 4-Decks at 1024 wide and 700 or 900 high, and at 1440 wide.
        let sizes = [(82.0, 224.0), (82.0, 290.0), (115.0, 250.0)];
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

    /// The two-deck mixer of the tests: 360 by 300.
    fn mixer() -> (Rect, MixerLayout) {
        let rect = Rect::from_min_size(pos2(10.0, 20.0), vec2(360.0, 300.0));
        (rect, mixer_layout(rect, true, true))
    }

    #[test]
    fn faders_sit_in_the_middle_band() {
        let (rect, layout) = mixer();
        let band = egui::Rangef::new(
            rect.center().x - rect.width() * 0.2,
            rect.center().x + rect.width() * 0.2,
        );
        let [left, right] = &layout.sides;
        for side in &layout.sides {
            assert!(band.contains(side.fader.left()) && band.contains(side.fader.right()));
        }
        assert!(left.fader.right() < rect.center().x);
        assert!(right.fader.left() > rect.center().x);
    }

    #[test]
    fn knobs_sit_outside_their_fader() {
        for eq in [true, false] {
            let (rect, _) = mixer();
            let layout = mixer_layout(rect, eq, true);
            let [left, right] = &layout.sides;
            assert_eq!(left.knobs.len(), if eq { 5 } else { 2 });
            assert_eq!(right.knobs.len(), left.knobs.len());
            assert!(left.knobs.iter().all(|k| k.x < left.fader.min.x));
            assert!(right.knobs.iter().all(|k| k.x > right.fader.max.x));
        }
    }

    #[test]
    fn gain_is_on_top() {
        let (_, layout) = mixer();
        for side in &layout.sides {
            let gain = side.knobs[0].y;
            assert!(side.knobs[1..].iter().all(|k| k.y > gain));
        }
    }

    #[test]
    fn pfl_under_the_faders() {
        let (_, layout) = mixer();
        for side in &layout.sides {
            assert!(side.pfl.min.y >= side.fader.max.y, "{side:?}");
            assert!(side.fader.x_range().contains(side.pfl.center().x));
        }
    }

    #[test]
    fn crossfader_at_the_bottom_and_master_on_top() {
        let (rect, layout) = mixer();
        let cross = layout.crossfader;
        assert!((cross.center().x - rect.center().x).abs() < 0.5);
        for side in &layout.sides {
            for part in [side.fader, side.pfl, side.header] {
                assert!(cross.top() >= part.bottom(), "{part:?}");
            }
            assert!(
                side.knobs
                    .iter()
                    .all(|k| k.y + layout.knob_radius <= cross.top())
            );
            assert!(layout.master.bottom() <= side.fader.top());
            assert!(
                side.knobs
                    .iter()
                    .all(|k| k.y - layout.knob_radius >= layout.master.bottom())
            );
        }
        assert!(rect.contains_rect(cross) && rect.contains_rect(layout.master));
    }

    #[test]
    fn centre_meter_between_the_faders() {
        let (rect, layout) = mixer();
        let [left, right] = &layout.sides;
        let centre = layout.centre_meter.unwrap();
        assert!(centre.left() > left.fader.right() && centre.right() < right.fader.left());
        for side in &layout.sides {
            assert!(centre.contains_rect(side.meter.unwrap()));
        }
        // Without meters the faders stay where they were.
        let bare = mixer_layout(rect, true, false);
        assert!(bare.centre_meter.is_none());
        for (side, with) in bare.sides.iter().zip(&layout.sides) {
            assert!(side.meter.is_none());
            assert_eq!(side.fader, with.fader);
        }
    }

    #[test]
    fn the_mixer_keeps_its_parts_inside_at_every_window() {
        // Two decks at 1024x700 and 1440x900, and the tests' size.
        for (width, height) in [
            (246.0, 250.0),
            (246.0, 300.0),
            (346.0, 290.0),
            (360.0, 300.0),
        ] {
            for eq in [true, false] {
                let rect = Rect::from_min_size(pos2(10.0, 20.0), vec2(width, height));
                let layout = mixer_layout(rect, eq, true);
                let r = layout.knob_radius;
                let at = format!("{width}x{height} eq={eq}: {layout:?}");
                assert!(r >= 8.0, "{at}");
                for side in &layout.sides {
                    for part in [side.fader, side.pfl, side.header] {
                        assert!(rect.contains_rect(part), "{at}");
                    }
                    for (i, (centre, caption)) in side.knobs.iter().zip(&side.captions).enumerate()
                    {
                        let circle = Rect::from_center_size(*centre, Vec2::splat(2.0 * r));
                        assert!(
                            rect.contains_rect(circle) && rect.contains_rect(*caption),
                            "{at}"
                        );
                        assert!(caption.top() >= circle.bottom(), "{at}");
                        assert!(
                            !caption.intersects(side.fader) && !caption.intersects(side.pfl),
                            "{at}"
                        );
                        if let Some(next) = side.knobs.get(i + 1) {
                            assert!(caption.bottom() <= next.y - r, "{at}");
                        }
                    }
                }
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
