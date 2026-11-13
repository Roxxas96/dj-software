//! The deck panel draws headless, without a GPU.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

use actions::{Action, Input, PadMode, Side};
use analysis::Waveform;

use egui::accesskit::Role;
use egui::{Event, Key, Modifiers, PointerButton, Pos2, pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use egui_kittest::kittest::{AccessKitNode, NodeT};
use engine::{ChannelSnapshot, DeckId, DeckSnapshot, EffectKind, Grid, SamplerSnapshot};
use library::TrackRow;
use ui::controls::{HeldButtons, Sink};
use ui::cover::Covers;
use ui::deck::{DeckPanels, DeckReads, DeckView};
use ui::layout::sections;
use ui::{
    ControlFrame, DeckInfo, JogDisplay, Layout, LibraryStep, TimeMode, UiCommand, ViewChange,
    ViewState,
};

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

/// A short silent WAV at `path` whose ID3 tag holds a 16-pixel red PNG as its front cover.
fn wav_with_cover(path: &std::path::Path) {
    use lofty::config::{ParseOptions, WriteOptions};
    use lofty::file::AudioFile;
    use lofty::id3::v2::{AttachedPictureFrame, Frame, Id3v2Tag};
    use lofty::iff::wav::WavFile;
    use lofty::picture::{MimeType, Picture, PictureType};
    let frames = 441u32;
    let mut bytes = b"RIFF".to_vec();
    bytes.extend_from_slice(&(36 + frames * 2).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&44_100u32.to_le_bytes());
    bytes.extend_from_slice(&88_200u32.to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&(frames * 2).to_le_bytes());
    bytes.resize(bytes.len() + (frames * 2) as usize, 0);
    std::fs::write(path, bytes).unwrap();
    let mut png = Vec::new();
    image::RgbaImage::from_pixel(16, 16, image::Rgba([255, 0, 0, 255]))
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    let mut tag = Id3v2Tag::new();
    tag.insert(Frame::Picture(AttachedPictureFrame::new(
        lofty::TextEncoding::Latin1,
        Picture::unchecked(png)
            .pic_type(PictureType::CoverFront)
            .mime_type(MimeType::Png)
            .build(),
    )));
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .unwrap();
    let mut wav = WavFile::read_from(&mut file, ParseOptions::new()).unwrap();
    wav.set_id3v2(tag);
    wav.save_to(&mut file, WriteOptions::default()).unwrap();
}

#[test]
fn the_deck_header_draws_the_track_s_embedded_cover() {
    let path = std::env::temp_dir().join(format!("open-jockey-cover-{}.wav", std::process::id()));
    wav_with_cover(&path);
    let (snapshot, mut frame) = loaded();
    frame.decks[0].as_mut().unwrap().path = path.clone();
    let mut reads = DeckReads::default();
    reads.decks[0].0 = snapshot;
    let sections = sections(Layout::Pro);
    let mut panels = DeckPanels::default();
    let mut covers = Covers::default();
    let ctx = egui::Context::default();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let drawn = loop {
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            ui.set_min_size(egui::vec2(1440.0, 400.0));
            panels.show_reads(
                ui,
                &frame,
                &reads,
                &mut covers,
                &sections,
                &mut Sink::new(&channel().0, &mut HeldButtons::default()),
            );
        });
        output.textures_delta.clear();
        let mut stack: Vec<egui::Shape> = output.shapes.into_iter().map(|c| c.shape).collect();
        let mut textured = false;
        while let Some(shape) = stack.pop() {
            match shape {
                egui::Shape::Vec(inner) => stack.extend(inner),
                egui::Shape::Mesh(mesh) => {
                    textured |= mesh.texture_id != egui::TextureId::default()
                }
                egui::Shape::Rect(rect) => {
                    textured |= rect.fill_texture_id() != egui::TextureId::default();
                }
                _ => {}
            }
        }
        if textured || std::time::Instant::now() > deadline {
            break textured;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    };
    let _ = std::fs::remove_file(&path);
    assert!(drawn, "the header never drew the cover");
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

/// A trackpad tap at `at`: the press and the release reach the same frame (kittest's
/// queued events take a frame each).
fn tap<S>(harness: &mut Harness<'_, S>, at: Pos2) {
    let events = &mut harness.input_mut().events;
    events.push(pointer(true, at));
    events.push(pointer(false, at));
    harness.step();
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

/// The fills of the rectangles drawn inside `area` in the harness's last frame.
fn fills_in<S>(harness: &Harness<'_, S>, area: egui::Rect) -> Vec<egui::Color32> {
    let mut fills = Vec::new();
    let mut stack: Vec<egui::Shape> = harness
        .output()
        .shapes
        .iter()
        .map(|c| c.shape.clone())
        .collect();
    while let Some(shape) = stack.pop() {
        match shape {
            egui::Shape::Vec(inner) => stack.extend(inner),
            egui::Shape::Rect(rect) if area.expand(0.5).contains_rect(rect.rect) => {
                fills.push(rect.fill);
            }
            _ => {}
        }
    }
    fills
}

#[test]
fn a_held_button_looks_pressed_until_it_is_let_go() {
    let (snapshot, frame) = loaded_plain();
    let (mut harness, _received) = sending(view(&snapshot, &frame));
    let play = harness.get_by_label("PLAY").rect();
    let idle = fills_in(&harness, play);
    assert!(!idle.is_empty(), "PLAY draws no fill");
    harness.event(Event::PointerMoved(play.center()));
    harness.run_steps(1);
    let hovered = fills_in(&harness, play);
    assert_ne!(hovered, idle, "no hover highlight");
    harness.event(pointer(true, play.center()));
    harness.run_steps(2);
    let held = fills_in(&harness, play);
    assert_ne!(held, idle, "held looks idle");
    assert_ne!(held, hovered, "held looks only hovered");
    harness.event(pointer(false, play.center()));
    harness.event(Event::PointerGone);
    harness.run_steps(2);
    assert_eq!(fills_in(&harness, play), idle, "not back to idle");
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
fn right_click_resets_pitch_to_zero() {
    let (snapshot, mut frame) = loaded_plain();
    frame.control.decks[0].tempo = 0.8;
    let (mut harness, received) = sending(view(&snapshot, &frame));
    let label = harness.get_by_label("+0.0%").rect();
    right_click(&mut harness, label.center_top() - vec2(0.0, 30.0));
    let sent: Vec<UiCommand> = received.try_iter().collect();
    let tempo = Action::Tempo(DeckId::new(0).unwrap());
    assert_eq!(sent, [UiCommand::Action(tempo, Input::Absolute(0.5))]);
}

#[test]
fn ctrl_click_on_the_pitch_resets_it_on_macos() {
    let (snapshot, mut frame) = loaded_plain();
    frame.control.decks[0].tempo = 0.8;
    let (mut harness, received) = sending(view(&snapshot, &frame));
    let label = harness.get_by_label("+0.0%").rect();
    let at = label.center_top() - vec2(0.0, 30.0);
    harness.event(Event::PointerMoved(at));
    harness.run_steps(1);
    for pressed in [true, false] {
        let event = Event::PointerButton {
            pos: at,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::CTRL,
        };
        harness.event_modifiers(event, Modifiers::CTRL);
        harness.run_steps(1);
    }
    harness.run_steps(1);
    let sent: Vec<UiCommand> = received.try_iter().collect();
    let tempo = Action::Tempo(DeckId::new(0).unwrap());
    if cfg!(target_os = "macos") {
        assert_eq!(sent, [UiCommand::Action(tempo, Input::Absolute(0.5))]);
    } else {
        assert!(!sent.contains(&UiCommand::Action(tempo, Input::Absolute(0.5))));
    }
}

#[test]
fn right_click_on_the_fx_level_sends_nothing() {
    let (snapshot, frame) = loaded_plain();
    let (mut harness, received) = sending(view(&snapshot, &frame));
    let at = harness.get_by_label("FX 50%").rect().center_top() - vec2(0.0, 6.0);
    right_click(&mut harness, at);
    let sent: Vec<UiCommand> = received.try_iter().collect();
    assert_eq!(sent, []);
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

    for hardware in [
        actions::HardwarePadMode::Sampler,
        actions::HardwarePadMode::Trans,
    ] {
        frame.control.sides[0].pad_mode = PadMode::Hardware;
        frame.control.sides[0].hardware_mode = Some(hardware);
        let sampler = view(&snapshot, &frame);
        let (mut harness, received) = sending(sampler.clone());
        let at = pad_at(&harness, &sampler, 2);
        press_and_release(&mut harness, at);
        let sent: Vec<UiCommand> = received.try_iter().collect();
        assert_eq!(
            sent,
            press_release(Action::SamplerPlay(slot(2))),
            "{hardware:?}"
        );
    }
}

/// [`sending`] with the hotcue editor drawn after the panel, as the window draws it.
fn editing(view: DeckView) -> (Harness<'static, Sending>, Receiver<UiCommand>) {
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
                let mut sink = Sink::new(&state.commands, &mut state.held);
                ui::deck::panel(ui, &state.view, None, &sections, &mut sink);
                ui::hotcue_menu::show(ui.ctx(), &mut sink);
                state.held.sweep(ui.ctx(), &state.commands);
            },
            state,
        );
    harness.run_steps(3);
    (harness, received)
}

fn secondary(pressed: bool, at: Pos2) -> Event {
    Event::PointerButton {
        pos: at,
        button: PointerButton::Secondary,
        pressed,
        modifiers: Modifiers::NONE,
    }
}

/// Right-clicks at `at`.
fn right_click<S>(harness: &mut Harness<'_, S>, at: Pos2) {
    harness.event(Event::PointerMoved(at));
    harness.run_steps(1);
    harness.event(secondary(true, at));
    harness.run_steps(1);
    harness.event(secondary(false, at));
    harness.run_steps(2);
}

/// Deck 1 with hotcue 3 set, red and named "Drop".
fn with_drop() -> DeckView {
    let (mut snapshot, mut frame) = loaded_plain();
    snapshot.hotcues[2] = Some(engine::Hotcue::point(1000.0));
    Arc::make_mut(&mut frame.decks[0].as_mut().unwrap().hotcue_marks)[2] =
        Some(ui::HotcueMark::new(Some(0xCC0000), Some("Drop".into())));
    view(&snapshot, &frame)
}

fn label_step(name: Option<&str>, color: Option<u32>) -> UiCommand {
    UiCommand::Library(LibraryStep::HotcueLabel {
        deck: DeckId::new(0).unwrap(),
        track: 1,
        slot: slot(3),
        name: name.map(str::to_owned),
        color,
    })
}

#[test]
fn right_click_on_a_set_pad_opens_the_editor() {
    let view = with_drop();
    let (mut harness, received) = editing(view.clone());
    assert!(harness.query_by_role(Role::TextInput).is_none());
    let at = pad_at(&harness, &view, 3);
    right_click(&mut harness, at);
    let name = harness.get_by_role(Role::TextInput);
    assert_eq!(name.value().as_deref(), Some("Drop"));
    assert_eq!(ui::hotcue_menu::SWATCHES.len(), 9);
    for (label, _) in ui::hotcue_menu::SWATCHES {
        harness.get_by_label(label);
    }
    harness.get_by_label("No colour");
    harness.get_by_label("Delete");
    assert!(harness.query_by_label("Set hotcue here").is_none());
    assert_eq!(
        received.try_iter().count(),
        0,
        "a right-click sends nothing"
    );

    // A swatch saves its colour at once and leaves the editor open.
    let (green, colour) = ui::hotcue_menu::SWATCHES[4];
    press_label(&mut harness, green);
    let sent: Vec<UiCommand> = received.try_iter().collect();
    assert_eq!(sent, [label_step(Some("Drop"), colour)]);
    harness.get_by_role(Role::TextInput);

    // Enter saves the name, trimmed, with the colour chosen, and closes the editor.
    harness.get_by_role(Role::TextInput).focus();
    harness.run_steps(1);
    harness.key_combination_modifiers(Modifiers::COMMAND, &[Key::A]);
    harness.run_steps(1);
    harness
        .get_by_role(Role::TextInput)
        .type_text("  Big drop  ");
    harness.run_steps(1);
    harness.key_press(Key::Enter);
    harness.run_steps(2);
    let sent: Vec<UiCommand> = received.try_iter().collect();
    assert_eq!(sent, [label_step(Some("Big drop"), colour)]);
    assert!(harness.query_by_role(Role::TextInput).is_none());

    // "No colour" goes back to the slot's default; Delete presses and releases delete.
    right_click(&mut harness, at);
    press_label(&mut harness, "No colour");
    press_label(&mut harness, "Delete");
    let sent: Vec<UiCommand> = received.try_iter().collect();
    let delete = Action::HotcueDelete(DeckId::new(0).unwrap(), slot(3));
    let mut expected = vec![label_step(Some("Drop"), None)];
    expected.extend(press_release(delete));
    assert_eq!(sent, expected);
    assert!(
        harness.query_by_label("Delete").is_none(),
        "Delete closes it"
    );
}

#[test]
fn ctrl_click_on_a_pad_is_a_right_click_on_macos() {
    let view = with_drop();
    let (mut harness, received) = editing(view.clone());
    let at = pad_at(&harness, &view, 3);
    harness.event(Event::PointerMoved(at));
    harness.run_steps(1);
    for pressed in [true, false] {
        let event = Event::PointerButton {
            pos: at,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::CTRL,
        };
        harness.event_modifiers(event, Modifiers::CTRL);
        harness.run_steps(1);
    }
    harness.run_steps(2);
    let sent: Vec<UiCommand> = received.try_iter().collect();
    if cfg!(target_os = "macos") {
        harness.get_by_label("Delete");
        assert_eq!(sent, [], "no pad press");
    } else {
        assert!(harness.query_by_label("Delete").is_none());
        assert_eq!(
            sent,
            press_release(Action::Hotcue(DeckId::new(0).unwrap(), slot(3)))
        );
    }
}

#[test]
fn right_click_on_an_empty_pad_offers_set_here_only() {
    let view = with_drop();
    let (mut harness, received) = editing(view.clone());
    let at = pad_at(&harness, &view, 5);
    right_click(&mut harness, at);
    harness.get_by_label("Set hotcue here");
    assert!(harness.query_by_role(Role::TextInput).is_none());
    assert!(harness.query_by_label("Delete").is_none());
    assert!(harness.query_by_label("No colour").is_none());
    press_label(&mut harness, "Set hotcue here");
    let sent: Vec<UiCommand> = received.try_iter().collect();
    assert_eq!(
        sent,
        press_release(Action::Hotcue(DeckId::new(0).unwrap(), slot(5)))
    );
    assert!(harness.query_by_label("Set hotcue here").is_none());
}

#[test]
fn the_hotcue_editor_closes_on_escape_and_a_click_outside() {
    let view = with_drop();
    let (mut harness, received) = editing(view.clone());
    let at = pad_at(&harness, &view, 3);
    right_click(&mut harness, at);
    harness.key_press(Key::Escape);
    harness.run_steps(2);
    assert!(
        harness.query_by_label("Delete").is_none(),
        "Escape closes it"
    );
    right_click(&mut harness, at);
    harness.get_by_label("Delete");
    let outside = harness.get_by_label("Just Us").rect().center();
    harness.event(Event::PointerMoved(outside));
    harness.run_steps(1);
    harness.event(pointer(true, outside));
    harness.run_steps(1);
    harness.event(pointer(false, outside));
    harness.run_steps(2);
    assert!(
        harness.query_by_label("Delete").is_none(),
        "a click outside closes it"
    );
    assert_eq!(received.try_iter().count(), 0, "closing saves nothing");
}

#[test]
fn a_right_click_on_a_pad_in_another_mode_opens_nothing() {
    let (snapshot, mut frame) = loaded_plain();
    frame.control.sides[0].pad_mode = PadMode::LoopRoll;
    let view = view(&snapshot, &frame);
    let (mut harness, _received) = editing(view.clone());
    let at = pad_at(&harness, &view, 3);
    right_click(&mut harness, at);
    assert!(harness.query_by_label("Set hotcue here").is_none());
    assert!(harness.query_by_label("Delete").is_none());
}

#[test]
fn a_swatch_saves_the_name_as_typed() {
    let view = with_drop();
    let (mut harness, received) = editing(view.clone());
    let at = pad_at(&harness, &view, 3);
    right_click(&mut harness, at);
    harness.get_by_role(Role::TextInput).focus();
    harness.run_steps(1);
    harness.key_combination_modifiers(Modifiers::COMMAND, &[Key::A]);
    harness.run_steps(1);
    harness
        .get_by_role(Role::TextInput)
        .type_text("  Big drop ");
    harness.run_steps(1);
    let (blue, colour) = ui::hotcue_menu::SWATCHES[2];
    press_label(&mut harness, blue);
    let sent: Vec<UiCommand> = received.try_iter().collect();
    assert_eq!(sent, [label_step(Some("Big drop"), colour)]);
    // The field has the keyboard again: Enter still saves, and closes the editor.
    harness.key_press(Key::Enter);
    harness.run_steps(2);
    let sent: Vec<UiCommand> = received.try_iter().collect();
    assert_eq!(sent, [label_step(Some("Big drop"), colour)]);
    assert!(harness.query_by_role(Role::TextInput).is_none());
}

#[test]
fn the_click_that_closes_the_editor_plays_no_pad() {
    let view = with_drop();
    let (mut harness, received) = editing(view.clone());
    let at = pad_at(&harness, &view, 3);
    right_click(&mut harness, at);
    harness.get_by_label("Delete");
    let other = pad_at(&harness, &view, 5);
    press_and_release(&mut harness, other);
    assert!(
        harness.query_by_label("Delete").is_none(),
        "the click closes it"
    );
    assert_eq!(
        received.try_iter().collect::<Vec<_>>(),
        [],
        "and presses nothing"
    );
    // The next click plays the pad.
    press_and_release(&mut harness, other);
    assert_eq!(
        received.try_iter().collect::<Vec<_>>(),
        press_release(Action::Hotcue(DeckId::new(0).unwrap(), slot(5)))
    );
}

/// Drags the pointer from `from` to each of `path`, a frame apart, and lets go.
fn drag_through<S>(harness: &mut Harness<'_, S>, from: Pos2, path: &[Pos2]) {
    harness.event(Event::PointerMoved(from));
    harness.run_steps(1);
    harness.event(pointer(true, from));
    harness.run_steps(2);
    for &at in path {
        harness.event(Event::PointerMoved(at));
        harness.run_steps(1);
    }
    let end = path.last().copied().unwrap_or(from);
    harness.event(pointer(false, end));
    harness.run_steps(2);
}

/// A quarter turn clockwise round the jog, from twelve o'clock to three, and what it sent.
fn quarter_turn(view: DeckView) -> Vec<UiCommand> {
    let (mut harness, received) = sending(view);
    let rect = harness.get_by_label("jog").rect();
    let (centre, radius) = (rect.center(), rect.width() * 0.4);
    let arc: Vec<Pos2> = (1..=10)
        .map(|i| {
            let angle = std::f32::consts::FRAC_PI_2 * i as f32 / 10.0;
            centre + vec2(angle.sin(), -angle.cos()) * radius
        })
        .collect();
    drag_through(&mut harness, centre + vec2(0.0, -radius), &arc);
    received.try_iter().collect()
}

/// The steps `sent` turned `action` by.
fn steps_of(sent: &[UiCommand], action: Action) -> i32 {
    sent.iter()
        .filter_map(|command| match command {
            UiCommand::Action(a, Input::Relative(steps)) if *a == action => Some(*steps),
            _ => None,
        })
        .sum()
}

#[test]
fn jog_drag_on_a_paused_deck_touches_scratches_and_releases() {
    let (mut snapshot, frame) = loaded_plain();
    snapshot.playing = false;
    let sent = quarter_turn(view(&snapshot, &frame));
    let id = DeckId::new(0).unwrap();
    assert_eq!(
        sent.first(),
        Some(&UiCommand::Action(Action::JogTouch(id), Input::Press))
    );
    assert_eq!(
        sent.last(),
        Some(&UiCommand::Action(Action::JogTouch(id), Input::Release))
    );
    // A quarter of 720 steps per turn.
    let steps = steps_of(&sent, Action::JogScratch(id));
    assert!((steps - 180).abs() <= 2, "{steps} steps: {sent:?}");
    let scratches = sent
        .iter()
        .filter(|c| matches!(c, UiCommand::Action(Action::JogScratch(_), _)))
        .count();
    assert_eq!(sent.len(), 2 + scratches, "{sent:?}");
}

#[test]
fn jog_drag_on_a_playing_deck_nudges() {
    let (snapshot, frame) = loaded_plain();
    let sent = quarter_turn(view(&snapshot, &frame));
    let id = DeckId::new(0).unwrap();
    // The ring bends the pitch: no touch, which would hold the deck.
    let steps = steps_of(&sent, Action::JogRing(id));
    assert!((steps - 180).abs() <= 2, "{steps} steps: {sent:?}");
    assert!(
        sent.iter()
            .all(|c| matches!(c, UiCommand::Action(Action::JogRing(_), Input::Relative(_)))),
        "{sent:?}"
    );
}

/// Presses at `at` and holds still for more than a second (egui calls that a drag), then
/// lets go.
fn hold_still<S>(harness: &mut Harness<'_, S>, at: Pos2) {
    harness.event(Event::PointerMoved(at));
    harness.run_steps(1);
    harness.event(pointer(true, at));
    harness.run_steps(110);
    harness.event(pointer(false, at));
    harness.run_steps(2);
}

#[test]
fn a_slow_still_click_on_the_jog_sends_nothing() {
    for playing in [true, false] {
        let (mut snapshot, frame) = loaded_plain();
        snapshot.playing = playing;
        let (mut harness, received) = sending(view(&snapshot, &frame));
        let at = harness.get_by_label("jog").rect().center() + vec2(0.0, -30.0);
        hold_still(&mut harness, at);
        let sent: Vec<UiCommand> = received.try_iter().collect();
        assert!(sent.is_empty(), "playing {playing}: {sent:?}");
    }
}

#[test]
fn a_still_press_on_the_overview_seeks_once() {
    let (snapshot, frame) = loaded_plain();
    let (mut harness, received) = sending(view(&snapshot, &frame));
    let rect = harness.get_by_label("overview").rect();
    let at = pos2(rect.left() + rect.width() * 0.25, rect.center().y);
    harness.event(Event::PointerMoved(at));
    harness.run_steps(1);
    harness.event(pointer(true, at));
    harness.run_steps(8);
    harness.event(pointer(false, at));
    harness.run_steps(2);
    let sent: Vec<UiCommand> = received.try_iter().collect();
    assert_eq!(sent.len(), 1, "{sent:?}");
    let UiCommand::Action(Action::Seek(_), Input::Absolute(x)) = sent[0] else {
        panic!("{sent:?}");
    };
    assert!((x - 0.25).abs() <= 0.01, "{x}");
}

#[test]
fn a_tap_on_the_overview_seeks_and_a_tap_on_cue_presses_it() {
    let (snapshot, frame) = loaded_plain();
    let (mut harness, received) = sending(view(&snapshot, &frame));
    let rect = harness.get_by_label("overview").rect();
    let at = pos2(rect.left() + rect.width() * 0.25, rect.center().y);
    harness.event(Event::PointerMoved(at));
    harness.run_steps(1);
    tap(&mut harness, at);
    harness.run_steps(2);
    let sent: Vec<UiCommand> = received.try_iter().collect();
    assert!(
        matches!(sent[..], [UiCommand::Action(Action::Seek(_), Input::Absolute(x))] if (x - 0.25).abs() <= 0.01),
        "{sent:?}"
    );
    let cue = harness.get_by_label("CUE").rect().center();
    harness.event(Event::PointerMoved(cue));
    harness.run_steps(1);
    tap(&mut harness, cue);
    harness.run_steps(2);
    let sent: Vec<UiCommand> = received.try_iter().collect();
    assert_eq!(sent, press_release(Action::Cue(DeckId::new(0).unwrap())));
}

#[test]
fn a_press_on_the_pitch_handle_takes_it_where_it_is() {
    let (snapshot, mut frame) = loaded_plain();
    frame.control.decks[0].tempo = 0.25;
    let view = view(&snapshot, &frame);
    let (mut harness, received) = sending(view.clone());
    let tempo = Action::Tempo(DeckId::new(0).unwrap());
    // The slider's handle (6 points tall) rides its track, 3 points in from each end.
    let rect = harness.get_by_label("pitch").rect();
    let y = rect.top() + 3.0 + (rect.height() - 6.0) * view.pitch_slider;
    let at = pos2(rect.center().x, y + 2.0);
    harness.event(Event::PointerMoved(at));
    harness.run_steps(1);
    harness.event(pointer(true, at));
    harness.run_steps(3);
    harness.event(pointer(false, at));
    harness.run_steps(2);
    let sent: Vec<UiCommand> = received.try_iter().collect();
    let [UiCommand::Action(action, Input::Absolute(v))] = sent[..] else {
        panic!("{sent:?}");
    };
    assert_eq!(action, tempo);
    assert!(
        (v - view.pitch_slider).abs() < 1e-4,
        "{v} for {}",
        view.pitch_slider
    );
}

#[test]
fn losing_focus_releases_a_held_cue() {
    let (snapshot, frame) = loaded_plain();
    let (mut harness, received) = sending(view(&snapshot, &frame));
    let cue = harness.get_by_label("CUE").rect().center();
    harness.event(Event::PointerMoved(cue));
    harness.run_steps(1);
    harness.event(pointer(true, cue));
    harness.run_steps(2);
    let input = harness.input_mut();
    input.focused = false;
    input.events.push(Event::WindowFocused(false));
    harness.run_steps(2);
    let sent: Vec<UiCommand> = received.try_iter().collect();
    assert_eq!(sent, press_release(Action::Cue(DeckId::new(0).unwrap())));
}

#[test]
fn overview_click_seeks() {
    let (snapshot, frame) = loaded_plain();
    let view = view(&snapshot, &frame);
    let (mut harness, received) = sending(view);
    let rect = harness.get_by_label("overview").rect();
    let at = pos2(rect.left() + rect.width() * 0.25, rect.center().y);
    press_and_release(&mut harness, at);
    let sent: Vec<UiCommand> = received.try_iter().collect();
    let seeks: Vec<f32> = sent
        .iter()
        .filter_map(|command| match command {
            UiCommand::Action(Action::Seek(_), Input::Absolute(x)) => Some(*x),
            _ => None,
        })
        .collect();
    // Once: a press held still does not seek again every frame.
    assert_eq!(seeks.len(), 1, "{sent:?}");
    assert!((seeks[0] - 0.25).abs() <= 0.01, "{seeks:?}");
    assert!(
        sent.iter()
            .all(|c| matches!(c, UiCommand::Action(Action::Seek(_), _)))
    );
}

#[test]
fn drag_on_an_empty_deck_sends_nothing() {
    let snapshot = DeckSnapshot::default();
    let frame = ControlFrame::default();
    let view = view(&snapshot, &frame);
    let (mut harness, received) = sending(view);
    let jog = harness.get_by_label("jog").rect();
    let overview = harness.get_by_label("overview").rect();
    for rect in [jog, overview] {
        let from = rect.center();
        drag_through(
            &mut harness,
            from,
            &[from + vec2(10.0, 0.0), from + vec2(20.0, 5.0)],
        );
    }
    let sent: Vec<UiCommand> = received.try_iter().collect();
    assert!(sent.is_empty(), "{sent:?}");
}

#[test]
fn a_click_on_the_jog_of_a_playing_deck_sends_nothing() {
    let (snapshot, frame) = loaded_plain();
    let view = view(&snapshot, &frame);
    let (mut harness, received) = sending(view);
    let at = harness.get_by_label("jog").rect().center() + vec2(0.0, -30.0);
    press_and_release(&mut harness, at);
    let sent: Vec<UiCommand> = received.try_iter().collect();
    assert!(sent.is_empty(), "{sent:?}");
}

#[test]
fn a_drag_at_the_jog_hub_turns_nothing() {
    let (snapshot, frame) = loaded_plain();
    let view = view(&snapshot, &frame);
    let (mut harness, received) = sending(view);
    let centre = harness.get_by_label("jog").rect().center();
    drag_through(
        &mut harness,
        centre + vec2(-5.0, 0.0),
        &[centre + vec2(0.0, 5.0), centre + vec2(5.0, 0.0)],
    );
    let scratched = received
        .try_iter()
        .any(|c| matches!(c, UiCommand::Action(Action::JogScratch(_), _)));
    assert!(!scratched);
}
