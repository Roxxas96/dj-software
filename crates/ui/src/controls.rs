//! Mouse gestures for the window's controls: what a drag, a wheel move, a click or a
//! double-click on a knob, a fader or a button means, as the [`Input`] a controller would
//! send for the same control.
//!
//! These read the [`Response`] and the input state only; they draw nothing and send
//! nothing. The caller allocates the widget with the sense it needs (drag for a knob,
//! click and drag for a fader or a button) and sends what comes back as a
//! [`UiCommand::Action`](crate::UiCommand).

use actions::Input;
use egui::{PointerButton, Rect, Response, Ui};

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
    let held_id = response.id.with("button_held");
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
}
