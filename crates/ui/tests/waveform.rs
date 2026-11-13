//! The waveform lanes draw headless, without a GPU.

use std::sync::Arc;

use analysis::Waveform;
use egui::{Mesh, Sense, vec2};
use egui_kittest::Harness;
use engine::{DeckId, DeckSnapshot, Grid, Hotcue};
use ui::waveform::Lane;
use ui::{ControlFrame, WaveformPosition, WaveformStyle};

fn draw(snapshot: DeckSnapshot, waveform: &Waveform) {
    draw_styled(snapshot, waveform, WaveformStyle::Frequency, false);
}

fn draw_styled(snapshot: DeckSnapshot, waveform: &Waveform, style: WaveformStyle, overlay: bool) {
    let mut mesh = Arc::new(Mesh::default());
    let mut harness = Harness::builder()
        .with_size([1440.0, 200.0])
        .build_ui(|ui| {
            let (rect, _) = ui.allocate_exact_size(ui.available_size(), Sense::hover());
            let lane = Lane {
                deck: DeckId::new(0).unwrap(),
                snapshot: &snapshot,
                waveform: Some(waveform),
                position_frames: 441_000.0,
                beats: 16.0,
                style,
                palette: ui::WaveformPalette::default(),
                overlay,
                marks: None,
            };
            lane.draw(&ui.painter_at(rect), rect, &mut mesh);
        });
    harness.run_steps(3);
    drop(harness);
    assert!(!mesh.vertices.is_empty());
}

#[test]
fn lane_draws_with_and_without_a_grid() {
    let waveform = Waveform {
        frames_per_bucket: 128,
        buckets: (0..10_000)
            .map(|i| [(i % 256) as u8, 200, 80, 30])
            .collect(),
    };
    let mut loaded = DeckSnapshot {
        loaded: true,
        playing: true,
        position_frames: 441_000.0,
        duration_frames: 1_280_000,
        track_sample_rate: 44_100,
        cue_frames: 430_000.0,
        loop_in: Some(440_000.0),
        loop_out: Some(452_000.0),
        loop_active: true,
        ..DeckSnapshot::default()
    };
    loaded.hotcues[0] = Some(Hotcue::point(435_000.0));
    loaded.hotcues[2] = Some(Hotcue::point(450_000.0));
    draw(loaded, &waveform);
    let gridded = DeckSnapshot {
        grid: Some(Grid {
            beat_period_frames: 22_050.0,
            first_beat_frames: 1_000.0,
        }),
        ..loaded
    };
    draw(gridded, &waveform);
}

#[test]
fn every_style_draws() {
    let waveform = Waveform {
        frames_per_bucket: 128,
        buckets: (0..10_000)
            .map(|i| [(i % 256) as u8, 200, 80, 30])
            .collect(),
    };
    let loaded = DeckSnapshot {
        loaded: true,
        playing: true,
        position_frames: 441_000.0,
        duration_frames: 1_280_000,
        track_sample_rate: 44_100,
        ..DeckSnapshot::default()
    };
    for style in [
        WaveformStyle::Frequency,
        WaveformStyle::DeckColour,
        WaveformStyle::Bands,
        WaveformStyle::Overlaid,
    ] {
        draw_styled(loaded, &waveform, style, false);
        draw_styled(loaded, &waveform, style, true);
        for position in [WaveformPosition::Top, WaveformPosition::Middle] {
            for overview in [true, false] {
                let mut frame = ControlFrame::default();
                frame.view.waveform_style = style;
                frame.view.waveform_position = position;
                frame.view.overview = overview;
                let mut screen = ui::Screen::default();
                let mut harness = Harness::builder()
                    .with_size([1440.0, 900.0])
                    .build_ui(|ui| ui::root(ui, &frame, None, &mut screen));
                harness.run_steps(3);
            }
        }
    }
}

/// Every shape `add_contents` paints in one pass, nested ones flattened.
fn painted(add_contents: impl FnMut(&mut egui::Ui)) -> Vec<egui::Shape> {
    let mut add_contents = add_contents;
    let ctx = egui::Context::default();
    let mut output = ctx.run_ui(egui::RawInput::default(), |ui| add_contents(ui));
    output.textures_delta.clear();
    let mut flat = Vec::new();
    for clipped in output.shapes {
        let mut stack = vec![clipped.shape];
        while let Some(shape) = stack.pop() {
            match shape {
                egui::Shape::Vec(inner) => stack.extend(inner),
                other => flat.push(other),
            }
        }
    }
    flat
}

#[test]
fn a_named_hotcue_flag_shows_its_name_beside_the_number() {
    let mut snapshot = DeckSnapshot {
        loaded: true,
        track_sample_rate: 44_100,
        duration_frames: 1_280_000,
        ..DeckSnapshot::default()
    };
    snapshot.hotcues[1] = Some(Hotcue::point(441_000.0));
    let mut marks: [Option<ui::HotcueMark>; 8] = Default::default();
    marks[1] = Some(ui::HotcueMark::new(None, Some("Drop".into())));
    let mut mesh = Arc::new(Mesh::default());
    let shapes = painted(|ui| {
        let (rect, _) = ui.allocate_exact_size(vec2(1440.0, 200.0), Sense::hover());
        let lane = Lane {
            deck: DeckId::new(0).unwrap(),
            snapshot: &snapshot,
            waveform: None,
            position_frames: 441_000.0,
            beats: 16.0,
            style: WaveformStyle::Frequency,
            palette: ui::WaveformPalette::default(),
            overlay: false,
            marks: Some(&marks),
        };
        lane.draw(&ui.painter_at(rect), rect, &mut mesh);
    });
    let texts: Vec<String> = shapes
        .iter()
        .filter_map(|s| match s {
            egui::Shape::Text(t) => Some(t.galley.text().to_owned()),
            _ => None,
        })
        .collect();
    assert!(texts.contains(&"2".to_owned()), "{texts:?}");
    assert!(texts.contains(&"Drop".to_owned()), "{texts:?}");
}

#[test]
fn flag_right_click_opens_the_same_editor() {
    use egui::accesskit::Role;
    use egui::{Event, Modifiers, PointerButton, pos2};
    use egui_kittest::kittest::Queryable;
    use ui::controls::{HeldButtons, Sink};
    use ui::deck::DeckReads;

    // Deck 1 holds track 1 with hotcue 2, "Drop", where the playhead is.
    let mut snapshot = DeckSnapshot {
        loaded: true,
        track_sample_rate: 44_100,
        duration_frames: 1_280_000,
        position_frames: 441_000.0,
        load_seq: 1,
        ..DeckSnapshot::default()
    };
    snapshot.hotcues[1] = Some(Hotcue::point(441_000.0));
    let mut frame = ControlFrame::default();
    let mut marks: [Option<ui::HotcueMark>; 8] = Default::default();
    marks[1] = Some(ui::HotcueMark::new(Some(0xCC0000), Some("Drop".into())));
    frame.decks[0] = Some(ui::DeckInfo {
        load_seq: 1,
        row: library::TrackRow {
            id: 1,
            title: None,
            artist: None,
            bpm: None,
            key: None,
            duration_frames: None,
            sample_rate: None,
            has_cues: true,
            missing: false,
        },
        path: "/music/a.mp3".into(),
        overview: None,
        detail: None,
        stems: None,
        hotcue_marks: Arc::new(marks),
    });
    let mut reads = DeckReads::default();
    reads.decks[0] = (snapshot, 0);
    let (commands, received) = std::sync::mpsc::channel();
    let mut lanes = ui::waveform::Lanes::default();
    let mut held = HeldButtons::default();
    let mut harness = Harness::builder()
        .with_size([1000.0, 200.0])
        .with_step_dt(0.01)
        .build_ui(move |ui| {
            let mut sink = Sink::new(&commands, &mut held);
            lanes.show(ui, &frame, &reads, &mut sink);
            ui::hotcue_menu::show(ui.ctx(), &mut sink);
            held.sweep(ui.ctx(), &commands);
        });
    harness.run_steps(3);
    assert!(harness.query_by_role(Role::TextInput).is_none());
    // Deck 1's lane is the top half; the flag hangs from its top at the playhead.
    let flag = pos2(506.0, 14.0);
    let click = |pressed| Event::PointerButton {
        pos: flag,
        button: PointerButton::Secondary,
        pressed,
        modifiers: Modifiers::NONE,
    };
    harness.event(Event::PointerMoved(flag));
    harness.run_steps(1);
    harness.event(click(true));
    harness.run_steps(1);
    harness.event(click(false));
    harness.run_steps(2);
    let name = harness.get_by_role(Role::TextInput);
    assert_eq!(name.value().as_deref(), Some("Drop"));
    harness.get_by_label("No colour");
    harness.get_by_label("Delete");
    // A swatch saves for the flag's deck, track and slot.
    let (green, colour) = ui::hotcue_menu::SWATCHES[4];
    let swatch = harness.get_by_label(green).rect().center();
    harness.event(Event::PointerMoved(swatch));
    harness.run_steps(1);
    for pressed in [true, false] {
        harness.event(Event::PointerButton {
            pos: swatch,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        });
        harness.run_steps(1);
    }
    let sent: Vec<ui::UiCommand> = received.try_iter().collect();
    assert_eq!(
        sent,
        [ui::UiCommand::Library(ui::LibraryStep::HotcueLabel {
            deck: DeckId::new(0).unwrap(),
            track: 1,
            slot: engine::HotcueSlot::new(2).unwrap(),
            name: Some("Drop".into()),
            color: colour,
        })]
    );
}

/// Deck 1 loaded in the top lane of a 1000-point-wide window, which sends to the receiver
/// returned; the second number is the lane's width.
fn scrubbing(
    loaded: bool,
) -> (
    Harness<'static>,
    std::sync::mpsc::Receiver<ui::UiCommand>,
    std::rc::Rc<std::cell::Cell<f32>>,
) {
    use ui::controls::{HeldButtons, Sink};
    use ui::deck::DeckReads;

    let snapshot = DeckSnapshot {
        loaded,
        track_sample_rate: 44_100,
        duration_frames: 1_280_000,
        position_frames: 441_000.0,
        load_seq: 1,
        ..DeckSnapshot::default()
    };
    let frame = ControlFrame::default();
    let mut reads = DeckReads::default();
    reads.decks[0] = (snapshot, 0);
    let (commands, received) = std::sync::mpsc::channel();
    let width = std::rc::Rc::new(std::cell::Cell::new(0.0));
    let seen = std::rc::Rc::clone(&width);
    let mut lanes = ui::waveform::Lanes::default();
    let mut held = HeldButtons::default();
    let mut harness = Harness::builder()
        .with_size([1000.0, 200.0])
        .with_step_dt(0.01)
        .build_ui(move |ui| {
            seen.set(ui.available_width());
            let mut sink = Sink::new(&commands, &mut held);
            lanes.show(ui, &frame, &reads, &mut sink);
            held.sweep(ui.ctx(), &commands);
        });
    harness.run_steps(3);
    (harness, received, width)
}

fn left_button(pos: egui::Pos2, pressed: bool) -> egui::Event {
    egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    }
}

#[test]
fn lane_drag_follows_the_pointer() {
    use egui::{Event, pos2};
    use ui::UiCommand;

    let (mut harness, received, width) = scrubbing(true);
    let start = pos2(600.0, 50.0);
    harness.event(Event::PointerMoved(start));
    harness.run_steps(1);
    harness.event(left_button(start, true));
    harness.run_steps(2);
    // 100 points to the left, ten a frame.
    for i in 1..=10 {
        harness.event(Event::PointerMoved(pos2(600.0 - 10.0 * i as f32, 50.0)));
        harness.run_steps(1);
    }
    harness.event(left_button(pos2(500.0, 50.0), false));
    harness.run_steps(2);
    let sent: Vec<UiCommand> = received.try_iter().collect();
    let deck = DeckId::new(0).unwrap();
    assert_eq!(
        sent.first(),
        Some(&UiCommand::Action(
            actions::Action::JogTouch(deck),
            actions::Input::Press
        ))
    );
    assert_eq!(
        sent.last(),
        Some(&UiCommand::Action(
            actions::Action::JogTouch(deck),
            actions::Input::Release
        ))
    );
    let steps: i32 = sent
        .iter()
        .filter_map(|c| match c {
            UiCommand::Action(actions::Action::JogScratch(d), actions::Input::Relative(s))
                if *d == deck =>
            {
                Some(*s)
            }
            _ => None,
        })
        .sum();
    // 100 points of 16 beats of 0.5 s over the lane's width, in steps of 1.8 s / 720.
    let frames_per_point = 16.0 * 0.5 * 44_100.0 / f64::from(width.get());
    let frames_per_step = 1.8 * 44_100.0 / 720.0;
    let expected = 100.0 * frames_per_point / frames_per_step;
    assert!(steps > 0, "a drag to the left goes forward: {steps}");
    assert!(
        (f64::from(steps) - expected).abs() <= 1.0,
        "{steps} steps, expected {expected}"
    );
}

#[test]
fn a_drag_on_an_empty_lane_sends_nothing() {
    use egui::{Event, pos2};

    let (mut harness, received, _) = scrubbing(false);
    let start = pos2(600.0, 50.0);
    harness.event(Event::PointerMoved(start));
    harness.run_steps(1);
    harness.event(left_button(start, true));
    harness.run_steps(2);
    harness.event(Event::PointerMoved(pos2(500.0, 50.0)));
    harness.run_steps(1);
    harness.event(left_button(pos2(500.0, 50.0), false));
    harness.run_steps(2);
    let sent: Vec<ui::UiCommand> = received.try_iter().collect();
    assert!(sent.is_empty(), "{sent:?}");
}
