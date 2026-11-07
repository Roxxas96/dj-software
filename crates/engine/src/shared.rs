use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering, fence};

use crate::{DeckId, Grid, HOTCUES, SAMPLER_SLOTS, SamplerSnapshot};

/// A deck's state as last published by the renderer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DeckSnapshot {
    pub loaded: bool,
    pub playing: bool,
    /// Playhead in track frames.
    pub position_frames: f64,
    pub duration_frames: usize,
    /// 0 when no track is loaded.
    pub track_sample_rate: u32,
    pub tempo_ratio: f64,
    /// Cue point in track frames.
    pub cue_frames: f64,
    /// Cue is held and the deck is previewing from the cue point.
    pub previewing: bool,
    pub grid: Option<Grid>,
    /// Hotcue positions in track frames, `None` for an empty slot.
    pub hotcues: [Option<f64>; HOTCUES],
    /// Active or stored loop start and end in track frames.
    pub loop_in: Option<f64>,
    pub loop_out: Option<f64>,
    pub loop_active: bool,
    /// Length in beats of the next auto loop.
    pub loop_beats: f64,
    /// The jog wheel scratches (vinyl mode) rather than only nudges.
    pub vinyl: bool,
    /// The jog wheel's top is touched.
    pub jog_touched: bool,
    /// Slip mode is on.
    pub slip: bool,
    /// Keylock is on.
    pub keylock: bool,
    /// While slipping, where the track would be, in track frames.
    pub ghost_frames: Option<f64>,
    /// A loop roll or slice is held and the playhead wraps in its roll loop.
    pub roll_active: bool,
    /// A backspin is running.
    pub backspinning: bool,
    /// The first beat, counted from the grid's first beat, of the 8-beat block the slicer
    /// pads play: the block of where the track would be, held while a slice is. `None`
    /// without a grid.
    pub slicer_block_beat: Option<f64>,
    /// How many `Load`s the deck has applied. In a snapshot from
    /// [`EngineController::deck_consistent`](crate::EngineController::deck_consistent)
    /// every field is from the same block as this count.
    pub load_seq: u64,
    /// The loaded track has its stems.
    pub stems: bool,
    /// The stems are in use: the EQ knobs control them ([`EqMode::Stems`](crate::EqMode)
    /// with stems present).
    pub stems_active: bool,
}

impl Default for DeckSnapshot {
    /// An empty deck: nothing loaded, original tempo, a 4-beat auto loop, vinyl mode.
    fn default() -> Self {
        Self {
            loaded: false,
            playing: false,
            position_frames: 0.0,
            duration_frames: 0,
            track_sample_rate: 0,
            tempo_ratio: 1.0,
            cue_frames: 0.0,
            previewing: false,
            grid: None,
            hotcues: [None; HOTCUES],
            loop_in: None,
            loop_out: None,
            loop_active: false,
            loop_beats: 4.0,
            vinyl: true,
            jog_touched: false,
            slip: false,
            keylock: false,
            ghost_frames: None,
            roll_active: false,
            backspinning: false,
            slicer_block_beat: None,
            load_seq: 0,
            stems: false,
            stems_active: false,
        }
    }
}

/// A channel strip's state as last published by the renderer, for the status line. It is
/// published inside the same seqlock window as the channel's deck snapshot.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChannelSnapshot {
    /// Which of the three effect slots are on.
    pub fx_enabled: [bool; 3],
    /// The effect amount in `[0, 1]`.
    pub fx_amount: f32,
}

impl Default for ChannelSnapshot {
    fn default() -> Self {
        Self {
            fx_enabled: [false; 3],
            fx_amount: 0.5,
        }
    }
}

/// Peak absolute sample values of the last rendered block, for metering.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Meters {
    /// Per channel, in deck order, post-fader (before the crossfader).
    pub channel_peak: [f32; 4],
    /// Left and right of the master output, after the clamp.
    pub master_peak: [f32; 2],
}

impl Meters {
    /// Keeps the larger of each peak.
    pub(crate) fn merge(&mut self, other: &Meters) {
        for (peak, other) in self.channel_peak.iter_mut().zip(other.channel_peak) {
            *peak = peak.max(other);
        }
        for (peak, other) in self.master_peak.iter_mut().zip(other.master_peak) {
            *peak = peak.max(other);
        }
    }
}

/// State the renderer publishes for the controller. Each field is an independent `Relaxed`
/// atomic, so a plain read may mix two consecutive blocks (fine for display). Deck
/// snapshots are also published seqlock-style (Ruling R14), so a reader can get one block's
/// snapshot whole ([`Shared::load_deck_consistent`]); the renderer never waits for readers.
#[derive(Debug, Default)]
pub(crate) struct Shared {
    decks: [DeckShared; DeckId::ALL.len()],
    channel_peak: [AtomicU32; DeckId::ALL.len()],
    master_peak: [AtomicU32; 2],
    sampler_playing: [AtomicBool; SAMPLER_SLOTS],
}

impl Shared {
    pub(crate) fn store_deck(
        &self,
        id: DeckId,
        snapshot: &DeckSnapshot,
        channel: &ChannelSnapshot,
    ) {
        self.decks[id.index()].store(snapshot, channel);
    }

    pub(crate) fn load_channel(&self, id: DeckId) -> ChannelSnapshot {
        self.decks[id.index()].load_channel()
    }

    pub(crate) fn load_deck(&self, id: DeckId) -> DeckSnapshot {
        self.decks[id.index()].load()
    }

    pub(crate) fn load_deck_consistent(&self, id: DeckId) -> Option<DeckSnapshot> {
        self.decks[id.index()].load_consistent()
    }

    pub(crate) fn store_meters(&self, meters: &Meters) {
        store_f32s(&self.channel_peak, &meters.channel_peak);
        store_f32s(&self.master_peak, &meters.master_peak);
    }

    pub(crate) fn store_sampler(&self, snapshot: &SamplerSnapshot) {
        for (atomic, playing) in self.sampler_playing.iter().zip(snapshot.playing) {
            atomic.store(playing, Ordering::Relaxed);
        }
    }

    pub(crate) fn load_sampler(&self) -> SamplerSnapshot {
        SamplerSnapshot {
            playing: self
                .sampler_playing
                .each_ref()
                .map(|a| a.load(Ordering::Relaxed)),
        }
    }

    pub(crate) fn load_meters(&self) -> Meters {
        Meters {
            channel_peak: self
                .channel_peak
                .each_ref()
                .map(|a| f32::from_bits(a.load(Ordering::Relaxed))),
            master_peak: self
                .master_peak
                .each_ref()
                .map(|a| f32::from_bits(a.load(Ordering::Relaxed))),
        }
    }
}

fn store_f32s(atomics: &[AtomicU32], values: &[f32]) {
    for (atomic, value) in atomics.iter().zip(values) {
        atomic.store(value.to_bits(), Ordering::Relaxed);
    }
}

/// How many times a consistent read is tried before giving up.
const CONSISTENT_READ_TRIES: usize = 16;

#[derive(Debug, Default)]
struct DeckShared {
    /// Seqlock sequence: odd while the renderer is storing a snapshot.
    seq: AtomicU64,
    loaded: AtomicBool,
    playing: AtomicBool,
    position_frames: AtomicU64,
    duration_frames: AtomicUsize,
    track_sample_rate: AtomicU32,
    tempo_ratio: AtomicU64,
    cue_frames: AtomicU64,
    previewing: AtomicBool,
    // `Option<f64>`s are stored as bits, with NaN for `None`.
    grid_period: AtomicU64,
    grid_first_beat: AtomicU64,
    hotcues: [AtomicU64; HOTCUES],
    loop_in: AtomicU64,
    loop_out: AtomicU64,
    loop_active: AtomicBool,
    loop_beats: AtomicU64,
    vinyl: AtomicBool,
    jog_touched: AtomicBool,
    slip: AtomicBool,
    keylock: AtomicBool,
    ghost_frames: AtomicU64,
    roll_active: AtomicBool,
    backspinning: AtomicBool,
    slicer_block_beat: AtomicU64,
    load_seq: AtomicU64,
    stems: AtomicBool,
    stems_active: AtomicBool,
    fx_enabled: [AtomicBool; 3],
    fx_amount: AtomicU32,
}

fn opt_bits(v: Option<f64>) -> u64 {
    v.unwrap_or(f64::NAN).to_bits()
}

fn bits_opt(bits: u64) -> Option<f64> {
    Some(f64::from_bits(bits)).filter(|v| !v.is_nan())
}

impl DeckShared {
    /// Publishes `s`. Only the renderer calls this, so the sequence has one writer.
    fn store(&self, s: &DeckSnapshot, channel: &ChannelSnapshot) {
        let seq = self.seq.load(Ordering::Relaxed);
        self.seq.store(seq.wrapping_add(1), Ordering::Relaxed);
        // The odd sequence is visible before any field store.
        fence(Ordering::Release);
        self.store_fields(s);
        self.store_channel(channel);
        // Every field store is visible before the even sequence.
        self.seq.store(seq.wrapping_add(2), Ordering::Release);
    }

    fn store_channel(&self, c: &ChannelSnapshot) {
        for (atomic, on) in self.fx_enabled.iter().zip(c.fx_enabled) {
            atomic.store(on, Ordering::Relaxed);
        }
        self.fx_amount
            .store(c.fx_amount.to_bits(), Ordering::Relaxed);
    }

    fn load_channel(&self) -> ChannelSnapshot {
        ChannelSnapshot {
            fx_enabled: self
                .fx_enabled
                .each_ref()
                .map(|a| a.load(Ordering::Relaxed)),
            fx_amount: f32::from_bits(self.fx_amount.load(Ordering::Relaxed)),
        }
    }

    fn store_fields(&self, s: &DeckSnapshot) {
        self.loaded.store(s.loaded, Ordering::Relaxed);
        self.playing.store(s.playing, Ordering::Relaxed);
        self.position_frames
            .store(s.position_frames.to_bits(), Ordering::Relaxed);
        self.duration_frames
            .store(s.duration_frames, Ordering::Relaxed);
        self.track_sample_rate
            .store(s.track_sample_rate, Ordering::Relaxed);
        self.tempo_ratio
            .store(s.tempo_ratio.to_bits(), Ordering::Relaxed);
        self.cue_frames
            .store(s.cue_frames.to_bits(), Ordering::Relaxed);
        self.previewing.store(s.previewing, Ordering::Relaxed);
        // A grid is published as its two fields; no grid is a NaN period.
        let period = s.grid.map(|g| g.beat_period_frames);
        self.grid_period.store(opt_bits(period), Ordering::Relaxed);
        self.grid_first_beat.store(
            s.grid.map_or(0.0, |g| g.first_beat_frames).to_bits(),
            Ordering::Relaxed,
        );
        for (atomic, hotcue) in self.hotcues.iter().zip(s.hotcues) {
            atomic.store(opt_bits(hotcue), Ordering::Relaxed);
        }
        self.loop_in.store(opt_bits(s.loop_in), Ordering::Relaxed);
        self.loop_out.store(opt_bits(s.loop_out), Ordering::Relaxed);
        self.loop_active.store(s.loop_active, Ordering::Relaxed);
        self.loop_beats
            .store(s.loop_beats.to_bits(), Ordering::Relaxed);
        self.vinyl.store(s.vinyl, Ordering::Relaxed);
        self.jog_touched.store(s.jog_touched, Ordering::Relaxed);
        self.slip.store(s.slip, Ordering::Relaxed);
        self.keylock.store(s.keylock, Ordering::Relaxed);
        self.ghost_frames
            .store(opt_bits(s.ghost_frames), Ordering::Relaxed);
        self.roll_active.store(s.roll_active, Ordering::Relaxed);
        self.backspinning.store(s.backspinning, Ordering::Relaxed);
        self.slicer_block_beat
            .store(opt_bits(s.slicer_block_beat), Ordering::Relaxed);
        self.load_seq.store(s.load_seq, Ordering::Relaxed);
        self.stems.store(s.stems, Ordering::Relaxed);
        self.stems_active.store(s.stems_active, Ordering::Relaxed);
    }

    /// One block's snapshot whole, or `None` if the renderer was storing one at each of
    /// [`CONSISTENT_READ_TRIES`] tries.
    fn load_consistent(&self) -> Option<DeckSnapshot> {
        for _ in 0..CONSISTENT_READ_TRIES {
            let before = self.seq.load(Ordering::Acquire);
            if before % 2 == 1 {
                std::hint::spin_loop();
                continue;
            }
            let snapshot = self.load();
            // The field loads happen before the second sequence load.
            fence(Ordering::Acquire);
            if self.seq.load(Ordering::Relaxed) == before {
                return Some(snapshot);
            }
        }
        None
    }

    /// The fields as they are now, possibly from two blocks.
    fn load(&self) -> DeckSnapshot {
        DeckSnapshot {
            loaded: self.loaded.load(Ordering::Relaxed),
            playing: self.playing.load(Ordering::Relaxed),
            position_frames: f64::from_bits(self.position_frames.load(Ordering::Relaxed)),
            duration_frames: self.duration_frames.load(Ordering::Relaxed),
            track_sample_rate: self.track_sample_rate.load(Ordering::Relaxed),
            tempo_ratio: f64::from_bits(self.tempo_ratio.load(Ordering::Relaxed)),
            cue_frames: f64::from_bits(self.cue_frames.load(Ordering::Relaxed)),
            previewing: self.previewing.load(Ordering::Relaxed),
            grid: bits_opt(self.grid_period.load(Ordering::Relaxed)).map(|beat_period_frames| {
                Grid {
                    beat_period_frames,
                    first_beat_frames: f64::from_bits(self.grid_first_beat.load(Ordering::Relaxed)),
                }
            }),
            hotcues: self
                .hotcues
                .each_ref()
                .map(|a| bits_opt(a.load(Ordering::Relaxed))),
            loop_in: bits_opt(self.loop_in.load(Ordering::Relaxed)),
            loop_out: bits_opt(self.loop_out.load(Ordering::Relaxed)),
            loop_active: self.loop_active.load(Ordering::Relaxed),
            loop_beats: f64::from_bits(self.loop_beats.load(Ordering::Relaxed)),
            vinyl: self.vinyl.load(Ordering::Relaxed),
            jog_touched: self.jog_touched.load(Ordering::Relaxed),
            slip: self.slip.load(Ordering::Relaxed),
            keylock: self.keylock.load(Ordering::Relaxed),
            ghost_frames: bits_opt(self.ghost_frames.load(Ordering::Relaxed)),
            roll_active: self.roll_active.load(Ordering::Relaxed),
            backspinning: self.backspinning.load(Ordering::Relaxed),
            slicer_block_beat: bits_opt(self.slicer_block_beat.load(Ordering::Relaxed)),
            load_seq: self.load_seq.load(Ordering::Relaxed),
            stems: self.stems.load(Ordering::Relaxed),
            stems_active: self.stems_active.load(Ordering::Relaxed),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    fn numbered(n: u64) -> DeckSnapshot {
        DeckSnapshot {
            loaded: true,
            cue_frames: n as f64,
            position_frames: n as f64,
            hotcues: [Some(n as f64); HOTCUES],
            load_seq: n,
            ..DeckSnapshot::default()
        }
    }

    #[test]
    fn a_read_during_a_store_is_not_consistent() {
        let deck = DeckShared::default();
        deck.store(&numbered(3), &ChannelSnapshot::default());
        assert_eq!(deck.load_consistent(), Some(numbered(3)));
        // The renderer is in the middle of a store: odd sequence, fields half written.
        let seq = deck.seq.load(Ordering::Relaxed);
        deck.seq.store(seq + 1, Ordering::Relaxed);
        deck.store_fields(&DeckSnapshot {
            load_seq: 4,
            ..numbered(3)
        });
        assert_eq!(deck.load_consistent(), None);
        assert_eq!(
            deck.load().load_seq,
            4,
            "a plain read still shows the fields"
        );
        deck.seq.store(seq + 2, Ordering::Release);
        assert_eq!(deck.load_consistent().map(|s| s.load_seq), Some(4));
    }

    #[test]
    fn consistent_reads_never_mix_blocks() {
        let deck = Arc::new(DeckShared::default());
        let writer = {
            let deck = Arc::clone(&deck);
            std::thread::spawn(move || {
                for n in 0..200_000 {
                    deck.store(&numbered(n), &ChannelSnapshot::default());
                }
            })
        };
        let check = |s: &DeckSnapshot| {
            let n = s.load_seq as f64;
            assert_eq!((s.cue_frames, s.position_frames), (n, n));
            assert!(s.hotcues.iter().all(|h| *h == Some(n)), "{s:?}");
        };
        while !writer.is_finished() {
            if let Some(s) = deck.load_consistent() {
                check(&s);
            }
        }
        writer.join().unwrap();
        // With the writer gone a read always succeeds and sees its last block, even if the
        // writer finished before the loop above got one in.
        let last = deck.load_consistent().expect("no writer left to race");
        check(&last);
        assert_eq!(last.load_seq, 199_999);
    }
}
