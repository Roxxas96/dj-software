//! The LAYOUT menu asks the session for view changes, and the light scheme draws.

use std::sync::mpsc::channel;

use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use ui::{ColourScheme, ControlFrame, Layout, UiCommand, ViewChange, ViewState};

#[test]
fn the_layout_menu_sends_view_changes() {
    let frame = ControlFrame::default();
    let (commands, asked) = channel();
    let mut screen = ui::Screen {
        commands,
        ..ui::Screen::default()
    };
    let mut harness = Harness::builder()
        .with_size([1440.0, 900.0])
        .build_ui(|ui| ui::root(ui, &frame, None, &mut screen));
    harness.run_steps(2);
    harness.get_by_label("LAYOUT").click();
    harness.run_steps(2);
    harness.get_by_label("Colour Scheme").click();
    harness.run_steps(2);
    harness.get_by_label("Light").click();
    harness.run_steps(2);
    assert_eq!(
        asked.try_iter().collect::<Vec<_>>(),
        [UiCommand::View(ViewChange::SetColourScheme(
            ColourScheme::Light
        ))]
    );
    harness.get_by_label("LAYOUT").click();
    harness.run_steps(2);
    harness.get_by_label("Starter").click();
    harness.run_steps(2);
    assert_eq!(
        asked.try_iter().collect::<Vec<_>>(),
        [UiCommand::View(ViewChange::SetLayout(Layout::Starter))]
    );
}

#[test]
fn the_light_scheme_draws() {
    let frame = ControlFrame {
        view: ViewState {
            colour_scheme: ColourScheme::Light,
            ..ViewState::default()
        },
        ..ControlFrame::default()
    };
    let mut screen = ui::Screen::default();
    let mut harness = Harness::builder()
        .with_size([1440.0, 900.0])
        .build_ui(|ui| ui::root(ui, &frame, None, &mut screen));
    harness.run_steps(3);
    harness.get_by_label("LAYOUT");
    assert_eq!(ui::theme::palette(&harness.ctx), ui::theme::Palette::LIGHT);
}

#[test]
fn four_decks_and_performance_disable_what_they_ignore() {
    let draw = |view: ViewState| {
        let frame = ControlFrame {
            view,
            ..ControlFrame::default()
        };
        let (commands, asked) = channel();
        let mut screen = ui::Screen {
            commands,
            ..ui::Screen::default()
        };
        let mut harness = Harness::builder()
            .with_size([1440.0, 900.0])
            .build_ui(move |ui| ui::root(ui, &frame, None, &mut screen));
        harness.run_steps(2);
        harness.get_by_label("LAYOUT").click();
        harness.run_steps(2);
        (harness, asked)
    };
    let disabled = |harness: &Harness<'_>, label: &str| {
        harness.get_by_label(label).accesskit_node().is_disabled()
    };
    let four = ViewState {
        four_decks: true,
        ..ViewState::default()
    };
    let (mut harness, asked) = draw(four);
    assert!(disabled(&harness, "Starter") && disabled(&harness, "Jogwheel Display"));
    harness.get_by_label("Starter").click();
    harness.run_steps(2);
    assert_eq!(asked.try_iter().collect::<Vec<_>>(), []);
    drop(harness);

    let performance = ViewState {
        layout: Layout::Performance,
        ..ViewState::default()
    };
    let (harness, _) = draw(performance);
    assert!(!disabled(&harness, "Starter"));
    assert!(disabled(&harness, "Jogwheel Display"));
    drop(harness);
    let (harness, _) = draw(ViewState::default());
    assert!(!disabled(&harness, "Jogwheel Display"));
}
