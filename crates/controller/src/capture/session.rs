//! The capture session: a pure state machine walking the checklist steps.
//!
//! The CLI feeds it timestamped messages and polls it with the same clock
//! ([`crate::clock::now_micros`]); it never touches ports or stdin.

use super::checklist::{Step, StepKind};
use super::fixture::{Control, Fixture, Meta, is_release, key_and_value};
use crate::midi::{MidiMessage, TimedMidi};

/// Quiet gap after the last message that completes a button step.
pub const BUTTON_GAP_MICROS: u64 = 400_000;
/// Quiet gap that completes a continuous, jog or pads step.
pub const LONG_GAP_MICROS: u64 = 1_500_000;
/// Distinct note-on notes a pads step needs.
pub const PAD_COUNT: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepStatus {
    /// No message yet for the current step.
    Waiting,
    /// Messages arrived; waiting for the quiet gap (and, for pads, 8 notes).
    Recording,
    /// The step just completed (or there is no step left).
    Done,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Slot {
    Missing,
    Skipped,
    Captured(Control),
}

pub struct CaptureSession {
    steps: Vec<Step>,
    slots: Vec<Slot>,
    /// First missing step, if any.
    cursor: Option<usize>,
    buffer: Vec<TimedMidi>,
    /// Steps in the order they were completed or skipped, for `redo`.
    history: Vec<usize>,
    meta: Meta,
    /// Controls from a resumed fixture whose ids are not in the checklist; kept as is.
    extras: Vec<Control>,
}

impl CaptureSession {
    pub fn new(steps: Vec<Step>) -> Self {
        let slots = vec![Slot::Missing; steps.len()];
        let mut session = CaptureSession {
            steps,
            slots,
            cursor: None,
            buffer: Vec::new(),
            history: Vec::new(),
            meta: Meta::default(),
            extras: Vec::new(),
        };
        session.update_cursor();
        session
    }

    /// Continues a capture saved in `prior`: its captured and skipped steps are
    /// done, and the session starts at the first missing step. Controls whose
    /// ids are not in `steps` are kept unchanged in the output.
    pub fn resume(steps: Vec<Step>, prior: Fixture) -> Self {
        let mut session = CaptureSession::new(steps);
        for control in prior.controls {
            match session.index_of(&control.id) {
                Some(i) => session.slots[i] = Slot::Captured(control),
                None => session.extras.push(control),
            }
        }
        for id in &prior.meta.skipped {
            if let Some(i) = session.index_of(id)
                && session.slots[i] == Slot::Missing
            {
                session.slots[i] = Slot::Skipped;
            }
        }
        session.meta = Meta {
            skipped: Vec::new(),
            ..prior.meta
        };
        session.update_cursor();
        session
    }

    pub fn set_meta(&mut self, meta: Meta) {
        self.meta = Meta {
            skipped: Vec::new(),
            ..meta
        };
    }

    /// The step being captured: the first step neither captured nor skipped.
    pub fn current(&self) -> Option<&Step> {
        self.cursor.map(|i| &self.steps[i])
    }

    /// `(1-based number of the current step, total steps)`; the number is
    /// `total + 1` when every step is done.
    pub fn position(&self) -> (usize, usize) {
        let total = self.steps.len();
        (self.cursor.unwrap_or(total) + 1, total)
    }

    /// Steps captured or skipped.
    pub fn done_count(&self) -> usize {
        self.slots.iter().filter(|s| **s != Slot::Missing).count()
    }

    /// Distinct pressed notes recorded so far for the current step, ignoring
    /// the SHIFT modifier and keys whose first message was a release.
    pub fn distinct_notes(&self) -> usize {
        let Some(step) = self.current() else {
            return 0;
        };
        Control::from_messages(&step.id, step.kind, &self.relevant())
            .messages
            .iter()
            .filter(|m| m.status & 0xF0 == 0x90)
            .count()
    }

    /// For a SHIFT step, the `(channel, note)` of the first key pressed in the
    /// step: the held SHIFT, which is left out of the record.
    pub fn modifier(&self) -> Option<(u8, u8)> {
        if !self.current()?.shift {
            return None;
        }
        self.buffer.iter().find_map(|timed| match timed.message {
            MidiMessage::NoteOn { channel, note, .. } => Some((channel, note)),
            _ => None,
        })
    }

    /// The current step's messages that count: without the SHIFT modifier and
    /// without pairs whose first message was a release.
    fn relevant(&self) -> Vec<TimedMidi> {
        let modifier = self.modifier();
        let mut released_first: Vec<(u8, u8)> = Vec::new();
        let mut seen: Vec<(u8, u8)> = Vec::new();
        let mut out = Vec::new();
        for timed in &self.buffer {
            let (status, data1, _) = key_and_value(timed.message);
            if modifier.is_some_and(|(channel, note)| (0x90 | channel, note) == (status, data1)) {
                continue;
            }
            if released_first.contains(&(status, data1)) {
                continue;
            }
            if !seen.contains(&(status, data1)) {
                if is_release(timed.message) {
                    released_first.push((status, data1));
                    continue;
                }
                seen.push((status, data1));
            }
            out.push(*timed);
        }
        out
    }

    /// Records a message for the current step (ignored when every step is done).
    pub fn feed(&mut self, msg: TimedMidi) {
        if self.cursor.is_some() {
            self.buffer.push(msg);
        }
    }

    /// Completes the current step once its quiet gap has passed (and, for pads,
    /// 8 distinct notes were seen). Returns `Done` once, then moves on.
    pub fn poll(&mut self, now_micros: u64) -> StepStatus {
        let Some(i) = self.cursor else {
            return StepStatus::Done;
        };
        let relevant = self.relevant();
        let Some(last) = relevant.last() else {
            return StepStatus::Waiting;
        };
        let kind = self.steps[i].kind;
        let gap = match kind {
            StepKind::Button => BUTTON_GAP_MICROS,
            StepKind::Pads | StepKind::Continuous | StepKind::Jog => LONG_GAP_MICROS,
        };
        let quiet = now_micros.saturating_sub(last.micros) >= gap;
        let enough = kind != StepKind::Pads || self.distinct_notes() >= PAD_COUNT;
        if quiet && enough {
            self.commit(i);
            StepStatus::Done
        } else {
            StepStatus::Recording
        }
    }

    /// Completes the current step now with what was recorded. Returns false
    /// (and does nothing) if nothing was recorded.
    pub fn accept(&mut self) -> bool {
        match self.cursor {
            Some(i) if !self.relevant().is_empty() => {
                self.commit(i);
                true
            }
            _ => false,
        }
    }

    /// Marks the current step as skipped and moves on.
    pub fn skip(&mut self) {
        if let Some(i) = self.cursor {
            self.buffer.clear();
            self.slots[i] = Slot::Skipped;
            self.history.push(i);
            self.update_cursor();
        }
    }

    /// Discards the partial recording and reopens the previously completed or
    /// skipped step, which becomes current again.
    pub fn redo(&mut self) {
        self.buffer.clear();
        let previous = self.history.pop().or_else(|| {
            let end = self.cursor.unwrap_or(self.slots.len());
            (0..end).rev().find(|&i| self.slots[i] != Slot::Missing)
        });
        if let Some(i) = previous {
            self.slots[i] = Slot::Missing;
            self.history.retain(|&h| h != i);
            self.update_cursor();
        }
    }

    /// The captured control with this id, if any.
    pub fn control(&self, id: &str) -> Option<&Control> {
        let i = self.index_of(id)?;
        match &self.slots[i] {
            Slot::Captured(control) => Some(control),
            _ => None,
        }
    }

    /// A snapshot of everything captured so far, in checklist order.
    pub fn fixture(&self) -> Fixture {
        let mut meta = self.meta.clone();
        meta.skipped = self
            .steps
            .iter()
            .zip(&self.slots)
            .filter(|(_, slot)| **slot == Slot::Skipped)
            .map(|(step, _)| step.id.clone())
            .collect();
        let mut controls: Vec<Control> = self
            .slots
            .iter()
            .filter_map(|slot| match slot {
                Slot::Captured(control) => Some(control.clone()),
                _ => None,
            })
            .collect();
        controls.extend(self.extras.iter().cloned());
        Fixture { meta, controls }
    }

    /// Stores an LED probe answer on a captured control (no-op if not captured).
    pub fn set_led(&mut self, id: &str, led: bool, values_vary: Option<bool>) {
        if let Some(i) = self.index_of(id)
            && let Slot::Captured(control) = &mut self.slots[i]
        {
            control.led = Some(led);
            control.led_values_vary = values_vary;
        }
    }

    pub fn finish(self) -> Fixture {
        self.fixture()
    }

    fn commit(&mut self, i: usize) {
        let relevant = self.relevant();
        let step = &self.steps[i];
        let control = Control::from_messages(&step.id, step.kind, &relevant);
        self.buffer.clear();
        self.slots[i] = Slot::Captured(control);
        self.history.push(i);
        self.update_cursor();
    }

    fn index_of(&self, id: &str) -> Option<usize> {
        self.steps.iter().position(|s| s.id == id)
    }

    fn update_cursor(&mut self) {
        self.cursor = self.slots.iter().position(|s| *s == Slot::Missing);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::fixture::{Cc14, MessageRange};
    use crate::midi::MidiMessage;

    const MS: u64 = 1000;

    fn step(id: &str, kind: StepKind) -> Step {
        Step {
            id: id.to_owned(),
            section: "Test".to_owned(),
            prompt: "do it".to_owned(),
            setup: None,
            prepare: None,
            kind,
            shift: false,
        }
    }

    fn timed(ms: u64, bytes: [u8; 3]) -> TimedMidi {
        TimedMidi {
            micros: ms * MS,
            message: MidiMessage::parse(&bytes).unwrap(),
        }
    }

    #[test]
    fn button_step_done_after_quiet_gap() {
        let mut session = CaptureSession::new(vec![
            step("play", StepKind::Button),
            step("cue", StepKind::Button),
        ]);
        assert_eq!(session.poll(0), StepStatus::Waiting);
        session.feed(timed(0, [0x90, 0x0B, 0x7F]));
        session.feed(timed(80, [0x90, 0x0B, 0x00]));
        assert_eq!(session.poll(300 * MS), StepStatus::Recording);
        assert_eq!(session.current().unwrap().id, "play");
        assert_eq!(session.poll(481 * MS), StepStatus::Done);
        assert_eq!(session.current().unwrap().id, "cue");
        assert_eq!(session.poll(482 * MS), StepStatus::Waiting);
        let fixture = session.finish();
        assert_eq!(fixture.controls.len(), 1);
        assert_eq!(fixture.controls[0].id, "play");
        assert_eq!(fixture.controls[0].kind, StepKind::Button);
    }

    #[test]
    fn continuous_step_records_min_max() {
        let mut session = CaptureSession::new(vec![step("fader", StepKind::Continuous)]);
        for (i, value) in [64u8, 10, 0, 50, 127, 90].into_iter().enumerate() {
            session.feed(timed(i as u64 * 10, [0xB0, 0x13, value]));
        }
        // 50 ms after the last message: a button would be recording too, but the
        // 1500 ms gap applies here.
        assert_eq!(session.poll(1000 * MS), StepStatus::Recording);
        assert_eq!(session.poll(1549 * MS), StepStatus::Recording);
        assert_eq!(session.poll(1550 * MS), StepStatus::Done);
        assert_eq!(session.current(), None);
        let fixture = session.finish();
        assert_eq!(
            fixture.controls[0].messages,
            [MessageRange {
                status: 0xB0,
                data1: 0x13,
                first: Some(64),
                min: 0,
                max: 127
            }]
        );
        assert_eq!(fixture.controls[0].cc14, None);
    }

    #[test]
    fn cc_pair_recorded_as_cc14() {
        let mut session = CaptureSession::new(vec![step("tempo", StepKind::Continuous)]);
        session.feed(timed(0, [0xB1, 0x00, 0x40]));
        session.feed(timed(1, [0xB1, 0x20, 0x10]));
        session.feed(timed(2, [0xB1, 0x00, 0x7F]));
        session.feed(timed(3, [0xB1, 0x20, 0x7F]));
        assert_eq!(session.poll(2000 * MS), StepStatus::Done);
        let control = session.finish().controls.remove(0);
        assert_eq!(control.cc14, Some(Cc14 { msb: 0, lsb: 32 }));
        assert_eq!(control.messages.len(), 2);
        assert_eq!(control.messages[0].min, 0x40);
        assert_eq!(control.messages[0].max, 0x7F);

        // An MSB with an LSB on another channel is not a 14-bit pair.
        let mut session = CaptureSession::new(vec![step("x", StepKind::Continuous)]);
        session.feed(timed(0, [0xB0, 0x05, 1]));
        session.feed(timed(1, [0xB1, 0x25, 1]));
        assert_eq!(session.poll(2000 * MS), StepStatus::Done);
        assert_eq!(session.finish().controls[0].cc14, None);
    }

    #[test]
    fn pads_step_needs_eight_distinct_notes() {
        let mut session = CaptureSession::new(vec![step("pads", StepKind::Pads)]);
        // Seven distinct pads, one of them hit twice, then a long pause.
        for (i, note) in [0u8, 1, 2, 3, 3, 4, 5, 6].into_iter().enumerate() {
            let t = i as u64 * 100;
            session.feed(timed(t, [0x97, note, 0x7F]));
            session.feed(timed(t + 50, [0x97, note, 0x00]));
        }
        assert_eq!(session.poll(10_000 * MS), StepStatus::Recording);
        session.feed(timed(10_000, [0x97, 7, 0x7F]));
        session.feed(timed(10_050, [0x97, 7, 0x00]));
        assert_eq!(session.poll(11_000 * MS), StepStatus::Recording);
        assert_eq!(session.poll(11_550 * MS), StepStatus::Done);
        let control = session.finish().controls.remove(0);
        assert_eq!(control.messages.len(), 8);
        assert!(control.messages.iter().all(|m| m.status == 0x97));
    }

    #[test]
    fn skip_and_redo() {
        let mut session = CaptureSession::new(vec![
            step("a", StepKind::Button),
            step("b", StepKind::Button),
            step("c", StepKind::Button),
        ]);
        // Redo with nothing done just clears the partial recording.
        session.feed(timed(0, [0x90, 1, 127]));
        session.redo();
        assert_eq!(session.current().unwrap().id, "a");
        assert_eq!(session.poll(1000 * MS), StepStatus::Waiting);

        session.feed(timed(1000, [0x90, 1, 127]));
        assert_eq!(session.poll(2000 * MS), StepStatus::Done);
        session.skip();
        assert_eq!(session.current().unwrap().id, "c");
        assert_eq!(session.fixture().meta.skipped, ["b"]);

        // Redo goes back to the skipped step, then to the captured one.
        session.redo();
        assert_eq!(session.current().unwrap().id, "b");
        assert!(session.fixture().meta.skipped.is_empty());
        session.redo();
        assert_eq!(session.current().unwrap().id, "a");
        assert!(session.fixture().controls.is_empty());

        // Re-capture a with a different note, then b and c.
        session.feed(timed(3000, [0x90, 2, 127]));
        assert_eq!(session.poll(4000 * MS), StepStatus::Done);
        assert_eq!(session.current().unwrap().id, "b");
        session.feed(timed(4000, [0x90, 3, 127]));
        assert_eq!(session.poll(5000 * MS), StepStatus::Done);
        session.skip();
        assert_eq!(session.current(), None);
        assert_eq!(session.poll(6000 * MS), StepStatus::Done);
        let fixture = session.finish();
        let ids: Vec<&str> = fixture.controls.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["a", "b"]);
        assert_eq!(fixture.controls[0].messages[0].data1, 2);
        assert_eq!(fixture.meta.skipped, ["c"]);
    }

    #[test]
    fn accept_ends_a_step_early() {
        let mut session = CaptureSession::new(vec![step("pads", StepKind::Pads)]);
        assert!(!session.accept());
        session.feed(timed(0, [0x97, 0, 127]));
        assert!(session.accept());
        assert_eq!(session.current(), None);
        assert_eq!(session.finish().controls[0].messages.len(), 1);
    }

    #[test]
    fn resume_continues_from_first_missing_step() {
        let steps = vec![
            step("a", StepKind::Button),
            step("b", StepKind::Button),
            step("c", StepKind::Button),
            step("d", StepKind::Button),
        ];
        let mut session = CaptureSession::new(steps.clone());
        session.set_meta(Meta {
            controller: "C".to_owned(),
            port: "P".to_owned(),
            ..Meta::default()
        });
        session.feed(timed(0, [0x90, 1, 127]));
        assert_eq!(session.poll(1000 * MS), StepStatus::Done);
        session.skip();
        let mut prior = session.fixture();
        prior.controls.push(Control::from_messages(
            "gone",
            StepKind::Button,
            &[timed(0, [0x90, 9, 127])],
        ));
        prior.controls.push(Control::from_messages(
            "d",
            StepKind::Button,
            &[timed(0, [0x90, 4, 127])],
        ));

        let mut resumed = CaptureSession::resume(steps, prior);
        assert_eq!(resumed.current().unwrap().id, "c");
        assert_eq!(resumed.position(), (3, 4));
        assert_eq!(resumed.done_count(), 3);
        resumed.feed(timed(0, [0x90, 3, 127]));
        assert_eq!(resumed.poll(1000 * MS), StepStatus::Done);
        assert_eq!(resumed.current(), None);
        let fixture = resumed.finish();
        let ids: Vec<&str> = fixture.controls.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["a", "c", "d", "gone"]);
        assert_eq!(fixture.meta.port, "P");
        assert_eq!(fixture.meta.skipped, ["b"]);
    }

    fn shift_step(id: &str, kind: StepKind) -> Step {
        Step {
            shift: true,
            ..step(id, kind)
        }
    }

    #[test]
    fn release_first_pairs_are_dropped() {
        // SHIFT (note 0x3F) released at the start of a step it was held across.
        let mut session = CaptureSession::new(vec![step("cue", StepKind::Button)]);
        session.feed(timed(0, [0x90, 0x3F, 0x00]));
        assert_eq!(session.poll(1000 * MS), StepStatus::Waiting);
        assert!(!session.accept());
        session.feed(timed(1000, [0x90, 0x0C, 0x7F]));
        session.feed(timed(1050, [0x90, 0x0C, 0x00]));
        session.feed(timed(1060, [0x90, 0x3F, 0x00]));
        assert_eq!(session.poll(2000 * MS), StepStatus::Done);
        let control = session.finish().controls.remove(0);
        assert_eq!(control.messages.len(), 1, "{control:?}");
        assert_eq!(control.messages[0].data1, 0x0C);
    }

    #[test]
    fn released_mode_button_does_not_count_as_pad() {
        let mut session = CaptureSession::new(vec![step("pads", StepKind::Pads)]);
        // HOT CUE (note 0x1B) pressed before listening, released after.
        session.feed(timed(0, [0x90, 0x1B, 0x00]));
        for note in 0..7u8 {
            let t = 100 + u64::from(note) * 100;
            session.feed(timed(t, [0x97, note, 0x7F]));
            session.feed(timed(t + 50, [0x97, note, 0x00]));
        }
        assert_eq!(session.distinct_notes(), 7);
        assert_eq!(session.poll(10_000 * MS), StepStatus::Recording);
    }

    #[test]
    fn shift_step_ignores_the_held_modifier() {
        let mut session = CaptureSession::new(vec![
            shift_step("play.shift", StepKind::Button),
            step("cue", StepKind::Button),
        ]);
        // SHIFT pressed and held: alone it neither records nor completes the step.
        session.feed(timed(0, [0x90, 0x3F, 0x7F]));
        assert_eq!(session.modifier(), Some((0, 0x3F)));
        assert_eq!(session.poll(2000 * MS), StepStatus::Waiting);
        session.feed(timed(2000, [0x90, 0x47, 0x7F]));
        session.feed(timed(2050, [0x90, 0x47, 0x00]));
        // SHIFT released inside the quiet gap: does not delay completion.
        session.feed(timed(2300, [0x90, 0x3F, 0x00]));
        assert_eq!(session.poll(2451 * MS), StepStatus::Done);
        let fixture = session.finish();
        assert_eq!(fixture.controls[0].messages.len(), 1);
        assert_eq!(fixture.controls[0].messages[0].data1, 0x47);
    }

    #[test]
    fn shift_pads_ignore_the_modifier_in_the_count() {
        let mut session = CaptureSession::new(vec![shift_step("pads.shift", StepKind::Pads)]);
        session.feed(timed(0, [0x90, 0x3F, 0x7F]));
        for note in 0..8u8 {
            let t = 100 + u64::from(note) * 100;
            session.feed(timed(t, [0x97, note + 8, 0x7F]));
            session.feed(timed(t + 50, [0x97, note + 8, 0x00]));
        }
        assert_eq!(session.distinct_notes(), 8);
        assert_eq!(session.poll(3000 * MS), StepStatus::Done);
        let control = session.finish().controls.remove(0);
        assert_eq!(control.messages.len(), 8);
        assert!(control.messages.iter().all(|m| m.status == 0x97));
    }

    #[test]
    fn set_led_stores_probe_answer() {
        let mut session = CaptureSession::new(vec![step("play", StepKind::Button)]);
        session.feed(timed(0, [0x90, 0x0B, 0x7F]));
        assert_eq!(session.poll(1000 * MS), StepStatus::Done);
        session.set_led("play", true, None);
        let control = session.control("play").unwrap();
        assert_eq!(control.led, Some(true));
        assert_eq!(control.led_values_vary, None);
    }
}
