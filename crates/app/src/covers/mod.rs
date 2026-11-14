//! Covers looked up online for the library's tracks, on a thread of their own
//! ([`CoverWorker`]): Deezer, then MusicBrainz with the Cover Art Archive (see
//! [`service`]). A cover found is shrunk to at most [`COVER_SIDE`] pixels a side, saved as
//! a JPEG in the library's `covers` folder and recorded through the library's writer.
//!
//! Only artist and title (or the file name without its extension) are sent. The control
//! and window threads never wait for a lookup: they set flags and queue requests without
//! blocking. A network failure records nothing (the track is looked up again later) and
//! pauses the lookups; any other answer that is not a cover, from every service, records a
//! miss. A track whose file cannot be read (its drive unplugged) is not looked up: nothing
//! can say whether it has a picture of its own.

pub mod service;

use std::cell::RefCell;
use std::collections::HashMap;
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use library::{COVERS_RETRY_MISSED, CoverQuery, CoverState, Library, Reader, WriteOp, Writer};
pub use service::{CoverService, LookupError};

/// The longest side of a cover kept, in pixels.
pub const COVER_SIDE: u32 = 512;
/// How many tracks are read from the library at a time.
const BATCH: usize = 20;
/// The widest or tallest picture decoded: larger ones are refused before decoding.
const MAX_DECODED_SIDE: u32 = 4096;
/// The most memory decoding one picture may take.
const MAX_DECODE_ALLOC: u64 = 96 * 1024 * 1024;
/// The JPEG quality covers are saved at.
const JPEG_QUALITY: u8 = 85;
/// How many requests may wait for the thread; more are refused.
const QUEUE: usize = 32;
/// How long dropping the worker waits for its thread to end: a request in flight is not
/// waited for, and its answer is then dropped.
const STOP_WAIT: Duration = Duration::from_secs(1);

/// How long the lookups wait.
#[derive(Debug, Clone, Copy)]
pub struct Timing {
    /// After a network failure, before the next request.
    pub offline_pause: Duration,
    /// With nothing to look up, before the library is asked again for new tracks.
    pub idle_poll: Duration,
    /// Before a track whose file could not be read, or whose lookup could not reach the
    /// services, is looked up again.
    pub retry_later: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            offline_pause: Duration::from_secs(60),
            idle_poll: Duration::from_secs(30),
            retry_later: Duration::from_secs(600),
        }
    }
}

/// What the thread is asked for besides its own walk through the library.
enum Request {
    /// Look this track up now, for another cover than the one it has ("Find another
    /// cover"); the track's removal count when it was asked, see [`CoverWorker::removed`].
    Find(i64, u64),
    /// Something changed (the switch, the retry flag, a stop): look again at once.
    Wake,
}

/// The lookups' thread, and how the session talks to it. Dropping it stops the thread.
pub struct CoverWorker {
    /// `library.cover_lookup`: lookups run only while it is set.
    enabled: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    requests: SyncSender<Request>,
    events: Receiver<String>,
    /// Disconnected once the thread has ended.
    ended: Receiver<()>,
    /// Per track, how many times its cover was removed while this worker ran.
    removals: Removals,
}

/// Per track, how many times its cover was removed: a "Find another cover" asked before a
/// removal is not written after it.
type Removals = Arc<Mutex<HashMap<i64, u64>>>;

fn removals_of(removals: &Removals, track: i64) -> u64 {
    removals
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&track)
        .copied()
        .unwrap_or(0)
}

impl CoverWorker {
    /// Starts looking covers up for `lib`'s tracks with `service`, while `setting` is set.
    ///
    /// # Errors
    ///
    /// The library cannot be read, or the thread cannot start.
    pub fn start(
        lib: &Library,
        service: Box<dyn CoverService>,
        setting: Arc<AtomicBool>,
    ) -> Result<Self, String> {
        Self::start_with(lib, service, setting, Timing::default())
    }

    /// [`CoverWorker::start`] with other waits, for tests.
    ///
    /// # Errors
    ///
    /// As [`CoverWorker::start`].
    pub fn start_with(
        lib: &Library,
        service: Box<dyn CoverService>,
        setting: Arc<AtomicBool>,
        timing: Timing,
    ) -> Result<Self, String> {
        let reader = lib.reader().map_err(|e| e.to_string())?;
        let (requests, incoming) = std::sync::mpsc::sync_channel(QUEUE);
        let (events_tx, events) = std::sync::mpsc::channel();
        let (ended_tx, ended) = std::sync::mpsc::channel::<()>();
        let stop = Arc::new(AtomicBool::new(false));
        let removals = Removals::default();
        let mut lookups = Lookups {
            removals: Arc::clone(&removals),
            reader,
            writer: lib.writer(),
            db: lib.path().to_owned(),
            service,
            enabled: Arc::clone(&setting),
            stop: Arc::clone(&stop),
            requests: incoming,
            events: events_tx,
            timing,
            later: HashMap::new(),
            paused_until: None,
        };
        std::thread::Builder::new()
            .name("cover lookups".to_owned())
            .spawn(move || {
                let _ended = ended_tx;
                lookups.run();
            })
            .map_err(|e| e.to_string())?;
        Ok(Self {
            enabled: setting,
            stop,
            requests,
            events,
            ended,
            removals,
        })
    }

    /// Turns the lookups on or off (`library.cover_lookup`). Off, no request is made after
    /// the one in flight, if any.
    pub fn set_enabled(&self, on: bool) {
        if self.enabled.swap(on, Ordering::Relaxed) != on {
            self.wake();
        }
    }

    /// Looks `track` up now, for another cover than the one it has (or after a miss, or a
    /// removal). Never waits.
    ///
    /// # Errors
    ///
    /// Lookups are off, or too many are waiting.
    pub fn find_now(&self, track: i64) -> Result<(), String> {
        if !self.enabled.load(Ordering::Relaxed) {
            return Err(
                "Cover lookups are off: turn library.cover_lookup on in Settings \u{2192} Library"
                    .to_owned(),
            );
        }
        let removed = removals_of(&self.removals, track);
        match self.requests.try_send(Request::Find(track, removed)) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => {
                Err("Too many cover lookups are waiting: try again in a moment".to_owned())
            }
            Err(TrySendError::Disconnected(_)) => Err("Cover lookups stopped".to_owned()),
        }
    }

    /// Asks the thread to look at the library again now (after "Retry missing covers").
    /// Never waits.
    pub fn wake(&self) {
        // A full queue wakes the thread anyway.
        let _ = self.requests.try_send(Request::Wake);
    }

    /// Says that `track`'s cover was just removed: a "Find another cover" asked for it
    /// before is dropped instead of bringing the cover back.
    pub fn removed(&self, track: i64) {
        *self
            .removals
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(track)
            .or_default() += 1;
    }

    /// What the lookups asked for have to say since the last call.
    pub fn take_events(&self) -> Vec<String> {
        self.events.try_iter().collect()
    }
}

impl Drop for CoverWorker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.wake();
        // Bounded: a request in flight ends on its own, and its answer is dropped.
        let _ = self.ended.recv_timeout(STOP_WAIT);
    }
}

/// The lookups' thread state. Its writes wait to be committed, so the next batch read
/// from the library already leaves out the tracks just looked up.
struct Lookups {
    reader: Reader,
    writer: Writer,
    /// The library's database file: the covers folder is next to it.
    db: PathBuf,
    service: Box<dyn CoverService>,
    enabled: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    requests: Receiver<Request>,
    events: Sender<String>,
    timing: Timing,
    /// The tracks whose file could not be read or whose lookup failed on the network, and
    /// when they may be looked up again.
    later: HashMap<i64, Instant>,
    /// After a network failure: no request before this.
    paused_until: Option<Instant>,
    removals: Removals,
}

/// One walk through the tracks needing a cover, by id.
#[derive(Default)]
struct Pass {
    /// The last track id looked at.
    after: Option<i64>,
    /// The missed tracks are retried in this pass (the retry flag was set when it began).
    retrying: bool,
    /// How many lookups the pass made.
    looked_up: usize,
}

/// What a track's file says before any request.
enum Picture {
    /// It has a picture of its own: nothing is sent.
    Own,
    /// It has none: it may be looked up.
    None,
    /// It cannot be read (its drive unplugged): nothing is sent, nothing recorded.
    Unknown,
    /// The track is missing (its file went to the Trash since it was read): nothing.
    Gone,
}

/// How a lookup ended.
enum Outcome {
    Found(Vec<u8>),
    Nothing,
    Failed(LookupError),
}

impl Lookups {
    fn run(&mut self) {
        self.sweep_parts();
        let mut pass = self.new_pass();
        while !self.stopped() {
            if !self.enabled.load(Ordering::Relaxed) {
                self.wait(self.timing.idle_poll);
                continue;
            }
            if let Some(until) = self.paused_until {
                let now = Instant::now();
                if until > now {
                    self.wait(until - now);
                    continue;
                }
                self.paused_until = None;
            }
            let batch = match self.reader.tracks_needing_covers_after(
                pass.after.unwrap_or(i64::MIN),
                BATCH,
                pass.retrying,
            ) {
                Ok(batch) => batch,
                Err(_) => {
                    // The library is busy or gone: try again later.
                    self.wait(self.timing.idle_poll);
                    continue;
                }
            };
            if batch.is_empty() {
                self.end_pass(&pass);
                if pass.looked_up == 0 {
                    self.wait(self.timing.idle_poll);
                }
                pass = self.new_pass();
                continue;
            }
            for query in batch {
                if self.stopped() || !self.enabled.load(Ordering::Relaxed) {
                    break;
                }
                self.answer_requests();
                let before = pass.after;
                pass.after = Some(query.track);
                if self
                    .later
                    .get(&query.track)
                    .is_some_and(|at| *at > Instant::now())
                {
                    continue;
                }
                pass.looked_up += 1;
                if !self.look_up(&query) {
                    // A retry goes on from this track once the pause is over, not from the
                    // first missed track again.
                    if pass.retrying {
                        self.later.remove(&query.track);
                        pass.after = before;
                    }
                    break;
                }
                // Turned off midway: this track is looked up again when they are back on.
                if !self.go_on() {
                    pass.after = before;
                    break;
                }
            }
        }
    }

    fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    /// Removes the temporary files a save that never finished left in the covers folder.
    fn sweep_parts(&self) {
        let [file, _] = library::cover_files(&self.db, 0);
        let Some(folder) = file.parent() else { return };
        let Ok(entries) = std::fs::read_dir(folder) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.') && name.ends_with(".part") {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }

    /// A new walk through the library, which retries the missed tracks if the retry flag
    /// is set now; the tracks whose wait after a failure is over are forgotten.
    fn new_pass(&mut self) -> Pass {
        let now = Instant::now();
        self.later.retain(|_, at| *at > now);
        Pass {
            retrying: self
                .reader
                .setting(COVERS_RETRY_MISSED)
                .ok()
                .flatten()
                .as_deref()
                == Some("1"),
            ..Pass::default()
        }
    }

    /// A pass that retried the missed tracks clears the retry flag: they were all looked up
    /// again (a network failure holds the pass at its track until the pause is over).
    fn end_pass(&self, pass: &Pass) {
        if pass.retrying {
            let _ = self.writer.submit_wait(WriteOp::SetSetting {
                key: COVERS_RETRY_MISSED.to_owned(),
                value: Some("0".to_owned()),
            });
        }
    }

    /// Waits up to `wait`, answering the requests that come meanwhile; a wake or a stop
    /// ends it early.
    fn wait(&mut self, wait: Duration) {
        let deadline = Instant::now() + wait;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.requests.recv_timeout(left) {
                Ok(Request::Find(track, removed)) => self.find_another(track, removed),
                Ok(Request::Wake) => return,
                Err(RecvTimeoutError::Timeout) => return,
                Err(RecvTimeoutError::Disconnected) => {
                    self.stop.store(true, Ordering::Relaxed);
                    return;
                }
            }
            if self.stopped() {
                return;
            }
        }
    }

    /// Answers the "Find another cover" requests waiting, between two lookups.
    fn answer_requests(&mut self) {
        while let Ok(request) = self.requests.try_recv() {
            if let Request::Find(track, removed) = request {
                self.find_another(track, removed);
            }
            if self.stopped() {
                return;
            }
        }
    }

    /// Looks `query` up and records what was found, or the miss. Returns `false` after a
    /// network failure (lookups then pause).
    fn look_up(&mut self, query: &CoverQuery) -> bool {
        match self.own_picture(query.track) {
            // A file with a picture of its own shows it: nothing is sent, and the miss
            // keeps it from being checked again (a retry checks the file again, locally).
            Picture::Own => {
                let _ = self
                    .writer
                    .submit_wait(WriteOp::MissCover { track: query.track });
                return true;
            }
            Picture::None => {}
            // Nothing is known of the file: nothing is sent or recorded, and the file is
            // read again later (the drive may be back).
            Picture::Unknown => {
                let later = Instant::now() + self.timing.retry_later;
                self.later.insert(query.track, later);
                return true;
            }
            Picture::Gone => return true,
        }
        match self.ask(query, None) {
            Outcome::Found(jpeg) => {
                self.keep(query.track, &jpeg, None);
                true
            }
            Outcome::Nothing => {
                if !self.stopped() {
                    let _ = self
                        .writer
                        .submit_wait(WriteOp::MissCover { track: query.track });
                }
                true
            }
            Outcome::Failed(e) => self.failed(query.track, &e),
        }
    }

    /// What `track`'s file says of a picture of its own, read as the window reads it, and
    /// whether the track is still there (the library is read again here: the batch may be
    /// older than a delete).
    fn own_picture(&self, track: i64) -> Picture {
        match self.reader.track(track) {
            Ok(Some(details)) if details.row.missing => Picture::Gone,
            Ok(Some(details)) => match ui::cover::own_picture(&details.path) {
                Some(true) => Picture::Own,
                Some(false) => Picture::None,
                None => Picture::Unknown,
            },
            Ok(None) => Picture::Gone,
            Err(_) => Picture::Unknown,
        }
    }

    /// Whether the lookups may go on: on, and not stopping.
    fn go_on(&self) -> bool {
        self.enabled.load(Ordering::Relaxed) && !self.stopped()
    }

    /// "Find another cover" for `track`: a cover other than the one it has replaces it,
    /// even a removed one, unless the cover was removed again since it was asked for
    /// (`removed` is the track's removal count then); none is said, and a track without a
    /// cover records the miss. A file with a picture of its own, a file that cannot be read
    /// and a missing track are not looked up.
    fn find_another(&mut self, track: i64, removed: u64) {
        if !self.go_on() {
            return;
        }
        let Ok(Some(query)) = self.reader.cover_query(track) else {
            return;
        };
        let name = service::query_text(&query);
        let refused = match self.own_picture(track) {
            Picture::None => None,
            Picture::Own => Some("has a picture of its own"),
            Picture::Unknown => Some("cannot be read"),
            Picture::Gone => Some("is missing"),
        };
        if let Some(why) = refused {
            let _ = self
                .events
                .send(format!("{name} {why}: no cover was looked up"));
            return;
        }
        let current = self.reader.cover(track).ok().flatten();
        let found = current
            .as_ref()
            .filter(|row| row.state == CoverState::Found);
        let had = found
            .and_then(|row| row.file.as_deref())
            .and_then(|file| std::fs::read(file).ok());
        match self.ask(&query, had.as_deref()) {
            // Removed again while it was looked up: the removal stands.
            Outcome::Found(_) if removals_of(&self.removals, track) != removed => {}
            Outcome::Found(jpeg) => self.keep(track, &jpeg, Some(removed)),
            Outcome::Nothing if found.is_some() => {
                let _ = self.events.send(format!("No other cover found for {name}"));
            }
            Outcome::Nothing => {
                let _ = self.writer.submit_wait(WriteOp::MissCover { track });
                let _ = self.events.send(format!("No cover found for {name}"));
            }
            Outcome::Failed(LookupError::Cancelled) => {}
            Outcome::Failed(e) => {
                let _ = self.events.send(format!(
                    "Cover lookup for {name} failed: {e}; try again later"
                ));
                self.failed(track, &e);
            }
        }
    }

    /// Asks the service for a cover of `query` that decodes and is not `had` (the JPEG the
    /// track has now), shrunk.
    fn ask(&self, query: &CoverQuery, had: Option<&[u8]>) -> Outcome {
        let kept = RefCell::new(None);
        let accept = |bytes: &[u8]| match shrink(bytes) {
            Ok(jpeg) if had != Some(jpeg.as_slice()) => {
                *kept.borrow_mut() = Some(jpeg);
                true
            }
            _ => false,
        };
        let go_on = || self.go_on();
        match self.service.find_other(query, &accept, &go_on) {
            Ok(Some(_)) => kept.into_inner().map_or(Outcome::Nothing, Outcome::Found),
            // A service that failed for good (not on the network) has no cover for it:
            // asking again would change nothing.
            Ok(None) | Err(LookupError::Failed(_)) => Outcome::Nothing,
            Err(e) => Outcome::Failed(e),
        }
    }

    /// A lookup that could not reach the services records nothing: the track is looked up
    /// again later, and the lookups pause. Returns whether the failure was not the
    /// network's (a cancel).
    fn failed(&mut self, track: i64, e: &LookupError) -> bool {
        if *e == LookupError::Cancelled {
            return true;
        }
        let now = Instant::now();
        self.later.insert(track, now + self.timing.retry_later);
        if e.is_unreachable() {
            self.paused_until = Some(now + self.timing.offline_pause);
            return false;
        }
        true
    }

    /// Saves `jpeg` as `track`'s cover and records it. A new cover for a track that has
    /// one goes to its other file (see [`library::cover_files`]). `removed`, for the user's
    /// own request, replaces even a removed cover, unless the track's removal count is no
    /// longer `removed` when it is recorded (it was removed again meanwhile). Nothing is
    /// kept once the lookups were turned off or are stopping.
    fn keep(&self, track: i64, jpeg: &[u8], removed: Option<u64>) {
        if !self.go_on() {
            return;
        }
        let [first, second] = library::cover_files(&self.db, track);
        let in_use = self
            .reader
            .cover(track)
            .ok()
            .flatten()
            .and_then(|row| row.file);
        let file = if in_use.as_ref() == Some(&first) {
            second
        } else {
            first
        };
        match save(&file, jpeg) {
            // Removed again while it was saved: the removal stands, and the file it was
            // saved to (not the track's cover) goes.
            Ok(()) if removed.is_some_and(|r| removals_of(&self.removals, track) != r) => {
                let _ = std::fs::remove_file(&file);
            }
            Ok(()) => {
                let force = removed.is_some();
                let _ = self
                    .writer
                    .submit_wait(WriteOp::SetCover { track, file, force });
            }
            Err(e) => {
                let _ = self.events.send(format!(
                    "could not save a cover in {}: {e}",
                    crate::format::safe(file.parent().unwrap_or(&file).display())
                ));
            }
        }
    }
}

/// `bytes`, a JPEG or PNG from a service, as a JPEG of at most [`COVER_SIDE`] a side. The
/// picture is untrusted: one wider or taller than [`MAX_DECODED_SIDE`], or needing more
/// than [`MAX_DECODE_ALLOC`] to decode, is refused before it is decoded.
///
/// # Errors
///
/// Not a JPEG or PNG, too large, or broken.
pub fn shrink(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let format = image::guess_format(bytes).map_err(|e| e.to_string())?;
    if !matches!(format, image::ImageFormat::Jpeg | image::ImageFormat::Png) {
        return Err(format!("{format:?} pictures are not read"));
    }
    let mut reader = image::ImageReader::with_format(Cursor::new(bytes), format);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_DECODED_SIDE);
    limits.max_image_height = Some(MAX_DECODED_SIDE);
    limits.max_alloc = Some(MAX_DECODE_ALLOC);
    reader.limits(limits);
    let mut image = reader.decode().map_err(|e| e.to_string())?;
    if image.width() > COVER_SIDE || image.height() > COVER_SIDE {
        image = image.resize(
            COVER_SIDE,
            COVER_SIDE,
            image::imageops::FilterType::Triangle,
        );
    }
    let rgb = image.to_rgb8();
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, JPEG_QUALITY)
        .encode_image(&rgb)
        .map_err(|e| e.to_string())?;
    Ok(jpeg)
}

/// Writes `jpeg` to `file` whole or not at all: to a temporary file next to it, then
/// renamed over it.
fn save(file: &Path, jpeg: &[u8]) -> std::io::Result<()> {
    let folder = file.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(folder)?;
    let name = file
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let temporary = folder.join(format!(".{name}.{}.part", std::process::id()));
    let written = std::fs::File::create(&temporary).and_then(|mut out| {
        out.write_all(jpeg)?;
        out.sync_all()
    });
    let renamed = written.and_then(|()| std::fs::rename(&temporary, file));
    if renamed.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    renamed
}

#[cfg(test)]
pub(crate) mod tests {
    use std::io::Cursor;
    use std::path::PathBuf;
    use std::sync::Mutex;
    use std::sync::atomic::Ordering;
    use std::time::Instant;

    use library::{COVERS_RETRY_MISSED, CoverQuery, CoverState, RawStatement, WriteOp};
    use rusqlite::types::Value;

    use super::*;

    /// What a [`FakeService`] answers.
    #[derive(Clone)]
    pub(crate) enum Answer {
        /// These pictures, the first one accepted.
        Pictures(Vec<Vec<u8>>),
        Nothing,
        Fail(LookupError),
    }

    /// What the test sees of a [`FakeService`].
    pub(crate) struct FakeState {
        pub answer: Mutex<Answer>,
        pub delay: Mutex<Duration>,
        pub asked: Mutex<Vec<CoverQuery>>,
    }

    impl FakeState {
        pub(crate) fn calls(&self) -> usize {
            self.asked.lock().unwrap().len()
        }

        pub(crate) fn answer(&self, answer: Answer) {
            *self.answer.lock().unwrap() = answer;
        }
    }

    /// A cover service that never reaches the network: it records the queries and gives
    /// its answer, after its delay.
    pub(crate) struct FakeService(pub Arc<FakeState>);

    impl FakeService {
        pub(crate) fn boxed(
            answer: Answer,
            delay: Duration,
        ) -> (Box<dyn CoverService>, Arc<FakeState>) {
            let state = Arc::new(FakeState {
                answer: Mutex::new(answer),
                delay: Mutex::new(delay),
                asked: Mutex::default(),
            });
            (Box::new(Self(Arc::clone(&state))), state)
        }
    }

    impl CoverService for FakeService {
        fn find_other(
            &self,
            query: &CoverQuery,
            accept: &dyn Fn(&[u8]) -> bool,
            _go_on: &dyn Fn() -> bool,
        ) -> Result<Option<Vec<u8>>, LookupError> {
            self.0.asked.lock().unwrap().push(query.clone());
            let delay = *self.0.delay.lock().unwrap();
            std::thread::sleep(delay);
            match self.0.answer.lock().unwrap().clone() {
                Answer::Pictures(pictures) => {
                    Ok(pictures.into_iter().find(|picture| accept(picture)))
                }
                Answer::Nothing => Ok(None),
                Answer::Fail(e) => Err(e),
            }
        }
    }

    pub(crate) fn png(side: u32, colour: [u8; 3]) -> Vec<u8> {
        png_sized(side, side, colour)
    }

    fn png_sized(width: u32, height: u32, colour: [u8; 3]) -> Vec<u8> {
        let mut bytes = Vec::new();
        image::RgbImage::from_pixel(width, height, image::Rgb(colour))
            .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
            .unwrap();
        bytes
    }

    fn db(dir: &tempfile::TempDir) -> PathBuf {
        dir.path().join("lib").join("library.sqlite3")
    }

    fn open(dir: &tempfile::TempDir) -> Library {
        Library::open(&db(dir)).unwrap()
    }

    pub(crate) fn add_track(lib: &Library, id: i64, tags: Option<(&str, &str)>, path: &str) {
        let text = |s: Option<&str>| s.map_or(Value::Null, |s| Value::Text(s.to_owned()));
        lib.writer()
            .submit_wait(WriteOp::Raw(vec![RawStatement {
                sql: "INSERT INTO tracks(id, path, fingerprint, artist, title, added_at) \
                      VALUES (?1, ?2, ?3, ?4, ?5, 0)"
                    .into(),
                params: vec![
                    Value::Integer(id),
                    Value::Text(path.to_owned()),
                    Value::Blob(vec![0; 32]),
                    text(tags.map(|t| t.0)),
                    text(tags.map(|t| t.1)),
                ],
            }]))
            .unwrap();
    }

    /// An empty file `name` in `dir` (a file with no picture of its own), as a path for
    /// [`add_track`]: a track whose file cannot be read is never looked up.
    pub(crate) fn song(dir: &tempfile::TempDir, name: &str) -> String {
        let path = dir.path().join("music").join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"").unwrap();
        path.to_str().unwrap().to_owned()
    }

    pub(crate) fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !done() {
            assert!(Instant::now() < deadline, "timed out waiting: {what}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn state(lib: &Library, track: i64) -> Option<CoverState> {
        lib.reader()
            .unwrap()
            .cover(track)
            .unwrap()
            .map(|row| row.state)
    }

    fn on() -> Arc<AtomicBool> {
        Arc::new(AtomicBool::new(true))
    }

    fn fast() -> Timing {
        Timing {
            offline_pause: Duration::from_secs(2),
            idle_poll: Duration::from_millis(50),
            retry_later: Duration::from_secs(600),
        }
    }

    /// Times `f`, which must return at once.
    fn at_once<T>(what: &str, f: impl FnOnce() -> T) -> T {
        let began = Instant::now();
        let out = f();
        assert!(
            began.elapsed() < Duration::from_millis(100),
            "{what} took {:?}",
            began.elapsed()
        );
        out
    }

    #[test]
    fn a_found_cover_is_saved_resized_and_recorded() {
        let dir = tempfile::tempdir().unwrap();
        let lib = open(&dir);
        add_track(
            &lib,
            1,
            Some(("Daft Punk", "Da Funk")),
            &song(&dir, "da funk.mp3"),
        );
        let (service, fake) = FakeService::boxed(
            Answer::Pictures(vec![png(2000, [200, 0, 0])]),
            Duration::ZERO,
        );
        let _worker = CoverWorker::start_with(&lib, service, on(), fast()).unwrap();
        wait_until("the cover", || state(&lib, 1) == Some(CoverState::Found));
        let row = lib.reader().unwrap().cover(1).unwrap().unwrap();
        let [file, _] = library::cover_files(&db(&dir), 1);
        assert_eq!(row.file, Some(file.clone()));
        let saved = std::fs::read(&file).unwrap();
        assert_eq!(
            image::guess_format(&saved).unwrap(),
            image::ImageFormat::Jpeg
        );
        let image = image::load_from_memory(&saved).unwrap();
        assert_eq!((image.width(), image.height()), (512, 512));
        // Saved through a temporary file renamed in place: nothing else is left.
        let names: Vec<_> = std::fs::read_dir(file.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, ["1.jpg"]);
        assert_eq!(fake.calls(), 1);
    }

    #[test]
    fn the_service_is_never_given_a_path() {
        let dir = tempfile::tempdir().unwrap();
        let lib = open(&dir);
        add_track(
            &lib,
            1,
            Some(("Artist", "Title")),
            &song(&dir, "Private Folder/my song.flac"),
        );
        add_track(
            &lib,
            2,
            None,
            &song(&dir, "Private Folder/Untagged One.wav"),
        );
        let (service, fake) = FakeService::boxed(Answer::Nothing, Duration::ZERO);
        let _worker = CoverWorker::start_with(&lib, service, on(), fast()).unwrap();
        wait_until("two lookups", || fake.calls() == 2);
        let asked = fake.asked.lock().unwrap().clone();
        assert_eq!(service::query_text(&asked[0]), "Artist Title");
        assert_eq!(service::query_text(&asked[1]), "Untagged One");
        for query in asked {
            let seen = format!("{query:?}");
            assert!(!seen.contains("Private") && !seen.contains('/'), "{seen}");
        }
    }

    #[test]
    fn a_clean_no_result_records_a_miss() {
        let dir = tempfile::tempdir().unwrap();
        let lib = open(&dir);
        add_track(&lib, 1, Some(("A", "T")), &song(&dir, "a.mp3"));
        let (service, fake) = FakeService::boxed(Answer::Nothing, Duration::ZERO);
        let _worker = CoverWorker::start_with(&lib, service, on(), fast()).unwrap();
        wait_until("the miss", || state(&lib, 1) == Some(CoverState::Missed));
        // Not looked up again.
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(fake.calls(), 1);
    }

    #[test]
    fn an_unreachable_service_records_nothing_and_never_blocks() {
        let dir = tempfile::tempdir().unwrap();
        let lib = open(&dir);
        add_track(&lib, 1, Some(("A", "T")), &song(&dir, "a.mp3"));
        add_track(&lib, 2, Some(("B", "U")), &song(&dir, "b.mp3"));
        let offline = Answer::Fail(LookupError::Unreachable("offline".into()));
        let (service, fake) = FakeService::boxed(offline, Duration::from_millis(500));
        let worker = CoverWorker::start_with(&lib, service, on(), fast()).unwrap();
        wait_until("the first lookup", || fake.calls() == 1);
        // While it waits on the service, nothing asked of the worker waits.
        at_once("find", || worker.find_now(2)).unwrap();
        at_once("wake", || worker.wake());
        at_once("switch", || worker.set_enabled(true));
        at_once("events", || worker.take_events());
        std::thread::sleep(Duration::from_millis(700));
        // Nothing recorded: the track is looked up again later. And the lookups pause.
        assert_eq!(state(&lib, 1), None);
        let calls = fake.calls();
        std::thread::sleep(Duration::from_millis(600));
        assert_eq!(fake.calls(), calls, "no pause after a network failure");
        assert!(calls <= 2);
        // Stopping does not wait for a request to end.
        *fake.delay.lock().unwrap() = Duration::from_secs(5);
        worker.wake();
        std::thread::sleep(Duration::from_millis(1700));
        let began = Instant::now();
        drop(worker);
        assert!(began.elapsed() < Duration::from_millis(1500));
    }

    #[test]
    fn turning_the_setting_off_stops_lookups() {
        let dir = tempfile::tempdir().unwrap();
        let lib = open(&dir);
        for id in 1..=10 {
            add_track(
                &lib,
                id,
                Some(("A", "T")),
                &song(&dir, &format!("{id}.mp3")),
            );
        }
        let setting = on();
        let (service, fake) = FakeService::boxed(Answer::Nothing, Duration::from_millis(100));
        let worker = CoverWorker::start_with(&lib, service, Arc::clone(&setting), fast()).unwrap();
        wait_until("a lookup", || fake.calls() >= 1);
        worker.set_enabled(false);
        assert!(!setting.load(Ordering::Relaxed));
        std::thread::sleep(Duration::from_millis(300));
        let calls = fake.calls();
        assert!(calls <= 3, "{calls}");
        std::thread::sleep(Duration::from_millis(400));
        assert_eq!(fake.calls(), calls);
        // Off, a track is not looked up even when asked for.
        assert!(worker.find_now(10).is_err());
        worker.set_enabled(true);
        wait_until("lookups again", || fake.calls() > calls);
    }

    #[test]
    fn find_another_cover_retries_a_miss() {
        let dir = tempfile::tempdir().unwrap();
        let lib = open(&dir);
        add_track(&lib, 1, Some(("A", "T")), &song(&dir, "a.mp3"));
        lib.writer()
            .submit_wait(WriteOp::MissCover { track: 1 })
            .unwrap();
        let red = png(600, [255, 0, 0]);
        let blue = png(600, [0, 0, 255]);
        let (service, fake) =
            FakeService::boxed(Answer::Pictures(vec![red.clone()]), Duration::ZERO);
        let worker = CoverWorker::start_with(&lib, service, on(), fast()).unwrap();
        // A miss is not looked up again on its own.
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(fake.calls(), 0);
        worker.find_now(1).unwrap();
        wait_until("the cover", || state(&lib, 1) == Some(CoverState::Found));
        let [first, second] = library::cover_files(&db(&dir), 1);
        assert!(first.exists());
        // Another one: the same picture is passed over, and the new one is kept in the
        // other file, so the window reads it again.
        fake.answer(Answer::Pictures(vec![red, blue]));
        worker.find_now(1).unwrap();
        wait_until("the other cover", || {
            lib.reader().unwrap().cover(1).unwrap().unwrap().file == Some(second.clone())
        });
        wait_until("the old file deleted", || !first.exists());
        let image = image::open(&second).unwrap().to_rgb8();
        let pixel = image.get_pixel(10, 10).0;
        assert!(pixel[2] > 200 && pixel[0] < 50, "{pixel:?}");
        // No other one: the cover stays, and the window is told.
        fake.answer(Answer::Pictures(vec![]));
        worker.find_now(1).unwrap();
        let mut events = Vec::new();
        wait_until("the event", || {
            events.extend(worker.take_events());
            !events.is_empty()
        });
        assert!(events[0].contains("No other cover"), "{events:?}");
        assert!(second.exists());
        assert_eq!(state(&lib, 1), Some(CoverState::Found));
    }

    #[test]
    fn retry_missing_covers_looks_them_up_once() {
        let dir = tempfile::tempdir().unwrap();
        let lib = open(&dir);
        add_track(&lib, 1, Some(("A", "T")), &song(&dir, "a.mp3"));
        lib.writer()
            .submit_wait(WriteOp::MissCover { track: 1 })
            .unwrap();
        let (service, fake) = FakeService::boxed(Answer::Nothing, Duration::ZERO);
        let worker = CoverWorker::start_with(&lib, service, on(), fast()).unwrap();
        lib.writer()
            .submit_wait(WriteOp::SetSetting {
                key: COVERS_RETRY_MISSED.into(),
                value: Some("1".into()),
            })
            .unwrap();
        worker.wake();
        wait_until("the retry", || fake.calls() == 1);
        let setting = || lib.reader().unwrap().setting(COVERS_RETRY_MISSED).unwrap();
        wait_until("the flag reset", || setting().as_deref() == Some("0"));
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(fake.calls(), 1);
        assert_eq!(state(&lib, 1), Some(CoverState::Missed));
    }

    #[test]
    fn remove_cover_shows_the_tile_again() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("library.sqlite3");
        let mode = crate::library_mode::testing::start(&path);
        crate::library_mode::testing::add_track_7(&mode);
        let lib = mode.library();
        // Its file, readable and without a picture.
        lib.writer()
            .submit_wait(WriteOp::Raw(vec![RawStatement {
                sql: "UPDATE tracks SET path = ?1 WHERE id = 7".into(),
                params: vec![Value::Text(song(&dir, "7.wav"))],
            }]))
            .unwrap();
        let (service, fake) = FakeService::boxed(
            Answer::Pictures(vec![png(100, [0, 255, 0])]),
            Duration::ZERO,
        );
        let worker = CoverWorker::start_with(&lib, service, on(), fast()).unwrap();
        let [file, _] = library::cover_files(&path, 7);
        wait_until("the cover shown", || {
            mode.cover_file(7) == Some(file.clone())
        });
        mode.remove_cover(7).unwrap();
        wait_until("the tile again", || mode.cover_file(7).is_none());
        assert!(!file.exists());
        assert_eq!(state(&lib, 7), Some(CoverState::Removed));
        // Never looked up again, until asked.
        worker.wake();
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(fake.calls(), 1);
        worker.find_now(7).unwrap();
        wait_until("found again", || mode.cover_file(7).is_some());
    }

    /// A short silent WAV at `path`, with `picture` as its ID3 front cover when given.
    fn wav(path: &std::path::Path, picture: Option<Vec<u8>>) {
        use lofty::config::{ParseOptions, WriteOptions};
        use lofty::file::AudioFile;
        use lofty::id3::v2::{AttachedPictureFrame, Frame, Id3v2Tag};
        use lofty::iff::wav::WavFile;
        use lofty::picture::{MimeType, Picture, PictureType};
        let frames = 441u32;
        let mut bytes = b"RIFF".to_vec();
        bytes.extend_from_slice(&(36 + frames * 2).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&44_100u32.to_le_bytes());
        bytes.extend_from_slice(&88_200u32.to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&(frames * 2).to_le_bytes());
        bytes.resize(bytes.len() + (frames * 2) as usize, 0);
        std::fs::write(path, bytes).unwrap();
        let Some(png) = picture else { return };
        let mut tag = Id3v2Tag::new();
        tag.insert(Frame::Picture(AttachedPictureFrame::new(
            lofty::TextEncoding::Latin1,
            Picture::unchecked(png)
                .pic_type(PictureType::CoverFront)
                .mime_type(MimeType::Png)
                .build(),
        )));
        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .unwrap();
        let mut wav = WavFile::read_from(&mut file, ParseOptions::new()).unwrap();
        wav.set_id3v2(tag);
        wav.save_to(&mut file, WriteOptions::default()).unwrap();
    }

    #[test]
    fn a_track_with_its_own_picture_is_never_sent() {
        use service::tests::{RecordingHttp, fixture};
        use service::{Chain, DeezerService, Http, MusicBrainzService};
        let dir = tempfile::tempdir().unwrap();
        let lib = open(&dir);
        let own = dir.path().join("own.wav");
        let plain = dir.path().join("plain.wav");
        wav(&own, Some(png(16, [9, 9, 9])));
        wav(&plain, None);
        add_track(&lib, 1, Some(("Pictured", "Owner")), own.to_str().unwrap());
        add_track(&lib, 2, Some(("Bare", "Track")), plain.to_str().unwrap());
        let http = Arc::new(
            RecordingHttp::default()
                .answer("https://api.deezer.com/", Ok(fixture("deezer_empty.json")))
                .answer(
                    "https://musicbrainz.org/",
                    Ok(fixture("musicbrainz_empty.json")),
                ),
        );
        let shared = Arc::clone(&http) as Arc<dyn Http>;
        let chain = Chain(vec![
            Box::new(DeezerService::new(Arc::clone(&shared), Duration::ZERO)),
            Box::new(MusicBrainzService::new(shared, Duration::ZERO)),
        ]);
        let worker = CoverWorker::start_with(&lib, Box::new(chain), on(), fast()).unwrap();
        wait_until("both checked", || {
            state(&lib, 1) == Some(CoverState::Missed) && state(&lib, 2) == Some(CoverState::Missed)
        });
        // The pictured track was checked on disk only; the other one was looked up.
        let asked = http.asked();
        assert_eq!(asked.len(), 2, "{asked:?}");
        assert!(
            asked
                .iter()
                .all(|url| !url.contains("Pictured") && !url.contains("Owner"))
        );
        // Asked for by hand, it still sends nothing.
        worker.find_now(1).unwrap();
        let mut events = Vec::new();
        wait_until("the event", || {
            events.extend(worker.take_events());
            !events.is_empty()
        });
        assert!(events[0].contains("picture of its own"), "{events:?}");
        assert_eq!(http.asked().len(), 2);
    }

    #[test]
    fn a_service_that_fails_for_good_is_a_miss() {
        let dir = tempfile::tempdir().unwrap();
        let lib = open(&dir);
        add_track(&lib, 1, Some(("A", "T")), &song(&dir, "a.mp3"));
        let broken = Answer::Fail(LookupError::Failed("Deezer: invalid query".into()));
        let (service, fake) = FakeService::boxed(broken, Duration::ZERO);
        let timing = Timing {
            retry_later: Duration::from_millis(100),
            ..fast()
        };
        let _worker = CoverWorker::start_with(&lib, service, on(), timing).unwrap();
        wait_until("the miss", || state(&lib, 1) == Some(CoverState::Missed));
        // Recorded once, and not asked again, even past the wait after a failure.
        std::thread::sleep(Duration::from_millis(400));
        assert_eq!(fake.calls(), 1);
    }

    #[test]
    fn a_file_that_cannot_be_read_sends_nothing() {
        use service::tests::RecordingHttp;
        use service::{Chain, DeezerService, Http, MusicBrainzService};
        let dir = tempfile::tempdir().unwrap();
        let lib = open(&dir);
        // On a drive that is not plugged in.
        let away = dir.path().join("Unplugged/song.mp3");
        add_track(&lib, 1, Some(("Away", "Song")), away.to_str().unwrap());
        let http = Arc::new(RecordingHttp::default());
        let shared = Arc::clone(&http) as Arc<dyn Http>;
        let chain = Chain(vec![
            Box::new(DeezerService::new(Arc::clone(&shared), Duration::ZERO)),
            Box::new(MusicBrainzService::new(shared, Duration::ZERO)),
        ]);
        let worker = CoverWorker::start_with(&lib, Box::new(chain), on(), fast()).unwrap();
        std::thread::sleep(Duration::from_millis(300));
        assert!(http.asked().is_empty());
        assert_eq!(state(&lib, 1), None);
        // Asked for by hand, the same, and the events say why.
        worker.find_now(1).unwrap();
        let mut events = Vec::new();
        wait_until("the event", || {
            events.extend(worker.take_events());
            !events.is_empty()
        });
        assert_eq!(events, ["Away Song cannot be read: no cover was looked up"]);
        assert!(http.asked().is_empty());
        assert_eq!(state(&lib, 1), None);
    }

    #[test]
    fn a_missing_track_is_not_looked_up() {
        let dir = tempfile::tempdir().unwrap();
        let lib = open(&dir);
        add_track(&lib, 1, Some(("A", "T")), &song(&dir, "a.mp3"));
        lib.writer()
            .submit_wait(WriteOp::MissCover { track: 1 })
            .unwrap();
        lib.writer()
            .submit_wait(WriteOp::MarkMissing(vec![1]))
            .unwrap();
        let (service, fake) = FakeService::boxed(Answer::Nothing, Duration::ZERO);
        let worker = CoverWorker::start_with(&lib, service, on(), fast()).unwrap();
        worker.find_now(1).unwrap();
        let mut events = Vec::new();
        wait_until("the event", || {
            events.extend(worker.take_events());
            !events.is_empty()
        });
        assert_eq!(events, ["A T is missing: no cover was looked up"]);
        assert_eq!(fake.calls(), 0);
    }

    /// A service with no cover for anything, unreachable the first time it is asked about
    /// the title `flaky`.
    struct FlakyOnce {
        flaky: &'static str,
        failed: AtomicBool,
        asked: Arc<Mutex<Vec<String>>>,
    }

    impl CoverService for FlakyOnce {
        fn find_other(
            &self,
            query: &CoverQuery,
            _accept: &dyn Fn(&[u8]) -> bool,
            _go_on: &dyn Fn() -> bool,
        ) -> Result<Option<Vec<u8>>, LookupError> {
            let title = query.title.clone().unwrap_or_default();
            self.asked.lock().unwrap().push(title.clone());
            if title == self.flaky && !self.failed.swap(true, Ordering::Relaxed) {
                return Err(LookupError::Unreachable("offline".into()));
            }
            Ok(None)
        }
    }

    #[test]
    fn a_retry_goes_on_from_where_the_network_failed() {
        let dir = tempfile::tempdir().unwrap();
        let lib = open(&dir);
        for id in 1..=4 {
            let title = id.to_string();
            add_track(
                &lib,
                id,
                Some(("A", &title)),
                &song(&dir, &format!("{id}.mp3")),
            );
            lib.writer()
                .submit_wait(WriteOp::MissCover { track: id })
                .unwrap();
        }
        lib.writer()
            .submit_wait(WriteOp::SetSetting {
                key: COVERS_RETRY_MISSED.to_owned(),
                value: Some("1".to_owned()),
            })
            .unwrap();
        let asked = Arc::new(Mutex::new(Vec::new()));
        let service = FlakyOnce {
            flaky: "3",
            failed: AtomicBool::new(false),
            asked: Arc::clone(&asked),
        };
        let timing = Timing {
            offline_pause: Duration::from_millis(200),
            ..fast()
        };
        let _worker = CoverWorker::start_with(&lib, Box::new(service), on(), timing).unwrap();
        let reader = lib.reader().unwrap();
        wait_until("the retry done", || {
            reader.setting(COVERS_RETRY_MISSED).unwrap().as_deref() == Some("0")
        });
        std::thread::sleep(Duration::from_millis(300));
        // Track 3 again after the pause, then 4: the tracks before it once each.
        assert_eq!(*asked.lock().unwrap(), ["1", "2", "3", "3", "4"]);
    }

    #[test]
    fn a_cover_in_flight_is_dropped_once_lookups_are_off() {
        let dir = tempfile::tempdir().unwrap();
        let lib = open(&dir);
        add_track(&lib, 1, Some(("A", "T")), &song(&dir, "a.mp3"));
        let setting = on();
        let (service, fake) = FakeService::boxed(
            Answer::Pictures(vec![png(64, [0, 200, 0])]),
            Duration::from_millis(400),
        );
        let worker = CoverWorker::start_with(&lib, service, Arc::clone(&setting), fast()).unwrap();
        wait_until("the lookup under way", || fake.calls() == 1);
        worker.set_enabled(false);
        std::thread::sleep(Duration::from_millis(700));
        assert_eq!(state(&lib, 1), None);
        let [first, second] = library::cover_files(&db(&dir), 1);
        assert!(!first.exists() && !second.exists());
    }

    #[test]
    fn a_removal_just_before_a_found_cover_is_recorded_stands() {
        let dir = tempfile::tempdir().unwrap();
        let lib = open(&dir);
        add_track(&lib, 1, Some(("A", "T")), &song(&dir, "a.mp3"));
        let (service, _fake) = FakeService::boxed(Answer::Nothing, Duration::ZERO);
        let (_requests, incoming) = std::sync::mpsc::sync_channel(1);
        let (events, _events) = std::sync::mpsc::channel();
        let lookups = Lookups {
            reader: lib.reader().unwrap(),
            writer: lib.writer(),
            db: db(&dir),
            service,
            enabled: on(),
            stop: Arc::new(AtomicBool::new(false)),
            requests: incoming,
            events,
            timing: fast(),
            later: HashMap::new(),
            paused_until: None,
            removals: Removals::default(),
        };
        // "Find another cover" asked with no removal yet; a removal lands as the picture
        // is saved, before it is recorded.
        lookups.removals.lock().unwrap().insert(1, 1);
        let jpeg = shrink(&png(64, [0, 0, 200])).unwrap();
        lookups.keep(1, &jpeg, Some(0));
        assert_eq!(state(&lib, 1), None);
        let [first, second] = library::cover_files(&db(&dir), 1);
        assert!(!first.exists() && !second.exists());
        // Without the removal, it is kept.
        lookups.keep(1, &jpeg, Some(1));
        assert_eq!(state(&lib, 1), Some(CoverState::Found));
    }

    #[test]
    fn a_removal_after_find_another_stands() {
        let dir = tempfile::tempdir().unwrap();
        let lib = open(&dir);
        add_track(&lib, 1, Some(("A", "T")), &song(&dir, "a.mp3"));
        lib.writer()
            .submit_wait(WriteOp::MissCover { track: 1 })
            .unwrap();
        let (service, fake) = FakeService::boxed(
            Answer::Pictures(vec![png(64, [0, 0, 255])]),
            Duration::from_millis(400),
        );
        let worker = CoverWorker::start_with(&lib, service, on(), fast()).unwrap();
        worker.find_now(1).unwrap();
        wait_until("the lookup under way", || fake.calls() == 1);
        // Removed while the find is under way, as the session does it.
        worker.removed(1);
        lib.writer()
            .submit_wait(WriteOp::ClearCover { track: 1 })
            .unwrap();
        std::thread::sleep(Duration::from_millis(800));
        assert_eq!(state(&lib, 1), Some(CoverState::Removed));
        let [first, second] = library::cover_files(&db(&dir), 1);
        assert!(!first.exists() && !second.exists());
        // A find asked after the removal is written.
        worker.find_now(1).unwrap();
        wait_until("found", || state(&lib, 1) == Some(CoverState::Found));
    }

    #[test]
    fn unfinished_saves_are_swept_at_start() {
        let dir = tempfile::tempdir().unwrap();
        let lib = open(&dir);
        let [file, _] = library::cover_files(&db(&dir), 1);
        let folder = file.parent().unwrap();
        std::fs::create_dir_all(folder).unwrap();
        let part = folder.join(".1.jpg.4242.part");
        std::fs::write(&part, b"half").unwrap();
        std::fs::write(folder.join("7.jpg"), b"kept").unwrap();
        let (service, _fake) = FakeService::boxed(Answer::Nothing, Duration::ZERO);
        let _worker = CoverWorker::start_with(&lib, service, on(), fast()).unwrap();
        wait_until("the part swept", || !part.exists());
        assert!(folder.join("7.jpg").exists());
    }

    #[test]
    fn untrusted_pictures_are_decoded_with_limits() {
        let jpeg = shrink(&png(2000, [1, 2, 3])).unwrap();
        let image = image::load_from_memory(&jpeg).unwrap();
        assert_eq!((image.width(), image.height()), (512, 512));
        // Small ones keep their size; wide ones keep their shape.
        let image = image::load_from_memory(&shrink(&png(300, [1, 2, 3])).unwrap()).unwrap();
        assert_eq!((image.width(), image.height()), (300, 300));
        let wide = shrink(&png_sized(1024, 256, [1, 2, 3])).unwrap();
        let image = image::load_from_memory(&wide).unwrap();
        assert_eq!((image.width(), image.height()), (512, 128));
        // Too large to decode safely, not a picture, empty.
        assert!(shrink(&png_sized(9000, 8, [1, 2, 3])).is_err());
        assert!(shrink(b"<html>not a picture</html>").is_err());
        assert!(shrink(&[]).is_err());
    }
}
