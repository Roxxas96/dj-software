//! `play --library`: the browser line, loading library tracks with their cue point,
//! hotcues and beatgrid, beatgrid edits, cue and hotcue write-through, background analysis
//! and folder watching, driven from the control loop. With `[stems] enabled`, each library
//! track's stems are loaded from the stems cache or separated once its load has applied.
//!
//! The control loop never waits on the database or a decode: browser moves, lookups and
//! grid edits run on the library worker, decoding on its loader thread
//! ([`worker`]), stems on their own thread ([`stem_jobs`]); cue points and hotcues are
//! written with the non-blocking `Writer::submit`.

mod memory_sync;
mod playlist_edits;
mod stem_jobs;
mod worker;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use actions::{AppRequest, GridEditRequest};
use analysis::KeyNotation;
use anyhow::Context;
use engine::{
    Command, DeckCommand, DeckId, DeckSnapshot, EngineController, Grid, HotcueSlot, StemSet,
    TrackBuffer,
};
use library::{
    Analyzer, BrowserEdit, BrowserView, FailedWrite, GridEdit, KEY_NOTATION, Library, PendingTrack,
    TrackRow, WatchHealth, Watcher, WriteOp, Writer, default_threads,
};

use crate::console::ConsoleCommand;
use crate::format::{bpm_text, grid_text, key_text, notation_from_setting, safe, track_name};
use crate::printer::Printer;
use memory_sync::{MemoryAction, MemorySync, engine_grid};
use stem_jobs::{Reply, StemsControl, StemsSetup};
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
    path: PathBuf,
    /// The deck's `load_seq` once the engine has applied this track's `Load`.
    load_seq: u64,
    memory: MemorySync,
    /// The grid last sent to the deck (with the `Load` or a `SetGrid`).
    grid: Option<Grid>,
    /// The decoded track, for the window's detailed waveform.
    audio: Arc<TrackBuffer>,
    /// The decoded track, kept (with stems enabled) until its stems are asked for.
    buffer: Option<Arc<TrackBuffer>>,
    stems: DeckStems,
}

/// Where a deck's load is with its stems.
#[derive(Debug)]
enum DeckStems {
    /// The load has not applied yet.
    Unrequested,
    /// Waiting on this stems job.
    Job(u64),
    /// The stems came; the engine command queue was full, so they are sent again.
    Sending(Arc<StemSet>),
    /// Nothing more to do: the stems were sent, or there are none (stems disabled, no
    /// model, a failed separation).
    Done,
    /// Over 20 minutes: refused.
    TooLong,
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
    /// The last value saved this session for each cue point and hotcue, as frames bits and
    /// loop length bits (`None`: the hotcue was deleted).
    latest_memory: HashMap<(i64, Spot), Option<Stored>>,
    /// Cue and hotcue writes already retried once, as (track, spot, frames and loop length bits).
    retried_memory: HashSet<(i64, Spot, Option<Stored>)>,
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
    /// Stems, when enabled.
    stems: Option<StemsControl>,
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
    /// Set from another thread that changed the library: a refresh is wanted.
    changed_elsewhere: Arc<AtomicBool>,
    /// The hotcues deleted in the latest round, as (track, slot): their colour and name
    /// are gone from the library.
    deleted_hotcues: Vec<(i64, HotcueSlot)>,
    /// The hotcue labels whose write failed since the window last asked, as (track, slot).
    failed_labels: Vec<(i64, HotcueSlot)>,
    last_refresh: Option<Instant>,
    /// The status lines should be redrawn now.
    dirty: bool,
}

impl LibraryMode {
    /// The open library, for threads that read it on their own.
    pub fn library(&self) -> Arc<Library> {
        Arc::clone(&self.lib)
    }

    /// A flag for a thread that changes the library (an import) to set: the browser and
    /// the decks' rows are then read again, within a second.
    pub fn changed_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.changed_elsewhere)
    }

    /// The hotcues deleted in the latest round, as (track, slot), for the window to stop
    /// showing their colour and name.
    pub fn deleted_hotcues(&self) -> &[(i64, HotcueSlot)] {
        &self.deleted_hotcues
    }

    /// The hotcue labels whose write failed since the last call, as (track, slot), for the
    /// window to stop showing them.
    pub fn take_failed_labels(&mut self) -> Vec<(i64, HotcueSlot)> {
        std::mem::take(&mut self.failed_labels)
    }

    /// A read-only connection to the library.
    pub fn reader(&self) -> Result<library::Reader, library::LibraryError> {
        self.lib.reader()
    }

    /// Opens the library, queues the tracks not analyzed yet, watches the library folders
    /// and starts the worker, and with `stems` enabled the stems thread. Prints what it
    /// does (before the control loop starts).
    pub fn start(db: Option<&Path>, stems: &crate::config::StemsConfig) -> anyhow::Result<Self> {
        Self::start_with(db, stems.enabled.then(|| StemsSetup::from_config(stems)))
    }

    /// [`LibraryMode::start`] with stems set up as `stems` says (`None`: disabled).
    fn start_with(db: Option<&Path>, stems: Option<StemsSetup>) -> anyhow::Result<Self> {
        Self::start_browsing(db, stems, None)
    }

    /// [`LibraryMode::start_with`] with the browser's tree starting at `drives` (`None`:
    /// the computer's drives).
    fn start_browsing(
        db: Option<&Path>,
        stems: Option<StemsSetup>,
        drives: Option<Vec<library::Drive>>,
    ) -> anyhow::Result<Self> {
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
        let (jobs, worker) = worker::spawn(Arc::clone(&lib), events_tx, drives)
            .context("starting the library worker")?;
        let stems = stems
            .map(|setup| StemsControl::start(Arc::clone(&lib), setup))
            .transpose()
            .context("starting the stems thread")?;
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
            stems,
            decks: [None, None, None, None],
            outgoing: [None, None, None, None],
            engine_loads: [0; 4],
            load_seq: [0; 4],
            notation,
            browser: None,
            analysis_status: None,
            last_poll: Instant::now(),
            refresh_wanted: false,
            changed_elsewhere: Arc::default(),
            deleted_hotcues: Vec::new(),
            failed_labels: Vec::new(),
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
        if let Err(why) = self.try_send_job(job) {
            printer.event(why);
        }
    }

    /// Hands `job` to the worker, or says why it cannot.
    fn try_send_job(&self, job: Job) -> Result<(), String> {
        let sent = self
            .jobs
            .as_ref()
            .is_some_and(|jobs| jobs.send(job).is_ok());
        if sent {
            Ok(())
        } else {
            Err("warning: the library worker stopped; library actions do nothing".into())
        }
    }

    /// Loads library track `track` on `deck`, as the console's `load` does: the decode,
    /// the remembered cues and hotcues, the analysis and the stems follow the same way.
    ///
    /// # Errors
    ///
    /// Why the load cannot be asked for (the worker stopped).
    pub fn load_track(&mut self, deck: DeckId, track: i64) -> Result<(), String> {
        let seq = self.next_seq(deck);
        let refuse_if_playing = false;
        self.try_send_job(Job::LoadTrack {
            deck,
            track,
            seq,
            refuse_if_playing,
        })
    }

    /// [`LibraryMode::load_track`] for the window: refused with the event `Deck N is
    /// playing` if the deck plays once the track is decoded, its track left in place.
    ///
    /// # Errors
    ///
    /// Why the load cannot be asked for (the worker stopped).
    pub fn load_track_unless_playing(&mut self, deck: DeckId, track: i64) -> Result<(), String> {
        let seq = self.next_seq(deck);
        self.try_send_job(Job::LoadTrack {
            deck,
            track,
            seq,
            refuse_if_playing: true,
        })
    }

    /// Changes the browser as `edit` asks; the new view arrives with a later
    /// [`poll`](Self::poll).
    ///
    /// # Errors
    ///
    /// Why the change cannot be asked for (the worker stopped).
    pub fn browse(&mut self, edit: BrowserEdit) -> Result<(), String> {
        self.try_send_job(Job::Edit(edit))
    }

    /// Appends library tracks `tracks` to `playlist`, in one write; the browser is read
    /// again once it is done.
    ///
    /// # Errors
    ///
    /// Why the write cannot be asked for (the worker stopped). A failed write is
    /// reported as an event.
    pub fn add_to_playlist(&self, playlist: i64, tracks: &[i64]) -> Result<(), String> {
        self.try_send_job(Job::EditPlaylist(playlist_edits::append(playlist, tracks)))
    }

    /// Moves the entries `from` of `playlist` (each a position from 0 and the track seen
    /// there) together, in playlist order, to the gap before position `to` (counted before
    /// the move), in one write; nothing moves unless each still holds its track.
    ///
    /// # Errors
    ///
    /// As [`LibraryMode::add_to_playlist`].
    pub fn move_in_playlist(
        &self,
        playlist: i64,
        from: &[(usize, i64)],
        to: usize,
    ) -> Result<(), String> {
        self.try_send_job(Job::EditPlaylist(playlist_edits::move_to(
            playlist, from, to,
        )))
    }

    /// Removes the entries `positions` of `playlist` (each a position from 0 and the track
    /// seen there), in one write; nothing is removed unless each still holds its track.
    ///
    /// # Errors
    ///
    /// As [`LibraryMode::add_to_playlist`].
    pub fn remove_from_playlist(
        &self,
        playlist: i64,
        positions: &[(usize, i64)],
    ) -> Result<(), String> {
        self.try_send_job(Job::EditPlaylist(playlist_edits::remove(
            playlist, positions,
        )))
    }

    /// Makes a playlist called `name` and highlights it in the browser.
    ///
    /// # Errors
    ///
    /// Why it is refused: the name is empty, or a playlist already has it (the same
    /// letters, case included, as the console's `playlist create`); or the worker stopped.
    pub fn create_playlist(&self, name: &str) -> Result<(), String> {
        let name = self.checked_name(name, None)?;
        self.try_send_job(Job::CreatePlaylist(name))
    }

    /// Renames `playlist` to `name`; the browser is read again once it is done.
    ///
    /// # Errors
    ///
    /// As [`LibraryMode::create_playlist`]. Renaming a playlist to its own name is no clash.
    pub fn rename_playlist(&self, playlist: i64, name: &str) -> Result<(), String> {
        let id = playlist;
        let name = self.checked_name(name, Some(id))?;
        self.try_send_job(Job::EditPlaylist(WriteOp::RenamePlaylist { id, name }))
    }

    /// Deletes `playlist`, not its tracks; the browser is read again once it is done.
    ///
    /// # Errors
    ///
    /// As [`LibraryMode::add_to_playlist`].
    pub fn delete_playlist(&self, playlist: i64) -> Result<(), String> {
        self.try_send_job(Job::EditPlaylist(WriteOp::DeletePlaylist(playlist)))
    }

    /// `name` trimmed, if it is not empty and no playlist but `own` has it.
    fn checked_name(&self, name: &str, own: Option<i64>) -> Result<String, String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("A playlist needs a name".to_owned());
        }
        let reader = self.lib.reader().map_err(safe)?;
        let taken = reader.playlists().map_err(safe)?;
        if taken.iter().any(|p| p.name == name && Some(p.id) != own) {
            return Err(format!("There is already a playlist called {name}"));
        }
        Ok(name.to_owned())
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
                if let Err(why) = self.load_track(deck, track) {
                    printer.event(why);
                }
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
        let playing = DeckId::ALL.into_iter().any(|d| engine.deck(d).playing);
        self.set_playing(playing);
        self.poll_stems(playing, engine, printer);
        if self.changed_elsewhere.swap(false, Ordering::Relaxed) {
            self.refresh_wanted = true;
        }
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
            Event::FolderAdded => {
                self.refresh_wanted = true;
                self.find_new_tracks = true;
            }
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
        if loaded.refuse_if_playing && engine.deck(deck).playing {
            printer.event(format!("Deck {n} is playing"));
            return;
        }
        // Save the outgoing track's last cue and hotcue changes first.
        self.sync_deck(deck, engine, printer);
        if engine.deck(deck).playing {
            printer.event(format!("warning: deck {n} was playing"));
        }
        let frames = loaded.buffer.frames();
        let rate = loaded.buffer.sample_rate();
        let audio = Arc::clone(&loaded.buffer);
        let buffer = self.stems.is_some().then(|| Arc::clone(&loaded.buffer));
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
            path: loaded.path,
            load_seq: after + 1,
            memory,
            grid: loaded.memory.clamped(frames).grid,
            audio,
            buffer,
            stems: DeckStems::Unrequested,
        };
        // The outgoing track's changes keep being saved until the engine applies this load.
        // A track whose own load never showed had none: the earlier outgoing track then
        // stays, until the load that replaced it shows.
        if let Some(mut old) = self.decks[i].replace(incoming) {
            // The outgoing track wants no stems now; its job is cancelled unless another
            // deck holding the track waits on it too.
            old.buffer = None;
            old.stems = DeckStems::Done;
            if old.memory.load_applied() {
                self.outgoing[i] = Some((old, after));
            }
        }
        self.cancel_unwanted_stems();
        // A grid edited while the track was decoding reaches the deck at the refresh.
        self.refresh_wanted = true;
        self.dirty = true;
    }

    fn sync_cues(&mut self, engine: &mut EngineController, printer: &Printer) {
        self.deleted_hotcues.clear();
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
                MemoryAction::SaveHotcue(slot, hotcue) => (slot, Some(hotcue)),
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
                MemoryAction::SaveHotcue(slot, hotcue) => WriteOp::SetHotcue {
                    track: id,
                    slot: slot.number(),
                    frames: hotcue.frames,
                    loop_frames: hotcue.loop_frames,
                },
                MemoryAction::DeleteHotcue(slot) => {
                    self.deleted_hotcues.push((id, slot));
                    WriteOp::DeleteHotcue {
                        track: id,
                        slot: slot.number(),
                    }
                }
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
            if let WriteOp::SetHotcueLabel { track, slot, .. } = failed.op
                && let Some(slot) = HotcueSlot::new(slot)
            {
                self.failed_labels.push((track, slot));
            }
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
            | WriteOp::SetHotcueLabel { track, .. }
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

    /// Stems, once per control-loop round: asks for the stems of each deck whose load has
    /// just applied, applies the stems thread's replies, sends stems the engine queue had
    /// no room for, and tells the thread what the decks are doing.
    fn poll_stems(&mut self, playing: bool, engine: &mut EngineController, printer: &Printer) {
        for deck in DeckId::ALL {
            self.request_stems(deck, printer);
        }
        let replies = self
            .stems
            .as_mut()
            .map(StemsControl::take_replies)
            .unwrap_or_default();
        for reply in replies {
            self.on_stems_reply(reply, engine, printer);
        }
        self.send_stems(engine);
        if let Some(stems) = &self.stems {
            let playheads = DeckId::ALL.map(|d| engine.deck(d).position_frames);
            let loaded = self
                .decks
                .iter()
                .flatten()
                .chain(self.outgoing.iter().flatten().map(|(t, _)| t))
                .map(|t| t.row.id)
                .collect();
            let job_decks = DeckId::ALL
                .into_iter()
                .filter_map(|d| match self.decks[d.index()].as_ref()?.stems {
                    DeckStems::Job(job) => Some((job, d)),
                    _ => None,
                })
                .collect();
            stems.update(playing, playheads, loaded, job_decks);
        }
    }

    /// Once `deck`'s load has applied: records the load (the stems cache keeps recently
    /// loaded tracks longer) and, with stems enabled, asks for its stems, unless the track
    /// is over 20 minutes.
    fn request_stems(&mut self, deck: DeckId, printer: &Printer) {
        let Some(track) = self.decks[deck.index()]
            .as_mut()
            .filter(|t| matches!(t.stems, DeckStems::Unrequested) && t.memory.load_applied())
        else {
            return;
        };
        let id = track.row.id;
        if let Err(e) = self.writer.submit(WriteOp::TrackLoaded(id)) {
            printer.event(format!(
                "warning: could not record the load of {}: {}",
                track_name(&track.row),
                safe(e)
            ));
        }
        track.stems = match (self.stems.as_mut(), track.buffer.take()) {
            (Some(stems), Some(buffer)) => {
                if stems::check_length(buffer.frames(), buffer.sample_rate()).is_err() {
                    DeckStems::TooLong
                } else {
                    stems
                        .request(deck, id, track_name(&track.row), buffer)
                        .map_or(DeckStems::Done, DeckStems::Job)
                }
            }
            _ => DeckStems::Done,
        };
        self.dirty = true;
    }

    /// Applies one reply of the stems thread. Stems go only to the decks still waiting on
    /// their job (a deck's newer load has its own state) and holding its track; any other
    /// result is dropped.
    fn on_stems_reply(&mut self, reply: Reply, engine: &mut EngineController, printer: &Printer) {
        let Some(stems) = self.stems.as_mut() else {
            return;
        };
        match reply {
            Reply::Message(text) => printer.event(text),
            Reply::Status(job, status) => self.dirty |= stems.set_status(job, status),
            Reply::Stems(job, set) => {
                let track = stems.finish(job);
                for t in self.decks.iter_mut().flatten() {
                    if matches!(t.stems, DeckStems::Job(j) if j == job) && Some(t.row.id) == track {
                        t.stems = DeckStems::Sending(Arc::clone(&set));
                    }
                }
                drop(set);
                self.send_stems(engine);
                self.dirty = true;
            }
            Reply::Done(job) => {
                stems.finish(job);
                for t in self.decks.iter_mut().flatten() {
                    if matches!(t.stems, DeckStems::Job(j) if j == job) {
                        t.stems = DeckStems::Done;
                    }
                }
                self.dirty = true;
            }
            Reply::Evicted(eviction) => {
                for line in stems.eviction_lines(&eviction) {
                    printer.event(line);
                }
            }
        }
    }

    /// Sends the stems waiting for the engine queue. A `SetStems` follows any `Load` sent
    /// before it, so it reaches the load the deck's [`DeckTrack`] is for.
    fn send_stems(&mut self, engine: &mut EngineController) {
        for deck in DeckId::ALL {
            let Some(track) = self.decks[deck.index()].as_mut() else {
                continue;
            };
            let DeckStems::Sending(set) = &track.stems else {
                continue;
            };
            let command = Command::Deck(deck, DeckCommand::SetStems(Some(Arc::clone(set))));
            if engine.send(command).is_ok() {
                track.stems = DeckStems::Done;
            }
        }
    }

    /// Cancels the stems jobs no deck waits on any more.
    fn cancel_unwanted_stems(&mut self) {
        let Some(stems) = self.stems.as_mut() else {
            return;
        };
        let waiting: Vec<u64> = self
            .decks
            .iter()
            .flatten()
            .filter_map(|t| match t.stems {
                DeckStems::Job(job) => Some(job),
                _ => None,
            })
            .collect();
        stems.cancel_unwanted(|job| waiting.contains(&job));
    }

    /// The deck line's stems text: `stems` once the deck has them, `stems 40%` while they
    /// are separated, `no stems (too long)` for a refused track.
    fn stems_text(&self, track: Option<&DeckTrack>, snap: &DeckSnapshot) -> Option<String> {
        let stems = self.stems.as_ref()?;
        if snap.stems {
            return Some("stems".to_owned());
        }
        match track?.stems {
            DeckStems::TooLong => Some("no stems (too long)".to_owned()),
            DeckStems::Job(job) => stem_jobs::status_text(stems.status(job)),
            _ => None,
        }
    }

    /// Saves `name` and `color` (`0xRRGGBB`) as the label of hotcue `slot` of `track`,
    /// which the window saw on `deck`. Refused, saying why, when `deck` no longer holds
    /// `track`. The write does not wait: a failure is reported with the other failed
    /// writes.
    pub fn set_hotcue_label(
        &mut self,
        deck: DeckId,
        track: i64,
        slot: HotcueSlot,
        name: Option<String>,
        color: Option<u32>,
    ) -> Result<(), String> {
        let current = self.decks[deck.index()].as_ref().map(|t| t.row.id);
        if current != Some(track) {
            return Err("The track changed before the hotcue was saved".to_owned());
        }
        let op = WriteOp::SetHotcueLabel {
            track,
            slot: slot.number(),
            name,
            color,
        };
        let what = write_text(&op);
        self.writer
            .submit(op)
            .map_err(|e| format!("warning: could not {what}: {}", safe(e)))
    }

    /// What the browser shows, once read.
    pub fn browser(&self) -> Option<&BrowserView> {
        self.browser.as_ref()
    }

    /// How keys are shown: the library's `key_notation` setting.
    pub fn notation(&self) -> KeyNotation {
        self.notation
    }

    /// The library track last loaded on `deck` (it may not have reached the engine yet:
    /// see [`DeckView::load_seq`]), with the stems' state as `snap`, the deck's snapshot,
    /// shows it.
    pub fn deck_view(&self, deck: DeckId, snap: &DeckSnapshot) -> Option<DeckView<'_>> {
        let track = self.decks[deck.index()].as_ref()?;
        Some(DeckView {
            row: &track.row,
            path: &track.path,
            load_seq: track.load_seq,
            audio: &track.audio,
            stems: self.stems_text(Some(track), snap),
        })
    }

    /// The browser line, then one line per loaded deck.
    pub fn status_lines(
        &self,
        engine: &EngineController,
        fx_slots: &[[engine::EffectKind; 3]; 4],
    ) -> Vec<String> {
        let mut lines = vec![browser_line(
            self.browser.as_ref(),
            self.notation,
            self.analysis_status.as_deref(),
        )];
        for deck in DeckId::ALL {
            let snap = engine.deck(deck);
            if snap.loaded {
                let current = self.decks[deck.index()].as_ref();
                let stems = self.stems_text(current, &snap);
                lines.push(deck_status(
                    deck,
                    &snap,
                    current.map(|t| &t.row),
                    &fx_slots[deck.index()],
                    &engine.channel(deck),
                    stems.as_deref(),
                ));
            }
        }
        lines
    }

    /// Saves the last cue and hotcue changes, stops stems, analysis, watching and the worker,
    /// waits for queued writes, and reports any that failed.
    pub fn shutdown(mut self, engine: &mut EngineController, printer: &Printer) {
        if let Some(stems) = self.stems.take() {
            for text in stems.shutdown() {
                printer.event(text);
            }
        }
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

/// A library track on a deck, as [`LibraryMode::deck_view`] shows it.
pub struct DeckView<'a> {
    pub row: &'a TrackRow,
    pub path: &'a Path,
    /// The deck's `load_seq` from which the engine plays this track.
    pub load_seq: u64,
    /// The decoded track.
    pub audio: &'a Arc<TrackBuffer>,
    /// The stems' state: `stems`, `stems 40%`, `no stems (too long)`.
    pub stems: Option<String>,
}

/// A cue point or one hotcue slot of a track.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Spot {
    Cue,
    Hotcue(u8),
}

/// What a cue or hotcue write stores: the frames bits and the loop length bits, if any.
type Stored = (u64, Option<u64>);

/// The track, spot and value (`None` for a deletion) a cue or hotcue write stores; `None`
/// for other writes.
fn memory_write(op: &WriteOp) -> Option<(i64, Spot, Option<Stored>)> {
    match *op {
        WriteOp::SetCue { track, frames } => {
            Some((track, Spot::Cue, Some((frames.to_bits(), None))))
        }
        WriteOp::SetHotcue {
            track,
            slot,
            frames,
            loop_frames,
        } => Some((
            track,
            Spot::Hotcue(slot),
            Some((frames.to_bits(), loop_frames.map(f64::to_bits))),
        )),
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
        WriteOp::SetHotcueLabel { slot, .. } => format!("name hotcue {slot}"),
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

/// A deck's line: time, the stems' state, BPM at the current tempo once analyzed, and the
/// track.
fn deck_status(
    deck: DeckId,
    snap: &DeckSnapshot,
    track: Option<&TrackRow>,
    fx_slots: &[engine::EffectKind; 3],
    channel: &engine::ChannelSnapshot,
    stems: Option<&str>,
) -> String {
    let mark = if snap.playing { '>' } else { ' ' };
    let mut line = format!(
        "{mark}{}{}",
        crate::play::deck_line(deck, snap),
        crate::play::fx_suffix(fx_slots, channel)
    );
    if let Some(stems) = stems {
        line.push_str(&format!("  {stems}"));
    }
    if let Some(track) = track {
        if let Some(bpm) = track.bpm {
            line.push_str(&format!("  {} BPM", bpm_text(bpm * snap.tempo_ratio)));
        }
        line.push_str(&format!("  {}", track_name(track)));
    }
    line
}

/// Library-mode fixtures, for this module's tests and the window session's.
#[cfg(test)]
pub(crate) mod testing {
    use super::*;

    /// Library mode on a new library at `db`, without stems.
    pub(crate) fn start(db: &Path) -> LibraryMode {
        LibraryMode::start_with(Some(db), None).unwrap()
    }

    pub(crate) fn row() -> TrackRow {
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

    /// Adds track 7 to the library, so cue and hotcue writes for it succeed.
    pub(crate) fn add_track_7(mode: &LibraryMode) {
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
    pub(crate) fn load_7(
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

    pub(crate) fn loaded(seq: u64) -> worker::Loaded {
        worker::Loaded {
            deck: DeckId::new(0).unwrap(),
            seq,
            row: row(),
            path: PathBuf::from("/m/7.wav"),
            buffer: Arc::new(engine::TrackBuffer::new(44_100, vec![0.0; 2 * 4410]).unwrap()),
            memory: engine::TrackMemory::default(),
            hotcues_read: true,
            refuse_if_playing: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::*;
    use super::*;
    use library::Focus;

    /// A view of an empty tree and list.
    fn empty_view() -> BrowserView {
        BrowserView {
            path: String::new(),
            index: 0,
            count: 0,
            track: None,
            focus: Focus::Tree,
            tree: Vec::new(),
            tree_cursor: 0,
            list: Vec::new(),
            list_offset: 0,
            list_count: 0,
            list_cursor: 0,
            search: None,
            sort: None,
            seq: 0,
            playlist: None,
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
            ..empty_view()
        };
        assert_eq!(
            browser_line(Some(&view), KeyNotation::Camelot, None),
            "▸ Playlists ▸ Peak time  12/48  Artist – Title  128.00 BPM  8A  •"
        );
        let empty = BrowserView {
            path: "Macintosh HD".into(),
            ..empty_view()
        };
        assert_eq!(
            browser_line(Some(&empty), KeyNotation::Camelot, Some("analyzing 12/340")),
            "▸ Macintosh HD  0/0   analyzing 12/340"
        );
    }

    #[test]
    fn deck_status_scales_bpm_by_tempo() {
        const SLOTS: [engine::EffectKind; 3] = [engine::EffectKind::Echo; 3];
        const CH: engine::ChannelSnapshot = engine::ChannelSnapshot {
            fx_enabled: [false; 3],
            fx_amount: 0.5,
        };
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
            deck_status(deck, &snap, Some(&row()), &SLOTS, &CH, None),
            ">Deck 3  01:05 / 03:20  134.40 BPM  Artist – Title"
        );
        let unanalyzed = TrackRow { bpm: None, ..row() };
        assert_eq!(
            deck_status(deck, &snap, Some(&unanalyzed), &SLOTS, &CH, None),
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
        let mode =
            LibraryMode::start_with(Some(&dir.path().join("library.sqlite3")), None).unwrap();
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
        let set = |engine: &mut EngineController, deck, value: Option<f64>| {
            engine
                .send(Command::Deck(
                    deck,
                    DeckCommand::SetHotcue(slot1, value.map(engine::Hotcue::point)),
                ))
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
        assert_eq!(
            engine.deck(d3).hotcues[0].map(|h| h.frames),
            Some(100.0),
            "deck 3 has it"
        );
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
    fn a_deleted_hotcue_is_reported_for_one_round() {
        let (_dir, mut mode, mut engine, mut renderer, printer) = mode_rendering();
        add_track_7(&mode);
        let d1 = DeckId::new(0).unwrap();
        load_7(&mut mode, 0, &mut engine, &printer);
        render(&mut renderer);
        mode.sync_cues(&mut engine, &printer);
        let slot2 = HotcueSlot::new(2).unwrap();
        let send = |engine: &mut EngineController, command| {
            engine.send(Command::Deck(d1, command)).unwrap();
        };
        send(
            &mut engine,
            DeckCommand::SetHotcue(slot2, Some(engine::Hotcue::point(100.0))),
        );
        render(&mut renderer);
        mode.sync_cues(&mut engine, &printer);
        assert!(mode.deleted_hotcues().is_empty());

        send(&mut engine, DeckCommand::HotcueDelete(slot2));
        render(&mut renderer);
        mode.sync_cues(&mut engine, &printer);
        assert_eq!(mode.deleted_hotcues(), [(7, slot2)]);
        // Set again elsewhere: a new hotcue, nothing more deleted.
        send(
            &mut engine,
            DeckCommand::SetHotcue(slot2, Some(engine::Hotcue::point(500.0))),
        );
        render(&mut renderer);
        mode.sync_cues(&mut engine, &printer);
        assert!(mode.deleted_hotcues().is_empty());
        assert_eq!(stored_hotcues(&mode)[1], Some(500.0));
        mode.shutdown(&mut engine, &printer);
        printer.finish();
    }

    #[test]
    fn a_saved_loop_reaches_the_library_and_the_other_decks() {
        let (_dir, mut mode, mut engine, mut renderer, printer) = mode_rendering();
        add_track_7(&mode);
        let (d1, d3) = (DeckId::new(0).unwrap(), DeckId::new(2).unwrap());
        load_7(&mut mode, 0, &mut engine, &printer);
        load_7(&mut mode, 2, &mut engine, &printer);
        render(&mut renderer);
        mode.sync_cues(&mut engine, &printer);
        let slot1 = engine::HotcueSlot::new(1).unwrap();
        let saved_loop = engine::Hotcue {
            frames: 100.0,
            loop_frames: Some(50.0),
        };
        engine
            .send(Command::Deck(
                d1,
                DeckCommand::SetHotcue(slot1, Some(saved_loop)),
            ))
            .unwrap();
        render(&mut renderer);
        mode.sync_cues(&mut engine, &printer);
        mode.sync_cues(&mut engine, &printer);
        flush(&mode);
        let points = mode.lib.reader().unwrap().hotcue_points(7).unwrap();
        let point = points[0].as_ref().unwrap();
        assert_eq!((point.frames, point.loop_frames), (100.0, Some(50.0)));
        render(&mut renderer);
        assert_eq!(
            engine.deck(d3).hotcues[0],
            Some(saved_loop),
            "deck 3 has it"
        );
        assert_eq!(mode.lib.failed_write_count(), 0);
        mode.shutdown(&mut engine, &printer);
        printer.finish();
    }

    #[test]
    fn turning_a_loop_slot_into_a_hotcue_clears_the_loop() {
        let (_dir, mut mode, mut engine, mut renderer, printer) = mode_rendering();
        let d1 = DeckId::new(0).unwrap();
        // Track 7 is not in the library yet, so both saves fail.
        load_7(&mut mode, 0, &mut engine, &printer);
        render(&mut renderer);
        let slot1 = engine::HotcueSlot::new(1).unwrap();
        let saved_loop = engine::Hotcue {
            frames: 100.0,
            loop_frames: Some(50.0),
        };
        let mut track = mode.decks[d1.index()].take().unwrap();
        // The loop, then a plain hotcue at the same position: only the loop length differs.
        mode.save_memory(
            &mut track,
            &[
                MemoryAction::SaveHotcue(slot1, saved_loop),
                MemoryAction::SaveHotcue(slot1, engine::Hotcue::point(100.0)),
            ],
            &printer,
        );
        mode.decks[d1.index()] = Some(track);
        flush(&mode);
        assert_eq!(mode.lib.failed_write_count(), 2);
        add_track_7(&mode);
        // Only the latest write is retried, so the loop cannot come back over it.
        mode.report_failed_writes(&printer);
        flush(&mode);
        let points = mode.lib.reader().unwrap().hotcue_points(7).unwrap();
        let point = points[0].as_ref().unwrap();
        assert_eq!((point.frames, point.loop_frames), (100.0, None));
        mode.shutdown(&mut engine, &printer);
        printer.finish();
    }

    #[test]
    fn a_failed_loop_length_change_is_a_new_value_for_the_retry() {
        let (_dir, mut mode, mut engine, printer) = mode();
        let set = |loop_frames| WriteOp::SetHotcue {
            track: 7,
            slot: 3,
            frames: 500.0,
            loop_frames,
        };
        // Track 7 does not exist yet: writes fail. The latest is the loop of 80 frames.
        mode.latest_memory.insert(
            (7, Spot::Hotcue(3)),
            Some((500.0_f64.to_bits(), Some(80.0_f64.to_bits()))),
        );
        for op in [set(None), set(Some(50.0)), set(Some(80.0))] {
            mode.writer.submit(op).unwrap();
        }
        flush(&mode);
        assert_eq!(mode.lib.failed_write_count(), 3);
        mode.report_failed_writes(&printer);
        flush(&mode);
        assert_eq!(
            mode.lib.failed_write_count(),
            4,
            "only the write with the latest loop length is retried"
        );
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
            DeckCommand::SetHotcue(slot1, Some(engine::Hotcue::point(100.0))),
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
        hotcues[0] = Some(engine::Hotcue::point(100.0));
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
            Some(&Some((100.0_f64.to_bits(), None)))
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
            path: PathBuf::from("/m/7.wav"),
            load_seq: 1,
            memory,
            grid: None,
            audio: short_track(),
            buffer: None,
            stems: DeckStems::Unrequested,
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
            .insert((7, Spot::Cue), Some((500.0_f64.to_bits(), None)));
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
            .insert((7, Spot::Cue), Some((900.0_f64.to_bits(), None)));
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
            loop_frames: None,
        };
        let delete = WriteOp::DeleteHotcue { track: 7, slot: 3 };
        // Track 7 does not exist yet: writes fail.
        mode.latest_memory
            .insert((7, Spot::Hotcue(3)), Some((500.0_f64.to_bits(), None)));
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
                loop_frames: None,
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
            hotcues[0] = hotcue.map(engine::Hotcue::point);
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
            Some(&Some((50.0_f64.to_bits(), None)))
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
            hotcues[0] = hotcue.map(engine::Hotcue::point);
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
        assert_eq!(hotcue_1(&mode, 7), Some(Some((100.0_f64.to_bits(), None))));
        assert_eq!(hotcue_1(&mode, 8), None);
        // Track 8's load shows: the outgoing track is done with.
        mode.sync_snapshot(deck, &shown(2, None), &printer);
        assert!(mode.outgoing[0].is_none());
        assert_eq!(hotcue_1(&mode, 8), None);
        mode.shutdown(&mut engine, &printer);
        printer.finish();
    }

    // Stems on load (plan 8, Task 6).

    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

    use stem_jobs::{Reply, SeparatorSource, StemSeparator, StemsSetup};
    use stems::Separator;

    /// The synthetic separator, counting its runs.
    struct Counting {
        calls: Arc<AtomicUsize>,
    }

    impl stems::Separator for Counting {
        fn separate(
            &mut self,
            track: &engine::TrackBuffer,
            progress: &dyn Fn(f32),
            cancel: &std::sync::atomic::AtomicBool,
        ) -> Result<engine::StemSet, stems::SeparateError> {
            self.calls.fetch_add(1, AtomicOrdering::Relaxed);
            stems::SyntheticSeparator::new().separate(track, progress, cancel)
        }
    }

    impl StemSeparator for Counting {}

    /// Separates once: says it `entered`, then reports `report` done every few milliseconds
    /// (as after each chunk) until released, then returns the synthetic stems whatever the
    /// cancel flag says (a result that lost the race with a newer load). Later runs fail.
    struct Scripted {
        calls: Arc<AtomicUsize>,
        report: f32,
        entered: std::sync::mpsc::Sender<()>,
        release: std::sync::mpsc::Receiver<()>,
    }

    impl stems::Separator for Scripted {
        fn separate(
            &mut self,
            track: &engine::TrackBuffer,
            progress: &dyn Fn(f32),
            _cancel: &std::sync::atomic::AtomicBool,
        ) -> Result<engine::StemSet, stems::SeparateError> {
            if self.calls.fetch_add(1, AtomicOrdering::Relaxed) > 0 {
                return Err(stems::SeparateError::Model("only once".into()));
            }
            let _ = self.entered.send(());
            loop {
                progress(self.report);
                match self.release.recv_timeout(Duration::from_millis(2)) {
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                    _ => break,
                }
            }
            stems::SyntheticSeparator::new().separate(
                track,
                &|_| {},
                &std::sync::atomic::AtomicBool::new(false),
            )
        }
    }

    impl StemSeparator for Scripted {}

    fn counting() -> (SeparatorSource, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&calls);
        let source = SeparatorSource::Open(Box::new(move || {
            Ok(Box::new(Counting { calls: counter }) as Box<dyn StemSeparator>)
        }));
        (source, calls)
    }

    /// A [`Scripted`] separator, with its run counter, the `entered` signal and the release.
    fn scripted(
        report: f32,
    ) -> (
        SeparatorSource,
        Arc<AtomicUsize>,
        std::sync::mpsc::Receiver<()>,
        std::sync::mpsc::Sender<()>,
    ) {
        let calls = Arc::new(AtomicUsize::new(0));
        let (entered_tx, entered) = channel();
        let (release, release_rx) = channel();
        let counter = Arc::clone(&calls);
        let source = SeparatorSource::Open(Box::new(move || {
            Ok(Box::new(Scripted {
                calls: counter,
                report,
                entered: entered_tx,
                release: release_rx,
            }) as Box<dyn StemSeparator>)
        }));
        (source, calls, entered, release)
    }

    fn setup(separator: SeparatorSource) -> StemsSetup {
        StemsSetup {
            budget_bytes: 1_000_000_000,
            kicks_hz: engine::DEFAULT_KICKS_HZ,
            separator,
        }
    }

    /// Adds library tracks `ids` (fingerprint `x'00'`) to the library at `db`.
    fn add_tracks(lib: &Library, ids: &[i64]) {
        let statements = ids
            .iter()
            .map(|id| library::RawStatement {
                sql: format!(
                    "INSERT INTO tracks(id, path, fingerprint, added_at) \
                     VALUES ({id}, '/m/{id}.wav', x'00', 0)"
                ),
                params: vec![],
            })
            .collect();
        lib.writer().submit_wait(WriteOp::Raw(statements)).unwrap();
    }

    /// A library mode on a fresh library holding tracks 7 and 8, with stems per `stems`,
    /// its engine and renderer, and a printer whose events are kept. `before` runs on the
    /// library before the mode starts.
    #[allow(clippy::type_complexity)]
    fn stems_mode(
        stems: Option<StemsSetup>,
        before: impl FnOnce(&Library),
    ) -> (
        tempfile::TempDir,
        LibraryMode,
        EngineController,
        engine::EngineRenderer,
        Printer,
        Arc<std::sync::Mutex<Vec<String>>>,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("library.sqlite3");
        {
            let lib = Library::open(&db).unwrap();
            add_tracks(&lib, &[7, 8]);
            before(&lib);
        }
        let mode = LibraryMode::start_with(Some(&db), stems).unwrap();
        let (engine, renderer) = engine::engine(engine::EngineConfig {
            sample_rate: 44_100,
            max_block_frames: 512,
        });
        let (printer, events) = Printer::capturing();
        (dir, mode, engine, renderer, printer, events)
    }

    /// Loads `buffer` as track `id` on `deck` (0-based).
    fn load_track(
        mode: &mut LibraryMode,
        deck: usize,
        id: i64,
        buffer: Arc<engine::TrackBuffer>,
        engine: &mut EngineController,
        printer: &Printer,
    ) {
        let deck = DeckId::new(deck).unwrap();
        let seq = mode.next_seq(deck);
        let next = worker::Loaded {
            deck,
            seq,
            row: TrackRow { id, ..row() },
            buffer,
            ..loaded(seq)
        };
        mode.load(next, engine, printer);
    }

    fn short_track() -> Arc<engine::TrackBuffer> {
        Arc::new(engine::TrackBuffer::new(44_100, vec![0.1; 2 * 4410]).unwrap())
    }

    /// Renders and polls until `done` holds, for at most 10 s.
    fn run_until(
        mode: &mut LibraryMode,
        engine: &mut EngineController,
        renderer: &mut engine::EngineRenderer,
        printer: &Printer,
        mut done: impl FnMut(&LibraryMode, &EngineController) -> bool,
    ) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            render(renderer);
            mode.poll(Instant::now(), engine, printer);
            if done(mode, engine) {
                return;
            }
            assert!(Instant::now() < deadline, "not done within 10 s");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// The status line of deck `n` (0-based), if loaded.
    fn deck_line_of(mode: &LibraryMode, engine: &EngineController, n: usize) -> String {
        let slots = [[engine::EffectKind::Echo; 3]; 4];
        let lines = mode.status_lines(engine, &slots);
        lines
            .into_iter()
            .find(|l| l.contains(&format!("Deck {}", n + 1)))
            .unwrap_or_default()
    }

    fn stems_rows(mode: &LibraryMode, track: i64) -> i64 {
        mode.lib
            .reader()
            .unwrap()
            .connection()
            .query_row(
                "SELECT count(*) FROM stems WHERE track_id = ?1",
                [track],
                |r| r.get(0),
            )
            .unwrap()
    }

    fn idle(mode: &LibraryMode) -> bool {
        mode.stems.as_ref().is_some_and(|s| s.idle())
    }

    #[test]
    fn cached_stems_are_sent_after_load() {
        let track = short_track();
        let set = stems::SyntheticSeparator::new()
            .separate(&track, &|_| {}, &std::sync::atomic::AtomicBool::new(false))
            .unwrap();
        let (source, calls) = counting();
        let (_dir, mut mode, mut engine, mut renderer, printer, _events) =
            stems_mode(Some(setup(source)), |lib| {
                let dir = crate::library_cmd::stems_dir(lib.path());
                let mut cache = library::StemCache::new(lib, dir, 1_000_000_000).unwrap();
                let key = library::StemKey {
                    track: 7,
                    fingerprint: &[0],
                    sample_rate: 44_100,
                    frames: 4410,
                };
                cache.put(&key, &set, false, &[]).unwrap();
            });
        let d1 = DeckId::new(0).unwrap();
        load_track(&mut mode, 0, 7, track, &mut engine, &printer);
        // Nothing is asked for before the engine shows the load.
        assert!(idle(&mode));
        run_until(&mut mode, &mut engine, &mut renderer, &printer, |_, e| {
            e.deck(d1).stems
        });
        assert_eq!(calls.load(AtomicOrdering::Relaxed), 0, "no separation");
        flush(&mode);
        let loaded_at: Option<i64> = mode
            .lib
            .reader()
            .unwrap()
            .connection()
            .query_row("SELECT last_loaded_at FROM tracks WHERE id = 7", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert!(loaded_at.is_some(), "the load is recorded for eviction");
        assert!(deck_line_of(&mode, &engine, 0).contains("  stems  "));
        mode.shutdown(&mut engine, &printer);
        printer.finish();
    }

    #[test]
    fn separation_result_reaches_the_same_track() {
        let (source, calls) = counting();
        let (_dir, mut mode, mut engine, mut renderer, printer, _events) =
            stems_mode(Some(setup(source)), |_| {});
        let (d1, d3) = (DeckId::new(0).unwrap(), DeckId::new(2).unwrap());
        // The same track on two decks: one separation serves both.
        load_track(&mut mode, 0, 7, short_track(), &mut engine, &printer);
        load_track(&mut mode, 2, 7, short_track(), &mut engine, &printer);
        run_until(&mut mode, &mut engine, &mut renderer, &printer, |_, e| {
            e.deck(d1).stems && e.deck(d3).stems
        });
        run_until(&mut mode, &mut engine, &mut renderer, &printer, |m, _| {
            idle(m) && stems_rows(m, 7) == 1
        });
        assert_eq!(calls.load(AtomicOrdering::Relaxed), 1);
        mode.shutdown(&mut engine, &printer);
        printer.finish();
    }

    #[test]
    fn stale_separation_never_reaches_a_new_track() {
        let (source, calls, entered, release) = scripted(0.5);
        let (_dir, mut mode, mut engine, mut renderer, printer, _events) =
            stems_mode(Some(setup(source)), |_| {});
        let d1 = DeckId::new(0).unwrap();
        load_track(&mut mode, 0, 7, short_track(), &mut engine, &printer);
        run_until(&mut mode, &mut engine, &mut renderer, &printer, |_, _| {
            entered.try_recv().is_ok()
        });
        let old_job = match mode.decks[0].as_ref().unwrap().stems {
            DeckStems::Job(job) => job,
            ref other => panic!("{other:?}"),
        };
        // Track 8 replaces track 7 while 7 is separating; 7's stems then come in late.
        load_track(&mut mode, 0, 8, short_track(), &mut engine, &printer);
        run_until(&mut mode, &mut engine, &mut renderer, &printer, |m, _| {
            m.decks[0].as_ref().unwrap().memory.load_applied()
        });
        release.send(()).unwrap();
        run_until(&mut mode, &mut engine, &mut renderer, &printer, |m, _| {
            idle(m) && calls.load(AtomicOrdering::Relaxed) == 2
        });
        render(&mut renderer);
        assert!(
            !engine.deck(d1).stems,
            "track 7's stems never reach track 8"
        );
        assert_eq!(
            stems_rows(&mode, 7),
            1,
            "a result no deck wanted is still kept (Ruling R14)"
        );
        // Even a result for the old job that does reach the control loop is dropped.
        let set = stems::SyntheticSeparator::new()
            .separate(
                &short_track(),
                &|_| {},
                &std::sync::atomic::AtomicBool::new(false),
            )
            .unwrap();
        mode.on_stems_reply(Reply::Stems(old_job, Arc::new(set)), &mut engine, &printer);
        render(&mut renderer);
        assert!(!engine.deck(d1).stems);
        mode.shutdown(&mut engine, &printer);
        printer.finish();
    }

    #[test]
    fn cached_stems_arrive_during_a_separation() {
        let set = stems::SyntheticSeparator::new()
            .separate(
                &short_track(),
                &|_| {},
                &std::sync::atomic::AtomicBool::new(false),
            )
            .unwrap();
        let (source, _calls, entered, release) = scripted(0.5);
        let (_dir, mut mode, mut engine, mut renderer, printer, _events) =
            stems_mode(Some(setup(source)), |lib| {
                let dir = crate::library_cmd::stems_dir(lib.path());
                let mut cache = library::StemCache::new(lib, dir, 1_000_000_000).unwrap();
                let key = library::StemKey {
                    track: 8,
                    fingerprint: &[0],
                    sample_rate: 44_100,
                    frames: 4410,
                };
                cache.put(&key, &set, false, &[]).unwrap();
            });
        let (d1, d2) = (DeckId::new(0).unwrap(), DeckId::new(1).unwrap());
        load_track(&mut mode, 0, 7, short_track(), &mut engine, &printer);
        run_until(&mut mode, &mut engine, &mut renderer, &printer, |_, _| {
            entered.try_recv().is_ok()
        });
        // Track 7 is separating (and stays so until released): cached track 8 still comes.
        load_track(&mut mode, 1, 8, short_track(), &mut engine, &printer);
        run_until(&mut mode, &mut engine, &mut renderer, &printer, |_, e| {
            e.deck(d2).stems
        });
        assert!(!engine.deck(d1).stems, "track 7 is still separating");
        release.send(()).unwrap();
        run_until(&mut mode, &mut engine, &mut renderer, &printer, |_, e| {
            e.deck(d1).stems
        });
        mode.shutdown(&mut engine, &printer);
        printer.finish();
    }

    /// Reports its only chunk done, says it `entered`, then waits to be released before
    /// returning the synthetic stems: a job sent meanwhile arrives after the last chunk.
    struct LastChunk {
        entered: std::sync::mpsc::Sender<()>,
        release: std::sync::mpsc::Receiver<()>,
    }

    impl stems::Separator for LastChunk {
        fn separate(
            &mut self,
            track: &engine::TrackBuffer,
            progress: &dyn Fn(f32),
            cancel: &std::sync::atomic::AtomicBool,
        ) -> Result<engine::StemSet, stems::SeparateError> {
            progress(1.0);
            let _ = self.entered.send(());
            let _ = self.release.recv();
            stems::SyntheticSeparator::new().separate(track, &|_| {}, cancel)
        }
    }

    impl StemSeparator for LastChunk {}

    #[test]
    fn cached_stems_requested_after_the_last_chunk_skip_the_cache_write() {
        let set = stems::SyntheticSeparator::new()
            .separate(
                &short_track(),
                &|_| {},
                &std::sync::atomic::AtomicBool::new(false),
            )
            .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let lib = Library::open(&dir.path().join("library.sqlite3")).unwrap();
        add_tracks(&lib, &[7, 8]);
        {
            let stems_dir = crate::library_cmd::stems_dir(lib.path());
            let mut cache = library::StemCache::new(&lib, stems_dir, 1_000_000_000).unwrap();
            let key = library::StemKey {
                track: 8,
                fingerprint: &[0],
                sample_rate: 44_100,
                frames: 4410,
            };
            cache.put(&key, &set, false, &[]).unwrap();
        }
        let (entered_tx, entered) = channel();
        let (release, release_rx) = channel();
        let source = SeparatorSource::Open(Box::new(move || {
            Ok(Box::new(LastChunk {
                entered: entered_tx,
                release: release_rx,
            }) as Box<dyn StemSeparator>)
        }));
        let mut control = stem_jobs::StemsControl::start(Arc::new(lib), setup(source)).unwrap();
        let d1 = DeckId::new(0).unwrap();
        let separated = control
            .request(d1, 7, "seven".into(), short_track())
            .unwrap();
        entered.recv_timeout(Duration::from_secs(10)).unwrap();
        // Track 8 is asked for once track 7's last chunk is done, before its cache write.
        let cached = control
            .request(DeckId::new(1).unwrap(), 8, "eight".into(), short_track())
            .unwrap();
        release.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut replies = Vec::new();
        while !replies
            .iter()
            .any(|r| matches!(r, Reply::Done(id) if *id == separated))
        {
            assert!(Instant::now() < deadline, "not done within 10 s");
            std::thread::sleep(Duration::from_millis(2));
            replies.extend(control.take_replies());
        }
        let position = |wanted: &dyn Fn(&Reply) -> bool| replies.iter().position(wanted);
        let hit = position(&|r| matches!(r, Reply::Stems(id, _) if *id == cached))
            .expect("track 8's cached stems");
        let written = position(&|r| matches!(r, Reply::Evicted(_))).expect("track 7's write");
        assert!(
            hit < written,
            "the hit waited for the cache write: {replies:?}"
        );
        control.shutdown();
    }

    #[test]
    fn missing_model_is_reported_once() {
        let (_dir, mut mode, mut engine, mut renderer, printer, events) =
            stems_mode(Some(setup(SeparatorSource::Missing)), |_| {});
        load_track(&mut mode, 0, 7, short_track(), &mut engine, &printer);
        load_track(&mut mode, 1, 8, short_track(), &mut engine, &printer);
        run_until(&mut mode, &mut engine, &mut renderer, &printer, |m, _| {
            idle(m)
                && m.decks[..2]
                    .iter()
                    .all(|t| matches!(t.as_ref().unwrap().stems, DeckStems::Done))
        });
        let lines = [0, 1].map(|n| deck_line_of(&mode, &engine, n));
        mode.shutdown(&mut engine, &printer);
        printer.finish();
        let events = events.lock().unwrap();
        let stems: Vec<&String> = events.iter().filter(|e| e.contains("stems")).collect();
        assert_eq!(
            stems,
            ["stems: no model, run `open-jockey stems setup`"],
            "{events:?}"
        );
        for line in lines {
            assert!(!line.contains("stems"), "{line}");
        }
    }

    #[test]
    fn long_tracks_are_refused() {
        let (source, calls) = counting();
        let (_dir, mut mode, mut engine, mut renderer, printer, _events) =
            stems_mode(Some(setup(source)), |_| {});
        // 20 minutes and one second, at a low rate to keep the test small.
        let rate = 1_000;
        let frames = (stems::MAX_SECONDS as usize + 1) * rate;
        let long = Arc::new(engine::TrackBuffer::new(rate as u32, vec![0.0; 2 * frames]).unwrap());
        load_track(&mut mode, 0, 7, long, &mut engine, &printer);
        run_until(&mut mode, &mut engine, &mut renderer, &printer, |m, e| {
            deck_line_of(m, e, 0).contains("  no stems (too long)")
        });
        assert!(idle(&mode));
        assert_eq!(calls.load(AtomicOrdering::Relaxed), 0);
        mode.shutdown(&mut engine, &printer);
        printer.finish();
    }

    #[test]
    fn status_shows_progress() {
        const SLOTS: [engine::EffectKind; 3] = [engine::EffectKind::Echo; 3];
        const CH: engine::ChannelSnapshot = engine::ChannelSnapshot {
            fx_enabled: [false; 3],
            fx_amount: 0.5,
        };
        let snap = DeckSnapshot {
            loaded: true,
            position_frames: 44_100.0 * 65.0,
            duration_frames: 44_100 * 200,
            track_sample_rate: 44_100,
            tempo_ratio: 1.0,
            ..DeckSnapshot::default()
        };
        let deck = DeckId::new(0).unwrap();
        let line = |stems| deck_status(deck, &snap, Some(&row()), &SLOTS, &CH, stems);
        assert_eq!(
            line(Some("stems 40%")),
            " Deck 1  01:05 / 03:20  stems 40%  128.00 BPM  Artist – Title"
        );
        assert_eq!(
            line(Some("stems")),
            " Deck 1  01:05 / 03:20  stems  128.00 BPM  Artist – Title"
        );
        assert_eq!(
            line(Some("no stems (too long)")),
            " Deck 1  01:05 / 03:20  no stems (too long)  128.00 BPM  Artist – Title"
        );

        // Live: the separation's progress, then the stems.
        let (source, _calls, entered, release) = scripted(0.4);
        let (_dir, mut mode, mut engine, mut renderer, printer, _events) =
            stems_mode(Some(setup(source)), |_| {});
        load_track(&mut mode, 0, 7, short_track(), &mut engine, &printer);
        run_until(&mut mode, &mut engine, &mut renderer, &printer, |_, _| {
            entered.try_recv().is_ok()
        });
        run_until(&mut mode, &mut engine, &mut renderer, &printer, |m, e| {
            deck_line_of(m, e, 0).contains("  stems 40%  ")
        });
        release.send(()).unwrap();
        run_until(&mut mode, &mut engine, &mut renderer, &printer, |m, e| {
            deck_line_of(m, e, 0).contains("  stems  ")
        });
        mode.shutdown(&mut engine, &printer);
        printer.finish();
    }

    #[test]
    fn browsing_a_new_folder_adds_its_files_once() {
        let dir = tempfile::tempdir().unwrap();
        let music = dunce::canonicalize(dir.path()).unwrap().join("music");
        std::fs::create_dir_all(&music).unwrap();
        let fixtures = concat!(env!("CARGO_MANIFEST_DIR"), "/../decode/tests/fixtures/");
        std::fs::copy(format!("{fixtures}sine440.wav"), music.join("a.wav")).unwrap();
        std::fs::copy(format!("{fixtures}sine440_mono.wav"), music.join("b.wav")).unwrap();
        let drives = vec![library::Drive {
            name: "music".into(),
            path: music,
        }];
        let db = dir.path().join("library.sqlite3");
        let mut mode = LibraryMode::start_browsing(Some(&db), None, Some(drives)).unwrap();
        let (mut engine, mut renderer) = engine::engine(engine::EngineConfig {
            sample_rate: 44_100,
            max_block_frames: 512,
        });
        let printer = Printer::spawn();
        run_until(&mut mode, &mut engine, &mut renderer, &printer, |m, _| {
            m.browser
                .as_ref()
                .is_some_and(|b| b.list.len() == 2 && b.list.iter().all(|r| r.track.is_some()))
        });
        assert_eq!(
            mode.reader().unwrap().folders().unwrap(),
            Vec::<std::path::PathBuf>::new()
        );
        mode.shutdown(&mut engine, &printer);
        printer.finish();
    }

    #[test]
    fn a_change_made_elsewhere_rereads_the_browser() {
        let dir = tempfile::tempdir().unwrap();
        let mut mode = start(&dir.path().join("library.sqlite3"));
        let (mut engine, _renderer) = engine::engine(engine::EngineConfig {
            sample_rate: 44_100,
            max_block_frames: 512,
        });
        let (printer, _events) = Printer::capturing();
        mode.refresh_wanted = false;
        let changed = mode.changed_flag();
        changed.store(true, Ordering::Relaxed);
        mode.poll(mode.last_poll, &mut engine, &printer);
        assert!(mode.refresh_wanted);
        assert!(!changed.load(Ordering::Relaxed));
        printer.finish();
    }
}
