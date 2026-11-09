//! The waveforms and hotcue marks of the tracks on the decks, found off the control thread:
//! the stored overview and the hotcues' colours and names are read from the library and the
//! detailed waveform is computed from the decoded track, on a thread of their own.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread::JoinHandle;

use analysis::Waveform;
use engine::TrackBuffer;
use library::Library;
use ui::HotcueMark;

/// Frames per bucket of the detailed waveform.
const DETAIL_FRAMES_PER_BUCKET: u32 = 128;

/// A track to compute the waveforms of.
struct Job {
    deck: usize,
    load_seq: u64,
    track_id: i64,
    /// The decoded track; without it only the hotcue marks are read again.
    audio: Option<Arc<TrackBuffer>>,
}

/// A deck's hotcue marks, slot by slot.
type Marks = [Option<HotcueMark>; 8];

/// What the thread found for a deck's load.
struct Done {
    deck: usize,
    load_seq: u64,
    part: Part,
}

enum Part {
    /// The stored overview, if the library has one.
    Overview(Option<Arc<Waveform>>),
    Detail(Arc<Waveform>),
    /// The hotcues' colours and names, when they could be read.
    Marks(Option<Arc<Marks>>),
}

/// What is known of the waveforms of the load a deck is on.
#[derive(Default)]
struct Slot {
    load_seq: u64,
    overview: Option<Arc<Waveform>>,
    detail: Option<Arc<Waveform>>,
    marks: Arc<Marks>,
}

/// Which deck loads were asked for, and what came back for them.
#[derive(Default)]
struct Slots([Option<Slot>; 4]);

impl Slots {
    /// Starts `deck`'s slot for `load_seq`; false if it is on that load already.
    fn request(&mut self, deck: usize, load_seq: u64) -> bool {
        if self.0[deck]
            .as_ref()
            .is_some_and(|s| s.load_seq == load_seq)
        {
            return false;
        }
        self.0[deck] = Some(Slot {
            load_seq,
            ..Slot::default()
        });
        true
    }

    /// Forgets `deck`'s slot: nothing is loaded.
    fn clear(&mut self, deck: usize) {
        self.0[deck] = None;
    }

    /// Stores a result, unless the deck has moved on to another load since.
    fn receive(&mut self, done: Done) {
        let Some(slot) = self.0[done.deck]
            .as_mut()
            .filter(|s| s.load_seq == done.load_seq)
        else {
            return;
        };
        match done.part {
            Part::Overview(overview) => slot.overview = overview,
            Part::Detail(detail) => slot.detail = Some(detail),
            // Marks that could not be read leave the ones shown.
            Part::Marks(marks) => slot.marks = marks.unwrap_or_else(|| slot.marks.clone()),
        }
    }

    /// The hotcue marks of `deck`'s load `load_seq`; none until they are read.
    fn marks(&self, deck: usize, load_seq: u64) -> Arc<Marks> {
        match &self.0[deck] {
            Some(slot) if slot.load_seq == load_seq => Arc::clone(&slot.marks),
            _ => Arc::default(),
        }
    }

    /// The overview (the detailed waveform, if the library has none) and the detail of
    /// `deck`'s load `load_seq`.
    fn waveforms(
        &self,
        deck: usize,
        load_seq: u64,
    ) -> (Option<Arc<Waveform>>, Option<Arc<Waveform>>) {
        match &self.0[deck] {
            Some(slot) if slot.load_seq == load_seq => (
                slot.overview.clone().or_else(|| slot.detail.clone()),
                slot.detail.clone(),
            ),
            _ => (None, None),
        }
    }
}

/// What the thread checks before each step, so a load that is not wanted any more costs
/// nothing.
#[derive(Default)]
struct Wanted {
    stop: AtomicBool,
    /// Per deck, the load whose waveforms are wanted (0: none).
    load_seq: [AtomicU64; 4],
}

impl Wanted {
    fn wants(&self, job: &Job) -> bool {
        !self.stop.load(Ordering::Relaxed)
            && self.load_seq[job.deck].load(Ordering::Relaxed) == job.load_seq
    }
}

/// Computes the waveforms of each load on a deck in the background.
pub(super) struct DetailJobs {
    slots: Slots,
    wanted: Arc<Wanted>,
    jobs: Option<Sender<Job>>,
    done: Receiver<Done>,
    thread: Option<JoinHandle<()>>,
    /// Per deck, the library track whose load the slot holds.
    tracks: [i64; 4],
}

impl DetailJobs {
    pub fn new(library: Arc<Library>) -> Self {
        let marks_library = Arc::clone(&library);
        let mut reader = None;
        let read_overview = move |track_id| {
            if reader.is_none() {
                reader = library.reader().ok();
            }
            reader.as_ref()?.waveform(track_id).ok().flatten()
        };
        let mut reader = None;
        let read_marks = move |track_id| {
            if reader.is_none() {
                reader = marks_library.reader().ok();
            }
            let points = reader.as_ref()?.hotcue_points(track_id).ok()?;
            Some(points.map(|point| point.map(|p| HotcueMark::new(p.color, p.name))))
        };
        Self::start(read_overview, read_marks, |audio| {
            analysis::waveform(audio, DETAIL_FRAMES_PER_BUCKET)
        })
    }

    fn start(
        mut read_overview: impl FnMut(i64) -> Option<Waveform> + Send + 'static,
        mut read_marks: impl FnMut(i64) -> Option<Marks> + Send + 'static,
        compute: impl Fn(&TrackBuffer) -> Waveform + Send + 'static,
    ) -> Self {
        let (jobs, job_rx) = channel::<Job>();
        let (done_tx, done) = channel();
        let wanted = Arc::new(Wanted::default());
        let check = Arc::clone(&wanted);
        let thread = std::thread::Builder::new()
            .name("waveforms".to_owned())
            .spawn(move || {
                for job in job_rx {
                    if !check.wants(&job) {
                        continue;
                    }
                    let send = |part| {
                        done_tx.send(Done {
                            deck: job.deck,
                            load_seq: job.load_seq,
                            part,
                        })
                    };
                    if send(Part::Marks(read_marks(job.track_id).map(Arc::new))).is_err() {
                        return;
                    }
                    let Some(audio) = &job.audio else { continue };
                    let overview = read_overview(job.track_id).map(Arc::new);
                    if send(Part::Overview(overview)).is_err() {
                        return;
                    }
                    if !check.wants(&job) {
                        continue;
                    }
                    let detail = compute(audio);
                    if send(Part::Detail(Arc::new(detail))).is_err() {
                        return;
                    }
                }
            })
            .ok();
        Self {
            slots: Slots::default(),
            wanted,
            jobs: Some(jobs),
            done,
            thread,
            tracks: [0; 4],
        }
    }

    /// Takes the results that came in since.
    pub fn collect(&mut self) {
        for done in self.done.try_iter() {
            self.slots.receive(done);
        }
    }

    /// Notes that `deck` holds `track_id` as its load `load_seq`, with `audio` decoded;
    /// the first time a load is seen, its waveforms are asked for.
    pub fn observe(&mut self, deck: usize, load_seq: u64, track_id: i64, audio: &Arc<TrackBuffer>) {
        if self.slots.request(deck, load_seq)
            && let Some(jobs) = &self.jobs
        {
            self.wanted.load_seq[deck].store(load_seq, Ordering::Relaxed);
            self.tracks[deck] = track_id;
            let _ = jobs.send(Job {
                deck,
                load_seq,
                track_id,
                audio: Some(Arc::clone(audio)),
            });
        }
    }

    /// Reads the hotcue marks of every deck's track again, for after the library's hotcues
    /// changed without a load (an import).
    pub fn refresh_marks(&mut self) {
        let Some(jobs) = &self.jobs else { return };
        for (deck, slot) in self.slots.0.iter().enumerate() {
            let Some(slot) = slot else { continue };
            let _ = jobs.send(Job {
                deck,
                load_seq: slot.load_seq,
                track_id: self.tracks[deck],
                audio: None,
            });
        }
    }

    /// Notes that `deck` holds no library track.
    pub fn empty(&mut self, deck: usize) {
        self.slots.clear(deck);
        self.wanted.load_seq[deck].store(0, Ordering::Relaxed);
    }

    /// The hotcue marks of `deck`'s load `load_seq`, as far as they came in.
    pub fn marks(&self, deck: usize, load_seq: u64) -> Arc<Marks> {
        self.slots.marks(deck, load_seq)
    }

    /// The overview and detail of `deck`'s load `load_seq`, as far as they came in.
    pub fn waveforms(
        &self,
        deck: usize,
        load_seq: u64,
    ) -> (Option<Arc<Waveform>>, Option<Arc<Waveform>>) {
        self.slots.waveforms(deck, load_seq)
    }
}

impl Drop for DetailJobs {
    fn drop(&mut self) {
        // The thread skips what is queued and ends after the waveform it is on.
        self.wanted.stop.store(true, Ordering::Relaxed);
        self.jobs = None;
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn waveform() -> Arc<Waveform> {
        Arc::new(Waveform {
            frames_per_bucket: 128,
            buckets: vec![[1, 2, 3, 4]],
        })
    }

    fn detail(deck: usize, load_seq: u64) -> Done {
        Done {
            deck,
            load_seq,
            part: Part::Detail(waveform()),
        }
    }

    #[test]
    fn stale_detail_is_dropped() {
        let mut slots = Slots::default();
        assert!(slots.request(0, 3));
        assert!(slots.request(0, 4));
        slots.receive(detail(0, 3));
        assert_eq!(slots.waveforms(0, 4), (None, None));
        slots.receive(detail(0, 4));
        let (overview, detail) = slots.waveforms(0, 4);
        assert!(detail.is_some());
        // No stored overview: the detail stands in.
        assert!(Arc::ptr_eq(&overview.unwrap(), &detail.unwrap()));
    }

    #[test]
    fn a_stored_overview_is_kept_and_a_load_is_asked_for_once() {
        let mut slots = Slots::default();
        assert!(slots.request(1, 2));
        assert!(!slots.request(1, 2));
        let stored = waveform();
        slots.receive(Done {
            deck: 1,
            load_seq: 2,
            part: Part::Overview(Some(Arc::clone(&stored))),
        });
        let (overview, detail) = slots.waveforms(1, 2);
        assert!(Arc::ptr_eq(&overview.unwrap(), &stored));
        assert!(detail.is_none());
        slots.clear(1);
        assert_eq!(slots.waveforms(1, 2), (None, None));
    }

    #[test]
    fn dropping_skips_the_queued_loads() {
        use std::sync::atomic::AtomicUsize;
        use std::time::{Duration, Instant};

        let calls = Arc::new(AtomicUsize::new(0));
        let (started_tx, started) = channel();
        let counted = Arc::clone(&calls);
        let mut jobs = DetailJobs::start(
            |_| None,
            |_| None,
            move |_| {
                counted.fetch_add(1, Ordering::Relaxed);
                let _ = started_tx.send(());
                std::thread::sleep(Duration::from_millis(100));
                Waveform {
                    frames_per_bucket: 128,
                    buckets: Vec::new(),
                }
            },
        );
        let audio = Arc::new(TrackBuffer::new(44_100, vec![0.0; 2 * 441]).unwrap());
        jobs.observe(0, 1, 7, &audio);
        started.recv_timeout(Duration::from_secs(5)).unwrap();
        for seq in 2..=5 {
            jobs.observe(0, seq, 7, &audio);
        }
        let at = Instant::now();
        drop(jobs);
        assert!(at.elapsed() < Duration::from_millis(400));
        // Only the load the thread was on; the stale ones and the wanted one that was
        // still queued when the drop came were skipped.
        assert_eq!(calls.load(Ordering::Relaxed), 1);
    }

    fn marks_of(name: &str) -> Marks {
        let mut marks = Marks::default();
        marks[2] = Some(HotcueMark::new(Some(0x00ff00), Some(name.into())));
        marks
    }

    /// Collects until `deck`'s load `load_seq` has marks (or panics after a while).
    fn marks_after_collecting(jobs: &mut DetailJobs, deck: usize, load_seq: u64, name: &str) {
        use std::time::{Duration, Instant};
        let until = Instant::now() + Duration::from_secs(5);
        while *jobs.marks(deck, load_seq) != marks_of(name) {
            assert!(Instant::now() < until, "marks never came in");
            std::thread::sleep(Duration::from_millis(5));
            jobs.collect();
        }
    }

    #[test]
    fn marks_are_read_when_a_track_loads_and_again_on_a_refresh() {
        use std::sync::Mutex;

        let name = Arc::new(Mutex::new("Drop".to_owned()));
        let reading = Arc::clone(&name);
        let mut jobs = DetailJobs::start(
            |_| None,
            move |track| {
                assert_eq!(track, 7);
                Some(marks_of(&reading.lock().unwrap()))
            },
            |_| Waveform {
                frames_per_bucket: 128,
                buckets: Vec::new(),
            },
        );
        let audio = Arc::new(TrackBuffer::new(44_100, vec![0.0; 2 * 441]).unwrap());
        assert_eq!(*jobs.marks(0, 1), Marks::default());
        jobs.observe(0, 1, 7, &audio);
        marks_after_collecting(&mut jobs, 0, 1, "Drop");
        // A frame shares the marks instead of copying them.
        assert!(Arc::ptr_eq(&jobs.marks(0, 1), &jobs.marks(0, 1)));
        // Another load of the deck does not show them.
        assert_eq!(*jobs.marks(0, 2), Marks::default());
        // The library changed under the same load.
        *name.lock().unwrap() = "Break".to_owned();
        jobs.refresh_marks();
        marks_after_collecting(&mut jobs, 0, 1, "Break");
        // A deck with nothing loaded has nothing to read again.
        jobs.empty(0);
        jobs.refresh_marks();
        assert_eq!(*jobs.marks(0, 1), Marks::default());
    }
}
