use engine::{
    ChannelCommand, Command, DeckCommand, DeckId, EngineController, JogSurface, MasterCommand,
    SamplerCommand, TempoRange,
};

use crate::sync::plan_sync;
use crate::{
    Action, ActionKind, AppRequest, BrowserRequest, ControlState, GridEditRequest, Input, Side,
    ValueTarget,
};

/// What [`apply`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyResult {
    /// The action took effect after sending this many engine commands. Side actions
    /// (deck toggle, SHIFT) only change [`ControlState`] and report `Sent(0)`.
    Sent(u8),
    /// The action does not accept this input.
    Ignored,
    /// The engine's command queue was full; nothing changed.
    QueueFull,
    /// The action needs the app (the library, the loaded track): nothing was sent and the
    /// caller hands this request to the app.
    Request(AppRequest),
}

/// How far `deck.grid_shift_back` and `deck.grid_shift_forward` move the grid, in
/// milliseconds.
pub const GRID_SHIFT_MILLIS: i32 = 5;
/// The same with SHIFT held on the binding's side.
pub const GRID_SHIFT_FINE_MILLIS: i32 = 1;

/// A state change committed once the engine accepted the command.
enum Update {
    None,
    Value(ValueTarget, f32),
    TempoRange(DeckId, TempoRange),
    Pfl(DeckId, bool),
    FxOn(DeckId, usize, bool),
}

/// Performs `action` for `input` on `engine`, and records the result in `state` when the
/// engine accepted it. `at_micros` is when the input happened on the controller clock
/// (the MIDI message's time); only requests that need a time, such as a tap, use it. Inputs the action does not take (including `Release` for press-only
/// actions and non-finite absolute values) are ignored.
pub fn apply(
    action: &Action,
    input: Input,
    at_micros: u64,
    state: &mut ControlState,
    engine: &mut EngineController,
) -> ApplyResult {
    let (command, update) = match (*action, input) {
        // Side actions only live in the control state.
        (Action::DeckToggle(side), Input::Press) => {
            let [main, alt] = side.decks();
            let s = state.side_mut(side);
            s.active_deck = if s.active_deck == main { alt } else { main };
            return ApplyResult::Sent(0);
        }
        (Action::Shift(side), Input::Press | Input::Release) => {
            state.side_mut(side).shift = input == Input::Press;
            return ApplyResult::Sent(0);
        }

        // Grid edits need the library and the loaded track: the app performs them.
        (Action::GridShiftBack(d), Input::Press) => return grid(d, shift_by(state, d, -1)),
        (Action::GridShiftForward(d), Input::Press) => return grid(d, shift_by(state, d, 1)),
        (Action::GridSetDownbeat(d), Input::Press) => return grid(d, GridEditRequest::SetDownbeat),
        (Action::GridHalve(d), Input::Press) => return grid(d, GridEditRequest::Halve),
        (Action::GridDouble(d), Input::Press) => return grid(d, GridEditRequest::Double),
        (Action::GridTap(d), Input::Press) => return grid(d, GridEditRequest::Tap { at_micros }),
        (Action::GridLockToggle(d), Input::Press) => return grid(d, GridEditRequest::ToggleLock),

        // The browser and loading tracks need the library: the app performs them too.
        (Action::BrowserScroll, Input::Relative(steps)) => {
            return ApplyResult::Request(AppRequest::Browser(BrowserRequest::Scroll(steps)));
        }
        (Action::BrowserToggleFocus, Input::Press) => {
            return ApplyResult::Request(AppRequest::Browser(BrowserRequest::ToggleFocus));
        }
        (Action::LoadSelected(deck), Input::Press) => {
            return ApplyResult::Request(AppRequest::LoadSelected { deck });
        }

        (Action::PlayPause(d), Input::Press) => (deck(d, DeckCommand::TogglePlay), Update::None),
        (Action::Cue(d), Input::Press) => (deck(d, DeckCommand::CuePress), Update::None),
        (Action::Cue(d), Input::Release) => (deck(d, DeckCommand::CueRelease), Update::None),
        (Action::Hotcue(d, slot), Input::Press) => {
            (deck(d, DeckCommand::HotcuePress(slot)), Update::None)
        }
        (Action::Hotcue(d, slot), Input::Release) => {
            (deck(d, DeckCommand::HotcueRelease(slot)), Update::None)
        }
        (Action::HotcueDelete(d, slot), Input::Press) => {
            (deck(d, DeckCommand::HotcueDelete(slot)), Update::None)
        }
        // A shifted note's release ends its unshifted sibling's hold: SHIFT went down while
        // the pad or CUE was held, so the note-off arrives on the shifted note.
        (Action::HotcueDelete(d, slot), Input::Release) => {
            (deck(d, DeckCommand::HotcueRelease(slot)), Update::None)
        }
        (Action::PadMode(side, mode), Input::Press) => {
            state.side_mut(side).pad_mode = mode;
            return ApplyResult::Sent(0);
        }
        (Action::BeatJump(d, slot), Input::Press) => {
            let beats = state.beat_jump_pads[slot.index()];
            (deck(d, DeckCommand::BeatJump { beats }), Update::None)
        }
        (Action::BeatJumpShift(d, slot), Input::Press) => {
            let beats = state.beat_jump_shift_pads[slot.index()];
            (deck(d, DeckCommand::BeatJump { beats }), Update::None)
        }
        (Action::LoopRoll(d, slot), Input::Press) => {
            let beats = state.loop_roll_pads[slot.index()];
            (deck(d, DeckCommand::RollPress { beats }), Update::None)
        }
        (Action::LoopRoll(d, slot), Input::Release) => {
            let beats = state.loop_roll_pads[slot.index()];
            (deck(d, DeckCommand::RollRelease { beats }), Update::None)
        }
        (Action::Slice(d, slot), Input::Press) => {
            let slice = slot.index() as u8;
            (deck(d, DeckCommand::SlicePress(slice)), Update::None)
        }
        (Action::Slice(d, slot), Input::Release) => {
            let slice = slot.index() as u8;
            (deck(d, DeckCommand::SliceRelease(slice)), Update::None)
        }
        (Action::SlipToggle(d), Input::Press) => {
            let slip = !engine.deck(d).slip;
            (deck(d, DeckCommand::SetSlip(slip)), Update::None)
        }
        (Action::KeylockToggle(d), Input::Press) => {
            let keylock = !engine.deck(d).keylock;
            (deck(d, DeckCommand::SetKeylock(keylock)), Update::None)
        }
        (Action::FxToggle(d, n), Input::Press) if n < 3 => {
            let slot = usize::from(n);
            let on = !state.deck(d).fx_on[slot];
            (
                Command::Channel(d, ChannelCommand::FxEnabled(slot, on)),
                Update::FxOn(d, slot, on),
            )
        }
        (Action::SamplerPlay(slot), Input::Press) => (
            Command::Sampler(SamplerCommand::Play(slot.index())),
            Update::None,
        ),
        (Action::SamplerStop(slot), Input::Press) => (
            Command::Sampler(SamplerCommand::Stop(slot.index())),
            Update::None,
        ),
        (Action::LoopIn(d), Input::Press) => (deck(d, DeckCommand::LoopIn), Update::None),
        (Action::LoopOut(d), Input::Press) => (deck(d, DeckCommand::LoopOut), Update::None),
        (Action::AutoLoop(d), Input::Press) => (deck(d, DeckCommand::AutoLoop), Update::None),
        (Action::LoopHalve(d), Input::Press) => (deck(d, DeckCommand::LoopHalve), Update::None),
        (Action::LoopDouble(d), Input::Press) => (deck(d, DeckCommand::LoopDouble), Update::None),
        (Action::ReloopExit(d), Input::Press) => (deck(d, DeckCommand::ReloopExit), Update::None),
        (Action::VinylToggle(d), Input::Press) => {
            let vinyl = !engine.deck(d).vinyl;
            (deck(d, DeckCommand::SetVinyl(vinyl)), Update::None)
        }
        (Action::JogTouch(d), Input::Press | Input::Release) => (
            deck(d, DeckCommand::JogTouch(input == Input::Press)),
            Update::None,
        ),
        (Action::JogRelease(d), Input::Press | Input::Release) => {
            (deck(d, DeckCommand::JogTouch(false)), Update::None)
        }
        (Action::JogTop(d), Input::Relative(steps)) => {
            let turns = turns(state, steps);
            let surface = JogSurface::Top;
            (deck(d, DeckCommand::Jog { turns, surface }), Update::None)
        }
        (Action::JogRing(d), Input::Relative(steps)) => {
            let turns = turns(state, steps);
            let surface = JogSurface::Ring;
            (deck(d, DeckCommand::Jog { turns, surface }), Update::None)
        }
        (Action::JogSearch(d), Input::Relative(steps)) => {
            let turns = turns(state, steps);
            (deck(d, DeckCommand::JogSearch { turns }), Update::None)
        }
        (Action::JogScratch(d), Input::Relative(steps)) => {
            let turns = turns(state, steps);
            let surface = JogSurface::Platter;
            (deck(d, DeckCommand::Jog { turns, surface }), Update::None)
        }
        (Action::BackspinShort(d), Input::Press) => {
            let seconds = state.backspin_short;
            (deck(d, DeckCommand::Backspin { seconds }), Update::None)
        }
        (Action::BackspinLong(d), Input::Press) => {
            let seconds = state.backspin_long;
            (deck(d, DeckCommand::Backspin { seconds }), Update::None)
        }
        (Action::ScratchAnchorSet(d), Input::Press) => {
            state.deck_mut(d).scratch_anchor = Some(engine.deck(d).position_frames);
            return ApplyResult::Sent(0);
        }
        // `Seek` rather than a hotcue jump: the anchor is not a hotcue (no preview, no slot),
        // and dropping slip ghosts, rolls and a backspin is what a restart should do.
        (Action::ScratchAnchorReturn(d), Input::Press) => {
            let Some(anchor) = state.deck(d).scratch_anchor else {
                state.deck_mut(d).scratch_anchor = Some(engine.deck(d).position_frames);
                return ApplyResult::Sent(0);
            };
            (deck(d, DeckCommand::Seek(anchor)), Update::None)
        }
        // No snapshot pre-checks: the engine ignores AutoLoopBeats while a loop is active
        // and LoopExit without one, so an FX FADE pad 3/7 press followed by its loop note
        // in the same batch always acts on the engine's current state.
        (Action::LoopFade1(d), Input::Press) => {
            (deck(d, DeckCommand::AutoLoopBeats(1.0)), Update::None)
        }
        (Action::LoopFade4(d), Input::Press) => {
            (deck(d, DeckCommand::AutoLoopBeats(4.0)), Update::None)
        }
        (Action::LoopExit(d), Input::Press) => (deck(d, DeckCommand::LoopExit), Update::None),
        (Action::Sync(d), Input::Press) => return sync(d, state, engine),
        (Action::TempoRangeNext(d), Input::Press) => {
            let next = next_range(state.deck(d).tempo_range);
            (
                deck(d, DeckCommand::SetTempoRange(next)),
                Update::TempoRange(d, next),
            )
        }
        (Action::ReturnToStart(d), Input::Press) => (deck(d, DeckCommand::Seek(0.0)), Update::None),
        (Action::ReturnToStart(d), Input::Release) => {
            (deck(d, DeckCommand::CueRelease), Update::None)
        }
        (Action::PflToggle(d), Input::Press) => {
            let on = !state.deck(d).pfl;
            (
                Command::Channel(d, ChannelCommand::Pfl(on)),
                Update::Pfl(d, on),
            )
        }
        (Action::HeadphoneMasterToggle, Input::Press) => {
            let mix = if state.headphone_master { 0.0 } else { 1.0 };
            (
                Command::Master(MasterCommand::HeadphoneMix(mix)),
                Update::Value((ActionKind::HeadphoneMix, None), mix),
            )
        }

        // The engine drops non-finite values and clamps the rest: do the same so the state
        // stays in step with it.
        (_, Input::Absolute(v)) if v.is_finite() => {
            let v = v.clamp(0.0, 1.0);
            let (Some(command), Some(target)) =
                (absolute_command(*action, v), action.value_target())
            else {
                return ApplyResult::Ignored;
            };
            (command, Update::Value(target, v))
        }
        _ => return ApplyResult::Ignored,
    };

    if engine.send(command).is_err() {
        return ApplyResult::QueueFull;
    }
    match update {
        Update::None => {}
        Update::Value(target, v) => set_value(state, target, v),
        Update::TempoRange(d, range) => state.deck_mut(d).tempo_range = range,
        Update::Pfl(d, on) => state.deck_mut(d).pfl = on,
        Update::FxOn(d, slot, on) => state.deck_mut(d).fx_on[slot] = on,
    }
    ApplyResult::Sent(1)
}

/// Matches `deck`'s tempo to the playing deck it follows and aligns the beats. The state is
/// recorded like a moved slider and a pressed range button, so soft takeover arms.
fn sync(deck: DeckId, state: &mut ControlState, engine: &mut EngineController) -> ApplyResult {
    let snaps = DeckId::ALL.map(|id| engine.deck(id));
    let Some(plan) = plan_sync(deck, state, &snaps) else {
        return ApplyResult::Ignored;
    };
    let mut commands = Vec::with_capacity(3);
    let range_changed = plan.range != state.deck(deck).tempo_range;
    if range_changed {
        commands.push(self::deck(deck, DeckCommand::SetTempoRange(plan.range)));
    }
    commands.push(self::deck(deck, DeckCommand::SetTempoSlider(plan.slider)));
    commands.push(Command::PhaseSync {
        deck,
        master: plan.master,
        beat_ratio: plan.beat_ratio,
    });
    if engine.send_all(&commands).is_err() {
        return ApplyResult::QueueFull;
    }
    if range_changed {
        state.deck_mut(deck).tempo_range = plan.range;
    }
    set_value(state, (ActionKind::Tempo, Some(deck)), plan.slider);
    ApplyResult::Sent(commands.len() as u8)
}

fn grid(deck: DeckId, edit: GridEditRequest) -> ApplyResult {
    ApplyResult::Request(AppRequest::Grid { deck, edit })
}

/// A grid shift in `direction` (+1 or -1): [`GRID_SHIFT_MILLIS`], or
/// [`GRID_SHIFT_FINE_MILLIS`] while SHIFT is held on `deck`'s side.
fn shift_by(state: &ControlState, deck: DeckId, direction: i32) -> GridEditRequest {
    let step = if state.side(Side::of(deck)).shift {
        GRID_SHIFT_FINE_MILLIS
    } else {
        GRID_SHIFT_MILLIS
    };
    GridEditRequest::Shift {
        millis: direction * step,
    }
}

/// `steps` jog wheel steps as turns of the platter.
fn turns(state: &ControlState, steps: i32) -> f64 {
    f64::from(steps) / f64::from(state.jog_steps_per_turn.max(1))
}

fn deck(id: DeckId, cmd: DeckCommand) -> Command {
    Command::Deck(id, cmd)
}

/// The engine command for an absolute action at `v`.
fn absolute_command(action: Action, v: f32) -> Option<Command> {
    let channel = |d, cmd| Some(Command::Channel(d, cmd));
    let master = |cmd| Some(Command::Master(cmd));
    match action {
        Action::Tempo(d) => Some(deck(d, DeckCommand::SetTempoSlider(v))),
        Action::Trim(d) => channel(d, ChannelCommand::Trim(v)),
        Action::EqHigh(d) => channel(d, ChannelCommand::EqHigh(v)),
        Action::EqMid(d) => channel(d, ChannelCommand::EqMid(v)),
        Action::EqLow(d) => channel(d, ChannelCommand::EqLow(v)),
        Action::Filter(d) => channel(d, ChannelCommand::Filter(v)),
        Action::Fader(d) => channel(d, ChannelCommand::Fader(v)),
        Action::FxLevel(d) => channel(d, ChannelCommand::FxAmount(v)),
        Action::Crossfader => master(MasterCommand::Crossfader(v)),
        Action::MasterLevel => master(MasterCommand::MasterLevel(v)),
        Action::HeadphoneLevel => master(MasterCommand::HeadphoneLevel(v)),
        Action::HeadphoneMix => master(MasterCommand::HeadphoneMix(v)),
        _ => None,
    }
}

fn set_value(state: &mut ControlState, target: ValueTarget, v: f32) {
    if let Some(field) = state.value_mut(target) {
        *field = v;
    }
    // Turning the mix past the middle counts as headphone-master, so the toggle flips from
    // wherever the knob left it.
    if target == (ActionKind::HeadphoneMix, None) {
        state.headphone_master = v >= 0.5;
    }
}

fn next_range(range: TempoRange) -> TempoRange {
    match range {
        TempoRange::Six => TempoRange::Ten,
        TempoRange::Ten => TempoRange::Sixteen,
        TempoRange::Sixteen => TempoRange::Hundred,
        TempoRange::Hundred => TempoRange::Six,
    }
}
