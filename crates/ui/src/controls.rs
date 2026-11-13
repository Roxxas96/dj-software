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

/// The knob's new value for a drag, a wheel move, a double-click or a right-click on it,
/// or `None` when nothing happened. Dragging up raises the value; Shift drags a quarter as
/// fast. A double-click or a right-click (on macOS, Ctrl + click too: that press turns
/// nothing) gives `reset`. The result is clamped to `0..=1`. Call it every frame the knob
/// is shown.
pub fn knob_input(response: &Response, value: f32, reset: f32, ui: &Ui) -> Option<Input> {
    let press = context_press(response);
    if press.opened || response.double_clicked_by(PointerButton::Primary) {
        return Some(Input::Absolute(reset.clamp(0.0, 1.0)));
    }
    let mut delta = 0.0;
    if response.dragged_by(PointerButton::Primary) && !press.right_click {
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

/// How far, in points, the pointer must go from where it was pressed before a press is a
/// drag: egui's own click distance.
pub const DRAG_DISTANCE: f32 = 6.0;

/// Whether the primary-button press on `response` is a drag that has moved: the pointer
/// went [`DRAG_DISTANCE`] from the press at some point of it. egui also calls a press held
/// still past its click time a drag; that one is no drag here, so a slow click grabs
/// nothing. Call it every frame the widget is shown, so it forgets a drag that ended.
pub fn moved_drag(response: &Response) -> bool {
    let key = response.id.with("drag_moved");
    if !response.dragged_by(PointerButton::Primary) {
        response.ctx.data_mut(|d| d.remove::<bool>(key));
        return false;
    }
    if response
        .ctx
        .data(|d| d.get_temp::<bool>(key))
        .unwrap_or(false)
    {
        return true;
    }
    let moved = response.ctx.input(|i| {
        let pointer = &i.pointer;
        pointer
            .press_origin()
            .zip(pointer.latest_pos())
            .is_some_and(|(from, to)| from.distance(to) >= DRAG_DISTANCE)
    });
    if moved {
        response.ctx.data_mut(|d| d.insert_temp(key, true));
    }
    moved
}

/// Where the primary-button drag of `response` went since the last frame, as the pointer's
/// previous and present positions, or `None` when it did not move or is not a drag that
/// has moved ([`moved_drag`]). The first leg runs from the press itself: the points that
/// told the drag from a click are not lost.
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
    if !moved_drag(response) {
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

/// A fader's handle, for [`grab_fader`]: where it is (the fader's value, as
/// [`fader_input`] maps it) and how far it reaches either side of that point along the
/// track, in points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Handle {
    pub value: f32,
    pub reach: f32,
}

/// What a fader keeps for the length of a press: how far the pointer is from the point it
/// sets (0, or where it took the handle), and the value it sent last.
#[derive(Debug, Clone, Copy, Default)]
struct FaderPress {
    offset: f32,
    sent: Option<f32>,
}

/// The fader's value for a click or a drag at the pointer, mapped along `track` (the
/// handle's travel): a click jumps there, a drag follows. A vertical fader has 1 at the
/// top, a horizontal one at the right; a control that runs the other way inverts the
/// value. A double-click or a right-click gives `reset` when there is one; a fader without
/// one ignores a right-click. On macOS Ctrl + click is a right-click: that press moves
/// nothing. `None` when the pointer is not pressing the fader, or the value is the one it
/// sent last in this press: a press held still sends once. A tap (the press and the
/// release in one frame) sets the value too. Call it every frame the fader is shown.
pub fn fader_input(
    response: &Response,
    track: Rect,
    vertical: bool,
    reset: Option<f32>,
) -> Option<Input> {
    fader_press(response, track, vertical, reset, None)
}

/// [`fader_input`] for a fader whose `handle` the pointer can take: a press on the handle
/// moves it from where it is, by as far as the pointer goes, without a jump; a press on
/// the track elsewhere jumps there.
pub fn grab_fader(
    response: &Response,
    track: Rect,
    vertical: bool,
    reset: Option<f32>,
    handle: Handle,
) -> Option<Input> {
    fader_press(response, track, vertical, reset, Some(handle))
}

fn fader_press(
    response: &Response,
    track: Rect,
    vertical: bool,
    reset: Option<f32>,
    handle: Option<Handle>,
) -> Option<Input> {
    let ctx = &response.ctx;
    let key = response.id.with("fader_press");
    let context = context_press(response);
    if let Some(reset) = reset
        && (context.opened || response.double_clicked_by(PointerButton::Primary))
    {
        ctx.data_mut(|d| d.remove::<FaderPress>(key));
        return Some(Input::Absolute(reset.clamp(0.0, 1.0)));
    }
    if context.right_click {
        ctx.data_mut(|d| d.remove::<FaderPress>(key));
        return None;
    }
    let pressing = response.dragged_by(PointerButton::Primary)
        || (response.is_pointer_button_down_on() && ctx.input(|i| i.pointer.primary_down()));
    let tapped = !pressing && is_tap(response);
    let pos = response
        .interact_pointer_pos()
        .filter(|_| pressing || tapped);
    let Some(pos) = pos else {
        ctx.data_mut(|d| d.remove::<FaderPress>(key));
        return None;
    };
    // Along the track: points from its start, and the value there.
    let along = |p: Pos2| if vertical { p.y } else { p.x };
    let length = if vertical {
        track.height()
    } else {
        track.width()
    }
    .max(1.0);
    let start = along(track.min);
    let value_at = |at: f32| {
        let share = (at - start) / length;
        if vertical { 1.0 - share } else { share }
    };
    let point_of = |value: f32| {
        let share = if vertical { 1.0 - value } else { value };
        start + share * length
    };
    let mut press = ctx
        .data(|d| d.get_temp::<FaderPress>(key))
        .unwrap_or_else(|| {
            let origin = ctx.input(|i| i.pointer.press_origin()).unwrap_or(pos);
            let offset = handle
                .map(|h| point_of(h.value.clamp(0.0, 1.0)) - along(origin))
                .filter(|offset| offset.abs() <= handle.map_or(0.0, |h| h.reach))
                .unwrap_or(0.0);
            FaderPress { offset, sent: None }
        });
    let value = value_at(along(pos) + press.offset).clamp(0.0, 1.0);
    let changed = press.sent != Some(value);
    press.sent = Some(value);
    ctx.data_mut(|d| {
        if tapped {
            d.remove::<FaderPress>(key);
        } else {
            d.insert_temp(key, press);
        }
    });
    changed.then_some(Input::Absolute(value))
}

/// Whether the primary button went down and up on `response` within this one frame, as a
/// trackpad tap can: no frame saw it held.
fn is_tap(response: &Response) -> bool {
    response.clicked_by(PointerButton::Primary)
        && response
            .ctx
            .input(|i| i.pointer.button_pressed(PointerButton::Primary))
}

/// `Press` on the frame the pointer goes down on the button, `Release` on the frame that
/// press ends (the pointer comes up anywhere), `None` in between: a held button never
/// repeats. A tap (down and up in one frame) gives `Press`, then `Release` the next frame.
/// Whether the button is held is kept in egui's memory under the widget's id. egui keeps
/// the pointer down when the window loses focus: [`HeldButtons::sweep`] releases the
/// buttons then.
pub fn button_input(response: &Response) -> Option<Input> {
    let down = response.ctx.input(|i| i.pointer.primary_down());
    let pressing = response.is_pointer_button_down_on() && down || is_tap(response);
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

/// How a primary press on a control with a right-click menu counts, from
/// [`context_press`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextPress {
    /// The press is a right-click (on macOS, it began with Ctrl held): it presses nothing.
    pub right_click: bool,
    /// The control was right-clicked this frame: open its menu.
    pub opened: bool,
}

/// How the press on `response` counts. On macOS Ctrl + click is a right-click, as the
/// system has it: decided when the press begins, and kept until it ends, whatever Ctrl
/// does meanwhile. Call it every frame the control is shown.
pub fn context_press(response: &Response) -> ContextPress {
    let ctx = &response.ctx;
    let key = response.id.with("ctrl_right_click");
    let latched = ctx.data(|d| d.get_temp::<bool>(key));
    let right_click =
        latched.unwrap_or_else(|| cfg!(target_os = "macos") && ctx.input(|i| i.modifiers.ctrl));
    ctx.data_mut(|d| {
        if response.is_pointer_button_down_on() {
            d.insert_temp(key, right_click);
        } else {
            d.remove::<bool>(key);
        }
    });
    let opened =
        response.secondary_clicked() || right_click && response.clicked_by(PointerButton::Primary);
    ContextPress {
        right_click,
        opened,
    }
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

    /// The release of the button of `response`, which presses `action`, if it is held and
    /// its press ended this frame; never a press. For a control that presses only on a
    /// drag, to deliver the release once the drag is over.
    pub fn release(&mut self, response: &Response, action: Action) -> Option<UiCommand> {
        let held = response
            .ctx
            .data(|d| d.get_temp::<bool>(button_held(response.id)))
            .unwrap_or(false);
        if held {
            self.button(response, action)
        } else {
            None
        }
    }

    /// Ends the frame: releases every held button that was not drawn in it, or all of them
    /// when the window has lost focus (egui keeps the pointer down then), sending the
    /// releases to `commands`. Once the pointer is up, the buttons released that way are
    /// forgotten, so the next press on one counts.
    pub fn sweep(&mut self, ctx: &egui::Context, commands: &Sender<UiCommand>) {
        if !self.swept.is_empty() && !ctx.input(|i| i.pointer.primary_down()) {
            for id in self.swept.drain(..) {
                ctx.data_mut(|d| d.remove::<bool>(button_held(id)));
            }
        }
        let focused = ctx.input(|i| i.focused);
        let swept = &mut self.swept;
        self.held.retain_mut(|held| {
            if std::mem::take(&mut held.drawn) && focused {
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

    /// Sends the release of the button of `response`, which presses `action`, if its
    /// press ended this frame ([`HeldButtons::release`]).
    pub fn release(&mut self, response: &Response, action: Action) {
        if let Some(command) = self.held.release(response, action) {
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

    /// A fader on [`TRACK`] whose inputs the state records.
    fn fader_harness() -> Harness<'static, Seen> {
        let mut harness = Harness::builder().with_step_dt(0.01).build_ui_state(
            |ui, seen: &mut Seen| {
                let response =
                    ui.interact(TRACK, ui.scope_id().with("fader"), Sense::click_and_drag());
                if let Some(input) = fader_input(&response, TRACK, true, None) {
                    seen.inputs.push(input);
                }
            },
            Seen::default(),
        );
        harness.run_steps(2);
        harness
    }

    fn pointer_at(at: Pos2, pressed: bool) -> Event {
        Event::PointerButton {
            pos: at,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        }
    }

    /// A trackpad tap at `at`: the press and the release reach the same frame (kittest's
    /// queued events take a frame each).
    pub(crate) fn tap<S>(harness: &mut Harness<'_, S>, at: Pos2) {
        let events = &mut harness.input_mut().events;
        events.push(pointer_at(at, true));
        events.push(pointer_at(at, false));
        harness.step();
    }

    /// The window gains or loses focus, as eframe tells egui: the event and the flag.
    pub(crate) fn focus<S>(harness: &mut Harness<'_, S>, focused: bool) {
        let input = harness.input_mut();
        input.focused = focused;
        input.events.push(Event::WindowFocused(focused));
        harness.step();
    }

    #[test]
    fn a_still_press_on_a_fader_sends_once() {
        let mut harness = fader_harness();
        let at = pos2(30.0, 45.0);
        harness.event(Event::PointerMoved(at));
        harness.run_steps(1);
        harness.event(pointer_at(at, true));
        harness.run_steps(10);
        assert_eq!(harness.state().inputs, [Input::Absolute(0.75)]);
        // A move sends the new value, once.
        harness.event(Event::PointerMoved(pos2(30.0, 70.0)));
        harness.run_steps(5);
        assert_eq!(
            harness.state().inputs,
            [Input::Absolute(0.75), Input::Absolute(0.5)]
        );
        harness.event(pointer_at(pos2(30.0, 70.0), false));
        harness.run_steps(2);
        assert_eq!(harness.state().inputs.len(), 2);
    }

    #[test]
    fn a_tap_on_a_fader_sets_it() {
        let mut harness = fader_harness();
        let at = pos2(30.0, 45.0);
        harness.event(Event::PointerMoved(at));
        harness.run_steps(1);
        tap(&mut harness, at);
        harness.run_steps(3);
        assert_eq!(harness.state().inputs, [Input::Absolute(0.75)]);
    }

    #[test]
    fn a_tap_presses_and_releases_a_button() {
        let (mut harness, sent) = pressing();
        harness.event(Event::PointerMoved(CENTRE));
        harness.run_steps(1);
        tap(&mut harness, CENTRE);
        harness.run_steps(3);
        let cue = Action::Cue(engine::DeckId::new(0).unwrap());
        assert_eq!(
            sent.try_iter().collect::<Vec<_>>(),
            [
                UiCommand::Action(cue, Input::Press),
                UiCommand::Action(cue, Input::Release)
            ]
        );
    }

    #[test]
    fn losing_focus_releases_a_held_button() {
        let (mut harness, sent) = pressing();
        harness.event(Event::PointerMoved(CENTRE));
        harness.run_steps(1);
        press(&harness, Modifiers::NONE);
        harness.run_steps(2);
        let cue = Action::Cue(engine::DeckId::new(0).unwrap());
        assert_eq!(
            sent.try_iter().collect::<Vec<_>>(),
            [UiCommand::Action(cue, Input::Press)]
        );
        // egui keeps the button down when the window loses focus.
        focus(&mut harness, false);
        harness.run_steps(2);
        assert_eq!(
            sent.try_iter().collect::<Vec<_>>(),
            [UiCommand::Action(cue, Input::Release)]
        );
        // Back, and let go: the release was sent; the next press counts.
        focus(&mut harness, true);
        release(&harness);
        harness.run_steps(2);
        assert_eq!(sent.try_iter().count(), 0);
        press(&harness, Modifiers::NONE);
        harness.run_steps(2);
        assert_eq!(
            sent.try_iter().collect::<Vec<_>>(),
            [UiCommand::Action(cue, Input::Press)]
        );
    }

    #[test]
    fn a_press_on_the_handle_takes_it_without_a_jump() {
        // The handle at 0.5: the middle of the 100-point track, y = 70, reaching 4 points.
        let handle = Handle {
            value: 0.5,
            reach: 4.0,
        };
        let mut harness = Harness::builder().with_step_dt(0.01).build_ui_state(
            move |ui, seen: &mut Seen| {
                let response =
                    ui.interact(TRACK, ui.scope_id().with("fader"), Sense::click_and_drag());
                if let Some(input) = grab_fader(&response, TRACK, true, None, handle) {
                    seen.inputs.push(input);
                }
            },
            Seen::default(),
        );
        harness.run_steps(2);
        // Pressed 3 points under its middle: it stays where it is, then follows the drag.
        let at = pos2(30.0, 73.0);
        harness.event(Event::PointerMoved(at));
        harness.run_steps(1);
        harness.event(pointer_at(at, true));
        harness.run_steps(2);
        harness.event(Event::PointerMoved(pos2(30.0, 63.0)));
        harness.run_steps(2);
        harness.event(pointer_at(pos2(30.0, 63.0), false));
        harness.run_steps(2);
        let values: Vec<f32> = harness
            .state()
            .inputs
            .iter()
            .map(|i| match i {
                Input::Absolute(v) => *v,
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(values.len(), 2, "{values:?}");
        assert!((values[0] - 0.5).abs() < 1e-4, "{values:?}");
        assert!((values[1] - 0.6).abs() < 1e-4, "{values:?}");
        // Off the handle, the track jumps.
        let at = pos2(30.0, 45.0);
        harness.event(Event::PointerMoved(at));
        harness.run_steps(1);
        harness.event(pointer_at(at, true));
        harness.run_steps(2);
        assert_eq!(harness.state().inputs.last(), Some(&Input::Absolute(0.75)));
    }

    #[test]
    fn a_press_held_still_is_no_drag() {
        let mut harness = Harness::builder().with_step_dt(0.01).build_ui_state(
            |ui, moved: &mut Vec<bool>| {
                let rect = Rect::from_center_size(CENTRE, vec2(40.0, 40.0));
                let response = ui.interact(rect, Id::NULL.with("drag"), Sense::click_and_drag());
                moved.push(moved_drag(&response));
            },
            Vec::new(),
        );
        harness.event(Event::PointerMoved(CENTRE));
        harness.run_steps(1);
        press(&harness, Modifiers::NONE);
        // A second and more: egui calls it a drag by now.
        harness.run_steps(110);
        assert!(!harness.state().iter().any(|&m| m));
        // 3 points is still a click; 6 is a drag, and it stays one back near the press.
        harness.event(Event::PointerMoved(CENTRE + vec2(3.0, 0.0)));
        harness.run_steps(1);
        assert_eq!(harness.state().last(), Some(&false));
        harness.event(Event::PointerMoved(CENTRE + vec2(6.0, 0.0)));
        harness.run_steps(1);
        assert_eq!(harness.state().last(), Some(&true));
        harness.event(Event::PointerMoved(CENTRE));
        harness.run_steps(1);
        assert_eq!(harness.state().last(), Some(&true));
        release(&harness);
        harness.run_steps(1);
        assert_eq!(harness.state().last(), Some(&false));
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
