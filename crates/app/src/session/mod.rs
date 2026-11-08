//! The window session: `open-jockey` with no subcommand. The main thread runs the window;
//! the session thread behind it runs what `play --library --controller` runs in the
//! terminal (the controller, library mode and the stems jobs) and publishes a
//! [`ui::ControlFrame`] for the window, which reads the engine through its monitor.

mod detail;
mod frame;
mod zoom;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use anyhow::{Context, anyhow};
use audio_io::{AudioError, OutputStream};
use controller::Connection;
use controller::mapping::Mapping;
use engine::{EngineController, EngineMonitor};
use ui::{ControlFrame, Published};

use crate::config::Config;
use crate::library_mode::LibraryMode;
use crate::play::{ControllerPorts, LoopOutput, Pass};
use crate::printer::{Line, Printer};
use detail::DetailJobs;
use frame::FrameParts;

/// How often a missing audio device or MIDI backend is tried again.
const RETRY_EVERY: Duration = Duration::from_secs(2);
/// How often `quit` is checked while waiting to retry.
const QUIT_CHECK: Duration = Duration::from_millis(50);
/// The most often a frame is published.
const PUBLISH_EVERY: Duration = Duration::from_millis(10);
const KEYS: &str = "close the window to quit";

/// What the window reads.
#[derive(Clone)]
struct Shared {
    frame: Published<ControlFrame>,
    monitor: Published<Option<EngineMonitor>>,
    /// Waveform zoom steps the window asked for, not yet applied.
    zoom: Arc<AtomicI32>,
    /// Where the window's state (its zoom) is kept; `None` keeps nothing.
    state_path: Option<PathBuf>,
}

/// The open audio output with the engine it plays.
struct Audio {
    engine: EngineController,
    /// `None` for a test engine, rendered by the test.
    stream: Option<OutputStream>,
    device: String,
    output_latency: Duration,
}

/// Opens the window and runs the session behind it until the window is closed (or Ctrl-C),
/// then saves library mode's cues and hotcues. A session that cannot run (a bad
/// configuration or mapping) leaves the window open, showing why, and returns that error
/// once the window is closed.
pub fn run_window(config_path: Option<&Path>) -> anyhow::Result<()> {
    let quit = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&quit);
    ctrlc::set_handler(move || flag.store(true, Ordering::Relaxed))
        .context("installing the Ctrl-C handler")?;
    let shared = Shared {
        frame: Published::new(ControlFrame::default()),
        monitor: Published::new(None),
        zoom: Arc::new(AtomicI32::new(0)),
        state_path: state_path(config_path),
    };
    let session = {
        let (shared, quit) = (shared.clone(), Arc::clone(&quit));
        let config_path = config_path.map(Path::to_owned);
        std::thread::Builder::new()
            .name("session".to_owned())
            .spawn(move || session(config_path.as_deref(), &shared, &quit))
            .context("starting the session thread")?
    };
    let shown = ui::run(shared.frame, shared.monitor, Arc::clone(&quit), shared.zoom);
    quit.store(true, Ordering::Relaxed);
    let ran = session
        .join()
        .map_err(|_| anyhow!("the session thread panicked"))?;
    ran.and(shown)
}

/// `ui-state.toml` next to the configuration file in use.
fn state_path(config_path: Option<&Path>) -> Option<PathBuf> {
    match config_path {
        Some(path) => Some(path.with_file_name(zoom::STATE_FILE)),
        None => controller::builtin::config_dir().map(|dir| dir.join(zoom::STATE_FILE)),
    }
}

/// The session thread: loads the configuration, then [`run_config`]. When that ends the
/// window closes, unless it ended in an error: the window then shows the error until it is
/// closed.
fn session(config_path: Option<&Path>, shared: &Shared, quit: &AtomicBool) -> anyhow::Result<()> {
    let result = crate::config::load(config_path).and_then(|config| {
        let mut parts = FrameParts::new(&config);
        parts.zoom = zoom::Zoom::load(shared.state_path.clone(), config.ui.waveform_beats);
        shared.frame.publish(parts.idle());
        run_config(&config, shared, quit)
    });
    match &result {
        Ok(()) => quit.store(true, Ordering::Relaxed),
        Err(error) => {
            // The engine, if one ran, is gone.
            shared.monitor.publish(None);
            shared.frame.publish(ControlFrame {
                error: Some(crate::format::safe(format!("{error:#}"))),
                ..ControlFrame::default()
            });
        }
    }
    result
}

/// Opens the mapping and the library, then runs until `quit`.
fn run_config(config: &Config, shared: &Shared, quit: &AtomicBool) -> anyhow::Result<()> {
    let mapping = crate::legacy_dirs::load_mapping(&config.controller.mapping)
        .context("loading the controller mapping")?;
    let library = LibraryMode::start(None, &config.stems)?;
    let (samples, lines) = crate::play::load_samples_from(
        config,
        crate::play::open_sample_library(config, Some(&library)),
    );
    let mut samples_loaded = [false; engine::SAMPLER_SLOTS];
    for (slot, _) in &samples {
        if let Some(loaded) = samples_loaded.get_mut(*slot) {
            *loaded = true;
        }
    }
    let open = || open_audio(config);
    let mut start = Start {
        samples,
        samples_loaded,
        events: lines,
    };
    run_session(config, shared, quit, mapping, library, &mut start, open)
}

/// What the session starts the engine with, and reports first.
struct Start {
    samples: Vec<(usize, Arc<engine::TrackBuffer>)>,
    samples_loaded: [bool; engine::SAMPLER_SLOTS],
    events: Vec<String>,
}

/// Opens the configured output device, and the engine for it.
fn open_audio(config: &Config) -> Result<Audio, String> {
    let device = config.audio.device.as_deref();
    let pending =
        audio_io::open_output(device, config.audio.routing()).map_err(|error| match error {
            AudioError::DeviceNotFound { query, .. } => {
                format!("audio device {query} not found, waiting")
            }
            AudioError::NoDefaultDevice => "no audio device found, waiting".to_owned(),
            other => format!(
                "{}; retrying",
                crate::play::describe_audio_error(&other).replace('\n', "; ")
            ),
        })?;
    let config = pending.engine_config();
    let output_latency =
        Duration::from_secs_f64(config.max_block_frames as f64 / f64::from(config.sample_rate));
    let device = pending.device_name().to_owned();
    let (engine, renderer) = engine::engine(config);
    let stream = pending
        .start(renderer)
        .map_err(|e| format!("starting audio output: {e}; retrying"))?;
    Ok(Audio {
        engine,
        stream: Some(stream),
        device,
        output_latency,
    })
}

/// Waits for the audio device (asking `open` every [`RETRY_EVERY`]) and the MIDI backend,
/// then runs the control loop until `quit`, publishing frames for the window. Library mode
/// is shut down on every way out, so the last cue and hotcue changes are saved.
fn run_session(
    config: &Config,
    shared: &Shared,
    quit: &AtomicBool,
    mapping: Mapping,
    library: LibraryMode,
    start: &mut Start,
    mut open: impl FnMut() -> Result<Audio, String>,
) -> anyhow::Result<()> {
    let (lines_tx, lines) = channel();
    let printer = Printer::to_channel(lines_tx);
    let mut parts = FrameParts::new(config);
    parts.zoom = zoom::Zoom::load(shared.state_path.clone(), config.ui.waveform_beats);
    parts.detail = Some(DetailJobs::new(library.library()));
    parts.samples_loaded = start.samples_loaded;
    for text in start.events.drain(..) {
        parts.event(text);
    }

    let audio = retry(quit, &lines, &mut parts, shared, |parts| match open() {
        Ok(audio) => Some(audio),
        Err(why) => {
            parts.audio = Err(why);
            None
        }
    });
    let Some(mut audio) = audio else {
        // No engine ever ran: an idle one is enough to stop library mode.
        let (mut engine, _renderer) = engine::engine(engine::EngineConfig {
            sample_rate: 44_100,
            max_block_frames: 512,
        });
        library.shutdown(&mut engine, &printer);
        return finish(printer, &lines);
    };
    if let Err(error) = crate::play::configure_engine(
        &mut audio.engine,
        config,
        std::mem::take(&mut start.samples),
    ) {
        library.shutdown(&mut audio.engine, &printer);
        finish(printer, &lines)?;
        return Err(error);
    }
    shared.monitor.publish(Some(audio.engine.monitor()));
    parts.audio = Ok(audio.device.clone());
    parts.output_latency = audio.output_latency;

    let (tx, rx) = channel();
    let mut reported = false;
    let connection = retry(
        quit,
        &lines,
        &mut parts,
        shared,
        |parts| match Connection::new(&mapping, tx.clone()) {
            Ok(connection) => Some(connection),
            Err(e) => {
                if !std::mem::replace(&mut reported, true) {
                    parts.event(format!("cannot open MIDI: {e}; retrying"));
                }
                None
            }
        },
    );
    let Some(connection) = connection else {
        library.shutdown(&mut audio.engine, &printer);
        return finish(printer, &lines);
    };

    let ports = ControllerPorts {
        mapping,
        connection,
        rx,
    };
    let output = LoopOutput {
        printer: &printer,
        console: None,
        keys: KEYS,
    };
    let mut last_publish: Option<Instant> = None;
    let mut on_pass = |pass: Pass<'_>| {
        for line in lines.try_iter() {
            parts.take(line);
        }
        parts.take_zoom(
            shared.zoom.swap(0, Ordering::Relaxed),
            Some(pass.state.waveform_zoom),
        );
        let now = Instant::now();
        if last_publish.is_some_and(|last| now.duration_since(last) < PUBLISH_EVERY) {
            return;
        }
        let frame = parts.build(&pass);
        if frame != *shared.frame.latest() {
            last_publish = Some(now);
            shared.frame.publish(frame);
        }
    };
    crate::play::run_controller(
        ports,
        config,
        Some(library),
        &mut audio.engine,
        audio.stream.as_mut(),
        output,
        quit,
        &mut on_pass,
    );
    finish(printer, &lines)
}

/// Calls `attempt` every [`RETRY_EVERY`] until it gives a value or `quit` is set (`None`),
/// publishing the idle frame meanwhile.
fn retry<T>(
    quit: &AtomicBool,
    lines: &Receiver<Line>,
    parts: &mut FrameParts,
    shared: &Shared,
    mut attempt: impl FnMut(&mut FrameParts) -> Option<T>,
) -> Option<T> {
    loop {
        if quit.load(Ordering::Relaxed) {
            return None;
        }
        let value = attempt(parts);
        for line in lines.try_iter() {
            parts.take(line);
        }
        parts.take_zoom(shared.zoom.swap(0, Ordering::Relaxed), None);
        shared.frame.publish(parts.idle());
        if value.is_some() {
            return value;
        }
        let began = Instant::now();
        while began.elapsed() < RETRY_EVERY {
            if quit.load(Ordering::Relaxed) {
                return None;
            }
            std::thread::sleep(QUIT_CHECK);
        }
    }
}

/// Stops the printer and prints the events nobody saw: the window is closed by now, and
/// they may say a cue or hotcue could not be saved.
fn finish(printer: Printer, lines: &Receiver<Line>) -> anyhow::Result<()> {
    printer.finish();
    for line in lines.try_iter() {
        if let Line::Event(text) = line {
            eprintln!("{}", crate::format::safe(text));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library_mode::testing;
    use engine::{Command, DeckCommand, DeckId, HotcueSlot};

    fn render(renderer: &mut engine::EngineRenderer) {
        let (mut master, mut headphones) = (vec![0.0; 1024], vec![0.0; 1024]);
        renderer.render(&mut master, &mut headphones);
    }

    /// The built-in mapping on a port nothing has, so a plugged-in controller stays out of
    /// the test.
    fn mapping() -> Mapping {
        let mut mapping = controller::builtin::load_mapping_from("ddj-sb3", None).unwrap();
        mapping.controller.port = "open-jockey test: no such port".to_owned();
        mapping
    }

    #[test]
    fn a_bad_config_is_shown_and_keeps_the_window_open() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[beat_jump]\npads = [1, 2, 3]\n").unwrap();
        let shared = Shared {
            frame: Published::new(ControlFrame::default()),
            monitor: Published::new(None),
            zoom: Arc::new(AtomicI32::new(0)),
            state_path: None,
        };
        let quit = AtomicBool::new(false);

        assert!(session(Some(&path), &shared, &quit).is_err());

        let frame = shared.frame.latest();
        let error = frame.error.as_deref().expect("the frame carries the error");
        assert!(
            error.contains("config.toml: beat_jump.pads: expected exactly 8 numbers, got 3"),
            "{error}"
        );
        assert!(!quit.load(Ordering::Relaxed));
    }

    #[test]
    fn closing_saves_a_hotcue() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("library.sqlite3");
        let mut library = testing::start(&db);
        testing::add_track_7(&library);
        let (mut engine, mut renderer) = engine::engine(engine::EngineConfig {
            sample_rate: 44_100,
            max_block_frames: 512,
        });
        let (printer, _events) = Printer::capturing();
        testing::load_7(&mut library, 0, &mut engine, &printer);
        printer.finish();
        render(&mut renderer);
        // Hotcue 1 set as the controller's pad sets it.
        let deck1 = DeckId::new(0).unwrap();
        let slot1 = HotcueSlot::new(1).unwrap();
        engine
            .send(Command::Deck(
                deck1,
                DeckCommand::SetHotcue(slot1, Some(100.0)),
            ))
            .unwrap();
        render(&mut renderer);

        let config = Config::default();
        let shared = Shared {
            frame: Published::new(ControlFrame::default()),
            monitor: Published::new(None),
            zoom: Arc::new(AtomicI32::new(0)),
            state_path: None,
        };
        let quit = Arc::new(AtomicBool::new(false));
        let session = {
            let (shared, quit) = (shared.clone(), Arc::clone(&quit));
            let mut fake = Some(engine);
            std::thread::spawn(move || {
                let open = || {
                    Ok(Audio {
                        engine: fake.take().expect("opened once"),
                        stream: None,
                        device: "test output".to_owned(),
                        output_latency: Duration::ZERO,
                    })
                };
                let mut start = Start {
                    samples: Vec::new(),
                    samples_loaded: [false; engine::SAMPLER_SLOTS],
                    events: Vec::new(),
                };
                run_session(
                    &config,
                    &shared,
                    &quit,
                    mapping(),
                    library,
                    &mut start,
                    open,
                )
            })
        };
        let began = Instant::now();
        while shared.frame.latest().audio.is_err() {
            assert!(
                began.elapsed() < Duration::from_secs(10),
                "audio never opened"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(shared.frame.latest().audio.as_deref(), Ok("test output"));
        assert!(shared.monitor.latest().is_some());
        quit.store(true, Ordering::Relaxed);
        session.join().unwrap().unwrap();

        let lib = crate::library_cmd::open_library(Some(&db)).unwrap();
        assert_eq!(lib.reader().unwrap().hotcues(7).unwrap()[0], Some(100.0));
        drop(renderer);
    }
}
