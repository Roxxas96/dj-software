//! The mixer draws headless, without a GPU.

use std::sync::mpsc::{Receiver, channel};

use actions::{Action, Input};
use egui::epaint::{CircleShape, Shape};
use egui::{Color32, Event, Modifiers, PointerButton, Pos2, Rect, Vec2, pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use engine::{DeckId, EqMode, StemKnobs};
use ui::controls::{HeldButtons, Sink};
use ui::deck::DeckReads;
use ui::layout::{Sections, sections};
use ui::mixer::{Mixer, mixer_layout};
use ui::{ControlFrame, Layout, UiCommand};

/// The two-deck mixer's area in the tests.
const MIXER: Rect = Rect {
    min: pos2(0.0, 0.0),
    max: pos2(360.0, 300.0),
};

/// Draws the two-deck mixer in [`MIXER`] with `sections`.
fn draw_with(frame: ControlFrame, stems_active: bool, sections: Sections) -> Harness<'static> {
    let mut reads = DeckReads::default();
    for (snapshot, _) in &mut reads.decks {
        snapshot.stems_active = stems_active;
    }
    let mut mixer = Mixer::default();
    let mut harness = Harness::builder()
        .with_size([380.0, 320.0])
        .build_ui(move |ui| {
            let commands = channel().0;
            let mut held = HeldButtons::default();
            mixer.show(
                ui,
                MIXER,
                &frame,
                &reads,
                &sections,
                &mut Sink::new(&commands, &mut held),
            );
        });
    harness.run_steps(3);
    harness
}

fn draw(frame: &ControlFrame, stems_active: bool) -> Harness<'static> {
    draw_with(frame.clone(), stems_active, sections(Layout::Pro))
}

#[test]
fn mixer_shows_knob_labels() {
    let frame = ControlFrame {
        eq_mode: EqMode::Stems,
        stem_knobs: StemKnobs::Vdj,
        ..ControlFrame::default()
    };
    let harness = draw(&frame, true);
    harness.get_all_by_label("KICK").next().unwrap();
    harness.get_all_by_label("HIHAT").next().unwrap();
    assert_eq!(harness.get_all_by_label("GAIN").count(), 2);
    let plain = draw(&frame, false);
    plain.get_all_by_label("LOW").next().unwrap();
}

#[test]
fn a_pending_pickup_leaves_the_labels_in_place() {
    let mut frame = ControlFrame::default();
    frame.ghosts[0][5] = Some(0.2);
    frame.ghosts[0][1] = Some(0.9);
    assert_eq!(draw(&frame, false).get_all_by_label("FILTER").count(), 2);
}

#[test]
fn starter_keeps_the_eq() {
    let harness = draw_with(ControlFrame::default(), false, sections(Layout::Starter));
    for label in ["GAIN", "LOW", "FILTER"] {
        assert_eq!(harness.get_all_by_label(label).count(), 2, "{label}");
    }
}

#[test]
fn without_the_eq_gain_and_filter_stay() {
    let hidden = Sections {
        mixer_eq: false,
        ..sections(Layout::Pro)
    };
    let harness = draw_with(ControlFrame::default(), false, hidden);
    assert_eq!(harness.get_all_by_label("GAIN").count(), 2);
    assert_eq!(harness.get_all_by_label("FILTER").count(), 2);
    assert!(harness.query_by_label("LOW").is_none());
}

/// Every circle `shapes` draw, nested ones included.
fn circles(shapes: impl IntoIterator<Item = Shape>, out: &mut Vec<CircleShape>) {
    for shape in shapes {
        match shape {
            Shape::Circle(circle) => out.push(circle),
            Shape::Vec(inner) => circles(inner, out),
            _ => {}
        }
    }
}

#[test]
fn ghosts_still_draw() {
    // The left side's deck 1 waits for its gain to come back to the middle.
    let mut frame = ControlFrame::default();
    frame.ghosts[0][0] = Some(0.5);
    let harness = draw(&frame, false);
    let cue = ui::theme::palette(&harness.ctx).cue;
    let layout = mixer_layout(MIXER, true, true);
    let gain = layout.sides[0].knobs[0];
    // At 0.5 the marker sits straight above the knob, just outside its rim.
    let at = gain - egui::vec2(0.0, layout.knob_radius + 3.0);
    let mut drawn = Vec::new();
    circles(
        harness.output().shapes.iter().map(|c| c.shape.clone()),
        &mut drawn,
    );
    let hollow = |c: &CircleShape| c.fill == egui::Color32::TRANSPARENT && c.stroke.color == cue;
    assert!(
        drawn
            .iter()
            .any(|c| hollow(c) && c.center.distance(at) < 1.0),
        "no hollow marker at {at:?} by the gain at {gain:?}"
    );
    // Without a ghost there is none.
    let plain = draw(&ControlFrame::default(), false);
    let mut drawn = Vec::new();
    circles(
        plain.output().shapes.iter().map(|c| c.shape.clone()),
        &mut drawn,
    );
    assert!(!drawn.iter().any(hollow));
}

#[test]
fn a_narrow_strip_keeps_its_name_off_the_headphone_button() {
    // A strip of the 4-Decks mixer at 1024 wide.
    let frame = ControlFrame {
        view: ui::ViewState {
            four_decks: true,
            ..ui::ViewState::default()
        },
        ..ControlFrame::default()
    };
    let rect = Rect::from_min_max(pos2(0.0, 0.0), pos2(82.0, 224.0));
    let mut harness = Harness::builder()
        .with_size([200.0, 300.0])
        .build_ui(move |ui| {
            ui::mixer::strip(
                ui,
                rect,
                DeckId::new(0).unwrap(),
                &frame,
                false,
                (0.5, 0.7),
                &ui::layout::four_deck_sections(),
                &mut Sink::new(&channel().0, &mut HeldButtons::default()),
            );
        });
    harness.run_steps(3);
    let layout = ui::mixer::strip_layout(rect, 5, true);
    let name = harness.get_by_label("DECK 1").rect();
    assert!(
        layout.name.expand(0.5).contains_rect(name),
        "{name:?} {layout:?}"
    );
    assert!(!name.intersects(layout.pfl), "{name:?} {layout:?}");
    assert!(
        layout
            .pfl
            .expand(0.5)
            .contains_rect(harness.get_by_label("PFL").rect())
    );
}

/// Draws the two-deck mixer of `frame` in [`MIXER`], sending what it asks for to the
/// receiver returned. Short steps, so two clicks in a row make a double-click.
fn sending(frame: ControlFrame) -> (Harness<'static>, Receiver<UiCommand>) {
    let (commands, received) = channel();
    let reads = DeckReads::default();
    let sections = sections(Layout::Pro);
    let mut mixer = Mixer::default();
    let mut held = HeldButtons::default();
    let mut harness = Harness::builder()
        .with_size([380.0, 320.0])
        .with_step_dt(0.01)
        .build_ui(move |ui| {
            let mut sink = Sink::new(&commands, &mut held);
            mixer.show(ui, MIXER, &frame, &reads, &sections, &mut sink);
            held.sweep(ui.ctx(), &commands);
        });
    harness.run_steps(3);
    (harness, received)
}

fn button(pressed: bool, at: Pos2) -> Event {
    Event::PointerButton {
        pos: at,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    }
}

/// Presses at `from`, moves to `to` in five steps and lets go there.
fn drag(harness: &mut Harness<'_>, from: Pos2, to: Pos2) {
    harness.event(Event::PointerMoved(from));
    harness.run_steps(1);
    harness.event(button(true, from));
    harness.run_steps(1);
    for k in 1..=5 {
        harness.event(Event::PointerMoved(from + (to - from) * k as f32 / 5.0));
        harness.run_steps(1);
    }
    harness.event(button(false, to));
    harness.run_steps(2);
}

/// Presses at `at`, holds for a frame and lets go.
fn click(harness: &mut Harness<'_>, at: Pos2) {
    harness.event(Event::PointerMoved(at));
    harness.run_steps(1);
    harness.event(button(true, at));
    harness.run_steps(1);
    harness.event(button(false, at));
    harness.run_steps(2);
}

/// Clicks twice at `at`, long enough after any click before not to count with it.
fn double_click(harness: &mut Harness<'_>, at: Pos2) {
    harness.run_steps(40);
    harness.event(Event::PointerMoved(at));
    harness.run_steps(1);
    for _ in 0..2 {
        harness.event(button(true, at));
        harness.event(button(false, at));
        harness.run_steps(1);
    }
    harness.run_steps(1);
}

fn deck(number: usize) -> DeckId {
    DeckId::new(number - 1).unwrap()
}

/// The last value `received` holds for `action`.
fn last_value(received: &Receiver<UiCommand>, action: Action) -> Option<f32> {
    received
        .try_iter()
        .filter_map(|command| match command {
            UiCommand::Action(a, Input::Absolute(v)) if a == action => Some(v),
            _ => None,
        })
        .last()
}

#[test]
fn dragging_the_left_high_knob_sends_eq_high() {
    let (mut harness, received) = sending(ControlFrame::default());
    let layout = mixer_layout(MIXER, true, true);
    let high = layout.sides[0].knobs[1];
    // 30 points up of the 150 a full turn takes: from the middle to 0.7, less the few
    // points egui waits for before a press becomes a drag.
    drag(&mut harness, high, high - vec2(0.0, 30.0));
    let value = last_value(&received, Action::EqHigh(deck(1))).expect("no EqHigh");
    assert!((0.64..=0.7).contains(&value), "got {value}");
}

#[test]
fn stems_mode_keeps_the_eq_actions() {
    let frame = ControlFrame {
        eq_mode: EqMode::Stems,
        ..ControlFrame::default()
    };
    let (mut harness, received) = sending(frame);
    let low = mixer_layout(MIXER, true, true).sides[1].knobs[3];
    drag(&mut harness, low, low + vec2(0.0, 30.0));
    let value = last_value(&received, Action::EqLow(deck(2))).expect("no EqLow");
    assert!(value < 0.5, "got {value}");
}

#[test]
fn double_click_resets_the_gain_to_unity() {
    let mut frame = ControlFrame::default();
    frame.control.decks[0].trim = 0.9;
    let (mut harness, received) = sending(frame);
    let gain = mixer_layout(MIXER, true, true).sides[0].knobs[0];
    double_click(&mut harness, gain);
    assert_eq!(last_value(&received, Action::Trim(deck(1))), Some(0.5));
}

#[test]
fn dragging_a_channel_fader_sends_absolute() {
    let (mut harness, received) = sending(ControlFrame::default());
    let fader = mixer_layout(MIXER, true, true).sides[1].fader;
    drag(&mut harness, fader.center(), fader.center_top());
    let value = last_value(&received, Action::Fader(deck(2))).expect("no Fader");
    assert!(value > 0.95, "got {value}");
}

/// The first value a press at `at` sends for `action`, holding still for a few frames.
fn first_value_pressed(
    harness: &mut Harness<'_>,
    received: &Receiver<UiCommand>,
    at: Pos2,
    action: Action,
) -> Option<f32> {
    harness.event(Event::PointerMoved(at));
    harness.run_steps(1);
    harness.event(button(true, at));
    harness.run_steps(3);
    harness.event(button(false, at));
    harness.run_steps(2);
    received.try_iter().find_map(|command| match command {
        UiCommand::Action(a, Input::Absolute(v)) if a == action => Some(v),
        _ => None,
    })
}

#[test]
fn a_press_on_a_handle_takes_it_where_it_is() {
    let mut frame = ControlFrame::default();
    frame.control.decks[1].fader = 0.5;
    frame.control.crossfader = 0.3;
    frame.control.master_level = 0.6;
    let (mut harness, received) = sending(frame);
    let layout = mixer_layout(MIXER, true, true);
    // On the handle, a little off its middle: no jump.
    let fader = layout.sides[1].fader;
    let handle = pos2(fader.center().x, fader.center().y + 3.0);
    let channel = Action::Fader(deck(2));
    let value = first_value_pressed(&mut harness, &received, handle, channel);
    assert_eq!(value, Some(0.5));
    let slot = layout.crossfader.shrink2(vec2(5.0, 0.0));
    let x = slot.left() + 0.3 * slot.width() + 3.0;
    let at = pos2(x, slot.center().y);
    let value = first_value_pressed(&mut harness, &received, at, Action::Crossfader);
    assert!(value.is_some_and(|v| (v - 0.3).abs() < 1e-4), "{value:?}");
    let bar = layout.top.master_bar;
    let at = pos2(bar.left() + 0.6 * bar.width() - 2.0, bar.center().y);
    let value = first_value_pressed(&mut harness, &received, at, Action::MasterLevel);
    assert!(value.is_some_and(|v| (v - 0.6).abs() < 1e-4), "{value:?}");
    // Off the handle, the track jumps.
    let value = first_value_pressed(&mut harness, &received, fader.center_top(), channel);
    assert_eq!(value, Some(1.0));
}

#[test]
fn clicking_pfl_sends_a_press_and_release() {
    let (mut harness, received) = sending(ControlFrame::default());
    let pfl = mixer_layout(MIXER, true, true).sides[0].pfl;
    click(&mut harness, pfl.center());
    let sent: Vec<UiCommand> = received.try_iter().collect();
    let action = Action::PflToggle(deck(1));
    assert_eq!(
        sent,
        [
            UiCommand::Action(action, Input::Press),
            UiCommand::Action(action, Input::Release)
        ]
    );
}

#[test]
fn dragging_the_crossfader_sends_absolute() {
    let (mut harness, received) = sending(ControlFrame::default());
    let cross = mixer_layout(MIXER, true, true).crossfader;
    drag(&mut harness, cross.center(), cross.right_center());
    let value = last_value(&received, Action::Crossfader).expect("no Crossfader");
    assert!(value > 0.95, "got {value}");
    double_click(&mut harness, cross.left_center() + vec2(10.0, 0.0));
    assert_eq!(last_value(&received, Action::Crossfader), Some(0.5));
}

#[test]
fn master_and_headphone_knobs_send_their_actions() {
    let (mut harness, received) = sending(ControlFrame::default());
    // At this width the words would leave the master level's bar short: letters.
    harness.get_by_label("HP");
    harness.get_by_label("MX");
    harness.get_by_label("MASTER");
    let layout = mixer_layout(MIXER, true, true);
    let [level, mix] = layout.top.headphone;
    // 15 points of the 150 a full turn takes, less the drag's threshold.
    drag(&mut harness, level, level - vec2(0.0, 15.0));
    let value = last_value(&received, Action::HeadphoneLevel).expect("no HeadphoneLevel");
    assert!((0.84..=0.9).contains(&value), "got {value}");
    drag(&mut harness, mix, mix - vec2(0.0, 15.0));
    let value = last_value(&received, Action::HeadphoneMix).expect("no HeadphoneMix");
    assert!((0.04..=0.1).contains(&value), "got {value}");
    // The master level is a bar: a click sets it, a double-click puts it back.
    let bar = layout.top.master_bar;
    click(
        &mut harness,
        bar.left_center() + vec2(bar.width() / 4.0, 0.0),
    );
    let value = last_value(&received, Action::MasterLevel).expect("no MasterLevel");
    assert!((value - 0.25).abs() < 0.03, "got {value}");
    double_click(&mut harness, bar.center());
    assert_eq!(last_value(&received, Action::MasterLevel), Some(0.8));
    double_click(&mut harness, level);
    assert_eq!(last_value(&received, Action::HeadphoneLevel), Some(0.8));

    // The 4-Decks mixer has them too, with their letters.
    let frame = ControlFrame {
        view: ui::ViewState {
            four_decks: true,
            ..ui::ViewState::default()
        },
        ..ControlFrame::default()
    };
    let mut screen = ui::Screen::default();
    let mut four = Harness::builder()
        .with_size([1440.0, 900.0])
        .build_ui(move |ui| ui::root(ui, &frame, None, &mut screen));
    four.run_steps(3);
    four.get_by_label("HP");
    four.get_by_label("MX");
    assert!(four.query_by_label("HEADPHONE").is_none());
}

/// The hollow markers `harness` draws in the cue colour: the pickups.
fn hollow_markers(harness: &Harness<'_>) -> Vec<Rect> {
    let cue = ui::theme::palette(&harness.ctx).cue;
    fn walk(shapes: impl IntoIterator<Item = Shape>, cue: Color32, out: &mut Vec<Rect>) {
        for shape in shapes {
            match shape {
                Shape::Circle(c) if c.fill == Color32::TRANSPARENT && c.stroke.color == cue => {
                    out.push(Rect::from_center_size(
                        c.center,
                        Vec2::splat(2.0 * c.radius),
                    ));
                }
                Shape::Rect(r) if r.fill == Color32::TRANSPARENT && r.stroke.color == cue => {
                    out.push(r.rect);
                }
                Shape::Vec(inner) => walk(inner, cue, out),
                _ => {}
            }
        }
    }
    let mut out = Vec::new();
    walk(
        harness.output().shapes.iter().map(|c| c.shape.clone()),
        cue,
        &mut out,
    );
    out
}

#[test]
fn ghosts_for_every_absolute_control() {
    let window = |frame: ControlFrame| {
        let mut screen = ui::Screen::default();
        let mut harness = Harness::builder()
            .with_size([1440.0, 900.0])
            .build_ui(move |ui| ui::root(ui, &frame, None, &mut screen));
        harness.run_steps(3);
        hollow_markers(&harness)
    };
    assert_eq!(window(ControlFrame::default()), []);
    // Deck 1's trim, EQ, filter, fader and pitch, by their place in the ghosts.
    for (name, at) in [
        ("trim", 0),
        ("high", 1),
        ("mid", 2),
        ("low", 3),
        ("filter", 4),
        ("fader", 5),
        ("pitch", 6),
        ("fx level", 7),
    ] {
        let mut frame = ControlFrame::default();
        frame.ghosts[0][at] = Some(0.3);
        assert_eq!(window(frame).len(), 1, "{name}");
    }
    // The crossfader, the master level, the headphone level and mix.
    for (name, at) in [
        ("crossfader", 0),
        ("master", 1),
        ("headphone", 2),
        ("mix", 3),
    ] {
        let mut frame = ControlFrame::default();
        frame.global_ghosts[at] = Some(0.3);
        assert_eq!(window(frame).len(), 1, "{name}");
    }
}
