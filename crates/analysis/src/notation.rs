//! Musical keys and their Camelot, Open Key and standard notations.

/// One of the 24 major and minor keys: 0–11 are C, C♯ … B major; 12–23 are C … B minor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Key(u8);

/// How keys are shown to the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum KeyNotation {
    /// "8B" = C major, "8A" = A minor. The default, as in VirtualDJ.
    #[default]
    Camelot,
    /// "C", "Am", "F♯m".
    Standard,
    /// "1d" = C major, "1m" = A minor.
    OpenKey,
}

const NAMES: [&str; 12] = [
    "C", "C♯", "D", "D♯", "E", "F", "F♯", "G", "G♯", "A", "A♯", "B",
];

/// Camelot codes by key index (the published Camelot wheel: 8B = C, 8A = A minor, and one
/// step clockwise is a fifth up).
const CAMELOT: [&str; 24] = [
    // C     C♯    D      D♯    E      F     F♯    G     G♯    A      A♯    B
    "8B", "3B", "10B", "5B", "12B", "7B", "2B", "9B", "4B", "11B", "6B", "1B",
    // Cm    C♯m    Dm    D♯m   Em    Fm    F♯m    Gm    G♯m   Am    A♯m   Bm
    "5A", "12A", "7A", "2A", "9A", "4A", "11A", "6A", "1A", "8A", "3A", "10A",
];

/// Open Key codes by key index (the published Open Key wheel: 1d = C, 1m = A minor; each
/// number is the Camelot number minus 7, wrapped to 1–12).
const OPEN_KEY: [&str; 24] = [
    // C     C♯    D     D♯     E     F      F♯    G     G♯    A     A♯     B
    "1d", "8d", "3d", "10d", "5d", "12d", "7d", "2d", "9d", "4d", "11d", "6d",
    // Cm     C♯m   Dm     D♯m   Em    Fm    F♯m   Gm     G♯m   Am    A♯m   Bm
    "10m", "5m", "12m", "7m", "2m", "9m", "4m", "11m", "6m", "1m", "8m", "3m",
];

impl Key {
    /// `None` unless `index < 24`.
    pub fn new(index: u8) -> Option<Self> {
        (index < 24).then_some(Self(index))
    }

    pub(crate) fn major(root: usize) -> Self {
        Self((root % 12) as u8)
    }

    pub(crate) fn minor(root: usize) -> Self {
        Self((root % 12) as u8 + 12)
    }

    pub fn index(self) -> u8 {
        self.0
    }

    pub fn is_minor(self) -> bool {
        self.0 >= 12
    }

    /// Pitch class of the tonic, 0 = C.
    pub fn root(self) -> u8 {
        self.0 % 12
    }

    pub fn camelot(self) -> &'static str {
        CAMELOT[usize::from(self.0)]
    }

    pub fn open_key(self) -> &'static str {
        OPEN_KEY[usize::from(self.0)]
    }

    /// "C", "Am", "F♯m": sharps throughout, "m" for minor.
    pub fn standard(self) -> String {
        let name = NAMES[usize::from(self.root())];
        if self.is_minor() {
            format!("{name}m")
        } else {
            name.to_owned()
        }
    }

    pub fn to_notation(self, notation: KeyNotation) -> String {
        match notation {
            KeyNotation::Camelot => self.camelot().to_owned(),
            KeyNotation::Standard => self.standard(),
            KeyNotation::OpenKey => self.open_key().to_owned(),
        }
    }
}
