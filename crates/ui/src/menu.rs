//! VirtualDJ's LAYOUT menu in the top bar. Each entry asks the session for a
//! [`ViewChange`]; the menu draws the state the session published, so an entry shows its
//! effect one frame later.

use std::sync::mpsc::Sender;

use egui::Ui;

use crate::command::{UiCommand, ViewChange};
use crate::theme;
use crate::view::{ColourScheme, JogDisplay, Layout, ViewState, WaveformPosition, WaveformStyle};

/// Draws the LAYOUT button, with the current layout's name beside it, and its menu.
pub fn show(ui: &mut Ui, view: &ViewState, commands: &Sender<UiCommand>) {
    let dim = theme::palette(ui.ctx()).text_dim;
    ui.menu_button("LAYOUT", |ui| menu(ui, view, commands));
    ui.label(egui::RichText::new(layout_name(view.layout)).color(dim));
}

fn menu(ui: &mut Ui, view: &ViewState, commands: &Sender<UiCommand>) {
    let send = |change: ViewChange| {
        // The session going away is the window closing: nothing to tell.
        let _ = commands.send(UiCommand::View(change));
    };
    let choose = |ui: &mut Ui, selected: bool, name: &str, change: ViewChange| {
        if ui.radio(selected, name).clicked() {
            send(change);
        }
    };
    let toggle = |ui: &mut Ui, on: bool, name: &str, change: fn(bool) -> ViewChange| {
        let mut value = on;
        if ui.checkbox(&mut value, name).clicked() {
            send(change(value));
        }
    };

    for layout in Layout::ALL {
        choose(
            ui,
            view.layout == *layout,
            layout_name(*layout),
            ViewChange::SetLayout(*layout),
        );
    }
    ui.separator();
    ui.menu_button("Colour Scheme", |ui| {
        for (scheme, name) in [(ColourScheme::Dark, "Dark"), (ColourScheme::Light, "Light")] {
            choose(
                ui,
                view.colour_scheme == scheme,
                name,
                ViewChange::SetColourScheme(scheme),
            );
        }
    });
    ui.menu_button("Waveform Display", |ui| {
        for (style, name) in [
            (WaveformStyle::Frequency, "Frequency"),
            (WaveformStyle::DeckColour, "Deck colour"),
            (WaveformStyle::Bands, "Bands"),
            (WaveformStyle::Overlaid, "Overlaid"),
        ] {
            choose(
                ui,
                view.waveform_style == style,
                name,
                ViewChange::SetWaveformStyle(style),
            );
        }
        ui.separator();
        for (position, name) in [
            (WaveformPosition::Top, "Top"),
            (WaveformPosition::Middle, "Middle"),
        ] {
            choose(
                ui,
                view.waveform_position == position,
                name,
                ViewChange::SetWaveformPosition(position),
            );
        }
        toggle(ui, view.overview, "Overview", ViewChange::SetOverview);
    });
    ui.menu_button("Jogwheel Display", |ui| {
        for (display, name) in [(JogDisplay::Plain, "Plain"), (JogDisplay::Info, "Info")] {
            choose(
                ui,
                view.jog_display == display,
                name,
                ViewChange::SetJogDisplay(display),
            );
        }
    });
    ui.menu_button("Beat Counter", |ui| {
        toggle(ui, view.beat_counter, "On", ViewChange::SetBeatCounter);
    });
    ui.separator();
    toggle(ui, view.four_decks, "4-Decks", ViewChange::SetFourDecks);
    ui.menu_button("Panels", |ui| {
        toggle(ui, view.info_panel, "Info", ViewChange::SetInfoPanel);
        toggle(ui, view.sideview, "Sideview", ViewChange::SetSideview);
    });
}

fn layout_name(layout: Layout) -> &'static str {
    match layout {
        Layout::Starter => "Starter",
        Layout::Essentials => "Essentials",
        Layout::Pro => "Pro",
        Layout::Performance => "Performance",
    }
}
