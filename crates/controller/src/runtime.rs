//! The controller runtime: ties the resolver, actions, feedback and ports together on the
//! control thread, and follows the controller being unplugged and plugged back in.

use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use actions::{Action, ApplyResult, ControlState, Input, Side, ValueTarget, apply};
use engine::{DeckId, EngineController};

use crate::mapping::{InitAction, Mapping, Resolved, Resolver};
use crate::ports::{
    InputPort, MidiOut, OutputPort, PortError, find_match, input_port_names, open_input,
    open_output,
};
use crate::{Feedback, MidiMessage, TimedMidi, clock};

/// What one [`Runtime::handle_batch`] did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BatchStats {
    /// Actions that took effect.
    pub applied: u32,
    /// Absolute values dropped because a later value for the same target replaced them.
    pub coalesced: u32,
    /// Actions dropped because the engine's command queue was full.
    pub queue_full: u32,
}

/// An absolute value whose engine command waits for the end of its run.
#[derive(Debug, Clone, Copy)]
struct Pending {
    target: ValueTarget,
    action: Action,
    value: f32,
    /// The state's value before the run, restored if the command cannot be sent.
    original: f32,
}

/// One controller's mapping at work: resolves its messages, applies the actions to the
/// engine and drives its LEDs. Everything runs on the control thread.
#[derive(Debug)]
pub struct Runtime {
    init: Vec<InitAction>,
    resolver: Resolver,
    state: ControlState,
    feedback: Feedback,
    /// Whether the controller is attached; LEDs are only sent while it is.
    attached: bool,
    /// Absolute values of the current run, in order of their last occurrence.
    pending: Vec<Pending>,
    last: Vec<(MidiMessage, Resolved)>,
}

impl Runtime {
    pub fn new(mapping: Mapping) -> Self {
        Self {
            resolver: Resolver::new(&mapping),
            feedback: Feedback::new(&mapping),
            init: mapping.init,
            state: ControlState::default(),
            attached: false,
            pending: Vec::new(),
            last: Vec::new(),
        }
    }

    /// The control state the mapping drives (side decks, SHIFT, the values sent).
    pub fn state(&self) -> &ControlState {
        &self.state
    }

    /// The controller attached: applies the `[[init]]` actions, arms soft takeover for
    /// every target and sends every LED.
    ///
    /// An init action the engine's queue has no room for is skipped.
    pub fn attach(&mut self, engine: &mut EngineController, out: &mut dyn MidiOut) {
        for init in &self.init {
            apply(
                &init.action,
                Input::Absolute(init.value),
                &mut self.state,
                engine,
            );
        }
        self.reattach(engine, out);
    }

    /// The controller went away. Every held press is released (a held CUE stops
    /// previewing), SHIFT is released on both sides, and nothing is handled or sent until
    /// it comes back. A release the engine's queue has no room for is dropped.
    pub fn on_disconnect(&mut self, engine: &mut EngineController) {
        self.attached = false;
        for action in self.resolver.release_all_held() {
            apply(&action, Input::Release, &mut self.state, engine);
        }
        // Fallback: SHIFT is held state too, whether or not its press is still tracked.
        for side in Side::ALL {
            self.state.side_mut(side).shift = false;
        }
    }

    /// The controller came back: like [`attach`](Self::attach) but without the `[[init]]`
    /// actions, so the values set meanwhile are kept. Takeover is armed and every LED is
    /// sent again.
    pub fn on_reconnect(&mut self, engine: &mut EngineController, out: &mut dyn MidiOut) {
        self.reattach(engine, out);
    }

    fn reattach(&mut self, engine: &mut EngineController, out: &mut dyn MidiOut) {
        self.resolver.attach();
        self.feedback.resync();
        self.attached = true;
        self.tick(clock::now_micros(), engine, out);
    }

    /// Resolves and applies `msgs` in order. Ignored while the controller is not attached.
    ///
    /// Each resolved action is applied before the next message is resolved, as the
    /// resolver requires, with one exception: within a run of messages not interrupted by a
    /// button (or any non-absolute) action, absolute values for the same target are
    /// coalesced to the last one. Only that value is sent to the engine, at the end of the
    /// run, so a burst of fader messages costs one command. The control state follows
    /// every value immediately, so soft takeover and later messages see where the control
    /// is. Presses and releases are never coalesced and keep their order, and a run ends
    /// before each of them so they see (and the engine gets) every value sent before.
    pub fn handle_batch(
        &mut self,
        msgs: &[TimedMidi],
        engine: &mut EngineController,
    ) -> BatchStats {
        let mut stats = BatchStats::default();
        // Messages still queued after the controller was lost (or before it attached)
        // would bring back SHIFT or held presses no release will ever end.
        if !self.attached {
            return stats;
        }
        if !msgs.is_empty() {
            self.last.clear();
        }
        for timed in msgs {
            let resolved = self.resolver.resolve(&timed.message, &self.state);
            for &(action, input) in &resolved {
                match (input, action.value_target()) {
                    (Input::Absolute(v), Some(target)) if v.is_finite() => {
                        self.stage(target, action, v.clamp(0.0, 1.0), &mut stats, engine);
                    }
                    _ => {
                        self.flush(engine, &mut stats);
                        count(&mut stats, apply(&action, input, &mut self.state, engine));
                    }
                }
            }
            self.last.push((timed.message, resolved));
        }
        self.flush(engine, &mut stats);
        stats
    }

    /// Records `value` for `target` in the state and defers its engine command to the end
    /// of the run, replacing a value staged earlier in the run.
    fn stage(
        &mut self,
        target: ValueTarget,
        action: Action,
        value: f32,
        stats: &mut BatchStats,
        engine: &mut EngineController,
    ) {
        let original = match self.pending.iter().position(|p| p.target == target) {
            Some(i) => {
                stats.coalesced += 1;
                self.pending.remove(i).original
            }
            None => match self.state.value(target) {
                Some(v) => v,
                // Not a value the state tracks: nothing to coalesce on, send it now.
                None => {
                    self.flush(engine, stats);
                    let result = apply(&action, Input::Absolute(value), &mut self.state, engine);
                    count(stats, result);
                    return;
                }
            },
        };
        if let Some(field) = self.state.value_mut(target) {
            *field = value;
        }
        self.pending.push(Pending {
            target,
            action,
            value,
            original,
        });
    }

    /// Sends the staged values of the run. A value the engine did not take is rolled back
    /// in the state, so the state keeps matching the engine.
    fn flush(&mut self, engine: &mut EngineController, stats: &mut BatchStats) {
        for p in self.pending.drain(..) {
            let result = apply(&p.action, Input::Absolute(p.value), &mut self.state, engine);
            if !matches!(result, ApplyResult::Sent(_))
                && let Some(field) = self.state.value_mut(p.target)
            {
                *field = p.original;
            }
            count(stats, result);
        }
    }

    /// Updates the LEDs from the control state and the engine's deck snapshots. Does
    /// nothing while the controller is not attached.
    pub fn tick(&mut self, now_micros: u64, engine: &EngineController, out: &mut dyn MidiOut) {
        if !self.attached {
            return;
        }
        let snaps = DeckId::ALL.map(|id| engine.deck(id));
        self.feedback.update(now_micros, &self.state, &snaps, out);
    }

    /// Every message of the last non-empty batch with what it resolved to (empty when
    /// unmapped), for the MIDI monitor. Coalesced values are listed too.
    pub fn last_resolved(&self) -> &[(MidiMessage, Resolved)] {
        &self.last
    }
}

fn count(stats: &mut BatchStats, result: ApplyResult) {
    match result {
        ApplyResult::Sent(_) => stats.applied += 1,
        ApplyResult::QueueFull => stats.queue_full += 1,
        ApplyResult::Ignored => {}
    }
}

/// A change in the controller's presence seen by [`Connection::poll_presence`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresenceChange {
    None,
    /// The controller disappeared; its ports were closed.
    Lost,
    /// The controller is back and both its ports were reopened.
    Back,
}

/// The controller's input and output ports, found by the mapping's `port` substring, and
/// reopened when the controller is plugged back in.
pub struct Connection {
    port: String,
    tx: Sender<TimedMidi>,
    ports: Option<(InputPort, OutputPort)>,
    last_poll: Option<Instant>,
}

impl Connection {
    /// How often [`poll_presence`](Self::poll_presence) looks at the port list.
    pub const POLL_INTERVAL: Duration = Duration::from_secs(1);

    /// Opens the controller's input (forwarding to `tx`) and output. A controller that is
    /// not plugged in is not an error: the connection starts disconnected and reports
    /// [`PresenceChange::Back`] once it shows up.
    pub fn new(mapping: &Mapping, tx: Sender<TimedMidi>) -> Result<Self, PortError> {
        let mut connection = Self {
            port: mapping.controller.port.clone(),
            tx,
            ports: None,
            last_poll: None,
        };
        match connection.open() {
            Ok(()) | Err(PortError::NotFound { .. }) => Ok(connection),
            Err(e) => Err(e),
        }
    }

    fn open(&mut self) -> Result<(), PortError> {
        let input = open_input(&self.port, self.tx.clone())?;
        let output = open_output(&self.port)?;
        self.ports = Some((input, output));
        Ok(())
    }

    pub fn is_connected(&self) -> bool {
        self.ports.is_some()
    }

    /// The names of the open input and output ports.
    pub fn port_names(&self) -> Option<(&str, &str)> {
        self.ports
            .as_ref()
            .map(|(input, output)| (input.name(), output.name()))
    }

    /// Checks, at most once per [`POLL_INTERVAL`](Self::POLL_INTERVAL), whether the
    /// controller's input port is listed. On `Lost` both ports are closed; on `Back` both
    /// are reopened. A failed listing or reopen changes nothing and is retried next time.
    pub fn poll_presence(&mut self) -> PresenceChange {
        let now = Instant::now();
        if self
            .last_poll
            .is_some_and(|last| now.duration_since(last) < Self::POLL_INTERVAL)
        {
            return PresenceChange::None;
        }
        self.last_poll = Some(now);
        let Ok(names) = input_port_names() else {
            return PresenceChange::None;
        };
        let present = find_match(&names, &self.port).is_some();
        match (present, self.is_connected()) {
            (true, false) if self.open().is_ok() => PresenceChange::Back,
            (false, true) => {
                self.ports = None;
                PresenceChange::Lost
            }
            _ => PresenceChange::None,
        }
    }
}

impl MidiOut for Connection {
    /// Sends to the controller's output; dropped while disconnected.
    fn send(&mut self, msg: MidiMessage) {
        if let Some((_, output)) = &mut self.ports {
            MidiOut::send(output, msg);
        }
    }
}
