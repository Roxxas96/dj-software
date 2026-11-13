//! The library worker thread: everything library mode does with the database (browser
//! moves, track lookups, grid edits), and a loader thread that decodes tracks, so the
//! control loop never waits on either.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::JoinHandle;

use engine::{DeckId, Hotcue, TrackBuffer, TrackMemory};
use library::{
    Beatgrid, Browser, BrowserEdit, BrowserRequest, BrowserView, Drive, GridEdit, Library,
    PendingTrack, Reader, TrackRow, WriteOp,
};

use super::memory_sync::engine_grid;
use crate::format::safe;

/// The event when a playlist edit finds the entries it names moved since the window saw
/// them: nothing is written.
pub const PLAYLIST_CHANGED: &str = "The playlist changed; try again";

/// Work for the library worker.
#[derive(Debug)]
pub enum Job {
    Browse(BrowserRequest),
    /// A change to the browser from the window.
    Edit(BrowserEdit),
    /// Reread the browser, and the rows and beatgrids of the tracks on the decks; with
    /// `find_new_tracks`, also report the tracks that need analysis (the watcher found
    /// some).
    Refresh {
        decks: Vec<(DeckId, i64)>,
        find_new_tracks: bool,
    },
    /// Load the browser's selected track; `seq` tells a stale load from the latest.
    LoadSelected {
        deck: DeckId,
        seq: u64,
    },
    LoadTrack {
        deck: DeckId,
        track: i64,
        seq: u64,
        /// Drop the load if the deck plays once the track is decoded (the window's loads).
        refuse_if_playing: bool,
    },
    /// Write a playlist edit from the window, then reread the browser to show it.
    EditPlaylist(WriteOp),
    /// Append every track of the browser's list to `playlist` (a whole-list selection);
    /// the tracks are read here, when the job runs, then written as
    /// [`Job::EditPlaylist`] does.
    AddListToPlaylist {
        playlist: i64,
    },
    /// Make the playlist `name` (trimmed, not empty) unless a playlist has that name,
    /// reread the browser, and highlight the new playlist's tree row.
    CreatePlaylist(String),
    Grid {
        deck: DeckId,
        track: i64,
        edit: GridEdit,
        playhead: Option<f64>,
        /// The decoded track's sample rate, for beat periods.
        sample_rate: Option<u32>,
    },
}

/// What the worker reports to the control loop.
#[derive(Debug)]
pub enum Event {
    Browser(BrowserView),
    /// A line to print.
    Message(String),
    Loaded(Box<Loaded>),
    Grid {
        deck: DeckId,
        track: i64,
        grid: Beatgrid,
    },
    /// A deck's track as the library has it now.
    Track {
        deck: DeckId,
        row: TrackRow,
        grid: Option<Beatgrid>,
    },
    /// Tracks that need analysis.
    NewTracks(Vec<PendingTrack>),
    /// A browsed folder's files were added to the library: reread the browser and look
    /// for tracks to analyze.
    FolderAdded,
}

/// A decoded track, ready for the deck.
#[derive(Debug)]
pub struct Loaded {
    pub deck: DeckId,
    pub seq: u64,
    pub row: TrackRow,
    pub path: PathBuf,
    pub buffer: Arc<TrackBuffer>,
    /// The remembered cue point, hotcues and beatgrid (at the decoded rate).
    pub memory: TrackMemory,
    /// The hotcues could be read. If not, `memory` has none and the deck's hotcues are not
    /// saved for this load (Ruling R19), so its empty pads never overwrite stored ones.
    pub hotcues_read: bool,
    /// The load is dropped if the deck plays when it arrives (the window's loads).
    pub refuse_if_playing: bool,
}

struct LoadJob {
    deck: DeckId,
    seq: u64,
    refuse_if_playing: bool,
    row: TrackRow,
    path: PathBuf,
}

/// Starts the worker (and its loader), browsing `drives`, or the computer's drives when
/// `None`. It stops when the returned sender is dropped.
pub fn spawn(
    lib: Arc<Library>,
    events: Sender<Event>,
    drives: Option<Vec<Drive>>,
) -> std::io::Result<(Sender<Job>, JoinHandle<()>)> {
    // Unit tests must not read the real disks.
    let roots = move || {
        drives.clone().unwrap_or_else(|| {
            if cfg!(test) {
                Vec::new()
            } else {
                library::drives()
            }
        })
    };
    spawn_with(lib, events, decode::decode_file, roots)
}

/// [`spawn`] with the loader decoding through `decode` and the browser's tree starting
/// at the drives `roots` gives (called on the worker thread, again at each refresh).
fn spawn_with(
    lib: Arc<Library>,
    events: Sender<Event>,
    decode: DecodeFn,
    roots: impl Fn() -> Vec<Drive> + Send + 'static,
) -> std::io::Result<(Sender<Job>, JoinHandle<()>)> {
    let (tx, rx) = channel();
    let handle = std::thread::Builder::new()
        .name("library-worker".into())
        .spawn(move || run(&lib, &rx, &events, decode, roots))?;
    Ok((tx, handle))
}

/// The event for a playlist name another playlist has.
fn name_taken(name: &str) -> String {
    format!("There is already a playlist called {name}")
}

fn deck_number(deck: DeckId) -> usize {
    deck.index() + 1
}

/// How the loader decodes a file (the real decoder, or a stand-in in tests).
type DecodeFn = fn(&Path) -> Result<decode::Decoded, decode::DecodeError>;

fn run(
    lib: &Library,
    jobs: &Receiver<Job>,
    events: &Sender<Event>,
    decode: DecodeFn,
    roots: impl Fn() -> Vec<Drive> + Send + 'static,
) {
    let say = |text: String| {
        let _ = events.send(Event::Message(text));
    };
    let readers = (
        lib.reader(),
        lib.reader().and_then(|r| Browser::with_roots(r, roots)),
        lib.reader(),
    );
    let (reader, mut browser, loader_reader) = match readers {
        (Ok(reader), Ok(browser), Ok(loader_reader)) => (reader, browser, loader_reader),
        (Err(e), _, _) | (_, Err(e), _) | (_, _, Err(e)) => {
            say(format!("error: cannot read the library: {}", safe(e)));
            return;
        }
    };
    let _ = events.send(Event::Browser(browser.view()));
    let adder = FolderAdder::start(lib, events);
    adder.offer(&browser);
    let (load_tx, load_rx) = channel::<LoadJob>();
    let loader_events = events.clone();
    let loader = std::thread::Builder::new()
        .name("track-loader".into())
        .spawn(move || load_tracks(&loader_reader, &load_rx, &loader_events, decode));
    if let Err(e) = &loader {
        say(format!("error: cannot start the track loader: {e}"));
    }

    for job in jobs {
        let job = match job {
            Job::AddListToPlaylist { playlist } => match browser.list_track_ids() {
                Ok(tracks) => Job::EditPlaylist(super::playlist_edits::append(playlist, &tracks)),
                Err(e) => {
                    say(format!("warning: library browser: {}", safe(e)));
                    continue;
                }
            },
            job => job,
        };
        match job {
            Job::Browse(req) => match browser.apply(req) {
                Ok(view) => {
                    let _ = events.send(Event::Browser(view));
                    adder.offer(&browser);
                }
                Err(e) => say(format!("warning: library browser: {}", safe(e))),
            },
            Job::Edit(edit) => match browser.edit(edit) {
                Ok(view) => {
                    let _ = events.send(Event::Browser(view));
                    adder.offer(&browser);
                }
                Err(e) => {
                    say(format!("warning: library browser: {}", safe(e)));
                    // The view as it stands, answering the query: the window drops views
                    // older than the last query it sent.
                    let _ = events.send(Event::Browser(browser.view()));
                }
            },
            Job::AddListToPlaylist { .. } => {}
            Job::EditPlaylist(op) => {
                // Waited for, so the reread sees it.
                match lib.writer().submit_wait(op) {
                    Ok(_) => {}
                    Err(library::LibraryError::PlaylistChanged(_)) => {
                        say(PLAYLIST_CHANGED.to_owned());
                    }
                    Err(library::LibraryError::PlaylistNameTaken(name)) => say(name_taken(&name)),
                    Err(e) => say(format!(
                        "warning: could not change the playlist: {}",
                        safe(e)
                    )),
                }
                match browser.refresh() {
                    Ok(view) => {
                        let _ = events.send(Event::Browser(view));
                    }
                    Err(e) => say(format!("warning: library browser: {}", safe(e))),
                }
            }
            Job::CreatePlaylist(name) => {
                // Checked in the write's transaction: no other create slips in between.
                let created = lib.writer().submit_wait(WriteOp::Batch(vec![
                    WriteOp::ExpectNoPlaylistNamed {
                        name: name.clone(),
                        except: None,
                    },
                    WriteOp::CreatePlaylist { name },
                ]));
                let id = match created {
                    Ok(library::WriteOutcome::Created { id }) => Some(id),
                    Ok(_) => None,
                    Err(library::LibraryError::PlaylistNameTaken(name)) => {
                        say(name_taken(&name));
                        None
                    }
                    Err(e) => {
                        say(format!(
                            "warning: could not create the playlist: {}",
                            safe(e)
                        ));
                        None
                    }
                };
                match browser.refresh() {
                    Ok(view) => {
                        // Its row is there when the Playlists node is open.
                        let row = id.and_then(|id| {
                            view.tree.iter().position(|row| row.playlist == Some(id))
                        });
                        let shown = match row {
                            Some(at) => browser.edit(BrowserEdit::SelectTree(at)),
                            None => Ok(view),
                        };
                        match shown {
                            Ok(view) => {
                                let _ = events.send(Event::Browser(view));
                            }
                            Err(e) => say(format!("warning: library browser: {}", safe(e))),
                        }
                    }
                    Err(e) => say(format!("warning: library browser: {}", safe(e))),
                }
            }
            Job::Refresh {
                decks,
                find_new_tracks,
            } => {
                match browser.refresh() {
                    Ok(view) => {
                        let _ = events.send(Event::Browser(view));
                        adder.offer(&browser);
                    }
                    Err(e) => say(format!("warning: library browser: {}", safe(e))),
                }
                if find_new_tracks {
                    match reader.tracks_to_analyze() {
                        Ok(tracks) => {
                            let _ = events.send(Event::NewTracks(tracks));
                        }
                        Err(e) => say(format!(
                            "warning: could not look for tracks to analyze: {}",
                            safe(e)
                        )),
                    }
                }
                for (deck, id) in decks {
                    if let (Ok(Some(details)), Ok(grid)) = (reader.track(id), reader.beatgrid(id)) {
                        let _ = events.send(Event::Track {
                            deck,
                            row: details.row,
                            grid,
                        });
                    }
                }
            }
            Job::LoadSelected { deck, seq } => match browser.selected_track() {
                Some(row) => request_load(&reader, &load_tx, (deck, row.id, seq, false), &say),
                None => say(format!(
                    "Deck {}: nothing selected in the browser",
                    deck_number(deck)
                )),
            },
            Job::LoadTrack {
                deck,
                track,
                seq,
                refuse_if_playing,
            } => {
                let load = (deck, track, seq, refuse_if_playing);
                request_load(&reader, &load_tx, load, &say);
            }
            Job::Grid {
                deck,
                track,
                edit,
                playhead,
                sample_rate,
            } => match lib.edit_grid_at_rate(track, edit, playhead, sample_rate) {
                Ok(grid) => {
                    let _ = events.send(Event::Grid { deck, track, grid });
                }
                Err(e) => say(format!(
                    "Deck {}: beatgrid not changed: {}",
                    deck_number(deck),
                    safe(e)
                )),
            },
        }
    }
    drop(load_tx);
    adder.stop();
    if let Ok(loader) = loader {
        let _ = loader.join();
    }
}

/// Adds the files of the folder being browsed to the library on a thread of its own, so
/// a slow disk never holds up the worker's browsing and loading.
struct FolderAdder {
    /// The newest folder waiting: a newer one replaces an older one still waiting.
    next: Arc<(Mutex<Option<PathBuf>>, Condvar)>,
    /// Folders already tried this session, so a file that cannot be added (a corrupt
    /// one) doesn't queue its folder at every refresh.
    tried: RefCell<HashSet<PathBuf>>,
    /// False when the thread could not be started.
    running: bool,
    closed: Arc<AtomicBool>,
}

impl FolderAdder {
    fn start(lib: &Library, events: &Sender<Event>) -> Self {
        let next = Arc::new((Mutex::new(None::<PathBuf>), Condvar::new()));
        let closed = Arc::new(AtomicBool::new(false));
        let (writer, reader) = (lib.writer(), lib.reader());
        let (queue, shut, events) = (Arc::clone(&next), Arc::clone(&closed), events.clone());
        let thread = reader.ok().and_then(|reader| {
            std::thread::Builder::new()
                .name("folder-adder".into())
                .spawn(move || {
                    loop {
                        let folder = {
                            let (lock, wake) = &*queue;
                            let mut slot = lock.lock().unwrap_or_else(PoisonError::into_inner);
                            loop {
                                if shut.load(Ordering::Acquire) {
                                    return;
                                }
                                if let Some(folder) = slot.take() {
                                    break folder;
                                }
                                slot = wake.wait(slot).unwrap_or_else(PoisonError::into_inner);
                            }
                        };
                        match library::add_folder_files(&writer, reader.connection(), &folder) {
                            Ok(_) => {
                                let _ = events.send(Event::FolderAdded);
                            }
                            Err(e) => {
                                let _ = events.send(Event::Message(format!(
                                    "warning: could not add the files of {}: {}",
                                    safe(folder.display()),
                                    safe(e)
                                )));
                            }
                        }
                    }
                })
                .ok()
        });
        Self {
            next,
            tried: RefCell::new(HashSet::new()),
            running: thread.is_some(),
            closed,
        }
    }

    /// Queues the browser's highlighted folder if it lists files the library lacks and
    /// was not tried yet.
    fn offer(&self, browser: &Browser) {
        let Some(folder) = browser.folder_with_unknown_files() else {
            return;
        };
        if !self.running || !self.tried.borrow_mut().insert(folder.clone()) {
            return;
        }
        let (lock, wake) = &*self.next;
        *lock.lock().unwrap_or_else(PoisonError::into_inner) = Some(folder);
        wake.notify_one();
    }

    /// Tells the thread to end once its current folder is done (it is not waited for, so
    /// a slow disk never holds up shutdown).
    fn stop(&self) {
        self.closed.store(true, Ordering::Release);
        self.next.1.notify_one();
    }
}

/// Looks the track up and hands it to the loader, unless its file is missing.
/// Asks the loader to decode `(deck, track, seq, refuse_if_playing)`'s track.
fn request_load(
    reader: &Reader,
    loader: &Sender<LoadJob>,
    (deck, track, seq, refuse_if_playing): (DeckId, i64, u64, bool),
    say: &dyn Fn(String),
) {
    let n = deck_number(deck);
    let details = match reader.track(track) {
        Ok(Some(details)) => details,
        Ok(None) => return say(format!("Deck {n}: there is no track {track}")),
        Err(e) => return say(format!("Deck {n}: cannot load track {track}: {}", safe(e))),
    };
    if details.row.missing || !details.path.exists() {
        let title = safe(details.row.title.as_deref().unwrap_or("(untitled)"));
        return say(format!(
            "Deck {n}: cannot load {title} — file is missing ({})",
            safe(details.path.display())
        ));
    }
    let job = LoadJob {
        deck,
        seq,
        refuse_if_playing,
        row: details.row,
        path: details.path,
    };
    if loader.send(job).is_err() {
        say(format!(
            "Deck {n}: cannot load track {track}: the loader stopped"
        ));
    }
}

/// Track `track`'s cue point, hotcues and beatgrid, the grid at the decoded `rate`, and
/// whether the hotcues could be read. A part that cannot be read is left out (the track
/// still loads), with a warning.
fn read_memory(
    reader: &Reader,
    track: i64,
    rate: u32,
    events: &Sender<Event>,
) -> (TrackMemory, bool) {
    fn read_or_warn<T>(
        result: Result<T, library::LibraryError>,
        what: &str,
        consequence: &str,
        track: i64,
        events: &Sender<Event>,
    ) -> Option<T> {
        result
            .inspect_err(|e| {
                let _ = events.send(Event::Message(format!(
                    "warning: could not read the {what} of track {track}: {}{consequence}",
                    safe(e)
                )));
            })
            .ok()
    }
    let cue = read_or_warn(reader.cue(track), "cue point", "", track, events).flatten();
    let hotcues = read_or_warn(
        reader.hotcue_points(track),
        "hotcues",
        "; hotcues won't be remembered for this load",
        track,
        events,
    );
    let grid = read_or_warn(reader.beatgrid(track), "beatgrid", "", track, events).flatten();
    let memory = TrackMemory {
        cue: cue.unwrap_or(0.0),
        hotcues: hotcues
            .as_ref()
            .map(|h| {
                h.clone().map(|point| {
                    point.map(|p| Hotcue {
                        frames: p.frames,
                        loop_frames: p.loop_frames,
                    })
                })
            })
            .unwrap_or_default(),
        // Grid periods at the decoded rate, which is what the deck plays.
        grid: engine_grid(grid.as_ref(), rate),
    };
    (memory, hotcues.is_some())
}

/// Decodes queued tracks; of several queued for one deck only the latest is decoded.
fn load_tracks(
    reader: &Reader,
    jobs: &Receiver<LoadJob>,
    events: &Sender<Event>,
    decode: DecodeFn,
) {
    while let Ok(first) = jobs.recv() {
        let mut latest: HashMap<usize, LoadJob> = HashMap::new();
        for job in std::iter::once(first).chain(jobs.try_iter()) {
            latest.insert(job.deck.index(), job);
        }
        let mut batch: Vec<LoadJob> = latest.into_values().collect();
        batch.sort_by_key(|j| j.deck.index());
        for job in batch {
            load_one(reader, job, events, decode);
        }
    }
}

/// Decodes one track, then reads its memory (so a change saved during the decode is
/// loaded). A decoder panic (say, on a malformed file) fails this load only: it is
/// reported and the loader goes on.
fn load_one(reader: &Reader, job: LoadJob, events: &Sender<Event>, decode: DecodeFn) {
    let n = deck_number(job.deck);
    let result = catch_unwind(AssertUnwindSafe(|| decode(&job.path)))
        .map_err(|_| "the decoder crashed on this file".to_owned())
        .and_then(|r| r.map_err(|e| e.to_string()));
    match result {
        Ok(decoded) => {
            if decoded.truncated {
                let _ = events.send(Event::Message(format!(
                    "warning: {} may be incomplete (decoding stopped early)",
                    safe(job.path.display())
                )));
            }
            let (memory, hotcues_read) =
                read_memory(reader, job.row.id, decoded.track.sample_rate(), events);
            let _ = events.send(Event::Loaded(Box::new(Loaded {
                deck: job.deck,
                seq: job.seq,
                row: job.row,
                path: job.path,
                buffer: Arc::new(decoded.track),
                memory,
                hotcues_read,
                refuse_if_playing: job.refuse_if_playing,
            })));
        }
        Err(e) => {
            let _ = events.send(Event::Message(format!(
                "Deck {n}: cannot load {}: {}",
                crate::format::track_name(&job.row),
                safe(e)
            )));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use engine::HOTCUES;
    use library::{RawStatement, ScanProgress, TrackFilter, TrackSort, WriteOp};

    use super::*;

    const FIXTURE: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../decode/tests/fixtures/sine440.wav"
    );

    struct Setup {
        _dir: tempfile::TempDir,
        music: PathBuf,
        lib: Arc<Library>,
    }

    /// A library with the fixture copied in as each of `names` and scanned.
    fn setup(names: &[&str]) -> Setup {
        let dir = tempfile::tempdir().unwrap();
        let music = dunce::canonicalize(dir.path()).unwrap().join("music");
        std::fs::create_dir_all(&music).unwrap();
        for name in names {
            std::fs::copy(FIXTURE, music.join(name)).unwrap();
        }
        let lib = Library::open(&dir.path().join("library.sqlite3")).unwrap();
        library::add_folder(&lib, &music).unwrap();
        library::scan_folder(&lib, &music, &mut |_: ScanProgress| {}).unwrap();
        Setup {
            _dir: dir,
            music,
            lib: Arc::new(lib),
        }
    }

    /// Every track id, by artist, then id.
    fn ids(lib: &Library) -> Vec<i64> {
        lib.reader()
            .unwrap()
            .tracks(&TrackFilter::default(), TrackSort::default(), 0, 100)
            .unwrap()
            .into_iter()
            .map(|r| r.id)
            .collect()
    }

    fn deck(n: usize) -> DeckId {
        DeckId::new(n - 1).unwrap()
    }

    /// The next event `matches` accepts, skipping others, within 10 s.
    fn wait_for<T>(events: &Receiver<Event>, mut matches: impl FnMut(Event) -> Option<T>) -> T {
        loop {
            let event = events
                .recv_timeout(Duration::from_secs(10))
                .expect("no matching event within 10 s");
            if let Some(found) = matches(event) {
                return found;
            }
        }
    }

    fn message(event: Event) -> Option<String> {
        match event {
            Event::Message(text) => Some(text),
            _ => None,
        }
    }

    #[test]
    fn loading_a_missing_file_fails_clearly() {
        let s = setup(&["gone.wav"]);
        let id = ids(&s.lib)[0];
        std::fs::remove_file(s.music.join("gone.wav")).unwrap();
        let (tx, events) = channel();
        let (jobs, worker) = spawn(Arc::clone(&s.lib), tx, None).unwrap();
        jobs.send(Job::LoadTrack {
            deck: deck(2),
            track: id,
            seq: 1,
            refuse_if_playing: false,
        })
        .unwrap();
        let text = wait_for(&events, message);
        assert!(
            text.starts_with("Deck 2: cannot load gone — file is missing (")
                && text.contains("gone.wav"),
            "{text}"
        );
        drop(jobs);
        worker.join().unwrap();
        // Nothing was decoded for the engine.
        assert!(!events.try_iter().any(|e| matches!(e, Event::Loaded(_))));
    }

    #[test]
    fn loads_a_track_with_its_memory() {
        let s = setup(&["ok.wav"]);
        let id = ids(&s.lib)[0];
        let writer = s.lib.writer();
        writer
            .submit_wait(WriteOp::SetCue {
                track: id,
                frames: 1234.0,
            })
            .unwrap();
        writer
            .submit_wait(WriteOp::SetHotcue {
                track: id,
                slot: 3,
                frames: 900.0,
                loop_frames: None,
            })
            .unwrap();
        s.lib
            .edit_grid(id, library::GridEdit::SetBpm(120.0), None)
            .unwrap();
        let (tx, events) = channel();
        let (jobs, _worker) = spawn(Arc::clone(&s.lib), tx, None).unwrap();
        jobs.send(Job::LoadTrack {
            deck: deck(1),
            track: id,
            seq: 7,
            refuse_if_playing: false,
        })
        .unwrap();
        let loaded = wait_for(&events, |e| match e {
            Event::Loaded(l) => Some(l),
            _ => None,
        });
        assert_eq!((loaded.deck, loaded.seq, loaded.row.id), (deck(1), 7, id));
        assert_eq!(loaded.memory.cue, 1234.0);
        let mut hotcues = [None; HOTCUES];
        hotcues[2] = Some(Hotcue::point(900.0));
        assert_eq!(loaded.memory.hotcues, hotcues);
        let rate = loaded.buffer.sample_rate();
        assert_eq!(
            loaded.memory.grid,
            engine::Grid::from_bpm(120.0, 0.0, rate),
            "the grid at the decoded rate"
        );
        assert!(loaded.buffer.frames() > 0);
        assert!(loaded.hotcues_read);
    }

    #[test]
    fn a_loop_slot_loads_from_the_library() {
        let s = setup(&["ok.wav"]);
        let id = ids(&s.lib)[0];
        s.lib
            .writer()
            .submit_wait(WriteOp::PutHotcue {
                track: id,
                slot: 2,
                point: library::HotcuePoint {
                    frames: 900.0,
                    loop_frames: Some(300.0),
                    color: None,
                    name: None,
                },
            })
            .unwrap();
        let (tx, events) = channel();
        let (jobs, _worker) = spawn(Arc::clone(&s.lib), tx, None).unwrap();
        jobs.send(Job::LoadTrack {
            deck: deck(1),
            track: id,
            seq: 1,
            refuse_if_playing: false,
        })
        .unwrap();
        let loaded = wait_for(&events, |e| match e {
            Event::Loaded(l) => Some(l),
            _ => None,
        });
        assert_eq!(
            loaded.memory.hotcues[1],
            Some(Hotcue {
                frames: 900.0,
                loop_frames: Some(300.0)
            })
        );
    }

    #[test]
    fn unreadable_hotcues_are_not_remembered_for_the_load() {
        let s = setup(&["ok.wav"]);
        let id = ids(&s.lib)[0];
        s.lib
            .writer()
            .submit_wait(WriteOp::SetCue {
                track: id,
                frames: 1234.0,
            })
            .unwrap();
        // A hotcue row that is not a number: reading the hotcues fails.
        s.lib
            .writer()
            .submit_wait(WriteOp::Raw(vec![RawStatement {
                sql: format!("INSERT INTO hotcues(track_id, slot, frames) VALUES ({id}, 1, 'x')"),
                params: vec![],
            }]))
            .unwrap();
        let (tx, events) = channel();
        let (jobs, _worker) = spawn(Arc::clone(&s.lib), tx, None).unwrap();
        jobs.send(Job::LoadTrack {
            deck: deck(1),
            track: id,
            seq: 1,
            refuse_if_playing: false,
        })
        .unwrap();
        let text = wait_for(&events, message);
        assert!(
            text.starts_with(&format!(
                "warning: could not read the hotcues of track {id}: "
            )) && text.ends_with("; hotcues won't be remembered for this load"),
            "{text}"
        );
        let loaded = wait_for(&events, |e| match e {
            Event::Loaded(l) => Some(l),
            _ => None,
        });
        assert!(!loaded.hotcues_read);
        assert_eq!(loaded.memory.hotcues, [None; HOTCUES]);
        assert_eq!(loaded.memory.cue, 1234.0, "the rest still loads");
    }

    /// The library [`saving_decode`] writes to.
    static DURING_DECODE: std::sync::OnceLock<(Arc<Library>, i64)> = std::sync::OnceLock::new();

    /// Decodes, saving a hotcue for the track meanwhile, as the DJ might on another deck.
    fn saving_decode(path: &Path) -> Result<decode::Decoded, decode::DecodeError> {
        let (lib, track) = DURING_DECODE.get().unwrap();
        lib.writer()
            .submit_wait(WriteOp::SetHotcue {
                track: *track,
                slot: 1,
                frames: 77.0,
                loop_frames: None,
            })
            .unwrap();
        decode::decode_file(path)
    }

    #[test]
    fn the_memory_is_read_after_the_decode() {
        let s = setup(&["ok.wav"]);
        let id = ids(&s.lib)[0];
        DURING_DECODE.set((Arc::clone(&s.lib), id)).unwrap();
        let (tx, events) = channel();
        let (jobs, _worker) = spawn_with(Arc::clone(&s.lib), tx, saving_decode, Vec::new).unwrap();
        jobs.send(Job::LoadTrack {
            deck: deck(1),
            track: id,
            seq: 1,
            refuse_if_playing: false,
        })
        .unwrap();
        let loaded = wait_for(&events, |e| match e {
            Event::Loaded(l) => Some(l),
            _ => None,
        });
        assert_eq!(loaded.memory.hotcues[0], Some(Hotcue::point(77.0)));
    }

    fn panicking_decode(path: &Path) -> Result<decode::Decoded, decode::DecodeError> {
        if path.to_string_lossy().contains("boom") {
            panic!("injected decoder panic");
        }
        decode::decode_file(path)
    }

    #[test]
    fn a_decoder_panic_fails_only_that_load() {
        let s = setup(&["boom.wav", "fine.wav"]);
        let reader = s.lib.reader().unwrap();
        let id_of = |name: &str| {
            ids(&s.lib)
                .into_iter()
                .find(|&id| reader.track(id).unwrap().unwrap().path.ends_with(name))
                .unwrap()
        };
        let (boom, fine) = (id_of("boom.wav"), id_of("fine.wav"));
        let (tx, events) = channel();
        let (jobs, _worker) =
            spawn_with(Arc::clone(&s.lib), tx, panicking_decode, Vec::new).unwrap();
        jobs.send(Job::LoadTrack {
            deck: deck(1),
            track: boom,
            seq: 1,
            refuse_if_playing: false,
        })
        .unwrap();
        let text = wait_for(&events, message);
        assert_eq!(
            text,
            "Deck 1: cannot load boom: the decoder crashed on this file"
        );
        // The loader survived.
        jobs.send(Job::LoadTrack {
            deck: deck(1),
            track: fine,
            seq: 2,
            refuse_if_playing: false,
        })
        .unwrap();
        let loaded = wait_for(&events, |e| match e {
            Event::Loaded(l) => Some(l),
            _ => None,
        });
        assert_eq!(loaded.row.id, fine);
    }

    #[test]
    fn a_whole_list_selection_is_added_to_a_playlist_by_the_worker() {
        let s = setup(&["a.wav", "b.wav"]);
        let ids = ids(&s.lib);
        let writer = s.lib.writer();
        for (name, tracks) in [("Src", ids.clone()), ("Dst", Vec::new())] {
            writer
                .submit_wait(WriteOp::CreatePlaylistWith {
                    name: name.into(),
                    tracks,
                })
                .unwrap();
        }
        let (tx, events) = channel();
        let (jobs, _worker) =
            spawn_with(Arc::clone(&s.lib), tx, decode::decode_file, Vec::new).unwrap();
        let view = |events: &Receiver<Event>| {
            wait_for(events, |e| match e {
                Event::Browser(v) => Some(v),
                _ => None,
            })
        };
        view(&events);
        let reader = s.lib.reader().unwrap();
        let playlists = reader.playlists().unwrap();
        let id = |name: &str| playlists.iter().find(|p| p.name == name).unwrap().id;
        // The Playlists node, then Src and Dst.
        jobs.send(Job::Edit(BrowserEdit::SelectTree(1))).unwrap();
        assert_eq!(view(&events).path, "Playlists \u{25b8} Src");
        jobs.send(Job::Edit(BrowserEdit::SelectAll)).unwrap();
        assert!(view(&events).all_selected);
        jobs.send(Job::AddListToPlaylist {
            playlist: id("Dst"),
        })
        .unwrap();
        // The browser is read again once the write is done, and the selection with it.
        assert!(!view(&events).all_selected);
        let added = reader.playlist_tracks(id("Dst"), 0, 100).unwrap();
        assert_eq!(added.iter().map(|t| t.id).collect::<Vec<_>>(), ids);
    }

    #[test]
    fn a_failed_search_still_sends_the_view() {
        let s = setup(&["b.wav"]);
        let id = ids(&s.lib)[0];
        s.lib
            .writer()
            .submit_wait(WriteOp::CreatePlaylistWith {
                name: "Peak".into(),
                tracks: vec![id],
            })
            .unwrap();
        let (tx, events) = channel();
        let (jobs, _worker) =
            spawn_with(Arc::clone(&s.lib), tx, decode::decode_file, Vec::new).unwrap();
        let view = |events: &Receiver<Event>| {
            wait_for(events, |e| match e {
                Event::Browser(v) => Some(v),
                _ => None,
            })
        };
        view(&events);
        jobs.send(Job::Edit(BrowserEdit::SelectTree(1))).unwrap();
        assert_eq!(view(&events).path, "Playlists ▸ Peak");
        // The playlist can no longer be read.
        s.lib
            .writer()
            .submit_wait(WriteOp::Raw(vec![RawStatement {
                sql: "DROP TABLE playlist_tracks".into(),
                params: vec![],
            }]))
            .unwrap();
        let search = BrowserEdit::Search {
            text: Some("b".into()),
            seq: 1,
        };
        jobs.send(Job::Edit(search)).unwrap();
        // The window still gets a view of that query, with the list as it was.
        let failed = view(&events);
        assert_eq!((failed.seq, failed.search), (1, None));
        assert_eq!(failed.list_count, 1);
        // And later views carry it, so the window keeps showing them.
        jobs.send(Job::Browse(BrowserRequest::ToggleFocus)).unwrap();
        assert_eq!(view(&events).seq, 1);
    }

    #[test]
    fn refresh_keeps_the_selected_track_and_finds_new_ones() {
        let s = setup(&["b.wav", "c.wav"]);
        let reader = s.lib.reader().unwrap();
        let c = ids(&s.lib)
            .into_iter()
            .find(|&id| reader.track(id).unwrap().unwrap().path.ends_with("c.wav"))
            .unwrap();
        let (tx, events) = channel();
        let roots = vec![Drive {
            name: "music".into(),
            path: s.music.clone(),
        }];
        let (jobs, _worker) = spawn_with(Arc::clone(&s.lib), tx, decode::decode_file, move || {
            roots.clone()
        })
        .unwrap();
        let view = |events: &Receiver<Event>| {
            wait_for(events, |e| match e {
                Event::Browser(v) => Some(v),
                _ => None,
            })
        };
        // Tree: "music", "Playlists". Select the second file of the music folder.
        let initial = view(&events);
        assert_eq!((initial.path.as_str(), initial.list_count), ("music", 2));
        jobs.send(Job::Browse(BrowserRequest::ToggleFocus)).unwrap();
        jobs.send(Job::Browse(BrowserRequest::Scroll(1))).unwrap();
        let mut selected = None;
        for _ in 0..2 {
            selected = view(&events).track.map(|t| t.id);
        }
        assert_eq!(selected, Some(c));
        // A file that sorts first appears in the folder, and a track elsewhere in the
        // library (lowest id, no artist, like the others).
        std::fs::copy(FIXTURE, s.music.join("a.wav")).unwrap();
        s.lib
            .writer()
            .submit_wait(WriteOp::Raw(vec![RawStatement {
                sql: "INSERT INTO tracks(id, path, fingerprint, title, added_at) \
                      VALUES (0, '/elsewhere/new.wav', x'00', 'new', 0)"
                    .into(),
                params: vec![],
            }]))
            .unwrap();
        jobs.send(Job::Refresh {
            decks: Vec::new(),
            find_new_tracks: true,
        })
        .unwrap();
        let refreshed = view(&events);
        assert_eq!(refreshed.track.map(|t| t.id), Some(c));
        assert_eq!((refreshed.index, refreshed.count), (2, 3));
        let new = wait_for(&events, |e| match e {
            Event::NewTracks(ids) => Some(ids),
            _ => None,
        });
        assert!(new.iter().any(|t| t.id == 0), "{new:?}");
    }

    #[test]
    fn a_browsed_folder_is_added_once() {
        let dir = tempfile::tempdir().unwrap();
        let lib = Arc::new(Library::open(&dir.path().join("library.sqlite3")).unwrap());
        let music = dunce::canonicalize(dir.path()).unwrap().join("music");
        std::fs::create_dir_all(music.join("sub")).unwrap();
        std::fs::copy(FIXTURE, music.join("one.wav")).unwrap();
        std::fs::copy(
            FIXTURE.replace("sine440.wav", "sine440_mono.wav"),
            music.join("two.wav"),
        )
        .unwrap();
        std::fs::copy(
            FIXTURE.replace("sine440.wav", "sine440.flac"),
            music.join("sub/deep.flac"),
        )
        .unwrap();
        let roots = vec![Drive {
            name: "music".into(),
            path: music.clone(),
        }];
        let (tx, events) = channel();
        let (jobs, _worker) = spawn_with(Arc::clone(&lib), tx, decode::decode_file, move || {
            roots.clone()
        })
        .unwrap();
        // Highlighting the drive adds its two files, and only those.
        wait_for(&events, |e| matches!(e, Event::FolderAdded).then_some(()));
        assert_eq!(ids(&lib).len(), 2);
        // Highlighted again, it is not added again.
        jobs.send(Job::Refresh {
            decks: Vec::new(),
            find_new_tracks: false,
        })
        .unwrap();
        let view = wait_for(&events, |e| match e {
            Event::Browser(v) if v.list.iter().all(|r| r.track.is_some()) => Some(v),
            _ => None,
        });
        assert_eq!(view.list.len(), 2);
        assert!(
            events
                .recv_timeout(std::time::Duration::from_millis(300))
                .is_err_and(|e| e == std::sync::mpsc::RecvTimeoutError::Timeout)
        );
        assert!(lib.reader().unwrap().folders().unwrap().is_empty());
    }
}
