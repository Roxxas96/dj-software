//! The deck panel draws headless, without a GPU.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

use actions::{Action, Input, PadMode, Side};
use analysis::Waveform;

use egui::{Event, Modifiers, PointerButton, Pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use egui_kittest::kittest::{AccessKitNode, NodeT};
use engine::{ChannelSnapshot, DeckId, DeckSnapshot, EffectKind, Grid, SamplerSnapshot};
use library::TrackRow;
use ui::controls::{HeldButtons, Sink};
use ui::cover::Covers;
use ui::deck::{DeckPanels, DeckReads, DeckView};
use ui::layout::sections;
use ui::{ControlFrame, DeckInfo, JogDisplay, Layout, TimeMode, UiCommand, ViewChange, ViewState};

/// A 5:29 track at 126 BPM on deck 1, 57.1 s in.
fn loaded() -> (DeckSnapshot, ControlFrame) {
    let snapshot = DeckSnapshot {
        loaded: true,
        playing: true,
        position_frames: 2_518_110.0,
        duration_frames: 14_508_900,
        track_sample_rate: 44_100,
        grid: Grid::from_bpm(126.0, 0.0, 44_100),
        load_seq: 1,
        ..DeckSnapshot::default()
    };
    let mut frame = ControlFrame::default();
    frame.decks[0] = Some(DeckInfo {
        load_seq: 1,
        row: TrackRow {
            id: 1,
            title: Some("Just Us".into()),
            artist: Some("Someone".into()),
            bpm: Some(126.0),
            key: Some(21),
            duration_frames: Some(14_508_900),
            sample_rate: Some(44_100),
            has_cues: false,
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

fn draw(view: &DeckView) -> Harness<'_> {
    draw_in(view, Layout::Pro)
}

fn draw_in(view: &DeckView, layout: Layout) -> Harness<'_> {
    let sections = sections(layout);
    let mut harness = Harness::builder()
        .with_size([560.0, 340.0])
        .build_ui(move |ui| {
            ui::deck::panel(
                ui,
                view,
                None,
                &sections,
                &mut Sink::new(&channel().0, &mut HeldButtons::default()),
            )
        });
    harness.run_steps(3);
    harness
}

fn view(snapshot: &DeckSnapshot, frame: &ControlFrame) -> DeckView {
    DeckView::new(
        DeckId::new(0).unwrap(),
        snapshot,
        &ChannelSnapshot::default(),
        frame,
        &SamplerSnapshot::default(),
    )
}

/// [`loaded`] with the plain jog, so the BPM and the times show only in the header.
fn loaded_plain() -> (DeckSnapshot, ControlFrame) {
    let (snapshot, mut frame) = loaded();
    frame.view.jog_display = JogDisplay::Plain;
    (snapshot, frame)
}

#[test]
fn deck_panel_shows_title_and_bpm() {
    let (snapshot, frame) = loaded_plain();
    let view = view(&snapshot, &frame);
    let harness = draw(&view);
    harness.get_by_label("Just Us");
    harness.get_by_label("126.00");
    // The time cycles on a click; a deck starts on the time remaining.
    harness.get_by_label("−4:31.9");
    harness.get_by_label("0:57.1");
    harness.get_by_value("pads");
}

#[test]
fn the_time_shows_its_mode() {
    let (snapshot, mut frame) = loaded_plain();
    // The mode's time, then the other of elapsed and remaining beside it.
    for (mode, labels) in [
        (TimeMode::Remaining, ["−4:31.9", "0:57.1"]),
        (TimeMode::Elapsed, ["0:57.1", "−4:31.9"]),
        (TimeMode::Total, ["5:29.0", "0:57.1"]),
    ] {
        frame.view.time_mode[0] = mode;
        let view = view(&snapshot, &frame);
        let harness = draw(&view);
        for label in labels {
            harness.get_by_label(label);
        }
    }
}

#[test]
fn the_beat_counter_shows_when_asked() {
    let (snapshot, mut frame) = loaded();
    assert!(view(&snapshot, &frame).beat_counter.is_none());
    frame.view.beat_counter = true;
    // 57.1 s at 126 BPM is beat 119 from the first: bar 30, beat 4.
    let view = view(&snapshot, &frame);
    assert_eq!(view.beat_counter.as_deref(), Some("30.4"));
    draw(&view).get_by_label("30.4");
}

#[test]
fn info_jog_shows_the_bpm() {
    let (snapshot, mut frame) = loaded_plain();
    let plain = view(&snapshot, &frame);
    assert_eq!(draw(&plain).query_all_by_label("126.00").count(), 1);
    frame.view.jog_display = JogDisplay::Info;
    let info = view(&snapshot, &frame);
    // In the header and in the jog.
    assert_eq!(draw(&info).query_all_by_label("126.00").count(), 2);
}

#[test]
fn clicking_the_time_cycles_it() {
    let (snapshot, mut frame) = loaded_plain();
    frame.view.time_mode[0] = TimeMode::Elapsed;
    let mut reads = DeckReads::default();
    reads.decks[0].0 = snapshot;
    let (commands, received) = channel();
    let mut panels = DeckPanels::default();
    let mut covers = Covers::default();
    let sections = sections(Layout::Pro);
    let mut harness = Harness::builder()
        .with_size([1440.0, 400.0])
        .build_ui(move |ui| {
            panels.show_reads(
                ui,
                &frame,
                &reads,
                &mut covers,
                &sections,
                &mut Sink::new(&commands, &mut HeldButtons::default()),
            );
        });
    harness.run_steps(3);
    harness.get_by_label_contains("0:5").click();
    harness.run_steps(2);
    let deck = DeckId::new(0).unwrap();
    assert_eq!(
        received.try_recv().ok(),
        Some(UiCommand::View(ViewChange::CycleTime(deck)))
    );
}

#[test]
fn deck_panel_draws_the_overview_stems_and_jump_pads() {
    let (snapshot, mut frame) = loaded();
    let info = frame.decks[0].as_mut().unwrap();
    info.stems = Some("stems 40%".into());
    info.overview = Some(Arc::new(Waveform {
        frames_per_bucket: 1024,
        buckets: (0..14_169)
            .map(|i| [(i % 256) as u8, 200, 80, 30])
            .collect(),
    }));
    frame.control.sides[0].pad_mode = PadMode::BeatJump;
    let view = view(&snapshot, &frame);
    let harness = draw(&view);
    harness.get_by_label("stems 40%");
    harness.get_by_value("BEAT JUMP");
    harness.get_by_label("−4");
    harness.get_by_label("+32");
}

#[test]
fn a_named_hotcue_pad_shows_its_name_in_the_window() {
    let (mut snapshot, mut frame) = loaded();
    snapshot.hotcues[1] = Some(engine::Hotcue::point(1000.0));
    snapshot.hotcues[3] = Some(engine::Hotcue {
        frames: 2000.0,
        loop_frames: Some(500.0),
    });
    Arc::make_mut(&mut frame.decks[0].as_mut().unwrap().hotcue_marks)[1] =
        Some(ui::HotcueMark::new(Some(0xff0000), Some("Drop".into())));
    let view = view(&snapshot, &frame);
    let harness = draw(&view);
    harness.get_by_label_contains("Drop");
    harness.get_by_label_contains("4 loop");
}

#[test]
fn starter_hides_the_pads() {
    let (snapshot, frame) = loaded();
    let view = view(&snapshot, &frame);
    draw_in(&view, Layout::Pro).get_by_value("pads");
    let starter = draw_in(&view, Layout::Starter);
    assert!(starter.query_by_value("pads").is_none());
    starter.get_by_label("LOOP");
    let performance = draw_in(&view, Layout::Performance);
    performance.get_by_value("pads");
}

/// Draws `view` in the pro layout, sending what it asks for to `commands`.
fn draw_sending(view: DeckView, commands: Sender<UiCommand>) -> Harness<'static> {
    let sections = sections(Layout::Pro);
    let mut harness = Harness::builder()
        .with_size([560.0, 340.0])
        .build_ui(move |ui| {
            ui::deck::panel(
                ui,
                &view,
                None,
                &sections,
                &mut Sink::new(&commands, &mut HeldButtons::default()),
            )
        });
    harness.run_steps(3);
    harness
}

/// The entries of the open dropdown's list, in order.
fn dropdown_entries(harness: &Harness<'_>) -> Vec<String> {
    let list = harness.get_by_role(egui::accesskit::Role::ListBox);
    let mut entries = Vec::new();
    collect_labels(list.accesskit_node(), &mut entries);
    entries
}

/// The first label down each branch under `node`.
fn collect_labels(node: AccessKitNode<'_>, out: &mut Vec<String>) {
    for child in node.children() {
        match child.label().filter(|label| !label.is_empty()) {
            Some(label) => out.push(label),
            None => collect_labels(child, out),
        }
    }
}

#[test]
fn the_pad_dropdown_offers_only_software_modes() {
    let (snapshot, frame) = loaded();
    let (commands, received) = channel();
    let mut harness = draw_sending(view(&snapshot, &frame), commands);
    harness.get_by_value("pads").click();
    harness.run_steps(2);
    assert_eq!(
        dropdown_entries(&harness),
        [
            "pads (controller)",
            "BEAT JUMP",
            "LOOP ROLL",
            "SLICER",
            "PAD SCRATCH"
        ]
    );
    for hardware in ["HOT CUE", "SAMPLER", "FX FADE"] {
        assert!(harness.query_by_label(hardware).is_none(), "{hardware}");
    }
    harness.get_by_label("SLICER").click();
    harness.run_steps(2);
    assert_eq!(
        received.try_recv().ok(),
        Some(UiCommand::Action(
            Action::PadMode(Side::Left, PadMode::Slicer),
            Input::Press
        ))
    );
}

#[test]
fn the_pad_dropdown_shows_the_last_hardware_button() {
    let (snapshot, mut frame) = loaded();
    frame.control.sides[0].hardware_mode = Some(actions::HardwarePadMode::Sampler);
    let harness = draw_sending(view(&snapshot, &frame), channel().0);
    harness.get_by_value("SAMPLER");
}

#[test]
fn an_fx_dropdown_offers_every_effect() {
    let (snapshot, mut frame) = loaded();
    frame.fx_slots[0] = [EffectKind::Echo, EffectKind::Reverb, EffectKind::Echo];
    let (commands, received) = channel();
    let mut harness = draw_sending(view(&snapshot, &frame), commands);
    // The slot's arrow opens the list.
    harness.get_by_label("FX 2").click();
    harness.run_steps(2);
    assert_eq!(
        dropdown_entries(&harness),
        ["Echo", "Reverb", "Flanger", "Filter"]
    );
    harness.get_by_label("Filter").click();
    harness.run_steps(2);
    assert_eq!(
        received.try_recv().ok(),
        Some(UiCommand::FxSlot {
            deck: DeckId::new(0).unwrap(),
            slot: 1,
            kind: EffectKind::Filter,
        })
    );
}

#[test]
fn a_coloured_hotcue_pad_is_filled_with_its_colour() {
    let (mut snapshot, mut frame) = loaded();
    snapshot.hotcues[0] = Some(engine::Hotcue::point(1000.0));
    snapshot.hotcues[1] = Some(engine::Hotcue::point(2000.0));
    Arc::make_mut(&mut frame.decks[0].as_mut().unwrap().hotcue_marks)[0] =
        Some(ui::HotcueMark::new(Some(0xff0000), None));
    let view = view(&snapshot, &frame);
    let sections = sections(Layout::Pro);
    let ctx = egui::Context::default();
    let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
        ui.set_min_size(egui::vec2(560.0, 340.0));
        ui::deck::panel(
            ui,
            &view,
            None,
            &sections,
            &mut Sink::new(&channel().0, &mut HeldButtons::default()),
        );
    });
    output.textures_delta.clear();
    let mut fills = Vec::new();
    let mut stack: Vec<egui::Shape> = output.shapes.into_iter().map(|c| c.shape).collect();
    while let Some(shape) = stack.pop() {
        match shape {
            egui::Shape::Vec(inner) => stack.extend(inner),
            egui::Shape::Rect(rect) => fills.push(rect.fill),
            _ => {}
        }
    }
    let red = egui::Color32::from_rgb(0xff, 0, 0);
    assert_eq!(fills.iter().filter(|f| **f == red).count(), 1, "{fills:?}");
}

#[test]
fn jog_shows_info_by_default() {
    let (snapshot, mut frame) = loaded();
    frame.view = ViewState::default();
    let view = view(&snapshot, &frame);
    // In the header and in the jog.
    assert_eq!(draw(&view).query_all_by_label("126.00").count(), 2);
}

#[test]
fn edge_labels_name_the_sections() {
    let (snapshot, frame) = loaded();
    let view = view(&snapshot, &frame);
    let harness = draw(&view);
    for label in ["FX", "PADS", "LOOP"] {
        harness.get_by_label(label);
    }
}

#[test]
fn overview_sits_in_the_header() {
    let (snapshot, mut frame) = loaded();
    frame.decks[0].as_mut().unwrap().overview = Some(Arc::new(Waveform {
        frames_per_bucket: 1024,
        buckets: vec![[200, 200, 80, 30]; 14_169],
    }));
    let view = view(&snapshot, &frame);
    let harness = draw(&view);
    let cover = harness.get_by_label("cover").rect();
    let overview = harness.get_by_label("overview").rect();
    let title = harness.get_by_label("Just Us").rect();
    // The header is the cover's height, across the panel.
    assert!(overview.min.y >= cover.min.y && overview.max.y <= cover.max.y + 0.5);
    assert!(overview.min.x >= cover.max.x, "{overview:?} {cover:?}");
    // Under the title.
    assert!(overview.min.y >= title.max.y, "{overview:?} {title:?}");
    // The FX row starts below the header.
    let fx = harness.get_by_label("FX 1").rect();
    assert!(fx.min.y >= cover.max.y, "{fx:?} {cover:?}");
}

#[test]
fn empty_decks_keep_their_height() {
    let (snapshot, frame) = loaded();
    let full = view(&snapshot, &frame);
    let empty = view(&DeckSnapshot::default(), &ControlFrame::default());
    let bottom = |view: &DeckView| {
        let harness = draw(view);
        ["SYNC", "OUT", "8"].map(|label| harness.get_by_label(label).rect().max.y)
    };
    assert_eq!(bottom(&full), bottom(&empty));
    let sections = sections(Layout::Pro);
    assert_eq!(
        ui::deck::row_height(&sections),
        ui::deck::panel_height(&sections) + ui::theme::SECTION_GAP
    );
}

/// Every filled rect and every rect outline drawn for the two decks of `frame`.
fn deck_rects(frame: &ControlFrame) -> Vec<egui::epaint::RectShape> {
    let mut reads = DeckReads::default();
    reads.decks[0].0 = loaded().0;
    let sections = sections(Layout::Pro);
    let mut panels = DeckPanels::default();
    let mut covers = Covers::default();
    let ctx = egui::Context::default();
    let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
        ui.set_min_size(egui::vec2(1440.0, 400.0));
        panels.show_reads(
            ui,
            frame,
            &reads,
            &mut covers,
            &sections,
            &mut Sink::new(&channel().0, &mut HeldButtons::default()),
        );
    });
    output.textures_delta.clear();
    let mut rects = Vec::new();
    let mut stack: Vec<egui::Shape> = output.shapes.into_iter().map(|c| c.shape).collect();
    while let Some(shape) = stack.pop() {
        match shape {
            egui::Shape::Vec(inner) => stack.extend(inner),
            egui::Shape::Rect(rect) => rects.push(rect),
            _ => {}
        }
    }
    rects
}

#[test]
fn active_deck_has_an_accent_tab_not_a_border() {
    let (_, frame) = loaded();
    let rects = deck_rects(&frame);
    let p = ui::theme::Palette::DARK;
    let outlined: Vec<_> = rects
        .iter()
        .filter(|r| r.stroke.width > 0.0 && r.stroke.color.a() > 0)
        .collect();
    assert!(outlined.is_empty(), "{outlined:?}");
    let left = rects.iter().map(|r| r.rect.left()).fold(f32::MAX, f32::min);
    let right = rects
        .iter()
        .map(|r| r.rect.right())
        .fold(f32::MIN, f32::max);
    // Deck 1's tab on the left edge, deck 2's on the right, each 4 points wide.
    let tab = |accent: egui::Color32, edge: f32, of: fn(&egui::Rect) -> f32| {
        rects.iter().any(|r| {
            r.fill == accent
                && (r.rect.width() - 4.0).abs() < 0.01
                && (of(&r.rect) - edge).abs() < 0.5
        })
    };
    assert!(tab(p.accent_left, left, |r| r.left()), "no tab on deck 1");
    assert!(
        tab(p.accent_right, right, |r| r.right()),
        "no tab on deck 2"
    );
}

#[test]
fn the_right_deck_is_mirrored() {
    let (snapshot, mut frame) = loaded();
    // Deck 2 holds the track: the right side's deck.
    frame.decks[1] = frame.decks[0].take();
    let deck = DeckId::new(1).unwrap();
    let view = DeckView::new(
        deck,
        &snapshot,
        &ChannelSnapshot::default(),
        &frame,
        &SamplerSnapshot::default(),
    );
    let harness = draw(&view);
    // The jog's BPM is the lower of the two.
    let jog = harness
        .get_all_by_label("126.00")
        .map(|n| n.rect())
        .max_by(|a, b| a.min.y.total_cmp(&b.min.y))
        .unwrap();
    let fx = harness.get_by_label("FX 1").rect();
    assert!(
        fx.min.x > jog.max.x,
        "FX {fx:?} not right of the jog {jog:?}"
    );
    let label = harness.get_by_label("PADS").rect();
    let pad = harness.get_by_label("8").rect();
    assert!(
        label.min.x > pad.max.x,
        "PADS {label:?} not right of pad 8 {pad:?}"
    );
    let fx_label = harness.get_by_label("FX").rect();
    assert!(fx_label.min.x > fx.max.x, "FX label {fx_label:?} {fx:?}");
}

#[test]
fn the_right_decks_are_mirrored_in_four_decks() {
    let (snapshot, frame) = loaded();
    let sections = ui::layout::four_deck_sections();
    for (number, mirrored) in [(1, false), (2, true), (3, false), (4, true)] {
        let view = DeckView::new(
            DeckId::new(number - 1).unwrap(),
            &snapshot,
            &ChannelSnapshot::default(),
            &frame,
            &SamplerSnapshot::default(),
        );
        let mut harness = Harness::builder()
            .with_size([480.0, 120.0])
            .build_ui(move |ui| {
                ui::deck::panel(
                    ui,
                    &view,
                    None,
                    &sections,
                    &mut Sink::new(&channel().0, &mut HeldButtons::default()),
                )
            });
        harness.run_steps(3);
        let cue = harness.get_by_label("CUE").rect();
        let out = harness.get_by_label("OUT").rect();
        assert_eq!(cue.min.x > out.max.x, mirrored, "deck {number}");
    }
}

/// What a panel sending to a channel keeps: the view, whether it is drawn, the buttons
/// held and where the commands go.
struct Sending {
    view: DeckView,
    drawn: bool,
    held: HeldButtons,
    commands: Sender<UiCommand>,
}

/// Draws `view` in the pro layout, sending what it asks for to the receiver returned and
/// sweeping the held buttons at the end of each frame, as the window does. Short steps,
/// so two clicks in a row make a double-click.
fn sending(view: DeckView) -> (Harness<'static, Sending>, Receiver<UiCommand>) {
    let (commands, received) = channel();
    let sections = sections(Layout::Pro);
    let state = Sending {
        view,
        drawn: true,
        held: HeldButtons::default(),
        commands,
    };
    let mut harness = Harness::builder()
        .with_size([560.0, 340.0])
        .with_step_dt(0.01)
        .build_ui_state(
            move |ui, state: &mut Sending| {
                if state.drawn {
                    let mut sink = Sink::new(&state.commands, &mut state.held);
                    ui::deck::panel(ui, &state.view, None, &sections, &mut sink);
                }
                state.held.sweep(ui.ctx(), &state.commands);
            },
            state,
        );
    harness.run_steps(3);
    (harness, received)
}

fn pointer(pressed: bool, at: Pos2) -> Event {
    Event::PointerButton {
        pos: at,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    }
}

/// Presses at `at`, holds for two frames and lets go.
fn press_and_release<S>(harness: &mut Harness<'_, S>, at: Pos2) {
    harness.event(Event::PointerMoved(at));
    harness.run_steps(1);
    harness.event(pointer(true, at));
    harness.run_steps(2);
    harness.event(pointer(false, at));
    harness.run_steps(2);
}

/// Presses and lets go of the control labelled `label`.
fn press_label<S>(harness: &mut Harness<'_, S>, label: &str) {
    let at = harness.get_by_label(label).rect().center();
    press_and_release(harness, at);
}

fn press_release(action: Action) -> [UiCommand; 2] {
    [
        UiCommand::Action(action, Input::Press),
        UiCommand::Action(action, Input::Release),
    ]
}

#[test]
fn play_cue_sync_and_states_send_their_actions() {
    for number in [1, 2] {
        let (snapshot, mut frame) = loaded_plain();
        let id = DeckId::new(number - 1).unwrap();
        frame.decks.swap(0, id.index());
        let view = DeckView::new(
            id,
            &snapshot,
            &ChannelSnapshot::default(),
            &frame,
            &SamplerSnapshot::default(),
        );
        let (mut harness, received) = sending(view);
        for (label, action) in [
            ("PLAY", Action::PlayPause(id)),
            ("CUE", Action::Cue(id)),
            ("SYNC", Action::Sync(id)),
            ("VINYL", Action::VinylToggle(id)),
            ("SLIP", Action::SlipToggle(id)),
            ("KEYLOCK", Action::KeylockToggle(id)),
            ("IN", Action::LoopIn(id)),
            ("OUT", Action::LoopOut(id)),
        ] {
            press_label(&mut harness, label);
            let sent: Vec<UiCommand> = received.try_iter().collect();
            assert_eq!(sent, press_release(action), "deck {number} {label}");
        }
    }
}

#[test]
fn a_held_cue_no_longer_drawn_is_released() {
    let (snapshot, frame) = loaded_plain();
    let (mut harness, received) = sending(view(&snapshot, &frame));
    let cue = harness.get_by_label("CUE").rect().center();
    harness.event(Event::PointerMoved(cue));
    harness.run_steps(1);
    harness.event(pointer(true, cue));
    harness.run_steps(2);
    harness.state_mut().drawn = false;
    harness.run_steps(1);
    let sent: Vec<UiCommand> = received.try_iter().collect();
    assert_eq!(sent, press_release(Action::Cue(DeckId::new(0).unwrap())));
}

#[test]
fn double_click_resets_pitch_to_zero() {
    let (snapshot, mut frame) = loaded_plain();
    frame.control.decks[0].tempo = 0.8;
    let (mut harness, received) = sending(view(&snapshot, &frame));
    // The slider is over its pitch in percent.
    let label = harness.get_by_label("+0.0%").rect();
    let at = label.center_top() - vec2(0.0, 30.0);
    harness.event(Event::PointerMoved(at));
    harness.run_steps(1);
    for _ in 0..2 {
        harness.event(pointer(true, at));
        harness.event(pointer(false, at));
        harness.run_steps(1);
    }
    harness.run_steps(1);
    let tempo = Action::Tempo(DeckId::new(0).unwrap());
    let last = received
        .try_iter()
        .filter(|c| matches!(c, UiCommand::Action(a, _) if *a == tempo))
        .last();
    assert_eq!(last, Some(UiCommand::Action(tempo, Input::Absolute(0.5))));
}

#[test]
fn loop_arrows_halve_and_double() {
    let (mut snapshot, frame) = loaded_plain();
    snapshot.loop_beats = 0.5;
    let (mut harness, received) = sending(view(&snapshot, &frame));
    let id = DeckId::new(0).unwrap();
    for (label, action) in [
        ("<", Action::LoopHalve(id)),
        (">", Action::LoopDouble(id)),
        ("1/2", Action::AutoLoop(id)),
    ] {
        press_label(&mut harness, label);
        let sent: Vec<UiCommand> = received.try_iter().collect();
        assert_eq!(sent, press_release(action), "{label}");
    }
}

#[test]
fn pad_mode_arrows_step_and_wrap() {
    for (mode, previous, next) in [
        (PadMode::Hardware, PadMode::PadScratch, PadMode::BeatJump),
        (PadMode::PadScratch, PadMode::Slicer, PadMode::Hardware),
    ] {
        let (snapshot, mut frame) = loaded_plain();
        frame.control.sides[0].pad_mode = mode;
        let (mut harness, received) = sending(view(&snapshot, &frame));
        for (label, to) in [("previous pad mode", previous), ("next pad mode", next)] {
            press_label(&mut harness, label);
            let sent: Vec<UiCommand> = received.try_iter().collect();
            let action = Action::PadMode(Side::Left, to);
            assert_eq!(sent, press_release(action), "{mode:?} {label}");
        }
    }
}

#[test]
fn fx_toggle_and_level() {
    let (snapshot, frame) = loaded_plain();
    let (mut harness, received) = sending(view(&snapshot, &frame));
    let id = DeckId::new(0).unwrap();
    // The slot's effect toggles it; its arrow opens the list.
    press_label(&mut harness, "Reverb");
    let sent: Vec<UiCommand> = received.try_iter().collect();
    assert_eq!(sent, press_release(Action::FxToggle(id, 1)));
    // The level is the bar over its caption: a click three quarters along sets 0.75.
    let caption = harness.get_by_label("FX 50%").rect();
    let bar = caption.center_top() - vec2(0.0, 6.0);
    let at = bar + vec2(caption.width() / 4.0, 0.0);
    press_and_release(&mut harness, at);
    let level = received
        .try_iter()
        .filter_map(|c| match c {
            UiCommand::Action(Action::FxLevel(d), Input::Absolute(v)) if d == id => Some(v),
            _ => None,
        })
        .last()
        .expect("no FxLevel");
    assert!((level - 0.75).abs() < 0.03, "got {level}");
}

/// The middle of pad `number` (1 to 8) as `view` shows it, found by its text.
fn pad_at<S>(harness: &Harness<'_, S>, view: &DeckView, number: usize) -> Pos2 {
    let label = view.pads[number - 1].label.as_str();
    harness
        .get_all_by_label(label)
        .last()
        .unwrap_or_else(|| panic!("no pad {number} labelled {label:?}"))
        .rect()
        .center()
}

fn slot(number: usize) -> engine::HotcueSlot {
    engine::HotcueSlot::ALL[number - 1]
}

#[test]
fn clicking_a_hotcue_pad_sends_press_and_release() {
    for number in [1, 2] {
        let (snapshot, mut frame) = loaded_plain();
        let id = DeckId::new(number - 1).unwrap();
        frame.decks.swap(0, id.index());
        let view = DeckView::new(
            id,
            &snapshot,
            &ChannelSnapshot::default(),
            &frame,
            &SamplerSnapshot::default(),
        );
        let (mut harness, received) = sending(view.clone());
        let at = pad_at(&harness, &view, 3);
        press_and_release(&mut harness, at);
        let sent: Vec<UiCommand> = received.try_iter().collect();
        assert_eq!(
            sent,
            press_release(Action::Hotcue(id, slot(3))),
            "deck {number}"
        );
    }
}

#[test]
fn loop_roll_pad_holds_while_pressed() {
    let (snapshot, mut frame) = loaded_plain();
    frame.control.sides[0].pad_mode = PadMode::LoopRoll;
    let view = view(&snapshot, &frame);
    let (mut harness, received) = sending(view.clone());
    let at = pad_at(&harness, &view, 3);
    let roll = Action::LoopRoll(DeckId::new(0).unwrap(), slot(3));
    harness.event(Event::PointerMoved(at));
    harness.run_steps(1);
    harness.event(pointer(true, at));
    harness.run_steps(4);
    let sent: Vec<UiCommand> = received.try_iter().collect();
    assert_eq!(sent, [UiCommand::Action(roll, Input::Press)]);
    harness.event(pointer(false, at));
    harness.run_steps(2);
    let sent: Vec<UiCommand> = received.try_iter().collect();
    assert_eq!(sent, [UiCommand::Action(roll, Input::Release)]);
}

#[test]
fn a_mode_change_mid_press_releases_the_pressed_action() {
    let (snapshot, mut frame) = loaded_plain();
    frame.control.sides[0].pad_mode = PadMode::LoopRoll;
    let first = view(&snapshot, &frame);
    let (mut harness, received) = sending(first.clone());
    let at = pad_at(&harness, &first, 3);
    harness.event(Event::PointerMoved(at));
    harness.run_steps(1);
    harness.event(pointer(true, at));
    harness.run_steps(2);
    frame.control.sides[0].pad_mode = PadMode::Slicer;
    harness.state_mut().view = view(&snapshot, &frame);
    harness.run_steps(2);
    harness.event(pointer(false, at));
    harness.run_steps(2);
    let sent: Vec<UiCommand> = received.try_iter().collect();
    let roll = Action::LoopRoll(DeckId::new(0).unwrap(), slot(3));
    assert_eq!(sent, press_release(roll));
}

#[test]
fn shift_click_in_beat_jump_mode_sends_beat_jump_shift() {
    let (snapshot, mut frame) = loaded_plain();
    frame.control.sides[0].pad_mode = PadMode::BeatJump;
    let view = view(&snapshot, &frame);
    let (mut harness, received) = sending(view.clone());
    let at = pad_at(&harness, &view, 3);
    harness.event(Event::PointerMoved(at));
    harness.run_steps(1);
    harness.event(Event::ModifiersChanged(Modifiers::SHIFT));
    harness.event(Event::PointerButton {
        pos: at,
        button: PointerButton::Primary,
        pressed: true,
        modifiers: Modifiers::SHIFT,
    });
    harness.run_steps(2);
    // The key coming up before the mouse does not change what was pressed.
    harness.event(Event::ModifiersChanged(Modifiers::NONE));
    harness.event(pointer(false, at));
    harness.run_steps(2);
    let id = DeckId::new(0).unwrap();
    let sent: Vec<UiCommand> = received.try_iter().collect();
    assert_eq!(sent, press_release(Action::BeatJumpShift(id, slot(3))));
    press_and_release(&mut harness, at);
    let sent: Vec<UiCommand> = received.try_iter().collect();
    assert_eq!(sent, press_release(Action::BeatJump(id, slot(3))));
}

#[test]
fn slicer_and_sampler_pads_send_their_actions() {
    let (snapshot, mut frame) = loaded_plain();
    frame.control.sides[0].pad_mode = PadMode::Slicer;
    let slicer = view(&snapshot, &frame);
    let (mut harness, received) = sending(slicer.clone());
    let at = pad_at(&harness, &slicer, 5);
    press_and_release(&mut harness, at);
    let sent: Vec<UiCommand> = received.try_iter().collect();
    let slice = Action::Slice(DeckId::new(0).unwrap(), slot(5));
    assert_eq!(sent, press_release(slice));

    frame.control.sides[0].pad_mode = PadMode::Hardware;
    frame.control.sides[0].hardware_mode = Some(actions::HardwarePadMode::Sampler);
    let sampler = view(&snapshot, &frame);
    let (mut harness, received) = sending(sampler.clone());
    let at = pad_at(&harness, &sampler, 2);
    press_and_release(&mut harness, at);
    let sent: Vec<UiCommand> = received.try_iter().collect();
    assert_eq!(sent, press_release(Action::SamplerPlay(slot(2))));
}
