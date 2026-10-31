//! Watching library folders: file system events are debounced per path, then each path is
//! brought in line with the library by the same per-file logic as a folder scan.
//!
//! Removals only flag tracks missing. A rename arrives as a remove and a create, or as a
//! rename pair; either way the new path relinks the track (same id, cues kept).
//!
//! When the watch itself fails (an error from the OS watcher, or a root that cannot be
//! watched, say too many watches on Linux), the watcher is [`WatchHealth::Degraded`] and
//! rescans the affected roots every [`RESCAN_INTERVAL`] instead. Overflow events ("some
//! events were lost, rescan") queue a scan of the affected folder.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use notify::event::{EventKind, ModifyKind};
use notify::{RecommendedWatcher, RecursiveMode, Watcher as _};

use crate::scan::{normalize_path, scan_folder_with, scan_path};
use crate::{Library, LibraryError, ScanReport, Writer};

/// How long a path must stay quiet before it is scanned.
pub const DEBOUNCE: Duration = Duration::from_secs(2);

/// How often roots whose watch failed are rescanned.
pub const RESCAN_INTERVAL: Duration = Duration::from_secs(600);

/// How many recent per-path problems a [`Watcher`] remembers.
const RECENT_SKIPS: usize = 100;

/// Whether the watcher is receiving file system events.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WatchHealth {
    Watching,
    /// The watch failed for at least one root; those roots are rescanned periodically.
    Degraded {
        reason: String,
    },
}

#[derive(Debug, Default)]
struct Skips {
    recent: VecDeque<(PathBuf, String)>,
    total: u64,
}

/// A running folder watcher. Dropping it stops watching and ends its thread (a folder
/// scan in progress stops between files); changes still waiting out their debounce are
/// not applied.
#[derive(Debug)]
pub struct Watcher {
    inner: Option<RecommendedWatcher>,
    thread: Option<JoinHandle<()>>,
    errors: Receiver<LibraryError>,
    scans: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
    health: Arc<Mutex<WatchHealth>>,
    skips: Arc<Mutex<Skips>>,
    #[cfg(test)]
    events: Option<Sender<notify::Result<notify::Event>>>,
}

impl Watcher {
    /// Problems with the watch itself: errors from the OS watcher (say, too many watches
    /// on Linux, or a watched folder removed) and roots that could not be watched. Each
    /// also makes [`Watcher::health`] degraded, after which the watcher rescans the
    /// affected roots periodically. Problems with single files are not reported here,
    /// see [`Watcher::skipped`].
    #[must_use]
    pub fn errors(&self) -> &Receiver<LibraryError> {
        &self.errors
    }

    /// Whether events are being received, or the watcher has fallen back to rescans.
    #[must_use]
    pub fn health(&self) -> WatchHealth {
        lock(&self.health).clone()
    }

    /// The most recent (up to 100) paths that could not be scanned, with the reason,
    /// oldest first.
    #[must_use]
    pub fn skipped(&self) -> Vec<(PathBuf, String)> {
        lock(&self.skips).recent.iter().cloned().collect()
    }

    /// How many paths could not be scanned since the watcher started.
    #[must_use]
    pub fn skipped_count(&self) -> u64 {
        lock(&self.skips).total
    }

    /// How many debounced paths have been scanned so far.
    #[doc(hidden)]
    #[must_use]
    pub fn scan_count(&self) -> u64 {
        self.scans.load(Ordering::Relaxed)
    }

    /// Delivers `event` as if the OS watcher had.
    #[cfg(test)]
    fn inject(&self, event: notify::Result<notify::Event>) {
        if let Some(events) = &self.events {
            events.send(event).unwrap();
        }
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // Dropping notify's watcher drops the event sender, which ends the thread.
        self.inner = None;
        #[cfg(test)]
        {
            self.events = None;
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Watches `roots` (recursively) and keeps `lib` in line with what changes in them,
/// debouncing events by [`DEBOUNCE`] per path. It does not scan on start. A root that
/// cannot be watched does not fail the call: the watcher starts degraded and rescans
/// that root every [`RESCAN_INTERVAL`].
///
/// # Errors
///
/// Fails if a root cannot be resolved, the OS watcher cannot be created, or the library
/// cannot be read.
pub fn watch(lib: &Library, roots: &[PathBuf]) -> Result<Watcher, LibraryError> {
    start(lib, roots, DEBOUNCE, RESCAN_INTERVAL)
}

/// [`watch`] with a custom debounce.
///
/// # Errors
///
/// As [`watch`].
#[doc(hidden)]
pub fn watch_with_debounce(
    lib: &Library,
    roots: &[PathBuf],
    debounce: Duration,
) -> Result<Watcher, LibraryError> {
    start(lib, roots, debounce, RESCAN_INTERVAL)
}

fn start(
    lib: &Library,
    roots: &[PathBuf],
    debounce: Duration,
    rescan_every: Duration,
) -> Result<Watcher, LibraryError> {
    let roots = roots
        .iter()
        .map(|r| normalize_path(r))
        .collect::<Result<Vec<_>, _>>()?;
    let reader = Library::reader_at(lib.path())?;
    let (event_tx, event_rx) = mpsc::channel();
    #[cfg(test)]
    let events = Some(event_tx.clone());
    let mut inner = notify::recommended_watcher(move |res| {
        let _ = event_tx.send(res);
    })?;
    let (error_tx, errors) = mpsc::channel();
    let health = Arc::new(Mutex::new(WatchHealth::Watching));
    let mut degraded = Vec::new();
    for root in &roots {
        if let Err(err) = inner.watch(root, RecursiveMode::Recursive) {
            *lock(&health) = WatchHealth::Degraded {
                reason: err.to_string(),
            };
            let _ = error_tx.send(err.into());
            degraded.push(root.clone());
        }
    }
    let scans = Arc::new(AtomicU64::new(0));
    let stop = Arc::new(AtomicBool::new(false));
    let skips = Arc::new(Mutex::new(Skips::default()));
    let worker = Worker {
        writer: lib.writer(),
        reader,
        roots,
        debounce,
        rescan_every,
        errors: error_tx,
        scans: Arc::clone(&scans),
        stop: Arc::clone(&stop),
        health: Arc::clone(&health),
        skips: Arc::clone(&skips),
    };
    let thread = thread::Builder::new()
        .name("library-watch".into())
        .spawn(move || worker.run(&event_rx, degraded))?;
    Ok(Watcher {
        inner: Some(inner),
        thread: Some(thread),
        errors,
        scans,
        stop,
        health,
        skips,
        #[cfg(test)]
        events,
    })
}

/// A path waiting out its debounce.
struct Pending {
    due: Instant,
    /// Arrival order of the path's first event, to keep removes and creates in sequence.
    first: u64,
    /// Whether a folder at this path should be scanned (it was created, renamed, or an
    /// event overflow asked for a rescan).
    dirs: bool,
}

struct Worker {
    writer: Writer,
    reader: crate::Reader,
    roots: Vec<PathBuf>,
    debounce: Duration,
    rescan_every: Duration,
    errors: Sender<LibraryError>,
    scans: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
    health: Arc<Mutex<WatchHealth>>,
    skips: Arc<Mutex<Skips>>,
}

type Events = Receiver<notify::Result<notify::Event>>;

impl Worker {
    fn run(self, events: &Events, mut degraded: Vec<PathBuf>) {
        let mut pending: HashMap<PathBuf, Pending> = HashMap::new();
        let mut seq = 0_u64;
        let mut rescan_due = (!degraded.is_empty()).then(|| Instant::now() + self.rescan_every);
        while !self.stop.load(Ordering::Relaxed) {
            let next = pending.values().map(|p| p.due).chain(rescan_due).min();
            let received = match next {
                Some(due) => events.recv_timeout(due.saturating_duration_since(Instant::now())),
                None => events.recv().map_err(|_| RecvTimeoutError::Disconnected),
            };
            match received {
                Ok(Ok(event)) => self.queue_event(event, &mut pending, &mut seq),
                Ok(Err(err)) => {
                    self.degrade(&err, &mut degraded);
                    rescan_due.get_or_insert_with(|| Instant::now() + self.rescan_every);
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
                if self.stop.load(Ordering::Relaxed) || !self.process(&path, dirs) {
                    return;
                }
            }
            if rescan_due.is_some_and(|at| at <= now) {
                for root in &degraded {
                    if self.stop.load(Ordering::Relaxed) || !self.rescan(root) {
                        return;
                    }
                }
                rescan_due = Some(Instant::now() + self.rescan_every);
            }
        }
    }

    fn queue_event(
        &self,
        event: notify::Event,
        pending: &mut HashMap<PathBuf, Pending>,
        seq: &mut u64,
    ) {
        if event.need_rescan() {
            // Events were lost: scan the folder concerned, or every root.
            let paths = if event.paths.is_empty() {
                self.roots.clone()
            } else {
                event.paths
            };
            for path in paths {
                self.queue(pending, seq, path, true);
            }
            return;
        }
        if matches!(event.kind, EventKind::Access(_)) {
            return;
        }
        let dirs = matches!(
            event.kind,
            EventKind::Create(_) | EventKind::Modify(ModifyKind::Name(_))
        );
        for path in event.paths {
            self.queue(pending, seq, path, dirs);
        }
    }

    fn queue(
        &self,
        pending: &mut HashMap<PathBuf, Pending>,
        seq: &mut u64,
        path: PathBuf,
        dirs: bool,
    ) {
        *seq += 1;
        let due = Instant::now() + self.debounce;
        let entry = pending.entry(path).or_insert(Pending {
            due,
            first: *seq,
            dirs: false,
        });
        entry.due = due;
        entry.dirs |= dirs;
    }

    /// Records a watch failure: the roots it concerns (all, if it names no path) are
    /// rescanned from now on.
    fn degrade(&self, err: &notify::Error, degraded: &mut Vec<PathBuf>) {
        *lock(&self.health) = WatchHealth::Degraded {
            reason: err.to_string(),
        };
        let mut affected: Vec<PathBuf> = err
            .paths
            .iter()
            .filter_map(|p| self.root_of(p).map(Path::to_owned))
            .collect();
        if affected.is_empty() {
            affected = self.roots.clone();
        }
        for root in affected {
            if !degraded.contains(&root) {
                degraded.push(root);
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
        let result = scan_path(
            &self.writer,
            self.reader.connection(),
            root,
            path,
            dirs,
            Some(&self.stop),
        );
        self.finish(path, result)
    }

    /// Scans a whole root. False when the library has shut down.
    fn rescan(&self, root: &Path) -> bool {
        let result = scan_folder_with(
            &self.writer,
            self.reader.connection(),
            root,
            None,
            Some(&self.stop),
            &mut |_| {},
        );
        self.finish(root, result)
    }

    /// Files a scan's problems under skipped. False when the library has shut down.
    fn finish(&self, path: &Path, result: Result<ScanReport, LibraryError>) -> bool {
        let skipped = match result {
            Ok(report) => report.skipped,
            Err(LibraryError::WriterGone) => return false,
            Err(err) => vec![(path.to_owned(), err.to_string())],
        };
        if !skipped.is_empty() {
            let mut skips = lock(&self.skips);
            for skip in skipped {
                skips.total += 1;
                if skips.recent.len() == RECENT_SKIPS {
                    skips.recent.pop_front();
                }
                skips.recent.push_back(skip);
            }
        }
        true
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    const CAP: Duration = Duration::from_secs(10);

    fn fixture(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../decode/tests/fixtures")
            .join(name)
    }

    struct Setup {
        _dir: tempfile::TempDir,
        lib: Library,
        music: PathBuf,
    }

    /// A library, and a music folder holding a track the library does not know yet (the
    /// OS watcher, started later, never reports it).
    fn setup() -> Setup {
        let dir = tempfile::tempdir().unwrap();
        let lib = Library::open(&dir.path().join("db").join("library.sqlite3")).unwrap();
        let music = normalize_path(&{
            let m = dir.path().join("music");
            fs::create_dir_all(&m).unwrap();
            m
        })
        .unwrap();
        fs::copy(fixture("sine440.wav"), music.join("a.wav")).unwrap();
        Setup {
            _dir: dir,
            lib,
            music,
        }
    }

    fn track_count(lib: &Library) -> i64 {
        lib.reader()
            .unwrap()
            .connection()
            .query_row("SELECT COUNT(*) FROM tracks", [], |r| r.get(0))
            .unwrap()
    }

    fn wait_for(what: &str, mut done: impl FnMut() -> bool) {
        let start = Instant::now();
        while !done() {
            assert!(start.elapsed() < CAP, "timed out waiting for {what}");
            thread::sleep(Duration::from_millis(25));
        }
    }

    #[test]
    fn rescan_event_without_paths_scans_every_root() {
        let s = setup();
        let w = start(
            &s.lib,
            std::slice::from_ref(&s.music),
            Duration::from_millis(50),
            RESCAN_INTERVAL,
        )
        .unwrap();
        let rescan = notify::Event::new(EventKind::Other).set_flag(notify::event::Flag::Rescan);
        w.inject(Ok(rescan));
        wait_for("the rescan", || track_count(&s.lib) == 1);
        assert_eq!(w.health(), WatchHealth::Watching);
    }

    #[test]
    fn rescan_event_with_a_folder_scans_it() {
        let s = setup();
        let w = start(
            &s.lib,
            std::slice::from_ref(&s.music),
            Duration::from_millis(50),
            RESCAN_INTERVAL,
        )
        .unwrap();
        let rescan = notify::Event::new(EventKind::Other)
            .add_path(s.music.clone())
            .set_flag(notify::event::Flag::Rescan);
        w.inject(Ok(rescan));
        wait_for("the rescan", || track_count(&s.lib) == 1);
    }

    #[test]
    fn watch_error_degrades_and_rescans_periodically() {
        let s = setup();
        let w = start(
            &s.lib,
            std::slice::from_ref(&s.music),
            Duration::from_millis(50),
            Duration::from_millis(200),
        )
        .unwrap();
        assert_eq!(w.health(), WatchHealth::Watching);
        w.inject(Err(notify::Error::generic("too many watches")));
        wait_for("degraded health", || {
            matches!(w.health(), WatchHealth::Degraded { .. })
        });
        wait_for("the periodic rescan", || track_count(&s.lib) == 1);
        assert!(w.errors().try_recv().is_ok());
    }

    #[test]
    fn unreadable_file_is_skipped_not_a_watch_error() {
        let s = setup();
        fs::remove_file(s.music.join("a.wav")).unwrap();
        fs::copy(fixture("corrupt.mp3"), s.music.join("bad.mp3")).unwrap();
        let w = start(
            &s.lib,
            std::slice::from_ref(&s.music),
            Duration::from_millis(50),
            RESCAN_INTERVAL,
        )
        .unwrap();
        w.inject(Ok(
            notify::Event::new(EventKind::Other).set_flag(notify::event::Flag::Rescan)
        ));
        wait_for("the skip", || w.skipped_count() == 1);
        assert_eq!(w.skipped()[0].0, s.music.join("bad.mp3"));
        assert!(w.errors().try_recv().is_err());
        assert_eq!(w.health(), WatchHealth::Watching);
    }
}
