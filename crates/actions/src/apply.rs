use engine::{
    ChannelCommand, Command, DeckCommand, DeckId, EngineController, MasterCommand, TempoRange,
};

use crate::{Action, ActionKind, ControlState, Input, ValueTarget};

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
}

/// A state change committed once the engine accepted the command.
enum Update {
    None,
    Value(ValueTarget, f32),
    TempoRange(DeckId, TempoRange),
    Pfl(DeckId, bool),
}

/// Performs `action` for `input` on `engine`, and records the result in `state` when the
/// engine accepted it. Inputs the action does not take (including `Release` for press-only
/// actions and non-finite absolute values) are ignored.
pub fn apply(
    action: &Action,
    input: Input,
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

        (Action::PlayPause(d), Input::Press) => (deck(d, DeckCommand::TogglePlay), Update::None),
        (Action::Cue(d), Input::Press) => (deck(d, DeckCommand::CuePress), Update::None),
        (Action::Cue(d), Input::Release) => (deck(d, DeckCommand::CueRelease), Update::None),
        (Action::TempoRangeNext(d), Input::Press) => {
            let next = next_range(state.deck(d).tempo_range);
            (
                deck(d, DeckCommand::SetTempoRange(next)),
                Update::TempoRange(d, next),
            )
        }
        (Action::ReturnToStart(d), Input::Press) => (deck(d, DeckCommand::Seek(0.0)), Update::None),
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
    }
    ApplyResult::Sent(1)
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
