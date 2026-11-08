use std::collections::HashSet;
use std::sync::Arc;

use actions::{
    Action, ActionKind, ApplyResult, ControlState, Input, InputKind, Led, Side, StateKey, Target,
    apply, led,
};
use engine::{
    DeckId, DeckSnapshot, EngineConfig, EngineController, EngineRenderer, HotcueSlot, TempoRange,
    TrackBuffer, engine,
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
        Self::with_memory(engine::TrackMemory::default())
    }

    /// The same with `memory` loaded along with the track.
    fn with_memory(memory: engine::TrackMemory) -> Self {
        let (mut c, r) = engine(EngineConfig {
            sample_rate: SR,
            max_block_frames: BLOCK,
        });
        let track = Arc::new(TrackBuffer::new(SR, vec![0.5; 200_000 * 2]).unwrap());
        c.send(engine::Command::Deck(
            d(0),
            engine::DeckCommand::Load { track, memory },
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
        let result = apply(&action, input, 0, &mut self.state, &mut self.c);
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
        "deck.grid_shift_back",
        "deck.grid_shift_forward",
        "deck.grid_set_downbeat",
        "deck.grid_halve",
        "deck.grid_double",
        "deck.grid_tap",
        "deck.grid_lock_toggle",
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
        "browser.scroll",
        "browser.toggle_focus",
        "deck.load_selected",
        "deck.hotcue_1",
        "deck.hotcue_2",
        "deck.hotcue_3",
        "deck.hotcue_4",
        "deck.hotcue_5",
        "deck.hotcue_6",
        "deck.hotcue_7",
        "deck.hotcue_8",
        "deck.hotcue_delete_1",
        "deck.hotcue_delete_2",
        "deck.hotcue_delete_3",
        "deck.hotcue_delete_4",
        "deck.hotcue_delete_5",
        "deck.hotcue_delete_6",
        "deck.hotcue_delete_7",
        "deck.hotcue_delete_8",
        "deck.loop_in",
        "deck.loop_out",
        "deck.auto_loop",
        "deck.loop_halve",
        "deck.loop_double",
        "deck.reloop_exit",
        "deck.sync",
        "deck.vinyl_toggle",
        "deck.jog_touch",
        "deck.jog_release",
        "deck.jog_top",
        "deck.jog_ring",
        "deck.jog_search",
        "deck.jog_scratch",
        "deck.backspin_short",
        "deck.backspin_long",
        "deck.scratch_anchor_set",
        "deck.scratch_anchor_return",
        "deck.loop_fade_1",
        "deck.loop_fade_4",
        "deck.loop_exit",
        "deck.beat_jump_1",
        "deck.beat_jump_2",
        "deck.beat_jump_3",
        "deck.beat_jump_4",
        "deck.beat_jump_5",
        "deck.beat_jump_6",
        "deck.beat_jump_7",
        "deck.beat_jump_8",
        "deck.beat_jump_shift_1",
        "deck.beat_jump_shift_2",
        "deck.beat_jump_shift_3",
        "deck.beat_jump_shift_4",
        "deck.beat_jump_shift_5",
        "deck.beat_jump_shift_6",
        "deck.beat_jump_shift_7",
        "deck.beat_jump_shift_8",
        "deck.loop_roll_1",
        "deck.loop_roll_2",
        "deck.loop_roll_3",
        "deck.loop_roll_4",
        "deck.loop_roll_5",
        "deck.loop_roll_6",
        "deck.loop_roll_7",
        "deck.loop_roll_8",
        "deck.slice_1",
        "deck.slice_2",
        "deck.slice_3",
        "deck.slice_4",
        "deck.slice_5",
        "deck.slice_6",
        "deck.slice_7",
        "deck.slice_8",
        "deck.slip_toggle",
        "deck.keylock_toggle",
        "deck.fx_toggle_1",
        "deck.fx_toggle_2",
        "deck.fx_toggle_3",
        "deck.fx_level",
        "deck.stem_toggle_vocals",
        "deck.stem_toggle_instruments",
        "deck.stem_toggle_bass",
        "deck.stem_toggle_rhythm",
        "deck.stem_toggle_kicks",
        "deck.stem_toggle_melody",
        "deck.stem_toggle_hats",
        "sampler.play_1",
        "sampler.play_2",
        "sampler.play_3",
        "sampler.play_4",
        "sampler.play_5",
        "sampler.play_6",
        "sampler.play_7",
        "sampler.play_8",
        "sampler.stop_1",
        "sampler.stop_2",
        "sampler.stop_3",
        "sampler.stop_4",
        "sampler.stop_5",
        "sampler.stop_6",
        "sampler.stop_7",
        "sampler.stop_8",
        "pads.mode_hardware",
        "pads.mode_beat_jump",
        "pads.mode_loop_roll",
        "pads.mode_slicer",
        "pads.mode_pad_scratch",
    ];
    let ids: HashSet<_> = ActionKind::ALL.iter().map(|k| k.id()).collect();
    assert_eq!(ids, expected.into_iter().collect::<HashSet<_>>());
    assert_eq!(ids.len(), ActionKind::ALL.len(), "ids are unique");

    for kind in ActionKind::ALL {
        assert_eq!(Action::from_id(kind.id()), Some(kind));
        assert_eq!(ActionKind::from_id(kind.id()), Some(kind));
        let target = match kind.id().split('.').next().unwrap() {
            "deck" | "channel" => Target::Deck(d(2)),
            "side" | "modifier" | "pads" => Target::Side(Side::Right),
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
    assert_eq!(ActionKind::ALL.len(), 132);
    let hotcue = |n| ActionKind::Hotcue(HotcueSlot::new(n).unwrap());
    let hotcue_delete = |n| ActionKind::HotcueDelete(HotcueSlot::new(n).unwrap());
    let mut expected_release = vec![
        ActionKind::Cue,
        ActionKind::Shift,
        ActionKind::ReturnToStart,
    ];
    expected_release.extend((1..=8).map(hotcue));
    expected_release.extend((1..=8).map(hotcue_delete));
    expected_release.extend([ActionKind::JogTouch, ActionKind::JogRelease]);
    expected_release.extend((1..=8).map(|n| ActionKind::LoopRoll(slot(n))));
    expected_release.extend((1..=8).map(|n| ActionKind::Slice(slot(n))));
    assert_eq!(
        with_release.iter().collect::<HashSet<_>>(),
        expected_release.iter().collect::<HashSet<_>>()
    );
    assert_eq!(with_release.len(), 37);
    for kind in [
        ActionKind::JogTop,
        ActionKind::JogRing,
        ActionKind::JogSearch,
        ActionKind::JogScratch,
    ] {
        assert!(kind.accepts(InputKind::Relative));
        assert!(!kind.accepts(InputKind::Button));
    }
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
            apply(&action, input, 0, &mut rig.state, &mut c),
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
        ..DeckSnapshot::default()
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
            // `deck.sync` needs a playing deck to follow.
            rig.load_grid(2, 128.0, false);
            rig.load_grid(1, 125.0, true);
            let result = rig.apply(any_action(kind), input);
            assert_eq!(
                kind.accepts(input.kind()),
                result != ApplyResult::Ignored,
                "{kind:?} {input:?} -> {result:?}"
            );
        }
        // `browser.scroll` and the jog wheel actions are the ones that take relative steps.
        assert_eq!(
            kind.accepts(InputKind::Relative),
            matches!(
                kind,
                ActionKind::BrowserScroll
                    | ActionKind::JogTop
                    | ActionKind::JogRing
                    | ActionKind::JogSearch
                    | ActionKind::JogScratch
            ),
            "{kind:?}"
        );
        assert_eq!(kind.is_absolute(), kind.accepts(InputKind::Absolute));
    }
}

#[test]
fn grid_actions_return_requests() {
    use actions::{AppRequest, GridEditRequest as E};
    let mut rig = Rig::new();
    let before = rig.state.clone();
    let deck2 = d(1);
    let request = |edit| ApplyResult::Request(AppRequest::Grid { deck: deck2, edit });
    let cases = [
        (ActionKind::GridShiftBack, E::Shift { millis: -5 }),
        (ActionKind::GridShiftForward, E::Shift { millis: 5 }),
        (ActionKind::GridSetDownbeat, E::SetDownbeat),
        (ActionKind::GridHalve, E::Halve),
        (ActionKind::GridDouble, E::Double),
        (ActionKind::GridTap, E::Tap { at_micros: 0 }),
        (ActionKind::GridLockToggle, E::ToggleLock),
    ];
    for (kind, edit) in cases {
        let action = deck_action(kind, 1);
        assert_eq!(rig.apply(action, Input::Press), request(edit), "{kind:?}");
        assert_eq!(rig.apply(action, Input::Release), ApplyResult::Ignored);
    }
    // Requests change no control state (the app performs them).
    assert_eq!(rig.state, before);

    // A tap carries the time of its input.
    let tap = deck_action(ActionKind::GridTap, 1);
    assert_eq!(
        apply(&tap, Input::Press, 1_234_567, &mut rig.state, &mut rig.c),
        request(E::Tap {
            at_micros: 1_234_567
        })
    );

    let back = deck_action(ActionKind::GridShiftBack, 1);
    let forward = deck_action(ActionKind::GridShiftForward, 1);
    // SHIFT on the other side does not count.
    rig.state.side_mut(Side::Left).shift = true;
    assert_eq!(
        rig.apply(back, Input::Press),
        request(E::Shift { millis: -5 })
    );
    // SHIFT on deck 2's side: 1 ms steps.
    rig.state.side_mut(Side::Right).shift = true;
    assert_eq!(
        rig.apply(back, Input::Press),
        request(E::Shift { millis: -1 })
    );
    assert_eq!(
        rig.apply(forward, Input::Press),
        request(E::Shift { millis: 1 })
    );
    // Deck 4 is on the right side too.
    assert_eq!(
        rig.apply(deck_action(ActionKind::GridShiftForward, 3), Input::Press),
        ApplyResult::Request(AppRequest::Grid {
            deck: d(3),
            edit: E::Shift { millis: 1 }
        })
    );
}

#[test]
fn browser_and_load_actions_return_requests() {
    use actions::{AppRequest, BrowserRequest, Scope};
    assert_eq!(ActionKind::BrowserScroll.scope(), Scope::Global);
    assert_eq!(ActionKind::BrowserToggleFocus.scope(), Scope::Global);
    assert_eq!(ActionKind::LoadSelected.scope(), Scope::Deck);

    let mut rig = Rig::new();
    let before = rig.state.clone();
    let scroll = global_action(ActionKind::BrowserScroll);
    for steps in [1, -1, 5, -64] {
        assert_eq!(
            rig.apply(scroll, Input::Relative(steps)),
            ApplyResult::Request(AppRequest::Browser(BrowserRequest::Scroll(steps)))
        );
    }
    assert_eq!(rig.apply(scroll, Input::Press), ApplyResult::Ignored);

    let focus = global_action(ActionKind::BrowserToggleFocus);
    assert_eq!(
        rig.apply(focus, Input::Press),
        ApplyResult::Request(AppRequest::Browser(BrowserRequest::ToggleFocus))
    );
    assert_eq!(rig.apply(focus, Input::Release), ApplyResult::Ignored);

    for deck in 0..4 {
        let load = deck_action(ActionKind::LoadSelected, deck);
        assert_eq!(
            rig.apply(load, Input::Press),
            ApplyResult::Request(AppRequest::LoadSelected { deck: d(deck) })
        );
        assert_eq!(rig.apply(load, Input::Release), ApplyResult::Ignored);
    }
    // Requests change no control state (the app performs them).
    assert_eq!(rig.state, before);
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

#[test]
fn hotcue_ids_outside_1_to_8_are_unknown() {
    assert_eq!(ActionKind::from_id("deck.hotcue_0"), None);
    assert_eq!(ActionKind::from_id("deck.hotcue_9"), None);
    assert_eq!(ActionKind::from_id("deck.hotcue_delete_0"), None);
    assert_eq!(ActionKind::from_id("deck.hotcue_delete_9"), None);
    let three = ActionKind::from_id("deck.hotcue_3").unwrap();
    assert_eq!(three, ActionKind::Hotcue(HotcueSlot::new(3).unwrap()));
}

fn slot(n: u8) -> HotcueSlot {
    HotcueSlot::new(n).unwrap()
}

#[test]
fn hotcue_actions_send_slot_commands() {
    let mut rig = Rig::new();
    for n in [1, 8] {
        let pad = Action::new(ActionKind::Hotcue(slot(n)), Target::Deck(d(0))).unwrap();
        rig.render(1000);
        let at = rig.c.deck(d(0)).position_frames;
        // Press on an empty pad sets it at the playhead; release is sent too.
        assert_eq!(rig.apply(pad, Input::Press), ApplyResult::Sent(1));
        assert_eq!(rig.c.deck(d(0)).hotcues[slot(n).index()], Some(at));
        assert_eq!(rig.apply(pad, Input::Release), ApplyResult::Sent(1));
    }
    let snap = rig.c.deck(d(0));
    assert!(snap.hotcues[0].is_some() && snap.hotcues[7].is_some());
    assert!(snap.hotcues[1..7].iter().all(Option::is_none));

    let delete = Action::new(ActionKind::HotcueDelete(slot(8)), Target::Deck(d(0))).unwrap();
    assert_eq!(rig.apply(delete, Input::Press), ApplyResult::Sent(1));
    let snap = rig.c.deck(d(0));
    assert!(snap.hotcues[0].is_some());
    assert_eq!(snap.hotcues[7], None);
    // Its release ends a preview of the slot (see `shifted_releases_end_the_unshifted_hold`).
    assert_eq!(rig.apply(delete, Input::Release), ApplyResult::Sent(1));
}

#[test]
fn shifted_releases_end_the_unshifted_hold() {
    // SHIFT pressed during a held pad: the note-off arrives on the shifted note, which is
    // bound to the delete action. Its release ends the pad's preview.
    let mut rig = Rig::new();
    let pad = Action::new(ActionKind::Hotcue(slot(1)), Target::Deck(d(0))).unwrap();
    let delete = Action::new(ActionKind::HotcueDelete(slot(1)), Target::Deck(d(0))).unwrap();
    rig.c
        .send(engine::Command::Deck(
            d(0),
            engine::DeckCommand::SetHotcue(slot(1), Some(5_000.0)),
        ))
        .unwrap();
    rig.render(1);
    assert_eq!(rig.apply(pad, Input::Press), ApplyResult::Sent(1));
    assert!(rig.c.deck(d(0)).previewing);
    assert_eq!(rig.apply(delete, Input::Release), ApplyResult::Sent(1));
    let snap = rig.c.deck(d(0));
    assert!(!snap.previewing && !snap.playing, "pad preview ended");
    assert_eq!(snap.position_frames, 5_000.0);
    assert_eq!(
        snap.hotcues[0],
        Some(5_000.0),
        "the release deletes nothing"
    );

    // The same for CUE, whose shifted note is bound to return-to-start.
    let cue = deck_action(ActionKind::Cue, 0);
    let back = deck_action(ActionKind::ReturnToStart, 0);
    // Paused away from the cue point, a first press moves the cue point here.
    rig.apply(cue, Input::Press);
    rig.apply(cue, Input::Release);
    assert_eq!(rig.apply(cue, Input::Press), ApplyResult::Sent(1));
    assert!(rig.c.deck(d(0)).previewing);
    assert_eq!(rig.apply(back, Input::Release), ApplyResult::Sent(1));
    let snap = rig.c.deck(d(0));
    assert!(!snap.previewing && !snap.playing, "CUE preview ended");
    assert_eq!(
        snap.position_frames, 5_000.0,
        "back at the cue point, not the start"
    );

    // With nothing held the releases change nothing.
    let before = rig.c.deck(d(0));
    assert_eq!(rig.apply(delete, Input::Release), ApplyResult::Sent(1));
    assert_eq!(rig.apply(back, Input::Release), ApplyResult::Sent(1));
    assert_eq!(rig.c.deck(d(0)), before);
}

#[test]
fn takes_release_matches_what_apply_does() {
    for kind in ActionKind::ALL {
        let mut rig = Rig::new();
        let result = rig.apply(any_action(kind), Input::Release);
        assert_eq!(
            kind.takes_release(),
            result != ApplyResult::Ignored,
            "{kind:?} -> {result:?}"
        );
    }
}

#[test]
fn loop_actions_send_commands() {
    let mut rig = Rig::new();
    let act = |kind| deck_action(kind, 0);
    assert_eq!(
        rig.apply(act(ActionKind::LoopIn), Input::Press),
        ApplyResult::Sent(1)
    );
    let start = rig.c.deck(d(0)).position_frames;
    assert_eq!(rig.c.deck(d(0)).loop_in, Some(start));
    rig.apply(act(ActionKind::PlayPause), Input::Press);
    rig.render(4000);
    assert_eq!(
        rig.apply(act(ActionKind::LoopOut), Input::Press),
        ApplyResult::Sent(1)
    );
    let snap = rig.c.deck(d(0));
    assert!(snap.loop_active);
    let length = snap.loop_out.unwrap() - start;

    assert_eq!(
        rig.apply(act(ActionKind::LoopHalve), Input::Press),
        ApplyResult::Sent(1)
    );
    let halved = rig.c.deck(d(0)).loop_out.unwrap() - start;
    assert!((halved - length / 2.0).abs() < 1.0, "{halved} vs {length}");
    assert_eq!(
        rig.apply(act(ActionKind::LoopDouble), Input::Press),
        ApplyResult::Sent(1)
    );
    let doubled = rig.c.deck(d(0)).loop_out.unwrap() - start;
    assert!((doubled - length).abs() < 1.0);

    assert_eq!(
        rig.apply(act(ActionKind::ReloopExit), Input::Press),
        ApplyResult::Sent(1)
    );
    assert!(!rig.c.deck(d(0)).loop_active);
    rig.apply(act(ActionKind::ReloopExit), Input::Press);
    assert!(rig.c.deck(d(0)).loop_active);

    // No grid: an auto loop has nothing to size from, but the command is still sent.
    assert_eq!(
        rig.apply(act(ActionKind::AutoLoop), Input::Press),
        ApplyResult::Sent(1)
    );
    assert_eq!(
        rig.apply(act(ActionKind::LoopIn), Input::Release),
        ApplyResult::Ignored
    );
}

#[test]
fn vinyl_toggle_reads_the_snapshot() {
    let mut rig = Rig::new();
    let vinyl = deck_action(ActionKind::VinylToggle, 0);
    let was = rig.c.deck(d(0)).vinyl;
    assert_eq!(rig.apply(vinyl, Input::Press), ApplyResult::Sent(1));
    assert_eq!(rig.c.deck(d(0)).vinyl, !was);
    assert_eq!(rig.apply(vinyl, Input::Press), ApplyResult::Sent(1));
    assert_eq!(rig.c.deck(d(0)).vinyl, was);
}

#[test]
fn jog_touch_follows_press_and_release() {
    let mut rig = Rig::new();
    let touch = deck_action(ActionKind::JogTouch, 0);
    assert_eq!(rig.apply(touch, Input::Press), ApplyResult::Sent(1));
    assert!(rig.c.deck(d(0)).jog_touched);
    assert_eq!(rig.apply(touch, Input::Release), ApplyResult::Sent(1));
    assert!(!rig.c.deck(d(0)).jog_touched);
}

#[test]
fn jog_release_only_releases() {
    let mut rig = Rig::new();
    let touch = deck_action(ActionKind::JogTouch, 0);
    let release = deck_action(ActionKind::JogRelease, 0);
    // Its press never touches: a touch begun with SHIFT starts no scratch hold.
    assert_eq!(rig.apply(release, Input::Press), ApplyResult::Sent(1));
    assert!(!rig.c.deck(d(0)).jog_touched);
    // Both its press and its release end a touch begun without SHIFT.
    for input in [Input::Press, Input::Release] {
        rig.apply(touch, Input::Press);
        assert!(rig.c.deck(d(0)).jog_touched);
        assert_eq!(rig.apply(release, input), ApplyResult::Sent(1));
        assert!(!rig.c.deck(d(0)).jog_touched, "{input:?}");
    }
}

#[test]
fn jog_steps_become_turns() {
    // A paused deck moves 1.8 s of track per turn, search 10 s.
    let one_turn = 1.8 * f64::from(SR);
    let moved = |kind, steps, per_turn: Option<u16>| {
        let mut rig = Rig::new();
        if let Some(per_turn) = per_turn {
            rig.state.jog_steps_per_turn = per_turn;
        }
        assert_eq!(
            rig.apply(deck_action(kind, 0), Input::Relative(steps)),
            ApplyResult::Sent(1)
        );
        rig.c.deck(d(0)).position_frames
    };
    assert_eq!(ControlState::default().jog_steps_per_turn, 720);
    for kind in [ActionKind::JogTop, ActionKind::JogRing] {
        assert!((moved(kind, 720, None) - one_turn).abs() < 1.0);
        assert!((moved(kind, 360, Some(360)) - one_turn).abs() < 1.0);
        assert!((moved(kind, 360, None) - one_turn / 2.0).abs() < 1.0);
    }
    // 72 steps is a tenth of a turn: 1 s of search.
    assert!((moved(ActionKind::JogSearch, 72, None) - f64::from(SR)).abs() < 1.0);
    assert!((moved(ActionKind::JogSearch, 36, Some(360)) - f64::from(SR)).abs() < 1.0);
}

impl Rig {
    /// Loads a constant track with a grid at `bpm` on `deck` (replacing any track there) and
    /// starts it when `playing`.
    fn load_grid(&mut self, deck: usize, bpm: f64, playing: bool) {
        let track = Arc::new(TrackBuffer::new(SR, vec![0.5; 200_000 * 2]).unwrap());
        let memory = engine::TrackMemory {
            grid: engine::Grid::from_bpm(bpm, 0.0, SR),
            ..Default::default()
        };
        self.c
            .send(engine::Command::Deck(
                d(deck),
                engine::DeckCommand::Load { track, memory },
            ))
            .unwrap();
        if playing {
            self.toggle_play(deck);
        }
        self.render(1);
    }

    fn toggle_play(&mut self, deck: usize) {
        self.c
            .send(engine::Command::Deck(
                d(deck),
                engine::DeckCommand::TogglePlay,
            ))
            .unwrap();
        self.render(1);
    }

    fn sync(&mut self, deck: usize) -> ApplyResult {
        self.apply(deck_action(ActionKind::Sync, deck), Input::Press)
    }

    fn ratio(&self, deck: usize) -> f64 {
        self.c.deck(d(deck)).tempo_ratio
    }
}

#[test]
fn sync_matches_the_other_sides_playing_deck() {
    let mut rig = Rig::new();
    rig.load_grid(0, 128.0, false);
    rig.load_grid(1, 125.0, true);
    assert_eq!(rig.state.deck(d(0)).tempo_range, TempoRange::Ten);
    assert_eq!(rig.sync(0), ApplyResult::Sent(2));
    assert!((rig.ratio(0) - 125.0 / 128.0).abs() < 1e-6);
    let slider = 0.5 + (125.0 / 128.0 - 1.0) / 0.2;
    assert!((f64::from(rig.state.deck(d(0)).tempo) - slider).abs() < 1e-6);
    assert_eq!(rig.state.deck(d(0)).tempo_range, TempoRange::Ten);
}

#[test]
fn sync_prefers_the_other_sides_active_deck() {
    let mut rig = Rig::new();
    rig.load_grid(0, 128.0, false);
    rig.load_grid(1, 125.0, true);
    rig.load_grid(3, 120.0, true);
    // Deck 2 (index 1) is the lowest, but the right side's active deck wins.
    rig.state.side_mut(Side::Right).active_deck = d(3);
    assert_eq!(rig.sync(0), ApplyResult::Sent(2));
    assert!((rig.ratio(0) - 120.0 / 128.0).abs() < 1e-6);
}

#[test]
fn sync_falls_back_to_the_lowest_deck() {
    let mut rig = Rig::new();
    rig.load_grid(0, 128.0, false);
    rig.load_grid(2, 126.0, true);
    rig.load_grid(3, 120.0, true);
    // The right side's active deck (index 1) is empty: the lowest playing deck follows.
    assert_eq!(rig.sync(0), ApplyResult::Sent(2));
    assert!((rig.ratio(0) - 126.0 / 128.0).abs() < 1e-6);
}

#[test]
fn sync_picks_half_time() {
    let mut rig = Rig::new();
    rig.load_grid(0, 87.0, false);
    rig.load_grid(1, 174.0, true);
    assert_eq!(rig.sync(0), ApplyResult::Sent(2));
    assert!((rig.ratio(0) - 1.0).abs() < 1e-6);
    assert!((rig.state.deck(d(0)).tempo - 0.5).abs() < 1e-6);
}

#[test]
fn sync_widens_the_range_when_needed() {
    let mut rig = Rig::new();
    rig.load_grid(0, 100.0, false);
    rig.load_grid(1, 114.0, true);
    assert_eq!(rig.sync(0), ApplyResult::Sent(3));
    assert_eq!(rig.state.deck(d(0)).tempo_range, TempoRange::Sixteen);
    assert!((rig.ratio(0) - 1.14).abs() < 1e-6);
    assert!((f64::from(rig.state.deck(d(0)).tempo) - (0.5 + 0.14 / 0.32)).abs() < 1e-6);
}

#[test]
fn sync_without_a_master_or_grid_is_ignored() {
    let mut rig = Rig::new();
    rig.load_grid(0, 128.0, false);
    // No other deck at all, then a stopped one.
    assert_eq!(rig.sync(0), ApplyResult::Ignored);
    rig.load_grid(1, 125.0, false);
    assert_eq!(rig.sync(0), ApplyResult::Ignored);
    // A playing master, but no grid on deck 3 itself.
    rig.toggle_play(1);
    rig.load_grid(2, 120.0, false);
    assert_eq!(rig.sync(3), ApplyResult::Ignored);
    // Out of reach: more than 100 %.
    rig.load_grid(2, 30.0, false);
    assert_eq!(rig.sync(2), ApplyResult::Ignored);
    assert_eq!(rig.state.deck(d(2)).tempo, 0.5);
}

#[test]
fn sync_updates_control_state_tempo() {
    let mut rig = Rig::new();
    rig.load_grid(0, 128.0, false);
    rig.load_grid(1, 122.0, true);
    assert_eq!(rig.state.deck(d(0)).tempo, 0.5);
    rig.sync(0);
    let tempo = rig.state.deck(d(0)).tempo;
    assert!((f64::from(tempo) - (0.5 + (122.0 / 128.0 - 1.0) / 0.2)).abs() < 1e-6);
    // Moving the slider there by hand ends in the same state and tempo.
    let mut other = Rig::new();
    other.load_grid(0, 128.0, false);
    other.apply(deck_action(ActionKind::Tempo, 0), Input::Absolute(tempo));
    assert_eq!(other.ratio(0), rig.ratio(0));
}

/// 120 BPM, first beat at frame 0.
fn grid_memory() -> engine::TrackMemory {
    engine::TrackMemory {
        grid: engine::Grid::from_bpm(120.0, 0.0, SR),
        ..engine::TrackMemory::default()
    }
}

#[test]
fn pad_mode_actions_set_the_side_mode() {
    use actions::PadMode;
    let mut rig = Rig::new();
    assert_eq!(rig.state.side(Side::Left).pad_mode, PadMode::Hardware);
    for mode in PadMode::ALL {
        let kind = ActionKind::PadMode(mode);
        assert_eq!(kind.id(), format!("pads.mode_{}", mode.id()));
        let action = Action::new(kind, Target::Side(Side::Left)).unwrap();
        assert_eq!(rig.apply(action, Input::Press), ApplyResult::Sent(0));
        assert_eq!(rig.state.side(Side::Left).pad_mode, mode);
        assert_eq!(rig.state.side(Side::Right).pad_mode, PadMode::Hardware);
        assert_eq!(rig.apply(action, Input::Release), ApplyResult::Ignored);
        assert_eq!(PadMode::from_id(mode.id()), Some(mode));
    }
    assert_eq!(PadMode::from_id("nope"), None);
}

#[test]
fn defaults_hold_the_pad_sizes() {
    let s = ControlState::default();
    assert_eq!(
        s.beat_jump_pads,
        [-4.0, 4.0, -8.0, 8.0, -16.0, 16.0, -32.0, 32.0]
    );
    assert_eq!(
        s.beat_jump_shift_pads,
        [-1.0, 1.0, -2.0, 2.0, -4.0, 4.0, -64.0, 64.0]
    );
    assert_eq!(
        s.loop_roll_pads,
        [0.0625, 0.125, 0.25, 0.5, 1.0, 2.0, 4.0, 8.0]
    );
}

#[test]
fn beat_jump_uses_the_configured_sizes() {
    let mut rig = Rig::with_memory(grid_memory());
    let beat = 22_050.0;
    let seek = Action::new(ActionKind::ReturnToStart, Target::Deck(d(0))).unwrap();
    rig.apply(seek, Input::Press);
    let jump = |n| deck_action(ActionKind::BeatJump(slot(n)), 0);
    let shift_jump = |n| deck_action(ActionKind::BeatJumpShift(slot(n)), 0);

    // Pad 2 jumps 4 beats forward by default, the shifted pad 2 one beat.
    assert_eq!(rig.apply(jump(2), Input::Press), ApplyResult::Sent(1));
    assert_eq!(rig.c.deck(d(0)).position_frames, 4.0 * beat);
    assert_eq!(rig.apply(shift_jump(2), Input::Press), ApplyResult::Sent(1));
    assert_eq!(rig.c.deck(d(0)).position_frames, 5.0 * beat);
    // Releases are ignored.
    assert_eq!(rig.apply(jump(2), Input::Release), ApplyResult::Ignored);

    rig.state.beat_jump_pads[0] = -2.0;
    rig.state.beat_jump_shift_pads[0] = 0.5;
    rig.apply(jump(1), Input::Press);
    assert_eq!(rig.c.deck(d(0)).position_frames, 3.0 * beat);
    rig.apply(shift_jump(1), Input::Press);
    assert_eq!(rig.c.deck(d(0)).position_frames, 3.5 * beat);
}

#[test]
fn roll_and_slice_send_press_and_release() {
    let mut rig = Rig::with_memory(grid_memory());
    rig.apply(deck_action(ActionKind::PlayPause, 0), Input::Press);
    rig.render(4096);

    let roll = deck_action(ActionKind::LoopRoll(slot(5)), 0);
    assert_eq!(rig.apply(roll, Input::Press), ApplyResult::Sent(1));
    assert!(rig.c.deck(d(0)).roll_active);
    // A release of another size does not end the roll.
    let other = deck_action(ActionKind::LoopRoll(slot(6)), 0);
    rig.apply(other, Input::Release);
    assert!(rig.c.deck(d(0)).roll_active);
    assert_eq!(rig.apply(roll, Input::Release), ApplyResult::Sent(1));
    assert!(!rig.c.deck(d(0)).roll_active);

    let slice = deck_action(ActionKind::Slice(slot(3)), 0);
    assert_eq!(rig.apply(slice, Input::Press), ApplyResult::Sent(1));
    assert!(rig.c.deck(d(0)).roll_active);
    assert_eq!(rig.apply(slice, Input::Release), ApplyResult::Sent(1));
    assert!(!rig.c.deck(d(0)).roll_active);
}

#[test]
fn roll_and_slice_are_their_own_hold_partner() {
    for n in 1..=8 {
        for kind in [ActionKind::LoopRoll(slot(n)), ActionKind::Slice(slot(n))] {
            assert_eq!(kind.hold_partner(), Some(kind));
        }
        assert_eq!(ActionKind::BeatJump(slot(n)).hold_partner(), None);
    }
}

#[test]
fn slip_toggle_reads_the_snapshot() {
    let mut rig = Rig::new();
    let slip = deck_action(ActionKind::SlipToggle, 0);
    assert!(!rig.c.deck(d(0)).slip);
    assert_eq!(rig.apply(slip, Input::Press), ApplyResult::Sent(1));
    assert!(rig.c.deck(d(0)).slip);
    assert_eq!(rig.apply(slip, Input::Press), ApplyResult::Sent(1));
    assert!(!rig.c.deck(d(0)).slip);
    assert_eq!(rig.apply(slip, Input::Release), ApplyResult::Ignored);
}

#[test]
fn sampler_actions_send_slot_commands() {
    let mut rig = Rig::new();
    let track = Arc::new(TrackBuffer::new(SR, vec![0.5; 100_000 * 2]).unwrap());
    rig.c
        .send(engine::Command::Sampler(engine::SamplerCommand::Set(
            2,
            Some(track),
        )))
        .unwrap();
    rig.render(1);
    let play = global_action(ActionKind::SamplerPlay(slot(3)));
    let stop = global_action(ActionKind::SamplerStop(slot(3)));
    assert_eq!(rig.apply(play, Input::Press), ApplyResult::Sent(1));
    assert!(rig.c.sampler().playing[2]);
    assert!(!rig.c.sampler().playing[0]);
    assert_eq!(rig.apply(stop, Input::Press), ApplyResult::Sent(1));
    assert!(!rig.c.sampler().playing[2]);
    assert_eq!(rig.apply(play, Input::Release), ApplyResult::Ignored);
}

#[test]
fn keylock_toggle_reads_the_snapshot() {
    let mut rig = Rig::new();
    let keylock = deck_action(ActionKind::KeylockToggle, 0);
    assert!(!rig.c.deck(d(0)).keylock);
    assert_eq!(rig.apply(keylock, Input::Press), ApplyResult::Sent(1));
    assert!(rig.c.deck(d(0)).keylock);
    assert_eq!(rig.apply(keylock, Input::Press), ApplyResult::Sent(1));
    assert!(!rig.c.deck(d(0)).keylock);
    assert_eq!(rig.apply(keylock, Input::Release), ApplyResult::Ignored);
}

#[test]
fn fx_toggles_track_state_and_send() {
    let mut rig = Rig::new();
    let second = deck_action(ActionKind::FxToggle(1), 2);
    assert_eq!(rig.apply(second, Input::Press), ApplyResult::Sent(1));
    assert_eq!(rig.state.deck(d(2)).fx_on, [false, true, false]);
    assert_eq!(rig.c.channel(d(2)).fx_enabled, [false, true, false]);
    assert_eq!(rig.c.channel(d(0)).fx_enabled, [false; 3]);
    assert_eq!(rig.apply(second, Input::Press), ApplyResult::Sent(1));
    assert_eq!(rig.state.deck(d(2)).fx_on, [false; 3]);
    assert_eq!(rig.c.channel(d(2)).fx_enabled, [false; 3]);
    assert_eq!(rig.apply(second, Input::Release), ApplyResult::Ignored);
    let third = deck_action(ActionKind::FxToggle(2), 2);
    rig.apply(third, Input::Press);
    assert_eq!(rig.c.channel(d(2)).fx_enabled, [false, false, true]);
}

#[test]
fn stem_toggles_flip_and_send() {
    let mut rig = Rig::new();
    assert_eq!(rig.state.deck(d(0)).stems_on, [true; 7]);
    for (i, stem) in engine::Stem::ALL.into_iter().enumerate() {
        let kind = ActionKind::StemToggle(stem);
        assert!(kind.accepts(InputKind::Button));
        assert!(!kind.takes_release());
        let action = deck_action(kind, 0);
        assert_eq!(rig.apply(action, Input::Press), ApplyResult::Sent(1));
        assert!(!rig.state.deck(d(0)).stems_on[i]);
        assert_eq!(rig.apply(action, Input::Release), ApplyResult::Ignored);
        assert_eq!(rig.apply(action, Input::Press), ApplyResult::Sent(1));
        assert_eq!(rig.state.deck(d(0)).stems_on, [true; 7]);
        assert_eq!(rig.state.deck(d(1)).stems_on, [true; 7]);
    }
}

#[test]
fn new_stem_toggles_flip_and_send() {
    let mut rig = Rig::new();
    for stem in [
        engine::Stem::Kicks,
        engine::Stem::Melody,
        engine::Stem::Hats,
    ] {
        let kind = ActionKind::StemToggle(stem);
        assert_eq!(ActionKind::from_id(kind.id()), Some(kind));
        let action = deck_action(kind, 0);
        assert_eq!(rig.apply(action, Input::Press), ApplyResult::Sent(1));
        assert!(!rig.state.deck(d(0)).stems_on[stem.index()]);
        assert_eq!(rig.apply(action, Input::Press), ApplyResult::Sent(1));
        assert_eq!(rig.state.deck(d(0)).stems_on, [true; 7]);
    }
}

#[test]
fn stem_flags_reset_on_load() {
    let mut rig = Rig::new();
    let bass = deck_action(ActionKind::StemToggle(engine::Stem::Bass), 0);
    rig.apply(bass, Input::Press);
    assert_eq!(
        rig.state.deck(d(0)).stems_on,
        [true, true, false, true, true, true, true]
    );
    // A new load: the engine turns every stem back on, and the next press starts from all on.
    let track = Arc::new(TrackBuffer::new(SR, vec![0.5; 2000 * 2]).unwrap());
    rig.c
        .send(engine::Command::Deck(
            d(0),
            engine::DeckCommand::Load {
                track,
                memory: engine::TrackMemory::default(),
            },
        ))
        .unwrap();
    rig.render(1);
    // The press resets first (all on), then turns bass off again, not back on.
    rig.apply(bass, Input::Press);
    assert_eq!(
        rig.state.deck(d(0)).stems_on,
        [true, true, false, true, true, true, true]
    );
    rig.c
        .send(engine::Command::Deck(
            d(0),
            engine::DeckCommand::Load {
                track: Arc::new(TrackBuffer::new(SR, vec![0.5; 2000 * 2]).unwrap()),
                memory: engine::TrackMemory::default(),
            },
        ))
        .unwrap();
    rig.render(1);
    let vocals = deck_action(ActionKind::StemToggle(engine::Stem::Vocals), 0);
    rig.apply(vocals, Input::Press);
    assert_eq!(
        rig.state.deck(d(0)).stems_on,
        [false, true, true, true, true, true, true]
    );
}

#[test]
fn fx_level_is_a_takeover_value() {
    let mut rig = Rig::new();
    let level = deck_action(ActionKind::FxLevel, 1);
    assert!(ActionKind::FxLevel.is_absolute());
    assert_eq!(
        rig.state.value((ActionKind::FxLevel, Some(d(1)))),
        Some(0.5)
    );
    assert_eq!(rig.apply(level, Input::Absolute(0.8)), ApplyResult::Sent(1));
    assert_eq!(
        rig.state.value((ActionKind::FxLevel, Some(d(1)))),
        Some(0.8)
    );
    assert_eq!(
        rig.state.value((ActionKind::FxLevel, Some(d(0)))),
        Some(0.5)
    );
    assert_eq!(rig.c.channel(d(1)).fx_amount, 0.8);
    assert_eq!(rig.apply(level, Input::Press), ApplyResult::Ignored);
}

#[test]
fn jog_scratch_steps_become_platter_turns() {
    // A touched deck with vinyl off still scratches from the platter: 720 steps is one turn
    // of 1.8 s of track.
    let moved = |kind, steps, per_turn: Option<u16>| {
        let mut rig = Rig::new();
        if let Some(per_turn) = per_turn {
            rig.state.jog_steps_per_turn = per_turn;
        }
        rig.apply(deck_action(ActionKind::VinylToggle, 0), Input::Press);
        assert!(!rig.c.deck(d(0)).vinyl);
        rig.apply(deck_action(ActionKind::JogTouch, 0), Input::Press);
        let before = rig.c.deck(d(0)).position_frames;
        assert_eq!(
            rig.apply(deck_action(kind, 0), Input::Relative(steps)),
            ApplyResult::Sent(1)
        );
        // The scratch follows the hand over a few blocks.
        rig.render(40 * BLOCK);
        assert!(rig.c.deck(d(0)).jog_touched);
        rig.c.deck(d(0)).position_frames - before
    };
    let one_turn = 1.8 * f64::from(SR);
    assert!((moved(ActionKind::JogScratch, 720, None) - one_turn).abs() < 1.0);
    assert!((moved(ActionKind::JogScratch, 360, Some(360)) - one_turn).abs() < 1.0);
    assert!((moved(ActionKind::JogScratch, 360, None) - one_turn / 2.0).abs() < 1.0);
}

#[test]
fn backspin_uses_the_configured_lengths() {
    let state = ControlState::default();
    assert_eq!((state.backspin_short, state.backspin_long), (1.0, 2.0));
    let travelled = |kind, short, long| {
        let mut rig = Rig::new();
        rig.state.backspin_short = short;
        rig.state.backspin_long = long;
        rig.apply(deck_action(ActionKind::PlayPause, 0), Input::Press);
        rig.c
            .send(engine::Command::Deck(
                d(0),
                engine::DeckCommand::Seek(150_000.0),
            ))
            .unwrap();
        rig.render(1);
        assert_eq!(
            rig.apply(deck_action(kind, 0), Input::Press),
            ApplyResult::Sent(1)
        );
        assert!(rig.c.deck(d(0)).backspinning);
        let start = rig.c.deck(d(0)).position_frames;
        let mut lowest = start;
        for _ in 0..(2 * SR as usize / BLOCK) {
            rig.render(BLOCK);
            lowest = lowest.min(rig.c.deck(d(0)).position_frames);
        }
        // The spin runs back 1.5 times its length in seconds.
        (start - lowest) / f64::from(SR)
    };
    assert!((travelled(ActionKind::BackspinShort, 0.4, 5.0) - 0.6).abs() < 0.05);
    assert!((travelled(ActionKind::BackspinLong, 5.0, 0.8) - 1.2).abs() < 0.05);
}

#[test]
fn loop_fade_starts_a_loop_only_when_none_is_active() {
    let mut rig = Rig::new();
    rig.load_grid(0, 120.0, true);
    let loop_beats = |rig: &Rig| rig.c.deck(d(0)).loop_beats;
    rig.apply(deck_action(ActionKind::LoopFade1, 0), Input::Press);
    assert!(rig.c.deck(d(0)).loop_active);
    assert_eq!(loop_beats(&rig), 1.0);
    // With a loop active, the engine ignores the other fade: the loop and its size stay.
    rig.apply(deck_action(ActionKind::LoopFade4, 0), Input::Press);
    assert!(rig.c.deck(d(0)).loop_active);
    assert_eq!(loop_beats(&rig), 1.0);
    rig.apply(deck_action(ActionKind::LoopExit, 0), Input::Press);
    assert!(!rig.c.deck(d(0)).loop_active);
    rig.apply(deck_action(ActionKind::LoopFade4, 0), Input::Press);
    assert!(rig.c.deck(d(0)).loop_active);
    assert_eq!(loop_beats(&rig), 4.0);
}

#[test]
fn loop_exit_then_fade_in_one_batch_replaces_the_djs_loop() {
    // FX FADE pad 3 sends its own note (loop_exit) and then LOOP_FADE_SEL_1 within a
    // millisecond, so both are applied before the engine renders: neither may decide from
    // a snapshot that predates the other.
    let mut rig = Rig::new();
    rig.load_grid(0, 120.0, true);
    rig.apply(deck_action(ActionKind::LoopDouble, 0), Input::Press);
    rig.apply(deck_action(ActionKind::AutoLoop, 0), Input::Press);
    assert!(rig.c.deck(d(0)).loop_active);
    assert_ne!(rig.c.deck(d(0)).loop_beats, 1.0);
    for kind in [ActionKind::LoopExit, ActionKind::LoopFade1] {
        let result = apply(
            &deck_action(kind, 0),
            Input::Press,
            0,
            &mut rig.state,
            &mut rig.c,
        );
        assert_eq!(result, ApplyResult::Sent(1), "{kind:?}");
    }
    rig.render(1);
    assert!(rig.c.deck(d(0)).loop_active);
    assert_eq!(rig.c.deck(d(0)).loop_beats, 1.0);
}

#[test]
fn loop_exit_without_a_loop_does_nothing() {
    let mut rig = Rig::new();
    rig.load_grid(0, 120.0, true);
    let before = rig.c.deck(d(0));
    rig.apply(deck_action(ActionKind::LoopExit, 0), Input::Press);
    let after = rig.c.deck(d(0));
    assert!(!after.loop_active);
    assert_eq!(after.loop_beats, before.loop_beats);
    // Exiting never reloops, however often it is pressed.
    rig.apply(deck_action(ActionKind::AutoLoop, 0), Input::Press);
    assert!(rig.c.deck(d(0)).loop_active);
    for _ in 0..2 {
        rig.apply(deck_action(ActionKind::LoopExit, 0), Input::Press);
        assert!(!rig.c.deck(d(0)).loop_active);
    }
}

fn seek(rig: &mut Rig, frames: f64) {
    rig.c
        .send(engine::Command::Deck(
            d(0),
            engine::DeckCommand::Seek(frames),
        ))
        .unwrap();
    rig.render(1);
}

#[test]
fn pad_scratch_mode_is_a_pad_mode() {
    use actions::PadMode;
    assert_eq!(PadMode::PadScratch.id(), "pad_scratch");
    assert_eq!(PadMode::from_id("pad_scratch"), Some(PadMode::PadScratch));
    let mut rig = Rig::new();
    let action = Action::new(
        ActionKind::from_id("pads.mode_pad_scratch").unwrap(),
        Target::Side(Side::Right),
    )
    .unwrap();
    assert_eq!(rig.apply(action, Input::Press), ApplyResult::Sent(0));
    assert_eq!(rig.state.side(Side::Right).pad_mode, PadMode::PadScratch);
}

#[test]
fn scratch_anchor_set_stores_the_playhead() {
    let mut rig = Rig::new();
    seek(&mut rig, 50_000.0);
    assert_eq!(rig.state.deck(d(0)).scratch_anchor, None);
    let set = deck_action(ActionKind::ScratchAnchorSet, 0);
    assert_eq!(rig.apply(set, Input::Press), ApplyResult::Sent(0));
    assert_eq!(rig.state.deck(d(0)).scratch_anchor, Some(50_000.0));
    assert_eq!(rig.state.deck(d(1)).scratch_anchor, None);
    assert_eq!(rig.apply(set, Input::Release), ApplyResult::Ignored);
    // A later press moves the anchor.
    seek(&mut rig, 80_000.0);
    rig.apply(set, Input::Press);
    assert_eq!(rig.state.deck(d(0)).scratch_anchor, Some(80_000.0));
}

#[test]
fn scratch_anchor_return_seeks_to_the_anchor() {
    let mut rig = Rig::new();
    seek(&mut rig, 50_000.0);
    rig.apply(deck_action(ActionKind::ScratchAnchorSet, 0), Input::Press);
    seek(&mut rig, 120_000.0);
    let back = deck_action(ActionKind::ScratchAnchorReturn, 0);
    assert_eq!(rig.apply(back, Input::Press), ApplyResult::Sent(1));
    assert_eq!(rig.c.deck(d(0)).position_frames, 50_000.0);
    assert_eq!(rig.state.deck(d(0)).scratch_anchor, Some(50_000.0));
    assert_eq!(rig.apply(back, Input::Release), ApplyResult::Ignored);
}

#[test]
fn scratch_anchor_return_without_an_anchor_sets_it_here() {
    let mut rig = Rig::new();
    seek(&mut rig, 70_000.0);
    let back = deck_action(ActionKind::ScratchAnchorReturn, 0);
    assert_eq!(rig.apply(back, Input::Press), ApplyResult::Sent(0));
    assert_eq!(rig.state.deck(d(0)).scratch_anchor, Some(70_000.0));
    assert_eq!(rig.c.deck(d(0)).position_frames, 70_000.0);
    // The anchor is now there to return to.
    seek(&mut rig, 10_000.0);
    assert_eq!(rig.apply(back, Input::Press), ApplyResult::Sent(1));
    assert_eq!(rig.c.deck(d(0)).position_frames, 70_000.0);
}
