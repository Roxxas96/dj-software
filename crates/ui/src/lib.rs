//! The window: an egui (eframe) app that draws the [`ControlFrame`] the control thread
//! publishes and reads the engine's state through an [`EngineMonitor`]. It never sends
//! the engine anything, and the control thread never waits on it.

pub mod browser;
pub mod cover;
pub mod deck;
mod fonts;
pub mod mixer;
mod model;
pub mod theme;
mod topbar;
pub mod waveform;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

use anyhow::anyhow;
use engine::EngineMonitor;

pub use model::{ControlFrame, DeckInfo, EVENT_SHOWN, EVENTS_KEPT, GHOSTS, Published};

const TITLE: &str = "Open Jockey";
const WINDOW_SIZE: [f32; 2] = [1440.0, 900.0];

/// Opens the window and draws until it is closed, then sets `quit`. The window closes
/// itself once something else sets `quit`. `monitor` holds `None` until the audio device
/// opens. The waveform zoom steps the viewer asks for are added to `zoom` (positive zooms
/// in); the session takes them and publishes the new zoom in the frame.
pub fn run(
    frame: Published<ControlFrame>,
    monitor: Published<Option<EngineMonitor>>,
    quit: Arc<AtomicBool>,
    zoom: Arc<AtomicI32>,
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
            zoom,
            ..Screen::default()
        },
    };
    let result = eframe::run_native(
        TITLE,
        options,
        Box::new(|cc| {
            theme::apply(&cc.egui_ctx);
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

/// The share of the window's height the waveform lanes take.
const LANES_HEIGHT: f32 = 0.22;
/// The share of the window's height the deck panels and the mixer take.
const DECKS_HEIGHT: f32 = 0.38;

/// What the window keeps from one frame to the next.
#[derive(Debug, Default)]
pub struct Screen {
    pub lanes: waveform::Lanes,
    pub decks: deck::DeckPanels,
    pub covers: cover::Covers,
    pub mixer: mixer::Mixer,
    /// Waveform zoom steps asked for and not yet taken by the session.
    pub zoom: Arc<AtomicI32>,
}

/// Draws the whole window for `frame`; `monitor` is `None` until the audio device opens.
pub fn root(
    ui: &mut egui::Ui,
    frame: &ControlFrame,
    monitor: Option<&EngineMonitor>,
    screen: &mut Screen,
) {
    let window_height = ui.max_rect().height();
    egui::Panel::top("top_bar")
        .frame(egui::Frame::new().fill(theme::PANEL).inner_margin(6))
        .show(ui, |ui| topbar::show(ui, frame, monitor));
    egui::Panel::top("waveform_lanes")
        .frame(egui::Frame::new().fill(theme::BACKGROUND))
        .exact_size(window_height * LANES_HEIGHT)
        .resizable(false)
        .show_separator_line(false)
        .show(ui, |ui| screen.lanes.show(ui, frame, monitor, &screen.zoom));
    egui::Panel::top("deck_panels")
        .frame(egui::Frame::new().fill(theme::BACKGROUND))
        .exact_size(window_height * DECKS_HEIGHT)
        .resizable(false)
        .show_separator_line(false)
        .show(ui, |ui| {
            // The space between the decks is the mixer's.
            let space = screen.decks.show(ui, frame, monitor, &mut screen.covers);
            screen.mixer.show(ui, space, frame, monitor);
        });
    egui::CentralPanel::default()
        .frame(egui::Frame::new().fill(theme::BACKGROUND))
        .show(ui, |ui| {
            if let Some(error) = &frame.error {
                ui.centered_and_justified(|ui| {
                    ui.label(
                        egui::RichText::new(error.as_str())
                            .color(theme::ERROR)
                            .size(18.0),
                    );
                });
            } else if let Some(view) = &frame.browser {
                browser::show(ui, view, frame.key_notation, &mut screen.covers);
            }
        });
}
