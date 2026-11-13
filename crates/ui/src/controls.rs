//! Mouse gestures for the window's controls: what a drag, a wheel move, a click or a
//! double-click on a knob, a fader or a button means, as the [`Input`] a controller would
//! send for the same control.
//!
//! The gestures read the [`Response`] and the input state only; they draw nothing and
//! send nothing. The caller allocates the widget with the sense it needs (drag for a
//! knob, click and drag for a fader or a button) and sends what comes back as a
//! [`UiCommand::Action`] through a [`Sink`], which remembers the buttons held down
//! ([`HeldButtons`]).

use std::sync::mpsc::Sender;

use actions::{Action, Input};
use egui::{Id, PointerButton, Pos2, Rect, Response, Ui, Vec2};

use crate::UiCommand;

/// Points of upward drag that turn a knob over its full travel.
pub const KNOB_DRAG_POINTS: f32 = 150.0;
/// The fine drag (Shift held) is this many times slower.
pub const FINE_DIVISOR: f32 = 4.0;
/// A knob's change per wheel notch.
pub const WHEEL_STEP: f32 = 0.02;

/// Points egui reports for one notch of a mouse wheel on native platforms
/// (`InputOptions::line_scroll_speed`); a trackpad reports smaller steps that add up.
const POINTS_PER_NOTCH: f32 = 40.0;

/// The knob's new value for a drag, a wheel move or a double-click on it, or `None` when
/// nothing happened. Dragging up raises the value; Shift drags a quarter as fast. A
/// double-click gives `reset`. The result is clamped to `0..=1`.
pub fn knob_input(response: &Response, value: f32, reset: f32, ui: &Ui) -> Option<Input> {
    if response.double_clicked_by(PointerButton::Primary) {
        return Some(Input::Absolute(reset.clamp(0.0, 1.0)));
    }
    let mut delta = 0.0;
    if response.dragged_by(PointerButton::Primary) {
        let fine = ui.input(|i| i.modifiers.shift);
        let divisor = if fine { FINE_DIVISOR } else { 1.0 };
        delta -= response.drag_delta().y / (KNOB_DRAG_POINTS * divisor);
    }
    if response.hovered() {
        let scrolled = ui.input(|i| i.smooth_scroll_delta().y);
        delta += scrolled / POINTS_PER_NOTCH * WHEEL_STEP;
    }
    (delta != 0.0).then(|| Input::Absolute((value + delta).clamp(0.0, 1.0)))
}

/// Where the primary-button drag of `response` went since the last frame, as the pointer's
/// previous and present positions, or `None` when it did not move or is not a drag. A
/// drag starts a few points after the press, so the first leg runs from the press itself:
/// the points that told it from a click are not lost.
pub fn drag_path(response: &Response) -> Option<(Pos2, Pos2)> {
    let key = response.id.with("drag_last");
    let held =
        response.is_pointer_button_down_on() && response.ctx.input(|i| i.pointer.primary_down());
    let pos = response.interact_pointer_pos().filter(|_| held);
    let Some(pos) = pos else {
        response.ctx.data_mut(|d| d.remove::<Pos2>(key));
        return None;
    };
    let last = response
        .ctx
        .data_mut(|d| *d.get_temp_mut_or_insert_with(key, || pos));
    if !response.dragged_by(PointerButton::Primary) {
        return None;
    }
    response.ctx.data_mut(|d| d.insert_temp(key, pos));
    Some((last, pos))
}

/// The whole steps of a drag that moved `amount` steps this frame, the fraction left
/// over carried to the next frame under `id`. Not `dragging`: nothing, and the carry is
/// dropped, so the next drag starts clean.
pub fn carry_steps(ctx: &egui::Context, id: Id, amount: f32, dragging: bool) -> i32 {
    let carry = id.with("steps_carry");
    if !dragging {
        ctx.data_mut(|d| d.remove::<f32>(carry));
        return 0;
    }
    let total = amount + ctx.data(|d| d.get_temp::<f32>(carry)).unwrap_or(0.0);
    let steps = total.round();
    ctx.data_mut(|d| d.insert_temp(carry, total - steps));
    steps as i32
}

/// How long a knob goes on from the value it last sent rather than the frame's: the
/// session's copy lags a frame or two behind a drag or a wheel.
const KNOB_MEMORY: f64 = 0.5;

/// [`knob_input`] for a knob the frame shows at `shown`. While the knob turns (and for
/// half a second after) it goes on from the value it last sent, so a drag does not slow
/// down while the frame catches up. A wheel move that turns it is taken from the input,
/// so a scroll area around it does not scroll too.
pub fn turn_knob(response: &Response, shown: f32, reset: f32, ui: &Ui) -> Option<Input> {
    let memory = response.id.with("knob_sent");
    let now = ui.input(|i| i.time);
    let value = ui
        .ctx()
        .data(|d| d.get_temp::<(f32, f64)>(memory))
        .filter(|&(_, at)| response.dragged() || now - at < KNOB_MEMORY)
        .map_or(shown, |(sent, _)| sent);
    let input = knob_input(response, value, reset, ui)?;
    if let Input::Absolute(v) = input {
        ui.ctx().data_mut(|d| d.insert_temp(memory, (v, now)));
    }
    if response.hovered() {
        ui.ctx().input_mut(|i| i.smooth_scroll_delta = Vec2::ZERO);
    }
    Some(input)
}

/// The fader's value for a click or a drag at the pointer, mapped along `track` (the
/// handle's travel): a click jumps there, a drag follows. A vertical fader has 1 at the
/// top, a horizontal one at the right; a control that runs the other way inverts the
/// value. A double-click gives `reset` when there is one. `None` when the pointer is not
/// pressing the fader.
pub fn fader_input(
    response: &Response,
    track: Rect,
    vertical: bool,
    reset: Option<f32>,
) -> Option<Input> {
    if let Some(reset) = reset
        && response.double_clicked_by(PointerButton::Primary)
    {
        return Some(Input::Absolute(reset.clamp(0.0, 1.0)));
    }
    let pressing = response.dragged_by(PointerButton::Primary)
        || (response.is_pointer_button_down_on()
            && response.ctx.input(|i| i.pointer.primary_down()));
    if !pressing {
        return None;
    }
    let pos = response.interact_pointer_pos()?;
    let value = if vertical {
        1.0 - (pos.y - track.min.y) / track.height().max(1.0)
    } else {
        (pos.x - track.min.x) / track.width().max(1.0)
    };
    Some(Input::Absolute(value.clamp(0.0, 1.0)))
}

/// `Press` on the frame the pointer goes down on the button, `Release` on the frame that
/// press ends (the pointer comes up anywhere, or the window loses it), `None` in
/// between: a held button never repeats. Whether the button is held is kept in egui's
/// memory under the widget's id.
pub fn button_input(response: &Response) -> Option<Input> {
    let down = response.ctx.input(|i| i.pointer.primary_down());
    let pressing = response.is_pointer_button_down_on() && down;
    let held_id = button_held(response.id);
    let held = response
        .ctx
        .data(|d| d.get_temp::<bool>(held_id))
        .unwrap_or(false);
    let next = if held { down } else { pressing };
    if next == held {
        return None;
    }
    response.ctx.data_mut(|d| d.insert_temp(held_id, next));
    Some(if next { Input::Press } else { Input::Release })
}

/// Where [`button_input`] keeps whether the button `id` is held.
fn button_held(id: Id) -> Id {
    id.with("button_held")
}

/// The buttons held down with the mouse, by widget id, with the action each pressed.
/// A button's release always goes to the action it pressed, even if the button has
/// changed since (a pad whose mode changed); one that is no longer drawn is released at
/// the end of the frame ([`sweep`](Self::sweep)), so no action stays held. A button
/// released that way does nothing more until its press ends, even if it is drawn again.
#[derive(Debug, Default)]
pub struct HeldButtons {
    held: Vec<Held>,
    /// Buttons released by a sweep whose press has not ended yet.
    swept: Vec<Id>,
}

#[derive(Debug)]
struct Held {
    id: Id,
    action: Action,
    /// Drawn since the last sweep.
    drawn: bool,
}

impl HeldButtons {
    /// The command for the button of `response`, which presses `action`, if its press
    /// began or ended this frame: `Press` to `action`, or `Release` to the action it
    /// pressed.
    pub fn button(&mut self, response: &Response, action: Action) -> Option<UiCommand> {
        let place = self.held.iter().position(|h| h.id == response.id);
        if let Some(i) = place {
            self.held[i].drawn = true;
        }
        match button_input(response)? {
            Input::Press => {
                let held = Held {
                    id: response.id,
                    action,
                    drawn: true,
                };
                match place {
                    Some(i) => self.held[i] = held,
                    None => self.held.push(held),
                }
                Some(UiCommand::Action(action, Input::Press))
            }
            input => {
                if let Some(i) = self.swept.iter().position(|&id| id == response.id) {
                    // Its release was sent when it went.
                    self.swept.swap_remove(i);
                    return None;
                }
                let pressed = place.map_or(action, |i| self.held.swap_remove(i).action);
                Some(UiCommand::Action(pressed, input))
            }
        }
    }

    /// Ends the frame: releases every held button that was not drawn in it, sending the
    /// release to `commands`. Once the pointer is up, the buttons released that way are
    /// forgotten, so the next press on one counts.
    pub fn sweep(&mut self, ctx: &egui::Context, commands: &Sender<UiCommand>) {
        if !self.swept.is_empty() && !ctx.input(|i| i.pointer.primary_down()) {
            for id in self.swept.drain(..) {
                ctx.data_mut(|d| d.remove::<bool>(button_held(id)));
            }
        }
        let swept = &mut self.swept;
        self.held.retain_mut(|held| {
            if std::mem::take(&mut held.drawn) {
                return true;
            }
            let _ = commands.send(UiCommand::Action(held.action, Input::Release));
            if !swept.contains(&held.id) {
                swept.push(held.id);
            }
            false
        });
    }
}

/// Where the window's controls send: the session's commands, and the buttons held.
#[derive(Debug)]
pub struct Sink<'a> {
    pub commands: &'a Sender<UiCommand>,
    pub held: &'a mut HeldButtons,
}

impl<'a> Sink<'a> {
    pub fn new(commands: &'a Sender<UiCommand>, held: &'a mut HeldButtons) -> Self {
        Self { commands, held }
    }

    /// Sends `command` to the session; a window closing has no one to send to.
    pub fn send(&self, command: UiCommand) {
        let _ = self.commands.send(command);
    }

    /// Sends `input` for `action`, when there is one.
    pub fn act(&self, action: Action, input: Option<Input>) {
        if let Some(input) = input {
            self.send(UiCommand::Action(action, input));
        }
    }

    /// Sends the press or release of the button of `response`, which presses `action`.
    pub fn button(&mut self, response: &Response, action: Action) {
        if let Some(command) = self.held.button(response, action) {
            self.send(command);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Event, Modifiers, MouseWheelUnit, Sense, pos2, vec2};
    use egui_kittest::Harness;

    /// What the test widget saw: the value it shows, and every input the helper gave.
    #[derive(Default)]
    struct Seen {
        value: f32,
        inputs: Vec<Input>,
    }

    const CENTRE: egui::Pos2 = pos2(50.0, 50.0);
    const TRACK: Rect = Rect {
        min: pos2(20.0, 20.0),
        max: pos2(40.0, 120.0),
    };

    /// A 40-point square knob at 0.5 whose value follows what the helper returns.
    fn knob(reset: f32) -> Harness<'static, Seen> {
        let seen = Seen {
            value: 0.5,
            inputs: Vec::new(),
        };
        // Short steps, so two clicks in a row fall inside the double-click delay.
        let mut harness = Harness::builder().with_step_dt(0.01).build_ui_state(
            move |ui, seen: &mut Seen| {
                let rect = Rect::from_center_size(CENTRE, vec2(40.0, 40.0));
                let response =
                    ui.interact(rect, ui.scope_id().with("knob"), Sense::click_and_drag());
                if let Some(input) = knob_input(&response, seen.value, reset, ui) {
                    if let Input::Absolute(v) = input {
                        seen.value = v;
                    }
                    seen.inputs.push(input);
                }
            },
            seen,
        );
        harness.run_steps(2);
        harness
    }

    fn press<S>(harness: &Harness<'_, S>, modifiers: Modifiers) {
        harness.event_modifiers(
            Event::PointerButton {
                pos: CENTRE,
                button: PointerButton::Primary,
                pressed: true,
                modifiers,
            },
            modifiers,
        );
    }

    fn release<S>(harness: &Harness<'_, S>) {
        harness.event(Event::PointerButton {
            pos: CENTRE,
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        });
    }

    /// Drags from the centre up by `points`, holding `modifiers`, in several steps.
    fn drag_up(harness: &mut Harness<'_, Seen>, points: f32, modifiers: Modifiers) {
        harness.event(Event::PointerMoved(CENTRE));
        harness.run_steps(1);
        press(harness, modifiers);
        harness.run_steps(1);
        for k in 1..=5 {
            let y = CENTRE.y - points * k as f32 / 5.0;
            harness.event_modifiers(Event::PointerMoved(pos2(CENTRE.x, y)), modifiers);
            harness.run_steps(1);
        }
        release(harness);
        harness.run_steps(2);
    }

    #[test]
    fn dragging_up_turns_a_knob_up() {
        let mut harness = knob(0.5);
        drag_up(&mut harness, 75.0, Modifiers::NONE);
        let value = harness.state().value;
        assert!((value - 1.0).abs() < 0.05, "got {value}");
    }

    #[test]
    fn shift_drag_is_four_times_finer() {
        let mut harness = knob(0.5);
        drag_up(&mut harness, 75.0, Modifiers::SHIFT);
        let value = harness.state().value;
        assert!((value - 0.625).abs() < 0.03, "got {value}");
    }

    #[test]
    fn wheel_moves_two_percent() {
        let mut harness = knob(0.5);
        harness.event(Event::PointerMoved(CENTRE));
        harness.run_steps(1);
        harness.event(Event::MouseWheel {
            unit: MouseWheelUnit::Line,
            delta: vec2(0.0, 1.0),
            phase: egui::TouchPhase::Move,
            source: egui::MouseWheelSource::Unknown,
            modifiers: Modifiers::NONE,
        });
        harness.run_steps(10);
        let value = harness.state().value;
        assert!((value - 0.52).abs() < 0.005, "got {value}");
    }

    #[test]
    fn double_click_resets_a_knob() {
        let mut harness = knob(0.25);
        harness.event(Event::PointerMoved(CENTRE));
        harness.run_steps(1);
        for _ in 0..2 {
            press(&harness, Modifiers::NONE);
            release(&harness);
            harness.run_steps(1);
        }
        harness.run_steps(1);
        assert_eq!(harness.state().value, 0.25);
        assert_eq!(harness.state().inputs.last(), Some(&Input::Absolute(0.25)));
    }

    #[test]
    fn fader_click_jumps_there() {
        let seen = Seen::default();
        let mut harness = Harness::new_ui_state(
            |ui, seen: &mut Seen| {
                let response =
                    ui.interact(TRACK, ui.scope_id().with("fader"), Sense::click_and_drag());
                if let Some(input) = fader_input(&response, TRACK, true, None) {
                    seen.inputs.push(input);
                }
            },
            seen,
        );
        harness.run_steps(2);
        // A quarter of the way down the 100-point track, so 0.75 (the top is 1).
        let at = pos2(30.0, 45.0);
        harness.event(Event::PointerMoved(at));
        harness.run_steps(1);
        harness.event(Event::PointerButton {
            pos: at,
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        });
        harness.run_steps(2);
        let Some(Input::Absolute(v)) = harness.state().inputs.first().copied() else {
            panic!("no value: {:?}", harness.state().inputs);
        };
        assert!((v - 0.75).abs() < 1e-3, "got {v}");
    }

    #[test]
    fn button_press_and_release() {
        let seen = Seen::default();
        let mut harness = Harness::new_ui_state(
            |ui, seen: &mut Seen| {
                let rect = Rect::from_center_size(CENTRE, vec2(40.0, 40.0));
                let response =
                    ui.interact(rect, ui.scope_id().with("button"), Sense::click_and_drag());
                if let Some(input) = button_input(&response) {
                    seen.inputs.push(input);
                }
            },
            seen,
        );
        harness.run_steps(2);
        harness.event(Event::PointerMoved(CENTRE));
        harness.run_steps(1);
        press(&harness, Modifiers::NONE);
        harness.run_steps(4);
        assert_eq!(
            harness.state().inputs,
            [Input::Press],
            "held: one press only"
        );
        release(&harness);
        harness.run_steps(3);
        assert_eq!(harness.state().inputs, [Input::Press, Input::Release]);
    }

    /// A button that presses the action in the state, drawn while `drawn`, its commands
    /// sent to `sent` and its held buttons swept at the end of each frame.
    struct Pressing {
        action: Action,
        drawn: bool,
        held: HeldButtons,
        commands: Sender<UiCommand>,
    }

    fn pressing() -> (
        Harness<'static, Pressing>,
        std::sync::mpsc::Receiver<UiCommand>,
    ) {
        let (commands, sent) = std::sync::mpsc::channel();
        let state = Pressing {
            action: Action::Cue(engine::DeckId::new(0).unwrap()),
            drawn: true,
            held: HeldButtons::default(),
            commands,
        };
        let mut harness = Harness::new_ui_state(
            |ui, state: &mut Pressing| {
                if state.drawn {
                    let rect = Rect::from_center_size(CENTRE, vec2(40.0, 40.0));
                    let response =
                        ui.interact(rect, Id::NULL.with("held button"), Sense::click_and_drag());
                    let mut sink = Sink::new(&state.commands, &mut state.held);
                    sink.button(&response, state.action);
                }
                state.held.sweep(ui.ctx(), &state.commands);
            },
            state,
        );
        harness.run_steps(2);
        (harness, sent)
    }

    #[test]
    fn a_release_goes_to_the_action_pressed() {
        let (mut harness, sent) = pressing();
        let deck = engine::DeckId::new(0).unwrap();
        harness.event(Event::PointerMoved(CENTRE));
        harness.run_steps(1);
        press(&harness, Modifiers::NONE);
        harness.run_steps(2);
        harness.state_mut().action = Action::PlayPause(deck);
        harness.run_steps(2);
        release(&harness);
        harness.run_steps(2);
        let got: Vec<UiCommand> = sent.try_iter().collect();
        assert_eq!(
            got,
            [
                UiCommand::Action(Action::Cue(deck), Input::Press),
                UiCommand::Action(Action::Cue(deck), Input::Release),
            ]
        );
    }

    #[test]
    fn a_held_button_no_longer_drawn_is_released() {
        let (mut harness, sent) = pressing();
        let deck = engine::DeckId::new(0).unwrap();
        harness.event(Event::PointerMoved(CENTRE));
        harness.run_steps(1);
        press(&harness, Modifiers::NONE);
        harness.run_steps(2);
        harness.state_mut().drawn = false;
        harness.run_steps(1);
        let cue = Action::Cue(deck);
        let got: Vec<UiCommand> = sent.try_iter().collect();
        assert_eq!(
            got,
            [
                UiCommand::Action(cue, Input::Press),
                UiCommand::Action(cue, Input::Release)
            ]
        );
        // Drawn again and let go: nothing more, the release was sent.
        harness.state_mut().drawn = true;
        harness.run_steps(1);
        release(&harness);
        harness.run_steps(2);
        assert_eq!(sent.try_iter().count(), 0);
        // The next press counts.
        press(&harness, Modifiers::NONE);
        harness.run_steps(2);
        assert_eq!(
            sent.try_iter().collect::<Vec<_>>(),
            [UiCommand::Action(cue, Input::Press)]
        );
    }

    #[test]
    fn a_wheel_that_turns_a_knob_does_not_scroll() {
        // A knob at the top of a tall scroll area; the wheel moves over it.
        let seen = (0.5_f32, 0.0_f32);
        let mut harness = Harness::builder().with_size([200.0, 200.0]).build_ui_state(
            |ui, (value, offset): &mut (f32, f32)| {
                let shown = egui::ScrollArea::vertical().show(ui, |ui| {
                    let (rect, _) = ui.allocate_exact_size(vec2(40.0, 40.0), Sense::hover());
                    let response = ui.interact(
                        rect,
                        Id::NULL.with("scrolled knob"),
                        Sense::click_and_drag(),
                    );
                    if let Some(Input::Absolute(v)) = turn_knob(&response, *value, 0.5, ui) {
                        *value = v;
                    }
                    ui.allocate_space(vec2(40.0, 2000.0));
                });
                *offset = shown.state.clamped_offset().y;
            },
            seen,
        );
        harness.run_steps(2);
        harness.event(Event::PointerMoved(pos2(28.0, 28.0)));
        harness.run_steps(1);
        harness.event(Event::MouseWheel {
            unit: MouseWheelUnit::Line,
            delta: vec2(0.0, -1.0),
            phase: egui::TouchPhase::Move,
            source: egui::MouseWheelSource::Unknown,
            modifiers: Modifiers::NONE,
        });
        harness.run_steps(10);
        let (value, offset) = *harness.state();
        assert!((value - 0.48).abs() < 0.005, "the knob is at {value}");
        assert_eq!(offset, 0.0, "the area scrolled");
    }
}
