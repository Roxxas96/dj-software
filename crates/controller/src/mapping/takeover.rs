//! Soft takeover: a physical control only takes over a value once it reaches it, so
//! switching decks or changing a value elsewhere never makes the value jump.

use std::collections::HashMap;

use actions::ValueTarget;
use engine::DeckId;

/// How close the physical value must come to the current one to pick it up.
pub const TOLERANCE: f32 = 0.02;

/// Values closer than this count as unchanged when checking for changes made elsewhere.
const SAME: f32 = 1e-6;

/// Takeover state per [`ValueTarget`]. A target with no entry is armed with no history,
/// which is the state every target starts in when the controller attaches.
#[derive(Debug, Default)]
pub struct Takeover {
    entries: HashMap<ValueTarget, Entry>,
}

#[derive(Debug, Clone, Copy)]
struct Entry {
    armed: bool,
    /// The last value passed through for this target.
    sent: Option<f32>,
    /// The last physical value seen for this target.
    phys: Option<f32>,
    /// The current value when the last physical value was dropped.
    waiting_on: Option<f32>,
}

impl Entry {
    const ARMED: Entry = Entry {
        armed: true,
        sent: None,
        phys: None,
        waiting_on: None,
    };
}

impl Takeover {
    pub fn new() -> Self {
        Self::default()
    }

    /// Arms every target, forgetting all history: the controller (re)attached.
    pub fn arm_all(&mut self) {
        self.entries.clear();
    }

    /// Arms every target of `deck`, forgetting its history: the deck became active on its
    /// side, so the physical controls' previous positions belong to another deck.
    pub fn arm_deck(&mut self, deck: DeckId) {
        self.entries.retain(|(_, d), _| *d != Some(deck));
    }

    /// Arms `target`, with `prev_phys` as the physical value its control last had (on
    /// whatever target it drove then), so a crossing can be detected from the first move.
    pub fn arm(&mut self, target: ValueTarget, prev_phys: Option<f32>) {
        self.entries.insert(
            target,
            Entry {
                phys: prev_phys,
                ..Entry::ARMED
            },
        );
    }

    /// Whether the physical value `phys` for `target` may pass, given the target's
    /// `current` value. Passing disarms the target; it re-arms when `current` no longer
    /// matches the last value passed (another source changed it).
    pub fn filter(&mut self, target: ValueTarget, phys: f32, current: f32) -> bool {
        let entry = self.entries.entry(target).or_insert(Entry::ARMED);
        if entry.sent.is_some_and(|sent| (sent - current).abs() > SAME) {
            entry.armed = true;
        }
        let pass = !entry.armed
            || (phys - current).abs() <= TOLERANCE
            || entry.phys.is_some_and(|prev| {
                (prev < current && phys > current) || (prev > current && phys < current)
            });
        entry.phys = Some(phys);
        if pass {
            entry.armed = false;
            entry.sent = Some(phys);
            entry.waiting_on = None;
        } else {
            entry.waiting_on = Some(current);
        }
        pass
    }

    /// Targets whose last physical value was dropped: `(target, current, phys)`, with
    /// `current` as of that value.
    pub fn waiting(&self) -> impl Iterator<Item = (ValueTarget, f32, f32)> + '_ {
        self.entries.iter().filter_map(|(target, entry)| {
            entry
                .armed
                .then_some((*target, entry.waiting_on?, entry.phys?))
        })
    }
}

#[cfg(test)]
mod tests {
    use actions::ActionKind;

    use super::*;

    const XF: ValueTarget = (ActionKind::Crossfader, None);

    #[test]
    fn unknown_previous_uses_tolerance_only() {
        let mut t = Takeover::new();
        assert!(!t.filter(XF, 0.0, 0.5));
        assert!(t.filter(XF, 0.49, 0.5));
    }

    #[test]
    fn arm_deck_leaves_other_targets() {
        let mut t = Takeover::new();
        let fader1 = (ActionKind::Fader, DeckId::new(0));
        assert!(t.filter(fader1, 0.5, 0.5));
        assert!(t.filter(XF, 0.5, 0.5));
        t.arm_deck(DeckId::new(0).unwrap());
        assert!(!t.filter(fader1, 0.9, 0.5));
        assert!(t.filter(XF, 0.9, 0.5));
    }

    #[test]
    fn arm_with_previous_detects_crossing_on_first_move() {
        let mut t = Takeover::new();
        assert!(t.filter(XF, 0.5, 0.5));
        t.arm(XF, Some(0.9));
        assert!(!t.filter(XF, 0.8, 0.5));
        t.arm(XF, Some(0.9));
        assert!(t.filter(XF, 0.1, 0.5));
    }
}
