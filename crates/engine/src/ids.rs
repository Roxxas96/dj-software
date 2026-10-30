/// One of the four decks. Users see deck numbers as `index + 1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DeckId(u8);

impl DeckId {
    pub const ALL: [DeckId; 4] = [DeckId(0), DeckId(1), DeckId(2), DeckId(3)];

    /// Returns the deck at zero-based `index`, or `None` if `index > 3`.
    pub fn new(index: usize) -> Option<DeckId> {
        Self::ALL.get(index).copied()
    }

    pub fn index(self) -> usize {
        usize::from(self.0)
    }
}
