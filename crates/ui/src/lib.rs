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
pub mod sideview;
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
/// The smallest the window goes: the panels still fit.
const MIN_WINDOW_SIZE: [f32; 2] = [1024.0, 700.0];

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
            .with_inner_size(WINDOW_SIZE)
            .with_min_inner_size(MIN_WINDOW_SIZE),
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
    /// The shares a divider was dragged to, drawn until the session's copy catches up.
    pub sizes: Option<Dragged>,
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
            sizes: None,
            commands: channel().0,
        }
    }
}

/// The shares a divider was dragged to, and the session's shares when it was.
#[derive(Debug, Clone, Copy)]
pub struct Dragged {
    pub sizes: PanelSizes,
    pub base: PanelSizes,
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
    let window = ui.max_rect().size();
    let mut sections = if frame.view.four_decks {
        layout::four_deck_sections()
    } else {
        layout::sections(frame.view.layout)
    };
    sections.overview &= frame.view.overview;
    let middle = frame.view.waveform_position == WaveformPosition::Middle;
    // The session's shares are the source; a drag in progress (or just let go, until the
    // session has applied it) shows its own. The decks' share still at its default means
    // the viewer never set it: the layout's own share applies.
    if screen.sizes.is_some_and(|d| d.base != frame.view.sizes) {
        screen.sizes = None;
    }
    let mut sizes = screen.sizes.map_or(frame.view.sizes, |d| d.sizes);
    if sizes.decks == PanelSizes::default().decks {
        sizes.decks = sections.decks_share;
    }
    let mut drags = Drags {
        sizes: layout::clamp_sizes(sizes, window),
        window,
        moved: false,
        stopped: false,
    };
    egui::Panel::top("top_bar")
        .frame(egui::Frame::new().fill(p.panel).inner_margin(6))
        .show(ui, |ui| topbar::show(ui, frame, monitor, &screen.commands));
    let lanes = |ui: &mut egui::Ui, screen: &mut Screen, drags: &mut Drags| {
        let height = drags.sizes.lanes * window.y;
        let (drag, top) = row_panel(ui, "waveform_lanes", height, &p, |ui| {
            screen.lanes.show(ui, frame, monitor, &screen.commands)
        });
        drags.place(drag, top, |sizes, share| sizes.lanes = share);
    };
    if !middle {
        lanes(ui, screen, &mut drags);
    }
    let height = drags.sizes.decks * window.y;
    let (drag, top) = row_panel(ui, "deck_panels", height, &p, |ui| {
        // The space between the decks is the mixer's.
        let space = screen.decks.show(
            ui,
            frame,
            monitor,
            &mut screen.covers,
            &sections,
            &screen.commands,
        );
        screen.mixer.show(ui, space, frame, monitor, &sections);
    });
    drags.place(drag, top, |sizes, share| sizes.decks = share);
    if middle {
        lanes(ui, screen, &mut drags);
    }
    let arrangement = browser::Arrangement {
        sizes: drags.sizes,
        info: frame.view.info_panel,
        sideview: frame.view.sideview,
        window,
    };
    let mut changes = browser::Changes::default();
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
                changes = browser::show(
                    ui,
                    view,
                    frame.key_notation,
                    &mut screen.covers,
                    &arrangement,
                    |ui| sideview::show(ui, frame, monitor),
                );
            }
        });
    if let Some(dragged) = changes.sizes {
        drags.sizes = dragged;
        drags.moved = true;
    }
    if drags.moved {
        screen.sizes = Some(Dragged {
            sizes: drags.sizes,
            base: frame.view.sizes,
        });
    }
    if let Some(change) = changes.unfold {
        let _ = screen.commands.send(UiCommand::View(change));
    }
    if drags.stopped || changes.stopped {
        let change = ViewChange::SetSizes(drags.sizes);
        let _ = screen.commands.send(UiCommand::View(change));
    }
}

/// The shares as the dividers dragged this frame leave them.
struct Drags {
    sizes: PanelSizes,
    window: egui::Vec2,
    moved: bool,
    stopped: bool,
}

impl Drags {
    /// Takes a drag of the divider under the panel starting at `top`; `set` stores the
    /// panel's new share.
    fn place(&mut self, drag: browser::Drag, top: f32, set: fn(&mut PanelSizes, f32)) {
        self.stopped |= drag.stopped;
        if let Some(pointer) = drag.pointer {
            set(&mut self.sizes, (pointer.y - top) / self.window.y);
            self.sizes = layout::clamp_sizes(self.sizes, self.window);
            self.moved = true;
        }
    }
}

/// A panel of `height` below whatever was added before it, with a divider under it that
/// drags. Returns the drag and where the panel starts.
fn row_panel(
    ui: &mut egui::Ui,
    name: &str,
    height: f32,
    p: &theme::Palette,
    add: impl FnOnce(&mut egui::Ui),
) -> (browser::Drag, f32) {
    let shown = egui::Panel::top(name)
        .frame(egui::Frame::new().fill(p.background))
        .exact_size(height)
        .resizable(false)
        .show_separator_line(false)
        .show(ui, add);
    let rect = shown.response.rect;
    let strip = egui::Rect::from_min_max(
        egui::pos2(rect.min.x, rect.max.y - 4.0),
        egui::pos2(rect.max.x, rect.max.y + 4.0),
    );
    let drag = browser::divider(ui, name, strip, egui::CursorIcon::ResizeVertical);
    (drag, rect.min.y)
}
