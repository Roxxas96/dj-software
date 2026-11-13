//! The window's pads send what the shipped SB3 mapping binds its pad notes to, in every
//! pad mode, so the two cannot drift apart.

use actions::{Action, ActionKind, HardwarePadMode, PadMode, Side, Target, window_pad_action};
use controller::mapping::{InputBinding, MidiSource, parse_mapping};
use engine::HotcueSlot;

/// The pad notes: the HOT CUE bank, the same shifted, the SAMPLER bank and the same
/// shifted.
const HOT_CUE: u8 = 0x00;
const HOT_CUE_SHIFT: u8 = 0x08;
const SAMPLER: u8 = 0x30;
const SAMPLER_SHIFT: u8 = 0x38;

/// The action the SB3 mapping binds to pad `slot`'s note `bank + slot index` on `side`
/// while its pads are in `mode`, with the deck the side drives being its main one.
fn bound(bindings: &[InputBinding], side: Side, mode: PadMode, bank: u8, slot: usize) -> Action {
    let channel = 7 + side.index() as u8;
    let note = bank + slot as u8;
    let found: Vec<&InputBinding> = bindings
        .iter()
        .filter(|b| b.midi == MidiSource::Note { channel, note })
        .filter(|b| b.pad_mode == Some(mode))
        .collect();
    assert_eq!(found.len(), 1, "{side:?} {mode:?} note {note:#04x}");
    let target = match found[0].target {
        Target::Side(side) => Target::Deck(side.decks()[0]),
        other => other,
    };
    Action::new(found[0].kind, target).unwrap()
}

#[test]
fn the_windows_pads_send_what_the_sb3_mapping_binds() {
    let text = include_str!("../../../controllers/ddj-sb3/mapping.toml");
    let bindings = parse_mapping("sb3.toml", text).unwrap().inputs;
    for side in Side::ALL {
        let deck = side.decks()[0];
        for (slot, &hotcue) in HotcueSlot::ALL.iter().enumerate() {
            let window = |mode, hardware, shift| {
                window_pad_action(mode, hardware, side, deck, hotcue, shift)
            };
            let on = |mode, bank| bound(&bindings, side, mode, bank, slot);
            let cases = [
                (
                    PadMode::Hardware,
                    Some(HardwarePadMode::HotCue),
                    false,
                    HOT_CUE,
                ),
                (PadMode::Hardware, None, false, HOT_CUE),
                (
                    PadMode::Hardware,
                    Some(HardwarePadMode::Sampler),
                    false,
                    SAMPLER,
                ),
                (
                    PadMode::Hardware,
                    Some(HardwarePadMode::Trans),
                    false,
                    SAMPLER,
                ),
                // SHIFT + a sampler pad stops its slot.
                (
                    PadMode::Hardware,
                    Some(HardwarePadMode::Sampler),
                    true,
                    SAMPLER_SHIFT,
                ),
                (
                    PadMode::Hardware,
                    Some(HardwarePadMode::Trans),
                    true,
                    SAMPLER_SHIFT,
                ),
                (PadMode::BeatJump, None, false, HOT_CUE),
                (PadMode::BeatJump, None, true, HOT_CUE_SHIFT),
                (PadMode::LoopRoll, None, false, HOT_CUE),
                (PadMode::Slicer, None, false, HOT_CUE),
            ];
            for (mode, hardware, shift, bank) in cases {
                assert_eq!(
                    window(mode, hardware, shift),
                    Some(on(mode, bank)),
                    "{side:?} {mode:?} {hardware:?} shift {shift} slot {}",
                    slot + 1
                );
            }
            // The scratch routine's pads bind the scratch anchor, which no pad on screen
            // stands for: the window sends nothing.
            assert_eq!(window(PadMode::PadScratch, None, false), None);
        }
        // Every pad_scratch binding of the side is a scratch anchor action.
        let scratch: Vec<ActionKind> = bindings
            .iter()
            .filter(|b| b.pad_mode == Some(PadMode::PadScratch) && b.side() == Some(side))
            .map(|b| b.kind)
            .collect();
        assert!(!scratch.is_empty(), "{side:?}");
        for kind in scratch {
            assert!(
                matches!(
                    kind,
                    ActionKind::ScratchAnchorSet | ActionKind::ScratchAnchorReturn
                ),
                "{side:?} {kind:?}"
            );
        }
    }
}
