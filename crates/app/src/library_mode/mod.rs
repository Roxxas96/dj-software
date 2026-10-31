//! `play --library`: the browser line, loading library tracks, beatgrid edits, cue
//! write-through, background analysis and folder watching, driven from the control loop.
//!
//! The control loop never waits on the database or a decode: browser moves, lookups and
//! grid edits run on the library worker, decoding on its loader thread
//! ([`worker`]); cue points are written with the non-blocking `Writer::submit`.

mod cue_sync;
mod worker;

use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use actions::{AppRequest, GridEditRequest};
use analysis::KeyNotation;
use anyhow::Context;
use engine::{Command, DeckCommand, DeckId, DeckSnapshot, EngineController};
use library::{
    Analyzer, BrowserView, FailedWrite, GridEdit, KEY_NOTATION, Library, TrackRow, WatchHealth,
    Watcher, WriteOp, Writer, default_threads,
};

use crate::console::ConsoleCommand;
use crate::format::{bpm_text, grid_text, key_text, notation_from_setting, track_name};
use crate::printer::Printer;
use cue_sync::{CueAction, CueSync};
use worker::{Event, Job};

/// How often failed writes, analysis and the watcher are checked, and the most often the
/// browser is reread.
const POLL_EVERY: Duration = Duration::from_secs(1);
/// How many analysis failures or watcher problems are printed per check.
const REPORTS_SHOWN: usize = 5;

/// The library track on a deck.
struct DeckTrack {
    row: TrackRow,
    cue: CueSync,
}

pub struct LibraryMode {
    lib: Arc<Library>,
    writer: Writer,
    analyzer: Option<Analyzer>,
    /// Tracks queued for analysis since the start.
    analysis_total: usize,
    last_done: usize,
    watcher: Option<Watcher>,
    watch_scans: u64,
    watch_skipped: u64,
    degraded_reported: bool,
    jobs: Option<Sender<Job>>,
    events: Receiver<Event>,
    worker: Option<JoinHandle<()>>,
    decks: [Option<DeckTrack>; 4],
    load_seq: [u64; 4],
    notation: KeyNotation,
    browser: Option<BrowserView>,
    analysis_status: Option<String>,
    last_poll: Instant,
    refresh_wanted: bool,
    last_refresh: Option<Instant>,
    /// The status lines should be redrawn now.
    dirty: bool,
}

impl LibraryMode {
    /// Opens the library, queues the tracks not analyzed yet, watches the library folders
    /// and starts the worker. Prints what it does (before the control loop starts).
    pub fn start(db: Option<&Path>) -> anyhow::Result<Self> {
        let lib = Arc::new(crate::library_cmd::open_library(db)?);
        let reader = lib.reader()?;
        let notation = notation_from_setting(reader.setting(KEY_NOTATION)?.as_deref());

        let pending = reader.track_ids_to_analyze(false)?;
        let analyzer = if pending.is_empty() {
            None
        } else {
            let analyzer =
                Analyzer::start(&lib, default_threads()).context("starting the analyzer")?;
            analyzer.enqueue(&pending, false);
            println!("analyzing {} track(s) in the background", pending.len());
            Some(analyzer)
        };

        let folders = reader.folders()?;
        let watcher = if folders.is_empty() {
            None
        } else {
            match library::watch(&lib, &folders) {
                Ok(watcher) => Some(watcher),
                Err(e) => {
                    println!("warning: not watching the library folders: {e}");
                    None
                }
            }
        };

        let (events_tx, events) = channel();
        let (jobs, worker) =
            worker::spawn(Arc::clone(&lib), events_tx).context("starting the library worker")?;
        Ok(Self {
            writer: lib.writer(),
            lib,
            analyzer,
            analysis_total: pending.len(),
            last_done: 0,
            watcher,
            watch_scans: 0,
            watch_skipped: 0,
            degraded_reported: false,
            jobs: Some(jobs),
            events,
            worker: Some(worker),
            decks: [None, None, None, None],
            load_seq: [0; 4],
            notation,
            browser: None,
            analysis_status: None,
            last_poll: Instant::now(),
            refresh_wanted: false,
            last_refresh: None,
            dirty: true,
        })
    }

    fn send_job(&self, job: Job, printer: &Printer) {
        let sent = self
            .jobs
            .as_ref()
            .is_some_and(|jobs| jobs.send(job).is_ok());
        if !sent {
            printer.event("warning: the library worker stopped; library actions do nothing".into());
        }
    }

    /// Performs a request from a controller action.
    pub fn handle_request(
        &mut self,
        request: AppRequest,
        engine: &EngineController,
        printer: &Printer,
    ) {
        match request {
            AppRequest::Browser(req) => self.send_job(Job::Browse(req), printer),
            AppRequest::LoadSelected { deck } => {
                let seq = self.next_seq(deck);
                self.send_job(Job::LoadSelected { deck, seq }, printer);
            }
            AppRequest::Grid { deck, edit } => self.grid(deck, edit, engine, printer),
            _ => {}
        }
    }

    /// Performs a console `grid` or `load`.
    pub fn console(
        &mut self,
        command: ConsoleCommand,
        engine: &EngineController,
        printer: &Printer,
    ) {
        match command {
            ConsoleCommand::Grid { deck, edit } => self.grid(deck, edit, engine, printer),
            ConsoleCommand::Load { deck, track } => {
                let seq = self.next_seq(deck);
                self.send_job(Job::LoadTrack { deck, track, seq }, printer);
            }
            ConsoleCommand::Reconnect | ConsoleCommand::Help => {}
        }
    }

    fn next_seq(&mut self, deck: DeckId) -> u64 {
        self.load_seq[deck.index()] += 1;
        self.load_seq[deck.index()]
    }

    fn grid(
        &mut self,
        deck: DeckId,
        edit: GridEditRequest,
        engine: &EngineController,
        printer: &Printer,
    ) {
        let Some(track) = &self.decks[deck.index()] else {
            printer.event(format!(
                "Deck {}: load a library track to edit its beatgrid",
                deck.index() + 1
            ));
            return;
        };
        let snap = engine.deck(deck);
        let job = Job::Grid {
            deck,
            track: track.row.id,
            edit: grid_edit(edit, snap.track_sample_rate, snap.position_frames),
            playhead: Some(snap.position_frames),
        };
        self.send_job(job, printer);
    }

    /// Takes whether the status lines changed since the last call.
    pub fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }

    /// Called on every control-loop round: applies the worker's results, keeps cue points
    /// in step and, every second, reports failed writes, analysis and watcher news.
    pub fn poll(&mut self, now: Instant, engine: &mut EngineController, printer: &Printer) {
        while let Ok(event) = self.events.try_recv() {
            self.on_event(event, engine, printer);
        }
        self.sync_cues(engine, printer);
        if now.duration_since(self.last_poll) >= POLL_EVERY {
            self.last_poll = now;
            self.report_failed_writes(printer);
            self.poll_analysis(printer);
            self.poll_watcher(printer);
            let due = self
                .last_refresh
                .is_none_or(|last| now.duration_since(last) >= POLL_EVERY);
            if self.refresh_wanted && due {
                self.refresh_wanted = false;
                self.last_refresh = Some(now);
                let decks = DeckId::ALL
                    .into_iter()
                    .filter_map(|d| self.decks[d.index()].as_ref().map(|t| (d, t.row.id)))
                    .collect();
                self.send_job(Job::Refresh { decks }, printer);
            }
        }
    }

    fn on_event(&mut self, event: Event, engine: &mut EngineController, printer: &Printer) {
        match event {
            Event::Browser(view) => {
                self.browser = Some(view);
                self.dirty = true;
            }
            Event::Message(text) => printer.event(text),
            Event::Loaded(loaded) => self.load(*loaded, engine, printer),
            Event::Grid { deck, track, grid } => {
                if let Some(t) = self.decks[deck.index()]
                    .as_mut()
                    .filter(|t| t.row.id == track)
                {
                    t.row.bpm = grid.bpm;
                    printer.event(format!(
                        "Deck {} grid: {}",
                        deck.index() + 1,
                        grid_text(&grid, t.row.sample_rate)
                    ));
                }
                self.refresh_wanted = true;
            }
            Event::Track { deck, row } => {
                if let Some(t) = self.decks[deck.index()]
                    .as_mut()
                    .filter(|t| t.row.id == row.id)
                {
                    t.row = row;
                    self.dirty = true;
                }
            }
        }
    }

    fn load(&mut self, loaded: worker::Loaded, engine: &mut EngineController, printer: &Printer) {
        let deck = loaded.deck;
        let n = deck.index() + 1;
        if loaded.seq != self.load_seq[deck.index()] {
            return; // A later load for this deck was asked for.
        }
        // Save the outgoing track's last cue change first.
        self.sync_deck(deck, engine, printer);
        if engine.deck(deck).playing {
            printer.event(format!("warning: deck {n} was playing"));
        }
        let frames = loaded.buffer.frames();
        let rate = loaded.buffer.sample_rate();
        if engine
            .send(Command::Deck(deck, DeckCommand::Load(loaded.buffer)))
            .is_err()
        {
            printer.event(format!(
                "Deck {n}: cannot load {}: the engine command queue is full",
                track_name(&loaded.row)
            ));
            return;
        }
        printer.event(format!("Deck {n} ◂ {}", track_name(&loaded.row)));
        self.decks[deck.index()] = Some(DeckTrack {
            row: loaded.row,
            cue: CueSync::new(frames, rate, loaded.cue),
        });
        self.dirty = true;
    }

    fn sync_cues(&mut self, engine: &mut EngineController, printer: &Printer) {
        for deck in DeckId::ALL {
            self.sync_deck(deck, engine, printer);
        }
    }

    fn sync_deck(&mut self, deck: DeckId, engine: &mut EngineController, printer: &Printer) {
        let Some(track) = self.decks[deck.index()].as_mut() else {
            return;
        };
        match track.cue.step(&engine.deck(deck)) {
            CueAction::Nothing => {}
            CueAction::Restore(cue) => {
                if engine
                    .send(Command::Deck(deck, DeckCommand::SetCue(cue)))
                    .is_err()
                {
                    track.cue.restore_failed();
                }
            }
            CueAction::Save(frames) => {
                let op = WriteOp::SetCue {
                    track: track.row.id,
                    frames,
                };
                if let Err(e) = self.writer.submit(op) {
                    printer.event(format!(
                        "warning: could not save the cue point for {}: {e}",
                        track_name(&track.row)
                    ));
                }
                self.refresh_wanted |= !track.row.has_cues;
                track.row.has_cues = true;
            }
        }
    }

    fn report_failed_writes(&self, printer: &Printer) {
        for failed in self.lib.take_failed_writes() {
            printer.event(self.failed_write_text(&failed));
        }
    }

    fn failed_write_text(&self, failed: &FailedWrite) -> String {
        let (what, track) = match &failed.op {
            WriteOp::SetCue { track, .. } => ("the cue point", Some(*track)),
            WriteOp::EditGrid { track, .. } => ("the beatgrid", Some(*track)),
            WriteOp::StoreAnalysis(a) => ("the analysis", Some(a.track_id)),
            _ => ("a library change", None),
        };
        let name = track.map(|id| {
            self.decks
                .iter()
                .flatten()
                .find(|t| t.row.id == id)
                .map_or_else(|| format!("track {id}"), |t| track_name(&t.row))
        });
        match name {
            Some(name) => format!(
                "warning: could not save {what} for {name}: {}",
                failed.error
            ),
            None => format!("warning: could not save {what}: {}", failed.error),
        }
    }

    fn poll_analysis(&mut self, printer: &Printer) {
        let Some(analyzer) = &self.analyzer else {
            return;
        };
        let p = analyzer.progress();
        let finished = p.done + p.skipped + p.failed_count;
        let status = (p.queued > 0).then(|| {
            format!(
                "analyzing {}/{}",
                finished.min(self.analysis_total),
                self.analysis_total
            )
        });
        if status != self.analysis_status {
            self.analysis_status = status;
            self.dirty = true;
        }
        if p.done != self.last_done {
            self.last_done = p.done;
            self.refresh_wanted = true;
        }
        let failures = analyzer.take_failures();
        for (id, why) in failures.iter().take(REPORTS_SHOWN) {
            printer.event(format!("analysis failed for track {id}: {why}"));
        }
        if failures.len() > REPORTS_SHOWN {
            printer.event(format!(
                "… and {} more analysis failures (`dj-software library analyze` lists them)",
                failures.len() - REPORTS_SHOWN
            ));
        }
        for warning in analyzer.take_warnings().iter().take(REPORTS_SHOWN) {
            printer.event(format!("warning: {warning}"));
        }
    }

    fn poll_watcher(&mut self, printer: &Printer) {
        let Some(watcher) = &self.watcher else {
            return;
        };
        if let WatchHealth::Degraded { reason } = watcher.health()
            && !self.degraded_reported
        {
            self.degraded_reported = true;
            printer.event(format!(
                "library watching degraded: {reason} (rescanning every 10 minutes)"
            ));
        }
        // The health report covers watch errors; empty the queue so it never grows.
        let _ = watcher.errors().try_iter().count();
        let scans = watcher.scan_count();
        if scans != self.watch_scans {
            self.watch_scans = scans;
            self.refresh_wanted = true;
        }
        let skipped = watcher.skipped_count();
        if skipped != self.watch_skipped {
            let new = usize::try_from(skipped - self.watch_skipped).unwrap_or(usize::MAX);
            self.watch_skipped = skipped;
            let recent = watcher.skipped();
            let shown = new.min(REPORTS_SHOWN).min(recent.len());
            for (path, why) in &recent[recent.len() - shown..] {
                printer.event(format!("library: could not scan {}: {why}", path.display()));
            }
        }
    }

    /// The browser line, then one line per loaded deck.
    pub fn status_lines(&self, engine: &EngineController) -> Vec<String> {
        let mut lines = vec![browser_line(
            self.browser.as_ref(),
            self.notation,
            self.analysis_status.as_deref(),
        )];
        for deck in DeckId::ALL {
            let snap = engine.deck(deck);
            if snap.loaded {
                let track = self.decks[deck.index()].as_ref().map(|t| &t.row);
                lines.push(deck_status(deck, &snap, track));
            }
        }
        lines
    }

    /// Saves the last cue changes, stops analysis, watching and the worker, waits for
    /// queued writes, and reports any that failed.
    pub fn shutdown(mut self, engine: &mut EngineController, printer: &Printer) {
        self.sync_cues(engine, printer);
        self.analyzer = None;
        self.watcher = None;
        self.jobs = None;
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        // The writer applies ops in order: once this one is done, every cue queued
        // before it has been written (or has failed).
        let _ = self.writer.submit_wait(WriteOp::Batch(Vec::new()));
        for event in self.events.try_iter() {
            if let Event::Message(text) = event {
                printer.event(text);
            }
        }
        self.report_failed_writes(printer);
    }
}

/// A grid edit as the library takes it: the shift in frames at the track's rate, and the
/// downbeat at the playhead.
fn grid_edit(edit: GridEditRequest, sample_rate: u32, playhead: f64) -> GridEdit {
    match edit {
        GridEditRequest::Shift { millis } => GridEdit::Shift {
            frames: f64::from(millis) / 1000.0 * f64::from(sample_rate),
        },
        GridEditRequest::SetDownbeat => GridEdit::SetDownbeat {
            at_frames: playhead,
        },
        GridEditRequest::Halve => GridEdit::Halve,
        GridEditRequest::Double => GridEdit::Double,
        GridEditRequest::Tap { at_micros } => GridEdit::Tap { at_micros },
        GridEditRequest::ToggleLock => GridEdit::ToggleLock,
    }
}

/// `▸ Playlists ▸ Peak time  12/48  Artist – Title  128.00 BPM  8A  •`, with the
/// analysis progress after it.
fn browser_line(
    view: Option<&BrowserView>,
    notation: KeyNotation,
    analysis: Option<&str>,
) -> String {
    let mut line = match view {
        None => "▸ reading the library…".to_owned(),
        Some(view) => {
            let index = if view.count == 0 { 0 } else { view.index + 1 };
            let mut line = format!("▸ {}  {index}/{}", view.path, view.count);
            if let Some(track) = &view.track {
                line.push_str(&format!("  {}", track_name(track)));
                if let Some(bpm) = track.bpm {
                    line.push_str(&format!("  {} BPM", bpm_text(bpm)));
                }
                let key = key_text(track.key, notation);
                if !key.is_empty() {
                    line.push_str(&format!("  {key}"));
                }
                if track.has_cues {
                    line.push_str("  •");
                }
                if track.missing {
                    line.push_str("  missing");
                }
            }
            line
        }
    };
    if let Some(analysis) = analysis {
        line.push_str(&format!("   {analysis}"));
    }
    line
}

/// A deck's line: time, BPM at the current tempo once analyzed, and the track.
fn deck_status(deck: DeckId, snap: &DeckSnapshot, track: Option<&TrackRow>) -> String {
    let mark = if snap.playing { '>' } else { ' ' };
    let mut line = format!("{mark}{}", crate::play::deck_line(deck, snap));
    if let Some(track) = track {
        if let Some(bpm) = track.bpm {
            line.push_str(&format!("  {} BPM", bpm_text(bpm * snap.tempo_ratio)));
        }
        line.push_str(&format!("  {}", track_name(track)));
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;
    use library::Focus;

    fn row() -> TrackRow {
        TrackRow {
            id: 7,
            title: Some("Title".into()),
            artist: Some("Artist".into()),
            bpm: Some(128.0),
            key: Some(21),
            duration_frames: Some(44_100 * 200),
            sample_rate: Some(44_100),
            has_cues: true,
            missing: false,
        }
    }

    #[test]
    fn browser_line_matches_the_spec() {
        let view = BrowserView {
            path: "Playlists ▸ Peak time".into(),
            index: 11,
            count: 48,
            track: Some(row()),
            focus: Focus::List,
        };
        assert_eq!(
            browser_line(Some(&view), KeyNotation::Camelot, None),
            "▸ Playlists ▸ Peak time  12/48  Artist – Title  128.00 BPM  8A  •"
        );
        let empty = BrowserView {
            path: "All tracks".into(),
            index: 0,
            count: 0,
            track: None,
            focus: Focus::Tree,
        };
        assert_eq!(
            browser_line(Some(&empty), KeyNotation::Camelot, Some("analyzing 12/340")),
            "▸ All tracks  0/0   analyzing 12/340"
        );
    }

    #[test]
    fn deck_status_scales_bpm_by_tempo() {
        let snap = DeckSnapshot {
            loaded: true,
            playing: true,
            position_frames: 44_100.0 * 65.0,
            duration_frames: 44_100 * 200,
            track_sample_rate: 44_100,
            tempo_ratio: 1.05,
            cue_frames: 0.0,
            previewing: false,
        };
        let deck = DeckId::new(2).unwrap();
        assert_eq!(
            deck_status(deck, &snap, Some(&row())),
            ">Deck 3  01:05 / 03:20  134.40 BPM  Artist – Title"
        );
        let unanalyzed = TrackRow { bpm: None, ..row() };
        assert_eq!(
            deck_status(deck, &snap, Some(&unanalyzed)),
            ">Deck 3  01:05 / 03:20  Artist – Title"
        );
    }

    #[test]
    fn grid_requests_get_the_rate_and_playhead() {
        assert_eq!(
            grid_edit(GridEditRequest::Shift { millis: -5 }, 48_000, 0.0),
            GridEdit::Shift { frames: -240.0 }
        );
        assert_eq!(
            grid_edit(GridEditRequest::SetDownbeat, 44_100, 1234.5),
            GridEdit::SetDownbeat { at_frames: 1234.5 }
        );
        assert_eq!(
            grid_edit(GridEditRequest::Tap { at_micros: 9 }, 44_100, 0.0),
            GridEdit::Tap { at_micros: 9 }
        );
        assert_eq!(
            grid_edit(GridEditRequest::ToggleLock, 44_100, 0.0),
            GridEdit::ToggleLock
        );
    }
}
