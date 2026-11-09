//! What each of VirtualDJ's four layouts shows: which controls the deck panels and the
//! mixer draw, and how the height is shared between the decks and the browser.

use engine::DeckId;

use crate::view::Layout;

/// The share of the window's height the waveform lanes take, whatever the layout.
pub const LANES_SHARE: f32 = 0.22;

/// What a deck panel and the screen show.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sections {
    pub fx: bool,
    /// The effect level bar under the FX slots (the slots' names and on/off stay).
    pub fx_level: bool,
    pub pads: bool,
    pub jog: bool,
    /// The tempo slider; without the jog it is drawn compact.
    pub pitch: bool,
    pub loop_controls: bool,
    pub overview: bool,
    pub mixer_eq: bool,
    pub mixer_meters: bool,
    /// The deck panels are squeezed to fit two on a side (the 4-Decks view): the tempo
    /// slider, the states and the transport share one row.
    pub compact: bool,
    /// The browser's share of the window's height.
    pub browser_share: f32,
    /// The deck panels' and mixer's share of the window's height.
    pub decks_share: f32,
}

/// The sections `layout` shows.
pub fn sections(layout: Layout) -> Sections {
    let all = |decks_share: f32| Sections {
        fx: true,
        fx_level: true,
        pads: true,
        jog: true,
        pitch: true,
        loop_controls: true,
        overview: true,
        mixer_eq: true,
        mixer_meters: true,
        compact: false,
        browser_share: 1.0 - LANES_SHARE - decks_share,
        decks_share,
    };
    match layout {
        Layout::Starter => Sections {
            fx: false,
            fx_level: false,
            pads: false,
            mixer_meters: false,
            ..all(0.42)
        },
        Layout::Essentials => Sections {
            fx_level: false,
            ..all(0.40)
        },
        Layout::Pro => all(0.38),
        Layout::Performance => Sections {
            jog: false,
            ..all(0.26)
        },
    }
}

/// The sections of the 4-Decks view, whatever the layout: compact panels with the
/// overview, the loop, the EQ and the meters.
pub fn four_deck_sections() -> Sections {
    let decks_share = 0.40;
    Sections {
        fx: false,
        fx_level: false,
        pads: false,
        jog: false,
        pitch: false,
        loop_controls: true,
        overview: true,
        mixer_eq: true,
        mixer_meters: true,
        compact: true,
        browser_share: 1.0 - LANES_SHARE - decks_share,
        decks_share,
    }
}

/// The decks the screen has a panel for, in panel order. With two decks they are the
/// sides' `active` decks, left then right. With four they are always 1, 3, 2, 4: left top,
/// left bottom, right top, right bottom.
pub fn deck_slots(four_decks: bool, active: [DeckId; 2]) -> Vec<DeckId> {
    if four_decks {
        [0, 2, 1, 3].map(|i| DeckId::ALL[i]).to_vec()
    } else {
        active.to_vec()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn deck(number: usize) -> DeckId {
        DeckId::new(number - 1).unwrap()
    }

    #[test]
    fn four_deck_slots_are_fixed() {
        let numbers = |slots: Vec<DeckId>| slots.iter().map(|d| d.index() + 1).collect::<Vec<_>>();
        assert_eq!(numbers(deck_slots(true, [deck(3), deck(2)])), [1, 3, 2, 4]);
        assert_eq!(numbers(deck_slots(false, [deck(3), deck(2)])), [3, 2]);
    }

    #[test]
    fn sections_match_the_layouts() {
        let starter = sections(Layout::Starter);
        assert!(!starter.fx && !starter.pads && starter.jog);
        assert!(starter.loop_controls && starter.mixer_eq && !starter.mixer_meters);
        let performance = sections(Layout::Performance);
        assert!(!performance.jog && performance.pads && performance.fx);
        let pro = sections(Layout::Pro);
        assert!(
            pro.fx
                && pro.fx_level
                && pro.pads
                && pro.jog
                && pro.pitch
                && pro.loop_controls
                && pro.overview
                && pro.mixer_eq
                && pro.mixer_meters
        );
        assert!(performance.decks_share < pro.decks_share);
        for layout in Layout::ALL {
            let s = sections(*layout);
            assert!((s.decks_share + s.browser_share + LANES_SHARE - 1.0).abs() < 1e-6);
        }
    }
}
