//! Applies what the window asks for ([`UiCommand`]s) on the session thread: view changes
//! and the zoom to the [`ViewStore`](super::view::ViewStore), actions to the controller
//! runtime as a MIDI input would, settings to `config.toml` and, for the live ones, to
//! the engine, the controller runtime and the view.

use std::sync::mpsc::Receiver;
use std::time::Instant;

use actions::{Action, ApplyResult, ControlState, Input};
use controller::Runtime;
use engine::{
    ChannelCommand, Command, DeckId, EffectKind, EngineController, MasterCommand, SamplerCommand,
};
use ui::UiCommand;

use super::frame::FrameParts;
use super::settings::{Applied, Settings};
use crate::config::{Config, StemsConfig};

/// The controller runtime and the engine, once the control loop runs.
pub(super) type Control<'a> = Option<(&'a mut Runtime, &'a mut EngineController)>;

/// Applies every command the window sent since the last call. Before the control loop runs
/// (`control` is `None`) actions and effect choices are dropped: there is nothing to apply
/// them to. Settings are saved at once; their live changes reach the engine and the
/// controller once the loop runs.
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
            UiCommand::FxSlot { deck, slot, kind } => {
                if let Some((_, engine)) = control.as_mut() {
                    fx_from_window(deck, slot, kind, parts, engine);
                }
            }
            UiCommand::SetSetting { key, value } => {
                change_settings(parts, |settings| settings.set(&key, &value), now);
            }
            UiCommand::ResetSettings => {
                change_settings(parts, Settings::reset_all, now);
            }
        }
    }
    if let Some(settings) = &parts.settings {
        for error in settings.save_errors() {
            parts.event(format!("warning: {error}"));
        }
    }
    if let Some((runtime, engine)) = control {
        apply_live(parts, runtime.control_state_mut(), engine);
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

/// Puts effect `kind` in `deck`'s FX `slot` and shows it in the frames once the engine
/// has the command.
fn fx_from_window(
    deck: DeckId,
    slot: usize,
    kind: EffectKind,
    parts: &mut FrameParts,
    engine: &mut EngineController,
) {
    let Some(shown) = parts.fx_slots[deck.index()].get_mut(slot) else {
        return;
    };
    let command = Command::Channel(deck, ChannelCommand::FxSlot(slot, kind));
    if engine.send(command).is_err() {
        parts.event("warning: an effect choice was dropped, the engine queue was full".to_owned());
        return;
    }
    *shown = kind;
    let key = format!("fx.deck{}", deck.index() + 1);
    let ids = parts.fx_slots[deck.index()].map(|kind| format!("\"{}\"", kind.id()));
    let value = format!("[{}]", ids.join(", "));
    change_settings(parts, |settings| settings.set(&key, &value), Instant::now());
}

/// Saves a settings `change` and, if it was saved, applies the view's part of it at once
/// and leaves the rest for [`apply_live`]. A refused change shows on its row.
fn change_settings(
    parts: &mut FrameParts,
    change: impl FnOnce(&mut Settings) -> Result<Applied, String>,
    now: Instant,
) {
    let Some(settings) = parts.settings.as_mut() else {
        parts.event("settings cannot be saved: there is no configuration folder".to_owned());
        return;
    };
    let before = settings.config().ui.clone();
    let result = change(settings);
    parts.refresh_settings();
    let Ok(applied) = result else {
        return;
    };
    if !applied.live {
        parts.event("settings saved; some apply at the next launch".to_owned());
    }
    let ui = &applied.config.ui;
    if *ui != before {
        let beats = (ui.waveform_beats != before.waveform_beats).then_some(ui.waveform_beats);
        parts.view.set_zoom_range(ui.zoom_range(), beats, now);
    }
    parts.unapplied = Some(applied.config);
}

/// Sends the live settings of the configuration saved last that the engine and the
/// controller do not have yet. What the engine queue cannot take now (it is full) is
/// tried again on the next call.
fn apply_live(parts: &mut FrameParts, state: &mut ControlState, engine: &mut EngineController) {
    let Some(new) = parts.unapplied.take() else {
        return;
    };
    let mut done = true;
    let mut send = |command| {
        let sent = engine.send(command).is_ok();
        done &= sent;
        sent
    };
    let old = &mut parts.applied;
    if old.crossfader != new.crossfader
        && send(Command::Master(MasterCommand::CrossfaderEnabled(
            new.crossfader,
        )))
    {
        old.crossfader = new.crossfader;
        parts.crossfader_enabled = new.crossfader;
    }
    if old.stems.knobs != new.stems.knobs
        && send(Command::Master(MasterCommand::StemKnobs(new.stems.knobs)))
    {
        old.stems.knobs = new.stems.knobs;
        parts.stem_knobs = new.stems.knobs;
    }
    if old.stems.eq_mode != new.stems.eq_mode {
        // `stems.enabled` stays as it was at launch.
        let mode = StemsConfig {
            eq_mode: new.stems.eq_mode,
            ..old.stems.clone()
        }
        .startup_eq_mode();
        if send(Command::Master(MasterCommand::EqMode(mode))) {
            old.stems.eq_mode = new.stems.eq_mode;
            parts.eq_mode = mode;
        }
    }
    if (old.fx_slots, old.fx_decks) != (new.fx_slots, new.fx_decks) {
        let mut sent = true;
        for deck in DeckId::ALL {
            let shown = &mut parts.fx_slots[deck.index()];
            for (slot, kind) in new.fx_slots_for(deck).into_iter().enumerate() {
                if shown[slot] != kind {
                    if send(Command::Channel(deck, ChannelCommand::FxSlot(slot, kind))) {
                        shown[slot] = kind;
                    } else {
                        sent = false;
                    }
                }
            }
        }
        if sent {
            (old.fx_slots, old.fx_decks) = (new.fx_slots, new.fx_decks);
        }
    }
    if old.sampler.volume != new.sampler.volume
        && send(Command::Sampler(SamplerCommand::Volume(new.sampler.volume)))
    {
        old.sampler.volume = new.sampler.volume;
    }
    set_pad_sizes(state, old, &new);
    if !done {
        parts.unapplied = Some(new);
    }
}

/// The pad sizes and backspin lengths: controller state, not engine values, so there is
/// no soft takeover to arm. A key taken out of the file is back to its default.
fn set_pad_sizes(state: &mut ControlState, old: &mut Config, new: &Config) {
    let defaults = ControlState::default();
    if old.beat_jump_pads != new.beat_jump_pads {
        state.beat_jump_pads = new.beat_jump_pads.unwrap_or(defaults.beat_jump_pads);
        old.beat_jump_pads = new.beat_jump_pads;
    }
    if old.beat_jump_shift_pads != new.beat_jump_shift_pads {
        state.beat_jump_shift_pads = new
            .beat_jump_shift_pads
            .unwrap_or(defaults.beat_jump_shift_pads);
        old.beat_jump_shift_pads = new.beat_jump_shift_pads;
    }
    if old.loop_roll_pads != new.loop_roll_pads {
        state.loop_roll_pads = new.loop_roll_pads.unwrap_or(defaults.loop_roll_pads);
        old.loop_roll_pads = new.loop_roll_pads;
    }
    if old.backspin_short != new.backspin_short {
        state.backspin_short = new.backspin_short.unwrap_or(defaults.backspin_short);
        old.backspin_short = new.backspin_short;
    }
    if old.backspin_long != new.backspin_long {
        state.backspin_long = new.backspin_long.unwrap_or(defaults.backspin_long);
        old.backspin_long = new.backspin_long;
    }
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
    fn a_dropdown_fx_choice_reaches_the_engine() {
        let mapping = controller::builtin::load_mapping_from("ddj-sb3", None).unwrap();
        let mut runtime = Runtime::new(mapping);
        let (mut engine, _renderer) = engine::engine(engine::EngineConfig {
            sample_rate: 44_100,
            max_block_frames: 512,
        });
        let mut parts = FrameParts::new(&Config::default());
        let (tx, rx) = channel();
        let deck = DeckId::new(0).unwrap();
        let choose = UiCommand::FxSlot {
            deck,
            slot: 1,
            kind: EffectKind::Filter,
        };
        tx.send(choose.clone()).unwrap();

        // Before the loop runs there is no engine to send it to.
        drain(&rx, &mut parts, None);
        assert_eq!(parts.idle().fx_slots[0][1], EffectKind::Reverb);

        tx.send(choose).unwrap();
        drain(&rx, &mut parts, Some((&mut runtime, &mut engine)));
        let frame = parts.idle();
        assert_eq!(frame.fx_slots[0][1], EffectKind::Filter);
        // The other decks keep theirs.
        assert_eq!(frame.fx_slots[1][1], EffectKind::Reverb);
    }

    #[test]
    fn settings_are_saved_and_the_live_ones_applied() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let text = "# mine\n[backspin]\nshort = 1.0\n";
        std::fs::write(&path, text).unwrap();
        let config = crate::config::parse_text(text, "config.toml").unwrap();
        let mapping = controller::builtin::load_mapping_from("ddj-sb3", None).unwrap();
        let mut runtime = Runtime::new(mapping);
        let (mut engine, _renderer) = engine::engine(engine::EngineConfig {
            sample_rate: 44_100,
            max_block_frames: 512,
        });
        let mut parts = FrameParts::new(&config);
        parts.set_settings(Settings::load(path.clone(), &config));
        assert_eq!(parts.idle().settings.len(), 25);
        let (tx, rx) = channel();
        for (key, value) in [
            ("stems.knobs", "\"raw\""),
            ("mixer.crossfader", "false"),
            ("backspin.short", "3"),
            ("beat_jump.pads", "[1, -1, 2, -2, 3, -3, 4, -4]"),
            ("fx.slots", r#"["filter", "echo", "reverb"]"#),
            ("ui.waveform_min_beats", "8"),
            ("ui.waveform_beats", "32"),
            ("stems.kicks_hz", "10"),
        ] {
            tx.send(UiCommand::SetSetting {
                key: key.to_owned(),
                value: value.to_owned(),
            })
            .unwrap();
        }

        // Before the loop runs only the view changes; the rest waits for the engine.
        drain(&rx, &mut parts, None);
        let frame = parts.idle();
        assert_eq!(frame.view.waveform_beats, 32.0);
        assert_eq!(frame.stem_knobs, engine::StemKnobs::Vdj);
        let row = |frame: &ui::ControlFrame, key: &str| {
            frame
                .settings
                .iter()
                .find(|row| row.key == key)
                .unwrap()
                .clone()
        };
        assert_eq!(row(&frame, "stems.knobs").value, "\"raw\"");
        assert!(row(&frame, "stems.kicks_hz").error.is_some());

        drain(&rx, &mut parts, Some((&mut runtime, &mut engine)));
        let frame = parts.idle();
        assert_eq!(frame.stem_knobs, engine::StemKnobs::Raw);
        assert!(!frame.crossfader_enabled);
        assert_eq!(
            frame.fx_slots[3],
            [EffectKind::Filter, EffectKind::Echo, EffectKind::Reverb]
        );
        let state = runtime.state();
        assert_eq!(state.backspin_short, 3.0);
        assert_eq!(state.beat_jump_pads[2], 2.0);
        // The zoom range is the new one.
        tx.send(UiCommand::Zoom(20)).unwrap();
        drain(&rx, &mut parts, None);
        assert_eq!(parts.idle().view.waveform_beats, 8.0);

        // The dropdown's choice is saved as the deck's own effects.
        let deck2 = DeckId::new(1).unwrap();
        tx.send(UiCommand::FxSlot {
            deck: deck2,
            slot: 2,
            kind: EffectKind::Flanger,
        })
        .unwrap();
        // A next-launch setting is saved and pending.
        tx.send(UiCommand::SetSetting {
            key: "audio.device".to_owned(),
            value: "\"DDJ-SB3\"".to_owned(),
        })
        .unwrap();
        drain(&rx, &mut parts, Some((&mut runtime, &mut engine)));
        let frame = parts.idle();
        assert_eq!(frame.fx_slots[1][2], EffectKind::Flanger);
        assert!(row(&frame, "audio.device").pending);
        assert_eq!(
            frame.events.back().unwrap().1,
            "settings saved; some apply at the next launch"
        );
        parts.settings.as_mut().unwrap().wait_for_saves();
        let saved = std::fs::read_to_string(&path).unwrap();
        assert!(
            saved.starts_with("# mine\n[backspin]\nshort = 3\n"),
            "{saved}"
        );
        assert!(
            saved.contains(r#"deck2 = ["filter", "echo", "flanger"]"#),
            "{saved}"
        );

        // Reset: the pads and the knobs are back to their defaults.
        tx.send(UiCommand::ResetSettings).unwrap();
        drain(&rx, &mut parts, Some((&mut runtime, &mut engine)));
        let frame = parts.idle();
        assert_eq!(frame.stem_knobs, engine::StemKnobs::Vdj);
        assert_eq!(frame.fx_slots[1], Config::default().fx_slots);
        assert_eq!(runtime.state().backspin_short, 1.0);
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
            "settings cannot be saved: there is no configuration folder"
        );
    }
}
