//! The library worker thread: everything library mode does with the database (browser
//! moves, track lookups, grid edits), and a loader thread that decodes tracks, so the
//! control loop never waits on either.

use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread::JoinHandle;

use engine::{DeckId, HOTCUES, TrackBuffer, TrackMemory};
use library::{
    Beatgrid, Browser, BrowserRequest, BrowserView, GridEdit, Library, PendingTrack, Reader,
    TrackRow,
};

use super::memory_sync::engine_grid;
use crate::format::safe;

/// Work for the library worker.
#[derive(Debug)]
pub enum Job {
    Browse(BrowserRequest),
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
    },
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
}

/// A decoded track, ready for the deck.
#[derive(Debug)]
pub struct Loaded {
    pub deck: DeckId,
    pub seq: u64,
    pub row: TrackRow,
    pub buffer: Arc<TrackBuffer>,
    /// The remembered cue point, hotcues and beatgrid (at the decoded rate).
    pub memory: TrackMemory,
}

struct LoadJob {
    deck: DeckId,
    seq: u64,
    row: TrackRow,
    path: PathBuf,
    cue: Option<f64>,
    hotcues: [Option<f64>; HOTCUES],
    grid: Option<Beatgrid>,
}

/// Starts the worker (and its loader). It stops when the returned sender is dropped.
pub fn spawn(
    lib: Arc<Library>,
    events: Sender<Event>,
) -> std::io::Result<(Sender<Job>, JoinHandle<()>)> {
    spawn_with(lib, events, decode::decode_file)
}

/// [`spawn`] with the loader decoding through `decode`.
fn spawn_with(
    lib: Arc<Library>,
    events: Sender<Event>,
    decode: DecodeFn,
) -> std::io::Result<(Sender<Job>, JoinHandle<()>)> {
    let (tx, rx) = channel();
    let handle = std::thread::Builder::new()
        .name("library-worker".into())
        .spawn(move || run(&lib, &rx, &events, decode))?;
    Ok((tx, handle))
}

fn deck_number(deck: DeckId) -> usize {
    deck.index() + 1
}

/// How the loader decodes a file (the real decoder, or a stand-in in tests).
type DecodeFn = fn(&Path) -> Result<decode::Decoded, decode::DecodeError>;

fn run(lib: &Library, jobs: &Receiver<Job>, events: &Sender<Event>, decode: DecodeFn) {
    let say = |text: String| {
        let _ = events.send(Event::Message(text));
    };
    let (reader, mut browser) = match (lib.reader(), lib.reader().and_then(Browser::new)) {
        (Ok(reader), Ok(browser)) => (reader, browser),
        (Err(e), _) | (_, Err(e)) => {
            say(format!("error: cannot read the library: {}", safe(e)));
            return;
        }
    };
    let _ = events.send(Event::Browser(browser.view()));
    let (load_tx, load_rx) = channel::<LoadJob>();
    let loader_events = events.clone();
    let loader = std::thread::Builder::new()
        .name("track-loader".into())
        .spawn(move || load_tracks(&load_rx, &loader_events, decode));
    if let Err(e) = &loader {
        say(format!("error: cannot start the track loader: {e}"));
    }

    for job in jobs {
        match job {
            Job::Browse(req) => match browser.apply(req) {
                Ok(view) => {
                    let _ = events.send(Event::Browser(view));
                }
                Err(e) => say(format!("warning: library browser: {}", safe(e))),
            },
            Job::Refresh {
                decks,
                find_new_tracks,
            } => {
                match browser.refresh() {
                    Ok(view) => {
                        let _ = events.send(Event::Browser(view));
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
                Some(row) => request_load(&reader, &load_tx, deck, row.id, seq, &say),
                None => say(format!(
                    "Deck {}: nothing selected in the browser",
                    deck_number(deck)
                )),
            },
            Job::LoadTrack { deck, track, seq } => {
                request_load(&reader, &load_tx, deck, track, seq, &say);
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
    if let Ok(loader) = loader {
        let _ = loader.join();
    }
}

/// Looks the track up and hands it to the loader, unless its file is missing.
fn request_load(
    reader: &Reader,
    loader: &Sender<LoadJob>,
    deck: DeckId,
    track: i64,
    seq: u64,
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
    // A track whose memory cannot be read still loads, without it.
    let job = LoadJob {
        deck,
        seq,
        row: details.row,
        path: details.path,
        cue: read_or_warn(reader.cue(track), "cue point", track, say).flatten(),
        hotcues: read_or_warn(reader.hotcues(track), "hotcues", track, say).unwrap_or_default(),
        grid: read_or_warn(reader.beatgrid(track), "beatgrid", track, say).flatten(),
    };
    if loader.send(job).is_err() {
        say(format!(
            "Deck {n}: cannot load track {track}: the loader stopped"
        ));
    }
}

/// The value read, or `None` after saying why it could not be read.
fn read_or_warn<T>(
    result: Result<T, library::LibraryError>,
    what: &str,
    track: i64,
    say: &dyn Fn(String),
) -> Option<T> {
    result
        .inspect_err(|e| {
            say(format!(
                "warning: could not read the {what} of track {track}: {}",
                safe(e)
            ));
        })
        .ok()
}

/// Decodes queued tracks; of several queued for one deck only the latest is decoded.
fn load_tracks(jobs: &Receiver<LoadJob>, events: &Sender<Event>, decode: DecodeFn) {
    while let Ok(first) = jobs.recv() {
        let mut latest: HashMap<usize, LoadJob> = HashMap::new();
        for job in std::iter::once(first).chain(jobs.try_iter()) {
            latest.insert(job.deck.index(), job);
        }
        let mut batch: Vec<LoadJob> = latest.into_values().collect();
        batch.sort_by_key(|j| j.deck.index());
        for job in batch {
            load_one(job, events, decode);
        }
    }
}

/// Decodes one track. A decoder panic (say, on a malformed file) fails this load only:
/// it is reported and the loader goes on.
fn load_one(job: LoadJob, events: &Sender<Event>, decode: DecodeFn) {
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
            // Grid periods at the decoded rate, which is what the deck plays.
            let memory = TrackMemory {
                cue: job.cue.unwrap_or(0.0),
                hotcues: job.hotcues,
                grid: engine_grid(job.grid.as_ref(), decoded.track.sample_rate()),
            };
            let _ = events.send(Event::Loaded(Box::new(Loaded {
                deck: job.deck,
                seq: job.seq,
                row: job.row,
                buffer: Arc::new(decoded.track),
                memory,
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

    /// Track ids in browser order ("All tracks": artist, then id).
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
        let (jobs, worker) = spawn(Arc::clone(&s.lib), tx).unwrap();
        jobs.send(Job::LoadTrack {
            deck: deck(2),
            track: id,
            seq: 1,
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
            })
            .unwrap();
        s.lib
            .edit_grid(id, library::GridEdit::SetBpm(120.0), None)
            .unwrap();
        let (tx, events) = channel();
        let (jobs, _worker) = spawn(Arc::clone(&s.lib), tx).unwrap();
        jobs.send(Job::LoadTrack {
            deck: deck(1),
            track: id,
            seq: 7,
        })
        .unwrap();
        let loaded = wait_for(&events, |e| match e {
            Event::Loaded(l) => Some(l),
            _ => None,
        });
        assert_eq!((loaded.deck, loaded.seq, loaded.row.id), (deck(1), 7, id));
        assert_eq!(loaded.memory.cue, 1234.0);
        let mut hotcues = [None; HOTCUES];
        hotcues[2] = Some(900.0);
        assert_eq!(loaded.memory.hotcues, hotcues);
        let rate = loaded.buffer.sample_rate();
        assert_eq!(
            loaded.memory.grid,
            engine::Grid::from_bpm(120.0, 0.0, rate),
            "the grid at the decoded rate"
        );
        assert!(loaded.buffer.frames() > 0);
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
        let (jobs, _worker) = spawn_with(Arc::clone(&s.lib), tx, panicking_decode).unwrap();
        jobs.send(Job::LoadTrack {
            deck: deck(1),
            track: boom,
            seq: 1,
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
        })
        .unwrap();
        let loaded = wait_for(&events, |e| match e {
            Event::Loaded(l) => Some(l),
            _ => None,
        });
        assert_eq!(loaded.row.id, fine);
    }

    #[test]
    fn refresh_keeps_the_selected_track_and_finds_new_ones() {
        let s = setup(&["b.wav", "c.wav"]);
        let before = ids(&s.lib);
        let (tx, events) = channel();
        let (jobs, _worker) = spawn(Arc::clone(&s.lib), tx).unwrap();
        let view = |events: &Receiver<Event>| {
            wait_for(events, |e| match e {
                Event::Browser(v) => Some(v),
                _ => None,
            })
        };
        let _initial = view(&events);
        // Tree: "Folders ▸ music", "All tracks". Select the second track of All tracks.
        jobs.send(Job::Browse(BrowserRequest::Scroll(1))).unwrap();
        jobs.send(Job::Browse(BrowserRequest::ToggleFocus)).unwrap();
        jobs.send(Job::Browse(BrowserRequest::Scroll(1))).unwrap();
        let mut selected = None;
        for _ in 0..3 {
            selected = view(&events).track.map(|t| t.id);
        }
        assert_eq!(selected, Some(before[1]));
        // A track that sorts first appears (lowest id, no artist, like the others).
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
        assert_eq!(refreshed.track.map(|t| t.id), Some(before[1]));
        assert_eq!((refreshed.index, refreshed.count), (2, 3));
        let new = wait_for(&events, |e| match e {
            Event::NewTracks(ids) => Some(ids),
            _ => None,
        });
        assert!(new.iter().any(|t| t.id == 0), "{new:?}");
    }
}
