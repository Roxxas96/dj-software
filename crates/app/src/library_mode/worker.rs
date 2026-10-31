//! The library worker thread: everything library mode does with the database (browser
//! moves, track lookups, grid edits), and a loader thread that decodes tracks, so the
//! control loop never waits on either.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread::JoinHandle;

use engine::{DeckId, TrackBuffer};
use library::{
    Beatgrid, Browser, BrowserRequest, BrowserView, GridEdit, Library, Reader, TrackRow,
};

/// Work for the library worker.
#[derive(Debug)]
pub enum Job {
    Browse(BrowserRequest),
    /// Reread the browser, and the rows of the tracks on the decks.
    Refresh {
        decks: Vec<(DeckId, i64)>,
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
    Track {
        deck: DeckId,
        row: TrackRow,
    },
}

/// A decoded track, ready for the deck.
#[derive(Debug)]
pub struct Loaded {
    pub deck: DeckId,
    pub seq: u64,
    pub row: TrackRow,
    pub buffer: Arc<TrackBuffer>,
    /// The remembered cue point.
    pub cue: Option<f64>,
}

struct LoadJob {
    deck: DeckId,
    seq: u64,
    row: TrackRow,
    path: PathBuf,
    cue: Option<f64>,
}

/// Starts the worker (and its loader). It stops when the returned sender is dropped.
pub fn spawn(
    lib: Arc<Library>,
    events: Sender<Event>,
) -> std::io::Result<(Sender<Job>, JoinHandle<()>)> {
    let (tx, rx) = channel();
    let handle = std::thread::Builder::new()
        .name("library-worker".into())
        .spawn(move || run(&lib, &rx, &events))?;
    Ok((tx, handle))
}

fn deck_number(deck: DeckId) -> usize {
    deck.index() + 1
}

fn run(lib: &Library, jobs: &Receiver<Job>, events: &Sender<Event>) {
    let say = |text: String| {
        let _ = events.send(Event::Message(text));
    };
    let (reader, mut browser) = match (lib.reader(), lib.reader().and_then(Browser::new)) {
        (Ok(reader), Ok(browser)) => (reader, browser),
        (Err(e), _) | (_, Err(e)) => {
            say(format!("error: cannot read the library: {e}"));
            return;
        }
    };
    let _ = events.send(Event::Browser(browser.view()));
    let (load_tx, load_rx) = channel::<LoadJob>();
    let loader_events = events.clone();
    let loader = std::thread::Builder::new()
        .name("track-loader".into())
        .spawn(move || load_tracks(&load_rx, &loader_events));
    if let Err(e) = &loader {
        say(format!("error: cannot start the track loader: {e}"));
    }

    for job in jobs {
        match job {
            Job::Browse(req) => match browser.apply(req) {
                Ok(view) => {
                    let _ = events.send(Event::Browser(view));
                }
                Err(e) => say(format!("warning: library browser: {e}")),
            },
            Job::Refresh { decks } => {
                match browser.refresh() {
                    Ok(view) => {
                        let _ = events.send(Event::Browser(view));
                    }
                    Err(e) => say(format!("warning: library browser: {e}")),
                }
                for (deck, id) in decks {
                    if let Ok(Some(details)) = reader.track(id) {
                        let _ = events.send(Event::Track {
                            deck,
                            row: details.row,
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
            } => match lib.edit_grid(track, edit, playhead) {
                Ok(grid) => {
                    let _ = events.send(Event::Grid { deck, track, grid });
                }
                Err(e) => say(format!(
                    "Deck {}: beatgrid not changed: {e}",
                    deck_number(deck)
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
        Err(e) => return say(format!("Deck {n}: cannot load track {track}: {e}")),
    };
    if details.row.missing || !details.path.exists() {
        let title = details.row.title.as_deref().unwrap_or("(untitled)");
        return say(format!(
            "Deck {n}: cannot load {title} — file is missing ({})",
            details.path.display()
        ));
    }
    let cue = match reader.cue(track) {
        Ok(cue) => cue,
        Err(e) => {
            say(format!(
                "warning: could not read the cue point of track {track}: {e}"
            ));
            None
        }
    };
    let job = LoadJob {
        deck,
        seq,
        row: details.row,
        path: details.path,
        cue,
    };
    if loader.send(job).is_err() {
        say(format!(
            "Deck {n}: cannot load track {track}: the loader stopped"
        ));
    }
}

/// Decodes queued tracks; of several queued for one deck only the latest is decoded.
fn load_tracks(jobs: &Receiver<LoadJob>, events: &Sender<Event>) {
    while let Ok(first) = jobs.recv() {
        let mut latest: HashMap<usize, LoadJob> = HashMap::new();
        for job in std::iter::once(first).chain(jobs.try_iter()) {
            latest.insert(job.deck.index(), job);
        }
        let mut batch: Vec<LoadJob> = latest.into_values().collect();
        batch.sort_by_key(|j| j.deck.index());
        for job in batch {
            load_one(job, events);
        }
    }
}

fn load_one(job: LoadJob, events: &Sender<Event>) {
    let n = deck_number(job.deck);
    match decode::decode_file(&job.path) {
        Ok(decoded) => {
            if decoded.truncated {
                let _ = events.send(Event::Message(format!(
                    "warning: {} may be incomplete (decoding stopped early)",
                    job.path.display()
                )));
            }
            let _ = events.send(Event::Loaded(Box::new(Loaded {
                deck: job.deck,
                seq: job.seq,
                row: job.row,
                buffer: Arc::new(decoded.track),
                cue: job.cue,
            })));
        }
        Err(e) => {
            let _ = events.send(Event::Message(format!(
                "Deck {n}: cannot load {}: {e}",
                crate::format::track_name(&job.row)
            )));
        }
    }
}
