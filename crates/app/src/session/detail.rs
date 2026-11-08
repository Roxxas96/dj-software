//! The waveforms of the tracks on the decks, computed off the control thread: the stored
//! overview is read from the library and the detailed waveform is computed from the
//! decoded track, on a thread of their own.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread::JoinHandle;

use analysis::Waveform;
use engine::TrackBuffer;
use library::Library;

/// Frames per bucket of the detailed waveform.
const DETAIL_FRAMES_PER_BUCKET: u32 = 128;

/// A track to compute the waveforms of.
struct Job {
    deck: usize,
    load_seq: u64,
    track_id: i64,
    audio: Arc<TrackBuffer>,
}

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
}

/// What is known of the waveforms of the load a deck is on.
#[derive(Default)]
struct Slot {
    load_seq: u64,
    overview: Option<Arc<Waveform>>,
    detail: Option<Arc<Waveform>>,
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

/// Computes the waveforms of each load on a deck in the background.
pub(super) struct DetailJobs {
    slots: Slots,
    jobs: Option<Sender<Job>>,
    done: Receiver<Done>,
    thread: Option<JoinHandle<()>>,
}

impl DetailJobs {
    pub fn new(library: Arc<Library>) -> Self {
        let (jobs, job_rx) = channel::<Job>();
        let (done_tx, done) = channel();
        let thread = std::thread::Builder::new()
            .name("waveforms".to_owned())
            .spawn(move || {
                let reader = library.reader().ok();
                for job in job_rx {
                    let overview = reader
                        .as_ref()
                        .and_then(|r| r.waveform(job.track_id).ok().flatten())
                        .map(Arc::new);
                    let send = |part| {
                        done_tx.send(Done {
                            deck: job.deck,
                            load_seq: job.load_seq,
                            part,
                        })
                    };
                    if send(Part::Overview(overview)).is_err() {
                        return;
                    }
                    let detail = analysis::waveform(&job.audio, DETAIL_FRAMES_PER_BUCKET);
                    if send(Part::Detail(Arc::new(detail))).is_err() {
                        return;
                    }
                }
            })
            .ok();
        Self {
            slots: Slots::default(),
            jobs: Some(jobs),
            done,
            thread,
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
            let _ = jobs.send(Job {
                deck,
                load_seq,
                track_id,
                audio: Arc::clone(audio),
            });
        }
    }

    /// Notes that `deck` holds no library track.
    pub fn empty(&mut self, deck: usize) {
        self.slots.clear(deck);
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
        // The thread ends after the waveform it is on.
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
}
