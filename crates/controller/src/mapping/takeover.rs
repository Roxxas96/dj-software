//! Soft takeover: a physical control only takes over a value once it reaches it, so
//! switching decks or changing a value elsewhere never makes the value jump.

use std::collections::{HashMap, HashSet};

use actions::ValueTarget;
use engine::DeckId;

/// How close the physical value must come to the current one to pick it up.
pub const TOLERANCE: f32 = 0.02;

/// Values closer than this count as unchanged when checking for changes made elsewhere.
const SAME: f32 = 1e-6;

/// Takeover state per [`ValueTarget`]. A target with no entry is armed with no history,
/// which is the state every target starts in when the controller reattaches. After
/// [`disarm_all`](Takeover::disarm_all) (the first attach), a target with no entry is
/// disarmed instead, unless its deck was armed since.
#[derive(Debug, Default)]
pub struct Takeover {
    entries: HashMap<ValueTarget, Entry>,
    /// Targets with no entry take their first physical value as is.
    take_first: bool,
    /// Decks armed by [`arm_deck`](Takeover::arm_deck) while `take_first` holds.
    armed_decks: HashSet<DeckId>,
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

    const DISARMED: Entry = Entry {
        armed: false,
        ..Entry::ARMED
    };
}

impl Takeover {
    pub fn new() -> Self {
        Self::default()
    }

    /// Arms every target, forgetting all history: the controller reattached.
    pub fn arm_all(&mut self) {
        self.entries.clear();
        self.take_first = false;
        self.armed_decks.clear();
    }

    /// Disarms every target, forgetting all history: the controller attached for the first
    /// time, so nothing is known about where its controls are and the first physical value
    /// of each one is taken as is. Later arming (a deck switch, a layer change, a value
    /// changed elsewhere, [`arm_all`](Self::arm_all)) works as usual.
    pub fn disarm_all(&mut self) {
        self.entries.clear();
        self.take_first = true;
        self.armed_decks.clear();
    }

    /// Arms every target of `deck`, forgetting its history: the deck became active on its
    /// side, so the physical controls' previous positions belong to another deck.
    pub fn arm_deck(&mut self, deck: DeckId) {
        self.entries.retain(|(_, d), _| *d != Some(deck));
        if self.take_first {
            self.armed_decks.insert(deck);
        }
    }

    /// The state of a target with no entry yet.
    fn fresh(&self, target: ValueTarget) -> Entry {
        let deck_armed = target.1.is_some_and(|d| self.armed_decks.contains(&d));
        if self.take_first && !deck_armed {
            Entry::DISARMED
        } else {
            Entry::ARMED
        }
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
        let fresh = self.fresh(target);
        let entry = self.entries.entry(target).or_insert(fresh);
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
    fn disarm_all_takes_first_value_then_arms_on_change_elsewhere() {
        let mut t = Takeover::new();
        t.disarm_all();
        assert!(t.filter(XF, 0.0, 0.5), "first value taken");
        assert!(!t.filter(XF, 0.2, 0.3), "changed elsewhere: armed again");
        t.arm_all();
        assert!(!t.filter(XF, 0.0, 0.5), "reattach arms");
    }

    #[test]
    fn arm_deck_after_disarm_all_arms_that_deck_only() {
        let mut t = Takeover::new();
        t.disarm_all();
        let fader1 = (ActionKind::Fader, DeckId::new(0));
        let fader3 = (ActionKind::Fader, DeckId::new(2));
        t.arm_deck(DeckId::new(2).unwrap());
        assert!(!t.filter(fader3, 0.9, 0.0), "deck 3 became active: armed");
        assert!(
            t.filter(fader1, 0.9, 0.0),
            "deck 1 untouched: first value taken"
        );
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
