//! Watching library folders: file system events are debounced per path, then each path is
//! brought in line with the library by the same per-file logic as a folder scan.
//!
//! Removals only flag tracks missing. A rename arrives as a remove and a create, or as a
//! rename pair; either way the new path relinks the track (same id, cues kept).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use notify::event::{EventKind, ModifyKind};
use notify::{RecommendedWatcher, RecursiveMode, Watcher as _};

use crate::scan::{normalize_path, scan_path};
use crate::{Library, LibraryError, Writer};

/// How long a path must stay quiet before it is scanned.
pub const DEBOUNCE: Duration = Duration::from_secs(2);

/// A running folder watcher. Dropping it stops watching and ends its thread; changes
/// still waiting out their debounce are not applied.
#[derive(Debug)]
pub struct Watcher {
    inner: Option<RecommendedWatcher>,
    thread: Option<JoinHandle<()>>,
    errors: Receiver<LibraryError>,
    scans: Arc<AtomicU64>,
}

impl Watcher {
    /// Problems the watcher ran into: notify errors (say, too many watches on Linux, or
    /// a watched folder removed) and library errors while applying a change. The app
    /// should fall back to rescanning every 10 minutes when one shows up.
    #[must_use]
    pub fn errors(&self) -> &Receiver<LibraryError> {
        &self.errors
    }

    /// How many debounced paths have been scanned so far.
    #[must_use]
    pub fn scan_count(&self) -> u64 {
        self.scans.load(Ordering::Relaxed)
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        // Dropping notify's watcher drops the event sender, which ends the thread.
        self.inner = None;
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Watches `roots` (recursively) and keeps `lib` in line with what changes in them,
/// debouncing events by [`DEBOUNCE`] per path. It does not scan on start.
///
/// # Errors
///
/// Fails if a root cannot be resolved or watched, or the library cannot be read.
pub fn watch(lib: &Library, roots: &[PathBuf]) -> Result<Watcher, LibraryError> {
    watch_with_debounce(lib, roots, DEBOUNCE)
}

/// [`watch`] with a custom debounce.
///
/// # Errors
///
/// As [`watch`].
pub fn watch_with_debounce(
    lib: &Library,
    roots: &[PathBuf],
    debounce: Duration,
) -> Result<Watcher, LibraryError> {
    let roots = roots
        .iter()
        .map(|r| normalize_path(r))
        .collect::<Result<Vec<_>, _>>()?;
    let reader = Library::reader_at(lib.path())?;
    let (event_tx, event_rx) = mpsc::channel();
    let mut inner = notify::recommended_watcher(move |res| {
        let _ = event_tx.send(res);
    })?;
    for root in &roots {
        inner.watch(root, RecursiveMode::Recursive)?;
    }
    let (error_tx, errors) = mpsc::channel();
    let scans = Arc::new(AtomicU64::new(0));
    let worker = Worker {
        writer: lib.writer(),
        reader,
        roots,
        debounce,
        errors: error_tx,
        scans: Arc::clone(&scans),
    };
    let thread = thread::Builder::new()
        .name("library-watch".into())
        .spawn(move || worker.run(&event_rx))?;
    Ok(Watcher {
        inner: Some(inner),
        thread: Some(thread),
        errors,
        scans,
    })
}

/// A path waiting out its debounce.
struct Pending {
    due: Instant,
    /// Arrival order of the path's first event, to keep removes and creates in sequence.
    first: u64,
    /// Whether a folder at this path should be scanned (it was created or renamed).
    dirs: bool,
}

struct Worker {
    writer: Writer,
    reader: crate::Reader,
    roots: Vec<PathBuf>,
    debounce: Duration,
    errors: Sender<LibraryError>,
    scans: Arc<AtomicU64>,
}

impl Worker {
    fn run(self, events: &Receiver<notify::Result<notify::Event>>) {
        let mut pending: HashMap<PathBuf, Pending> = HashMap::new();
        let mut seq = 0_u64;
        loop {
            let next = pending.values().map(|p| p.due).min();
            let received = match next {
                Some(due) => events.recv_timeout(due.saturating_duration_since(Instant::now())),
                None => events.recv().map_err(|_| RecvTimeoutError::Disconnected),
            };
            match received {
                Ok(Ok(event)) => {
                    if matches!(event.kind, EventKind::Access(_)) {
                        continue;
                    }
                    let dirs = matches!(
                        event.kind,
                        EventKind::Create(_) | EventKind::Modify(ModifyKind::Name(_))
                    );
                    for path in event.paths {
                        seq += 1;
                        let due = Instant::now() + self.debounce;
                        let entry = pending.entry(path).or_insert(Pending {
                            due,
                            first: seq,
                            dirs: false,
                        });
                        entry.due = due;
                        entry.dirs |= dirs;
                    }
                }
                Ok(Err(err)) => {
                    let _ = self.errors.send(err.into());
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
            let now = Instant::now();
            let mut due: Vec<(u64, PathBuf, bool)> = pending
                .iter()
                .filter(|(_, p)| p.due <= now)
                .map(|(path, p)| (p.first, path.clone(), p.dirs))
                .collect();
            due.sort_by_key(|(first, _, _)| *first);
            for (_, path, dirs) in due {
                pending.remove(&path);
                if !self.process(&path, dirs) {
                    return;
                }
            }
        }
    }

    /// Scans one debounced path. False when the library has shut down.
    fn process(&self, path: &Path, dirs: bool) -> bool {
        let Some(root) = self.root_of(path) else {
            return true;
        };
        if !dirs && path.is_dir() {
            // A folder's own modification events (its listing changed) need no scan: the
            // files inside report for themselves.
            return true;
        }
        self.scans.fetch_add(1, Ordering::Relaxed);
        match scan_path(&self.writer, self.reader.connection(), root, path, dirs) {
            Ok(_) => true,
            Err(LibraryError::WriterGone) => false,
            Err(err) => {
                let _ = self.errors.send(err);
                true
            }
        }
    }

    /// The watched root holding `path`, the deepest if roots nest.
    fn root_of(&self, path: &Path) -> Option<&Path> {
        let path = crate::scan::resolve_path(path).ok()?;
        self.roots
            .iter()
            .filter(|r| path.starts_with(r))
            .max_by_key(|r| r.as_os_str().len())
            .map(PathBuf::as_path)
    }
}
