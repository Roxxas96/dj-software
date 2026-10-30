//! The capture session: a pure state machine walking the checklist steps.
//!
//! The CLI feeds it timestamped messages and polls it with the same clock
//! ([`crate::clock::now_micros`]); it never touches ports or stdin.

use super::checklist::{Step, StepKind};
use super::fixture::{Control, Fixture, Meta, Source, is_release, key_and_value};
use crate::midi::{MidiMessage, TimedMidi};

/// Quiet gap after the last message that completes a button step.
pub const BUTTON_GAP_MICROS: u64 = 400_000;
/// Quiet gap that completes a continuous, jog or pads step.
pub const LONG_GAP_MICROS: u64 = 1_500_000;
/// Ignore entries that an earlier checklist seeded by mistake: the SB3's
/// FILTER knobs (14-bit CC on channel 6), which it also sends when some buttons
/// are pressed. Dropped from `meta.ignored` on resume.
const MISTAKEN_SEEDS: [(u8, u8); 6] = [
    (0xB5, 0x02),
    (0xB5, 0x22),
    (0xB5, 0x04),
    (0xB5, 0x24),
    (0xB5, 0x06),
    (0xB5, 0x26),
];

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

/// What [`CaptureSession::redo`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Redo {
    /// The current step had started recording; it starts over.
    Restarted,
    /// The previously completed or skipped step is current again.
    Reopened,
    /// Nothing to redo (no step done yet and nothing recorded).
    Nothing,
}

/// Suspected noise: CC sources recorded with the same constant value in
/// several steps and never with a varying value. Only a suggestion: the caller
/// asks the user, then calls [`CaptureSession::ignore`] or
/// [`CaptureSession::decline_noise`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Noise {
    pub sources: Vec<Source>,
    /// The steps they were recorded in.
    pub ids: Vec<String>,
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
    /// Sources never recorded.
    ignored: Vec<Source>,
    /// Messages for the user (sources un-ignored, steps reopened by cleanup).
    notices: Vec<String>,
    /// Noise suggestions not yet taken by [`CaptureSession::take_noise`].
    noise: Vec<Noise>,
    /// Sources the user said are not noise; never suggested again.
    declined: Vec<Source>,
    /// The step last captured and its slot before that capture, so `rebind`
    /// and ignores can restore it.
    before: Option<(usize, Slot)>,
    /// Steps to capture before returning to the first missing one (goto,
    /// rebind, redo-only); the first is current.
    focus: Vec<usize>,
    /// Redo-only mode: the session ends when `focus` is empty.
    only: bool,
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
            ignored: Vec::new(),
            notices: Vec::new(),
            noise: Vec::new(),
            declined: Vec::new(),
            before: None,
            focus: Vec::new(),
            only: false,
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
        let continuous = session.continuous_sources();
        let mut prior_ignored = Vec::new();
        for source in &prior.meta.ignored {
            if MISTAKEN_SEEDS.contains(&source.key()) {
                session.notices.push(format!(
                    "no longer ignoring {source}: it was seeded by mistake (SB3 filter knob data)"
                ));
            } else if continuous.iter().any(|c| c.key() == source.key()) {
                session.notices.push(format!(
                    "no longer ignoring {source}: it varies in a knob, fader or jog capture"
                ));
            } else {
                prior_ignored.push(*source);
            }
        }
        session.meta = Meta {
            skipped: Vec::new(),
            ignored: Vec::new(),
            ..prior.meta
        };
        session.ignore(&prior_ignored);
        session.strip_continuous_from_buttons();
        session.detect_noise();
        session.update_cursor();
        session
    }

    /// Adds sources to the ignore list and strips them from everything
    /// recorded. Returns the ids of steps left with no messages; they are asked
    /// again. The step just captured is asked right away and falls back to its
    /// previous recording (goto/redo); in redo-only mode the others are queued.
    pub fn ignore(&mut self, sources: &[Source]) -> Vec<String> {
        for source in sources {
            if !self.ignored.iter().any(|i| i.key() == source.key()) {
                self.ignored.push(*source);
            }
        }
        self.strip_ignored()
    }

    /// True if `msg` comes from an ignored source; such messages are dropped by `feed`.
    pub fn is_ignored(&self, msg: MidiMessage) -> bool {
        self.ignored.iter().any(|s| s.matches(msg))
    }

    /// Noise suggestions since the last call (nothing is ignored yet).
    pub fn take_noise(&mut self) -> Vec<Noise> {
        std::mem::take(&mut self.noise)
    }

    /// Messages for the user since the last call.
    pub fn take_notices(&mut self) -> Vec<String> {
        std::mem::take(&mut self.notices)
    }

    /// Sources seen with a varying value in a continuous or jog capture.
    fn continuous_sources(&self) -> Vec<Source> {
        let mut out: Vec<Source> = Vec::new();
        for slot in &self.slots {
            let Slot::Captured(control) = slot else {
                continue;
            };
            if !matches!(control.kind, StepKind::Continuous | StepKind::Jog) {
                continue;
            }
            for m in control.messages.iter().filter(|m| m.min != m.max) {
                if !out.iter().any(|s| s.key() == m.source().key()) {
                    out.push(m.source());
                }
            }
        }
        out
    }

    /// Strips known continuous sources (knob data such as the SB3's filter
    /// dump) from button and pads controls. Controls left empty are asked
    /// again (the step just captured first; queued in redo-only mode).
    fn strip_continuous_from_buttons(&mut self) {
        let continuous = self.continuous_sources();
        if continuous.is_empty() {
            return;
        }
        let just = self.before.as_ref().map(|(i, _)| *i);
        for i in 0..self.slots.len() {
            let Slot::Captured(control) = &mut self.slots[i] else {
                continue;
            };
            if !matches!(control.kind, StepKind::Button | StepKind::Pads)
                || !control.strip(&continuous)
                || !control.messages.is_empty()
            {
                continue;
            }
            let id = control.id.clone();
            self.slots[i] = Slot::Missing;
            self.history.retain(|&h| h != i);
            if Some(i) == just {
                self.focus.retain(|&f| f != i);
                self.focus.insert(0, i);
            } else if self.only && !self.focus.contains(&i) {
                self.focus.push(i);
            }
            self.notices.push(format!(
                "{id} recorded only knob data; it will be asked again"
            ));
        }
        self.update_cursor();
    }

    /// The user says these are not noise: never suggest them again.
    pub fn decline_noise(&mut self, sources: &[Source]) {
        for source in sources {
            if !self.declined.iter().any(|d| d.key() == source.key()) {
                self.declined.push(*source);
            }
        }
    }

    /// Removes a source from the ignore list. Recordings already stripped are
    /// not restored. Returns false if it was not ignored.
    pub fn unignore(&mut self, source: Source) -> bool {
        let before = self.ignored.len();
        self.ignored.retain(|i| i.key() != source.key());
        self.ignored.len() != before
    }

    fn strip_ignored(&mut self) -> Vec<String> {
        let ignored = self.ignored.clone();
        let just = self.before.as_ref().map(|(i, _)| *i);
        let mut reopened = Vec::new();
        for i in 0..self.slots.len() {
            let Slot::Captured(control) = &mut self.slots[i] else {
                continue;
            };
            if !(control.strip(&ignored) && control.messages.is_empty()) {
                continue;
            }
            reopened.push(i);
            let mut fallback = match &self.before {
                Some((b, slot)) if *b == i => slot.clone(),
                _ => Slot::Missing,
            };
            let old_empty = match &mut fallback {
                Slot::Captured(old) => {
                    old.strip(&ignored);
                    old.messages.is_empty()
                }
                _ => false,
            };
            if old_empty {
                fallback = Slot::Missing;
            }
            if fallback == Slot::Missing {
                self.history.retain(|&h| h != i);
            }
            self.slots[i] = fallback;
        }
        self.extras.retain_mut(|control| {
            control.strip(&ignored);
            !control.messages.is_empty()
        });
        self.buffer
            .retain(|timed| !ignored.iter().any(|s| s.matches(timed.message)));
        for &i in &reopened {
            if Some(i) == just {
                self.focus.retain(|&f| f != i);
                self.focus.insert(0, i);
            } else if self.only && !self.focus.contains(&i) {
                self.focus.push(i);
            }
        }
        self.update_cursor();
        reopened.iter().map(|&i| self.steps[i].id.clone()).collect()
    }

    /// Suggests as noise every CC source recorded with the same constant value
    /// in two or more steps and never with a varying value. Notes are never
    /// suggested: a held key or a one-way move can look constant.
    fn detect_noise(&mut self) {
        // Per CC source: (step id, min, max) of each control it was recorded in.
        type Seen = (String, u8, u8);
        let mut seen: Vec<(Source, Vec<Seen>)> = Vec::new();
        for slot in &self.slots {
            let Slot::Captured(control) = slot else {
                continue;
            };
            for m in control.messages.iter().filter(|m| m.status & 0xF0 == 0xB0) {
                let entry = (control.id.clone(), m.min, m.max);
                match seen.iter_mut().find(|(s, _)| s.key() == m.source().key()) {
                    Some((_, list)) => list.push(entry),
                    None => seen.push((m.source(), vec![entry])),
                }
            }
        }
        let mut found: Vec<Noise> = Vec::new();
        for (source, list) in seen {
            let value = list[0].1;
            let noise = list.len() >= 2
                && list
                    .iter()
                    .all(|&(_, min, max)| min == value && max == value)
                && !self.declined.iter().any(|d| d.key() == source.key())
                && !self.ignored.iter().any(|i| i.key() == source.key());
            if !noise {
                continue;
            }
            let ids: Vec<String> = list.into_iter().map(|(id, _, _)| id).collect();
            match found.iter_mut().find(|n| n.ids == ids) {
                Some(group) => group.sources.push(source),
                None => found.push(Noise {
                    sources: vec![source],
                    ids,
                }),
            }
        }
        self.noise = found;
    }

    /// Rejects the recording just made for step `id`: its slot goes back to
    /// what it was before (missing, or the old recording in goto/redo mode)
    /// and the step is listened to again. Returns false for an unknown id.
    pub fn rebind(&mut self, id: &str) -> bool {
        let Some(i) = self.index_of(id) else {
            return false;
        };
        if let Some((b, slot)) = self.before.take()
            && b == i
        {
            if slot == Slot::Missing {
                self.history.retain(|&h| h != i);
            }
            self.slots[i] = slot;
            self.update_cursor();
        }
        self.goto(id)
    }

    /// Jumps to step `id` to capture it again (its old recording stays until
    /// the new one completes). Afterwards the session returns to the first
    /// missing step. Returns false for an unknown id.
    pub fn goto(&mut self, id: &str) -> bool {
        let Some(i) = self.index_of(id) else {
            return false;
        };
        self.buffer.clear();
        self.focus.retain(|&f| f != i);
        self.focus.insert(0, i);
        true
    }

    /// Redo-only mode: capture exactly these steps, in order, then end. Returns
    /// the first unknown id as an error (and changes nothing).
    pub fn redo_only(&mut self, ids: &[String]) -> Result<(), String> {
        let mut focus = Vec::new();
        for id in ids {
            let i = self.index_of(id).ok_or_else(|| id.clone())?;
            if !focus.contains(&i) {
                focus.push(i);
            }
        }
        self.buffer.clear();
        self.focus = focus;
        self.only = true;
        Ok(())
    }

    /// Marks every source recorded for step `id` as noise: they join the
    /// ignore list and are stripped everywhere, and step `id` is listened to
    /// again (keeping its previous recording, if any, until then). Returns the
    /// sources.
    pub fn mark_noise(&mut self, id: &str) -> Vec<Source> {
        let sources: Vec<Source> = self
            .control(id)
            .map(|c| c.messages.iter().map(|m| m.source()).collect())
            .unwrap_or_default();
        self.ignore(&sources);
        self.goto(id);
        sources
    }

    /// Step ids close to `query`: those containing it (ignoring case), else
    /// those with the same first segment (e.g. `deck1`).
    pub fn find_steps(&self, query: &str) -> Vec<String> {
        let query = query.trim().to_lowercase();
        if query.is_empty() {
            return Vec::new();
        }
        let containing: Vec<String> = self
            .steps
            .iter()
            .filter(|s| s.id.to_lowercase().contains(&query))
            .map(|s| s.id.clone())
            .collect();
        if !containing.is_empty() {
            return containing;
        }
        let head = query.split('.').next().unwrap_or_default();
        self.steps
            .iter()
            .filter(|s| s.id.to_lowercase().split('.').next() == Some(head))
            .map(|s| s.id.clone())
            .collect()
    }

    /// Sets the metadata; `skipped` and `ignored` are kept by the session.
    pub fn set_meta(&mut self, meta: Meta) {
        self.meta = Meta {
            skipped: Vec::new(),
            ignored: Vec::new(),
            ..meta
        };
    }

    /// The step being captured: the first step neither captured nor skipped.
    pub fn current(&self) -> Option<&Step> {
        self.current_index().map(|i| &self.steps[i])
    }

    /// `(1-based number of the current step, total steps)`; the number is
    /// `total + 1` when every step is done.
    pub fn position(&self) -> (usize, usize) {
        let total = self.steps.len();
        (self.current_index().unwrap_or(total) + 1, total)
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

    /// True if `msg` belongs to the held SHIFT of the current SHIFT step.
    pub fn is_modifier(&self, msg: MidiMessage) -> bool {
        let (status, data1, _) = key_and_value(msg);
        matches!(
            msg,
            MidiMessage::NoteOn { .. } | MidiMessage::NoteOff { .. }
        ) && self.modifier_pairs().contains(&(status, data1))
    }

    /// The `(status, data1)` note pairs left out of a SHIFT step.
    ///
    /// When the side's SHIFT button was captured (`shift_control`, one id per
    /// layer), they are exactly the union of those controls' notes, whatever the order or whether SHIFT was held before
    /// the step. Otherwise a key counts as SHIFT only if it is still held when
    /// another key is pressed; until then, the earliest key still held is set
    /// aside so SHIFT held alone never completes the step.
    fn modifier_pairs(&self) -> Vec<(u8, u8)> {
        let Some(step) = self.current().filter(|s| s.shift) else {
            return Vec::new();
        };
        let known: Vec<(u8, u8)> = step
            .shift_control
            .iter()
            .filter_map(|id| self.control(id))
            .flat_map(|shift| &shift.messages)
            .filter(|m| m.status & 0xF0 == 0x90)
            .map(|m| (m.status, m.data1))
            .collect();
        if !known.is_empty() {
            return known;
        }
        let mut held: Vec<(u8, u8)> = Vec::new();
        for timed in &self.buffer {
            let (status, data1, _) = key_and_value(timed.message);
            let pair = (status, data1);
            match timed.message {
                MidiMessage::NoteOn { .. } => {
                    if let Some(&modifier) = held.iter().find(|&&p| p != pair) {
                        return vec![modifier];
                    }
                    if !held.contains(&pair) {
                        held.push(pair);
                    }
                }
                MidiMessage::NoteOff { .. } => held.retain(|&p| p != pair),
                _ => {}
            }
        }
        held.first().copied().into_iter().collect()
    }

    /// The current step's messages that count: without the SHIFT modifier and
    /// without pairs whose first message was a release.
    fn relevant(&self) -> Vec<TimedMidi> {
        let modifiers = self.modifier_pairs();
        let mut released_first: Vec<(u8, u8)> = Vec::new();
        let mut seen: Vec<(u8, u8)> = Vec::new();
        let mut out = Vec::new();
        for timed in &self.buffer {
            let (status, data1, _) = key_and_value(timed.message);
            if modifiers.contains(&(status, data1)) {
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
        if self.current_index().is_some() && !self.is_ignored(msg.message) {
            self.buffer.push(msg);
        }
    }

    /// Completes the current step once its quiet gap has passed (and, for pads,
    /// 8 distinct notes were seen). Returns `Done` once, then moves on.
    pub fn poll(&mut self, now_micros: u64) -> StepStatus {
        let Some(i) = self.current_index() else {
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
        match self.current_index() {
            Some(i) if !self.relevant().is_empty() => {
                self.commit(i);
                true
            }
            _ => false,
        }
    }

    /// Marks the current step as skipped and moves on. A step reached with
    /// `goto`, `rebind` or `redo_only` keeps its previous recording instead,
    /// if it has one.
    pub fn skip(&mut self) {
        if !self.focus.is_empty() {
            self.buffer.clear();
            let i = self.focus.remove(0);
            // Never captured: skipping marks it skipped like any other step.
            if self.slots[i] == Slot::Missing {
                self.slots[i] = Slot::Skipped;
                self.history.push(i);
            }
            self.before = None;
            self.update_cursor();
            return;
        }
        if let Some(i) = self.cursor {
            self.buffer.clear();
            self.slots[i] = Slot::Skipped;
            self.history.push(i);
            self.update_cursor();
        }
    }

    /// If the current step has started recording, starts it over. Otherwise
    /// reopens the previously completed or skipped step, which becomes current
    /// (not while on a step reached with `goto` or `redo_only`).
    pub fn redo(&mut self) -> Redo {
        if !self.relevant().is_empty() {
            self.buffer.clear();
            return Redo::Restarted;
        }
        self.buffer.clear();
        if !self.focus.is_empty() || self.only {
            return Redo::Nothing;
        }
        let previous = self.history.pop().or_else(|| {
            let end = self.cursor.unwrap_or(self.slots.len());
            (0..end).rev().find(|&i| self.slots[i] != Slot::Missing)
        });
        if let Some(i) = previous {
            self.slots[i] = Slot::Missing;
            self.history.retain(|&h| h != i);
            self.update_cursor();
            return Redo::Reopened;
        }
        Redo::Nothing
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
        meta.ignored = self.ignored.clone();
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
        self.before = Some((i, self.slots[i].clone()));
        self.slots[i] = Slot::Captured(control);
        self.history.push(i);
        if self.focus.first() == Some(&i) {
            self.focus.remove(0);
        }
        self.update_cursor();
        self.strip_continuous_from_buttons();
        self.detect_noise();
    }

    fn index_of(&self, id: &str) -> Option<usize> {
        self.steps.iter().position(|s| s.id == id)
    }

    /// The current step: the first focused step, else (outside redo-only mode)
    /// the first missing one.
    fn current_index(&self) -> Option<usize> {
        match self.focus.first() {
            Some(&i) => Some(i),
            None if self.only => None,
            None => self.cursor,
        }
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
            shift_control: Vec::new(),
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
        assert!(session.is_modifier(MidiMessage::NoteOn {
            channel: 0,
            note: 0x3F,
            velocity: 127
        }));
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

    /// A SHIFT button step (note 0x3F) followed by `next`, with SHIFT captured.
    fn after_shift_capture(next: Step) -> CaptureSession {
        let mut session = CaptureSession::new(vec![step("deck1.shift", StepKind::Button), next]);
        session.feed(timed(0, [0x90, 0x3F, 0x7F]));
        session.feed(timed(50, [0x90, 0x3F, 0x00]));
        assert_eq!(session.poll(1000 * MS), StepStatus::Done);
        session
    }

    fn known_shift_step(id: &str, kind: StepKind) -> Step {
        Step {
            shift_control: vec!["deck1.shift".to_owned()],
            ..shift_step(id, kind)
        }
    }

    #[test]
    fn known_shift_excluded_when_control_pressed_first() {
        let mut session = after_shift_capture(known_shift_step("play.shift", StepKind::Button));
        session.feed(timed(2000, [0x90, 0x47, 0x7F]));
        session.feed(timed(2010, [0x90, 0x3F, 0x7F]));
        session.feed(timed(2050, [0x90, 0x47, 0x00]));
        session.feed(timed(2100, [0x90, 0x3F, 0x00]));
        assert!(session.is_modifier(MidiMessage::NoteOn {
            channel: 0,
            note: 0x3F,
            velocity: 127
        }));
        assert_eq!(session.poll(2451 * MS), StepStatus::Done);
        let control = session.finish().controls.remove(1);
        assert_eq!(control.messages.len(), 1, "{control:?}");
        assert_eq!(control.messages[0].data1, 0x47);
    }

    #[test]
    fn known_shift_held_before_step_records_the_control() {
        // SHIFT was pressed before listening (dropped), released at the end.
        let mut session = after_shift_capture(known_shift_step("play.shift", StepKind::Button));
        session.feed(timed(2000, [0x90, 0x47, 0x7F]));
        session.feed(timed(2050, [0x90, 0x47, 0x00]));
        session.feed(timed(2300, [0x90, 0x3F, 0x00]));
        assert_eq!(session.poll(2451 * MS), StepStatus::Done);
        let control = session.finish().controls.remove(1);
        assert_eq!(control.messages.len(), 1, "{control:?}");
        assert_eq!(control.messages[0].data1, 0x47);
    }

    #[test]
    fn held_shift_pads_with_shift_held_before_step_record_eight() {
        let mut session = after_shift_capture(known_shift_step("pads.shift", StepKind::Pads));
        for note in 0..8u8 {
            let t = 2000 + u64::from(note) * 100;
            session.feed(timed(t, [0x97, note + 8, 0x7F]));
            session.feed(timed(t + 50, [0x97, note + 8, 0x00]));
        }
        session.feed(timed(2900, [0x90, 0x3F, 0x00]));
        assert_eq!(session.distinct_notes(), 8);
        assert_eq!(session.poll(4500 * MS), StepStatus::Done);
        let control = session.finish().controls.remove(1);
        assert_eq!(control.messages.len(), 8);
        assert!(control.messages.iter().all(|m| m.status == 0x97));
    }

    #[test]
    fn unknown_shift_falls_back_to_overlap() {
        // SHIFT skipped: a key counts as SHIFT only while held during another press.
        let mut session = CaptureSession::new(vec![shift_step("play.shift", StepKind::Button)]);
        // Held before the step: the control alone completes the step.
        session.feed(timed(0, [0x90, 0x47, 0x7F]));
        session.feed(timed(50, [0x90, 0x47, 0x00]));
        session.feed(timed(100, [0x90, 0x3F, 0x00]));
        assert_eq!(session.poll(1000 * MS), StepStatus::Done);
        let control = session.finish().controls.remove(0);
        assert_eq!(control.messages.len(), 1);
        assert_eq!(control.messages[0].data1, 0x47);
    }

    #[test]
    fn r_restarts_current_step_once_recording() {
        let mut session = CaptureSession::new(vec![
            step("a", StepKind::Button),
            step("b", StepKind::Continuous),
        ]);
        session.feed(timed(0, [0x90, 1, 127]));
        assert_eq!(session.poll(1000 * MS), StepStatus::Done);
        session.feed(timed(1000, [0xB0, 0x13, 10]));
        assert_eq!(session.redo(), Redo::Restarted);
        assert_eq!(session.current().unwrap().id, "b");
        assert_eq!(session.poll(5000 * MS), StepStatus::Waiting);
        assert_eq!(session.redo(), Redo::Reopened);
        assert_eq!(session.current().unwrap().id, "a");
        assert_eq!(session.redo(), Redo::Nothing);
    }

    #[test]
    fn shift_control_list_excludes_either_layer() {
        // LEFT SHIFT sends note 0x3F on deck 1 and on channel 3 for deck 3.
        let mut session = CaptureSession::new(vec![
            step("deck1.shift", StepKind::Button),
            step("deck3.shift", StepKind::Button),
            Step {
                shift_control: vec!["deck1.shift".to_owned(), "deck3.shift".to_owned()],
                ..shift_step("load.left.shift", StepKind::Button)
            },
        ]);
        session.feed(timed(0, [0x90, 0x3F, 0x7F]));
        assert_eq!(session.poll(1000 * MS), StepStatus::Done);
        session.feed(timed(1000, [0x92, 0x3F, 0x7F]));
        assert_eq!(session.poll(2000 * MS), StepStatus::Done);
        // The side is on the deck-3 layer: its SHIFT (channel 3) is excluded.
        session.feed(timed(3000, [0x92, 0x3F, 0x7F]));
        session.feed(timed(3010, [0x96, 0x46, 0x7F]));
        session.feed(timed(3050, [0x96, 0x46, 0x00]));
        session.feed(timed(3100, [0x92, 0x3F, 0x00]));
        assert_eq!(session.poll(4000 * MS), StepStatus::Done);
        let control = session.control("load.left.shift").unwrap();
        assert_eq!(control.messages.len(), 1, "{control:?}");
        assert_eq!(control.messages[0].status, 0x96);
    }

    fn burst_source(data1: u8) -> Source {
        Source {
            status: 0xB6,
            data1,
        }
    }

    /// A burst of constant CC values (like the SB3's filter dump) at `ms`.
    fn feed_burst(session: &mut CaptureSession, ms: u64) {
        session.feed(timed(ms, [0xB6, 0x02, 0x32]));
        session.feed(timed(ms + 1, [0xB6, 0x22, 0x1A]));
    }

    #[test]
    fn ignored_source_never_recorded_and_does_not_start_step() {
        let mut session = CaptureSession::new(vec![step("key_lock", StepKind::Button)]);
        session.ignore(&[burst_source(0x02), burst_source(0x22)]);
        assert!(session.is_ignored(MidiMessage::ControlChange {
            channel: 6,
            controller: 2,
            value: 50
        }));
        feed_burst(&mut session, 0);
        assert_eq!(session.poll(5000 * MS), StepStatus::Waiting);
        assert!(!session.accept());
        assert_eq!(session.redo(), Redo::Nothing);
        session.feed(timed(5000, [0xB0, 0x1A, 0x7F]));
        feed_burst(&mut session, 5010);
        session.feed(timed(5050, [0xB0, 0x1A, 0x00]));
        assert_eq!(session.poll(6000 * MS), StepStatus::Done);
        let fixture = session.finish();
        assert_eq!(fixture.controls[0].messages.len(), 1);
        assert_eq!(fixture.controls[0].messages[0].data1, 0x1A);
        assert_eq!(fixture.meta.ignored.len(), 2);
    }

    #[test]
    fn noise_in_two_steps_is_suggested_then_stripped_on_confirm() {
        let mut session = CaptureSession::new(vec![
            step("key_lock", StepKind::Button),
            step("vinyl", StepKind::Button),
            step("sync", StepKind::Button),
        ]);
        session.feed(timed(0, [0xB0, 0x1A, 0x7F]));
        session.feed(timed(50, [0xB0, 0x1A, 0x00]));
        feed_burst(&mut session, 60);
        assert_eq!(session.poll(1000 * MS), StepStatus::Done);
        // One step alone is not enough.
        assert!(session.take_noise().is_empty());
        assert_eq!(session.control("key_lock").unwrap().messages.len(), 3);

        session.feed(timed(1000, [0xB0, 0x17, 0x7F]));
        feed_burst(&mut session, 1010);
        session.feed(timed(1050, [0xB0, 0x17, 0x00]));
        assert_eq!(session.poll(2000 * MS), StepStatus::Done);
        assert_eq!(
            session.take_noise(),
            [Noise {
                sources: vec![burst_source(0x02), burst_source(0x22)],
                ids: vec!["key_lock".to_owned(), "vinyl".to_owned()],
            }]
        );
        assert!(session.take_noise().is_empty());
        // Only a suggestion: nothing is stripped until the user confirms.
        assert_eq!(session.control("vinyl").unwrap().messages.len(), 3);
        assert!(session.fixture().meta.ignored.is_empty());
        let reopened = session.ignore(&[burst_source(0x02), burst_source(0x22)]);
        assert!(reopened.is_empty());
        for id in ["key_lock", "vinyl"] {
            let control = session.control(id).unwrap();
            assert_eq!(control.messages.len(), 1, "{control:?}");
            assert_eq!(control.cc14, None);
        }
        // From now on the burst is ignored outright.
        feed_burst(&mut session, 3000);
        assert_eq!(session.poll(9000 * MS), StepStatus::Waiting);
        let fixture = session.fixture();
        assert_eq!(
            fixture.meta.ignored,
            [burst_source(0x02), burst_source(0x22)]
        );
    }

    #[test]
    fn noise_only_control_is_asked_again_once_detected() {
        let mut session = CaptureSession::new(vec![
            step("key_lock", StepKind::Button),
            step("vinyl", StepKind::Button),
        ]);
        // The burst arrived alone during key_lock and was taken for it.
        feed_burst(&mut session, 0);
        assert_eq!(session.poll(1000 * MS), StepStatus::Done);
        session.feed(timed(1000, [0xB0, 0x17, 0x7F]));
        feed_burst(&mut session, 1010);
        session.feed(timed(1050, [0xB0, 0x17, 0x00]));
        assert_eq!(session.poll(2000 * MS), StepStatus::Done);
        let noise = session.take_noise();
        assert_eq!(noise.len(), 1);
        assert_eq!(session.ignore(&noise[0].sources), ["key_lock"]);
        assert_eq!(session.current().unwrap().id, "key_lock");
        assert!(session.control("key_lock").is_none());
        assert_eq!(session.control("vinyl").unwrap().messages.len(), 1);
    }

    #[test]
    fn resume_strips_ignored_and_reasks_noise_only_controls() {
        let steps = vec![
            step("shift", StepKind::Button),
            step("key_lock", StepKind::Button),
            step("vinyl", StepKind::Button),
        ];
        let mut prior = Fixture::new(Meta {
            ignored: vec![burst_source(0x22)],
            ..Meta::default()
        });
        prior.controls.push(Control::from_messages(
            "shift",
            StepKind::Button,
            &[timed(0, [0x90, 0x3F, 0x7F]), timed(1, [0x90, 0x3F, 0x00])],
        ));
        // Like the owner's capture: key_lock holds only the burst.
        prior.controls.push(Control::from_messages(
            "key_lock",
            StepKind::Button,
            &[timed(0, [0xB6, 0x02, 0x32]), timed(1, [0xB6, 0x22, 0x1A])],
        ));
        prior.controls.push(Control::from_messages(
            "vinyl",
            StepKind::Button,
            &[timed(0, [0xB0, 0x17, 0x7F]), timed(1, [0xB6, 0x22, 0x1A])],
        ));
        let mut session = CaptureSession::resume(steps, prior);
        // Fixture ignores apply on load; key_lock still has B5 02.
        assert_eq!(session.current(), None);
        assert_eq!(session.control("vinyl").unwrap().messages.len(), 1);
        // Checklist ignores applied after resume reopen the noise-only control.
        let reopened = session.ignore(&[burst_source(0x02)]);
        assert_eq!(reopened, ["key_lock"]);
        assert_eq!(session.current().unwrap().id, "key_lock");
        assert_eq!(session.done_count(), 2);
        assert_eq!(
            session.fixture().meta.ignored,
            [burst_source(0x22), burst_source(0x02)]
        );
    }

    fn capture_note(session: &mut CaptureSession, ms: u64, note: u8) {
        session.feed(timed(ms, [0x90, note, 0x7F]));
        session.feed(timed(ms + 50, [0x90, note, 0x00]));
        assert_eq!(session.poll((ms + 1000) * MS), StepStatus::Done);
    }

    #[test]
    fn review_rebind_and_mark_noise() {
        let mut session = CaptureSession::new(vec![
            step("a", StepKind::Button),
            step("b", StepKind::Button),
        ]);
        capture_note(&mut session, 0, 1);
        // Rebind: listen to a again; the new recording replaces the old one.
        assert!(session.goto("a"));
        assert_eq!(session.current().unwrap().id, "a");
        capture_note(&mut session, 2000, 2);
        assert_eq!(session.control("a").unwrap().messages[0].data1, 2);
        assert_eq!(session.current().unwrap().id, "b");

        // Mark noise: b caught only the burst.
        feed_burst(&mut session, 4000);
        assert_eq!(session.poll(5000 * MS), StepStatus::Done);
        let marked = session.mark_noise("b");
        assert_eq!(marked, [burst_source(0x02), burst_source(0x22)]);
        assert!(session.control("b").is_none());
        assert_eq!(session.current().unwrap().id, "b");
        feed_burst(&mut session, 6000);
        assert_eq!(session.poll(9000 * MS), StepStatus::Waiting);
        capture_note(&mut session, 9000, 3);
        assert_eq!(session.current(), None);
        let fixture = session.finish();
        assert_eq!(fixture.meta.ignored, marked);
        assert_eq!(fixture.controls[1].messages[0].data1, 3);
    }

    #[test]
    fn goto_returns_to_the_original_step() {
        let mut session = CaptureSession::new(vec![
            step("a", StepKind::Button),
            step("b", StepKind::Button),
            step("c", StepKind::Button),
            step("d", StepKind::Button),
        ]);
        capture_note(&mut session, 0, 1);
        capture_note(&mut session, 2000, 2);
        assert_eq!(session.current().unwrap().id, "c");
        assert!(!session.goto("nope"));

        // Back to an earlier step, then return to c.
        assert!(session.goto("a"));
        assert_eq!(session.position(), (1, 4));
        capture_note(&mut session, 4000, 11);
        assert_eq!(session.current().unwrap().id, "c");

        // Skipping a step you jumped to keeps its old recording.
        assert!(session.goto("b"));
        session.skip();
        assert_eq!(session.current().unwrap().id, "c");
        assert_eq!(session.control("b").unwrap().messages[0].data1, 2);
        assert!(session.fixture().meta.skipped.is_empty());

        // Ahead to a later step, then return to c; d is not asked again.
        assert!(session.goto("d"));
        capture_note(&mut session, 6000, 4);
        assert_eq!(session.current().unwrap().id, "c");
        capture_note(&mut session, 8000, 3);
        assert_eq!(session.current(), None);
        let ids: Vec<u8> = session
            .finish()
            .controls
            .iter()
            .map(|c| c.messages[0].data1)
            .collect();
        assert_eq!(ids, [11, 2, 3, 4]);
    }

    #[test]
    fn redo_only_touches_listed_steps() {
        let steps = vec![
            step("a", StepKind::Button),
            step("b", StepKind::Button),
            step("c", StepKind::Button),
            step("d", StepKind::Button),
        ];
        let mut first = CaptureSession::new(steps.clone());
        for (i, note) in [1u8, 2, 3].into_iter().enumerate() {
            capture_note(&mut first, i as u64 * 2000, note);
        }
        let prior = first.finish();

        let mut session = CaptureSession::resume(steps.clone(), prior.clone());
        assert_eq!(
            session.redo_only(&["c".to_owned(), "zzz".to_owned()]),
            Err("zzz".to_owned())
        );
        let mut session2 = CaptureSession::resume(steps, prior.clone());
        session2
            .redo_only(&["c".to_owned(), "a".to_owned()])
            .unwrap();
        assert_eq!(session2.current().unwrap().id, "c");
        capture_note(&mut session2, 0, 30);
        assert_eq!(session2.current().unwrap().id, "a");
        session2.skip();
        // d is still missing but not listed: the session is over.
        assert_eq!(session2.current(), None);
        let fixture = session2.finish();
        assert_eq!(fixture.controls[0], prior.controls[0]);
        assert_eq!(fixture.controls[1], prior.controls[1]);
        assert_eq!(fixture.controls[2].messages[0].data1, 30);
        assert!(fixture.meta.skipped.is_empty());
        let _ = &mut session;
    }

    #[test]
    fn find_steps_suggests_close_ids() {
        let session = CaptureSession::new(vec![
            step("deck1.key_lock", StepKind::Button),
            step("deck1.play", StepKind::Button),
            step("deck3.key_lock", StepKind::Button),
        ]);
        assert_eq!(
            session.find_steps("KEY_LOCK"),
            ["deck1.key_lock", "deck3.key_lock"]
        );
        assert_eq!(
            session.find_steps("deck1.keylock"),
            ["deck1.key_lock", "deck1.play"]
        );
        assert!(session.find_steps("zzz").is_empty());
    }

    /// Captures one button step: a press of `note`, plus `extra` messages.
    fn capture_with(session: &mut CaptureSession, ms: u64, note: u8, extra: &[[u8; 3]]) {
        // A CC button, so other CC sources are kept (a note button drops them).
        session.feed(timed(ms, [0xB0, note, 0x7F]));
        for (k, bytes) in extra.iter().enumerate() {
            session.feed(timed(ms + 10 + k as u64, *bytes));
        }
        session.feed(timed(ms + 50, [0xB0, note, 0x00]));
        assert_eq!(session.poll((ms + 1000) * MS), StepStatus::Done);
    }

    fn three_buttons() -> CaptureSession {
        CaptureSession::new(vec![
            step("a", StepKind::Button),
            step("b", StepKind::Button),
            step("c", StepKind::Button),
        ])
    }

    #[test]
    fn noise_suggestions_are_narrow() {
        // CC 0x10 varies in a (a real control), constant in b and c.
        let mut session = three_buttons();
        capture_with(&mut session, 0, 1, &[[0xB0, 0x10, 0], [0xB0, 0x10, 127]]);
        capture_with(&mut session, 2000, 2, &[[0xB0, 0x10, 64]]);
        capture_with(&mut session, 4000, 3, &[[0xB0, 0x10, 64]]);
        assert!(session.take_noise().is_empty());

        // Different constant values in two steps.
        let mut session = three_buttons();
        capture_with(&mut session, 0, 1, &[[0xB6, 0x02, 50]]);
        capture_with(&mut session, 2000, 2, &[[0xB6, 0x02, 60]]);
        assert!(session.take_noise().is_empty());

        // A note held through two steps (e.g. jog touch) is never suggested.
        let mut session = three_buttons();
        capture_with(&mut session, 0, 1, &[[0x90, 0x36, 0x7F]]);
        capture_with(&mut session, 2000, 2, &[[0x90, 0x36, 0x7F]]);
        assert!(session.take_noise().is_empty());

        // Declined sources are not suggested again.
        let mut session = three_buttons();
        capture_with(&mut session, 0, 1, &[[0xB6, 0x02, 50]]);
        capture_with(&mut session, 2000, 2, &[[0xB6, 0x02, 50]]);
        let noise = session.take_noise();
        assert_eq!(noise[0].sources, [burst_source(0x02)]);
        session.decline_noise(&noise[0].sources);
        capture_with(&mut session, 4000, 3, &[[0xB6, 0x02, 50]]);
        assert!(session.take_noise().is_empty());
        assert_eq!(session.control("c").unwrap().messages.len(), 2);
    }

    #[test]
    fn rebind_restores_the_slot_before_listening_again() {
        // First capture rejected: the step is missing again, nothing is saved.
        let mut session = three_buttons();
        capture_with(&mut session, 0, 1, &[]);
        assert!(session.rebind("a"));
        assert!(session.control("a").is_none());
        assert_eq!(session.current().unwrap().id, "a");
        assert!(session.fixture().controls.is_empty());
        // Skipping after r marks it skipped.
        session.skip();
        assert_eq!(session.fixture().meta.skipped, ["a"]);
        assert_eq!(session.current().unwrap().id, "b");

        // Re-capture via goto, rejected: the old recording comes back, and
        // skipping keeps it.
        let mut session = three_buttons();
        capture_with(&mut session, 0, 1, &[]);
        capture_with(&mut session, 2000, 2, &[]);
        assert!(session.goto("a"));
        capture_with(&mut session, 4000, 9, &[]);
        assert_eq!(session.control("a").unwrap().messages[0].data1, 9);
        assert!(session.rebind("a"));
        assert_eq!(session.control("a").unwrap().messages[0].data1, 1);
        assert_eq!(session.current().unwrap().id, "a");
        session.skip();
        assert_eq!(session.control("a").unwrap().messages[0].data1, 1);
        assert_eq!(session.current().unwrap().id, "c");
    }

    #[test]
    fn mark_noise_in_redo_mode_keeps_the_old_control() {
        let mut first = three_buttons();
        capture_with(&mut first, 0, 1, &[]);
        capture_with(&mut first, 2000, 2, &[]);
        capture_with(&mut first, 4000, 3, &[]);
        let mut session = CaptureSession::resume(
            vec![
                step("a", StepKind::Button),
                step("b", StepKind::Button),
                step("c", StepKind::Button),
            ],
            first.finish(),
        );
        session.redo_only(&["b".to_owned()]).unwrap();
        // The new capture of b caught only the burst; the user marks it noise.
        feed_burst(&mut session, 0);
        assert_eq!(session.poll(1000 * MS), StepStatus::Done);
        let marked = session.mark_noise("b");
        assert_eq!(marked, [burst_source(0x02), burst_source(0x22)]);
        assert_eq!(session.control("b").unwrap().messages[0].data1, 2);
        assert_eq!(session.current().unwrap().id, "b");
        session.skip();
        assert_eq!(session.current(), None);
        assert_eq!(session.finish().controls[1].messages[0].data1, 2);
    }

    #[test]
    fn redo_mode_queues_steps_reopened_by_ignores() {
        let steps = vec![
            step("a", StepKind::Button),
            step("b", StepKind::Button),
            step("c", StepKind::Button),
        ];
        let mut prior = Fixture::new(Meta::default());
        prior.controls.push(Control::from_messages(
            "a",
            StepKind::Button,
            &[timed(0, [0xB6, 0x02, 0x32])],
        ));
        prior.controls.push(Control::from_messages(
            "b",
            StepKind::Button,
            &[timed(0, [0x90, 2, 0x7F])],
        ));
        prior.controls.push(Control::from_messages(
            "c",
            StepKind::Button,
            &[timed(0, [0x90, 3, 0x7F])],
        ));
        let mut session = CaptureSession::resume(steps, prior);
        session.redo_only(&["c".to_owned()]).unwrap();
        assert_eq!(session.ignore(&[burst_source(0x02)]), ["a"]);
        assert_eq!(session.current().unwrap().id, "c");
        capture_note(&mut session, 0, 30);
        // a was emptied by the ignore: asked in this redo run, not left missing.
        assert_eq!(session.current().unwrap().id, "a");
        capture_note(&mut session, 2000, 10);
        assert_eq!(session.current(), None);
    }

    #[test]
    fn unignore_records_the_source_again() {
        let mut session = CaptureSession::new(vec![step("a", StepKind::Button)]);
        session.ignore(&[burst_source(0x02)]);
        assert!(session.unignore(burst_source(0x02)));
        assert!(!session.unignore(burst_source(0x02)));
        feed_burst(&mut session, 0);
        assert_eq!(session.poll(1000 * MS), StepStatus::Done);
        assert_eq!(session.control("a").unwrap().messages.len(), 2);
        assert!(session.fixture().meta.ignored.is_empty());
    }

    #[test]
    fn knob_data_is_stripped_from_buttons_and_never_suggested() {
        let mut session = CaptureSession::new(vec![
            step("deck1.x", StepKind::Button),
            step("channel1.filter", StepKind::Continuous),
            step("deck1.y", StepKind::Button),
            step("deck1.z", StepKind::Button),
        ]);
        // A CC-only button caught the filter dump (B5 02 = 50) and its own CC.
        session.feed(timed(0, [0xB0, 0x40, 0x7F]));
        session.feed(timed(1, [0xB5, 0x02, 0x32]));
        session.feed(timed(50, [0xB0, 0x40, 0x00]));
        assert_eq!(session.poll(1000 * MS), StepStatus::Done);
        // The filter knob moves B5 02: now known continuous.
        for (k, v) in [10u8, 0, 64, 127].into_iter().enumerate() {
            session.feed(timed(1000 + k as u64 * 10, [0xB5, 0x02, v]));
        }
        assert_eq!(session.poll(3000 * MS), StepStatus::Done);
        assert_eq!(session.control("deck1.x").unwrap().messages.len(), 1);
        assert!(session.take_notices().is_empty());
        // Another CC-only button that caught only the dump: emptied, asked again.
        session.feed(timed(4000, [0xB5, 0x02, 0x32]));
        assert_eq!(session.poll(5000 * MS), StepStatus::Done);
        assert!(session.control("deck1.y").is_none());
        assert_eq!(session.current().unwrap().id, "deck1.y");
        assert_eq!(session.take_notices().len(), 1);
        // The knob keeps its data, and the constant 50 seen in two buttons is
        // not suggested as noise (it varies in the knob).
        assert_eq!(
            session.control("channel1.filter").unwrap().messages[0].max,
            127
        );
        assert!(session.take_noise().is_empty());
    }

    #[test]
    fn resume_strips_knob_data_and_unignores_knob_sources() {
        let steps = vec![
            step("deck1.x", StepKind::Button),
            step("channel1.filter", StepKind::Continuous),
            step("channel1.trim", StepKind::Continuous),
        ];
        let seeded = [0x02u8, 0x22, 0x04, 0x24, 0x06, 0x26].map(|data1| Source {
            status: 0xB5,
            data1,
        });
        let mut prior = Fixture::new(Meta {
            ignored: [
                &seeded[..],
                &[
                    Source {
                        status: 0xB0,
                        data1: 0x13,
                    },
                    Source {
                        status: 0xB6,
                        data1: 0x50,
                    },
                ],
            ]
            .concat(),
            ..Meta::default()
        });
        // Recorded before this fix: a CC-only button that holds knob data.
        prior.controls.push(Control::from_messages(
            "deck1.x",
            StepKind::Button,
            &[timed(0, [0xB0, 0x14, 0x32])],
        ));
        prior.controls.push(Control::from_messages(
            "channel1.filter",
            StepKind::Continuous,
            &[timed(0, [0xB0, 0x13, 0x00]), timed(1, [0xB0, 0x13, 0x7F])],
        ));
        prior.controls.push(Control::from_messages(
            "channel1.trim",
            StepKind::Continuous,
            &[timed(0, [0xB0, 0x14, 0x00]), timed(1, [0xB0, 0x14, 0x7F])],
        ));
        let mut session = CaptureSession::resume(steps, prior);
        // Only the unrelated source stays ignored.
        assert_eq!(
            session.fixture().meta.ignored,
            [Source {
                status: 0xB6,
                data1: 0x50
            }]
        );
        let notices = session.take_notices();
        assert_eq!(notices.len(), 8, "{notices:?}");
        assert!(notices.iter().any(|n| n.contains("B0 13")));
        assert!(
            notices
                .iter()
                .any(|n| n.contains("B5 02") && n.contains("mistake"))
        );
        // deck1.x held only trim data: asked again; the knobs are untouched.
        assert_eq!(session.current().unwrap().id, "deck1.x");
        assert_eq!(
            session.control("channel1.filter").unwrap().messages.len(),
            1
        );
        assert_eq!(session.control("channel1.trim").unwrap().messages.len(), 1);
    }
}
