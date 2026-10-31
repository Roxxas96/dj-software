use std::collections::HashSet;
use std::sync::Arc;

use actions::{
    Action, ActionKind, ApplyResult, ControlState, Input, Led, Side, StateKey, Target, apply, led,
};
use engine::{
    DeckId, DeckSnapshot, EngineConfig, EngineController, EngineRenderer, TempoRange, TrackBuffer,
    engine,
};

const SR: u32 = 44_100;
const BLOCK: usize = 512;
/// Frames after which the mixer's gain ramps and EQ isolator have settled (see the engine
/// tests' `SETTLE_FRAMES`).
const SETTLE_FRAMES: usize = 4096;

struct Rig {
    c: EngineController,
    r: EngineRenderer,
    state: ControlState,
}

impl Rig {
    /// An engine with a constant track loaded on deck 1, and a default control state.
    fn new() -> Self {
        let (mut c, r) = engine(EngineConfig {
            sample_rate: SR,
            max_block_frames: BLOCK,
        });
        let track = Arc::new(TrackBuffer::new(SR, vec![0.5; 200_000 * 2]).unwrap());
        c.send(engine::Command::Deck(
            d(0),
            engine::DeckCommand::Load(track),
        ))
        .unwrap();
        let mut rig = Self {
            c,
            r,
            state: ControlState::default(),
        };
        rig.render(1);
        rig
    }

    /// Applies `input` to `action` and renders one block so the engine takes it in.
    fn apply(&mut self, action: Action, input: Input) -> ApplyResult {
        let result = apply(&action, input, &mut self.state, &mut self.c);
        self.render(1);
        result
    }

    /// Renders `frames` frames and returns the last headphone sample (left).
    fn render(&mut self, frames: usize) -> f32 {
        let mut master = vec![0.0; frames * 2];
        let mut headphones = vec![0.0; frames * 2];
        for (m, h) in master
            .chunks_mut(BLOCK * 2)
            .zip(headphones.chunks_mut(BLOCK * 2))
        {
            self.r.render(m, h);
        }
        headphones[headphones.len() - 2]
    }
}

fn d(index: usize) -> DeckId {
    DeckId::new(index).unwrap()
}

fn deck_action(kind: ActionKind, deck: usize) -> Action {
    Action::new(kind, Target::Deck(d(deck))).unwrap()
}

fn global_action(kind: ActionKind) -> Action {
    Action::new(kind, Target::Global).unwrap()
}

#[test]
fn ids_round_trip_for_every_action() {
    let expected = [
        "deck.play_pause",
        "deck.cue",
        "deck.tempo",
        "deck.tempo_range_next",
        "deck.return_to_start",
        "channel.trim",
        "channel.eq_high",
        "channel.eq_mid",
        "channel.eq_low",
        "channel.filter",
        "channel.fader",
        "channel.pfl_toggle",
        "master.crossfader",
        "master.level",
        "master.headphone_level",
        "master.headphone_mix",
        "master.headphone_master_toggle",
        "side.deck_toggle",
        "modifier.shift",
    ];
    let ids: HashSet<_> = ActionKind::ALL.iter().map(|k| k.id()).collect();
    assert_eq!(ids, expected.into_iter().collect::<HashSet<_>>());
    assert_eq!(ids.len(), ActionKind::ALL.len(), "ids are unique");

    for kind in ActionKind::ALL {
        assert_eq!(Action::from_id(kind.id()), Some(kind));
        assert_eq!(ActionKind::from_id(kind.id()), Some(kind));
        let target = match kind.id().split('.').next().unwrap() {
            "deck" | "channel" => Target::Deck(d(2)),
            "side" | "modifier" => Target::Side(Side::Right),
            _ => Target::Global,
        };
        let action = Action::new(kind, target).unwrap();
        assert_eq!(action.id(), kind.id());
        assert_eq!(action.kind(), kind);
        assert_eq!(action.target(), target);
        // A kind only becomes an action with the right kind of target.
        let wrong = match target {
            Target::Global => Target::Deck(d(0)),
            _ => Target::Global,
        };
        assert_eq!(Action::new(kind, wrong), None);
    }
    let with_release: Vec<_> = ActionKind::ALL
        .into_iter()
        .filter(|k| k.takes_release())
        .collect();
    assert_eq!(with_release, [ActionKind::Cue, ActionKind::Shift]);
    assert_eq!(Action::from_id("deck.nope"), None);
    assert_eq!(Action::from_id(""), None);
}

#[test]
fn state_key_ids_round_trip() {
    let expected = [
        (StateKey::DeckPlaying, "deck.playing"),
        (StateKey::DeckPlayLed, "deck.play_led"),
        (StateKey::DeckCueLed, "deck.cue_led"),
        (StateKey::ChannelPfl, "channel.pfl"),
        (StateKey::SideAltDeck, "side.alt_deck"),
        (StateKey::HeadphoneMaster, "master.headphone_master"),
        (StateKey::Shift, "side.shift"),
    ];
    assert_eq!(StateKey::ALL.len(), expected.len());
    for (key, id) in expected {
        assert_eq!(key.id(), id);
        assert_eq!(StateKey::from_id(id), Some(key));
    }
    assert_eq!(StateKey::from_id("deck_playing"), None);
}

#[test]
fn side_of_deck() {
    assert_eq!(Side::of(d(0)), Side::Left);
    assert_eq!(Side::of(d(1)), Side::Right);
    assert_eq!(Side::of(d(2)), Side::Left);
    assert_eq!(Side::of(d(3)), Side::Right);
}

#[test]
fn defaults_match_engine() {
    let s = ControlState::default();
    for id in DeckId::ALL {
        let deck = s.deck(id);
        assert_eq!(deck.tempo_range, TempoRange::Ten);
        assert!(!deck.pfl);
        for kind in [
            ActionKind::Trim,
            ActionKind::EqHigh,
            ActionKind::EqMid,
            ActionKind::EqLow,
            ActionKind::Filter,
            ActionKind::Tempo,
        ] {
            assert_eq!(s.value((kind, Some(id))), Some(0.5), "{}", kind.id());
        }
        assert_eq!(s.value((ActionKind::Fader, Some(id))), Some(1.0));
    }
    assert_eq!(s.value((ActionKind::Crossfader, None)), Some(0.5));
    assert_eq!(s.value((ActionKind::MasterLevel, None)), Some(0.8));
    assert_eq!(s.value((ActionKind::HeadphoneLevel, None)), Some(0.8));
    assert_eq!(s.value((ActionKind::HeadphoneMix, None)), Some(0.0));
    assert!(!s.headphone_master);
    assert_eq!(s.side(Side::Left).active_deck, d(0));
    assert_eq!(s.side(Side::Right).active_deck, d(1));
    assert!(!s.side(Side::Left).shift);
    assert!(!s.side(Side::Right).shift);
    // Buttons and mismatched targets have no value.
    assert_eq!(s.value((ActionKind::PlayPause, Some(d(0)))), None);
    assert_eq!(s.value((ActionKind::Fader, None)), None);
    assert_eq!(s.value((ActionKind::Crossfader, Some(d(0)))), None);
}

#[test]
fn play_pause_toggles_engine_playing() {
    let mut rig = Rig::new();
    let play = deck_action(ActionKind::PlayPause, 0);
    assert_eq!(rig.apply(play, Input::Press), ApplyResult::Sent(1));
    assert!(rig.c.deck(d(0)).playing);
    assert_eq!(rig.apply(play, Input::Press), ApplyResult::Sent(1));
    assert!(!rig.c.deck(d(0)).playing);
}

#[test]
fn cue_press_release_previews() {
    let mut rig = Rig::new();
    let cue = deck_action(ActionKind::Cue, 0);
    // Paused at the cue point (frame 0): holding CUE previews.
    assert_eq!(rig.apply(cue, Input::Press), ApplyResult::Sent(1));
    assert!(rig.c.deck(d(0)).previewing);
    rig.render(1000);
    assert!(rig.c.deck(d(0)).position_frames > 0.0);
    assert_eq!(rig.apply(cue, Input::Release), ApplyResult::Sent(1));
    let s = rig.c.deck(d(0));
    assert!(!s.previewing);
    assert!(!s.playing);
    assert_eq!(s.position_frames, 0.0);
}

#[test]
fn tempo_absolute_sets_ratio() {
    let mut rig = Rig::new();
    let tempo = deck_action(ActionKind::Tempo, 0);
    assert_eq!(rig.apply(tempo, Input::Absolute(1.0)), ApplyResult::Sent(1));
    assert!((rig.c.deck(d(0)).tempo_ratio - 1.10).abs() < 1e-9);
    assert_eq!(rig.state.value((ActionKind::Tempo, Some(d(0)))), Some(1.0));
}

#[test]
fn tempo_range_cycles() {
    let mut rig = Rig::new();
    let next = deck_action(ActionKind::TempoRangeNext, 0);
    let tempo = deck_action(ActionKind::Tempo, 0);
    rig.apply(tempo, Input::Absolute(1.0));
    for (range, ratio) in [
        (TempoRange::Sixteen, 1.16),
        (TempoRange::Hundred, 2.0),
        (TempoRange::Six, 1.06),
        (TempoRange::Ten, 1.10),
    ] {
        assert_eq!(rig.apply(next, Input::Press), ApplyResult::Sent(1));
        assert_eq!(rig.state.deck(d(0)).tempo_range, range);
        assert!((rig.c.deck(d(0)).tempo_ratio - ratio).abs() < 1e-9);
    }
}

#[test]
fn return_to_start_seeks_to_zero() {
    let mut rig = Rig::new();
    rig.apply(deck_action(ActionKind::PlayPause, 0), Input::Press);
    rig.render(2000);
    assert!(rig.c.deck(d(0)).position_frames > 1000.0);
    let back = deck_action(ActionKind::ReturnToStart, 0);
    assert_eq!(rig.apply(back, Input::Press), ApplyResult::Sent(1));
    assert!(rig.c.deck(d(0)).position_frames < BLOCK as f64 + 1.0);
}

#[test]
fn pfl_toggle_flips_state() {
    let mut rig = Rig::new();
    rig.apply(deck_action(ActionKind::PlayPause, 0), Input::Press);
    // Headphone mix defaults to cue only, so nothing reaches the headphones without PFL.
    assert!(rig.render(SETTLE_FRAMES).abs() < 1e-4);
    let pfl = deck_action(ActionKind::PflToggle, 0);
    assert_eq!(rig.apply(pfl, Input::Press), ApplyResult::Sent(1));
    assert!(rig.state.deck(d(0)).pfl);
    assert!(rig.render(SETTLE_FRAMES) > 0.1);
    assert_eq!(rig.apply(pfl, Input::Press), ApplyResult::Sent(1));
    assert!(!rig.state.deck(d(0)).pfl);
    assert!(rig.render(SETTLE_FRAMES).abs() < 1e-4);
    // Other decks are untouched.
    assert!(!rig.state.deck(d(1)).pfl);
}

#[test]
fn headphone_master_toggle_flips_mix() {
    let mut rig = Rig::new();
    rig.apply(deck_action(ActionKind::PlayPause, 0), Input::Press);
    assert!(rig.render(SETTLE_FRAMES).abs() < 1e-4);
    let toggle = global_action(ActionKind::HeadphoneMasterToggle);
    assert_eq!(rig.apply(toggle, Input::Press), ApplyResult::Sent(1));
    assert!(rig.state.headphone_master);
    assert_eq!(rig.state.value((ActionKind::HeadphoneMix, None)), Some(1.0));
    // Master only: the playing deck is heard in the headphones.
    assert!(rig.render(SETTLE_FRAMES) > 0.1);
    assert_eq!(rig.apply(toggle, Input::Press), ApplyResult::Sent(1));
    assert!(!rig.state.headphone_master);
    assert_eq!(rig.state.value((ActionKind::HeadphoneMix, None)), Some(0.0));
    assert!(rig.render(SETTLE_FRAMES).abs() < 1e-4);
}

#[test]
fn absolute_actions_update_values() {
    let mut rig = Rig::new();
    for kind in [
        ActionKind::Trim,
        ActionKind::EqHigh,
        ActionKind::EqMid,
        ActionKind::EqLow,
        ActionKind::Filter,
        ActionKind::Fader,
    ] {
        let action = deck_action(kind, 3);
        assert_eq!(
            rig.apply(action, Input::Absolute(0.25)),
            ApplyResult::Sent(1)
        );
        assert_eq!(rig.state.value((kind, Some(d(3)))), Some(0.25));
        assert_eq!(
            rig.state.value((kind, Some(d(0)))),
            ControlState::default().value((kind, Some(d(0))))
        );
    }
    for kind in [
        ActionKind::Crossfader,
        ActionKind::MasterLevel,
        ActionKind::HeadphoneLevel,
    ] {
        let action = global_action(kind);
        assert_eq!(
            rig.apply(action, Input::Absolute(0.3)),
            ApplyResult::Sent(1)
        );
        assert_eq!(rig.state.value((kind, None)), Some(0.3));
    }
    // Values are clamped like the engine clamps them.
    let fader = deck_action(ActionKind::Fader, 1);
    rig.apply(fader, Input::Absolute(1.5));
    assert_eq!(rig.state.value((ActionKind::Fader, Some(d(1)))), Some(1.0));
    // Turning the headphone mix past the middle counts as headphone-master.
    let mix = global_action(ActionKind::HeadphoneMix);
    rig.apply(mix, Input::Absolute(0.75));
    assert!(rig.state.headphone_master);
    rig.apply(mix, Input::Absolute(0.25));
    assert!(!rig.state.headphone_master);
}

#[test]
fn side_actions_update_state_without_engine_commands() {
    let mut rig = Rig::new();
    let toggle = Action::new(ActionKind::DeckToggle, Target::Side(Side::Left)).unwrap();
    assert_eq!(rig.apply(toggle, Input::Press), ApplyResult::Sent(0));
    assert_eq!(rig.state.side(Side::Left).active_deck, d(2));
    assert_eq!(rig.state.side(Side::Right).active_deck, d(1));
    assert_eq!(rig.apply(toggle, Input::Release), ApplyResult::Ignored);
    assert_eq!(rig.apply(toggle, Input::Press), ApplyResult::Sent(0));
    assert_eq!(rig.state.side(Side::Left).active_deck, d(0));

    let shift = Action::new(ActionKind::Shift, Target::Side(Side::Right)).unwrap();
    assert_eq!(rig.apply(shift, Input::Press), ApplyResult::Sent(0));
    assert!(rig.state.side(Side::Right).shift);
    assert!(!rig.state.side(Side::Left).shift);
    assert_eq!(rig.apply(shift, Input::Release), ApplyResult::Sent(0));
    assert!(!rig.state.side(Side::Right).shift);
}

#[test]
fn unaccepted_input_is_ignored() {
    let mut rig = Rig::new();
    let before = rig.state.clone();
    let play = deck_action(ActionKind::PlayPause, 0);
    let cue = deck_action(ActionKind::Cue, 0);
    let fader = deck_action(ActionKind::Fader, 0);
    let pfl = deck_action(ActionKind::PflToggle, 0);
    let range = deck_action(ActionKind::TempoRangeNext, 0);
    let hp = global_action(ActionKind::HeadphoneMasterToggle);
    for (action, input) in [
        (play, Input::Release),
        (play, Input::Absolute(1.0)),
        (play, Input::Relative(1)),
        (cue, Input::Absolute(1.0)),
        (fader, Input::Press),
        (fader, Input::Release),
        (fader, Input::Relative(-3)),
        (fader, Input::Absolute(f32::NAN)),
        (fader, Input::Absolute(f32::INFINITY)),
        (pfl, Input::Release),
        (range, Input::Release),
        (hp, Input::Release),
        (hp, Input::Absolute(0.0)),
    ] {
        assert_eq!(
            rig.apply(action, input),
            ApplyResult::Ignored,
            "{} {input:?}",
            action.id()
        );
    }
    assert_eq!(rig.state, before);
    assert!(!rig.c.deck(d(0)).playing);
    assert!(!rig.c.deck(d(0)).previewing);
}

#[test]
fn full_queue_leaves_state_unchanged() {
    let mut rig = Rig::new();
    // Fill the queue without rendering.
    while rig
        .c
        .send(engine::Command::Deck(d(1), engine::DeckCommand::Pause))
        .is_ok()
    {}
    let before = rig.state.clone();
    let mut c = rig.c;
    for (action, input) in [
        (deck_action(ActionKind::PflToggle, 0), Input::Press),
        (deck_action(ActionKind::TempoRangeNext, 0), Input::Press),
        (deck_action(ActionKind::Fader, 0), Input::Absolute(0.2)),
        (
            global_action(ActionKind::HeadphoneMasterToggle),
            Input::Press,
        ),
    ] {
        assert_eq!(
            apply(&action, input, &mut rig.state, &mut c),
            ApplyResult::QueueFull
        );
    }
    assert_eq!(rig.state, before);
}

fn snap(loaded: bool, playing: bool, position: f64, cue: f64, previewing: bool) -> DeckSnapshot {
    DeckSnapshot {
        loaded,
        playing,
        position_frames: position,
        duration_frames: if loaded { 100_000 } else { 0 },
        track_sample_rate: if loaded { SR } else { 0 },
        tempo_ratio: 1.0,
        cue_frames: cue,
        previewing,
    }
}

#[test]
fn led_rules() {
    let mut state = ControlState::default();
    let deck = Some(Target::Deck(d(0)));
    let unloaded = snap(false, false, 0.0, 0.0, false);
    let playing = snap(true, true, 5000.0, 100.0, false);
    let paused_at_cue = snap(true, false, 100.5, 100.0, false);
    let paused_elsewhere = snap(true, false, 5000.0, 100.0, false);
    let previewing = snap(true, true, 5000.0, 100.0, true);

    // Playing state.
    assert_eq!(
        led(StateKey::DeckPlaying, deck, &state, Some(&playing)),
        Led::On
    );
    assert_eq!(
        led(StateKey::DeckPlaying, deck, &state, Some(&paused_elsewhere)),
        Led::Off
    );

    // Play LED: on while playing, blinking when loaded and paused, off when unloaded.
    assert_eq!(
        led(StateKey::DeckPlayLed, deck, &state, Some(&playing)),
        Led::On
    );
    assert_eq!(
        led(StateKey::DeckPlayLed, deck, &state, Some(&paused_elsewhere)),
        Led::Blink
    );
    assert_eq!(
        led(StateKey::DeckPlayLed, deck, &state, Some(&unloaded)),
        Led::Off
    );

    // Cue LED: on when previewing or paused at cue, blinking when paused elsewhere, off
    // while playing or unloaded.
    assert_eq!(
        led(StateKey::DeckCueLed, deck, &state, Some(&previewing)),
        Led::On
    );
    assert_eq!(
        led(StateKey::DeckCueLed, deck, &state, Some(&paused_at_cue)),
        Led::On
    );
    assert_eq!(
        led(StateKey::DeckCueLed, deck, &state, Some(&paused_elsewhere)),
        Led::Blink
    );
    assert_eq!(
        led(StateKey::DeckCueLed, deck, &state, Some(&playing)),
        Led::Off
    );
    assert_eq!(
        led(StateKey::DeckCueLed, deck, &state, Some(&unloaded)),
        Led::Off
    );
    // No snapshot: nothing is known, so deck LEDs are off.
    assert_eq!(led(StateKey::DeckPlayLed, deck, &state, None), Led::Off);

    // PFL: on when enabled.
    assert_eq!(led(StateKey::ChannelPfl, deck, &state, None), Led::Off);
    state.deck_mut(d(0)).pfl = true;
    assert_eq!(led(StateKey::ChannelPfl, deck, &state, None), Led::On);
    // A side target reads the deck the side shows.
    let left = Some(Target::Side(Side::Left));
    assert_eq!(led(StateKey::ChannelPfl, left, &state, None), Led::On);

    // Side alt deck: on when the side shows deck 3 or 4.
    let right = Some(Target::Side(Side::Right));
    assert_eq!(led(StateKey::SideAltDeck, left, &state, None), Led::Off);
    state.side_mut(Side::Left).active_deck = d(2);
    assert_eq!(led(StateKey::SideAltDeck, left, &state, None), Led::On);
    assert_eq!(led(StateKey::SideAltDeck, right, &state, None), Led::Off);
    // Deck 1 is no longer shown on the left, so its PFL is not what the left side reads.
    assert_eq!(led(StateKey::ChannelPfl, left, &state, None), Led::Off);

    // Shift: on while held.
    assert_eq!(led(StateKey::Shift, right, &state, None), Led::Off);
    state.side_mut(Side::Right).shift = true;
    assert_eq!(led(StateKey::Shift, right, &state, None), Led::On);
    assert_eq!(led(StateKey::Shift, left, &state, None), Led::Off);

    // Headphone master.
    let global = Some(Target::Global);
    assert_eq!(
        led(StateKey::HeadphoneMaster, global, &state, None),
        Led::Off
    );
    state.headphone_master = true;
    assert_eq!(
        led(StateKey::HeadphoneMaster, global, &state, None),
        Led::On
    );

    // A deck key without a deck target, or a side key without a side, is off.
    assert_eq!(led(StateKey::ChannelPfl, None, &state, None), Led::Off);
    assert_eq!(led(StateKey::Shift, global, &state, None), Led::Off);
}

/// An action of `kind` on deck 3 or the right side, whichever its scope needs.
fn any_action(kind: ActionKind) -> Action {
    let target = match kind.scope() {
        actions::Scope::Deck => Target::Deck(d(2)),
        actions::Scope::Side => Target::Side(Side::Right),
        actions::Scope::Global => Target::Global,
    };
    Action::new(kind, target).unwrap()
}

#[test]
fn accepts_matches_what_apply_does() {
    use actions::InputKind;
    for kind in ActionKind::ALL {
        for input in [Input::Press, Input::Absolute(0.25), Input::Relative(1)] {
            let mut rig = Rig::new();
            let result = rig.apply(any_action(kind), input);
            assert_eq!(
                kind.accepts(input.kind()),
                result != ApplyResult::Ignored,
                "{kind:?} {input:?} -> {result:?}"
            );
        }
        assert!(!kind.accepts(InputKind::Relative));
        assert_eq!(kind.is_absolute(), kind.accepts(InputKind::Absolute));
    }
}

#[test]
fn state_key_scopes() {
    use actions::Scope;
    assert_eq!(StateKey::DeckPlayLed.scope(), Scope::Deck);
    assert_eq!(StateKey::ChannelPfl.scope(), Scope::Deck);
    assert_eq!(StateKey::SideAltDeck.scope(), Scope::Side);
    assert_eq!(StateKey::Shift.scope(), Scope::Side);
    assert_eq!(StateKey::HeadphoneMaster.scope(), Scope::Global);
}

#[test]
fn docs_list_every_action_id() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../docs/controllers/mapping-reference.md"
    );
    let docs = std::fs::read_to_string(path).expect("the mapping reference exists");
    for kind in ActionKind::ALL {
        assert!(
            docs.contains(&format!("`{}`", kind.id())),
            "docs/controllers/mapping-reference.md does not list action `{}`",
            kind.id()
        );
    }
    for key in StateKey::ALL {
        assert!(
            docs.contains(&format!("`{}`", key.id())),
            "docs/controllers/mapping-reference.md does not list state `{}`",
            key.id()
        );
    }
}
