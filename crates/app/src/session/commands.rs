//! Applies what the window asks for ([`UiCommand`]s) on the session thread: view changes
//! and the zoom to the [`ViewStore`](super::view::ViewStore), actions to the controller
//! runtime as a MIDI input would.

use std::sync::mpsc::Receiver;
use std::time::Instant;

use actions::{Action, ApplyResult, Input};
use controller::Runtime;
use engine::EngineController;
use ui::UiCommand;

use super::frame::FrameParts;

/// The controller runtime and the engine, once the control loop runs.
pub(super) type Control<'a> = Option<(&'a mut Runtime, &'a mut EngineController)>;

/// Applies every command the window sent since the last call. Before the control loop runs
/// (`control` is `None`) actions are dropped: there is nothing to apply them to.
pub(super) fn drain(commands: &Receiver<UiCommand>, parts: &mut FrameParts, mut control: Control) {
    let now = Instant::now();
    for command in commands.try_iter() {
        match command {
            UiCommand::View(change) => {
                parts.view.apply(change, now);
            }
            UiCommand::Zoom(steps) => {
                parts.view.zoom(steps, None, now);
            }
            UiCommand::Action(action, input) => {
                if let Some((runtime, engine)) = control.as_mut() {
                    action_from_window(&action, input, parts, runtime, engine);
                }
            }
            UiCommand::FxSlot { .. } => todo_command("choosing an FX slot", parts),
            UiCommand::SetSetting { .. } | UiCommand::ResetSettings => {
                todo_command("changing settings", parts);
            }
        }
    }
}

fn action_from_window(
    action: &Action,
    input: Input,
    parts: &mut FrameParts,
    runtime: &mut Runtime,
    engine: &mut EngineController,
) {
    match runtime.apply_ui(action, input, engine) {
        ApplyResult::Sent(_) | ApplyResult::Ignored => {}
        ApplyResult::QueueFull => parts
            .event("warning: a control action was dropped, the engine queue was full".to_owned()),
        // The library's actions (browse, load, beatgrid) are the controller's for now.
        ApplyResult::Request(_) => {
            parts.event(format!(
                "{} cannot be done from the window",
                action.kind().id()
            ));
        }
    }
}

/// A command the session does not handle yet: it says so and ignores it.
fn todo_command(what: &str, parts: &mut FrameParts) {
    parts.event(format!("{what} from the window is not available yet"));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use actions::{PadMode, Side};
    use std::sync::mpsc::channel;

    #[test]
    fn ui_actions_apply_like_midi() {
        let mapping = controller::builtin::load_mapping_from("ddj-sb3", None).unwrap();
        let mut runtime = Runtime::new(mapping);
        let (mut engine, _renderer) = engine::engine(engine::EngineConfig {
            sample_rate: 44_100,
            max_block_frames: 512,
        });
        let mut parts = FrameParts::new(&Config::default());
        let (tx, rx) = channel();
        let press = UiCommand::Action(Action::PadMode(Side::Left, PadMode::BeatJump), Input::Press);
        tx.send(press.clone()).unwrap();

        // Before the loop runs there is nothing to apply it to.
        drain(&rx, &mut parts, None);
        assert_eq!(runtime.state().side(Side::Left).pad_mode, PadMode::Hardware);

        tx.send(press).unwrap();
        drain(&rx, &mut parts, Some((&mut runtime, &mut engine)));
        assert_eq!(runtime.state().side(Side::Left).pad_mode, PadMode::BeatJump);
        assert_eq!(
            runtime.state().side(Side::Right).pad_mode,
            PadMode::Hardware
        );
    }

    #[test]
    fn view_and_zoom_commands_change_the_view() {
        let mut parts = FrameParts::new(&Config::default());
        let (tx, rx) = channel();
        tx.send(UiCommand::View(ui::ViewChange::SetFourDecks(true)))
            .unwrap();
        tx.send(UiCommand::Zoom(1)).unwrap();
        tx.send(UiCommand::ResetSettings).unwrap();
        drain(&rx, &mut parts, None);
        let frame = parts.idle();
        assert!(frame.view.four_decks);
        assert!((frame.view.waveform_beats - 12.8).abs() < 1e-9);
        assert_eq!(
            frame.events.back().unwrap().1,
            "changing settings from the window is not available yet"
        );
    }
}
