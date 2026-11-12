use actions::{Action, PadMode, Side, pad_action};
use engine::{DeckId, HotcueSlot};

fn slot(number: usize) -> HotcueSlot {
    HotcueSlot::ALL[number - 1]
}

#[test]
fn pad_action_per_mode() {
    let deck = DeckId::new(1).unwrap();
    let side = Side::of(deck);
    let act = |mode, shift| pad_action(mode, side, deck, slot(3), shift);
    assert_eq!(
        act(PadMode::Hardware, false),
        Some(Action::Hotcue(deck, slot(3)))
    );
    assert_eq!(
        act(PadMode::BeatJump, false),
        Some(Action::BeatJump(deck, slot(3)))
    );
    assert_eq!(
        act(PadMode::BeatJump, true),
        Some(Action::BeatJumpShift(deck, slot(3)))
    );
    assert_eq!(
        act(PadMode::LoopRoll, false),
        Some(Action::LoopRoll(deck, slot(3)))
    );
    assert_eq!(
        act(PadMode::Slicer, false),
        Some(Action::Slice(deck, slot(3)))
    );
    // The scratch routine's pads are not the pads on screen: they do nothing here.
    assert_eq!(act(PadMode::PadScratch, false), None);
}
