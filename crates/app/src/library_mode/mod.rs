//! `play --library`: the browser line, loading library tracks with their cue point,
//! hotcues and beatgrid, beatgrid edits, cue and hotcue write-through, background analysis
//! and folder watching, driven from the control loop.
//!
//! The control loop never waits on the database or a decode: browser moves, lookups and
//! grid edits run on the library worker, decoding on its loader thread
//! ([`worker`]); cue points and hotcues are written with the non-blocking `Writer::submit`.

mod memory_sync;
mod worker;

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use actions::{AppRequest, GridEditRequest};
use analysis::KeyNotation;
use anyhow::Context;
use engine::{Command, DeckCommand, DeckId, DeckSnapshot, EngineController, Grid};
use library::{
    Analyzer, BrowserView, FailedWrite, GridEdit, KEY_NOTATION, Library, PendingTrack, TrackRow,
    WatchHealth, Watcher, WriteOp, Writer, default_threads,
};

use crate::console::ConsoleCommand;
use crate::format::{bpm_text, grid_text, key_text, notation_from_setting, safe, track_name};
use crate::printer::Printer;
use memory_sync::{MemoryAction, MemorySync, engine_grid};
use worker::{Event, Job};

/// How often failed writes, analysis and the watcher are checked, and the most often the
/// browser is reread.
const POLL_EVERY: Duration = Duration::from_secs(1);
/// How many analysis failures or watcher problems are printed per check.
const REPORTS_SHOWN: usize = 5;
/// How many analysis workers may run while a deck is playing, so analysis cannot starve
/// the audio.
const WORKERS_WHILE_PLAYING: usize = 1;
/// At shutdown, how many times a deck's snapshot is read before its last cue and hotcue
/// changes are given up on, and the pause between reads (the renderer publishes one per
/// block, so a read fails only while it is storing one).
const SHUTDOWN_READ_ATTEMPTS: usize = 50;
const SHUTDOWN_READ_PAUSE: Duration = Duration::from_millis(1);

/// A file's size and mtime as last scanned: when it changes, the track is analyzed again.
type FileStamp = (Option<i64>, Option<i64>);

/// The library track on a deck.
struct DeckTrack {
    row: TrackRow,
    memory: MemorySync,
    /// The grid last sent to the deck (with the `Load` or a `SetGrid`).
    grid: Option<Grid>,
}

pub struct LibraryMode {
    lib: Arc<Library>,
    writer: Writer,
    analyzer: Option<Analyzer>,
    /// Tracks queued for analysis since the start, with their file as it was then. A
    /// track is queued again only when its file changed since (say, a copy that was
    /// still in progress completed); one that fails on the same file is not retried
    /// until the next session or `library analyze`.
    enqueued: HashMap<i64, FileStamp>,
    /// How many analysis jobs were queued since the start.
    queued_total: usize,
    /// Analysis is held to [`WORKERS_WHILE_PLAYING`] because a deck is playing.
    throttled: bool,
    /// The last value saved this session for each cue point and hotcue, as frames bits
    /// (`None`: the hotcue was deleted).
    latest_memory: HashMap<(i64, Spot), Option<u64>>,
    /// Cue and hotcue writes already retried once, as (track, spot, value).
    retried_memory: HashSet<(i64, Spot, Option<u64>)>,
    /// The watcher scanned something: look for tracks to analyze at the next refresh.
    find_new_tracks: bool,
    last_done: usize,
    watcher: Option<Watcher>,
    watch_scans: u64,
    watch_skipped: u64,
    degraded_reported: bool,
    jobs: Option<Sender<Job>>,
    events: Receiver<Event>,
    worker: Option<JoinHandle<()>>,
    decks: [Option<DeckTrack>; 4],
    /// The track each deck held before its latest `Load`, still saved until the engine
    /// applies the load that replaced it: until the deck's `load_seq` passes the second
    /// value.
    outgoing: [Option<(DeckTrack, u64)>; 4],
    /// The deck's `load_seq` once the engine has applied the latest `Load` sent.
    engine_loads: [u64; 4],
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
    /// A read-only connection to the library.
    pub fn reader(&self) -> Result<library::Reader, library::LibraryError> {
        self.lib.reader()
    }

    /// Opens the library, queues the tracks not analyzed yet, watches the library folders
    /// and starts the worker. Prints what it does (before the control loop starts).
    pub fn start(db: Option<&Path>) -> anyhow::Result<Self> {
        let lib = Arc::new(crate::library_cmd::open_library(db)?);
        let reader = lib.reader()?;
        let notation = notation_from_setting(reader.setting(KEY_NOTATION)?.as_deref());

        let pending = reader.tracks_to_analyze()?;
        // Started even with nothing to do: the watcher may find new tracks.
        let analyzer = Analyzer::start(&lib, default_threads()).context("starting the analyzer")?;
        if !pending.is_empty() {
            println!("analyzing {} track(s) in the background", pending.len());
        }

        let folders = reader.folders()?;
        let watcher = if folders.is_empty() {
            None
        } else {
            match library::watch(&lib, &folders) {
                Ok(watcher) => Some(watcher),
                Err(e) => {
                    println!("warning: not watching the library folders: {}", safe(e));
                    None
                }
            }
        };

        let (events_tx, events) = channel();
        let (jobs, worker) =
            worker::spawn(Arc::clone(&lib), events_tx).context("starting the library worker")?;
        let mut mode = Self {
            writer: lib.writer(),
            lib,
            analyzer: Some(analyzer),
            enqueued: HashMap::new(),
            queued_total: 0,
            throttled: false,
            latest_memory: HashMap::new(),
            retried_memory: HashSet::new(),
            find_new_tracks: false,
            last_done: 0,
            watcher,
            watch_scans: 0,
            watch_skipped: 0,
            degraded_reported: false,
            jobs: Some(jobs),
            events,
            worker: Some(worker),
            decks: [None, None, None, None],
            outgoing: [None, None, None, None],
            engine_loads: [0; 4],
            load_seq: [0; 4],
            notation,
            browser: None,
            analysis_status: None,
            last_poll: Instant::now(),
            refresh_wanted: false,
            last_refresh: None,
            dirty: true,
        };
        mode.enqueue_new(&pending);
        Ok(mode)
    }

    /// Queues for analysis the tracks of `tracks` not queued yet this session, or whose
    /// file changed since they were.
    fn enqueue_new(&mut self, tracks: &[PendingTrack]) {
        let new: Vec<i64> = tracks
            .iter()
            .filter(|t| self.enqueued.insert(t.id, (t.size, t.mtime)) != Some((t.size, t.mtime)))
            .map(|t| t.id)
            .collect();
        if let (Some(analyzer), false) = (&self.analyzer, new.is_empty()) {
            self.queued_total += new.len();
            analyzer.enqueue(&new, false);
        }
    }

    /// Holds analysis to [`WORKERS_WHILE_PLAYING`] while a deck plays, and lets every
    /// worker run again once all decks are paused or stopped.
    fn set_playing(&mut self, playing: bool) {
        if playing == self.throttled {
            return;
        }
        self.throttled = playing;
        if let Some(analyzer) = &self.analyzer {
            analyzer.set_max_active(if playing {
                WORKERS_WHILE_PLAYING
            } else {
                usize::MAX
            });
        }
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
        match self.grid_job(deck, edit, engine) {
            Ok(job) => self.send_job(job, printer),
            Err(message) => printer.event(message),
        }
    }

    /// The worker job for a grid edit on `deck`, or why it is refused: no library track,
    /// or the track still loading (the playhead would be the previous track's). A shift
    /// is converted to frames at the decoded track's rate.
    fn grid_job(
        &self,
        deck: DeckId,
        edit: GridEditRequest,
        engine: &EngineController,
    ) -> Result<Job, String> {
        let n = deck.index() + 1;
        let Some(track) = &self.decks[deck.index()] else {
            return Err(format!(
                "Deck {n}: load a library track to edit its beatgrid"
            ));
        };
        if !track.memory.load_applied() {
            return Err(format!(
                "Deck {n}: the track is still loading; edit its beatgrid in a moment"
            ));
        }
        let rate = track.memory.rate();
        let playhead = engine.deck(deck).position_frames;
        Ok(Job::Grid {
            deck,
            track: track.row.id,
            edit: grid_edit(edit, rate, playhead),
            playhead: Some(playhead),
            sample_rate: Some(rate),
        })
    }

    /// Takes whether the status lines changed since the last call.
    pub fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }

    /// Called on every control-loop round: applies the worker's results, keeps cue points
    /// and hotcues in step and, every second, reports failed writes, analysis and watcher news.
    pub fn poll(&mut self, now: Instant, engine: &mut EngineController, printer: &Printer) {
        while let Ok(event) = self.events.try_recv() {
            self.on_event(event, engine, printer);
        }
        self.sync_cues(engine, printer);
        self.set_playing(DeckId::ALL.into_iter().any(|d| engine.deck(d).playing));
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
                let find_new_tracks = std::mem::take(&mut self.find_new_tracks);
                self.send_job(
                    Job::Refresh {
                        decks,
                        find_new_tracks,
                    },
                    printer,
                );
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
                    .as_ref()
                    .filter(|t| t.row.id == track)
                {
                    printer.event(format!(
                        "Deck {} grid: {}",
                        deck.index() + 1,
                        grid_text(&grid, Some(t.memory.rate()))
                    ));
                }
                for t in self
                    .decks
                    .iter_mut()
                    .flatten()
                    .filter(|t| t.row.id == track)
                {
                    t.row.bpm = grid.bpm;
                }
                self.sync_grid(track, Some(&grid), engine);
                self.refresh_wanted = true;
            }
            Event::NewTracks(ids) => self.enqueue_new(&ids),
            Event::Track { deck, row, grid } => {
                let id = row.id;
                if let Some(t) = self.decks[deck.index()].as_mut().filter(|t| t.row.id == id) {
                    t.row = row;
                    self.dirty = true;
                    // Say, an analysis that finished after the track was loaded.
                    self.sync_grid(id, grid.as_ref(), engine);
                }
            }
        }
    }

    /// Sends `grid` (the library's grid for `track`) to every deck holding the track,
    /// unless they all have it already (a `SetGrid` drops a pending phase sync). If the
    /// engine queue is full, a refresh is asked for, which sends it again.
    fn sync_grid(
        &mut self,
        track: i64,
        grid: Option<&library::Beatgrid>,
        engine: &mut EngineController,
    ) {
        let holding = || self.decks.iter().flatten().filter(|t| t.row.id == track);
        // Every deck holding the track decoded the same file, at the same rate.
        let Some(rate) = holding().next().map(|t| t.memory.rate()) else {
            return;
        };
        let grid = engine_grid(grid, rate);
        if holding().all(|t| t.grid == grid) {
            return;
        }
        let loaded = self.decks.each_ref().map(|t| t.as_ref().map(|t| t.row.id));
        if engine
            .send_all(&grid_commands(track, grid, &loaded))
            .is_err()
        {
            self.refresh_wanted = true;
            return;
        }
        for t in self
            .decks
            .iter_mut()
            .flatten()
            .filter(|t| t.row.id == track)
        {
            t.grid = grid;
        }
    }

    fn load(&mut self, loaded: worker::Loaded, engine: &mut EngineController, printer: &Printer) {
        let deck = loaded.deck;
        let n = deck.index() + 1;
        if loaded.seq != self.load_seq[deck.index()] {
            return; // A later load for this deck was asked for.
        }
        // Save the outgoing track's last cue and hotcue changes first.
        self.sync_deck(deck, engine, printer);
        if engine.deck(deck).playing {
            printer.event(format!("warning: deck {n} was playing"));
        }
        let frames = loaded.buffer.frames();
        let rate = loaded.buffer.sample_rate();
        if engine
            .send(Command::Deck(
                deck,
                DeckCommand::Load {
                    track: loaded.buffer,
                    memory: loaded.memory,
                },
            ))
            .is_err()
        {
            printer.event(format!(
                "Deck {n}: cannot load {}: the engine command queue is full",
                track_name(&loaded.row)
            ));
            return;
        }
        printer.event(format!("Deck {n} ◂ {}", track_name(&loaded.row)));
        let i = deck.index();
        // Counted from what the engine shows and from the loads sent before, in case one
        // of those is not applied yet.
        let after = engine.deck(deck).load_seq.max(self.engine_loads[i]);
        self.engine_loads[i] = after + 1;
        let mut memory = MemorySync::new(frames, rate, loaded.memory, after);
        if !loaded.hotcues_read {
            memory = memory.without_hotcue_saves();
        }
        let incoming = DeckTrack {
            row: loaded.row,
            memory,
            grid: loaded.memory.clamped(frames).grid,
        };
        // The outgoing track's changes keep being saved until the engine applies this load.
        // A track whose own load never showed had none: the earlier outgoing track then
        // stays, until the load that replaced it shows.
        if let Some(old) = self.decks[i].replace(incoming)
            && old.memory.load_applied()
        {
            self.outgoing[i] = Some((old, after));
        }
        // A grid edited while the track was decoding reaches the deck at the refresh.
        self.refresh_wanted = true;
        self.dirty = true;
    }

    fn sync_cues(&mut self, engine: &mut EngineController, printer: &Printer) {
        for deck in DeckId::ALL {
            self.sync_deck(deck, engine, printer);
        }
    }

    /// Saves the cue and hotcue changes on `deck`. Only a snapshot from one block is used
    /// (Ruling R14): a mixed one could show one track's load count with another's cue
    /// points. If none can be read now, the next round tries again.
    fn sync_deck(&mut self, deck: DeckId, engine: &mut EngineController, printer: &Printer) {
        if let Some(snap) = engine.deck_consistent(deck) {
            let saved = self.sync_snapshot(deck, &snap, printer);
            self.push_hotcues(deck, &saved, engine, printer);
        }
    }

    /// Sends the hotcue changes saved from `from` (as (track, change)) to every other deck
    /// holding the same track whose load has applied, and has its [`MemorySync`] expect them
    /// so they are not saved back (Ruling R18): otherwise that deck's old pads would
    /// overwrite the new value at its next change.
    fn push_hotcues(
        &mut self,
        from: DeckId,
        saved: &[(i64, MemoryAction)],
        engine: &mut EngineController,
        printer: &Printer,
    ) {
        for &(track, action) in saved {
            let (slot, value) = match action {
                MemoryAction::SaveHotcue(slot, frames) => (slot, Some(frames)),
                MemoryAction::DeleteHotcue(slot) => (slot, None),
                MemoryAction::SaveCue(_) => continue,
            };
            for deck in DeckId::ALL.into_iter().filter(|&d| d != from) {
                let Some(other) = self.decks[deck.index()]
                    .as_mut()
                    .filter(|t| t.row.id == track && t.memory.wants_hotcue(slot, value))
                else {
                    continue;
                };
                let command = Command::Deck(deck, DeckCommand::SetHotcue(slot, value));
                if engine.send(command).is_ok() {
                    other.memory.pushed_hotcue(slot, value);
                } else {
                    printer.event(format!(
                        "warning: deck {}: hotcue {} changed on deck {} but the engine command \
                         queue is full; reload the track to see it",
                        deck.index() + 1,
                        slot.number(),
                        from.index() + 1
                    ));
                }
            }
        }
    }

    /// Saves the cue and hotcue changes on `deck` from a consistent snapshot, which `read`
    /// returns or not ([`EngineController::deck_consistent`]), trying up to
    /// [`SHUTDOWN_READ_ATTEMPTS`] times [`SHUTDOWN_READ_PAUSE`] apart: there is no next round
    /// at shutdown. False if no snapshot could be read.
    fn sync_deck_patiently(
        &mut self,
        deck: DeckId,
        mut read: impl FnMut() -> Option<DeckSnapshot>,
        printer: &Printer,
    ) -> bool {
        for attempt in 0..SHUTDOWN_READ_ATTEMPTS {
            if attempt > 0 {
                std::thread::sleep(SHUTDOWN_READ_PAUSE);
            }
            if let Some(snap) = read() {
                // The engine stops next: other decks need no hotcue pushes.
                self.sync_snapshot(deck, &snap, printer);
                return true;
            }
        }
        false
    }

    /// Saves the changes `snap` (one block's snapshot) shows to the track on `deck`, and to
    /// the outgoing track while `snap` does not show the load that replaced it. Returns
    /// what was saved, with the track it was saved for.
    fn sync_snapshot(
        &mut self,
        deck: DeckId,
        snap: &DeckSnapshot,
        printer: &Printer,
    ) -> Vec<(i64, MemoryAction)> {
        let i = deck.index();
        let mut saved = Vec::new();
        let Some(mut current) = self.decks[i].take() else {
            return saved;
        };
        match self.outgoing[i].take() {
            // Once a later load shows, the deck never holds that track again.
            Some((_, replaced_after)) if snap.load_seq > replaced_after => {}
            Some((mut old, replaced_after)) => {
                let actions = old.memory.step(snap);
                self.save_memory(&mut old, &actions, printer);
                saved.extend(actions.into_iter().map(|a| (old.row.id, a)));
                self.outgoing[i] = Some((old, replaced_after));
            }
            None => {}
        }
        let actions = current.memory.step(snap);
        self.save_memory(&mut current, &actions, printer);
        saved.extend(actions.into_iter().map(|a| (current.row.id, a)));
        self.decks[i] = Some(current);
        saved
    }

    /// Writes `actions` for `track` through to the library.
    fn save_memory(&mut self, track: &mut DeckTrack, actions: &[MemoryAction], printer: &Printer) {
        let id = track.row.id;
        for &action in actions {
            let op = match action {
                MemoryAction::SaveCue(frames) => WriteOp::SetCue { track: id, frames },
                MemoryAction::SaveHotcue(slot, frames) => WriteOp::SetHotcue {
                    track: id,
                    slot: slot.number(),
                    frames,
                },
                MemoryAction::DeleteHotcue(slot) => WriteOp::DeleteHotcue {
                    track: id,
                    slot: slot.number(),
                },
            };
            if let Some((track, spot, value)) = memory_write(&op) {
                self.latest_memory.insert((track, spot), value);
                self.retried_memory
                    .retain(|&(t, s, _)| (t, s) != (track, spot));
            }
            let what = write_text(&op);
            if let Err(e) = self.writer.submit(op) {
                printer.event(format!(
                    "warning: could not {what} for {}: {}",
                    track_name(&track.row),
                    safe(e)
                ));
            }
            // The browser's `•`: a cue above 0 or any hotcue. A cue moved to 0 or a deleted
            // hotcue may leave none, so the row is reread.
            match action {
                MemoryAction::SaveCue(frames) if frames <= 0.0 => self.refresh_wanted = true,
                MemoryAction::DeleteHotcue(_) => self.refresh_wanted = true,
                _ => {
                    self.refresh_wanted |= !track.row.has_cues;
                    track.row.has_cues = true;
                }
            }
        }
    }

    /// Reports the writes that failed. A failed cue or hotcue write that is still the
    /// latest for its cue point or hotcue slot is retried once first, silently (Ruling
    /// R13); it is reported if the retry fails too. A stale one is never retried, so it
    /// cannot overwrite a newer value.
    fn report_failed_writes(&mut self, printer: &Printer) {
        for failed in self.lib.take_failed_writes() {
            if let Some((track, spot, value)) = memory_write(&failed.op)
                && self.latest_memory.get(&(track, spot)) == Some(&value)
                && self.retried_memory.insert((track, spot, value))
                && self.writer.submit(failed.op.clone()).is_ok()
            {
                continue;
            }
            printer.event(self.failed_write_text(&failed));
        }
    }

    fn failed_write_text(&self, failed: &FailedWrite) -> String {
        let track = match &failed.op {
            WriteOp::SetCue { track, .. }
            | WriteOp::SetHotcue { track, .. }
            | WriteOp::DeleteHotcue { track, .. }
            | WriteOp::EditGrid { track, .. } => Some(*track),
            WriteOp::StoreAnalysis(a) => Some(a.track_id),
            _ => None,
        };
        let what = write_text(&failed.op);
        let name = track.map(|id| {
            self.decks
                .iter()
                .flatten()
                .chain(self.outgoing.iter().flatten().map(|(t, _)| t))
                .find(|t| t.row.id == id)
                .map_or_else(|| format!("track {id}"), |t| track_name(&t.row))
        });
        match name {
            Some(name) => format!(
                "warning: could not {what} for {name}: {}",
                safe(&failed.error)
            ),
            None => format!("warning: could not {what}: {}", safe(&failed.error)),
        }
    }

    fn poll_analysis(&mut self, printer: &Printer) {
        let Some(analyzer) = &self.analyzer else {
            return;
        };
        let p = analyzer.progress();
        let finished = p.done + p.skipped + p.failed_count;
        let total = self.queued_total;
        let status = (p.queued > 0).then(|| format!("analyzing {}/{total}", finished.min(total)));
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
            printer.event(format!("analysis failed for track {id}: {}", safe(why)));
        }
        if failures.len() > REPORTS_SHOWN {
            printer.event(format!(
                "… and {} more analysis failures (`open-jockey library analyze` lists them)",
                failures.len() - REPORTS_SHOWN
            ));
        }
        let warnings = analyzer.take_warnings();
        for warning in warnings.iter().take(REPORTS_SHOWN) {
            printer.event(format!("warning: {}", safe(warning)));
        }
        if warnings.len() > REPORTS_SHOWN {
            printer.event(format!(
                "… and {} more analysis warnings",
                warnings.len() - REPORTS_SHOWN
            ));
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
                "library watching degraded: {} (rescanning every 10 minutes)",
                safe(reason)
            ));
        }
        // The health report covers watch errors; empty the queue so it never grows.
        let _ = watcher.errors().try_iter().count();
        let scans = watcher.scan_count();
        if scans != self.watch_scans {
            self.watch_scans = scans;
            self.refresh_wanted = true;
            self.find_new_tracks = true;
        }
        let skipped = watcher.skipped_count();
        if skipped != self.watch_skipped {
            let new =
                usize::try_from(skipped.saturating_sub(self.watch_skipped)).unwrap_or(usize::MAX);
            self.watch_skipped = skipped;
            let recent = watcher.skipped();
            let shown = new.min(REPORTS_SHOWN).min(recent.len());
            for (path, why) in &recent[recent.len() - shown..] {
                printer.event(format!(
                    "library: could not scan {}: {}",
                    safe(path.display()),
                    safe(why)
                ));
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

    /// Saves the last cue and hotcue changes, stops analysis, watching and the worker, waits for
    /// queued writes, and reports any that failed.
    pub fn shutdown(mut self, engine: &mut EngineController, printer: &Printer) {
        for deck in DeckId::ALL {
            if !self.sync_deck_patiently(deck, || engine.deck_consistent(deck), printer) {
                printer.event(format!(
                    "warning: deck {}: could not read its last cue and hotcue changes to save them",
                    deck.index() + 1
                ));
            }
        }
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
        // A cue write retried just now: wait for it, and report it if it failed again.
        let _ = self.writer.submit_wait(WriteOp::Batch(Vec::new()));
        self.report_failed_writes(printer);
    }
}

/// A cue point or one hotcue slot of a track.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Spot {
    Cue,
    Hotcue(u8),
}

/// The track, spot and value (frames bits, `None` for a deletion) a cue or hotcue write
/// stores; `None` for other writes.
fn memory_write(op: &WriteOp) -> Option<(i64, Spot, Option<u64>)> {
    match *op {
        WriteOp::SetCue { track, frames } => Some((track, Spot::Cue, Some(frames.to_bits()))),
        WriteOp::SetHotcue {
            track,
            slot,
            frames,
        } => Some((track, Spot::Hotcue(slot), Some(frames.to_bits()))),
        WriteOp::DeleteHotcue { track, slot } => Some((track, Spot::Hotcue(slot), None)),
        _ => None,
    }
}

/// What a write does, for "could not …": `save hotcue 3`, `delete hotcue 3`.
fn write_text(op: &WriteOp) -> String {
    match op {
        WriteOp::SetCue { .. } => "save the cue point".to_owned(),
        WriteOp::SetHotcue { slot, .. } => format!("save hotcue {slot}"),
        WriteOp::DeleteHotcue { slot, .. } => format!("delete hotcue {slot}"),
        WriteOp::EditGrid { .. } => "save the beatgrid".to_owned(),
        WriteOp::StoreAnalysis(_) => "save the analysis".to_owned(),
        _ => "save a library change".to_owned(),
    }
}

/// `SetGrid(grid)` for every deck whose loaded library track (`loaded`, by deck index) is
/// `track`.
fn grid_commands(track: i64, grid: Option<Grid>, loaded: &[Option<i64>; 4]) -> Vec<Command> {
    DeckId::ALL
        .into_iter()
        .filter(|d| loaded[d.index()] == Some(track))
        .map(|d| Command::Deck(d, DeckCommand::SetGrid(grid)))
        .collect()
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
            ..DeckSnapshot::default()
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
    fn grid_commands_target_every_deck_holding_the_track() {
        // `Command` holds track buffers, so it has no `PartialEq`: compare what matters.
        fn targets(commands: Vec<Command>) -> Vec<(usize, Option<Grid>)> {
            commands
                .into_iter()
                .map(|c| match c {
                    Command::Deck(deck, DeckCommand::SetGrid(grid)) => (deck.index(), grid),
                    other => panic!("{other:?}"),
                })
                .collect()
        }
        let grid = Grid::from_bpm(120.0, 0.0, 44_100);
        let loaded = [Some(7), Some(8), Some(7), None];
        assert_eq!(
            targets(grid_commands(7, grid, &loaded)),
            [(0, grid), (2, grid)]
        );
        assert_eq!(targets(grid_commands(8, None, &loaded)), [(1, None)]);
        assert_eq!(targets(grid_commands(9, grid, &loaded)), []);
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

    fn mode() -> (tempfile::TempDir, LibraryMode, EngineController, Printer) {
        let (dir, mode, engine, _renderer, printer) = mode_rendering();
        (dir, mode, engine, printer)
    }

    /// [`mode`] with the engine's renderer, to render blocks with [`render`].
    fn mode_rendering() -> (
        tempfile::TempDir,
        LibraryMode,
        EngineController,
        engine::EngineRenderer,
        Printer,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let mode = LibraryMode::start(Some(&dir.path().join("library.sqlite3"))).unwrap();
        let (engine, renderer) = engine::engine(engine::EngineConfig {
            sample_rate: 44_100,
            max_block_frames: 512,
        });
        (dir, mode, engine, renderer, Printer::spawn())
    }

    fn render(renderer: &mut engine::EngineRenderer) {
        let (mut master, mut headphones) = (vec![0.0; 1024], vec![0.0; 1024]);
        renderer.render(&mut master, &mut headphones);
    }

    /// Adds track 7 to the library, so cue and hotcue writes for it succeed.
    fn add_track_7(mode: &LibraryMode) {
        mode.writer
            .submit_wait(WriteOp::Raw(vec![library::RawStatement {
                sql: "INSERT INTO tracks(id, path, fingerprint, added_at) \
                      VALUES (7, '/m/7.wav', x'00', 0)"
                    .into(),
                params: vec![],
            }]))
            .unwrap();
    }

    /// Loads track 7 (see [`loaded`]) on `deck`.
    fn load_7(
        mode: &mut LibraryMode,
        deck: usize,
        engine: &mut EngineController,
        printer: &Printer,
    ) {
        let deck = DeckId::new(deck).unwrap();
        let seq = mode.next_seq(deck);
        let next = worker::Loaded {
            deck,
            seq,
            ..loaded(seq)
        };
        mode.load(next, engine, printer);
    }

    fn stored_hotcues(mode: &LibraryMode) -> [Option<f64>; engine::HOTCUES] {
        flush(mode);
        mode.lib.reader().unwrap().hotcues(7).unwrap()
    }

    #[test]
    fn hotcues_reach_the_library_and_every_deck_holding_the_track() {
        let (_dir, mut mode, mut engine, mut renderer, printer) = mode_rendering();
        add_track_7(&mode);
        let (d1, d3) = (DeckId::new(0).unwrap(), DeckId::new(2).unwrap());
        load_7(&mut mode, 0, &mut engine, &printer);
        load_7(&mut mode, 2, &mut engine, &printer);
        render(&mut renderer);
        mode.sync_cues(&mut engine, &printer);
        let slot1 = engine::HotcueSlot::new(1).unwrap();
        let set = |engine: &mut EngineController, deck, value| {
            engine
                .send(Command::Deck(deck, DeckCommand::SetHotcue(slot1, value)))
                .unwrap();
        };

        // Pad 1 set on deck 1: saved, and sent to deck 3.
        set(&mut engine, d1, Some(100.0));
        render(&mut renderer);
        mode.sync_cues(&mut engine, &printer);
        // Deck 3 does not show it yet: its empty pad is no deletion.
        mode.sync_cues(&mut engine, &printer);
        assert_eq!(stored_hotcues(&mode)[0], Some(100.0));
        render(&mut renderer);
        assert_eq!(engine.deck(d3).hotcues[0], Some(100.0), "deck 3 has it");
        mode.sync_cues(&mut engine, &printer);
        assert_eq!(stored_hotcues(&mode)[0], Some(100.0));

        // Deleted on deck 3: deleted in the library and on deck 1.
        engine
            .send(Command::Deck(d3, DeckCommand::HotcueDelete(slot1)))
            .unwrap();
        render(&mut renderer);
        mode.sync_cues(&mut engine, &printer);
        mode.sync_cues(&mut engine, &printer);
        assert_eq!(stored_hotcues(&mode)[0], None);
        render(&mut renderer);
        assert_eq!(engine.deck(d1).hotcues[0], None, "deck 1 lost it too");
        mode.sync_cues(&mut engine, &printer);
        render(&mut renderer);
        mode.sync_cues(&mut engine, &printer);
        assert_eq!(stored_hotcues(&mode), [None; engine::HOTCUES]);
        assert_eq!(mode.lib.failed_write_count(), 0);
        mode.shutdown(&mut engine, &printer);
        printer.finish();
    }

    #[test]
    fn hotcues_are_not_saved_when_they_could_not_be_read() {
        let (_dir, mut mode, mut engine, mut renderer, printer) = mode_rendering();
        add_track_7(&mode);
        let d1 = DeckId::new(0).unwrap();
        let seq = mode.next_seq(d1);
        let unread = worker::Loaded {
            hotcues_read: false,
            ..loaded(seq)
        };
        mode.load(unread, &mut engine, &printer);
        render(&mut renderer);
        mode.sync_cues(&mut engine, &printer);
        let slot1 = engine::HotcueSlot::new(1).unwrap();
        for cmd in [
            DeckCommand::SetHotcue(slot1, Some(100.0)),
            DeckCommand::SetCue(50.0),
        ] {
            engine.send(Command::Deck(d1, cmd)).unwrap();
        }
        render(&mut renderer);
        mode.sync_cues(&mut engine, &printer);
        assert_eq!(stored_hotcues(&mode), [None; engine::HOTCUES]);
        assert_eq!(
            mode.lib.cue(7).unwrap(),
            Some(50.0),
            "the cue point is saved"
        );
        mode.shutdown(&mut engine, &printer);
        printer.finish();
    }

    #[test]
    fn the_last_changes_are_read_patiently() {
        let (_dir, mut mode, mut engine, printer) = mode();
        add_track_7(&mode);
        load_7(&mut mode, 0, &mut engine, &printer);
        let d1 = DeckId::new(0).unwrap();
        let mut hotcues = [None; engine::HOTCUES];
        hotcues[0] = Some(100.0);
        let shown = DeckSnapshot {
            loaded: true,
            duration_frames: 4410,
            track_sample_rate: 44_100,
            hotcues,
            load_seq: 1,
            ..DeckSnapshot::default()
        };
        // The renderer kept publishing for a few reads.
        let mut reads = 0;
        let read = || {
            reads += 1;
            (reads > 3).then_some(shown)
        };
        assert!(mode.sync_deck_patiently(d1, read, &printer));
        assert_eq!(
            mode.latest_memory.get(&(7, Spot::Hotcue(1))),
            Some(&Some(100.0_f64.to_bits()))
        );
        // A deck that never reads whole is given up on after a bounded number of tries.
        let mut reads = 0;
        assert!(!mode.sync_deck_patiently(
            d1,
            || {
                reads += 1;
                None
            },
            &printer
        ));
        assert_eq!(reads, SHUTDOWN_READ_ATTEMPTS);
        mode.shutdown(&mut engine, &printer);
        printer.finish();
    }

    fn loaded(seq: u64) -> worker::Loaded {
        worker::Loaded {
            deck: DeckId::new(0).unwrap(),
            seq,
            row: row(),
            buffer: Arc::new(engine::TrackBuffer::new(44_100, vec![0.0; 2 * 4410]).unwrap()),
            memory: engine::TrackMemory::default(),
            hotcues_read: true,
        }
    }

    #[test]
    fn a_stale_decode_is_dropped() {
        let (_dir, mut mode, mut engine, printer) = mode();
        let deck = DeckId::new(0).unwrap();
        mode.next_seq(deck);
        mode.next_seq(deck); // A second LOAD was asked for after the first.
        mode.load(loaded(1), &mut engine, &printer);
        assert!(
            mode.decks[0].is_none(),
            "the first, stale decode must not load"
        );
        mode.load(loaded(2), &mut engine, &printer);
        assert_eq!(mode.decks[0].as_ref().map(|t| t.row.id), Some(7));
        mode.shutdown(&mut engine, &printer);
        printer.finish();
    }

    #[test]
    fn a_new_library_grid_reaches_every_deck_holding_the_track() {
        let (_dir, mut mode, mut engine, printer) = mode();
        for n in [0, 2] {
            let deck = DeckId::new(n).unwrap();
            let seq = mode.next_seq(deck);
            mode.load(
                worker::Loaded {
                    deck,
                    seq,
                    ..loaded(0)
                },
                &mut engine,
                &printer,
            );
        }
        let grids = |mode: &LibraryMode| mode.decks.each_ref().map(|t| t.as_ref().map(|t| t.grid));
        assert_eq!(grids(&mode), [Some(None), None, Some(None), None]);
        // A finished analysis: the refresh reports the track with its new grid.
        let analyzed = library::Beatgrid {
            bpm: Some(120.0),
            first_beat_frames: Some(0.0),
            grid_locked: false,
        };
        mode.on_event(
            Event::Track {
                deck: DeckId::new(2).unwrap(),
                row: row(),
                grid: Some(analyzed),
            },
            &mut engine,
            &printer,
        );
        let grid = Grid::from_bpm(120.0, 0.0, 44_100);
        assert_eq!(grids(&mode), [Some(grid), None, Some(grid), None]);
        // A grid edit.
        let edited = library::Beatgrid {
            bpm: Some(125.0),
            ..analyzed
        };
        mode.on_event(
            Event::Grid {
                deck: DeckId::new(0).unwrap(),
                track: 7,
                grid: edited,
            },
            &mut engine,
            &printer,
        );
        let grid = Grid::from_bpm(125.0, 0.0, 44_100);
        assert_eq!(grids(&mode), [Some(grid), None, Some(grid), None]);
        assert_eq!(mode.decks[2].as_ref().unwrap().row.bpm, Some(125.0));
        mode.shutdown(&mut engine, &printer);
        printer.finish();
    }

    #[test]
    fn grid_edits_wait_for_the_load_and_use_the_track_rate() {
        let (_dir, mut mode, engine, printer) = mode();
        let deck = DeckId::new(0).unwrap();
        let shift = GridEditRequest::Shift { millis: 10 };
        let refused = |mode: &LibraryMode| mode.grid_job(deck, shift, &engine).unwrap_err();
        assert!(refused(&mode).contains("load a library track"));
        let mut memory = MemorySync::new(4410, 48_000, engine::TrackMemory::default(), 0);
        mode.decks[0] = Some(DeckTrack {
            row: TrackRow {
                sample_rate: Some(48_000),
                ..row()
            },
            memory,
            grid: None,
        });
        assert!(
            refused(&mode).contains("still loading"),
            "{}",
            refused(&mode)
        );
        // The engine shows the new track loaded.
        let snap = DeckSnapshot {
            loaded: true,
            playing: false,
            position_frames: 0.0,
            duration_frames: 4410,
            track_sample_rate: 48_000,
            tempo_ratio: 1.0,
            cue_frames: 0.0,
            previewing: false,
            load_seq: 1,
            ..DeckSnapshot::default()
        };
        memory.step(&snap);
        mode.decks[0].as_mut().unwrap().memory = memory;
        match mode.grid_job(deck, shift, &engine).unwrap() {
            Job::Grid {
                track,
                edit,
                sample_rate,
                ..
            } => {
                assert_eq!(track, 7);
                assert_eq!(edit, GridEdit::Shift { frames: 480.0 });
                assert_eq!(sample_rate, Some(48_000));
            }
            other => panic!("{other:?}"),
        }
        // The decoded rate is used even when the tags give none (or another one).
        mode.decks[0].as_mut().unwrap().row.sample_rate = None;
        match mode.grid_job(deck, shift, &engine).unwrap() {
            Job::Grid { edit, .. } => assert_eq!(edit, GridEdit::Shift { frames: 480.0 }),
            other => panic!("{other:?}"),
        }
        mode.decks[0].as_mut().unwrap().row.sample_rate = Some(44_100);
        match mode.grid_job(deck, shift, &engine).unwrap() {
            Job::Grid { edit, .. } => assert_eq!(edit, GridEdit::Shift { frames: 480.0 }),
            other => panic!("{other:?}"),
        }
        drop(printer);
    }

    fn pending(id: i64, size: i64) -> PendingTrack {
        PendingTrack {
            id,
            size: Some(size),
            mtime: Some(1),
        }
    }

    #[test]
    fn new_tracks_are_queued_once() {
        let (_dir, mut mode, mut engine, printer) = mode();
        let tracks =
            |ids: &[i64]| Event::NewTracks(ids.iter().map(|&id| pending(id, 10)).collect());
        mode.on_event(tracks(&[1, 2]), &mut engine, &printer);
        mode.on_event(tracks(&[2, 3]), &mut engine, &printer);
        assert_eq!((mode.enqueued.len(), mode.queued_total), (3, 3));
        mode.shutdown(&mut engine, &printer);
        printer.finish();
    }

    #[test]
    fn a_track_whose_file_changed_is_queued_again() {
        let (_dir, mut mode, mut engine, printer) = mode();
        mode.on_event(
            Event::NewTracks(vec![pending(1, 10)]),
            &mut engine,
            &printer,
        );
        // Still out of date, same file (say, its analysis failed): not retried.
        mode.on_event(
            Event::NewTracks(vec![pending(1, 10)]),
            &mut engine,
            &printer,
        );
        assert_eq!(mode.queued_total, 1);
        // The watcher rescanned it after a change (a copy completed): queued again.
        mode.on_event(
            Event::NewTracks(vec![pending(1, 20)]),
            &mut engine,
            &printer,
        );
        assert_eq!(mode.queued_total, 2);
        mode.shutdown(&mut engine, &printer);
        printer.finish();
    }

    #[test]
    fn analysis_is_held_to_one_worker_while_a_deck_plays() {
        let (_dir, mut mode, mut engine, printer) = mode();
        let limit = |mode: &LibraryMode| mode.analyzer.as_ref().unwrap().max_active();
        assert_eq!(limit(&mode), usize::MAX);
        mode.set_playing(true);
        assert_eq!(limit(&mode), WORKERS_WHILE_PLAYING);
        mode.set_playing(true);
        assert_eq!(limit(&mode), WORKERS_WHILE_PLAYING);
        mode.set_playing(false);
        assert_eq!(limit(&mode), usize::MAX);
        mode.shutdown(&mut engine, &printer);
        printer.finish();
    }

    /// Waits until every write queued so far has been applied (or has failed).
    fn flush(mode: &LibraryMode) {
        mode.writer.submit_wait(WriteOp::Batch(Vec::new())).unwrap();
    }

    #[test]
    fn a_failed_cue_write_is_retried_once_if_still_the_latest() {
        let (_dir, mut mode, mut engine, printer) = mode();
        // Track 7 does not exist yet: the write fails.
        mode.latest_memory
            .insert((7, Spot::Cue), Some(500.0_f64.to_bits()));
        mode.writer
            .submit(WriteOp::SetCue {
                track: 7,
                frames: 500.0,
            })
            .unwrap();
        flush(&mode);
        assert_eq!(mode.lib.failed_write_count(), 1);
        mode.report_failed_writes(&printer);
        flush(&mode);
        assert_eq!(mode.lib.failed_write_count(), 2, "retried once");
        mode.report_failed_writes(&printer);
        flush(&mode);
        assert_eq!(mode.lib.failed_write_count(), 2, "and only once");

        // A superseded value is not retried.
        mode.latest_memory
            .insert((7, Spot::Cue), Some(900.0_f64.to_bits()));
        mode.writer
            .submit(WriteOp::SetCue {
                track: 7,
                frames: 600.0,
            })
            .unwrap();
        flush(&mode);
        mode.report_failed_writes(&printer);
        flush(&mode);
        assert_eq!(mode.lib.failed_write_count(), 3);

        // A retry that succeeds stores the cue.
        mode.writer
            .submit(WriteOp::SetCue {
                track: 7,
                frames: 900.0,
            })
            .unwrap();
        flush(&mode);
        mode.writer
            .submit_wait(WriteOp::Raw(vec![library::RawStatement {
                sql: "INSERT INTO tracks(id, path, fingerprint, added_at) \
                      VALUES (7, '/m/7.wav', x'00', 0)"
                    .into(),
                params: vec![],
            }]))
            .unwrap();
        mode.report_failed_writes(&printer);
        flush(&mode);
        assert_eq!(mode.lib.failed_write_count(), 4);
        assert_eq!(mode.lib.cue(7).unwrap(), Some(900.0));
        mode.shutdown(&mut engine, &printer);
        printer.finish();
    }

    #[test]
    fn a_failed_hotcue_write_is_retried_once_if_still_the_latest() {
        let (_dir, mut mode, mut engine, printer) = mode();
        let set = |frames| WriteOp::SetHotcue {
            track: 7,
            slot: 3,
            frames,
        };
        let delete = WriteOp::DeleteHotcue { track: 7, slot: 3 };
        // Track 7 does not exist yet: writes fail.
        mode.latest_memory
            .insert((7, Spot::Hotcue(3)), Some(500.0_f64.to_bits()));
        mode.writer.submit(set(500.0)).unwrap();
        flush(&mode);
        mode.report_failed_writes(&printer);
        flush(&mode);
        assert_eq!(mode.lib.failed_write_count(), 2, "retried once");
        mode.report_failed_writes(&printer);
        flush(&mode);
        assert_eq!(mode.lib.failed_write_count(), 2, "and only once");

        // The slot was deleted since: the failed set is stale and never retried.
        mode.latest_memory.insert((7, Spot::Hotcue(3)), None);
        mode.writer.submit(set(600.0)).unwrap();
        flush(&mode);
        mode.report_failed_writes(&printer);
        flush(&mode);
        assert_eq!(mode.lib.failed_write_count(), 3);

        // A failed delete that is still the latest is retried: once the track exists, the
        // retry succeeds (the count of failures does not grow).
        mode.writer.submit(delete).unwrap();
        flush(&mode);
        assert_eq!(mode.lib.failed_write_count(), 4);
        mode.writer
            .submit_wait(WriteOp::Raw(vec![library::RawStatement {
                sql: "INSERT INTO tracks(id, path, fingerprint, added_at) \
                      VALUES (7, '/m/7.wav', x'00', 0)"
                    .into(),
                params: vec![],
            }]))
            .unwrap();
        mode.report_failed_writes(&printer);
        flush(&mode);
        assert_eq!(mode.lib.failed_write_count(), 4);
        mode.shutdown(&mut engine, &printer);
        printer.finish();
    }

    #[test]
    fn failed_hotcue_writes_name_the_slot() {
        let (_dir, mode, mut engine, printer) = mode();
        let text = |op| {
            mode.failed_write_text(&FailedWrite {
                op,
                error: library::LibraryError::NoSuchTrack(7),
            })
        };
        assert!(
            text(WriteOp::SetHotcue {
                track: 7,
                slot: 3,
                frames: 1.0,
            })
            .starts_with("warning: could not save hotcue 3 for track 7: ")
        );
        assert!(
            text(WriteOp::DeleteHotcue { track: 7, slot: 5 })
                .starts_with("warning: could not delete hotcue 5 for track 7: ")
        );
        assert!(
            text(WriteOp::SetCue {
                track: 7,
                frames: 1.0,
            })
            .starts_with("warning: could not save the cue point for track 7: ")
        );
        mode.shutdown(&mut engine, &printer);
        printer.finish();
    }

    #[test]
    fn two_loads_in_flight_never_save_to_the_first_track() {
        let (_dir, mut mode, mut engine, printer) = mode();
        let deck = DeckId::new(0).unwrap();
        let shown = |load_seq, hotcue: Option<f64>| {
            let mut hotcues = [None; engine::HOTCUES];
            hotcues[0] = hotcue;
            DeckSnapshot {
                loaded: true,
                duration_frames: 4410,
                track_sample_rate: 44_100,
                hotcues,
                load_seq,
                ..DeckSnapshot::default()
            }
        };
        let mut load = |mode: &mut LibraryMode, id| {
            let seq = mode.next_seq(deck);
            let next = worker::Loaded {
                seq,
                row: TrackRow { id, ..row() },
                ..loaded(seq)
            };
            mode.load(next, &mut engine, &printer);
        };
        // Track 7 (X) shows; then 8 (A) and 9 (B) are sent before any snapshot shows 8.
        load(&mut mode, 7);
        mode.sync_snapshot(deck, &shown(1, None), &printer);
        load(&mut mode, 8);
        load(&mut mode, 9);
        assert_eq!(mode.outgoing[0].as_ref().map(|(t, _)| t.row.id), Some(7));
        // The engine shows track 8 loaded, its hotcue 1 set: not track 7's, nor track 9's.
        mode.sync_snapshot(deck, &shown(2, Some(100.0)), &printer);
        assert!(mode.outgoing[0].is_none());
        assert!(mode.latest_memory.is_empty(), "{:?}", mode.latest_memory);
        // Track 9 shows: its own changes are saved.
        mode.sync_snapshot(deck, &shown(3, None), &printer);
        mode.sync_snapshot(deck, &shown(3, Some(50.0)), &printer);
        assert_eq!(
            mode.latest_memory.get(&(9, Spot::Hotcue(1))),
            Some(&Some(50.0_f64.to_bits()))
        );
        assert_eq!(mode.latest_memory.len(), 1);
        drop(load);
        mode.shutdown(&mut engine, &printer);
        printer.finish();
    }

    #[test]
    fn the_outgoing_track_is_saved_until_the_new_load_applies() {
        let (_dir, mut mode, mut engine, printer) = mode();
        let deck = DeckId::new(0).unwrap();
        let shown = |load_seq, hotcue: Option<f64>| {
            let mut hotcues = [None; engine::HOTCUES];
            hotcues[0] = hotcue;
            DeckSnapshot {
                loaded: true,
                duration_frames: 4410,
                track_sample_rate: 44_100,
                hotcues,
                load_seq,
                ..DeckSnapshot::default()
            }
        };
        let seq = mode.next_seq(deck);
        mode.load(loaded(seq), &mut engine, &printer);
        mode.sync_snapshot(deck, &shown(1, None), &printer);
        assert!(mode.decks[0].as_ref().unwrap().memory.load_applied());
        // Track 8 is sent; the engine still shows track 7, whose hotcue 1 is set now.
        let seq = mode.next_seq(deck);
        let next = worker::Loaded {
            seq,
            row: TrackRow { id: 8, ..row() },
            ..loaded(seq)
        };
        mode.load(next, &mut engine, &printer);
        assert!(
            mode.refresh_wanted,
            "a grid edited during the decode is fetched"
        );
        mode.sync_snapshot(deck, &shown(1, Some(100.0)), &printer);
        let hotcue_1 =
            |mode: &LibraryMode, track| mode.latest_memory.get(&(track, Spot::Hotcue(1))).copied();
        assert_eq!(hotcue_1(&mode, 7), Some(Some(100.0_f64.to_bits())));
        assert_eq!(hotcue_1(&mode, 8), None);
        // Track 8's load shows: the outgoing track is done with.
        mode.sync_snapshot(deck, &shown(2, None), &printer);
        assert!(mode.outgoing[0].is_none());
        assert_eq!(hotcue_1(&mode, 8), None);
        mode.shutdown(&mut engine, &printer);
        printer.finish();
    }
}
