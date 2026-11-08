//! Stems for library tracks: cache lookups and separations on one background thread, so
//! the control loop never waits on a decode (~0.7 s), a separation (~20 s and more) or a
//! cache write (~7 s), and the audio thread never sees any of it.
//!
//! One job per track: decks holding the same track share it. The thread looks a job up in
//! the stems cache as soon as it arrives; a miss waits its turn for the separator, one
//! separation at a time, so a separation's memory peak never meets another one's or a
//! cache hit's decode. A job no deck wants any more is cancelled (its flag is checked once
//! per chunk), and its result, if it still comes, is discarded.
//!
//! The separator opens on its own thread at startup (the CoreML compile cache is per
//! executable, so the first open after an update takes about a minute); cache hits are
//! served meanwhile and separations wait for it.

use std::cell::Cell;
use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, TryRecvError, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use engine::{DeckId, StemSet, TrackBuffer};
use library::{Eviction, Library, StemCache, StemKey};
use stems::{SeparateError, Separator};

use crate::format::safe;

/// Printed once at startup when stems are enabled and `stems setup` has not been run.
pub const NO_MODEL: &str = "stems: no model, run `open-jockey stems setup`";

/// While the separator is still opening, how often the thread looks for it.
const MODEL_POLL: Duration = Duration::from_millis(50);
/// While a deck plays, the separation pauses after each chunk for this fraction of the
/// chunk's time, leaving the processor some room for the audio (the analyzer, likewise,
/// holds back while a deck plays) ...
const PAUSE_WHILE_PLAYING: f64 = 0.25;
/// ... but never longer than this at once.
const MAX_PAUSE: Duration = Duration::from_millis(500);
/// At shutdown, how long to wait for the thread to finish what it is doing (a cache write
/// cannot be interrupted); after that it is left to end with the process.
const SHUTDOWN_WAIT: Duration = Duration::from_secs(2);

/// A separator whose next run can start around the deck's playhead.
pub trait StemSeparator: Separator {
    /// The next separation starts around `frame` ("around the playhead first").
    fn start_at(&mut self, _frame: usize) {}
}

impl StemSeparator for stems::SyntheticSeparator {}

impl StemSeparator for stems::ModelSeparator {
    fn start_at(&mut self, frame: usize) {
        stems::ModelSeparator::start_at(self, frame);
    }
}

/// Opens the separator (on its own thread).
pub type OpenSeparator = Box<dyn FnOnce() -> Result<Box<dyn StemSeparator>, String> + Send>;

/// Where separations come from.
pub enum SeparatorSource {
    /// No model installed: cached stems still load, nothing is separated.
    Missing,
    Open(OpenSeparator),
}

/// How stems run, when they are enabled.
pub struct StemsSetup {
    /// The stems cache's budget (`stems.cache_gb`).
    pub budget_bytes: u64,
    pub separator: SeparatorSource,
}

impl StemsSetup {
    /// The real setup: the model `stems setup` installed, if it is there.
    pub fn from_config(config: &crate::config::StemsConfig) -> Self {
        let model = stems::model_path();
        let separator = if model.is_file() {
            SeparatorSource::Open(Box::new(move || open_model(&model)))
        } else {
            SeparatorSource::Missing
        };
        Self {
            budget_bytes: crate::library_cmd::gb_to_bytes(config.cache_gb),
            separator,
        }
    }
}

fn open_model(model: &Path) -> Result<Box<dyn StemSeparator>, String> {
    let options = stems::ModelOptions {
        // On the CPU path, leave a core free, as the analyzer does.
        threads: Some(library::default_threads()),
        ..stems::ModelOptions::default()
    };
    stems::ModelSeparator::new(model, &options)
        .map(|separator| Box::new(separator) as Box<dyn StemSeparator>)
        .map_err(|e| e.to_string())
}

/// What the control loop tells the thread without a message: whether a deck plays, the
/// decks' playheads and the tracks they hold.
#[derive(Debug, Default)]
struct Shared {
    playing: AtomicBool,
    /// Each deck's playhead, as `f64` bits.
    playheads: [AtomicU64; 4],
    /// The library tracks the decks hold (or are still replacing): never evicted.
    loaded: Mutex<Vec<i64>>,
}

/// A job's state, as the status line shows it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum JobStatus {
    /// Waiting for the cache lookup or its turn.
    Queued,
    /// Waiting for the separator to open.
    PreparingModel,
    /// Separating, this fraction done.
    Separating(f32),
}

/// What the thread reports.
#[derive(Debug)]
pub enum Reply {
    /// A line to print.
    Message(String),
    Status(u64, JobStatus),
    /// Job `.0`'s stems, from the cache or just separated.
    Stems(u64, Arc<StemSet>),
    /// Job `.0` is over (after its stems, if any).
    Done(u64),
    /// What storing a separation evicted.
    Evicted(Eviction),
}

struct Job {
    id: u64,
    /// The deck that asked first: the separation starts around its playhead.
    deck: DeckId,
    track: i64,
    buffer: Arc<TrackBuffer>,
    cancel: Arc<AtomicBool>,
}

/// A job the control loop is waiting on.
struct JobEntry {
    track: i64,
    frames: usize,
    sample_rate: u32,
    cancel: Arc<AtomicBool>,
    status: JobStatus,
}

/// The control loop's side: queues jobs, cancels the ones no deck wants, and hands over
/// the thread's replies.
pub struct StemsControl {
    jobs: Option<Sender<Job>>,
    replies: Receiver<Reply>,
    shared: Arc<Shared>,
    running: HashMap<u64, JobEntry>,
    next_id: u64,
    /// The last cache notice printed.
    last_notice: Option<String>,
    /// Closed when the thread ends.
    finished: Receiver<()>,
}

impl StemsControl {
    /// Starts the thread (and the separator's opening). With no model, [`NO_MODEL`] is the
    /// first reply.
    pub fn start(lib: Arc<Library>, setup: StemsSetup) -> std::io::Result<Self> {
        let (jobs_tx, jobs) = channel();
        let (replies_tx, replies) = channel();
        let (finished_tx, finished) = channel::<()>();
        let shared = Arc::new(Shared::default());
        let model = match setup.separator {
            SeparatorSource::Missing => {
                let _ = replies_tx.send(Reply::Message(NO_MODEL.to_owned()));
                Model::Unavailable
            }
            SeparatorSource::Open(open) => {
                let (model_tx, model_rx) = channel();
                let replies = replies_tx.clone();
                let opened = std::thread::Builder::new()
                    .name("stems-model".into())
                    .spawn(move || {
                        let result = open();
                        if let Err(e) = &result {
                            let _ = replies.send(Reply::Message(format!(
                                "stems: the model could not be opened, the EQ knobs stay EQ: {}",
                                safe(e)
                            )));
                        }
                        let _ = model_tx.send(result);
                    });
                match opened {
                    Ok(_) => Model::Opening(model_rx),
                    Err(e) => {
                        let _ = replies_tx.send(Reply::Message(format!(
                            "stems: the model could not be opened, the EQ knobs stay EQ: {e}"
                        )));
                        Model::Unavailable
                    }
                }
            }
        };
        let thread_shared = Arc::clone(&shared);
        let dir = crate::library_cmd::stems_dir(lib.path());
        std::thread::Builder::new()
            .name("stems".into())
            .spawn(move || {
                let _finished = finished_tx;
                let say = |text: String| {
                    let _ = replies_tx.send(Reply::Message(text));
                };
                match StemCache::new(&lib, dir, setup.budget_bytes) {
                    Ok(cache) => Worker {
                        cache,
                        model,
                        shared: thread_shared,
                        replies: replies_tx.clone(),
                    }
                    .run(&jobs),
                    Err(e) => say(format!(
                        "warning: stems are off this session, the stems cache cannot be opened: {}",
                        safe(e)
                    )),
                }
            })?;
        Ok(Self {
            jobs: Some(jobs_tx),
            replies,
            shared,
            running: HashMap::new(),
            next_id: 0,
            last_notice: None,
            finished,
        })
    }

    /// Asks for the stems of `track`, decoded as `buffer`, for `deck`: joins the job
    /// already running for the same track and decode, if any. Returns the job, or `None`
    /// if the thread has stopped (it said why).
    pub fn request(&mut self, deck: DeckId, track: i64, buffer: Arc<TrackBuffer>) -> Option<u64> {
        let same = self.running.iter().find(|(_, j)| {
            j.track == track
                && j.frames == buffer.frames()
                && j.sample_rate == buffer.sample_rate()
                && !j.cancel.load(Ordering::Relaxed)
        });
        if let Some((&id, _)) = same {
            return Some(id);
        }
        self.next_id += 1;
        let id = self.next_id;
        let cancel = Arc::new(AtomicBool::new(false));
        self.running.insert(
            id,
            JobEntry {
                track,
                frames: buffer.frames(),
                sample_rate: buffer.sample_rate(),
                cancel: Arc::clone(&cancel),
                status: JobStatus::Queued,
            },
        );
        let job = Job {
            id,
            deck,
            track,
            buffer,
            cancel,
        };
        // A stopped thread (its cache could not open) has said so.
        if self
            .jobs
            .as_ref()
            .is_none_or(|jobs| jobs.send(job).is_err())
        {
            self.running.remove(&id);
            return None;
        }
        Some(id)
    }

    /// Cancels the jobs `wanted` says no deck waits on any more.
    pub fn cancel_unwanted(&mut self, wanted: impl Fn(u64) -> bool) {
        for (&id, job) in &self.running {
            if !wanted(id) {
                job.cancel.store(true, Ordering::Relaxed);
            }
        }
    }

    pub fn status(&self, id: u64) -> Option<JobStatus> {
        self.running.get(&id).map(|j| j.status)
    }

    /// No job is waiting.
    #[cfg(test)]
    pub fn idle(&self) -> bool {
        self.running.is_empty()
    }

    /// The replies so far.
    pub fn take_replies(&mut self) -> Vec<Reply> {
        self.replies.try_iter().collect()
    }

    /// Records job `id`'s status; true if what the status line shows changed.
    pub fn set_status(&mut self, id: u64, status: JobStatus) -> bool {
        let Some(job) = self.running.get_mut(&id) else {
            return false;
        };
        let changed = status_text(Some(job.status)) != status_text(Some(status));
        job.status = status;
        changed
    }

    /// Forgets job `id` (its stems came, or it is over), returning its track if it was
    /// still running.
    pub fn finish(&mut self, id: u64) -> Option<i64> {
        self.running.remove(&id).map(|j| j.track)
    }

    /// The lines to print for `eviction`: its notice when it is a new one, and how many
    /// entries could not be removed.
    pub fn eviction_lines(&mut self, eviction: &Eviction) -> Vec<String> {
        let mut lines = Vec::new();
        if let Some(notice) = &eviction.notice
            && self.last_notice.as_ref() != Some(notice)
        {
            lines.push(safe(notice));
        }
        self.last_notice.clone_from(&eviction.notice);
        if eviction.failed > 0 {
            lines.push(format!(
                "warning: stems cache: {} entr{} could not be removed (retried later)",
                eviction.failed,
                if eviction.failed == 1 { "y" } else { "ies" }
            ));
        }
        lines
    }

    /// Tells the thread whether a deck plays, where the playheads are and which tracks
    /// the decks hold.
    pub fn update(&self, playing: bool, playheads: [f64; 4], loaded: Vec<i64>) {
        self.shared.playing.store(playing, Ordering::Relaxed);
        for (slot, frame) in self.shared.playheads.iter().zip(playheads) {
            slot.store(frame.to_bits(), Ordering::Relaxed);
        }
        *self.shared.loaded.lock().unwrap_or_else(|e| e.into_inner()) = loaded;
    }

    /// Cancels every job and stops the thread, waiting up to [`SHUTDOWN_WAIT`] for it.
    /// Returns the messages it still had.
    pub fn shutdown(mut self) -> Vec<String> {
        for job in self.running.values() {
            job.cancel.store(true, Ordering::Relaxed);
        }
        self.jobs = None;
        let _ = self.finished.recv_timeout(SHUTDOWN_WAIT);
        self.replies
            .try_iter()
            .filter_map(|r| match r {
                Reply::Message(text) => Some(text),
                _ => None,
            })
            .collect()
    }
}

/// The status line's text for a job in `status`: `stems: preparing model` or `stems 40%`;
/// nothing while it waits for the cache or its turn.
pub fn status_text(status: Option<JobStatus>) -> Option<String> {
    match status? {
        JobStatus::Queued => None,
        JobStatus::PreparingModel => Some("stems: preparing model".to_owned()),
        JobStatus::Separating(done) => {
            // A fraction in [0, 1] as a whole percent, rounded down.
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let percent = (done.clamp(0.0, 1.0) * 100.0) as u32;
            Some(format!("stems {percent}%"))
        }
    }
}

/// The separator, as the thread has it.
enum Model {
    Opening(Receiver<Result<Box<dyn StemSeparator>, String>>),
    Ready(Box<dyn StemSeparator>),
    /// Missing, or failed to open (said once already).
    Unavailable,
}

impl Model {
    /// Takes the separator if it has opened since.
    fn poll(&mut self) {
        if let Model::Opening(rx) = self {
            *self = match rx.try_recv() {
                Ok(Ok(separator)) => Model::Ready(separator),
                Ok(Err(_)) | Err(TryRecvError::Disconnected) => Model::Unavailable,
                Err(TryRecvError::Empty) => return,
            };
        }
    }
}

/// The stems thread.
struct Worker {
    cache: StemCache,
    model: Model,
    shared: Arc<Shared>,
    replies: Sender<Reply>,
}

impl Worker {
    fn reply(&self, reply: Reply) {
        let _ = self.replies.send(reply);
    }

    /// Serves jobs until the control loop drops its sender.
    fn run(mut self, jobs: &Receiver<Job>) {
        // Cache misses waiting for the separator, with their fingerprint.
        let mut pending: VecDeque<(Job, Vec<u8>)> = VecDeque::new();
        let mut told_waiting = None;
        loop {
            self.model.poll();
            let first = if pending.is_empty() {
                match jobs.recv() {
                    Ok(job) => Some(job),
                    Err(_) => return,
                }
            } else {
                let wait = match self.model {
                    Model::Opening(_) => MODEL_POLL,
                    _ => Duration::ZERO,
                };
                match jobs.recv_timeout(wait) {
                    Ok(job) => Some(job),
                    Err(RecvTimeoutError::Timeout) => None,
                    Err(RecvTimeoutError::Disconnected) => return,
                }
            };
            let arrived: Vec<Job> = first.into_iter().chain(jobs.try_iter()).collect();
            for job in arrived {
                if let Some(miss) = self.look_up(job) {
                    self.reply(Reply::Status(miss.0.id, JobStatus::Queued));
                    pending.push_back(miss);
                }
            }
            pending.retain(|(job, _)| {
                let cancelled = job.cancel.load(Ordering::Relaxed);
                if cancelled {
                    self.reply(Reply::Done(job.id));
                }
                !cancelled
            });
            let Some((front, _)) = pending.front() else {
                continue;
            };
            match self.model {
                Model::Opening(_) => {
                    if told_waiting != Some(front.id) {
                        told_waiting = Some(front.id);
                        self.reply(Reply::Status(front.id, JobStatus::PreparingModel));
                    }
                }
                Model::Unavailable => {
                    let (job, _) = pending.pop_front().expect("a front job");
                    self.reply(Reply::Done(job.id));
                }
                Model::Ready(_) => {
                    let (job, fingerprint) = pending.pop_front().expect("a front job");
                    self.separate(job, &fingerprint);
                }
            }
        }
    }

    /// Sends `job`'s cached stems, or returns it with its fingerprint when it has to be
    /// separated. A track the library cannot key, or a miss with no separator, ends there.
    fn look_up(&self, job: Job) -> Option<(Job, Vec<u8>)> {
        if job.cancel.load(Ordering::Relaxed) {
            self.reply(Reply::Done(job.id));
            return None;
        }
        let fingerprint = match self.cache.fingerprint(job.track) {
            Ok(Some(fingerprint)) => fingerprint,
            Ok(None) => {
                self.reply(Reply::Done(job.id));
                return None;
            }
            Err(e) => {
                self.reply(Reply::Message(format!(
                    "warning: stems: cannot read track {}: {}",
                    job.track,
                    safe(e)
                )));
                self.reply(Reply::Done(job.id));
                return None;
            }
        };
        let key = key(&job, &fingerprint);
        if let Some(set) = self.cache.get(&key) {
            self.reply(Reply::Stems(job.id, Arc::new(set)));
            self.reply(Reply::Done(job.id));
            return None;
        }
        if matches!(self.model, Model::Unavailable) {
            self.reply(Reply::Done(job.id));
            return None;
        }
        Some((job, fingerprint))
    }

    /// Separates `job` around its deck's playhead, sends the stems unless the job was
    /// cancelled meanwhile, and keeps them in the cache.
    fn separate(&mut self, job: Job, fingerprint: &[u8]) {
        let Model::Ready(separator) = &mut self.model else {
            return;
        };
        let playhead =
            f64::from_bits(self.shared.playheads[job.deck.index()].load(Ordering::Relaxed));
        // A playhead in frames, never negative.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        separator.start_at(playhead.max(0.0) as usize);
        let _ = self
            .replies
            .send(Reply::Status(job.id, JobStatus::Separating(0.0)));
        let chunk_start = Cell::new(Instant::now());
        let progress = |done: f32| {
            let _ = self
                .replies
                .send(Reply::Status(job.id, JobStatus::Separating(done)));
            if self.shared.playing.load(Ordering::Relaxed) {
                let pause = chunk_start.get().elapsed().mul_f64(PAUSE_WHILE_PLAYING);
                std::thread::sleep(pause.min(MAX_PAUSE));
            }
            chunk_start.set(Instant::now());
        };
        let result = separator.separate(&job.buffer, &progress, &job.cancel);
        match result {
            // Cancelled meanwhile: no deck wants it now, so it is neither sent nor kept.
            Ok(_) if job.cancel.load(Ordering::Relaxed) => {}
            Ok(set) => {
                let set = Arc::new(set);
                self.reply(Reply::Stems(job.id, Arc::clone(&set)));
                let loaded = self
                    .shared
                    .loaded
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .clone();
                match self
                    .cache
                    .put(&key(&job, fingerprint), &set, false, &loaded)
                {
                    Ok(eviction) => self.reply(Reply::Evicted(eviction)),
                    Err(e) => self.reply(Reply::Message(format!(
                        "warning: stems: could not keep the stems of track {}: {}",
                        job.track,
                        safe(e)
                    ))),
                }
            }
            // The control loop refuses long tracks before asking.
            Err(SeparateError::Cancelled | SeparateError::TooLong { .. }) => {}
            Err(e) => self.reply(Reply::Message(format!(
                "stems: track {} could not be separated: {}",
                job.track,
                safe(e)
            ))),
        }
        self.reply(Reply::Done(job.id));
    }
}

fn key<'a>(job: &Job, fingerprint: &'a [u8]) -> StemKey<'a> {
    StemKey {
        track: job.track,
        fingerprint,
        sample_rate: job.buffer.sample_rate(),
        frames: job.buffer.frames(),
    }
}
