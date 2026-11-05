//! Background analysis: a pool of worker threads that decode tracks and store their
//! waveform, beatgrid and key.
//!
//! Decoding and analysis run on the workers; each result is written through the
//! [`Writer`] as one [`WriteOp::StoreAnalysis`], so a track's waveform, grid, key and
//! analyzer version change together or not at all. Every worker holds at most one decoded
//! track, so at most `threads` tracks are in memory at once.

use std::collections::{HashMap, VecDeque};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;

use analysis::{BpmRange, Key, Waveform, detect_beatgrid, detect_key, waveform};
use rusqlite::OptionalExtension;

use crate::settings::{BpmRangeError, InvalidBpmRange};
use crate::writer::{FileStamp, StoreAnalysis};
use crate::{Library, LibraryError, Reader, WriteOp, Writer};

/// The version of the analysis algorithms. Tracks analyzed by an older version are
/// analyzed again; bump it whenever an analysis algorithm changes.
pub const ANALYZER_VERSION: u32 = 1;

/// The stored waveform's resolution. It is computed at
/// [`analysis::DEFAULT_FRAMES_PER_BUCKET`] (so the band window behaves as designed) and
/// downsampled to this, bounding the database size (about 170 KB per hour at 44.1 kHz).
pub const WAVEFORM_FRAMES_PER_BUCKET: u32 = 1024;

/// A track's stored analysis.
#[derive(Debug, Clone, PartialEq)]
pub struct AnalysisRow {
    pub analyzer_version: i64,
    pub bpm: Option<f64>,
    /// In frames at the track's native sample rate.
    pub first_beat_frames: Option<f64>,
    /// The user locked the grid: re-analysis keeps `bpm` and `first_beat_frames`.
    pub grid_locked: bool,
    pub key: Option<Key>,
    pub waveform: Option<Waveform>,
    /// Seconds since the Unix epoch.
    pub analyzed_at: Option<i64>,
}

/// Something worth telling the user that did not stop a track from being analyzed.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum AnalysisWarning {
    /// The track's audio ended early (a read or decode error, or shorter than its
    /// container declares); what was decoded was analyzed and stored with analyzer
    /// version 0, so it is analyzed again next time.
    Truncated { track_id: i64 },
    /// The stored `bpm_range` setting is invalid, so the 90–180 default was used.
    /// Reported once per distinct value.
    InvalidBpmRange { value: String, error: BpmRangeError },
}

impl std::fmt::Display for AnalysisWarning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Truncated { track_id } => write!(
                f,
                "track {track_id}'s audio ended early; the analysis covers what was decoded"
            ),
            Self::InvalidBpmRange { value, error } => write!(
                f,
                "the bpm_range setting {value:?} is invalid ({error}); using 90-180"
            ),
        }
    }
}

/// How many of the most recent failures and warnings an [`Analyzer`] keeps for
/// [`Analyzer::take_failures`] and [`Analyzer::take_warnings`]; older ones are dropped
/// (the totals still count them).
pub const REPORT_CAPACITY: usize = 1000;

/// How many of the most recent failures and warnings [`Analyzer::progress`] copies.
pub const PROGRESS_TAIL: usize = 20;

/// A snapshot of an [`Analyzer`]'s work since it started. Cheap to take: it copies
/// counters and at most [`PROGRESS_TAIL`] failures and warnings.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AnalysisProgress {
    /// Tracks waiting or being analyzed.
    pub queued: usize,
    /// Of `queued`, the tracks being analyzed right now.
    pub active: usize,
    /// Tracks analyzed and stored.
    pub done: usize,
    /// Tracks left alone because their analysis was already up to date.
    pub skipped: usize,
    /// Tracks that could not be analyzed since the analyzer started. Their existing
    /// analysis, if any, is untouched.
    pub failed_count: usize,
    /// Warnings since the analyzer started.
    pub warning_count: usize,
    /// The most recent failures (at most [`PROGRESS_TAIL`]), oldest first, with the
    /// reason. Failures taken with [`Analyzer::take_failures`] are no longer listed.
    pub failed: Vec<(i64, String)>,
    /// The most recent warnings (at most [`PROGRESS_TAIL`]), oldest first. Warnings taken
    /// with [`Analyzer::take_warnings`] are no longer listed.
    pub warnings: Vec<AnalysisWarning>,
}

/// The default worker count: one less than the available parallelism, at least 1, so
/// analysis leaves a core for the UI and audio.
#[must_use]
pub fn default_threads() -> usize {
    std::thread::available_parallelism()
        .map_or(1, |n| n.get().saturating_sub(1))
        .max(1)
}

#[derive(Debug, Default)]
struct State {
    /// Track ids in queue order, each at most once ...
    queue: VecDeque<i64>,
    /// ... with whether to analyze it even if up to date.
    force: HashMap<i64, bool>,
    running: usize,
    /// At most this many jobs run at once (see [`Analyzer::set_max_active`]).
    max_active: usize,
    done: usize,
    skipped: usize,
}

/// Failures and warnings, behind their own lock so reporting never contends with the job
/// queue.
#[derive(Debug, Default)]
struct Reports {
    failed: VecDeque<(i64, String)>,
    failed_count: usize,
    warnings: VecDeque<AnalysisWarning>,
    warning_count: usize,
    /// The last invalid `bpm_range` value reported, so it is reported once.
    reported_bpm_range: Option<String>,
}

impl Reports {
    fn fail(&mut self, id: i64, error: String) {
        self.failed_count += 1;
        push_capped(&mut self.failed, (id, error));
    }

    fn warn(&mut self, warning: AnalysisWarning) {
        self.warning_count += 1;
        push_capped(&mut self.warnings, warning);
    }
}

fn push_capped<T>(items: &mut VecDeque<T>, item: T) {
    if items.len() >= REPORT_CAPACITY {
        items.pop_front();
    }
    items.push_back(item);
}

fn tail<T: Clone>(items: &VecDeque<T>) -> Vec<T> {
    items
        .iter()
        .skip(items.len().saturating_sub(PROGRESS_TAIL))
        .cloned()
        .collect()
}

#[derive(Debug)]
struct Shared {
    state: Mutex<State>,
    reports: Mutex<Reports>,
    wake: Condvar,
    stop: AtomicBool,
    db_path: PathBuf,
    writer: Writer,
}

impl Shared {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn reports(&self) -> MutexGuard<'_, Reports> {
        self.reports.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }
}

/// A pool of analysis workers. Dropping it stops them promptly: the queue is abandoned
/// and only the track each worker is on finishes (cut short between steps where
/// possible); nothing half-done is stored.
#[derive(Debug)]
pub struct Analyzer {
    shared: Arc<Shared>,
    threads: Vec<JoinHandle<()>>,
}

impl Analyzer {
    /// Starts `threads` workers (at least 1; see [`default_threads`]) on `lib`.
    ///
    /// Workers would ideally run at a lower OS priority, but the standard library has no
    /// portable way to set one and this crate forbids `unsafe`, so they run at normal
    /// priority; [`default_threads`] leaves a core free instead.
    ///
    /// A worker that cannot be spawned is skipped.
    ///
    /// # Errors
    ///
    /// [`LibraryError::Io`] if not a single worker thread could be spawned.
    pub fn start(lib: &Library, threads: usize) -> Result<Self, LibraryError> {
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                max_active: usize::MAX,
                ..State::default()
            }),
            reports: Mutex::default(),
            wake: Condvar::new(),
            stop: AtomicBool::new(false),
            db_path: lib.path().to_owned(),
            writer: lib.writer(),
        });
        let mut handles = Vec::new();
        let mut last_error = None;
        for i in 0..threads.max(1) {
            let thread_shared = Arc::clone(&shared);
            match std::thread::Builder::new()
                .name(format!("library-analysis-{i}"))
                .spawn(move || worker(&thread_shared))
            {
                Ok(handle) => handles.push(handle),
                Err(e) => last_error = Some(e),
            }
        }
        match last_error {
            Some(e) if handles.is_empty() => Err(LibraryError::Io(e)),
            _ => Ok(Self {
                shared,
                threads: handles,
            }),
        }
    }

    /// Queues tracks for analysis. Tracks whose stored analysis was made by
    /// [`ANALYZER_VERSION`] are skipped unless `force`. A track already waiting is not
    /// queued twice (forcing it if either request did).
    pub fn enqueue(&self, track_ids: &[i64], force: bool) {
        let mut state = self.shared.state();
        for &id in track_ids {
            match state.force.get_mut(&id) {
                Some(f) => *f |= force,
                None => {
                    state.force.insert(id, force);
                    state.queue.push_back(id);
                }
            }
        }
        drop(state);
        self.shared.wake.notify_all();
    }

    /// Lets at most `n` workers analyze at once; the others wait. `0` pauses analysis
    /// (tracks being analyzed finish), and anything at or above the worker count lets
    /// every worker run. A worker over the limit finishes its current track before it
    /// waits. The queue is kept.
    pub fn set_max_active(&self, n: usize) {
        self.shared.state().max_active = n;
        self.shared.wake.notify_all();
    }

    /// The limit set by [`Analyzer::set_max_active`] (`usize::MAX` when none was set).
    #[must_use]
    pub fn max_active(&self) -> usize {
        self.shared.state().max_active
    }

    /// What has happened so far. Takes the queue lock and the report lock one after the
    /// other, each only to copy counters and a short tail.
    #[must_use]
    pub fn progress(&self) -> AnalysisProgress {
        let (queued, active, done, skipped) = {
            let state = self.shared.state();
            (
                state.queue.len() + state.running,
                state.running,
                state.done,
                state.skipped,
            )
        };
        let reports = self.shared.reports();
        AnalysisProgress {
            queued,
            active,
            done,
            skipped,
            failed_count: reports.failed_count,
            warning_count: reports.warning_count,
            failed: tail(&reports.failed),
            warnings: tail(&reports.warnings),
        }
    }

    /// Takes the kept failures (the most recent [`REPORT_CAPACITY`]), oldest first. The
    /// total in [`AnalysisProgress::failed_count`] is unchanged.
    #[must_use]
    pub fn take_failures(&self) -> Vec<(i64, String)> {
        self.shared.reports().failed.drain(..).collect()
    }

    /// Takes the kept warnings (the most recent [`REPORT_CAPACITY`]), oldest first. The
    /// total in [`AnalysisProgress::warning_count`] is unchanged.
    #[must_use]
    pub fn take_warnings(&self) -> Vec<AnalysisWarning> {
        self.shared.reports().warnings.drain(..).collect()
    }
}

impl Drop for Analyzer {
    fn drop(&mut self) {
        {
            // Set under the lock so a worker cannot miss the wake-up between checking the
            // flag and waiting.
            let _state = self.shared.state();
            self.shared.stop.store(true, Ordering::Relaxed);
        }
        self.shared.wake.notify_all();
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

/// How one job ended.
enum Outcome {
    Done {
        truncated: bool,
    },
    Skipped,
    /// Stopped before storing anything.
    Stopped,
    Failed(String),
}

fn worker(shared: &Shared) {
    let mut reader = None;
    while let Some((id, force)) = next_job(shared) {
        let mut invalid_range = None;
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            run_job(shared, &mut reader, id, force, &mut invalid_range)
        }))
        .unwrap_or_else(|_| Outcome::Failed("analysis panicked".into()));
        // Reports first, so once `queued` reads 0 every report is visible. The two locks
        // are never held together.
        {
            let mut reports = shared.reports();
            if let Some(InvalidBpmRange { value, error }) = invalid_range
                && reports.reported_bpm_range.as_ref() != Some(&value)
            {
                reports.reported_bpm_range = Some(value.clone());
                reports.warn(AnalysisWarning::InvalidBpmRange { value, error });
            }
            match &outcome {
                Outcome::Done { truncated: true } => {
                    reports.warn(AnalysisWarning::Truncated { track_id: id });
                }
                Outcome::Failed(error) => reports.fail(id, error.clone()),
                Outcome::Done { .. } | Outcome::Skipped | Outcome::Stopped => {}
            }
        }
        let mut state = shared.state();
        // This worker takes the next job itself, so lowering `running` wakes no one.
        state.running -= 1;
        match outcome {
            Outcome::Done { .. } => state.done += 1,
            Outcome::Skipped => state.skipped += 1,
            Outcome::Stopped | Outcome::Failed(_) => {}
        }
    }
}

/// Waits for the next job, or returns `None` once stopped.
fn next_job(shared: &Shared) -> Option<(i64, bool)> {
    let mut state = shared.state();
    loop {
        if shared.stopped() {
            return None;
        }
        if state.running >= state.max_active {
            // Over the limit: wait for it to rise (or for the stop).
        } else if let Some(id) = state.queue.pop_front() {
            let force = state.force.remove(&id).unwrap_or(false);
            state.running += 1;
            return Some((id, force));
        }
        state = shared
            .wake
            .wait(state)
            .unwrap_or_else(PoisonError::into_inner);
    }
}

/// What the database says about a track before analyzing it.
struct TrackInfo {
    path: String,
    missing: bool,
    fingerprint: Vec<u8>,
    file: FileStamp,
    analyzer_version: Option<i64>,
    grid_locked: bool,
    bpm: Option<f64>,
    first_beat_frames: Option<f64>,
}

fn track_info(reader: &Reader, id: i64) -> Result<Option<TrackInfo>, LibraryError> {
    Ok(reader
        .connection()
        .prepare_cached(
            "SELECT t.path, t.missing, t.fingerprint, a.analyzer_version, \
                    coalesce(a.grid_locked, 0), a.bpm, a.first_beat_frames, t.size, t.mtime \
             FROM tracks t LEFT JOIN analysis a ON a.track_id = t.id WHERE t.id = ?1",
        )?
        .query_row([id], |r| {
            Ok(TrackInfo {
                path: r.get(0)?,
                missing: r.get(1)?,
                fingerprint: r.get(2)?,
                file: FileStamp {
                    size: r.get(7)?,
                    mtime: r.get(8)?,
                },
                analyzer_version: r.get(3)?,
                grid_locked: r.get(4)?,
                bpm: r.get(5)?,
                first_beat_frames: r.get(6)?,
            })
        })
        .optional()?)
}

fn run_job(
    shared: &Shared,
    reader: &mut Option<Reader>,
    id: i64,
    force: bool,
    invalid_range: &mut Option<InvalidBpmRange>,
) -> Outcome {
    let (info, range) = match read_job(shared, reader, id) {
        Ok(Some((info, range, invalid))) => {
            *invalid_range = invalid;
            (info, range)
        }
        Ok(None) => return Outcome::Failed(LibraryError::NoSuchTrack(id).to_string()),
        Err(e) => {
            // Reopen the connection for the next job in case this one is broken.
            *reader = None;
            return Outcome::Failed(e.to_string());
        }
    };
    // Up to date: nothing to do, even if the file has since gone missing.
    if !force && info.analyzer_version == Some(i64::from(ANALYZER_VERSION)) {
        return Outcome::Skipped;
    }
    if info.missing {
        return Outcome::Failed(format!(
            "the file is missing (flagged by the last scan): {}",
            info.path
        ));
    }
    let decoded = match decode::decode_file(Path::new(&info.path)) {
        Ok(d) => d,
        Err(decode::DecodeError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {
            return Outcome::Failed(format!("file not found: {}", info.path));
        }
        Err(e) => return Outcome::Failed(format!("could not decode {}: {e}", info.path)),
    };
    if shared.stopped() {
        return Outcome::Stopped;
    }
    let track = &decoded.track;
    let wave = waveform(track, analysis::DEFAULT_FRAMES_PER_BUCKET)
        .downsample(WAVEFORM_FRAMES_PER_BUCKET / analysis::DEFAULT_FRAMES_PER_BUCKET);
    let (bpm, first_beat_frames) = if info.grid_locked {
        // The writer keeps the locked values anyway; skip the detection.
        (info.bpm, info.first_beat_frames)
    } else {
        detect_beatgrid(track, range)
            .map_or((None, None), |g| (Some(g.bpm), Some(g.first_beat_frames)))
    };
    if shared.stopped() {
        return Outcome::Stopped;
    }
    let key = detect_key(track).map(Key::index);
    let truncated = decoded.truncated;
    drop(decoded);
    if shared.stopped() {
        return Outcome::Stopped;
    }
    let op = WriteOp::StoreAnalysis(Box::new(StoreAnalysis {
        track_id: id,
        fingerprint: info.fingerprint,
        file: Some(info.file),
        // An analysis of part of the audio is stored as out of date, so it is redone
        // (say, once a file that was still being copied is complete).
        analyzer_version: if truncated { 0 } else { ANALYZER_VERSION },
        bpm,
        first_beat_frames,
        key,
        waveform: wave.to_bytes(),
    }));
    match shared.writer.submit_wait(op) {
        Ok(_) => Outcome::Done { truncated },
        Err(e) => {
            if matches!(e, LibraryError::WriterGone) {
                // The library closed: nothing more can be stored.
                shared.stop.store(true, Ordering::Relaxed);
            }
            Outcome::Failed(e.to_string())
        }
    }
}

type JobInput = (TrackInfo, BpmRange, Option<InvalidBpmRange>);

fn read_job(
    shared: &Shared,
    reader: &mut Option<Reader>,
    id: i64,
) -> Result<Option<JobInput>, LibraryError> {
    let reader = match reader {
        Some(r) => r,
        None => reader.insert(Library::reader_at(&shared.db_path)?),
    };
    let Some(info) = track_info(reader, id)? else {
        return Ok(None);
    };
    let (range, invalid) = reader.bpm_range()?;
    Ok(Some((info, range, invalid)))
}

/// A track that needs analysis, with its file's size and modification time as last
/// scanned, so a caller can tell when a file changed after it was queued.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PendingTrack {
    pub id: i64,
    pub size: Option<i64>,
    pub mtime: Option<i64>,
}

impl Reader {
    /// The tracks [`Reader::track_ids_to_analyze`] lists (without the current ones), with
    /// their file's size and mtime.
    ///
    /// # Errors
    ///
    /// Any database error.
    pub fn tracks_to_analyze(&self) -> Result<Vec<PendingTrack>, LibraryError> {
        let mut stmt = self.connection().prepare_cached(
            "SELECT t.id, t.size, t.mtime FROM tracks t \
             LEFT JOIN analysis a ON a.track_id = t.id \
             WHERE t.missing = 0 AND a.analyzer_version IS NOT ?1 ORDER BY t.id",
        )?;
        let rows = stmt.query_map([ANALYZER_VERSION], |r| {
            Ok(PendingTrack {
                id: r.get(0)?,
                size: r.get(1)?,
                mtime: r.get(2)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// The ids of the tracks whose file is present and that have no analysis by
    /// [`ANALYZER_VERSION`] yet, in id order; with `include_current`, every present track.
    ///
    /// # Errors
    ///
    /// Any database error.
    pub fn track_ids_to_analyze(&self, include_current: bool) -> Result<Vec<i64>, LibraryError> {
        let mut stmt = self.connection().prepare_cached(
            "SELECT t.id FROM tracks t LEFT JOIN analysis a ON a.track_id = t.id \
             WHERE t.missing = 0 AND (?1 OR a.analyzer_version IS NOT ?2) ORDER BY t.id",
        )?;
        let rows = stmt.query_map((include_current, ANALYZER_VERSION), |r| r.get(0))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Track `track_id`'s stored analysis, if it has been analyzed (and not cleared since).
    ///
    /// # Errors
    ///
    /// Any database error, or [`LibraryError::CorruptWaveform`] if the stored waveform
    /// cannot be read.
    pub fn analysis(&self, track_id: i64) -> Result<Option<AnalysisRow>, LibraryError> {
        let row = self
            .connection()
            .prepare_cached(
                "SELECT analyzer_version, bpm, first_beat_frames, grid_locked, key, waveform, \
                        analyzed_at \
                 FROM analysis WHERE track_id = ?1",
            )?
            .query_row([track_id], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, Option<f64>>(1)?,
                    r.get::<_, Option<f64>>(2)?,
                    r.get::<_, bool>(3)?,
                    r.get::<_, Option<i64>>(4)?,
                    r.get::<_, Option<Vec<u8>>>(5)?,
                    r.get::<_, Option<i64>>(6)?,
                ))
            })
            .optional()?;
        let Some((analyzer_version, bpm, first_beat_frames, grid_locked, key, blob, at)) = row
        else {
            return Ok(None);
        };
        let waveform = blob
            .map(|b| Waveform::from_bytes(&b))
            .transpose()
            .map_err(|source| LibraryError::CorruptWaveform { track_id, source })?;
        Ok(Some(AnalysisRow {
            analyzer_version,
            bpm,
            first_beat_frames,
            grid_locked,
            // An out-of-range stored key reads as unknown.
            key: key.and_then(|k| u8::try_from(k).ok()).and_then(Key::new),
            waveform,
            analyzed_at: at,
        }))
    }
}

impl Library {
    /// [`Reader::analysis`] on a fresh reader.
    ///
    /// # Errors
    ///
    /// As [`Reader::analysis`].
    pub fn analysis(&self, track_id: i64) -> Result<Option<AnalysisRow>, LibraryError> {
        self.reader()?.analysis(track_id)
    }
}
