//! The window: an egui (eframe) app that draws the [`ControlFrame`] the control thread
//! publishes and reads the engine's state through an [`EngineMonitor`]. It never sends
//! the engine anything, and the control thread never waits on it.

mod model;
pub mod theme;
mod topbar;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::anyhow;
use engine::EngineMonitor;

pub use model::{ControlFrame, DeckInfo, EVENT_SHOWN, EVENTS_KEPT, GHOSTS, Published};

const TITLE: &str = "Open Jockey";
const WINDOW_SIZE: [f32; 2] = [1440.0, 900.0];

/// Opens the window and draws until it is closed, then sets `quit`. The window closes
/// itself once something else sets `quit`. `monitor` holds `None` until the audio device
/// opens.
pub fn run(
    frame: Published<ControlFrame>,
    monitor: Published<Option<EngineMonitor>>,
    quit: Arc<AtomicBool>,
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
    };
    let result = eframe::run_native(
        TITLE,
        options,
        Box::new(|cc| {
            theme::apply(&cc.egui_ctx);
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
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if self.quit.load(Ordering::Relaxed) {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        }
        let frame = self.frame.latest();
        let monitor = self.monitor.latest();
        root(ui, &frame, monitor.as_ref().as_ref());
        // The decks move on their own: draw at the display's rate.
        ui.ctx().request_repaint();
    }

    fn on_exit(&mut self) {
        self.quit.store(true, Ordering::Relaxed);
    }
}

/// Draws the whole window for `frame`; `monitor` is `None` until the audio device opens.
pub fn root(ui: &mut egui::Ui, frame: &ControlFrame, monitor: Option<&EngineMonitor>) {
    egui::Panel::top("top_bar")
        .frame(egui::Frame::new().fill(theme::PANEL).inner_margin(6))
        .show(ui, |ui| topbar::show(ui, frame, monitor));
    egui::CentralPanel::default()
        .frame(egui::Frame::new().fill(theme::BACKGROUND))
        .show(ui, |_ui| {});
}
