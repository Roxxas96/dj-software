//! The controller runtime: ties the resolver, actions, feedback and ports together on the
//! control thread, and follows the controller being unplugged and plugged back in.

use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use actions::{Action, AppRequest, ApplyResult, ControlState, Input, Side, ValueTarget, apply};
use engine::{DeckId, EngineController};

use crate::mapping::{InitAction, Mapping, Resolved, Resolver};
use crate::ports::{
    InputPort, MidiOut, OutputPort, PortError, find_match, input_port_names, open_input,
    open_output,
};
use crate::{Feedback, MidiMessage, TimedMidi, clock};

/// What one [`Runtime::handle_batch`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BatchStats {
    /// Actions that took effect, requests included.
    pub applied: u32,
    /// Inputs folded into another: absolute values replaced by a later value for the same
    /// target, and relative steps added to other steps for the same action.
    pub coalesced: u32,
    /// Actions dropped because the engine's command queue was full.
    pub queue_full: u32,
    /// What the actions asked the app to do (such as grid edits), in the order their
    /// presses came in. The app performs them.
    pub requests: Vec<AppRequest>,
}

/// An input whose action waits for the end of its run.
#[derive(Debug, Clone, Copy)]
struct Pending {
    action: Action,
    staged: Staged,
    /// When the (last) message came in.
    at_micros: u64,
}

#[derive(Debug, Clone, Copy)]
enum Staged {
    /// An absolute value, already recorded in the state.
    Value {
        target: ValueTarget,
        value: f32,
        /// The state's value before the run, restored if the command cannot be sent.
        original: f32,
    },
    /// The sum of the relative steps of the run.
    Steps(i32),
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
    /// Whether [`attach`](Runtime::attach) ran already.
    attached_once: bool,
    /// Absolute values and relative steps of the current run, in order of their last
    /// occurrence.
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
            attached_once: false,
            pending: Vec::new(),
            last: Vec::new(),
        }
    }

    /// The control state the mapping drives (side decks, SHIFT, the values sent).
    pub fn state(&self) -> &ControlState {
        &self.state
    }

    /// The controller attached: applies the `[[init]]` actions and sends every LED.
    ///
    /// On the first attach soft takeover starts disarmed: nothing is known about where the
    /// controls are, so the first value of each one is taken as is (a mapping that wants a
    /// value to stay put until touched, like a closed fader, sets it with `[[init]]`). Any
    /// later attach arms takeover for every target, like
    /// [`on_reconnect`](Self::on_reconnect).
    ///
    /// An init action the engine's queue has no room for is skipped.
    pub fn attach(&mut self, engine: &mut EngineController, out: &mut dyn MidiOut) {
        let now = clock::now_micros();
        for init in &self.init {
            apply(
                &init.action,
                Input::Absolute(init.value),
                now,
                &mut self.state,
                engine,
            );
        }
        if self.attached_once {
            self.resolver.attach();
        } else {
            self.resolver.first_attach();
        }
        self.attached_once = true;
        self.resync(engine, out);
    }

    /// The controller went away. Every held press is released (a held CUE stops
    /// previewing), SHIFT is released on both sides, and nothing is handled or sent until
    /// it comes back. A release the engine's queue has no room for is dropped.
    pub fn on_disconnect(&mut self, engine: &mut EngineController) {
        self.attached = false;
        let now = clock::now_micros();
        for action in self.resolver.release_all_held() {
            apply(&action, Input::Release, now, &mut self.state, engine);
        }
        // Fallback: SHIFT is held state too, whether or not its press is still tracked.
        for side in Side::ALL {
            self.state.side_mut(side).shift = false;
        }
    }

    /// The controller came back: like [`attach`](Self::attach) but without the `[[init]]`
    /// actions, so the values set meanwhile are kept. Takeover is armed for every target
    /// (the controls may have moved meanwhile) and every LED is sent again.
    pub fn on_reconnect(&mut self, engine: &mut EngineController, out: &mut dyn MidiOut) {
        self.resolver.attach();
        self.resync(engine, out);
    }

    fn resync(&mut self, engine: &mut EngineController, out: &mut dyn MidiOut) {
        self.feedback.resync();
        self.attached = true;
        self.tick(clock::now_micros(), engine, out);
    }

    /// Recovers a controller link that may have died without its port disappearing (a USB
    /// dropout shorter than the presence poll can leave the ports open but silent): the
    /// controller is handled as gone, `connection` drops and reopens its ports, and if they
    /// came back it is handled as back, like [`on_reconnect`](Self::on_reconnect) (or like
    /// [`attach`](Self::attach) if it never attached).
    ///
    /// Returns what [`Reconnect::force_reconnect`] reported. On anything but
    /// [`PresenceChange::Back`] the controller stays detached until presence polling finds
    /// it again.
    pub fn force_reconnect<C: Reconnect>(
        &mut self,
        engine: &mut EngineController,
        connection: &mut C,
    ) -> PresenceChange {
        if self.attached {
            self.on_disconnect(engine);
        }
        let change = connection.force_reconnect();
        if change == PresenceChange::Back {
            if self.attached_once {
                self.on_reconnect(engine, connection);
            } else {
                self.attach(engine, connection);
            }
        }
        change
    }

    /// Resolves and applies `msgs` in order. Ignored while the controller is not attached.
    ///
    /// Each resolved action is applied before the next message is resolved, as the
    /// resolver requires, with two exceptions within a run of messages not interrupted by
    /// a press or release:
    /// - absolute values for the same target are coalesced to the last one. Only that
    ///   value is sent to the engine, at the end of the run, so a burst of fader messages
    ///   costs one command. The control state follows every value immediately, so soft
    ///   takeover and later messages see where the control is;
    /// - relative steps for the same action are summed and applied once at the end of the
    ///   run, so a fast turn of the BROWSE knob is one scroll. Steps that sum to 0 do
    ///   nothing.
    ///
    /// Presses and releases are never coalesced and keep their order, and a run ends
    /// before each of them so they see (and the engine gets) every value sent before.
    /// Requests come out in [`BatchStats::requests`] in order: a press's at the press, a
    /// run's at the end of the run.
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
                        let v = v.clamp(0.0, 1.0);
                        self.stage(target, action, v, timed.micros, &mut stats, engine);
                    }
                    (Input::Relative(steps), _) => {
                        self.stage_steps(action, steps, timed.micros, &mut stats);
                    }
                    _ => {
                        self.flush(engine, &mut stats);
                        let result = apply(&action, input, timed.micros, &mut self.state, engine);
                        count(&mut stats, result);
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
        at_micros: u64,
        stats: &mut BatchStats,
        engine: &mut EngineController,
    ) {
        let staged = self
            .pending
            .iter()
            .enumerate()
            .find_map(|(i, p)| match p.staged {
                Staged::Value {
                    target: t,
                    original,
                    ..
                } if t == target => Some((i, original)),
                _ => None,
            });
        let original = match staged {
            Some((i, original)) => {
                stats.coalesced += 1;
                self.pending.remove(i);
                original
            }
            None => match self.state.value(target) {
                Some(v) => v,
                // Not a value the state tracks: nothing to coalesce on, send it now.
                None => {
                    self.flush(engine, stats);
                    let input = Input::Absolute(value);
                    let result = apply(&action, input, at_micros, &mut self.state, engine);
                    count(stats, result);
                    return;
                }
            },
        };
        if let Some(field) = self.state.value_mut(target) {
            *field = value;
        }
        self.pending.push(Pending {
            action,
            staged: Staged::Value {
                target,
                value,
                original,
            },
            at_micros,
        });
    }

    /// Adds `steps` to the steps staged for `action` in this run (if any) and defers the
    /// action to the end of the run.
    fn stage_steps(&mut self, action: Action, steps: i32, at_micros: u64, stats: &mut BatchStats) {
        let staged = self
            .pending
            .iter()
            .position(|p| p.action == action && matches!(p.staged, Staged::Steps(_)));
        let total = match staged.map(|i| self.pending.remove(i).staged) {
            Some(Staged::Steps(before)) => {
                stats.coalesced += 1;
                before.saturating_add(steps)
            }
            _ => steps,
        };
        self.pending.push(Pending {
            action,
            staged: Staged::Steps(total),
            at_micros,
        });
    }

    /// Applies the staged values and steps of the run. A value the engine did not take is
    /// rolled back in the state, so the state keeps matching the engine.
    fn flush(&mut self, engine: &mut EngineController, stats: &mut BatchStats) {
        for p in self.pending.drain(..) {
            let result = match p.staged {
                Staged::Value {
                    target,
                    value,
                    original,
                } => {
                    let input = Input::Absolute(value);
                    let result = apply(&p.action, input, p.at_micros, &mut self.state, engine);
                    if !matches!(result, ApplyResult::Sent(_))
                        && let Some(field) = self.state.value_mut(target)
                    {
                        *field = original;
                    }
                    result
                }
                Staged::Steps(0) => continue,
                Staged::Steps(steps) => {
                    let input = Input::Relative(steps);
                    apply(&p.action, input, p.at_micros, &mut self.state, engine)
                }
            };
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
        ApplyResult::Request(request) => {
            stats.applied += 1;
            stats.requests.push(request);
        }
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

/// A controller link that can be dropped and reopened on demand, for
/// [`Runtime::force_reconnect`].
pub trait Reconnect: MidiOut {
    /// Drops the ports (if open) and reopens them. Reports [`PresenceChange::Back`] when
    /// they reopened, [`PresenceChange::Lost`] when they were open but could not be
    /// reopened, and [`PresenceChange::None`] when they were closed and still are.
    fn force_reconnect(&mut self) -> PresenceChange;
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

impl Reconnect for Connection {
    /// Drops both ports, then reopens them. If reopening fails, the connection stays closed
    /// and [`poll_presence`](Connection::poll_presence) reopens it once the port is listed.
    fn force_reconnect(&mut self) -> PresenceChange {
        let was_connected = self.ports.take().is_some();
        match self.open() {
            Ok(()) => PresenceChange::Back,
            Err(_) if was_connected => PresenceChange::Lost,
            Err(_) => PresenceChange::None,
        }
    }
}

impl Connection {
    /// Drops and reopens both ports; see [`Reconnect::force_reconnect`].
    pub fn force_reconnect(&mut self) -> PresenceChange {
        Reconnect::force_reconnect(self)
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
