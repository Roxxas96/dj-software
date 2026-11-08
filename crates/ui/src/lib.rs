//! The window: an egui (eframe) app that draws the [`ControlFrame`] the control thread
//! publishes and reads the engine's state through an [`EngineMonitor`]. It never sends
//! the engine anything: it asks the session for changes with [`UiCommand`]s, and the
//! control thread never waits on it.

pub mod browser;
mod command;
pub mod cover;
pub mod deck;
mod fonts;
pub mod layout;
mod menu;
pub mod mixer;
mod model;
pub mod theme;
mod topbar;
mod view;
pub mod waveform;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Sender, channel};

use anyhow::anyhow;
use engine::EngineMonitor;

pub use command::{UiCommand, ViewChange};
pub use model::{ControlFrame, DeckInfo, EVENT_SHOWN, EVENTS_KEPT, GHOSTS, Published};
pub use view::{
    ColourScheme, JogDisplay, Layout, MAX_SHARE, MIN_SHARE, PanelSizes, TimeMode, ViewState,
    WaveformPosition, WaveformStyle,
};

const TITLE: &str = "Open Jockey";
const WINDOW_SIZE: [f32; 2] = [1440.0, 900.0];

/// Opens the window and draws until it is closed, then sets `quit`. The window closes
/// itself once something else sets `quit`. `monitor` holds `None` until the audio device
/// opens. What the viewer asks for (a zoom, a layout) is sent on `commands`; the session
/// applies it and publishes the result in the frame.
pub fn run(
    frame: Published<ControlFrame>,
    monitor: Published<Option<EngineMonitor>>,
    quit: Arc<AtomicBool>,
    commands: Sender<UiCommand>,
) -> anyhow::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(TITLE)
            .with_inner_size(WINDOW_SIZE),
        ..eframe::NativeOptions::default()
    };
    let app = App {
        frame,
        monitor,
        quit: Arc::clone(&quit),
        screen: Screen {
            commands,
            ..Screen::default()
        },
    };
    let result = eframe::run_native(
        TITLE,
        options,
        Box::new(|cc| {
            theme::apply(&cc.egui_ctx, ColourScheme::default());
            fonts::install(&cc.egui_ctx);
            Ok(Box::new(app))
        }),
    );
    quit.store(true, Ordering::Relaxed);
    result.map_err(|e| anyhow!("running the window: {e}"))
}

struct App {
    frame: Published<ControlFrame>,
    monitor: Published<Option<EngineMonitor>>,
    quit: Arc<AtomicBool>,
    screen: Screen,
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if self.quit.load(Ordering::Relaxed) {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        }
        let frame = self.frame.latest();
        let monitor = self.monitor.latest();
        root(ui, &frame, monitor.as_ref().as_ref(), &mut self.screen);
        // The decks move on their own: draw at the display's rate.
        ui.ctx().request_repaint();
    }

    fn on_exit(&mut self) {
        self.quit.store(true, Ordering::Relaxed);
    }
}

/// What the window keeps from one frame to the next.
#[derive(Debug)]
pub struct Screen {
    pub lanes: waveform::Lanes,
    pub decks: deck::DeckPanels,
    pub covers: cover::Covers,
    pub mixer: mixer::Mixer,
    /// Where what the viewer asks for goes: the session.
    pub commands: Sender<UiCommand>,
}

impl Default for Screen {
    /// A screen whose commands go nowhere.
    fn default() -> Self {
        Self {
            lanes: waveform::Lanes::default(),
            decks: deck::DeckPanels::default(),
            covers: cover::Covers::default(),
            mixer: mixer::Mixer::default(),
            commands: channel().0,
        }
    }
}

/// Draws the whole window for `frame`; `monitor` is `None` until the audio device opens.
pub fn root(
    ui: &mut egui::Ui,
    frame: &ControlFrame,
    monitor: Option<&EngineMonitor>,
    screen: &mut Screen,
) {
    theme::apply(ui.ctx(), frame.view.colour_scheme);
    let p = theme::palette(ui.ctx());
    let window_height = ui.max_rect().height();
    let sections = layout::sections(frame.view.layout);
    egui::Panel::top("top_bar")
        .frame(egui::Frame::new().fill(p.panel).inner_margin(6))
        .show(ui, |ui| topbar::show(ui, frame, monitor, &screen.commands));
    egui::Panel::top("waveform_lanes")
        .frame(egui::Frame::new().fill(p.background))
        .exact_size(window_height * layout::LANES_SHARE)
        .resizable(false)
        .show_separator_line(false)
        .show(ui, |ui| {
            screen.lanes.show(ui, frame, monitor, &screen.commands)
        });
    egui::Panel::top("deck_panels")
        .frame(egui::Frame::new().fill(p.background))
        .exact_size(window_height * sections.decks_share)
        .resizable(false)
        .show_separator_line(false)
        .show(ui, |ui| {
            // The space between the decks is the mixer's.
            let space = screen
                .decks
                .show(ui, frame, monitor, &mut screen.covers, &sections);
            screen.mixer.show(ui, space, frame, monitor, &sections);
        });
    egui::CentralPanel::default()
        .frame(egui::Frame::new().fill(p.background))
        .show(ui, |ui| {
            if let Some(error) = &frame.error {
                ui.centered_and_justified(|ui| {
                    ui.label(
                        egui::RichText::new(error.as_str())
                            .color(p.error)
                            .size(18.0),
                    );
                });
            } else if let Some(view) = &frame.browser {
                browser::show(ui, view, frame.key_notation, &mut screen.covers);
            }
        });
}
