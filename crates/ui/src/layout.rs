//! What each of VirtualDJ's four layouts shows: which controls the deck panels and the
//! mixer draw; the deck row is as tall as its contents and the browser takes the rest.

use engine::DeckId;

use egui::Vec2;

use crate::view::{Layout, MAX_SHARE, MIN_SHARE, PanelSizes, ViewState};

/// The share of the window's height the waveform lanes take, whatever the layout.
pub const LANES_SHARE: f32 = 0.22;

/// The least an area may take, in points.
pub const MIN_POINTS: f32 = 80.0;
/// The most the lanes and the decks may take of the height together.
const MAX_ROWS: f32 = 0.85;
/// The most the tree, the info panel and the sideview may take of the width together.
const MAX_COLUMNS: f32 = 0.8;

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
    /// The deck panels are squeezed to fit two on a side (the 4-Decks view): no tempo
    /// slider, and the transport, the states and the loop share one row.
    pub compact: bool,
}

/// The sections `layout` shows.
pub fn sections(layout: Layout) -> Sections {
    let all = Sections {
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
    };
    match layout {
        Layout::Starter => Sections {
            fx: false,
            fx_level: false,
            pads: false,
            mixer_meters: false,
            ..all
        },
        Layout::Essentials => Sections {
            fx_level: false,
            ..all
        },
        Layout::Pro => all,
        Layout::Performance => Sections { jog: false, ..all },
    }
}

/// `sizes` kept within what `window` can show: each share at least [`MIN_SHARE`] and
/// [`MIN_POINTS`] (of the height for the lanes and decks, of the width for the rest) and
/// at most [`MAX_SHARE`]; the decks' at least `min_decks` points too, so every area of
/// the deck panels shows. The lanes and decks take at most 0.85 of the height together
/// and the tree, info panel and sideview 0.8 of the width. A
/// share that is not a number goes back to its default.
pub fn clamp_sizes(sizes: PanelSizes, window: Vec2, min_decks: f32) -> PanelSizes {
    let defaults = PanelSizes::default();
    let rows = MIN_SHARE.max(MIN_POINTS / window.y.max(1.0));
    let decks_min = rows.max(min_decks / window.y.max(1.0)).min(MAX_ROWS - rows);
    let columns = MIN_SHARE.max(MIN_POINTS / window.x.max(1.0));
    let one = |value: f32, default: f32, min: f32| {
        let value = if value.is_finite() { value } else { default };
        value.clamp(min, MAX_SHARE.max(min))
    };
    let [lanes, decks] = fit(
        [
            one(sizes.lanes, defaults.lanes, rows),
            one(sizes.decks, defaults.decks, decks_min),
        ],
        [rows, decks_min],
        MAX_ROWS,
    );
    let [tree, info, sideview] = fit(
        [
            one(sizes.tree, defaults.tree, columns),
            one(sizes.info, defaults.info, columns),
            one(sizes.sideview, defaults.sideview, columns),
        ],
        [columns; 3],
        MAX_COLUMNS,
    );
    PanelSizes {
        lanes,
        decks,
        tree,
        info,
        sideview,
    }
}

/// The shares `window` draws for the session's `sizes` with `sections`: the decks' share
/// still at its default means the viewer never set it, and the deck row is then as tall as
/// its contents ([`crate::deck::row_height`]), the spare height going to the browser.
pub fn drawn_sizes(sizes: PanelSizes, sections: &Sections, window: Vec2) -> PanelSizes {
    let mut sizes = sizes;
    let contents = crate::deck::row_height(sections);
    if sizes.decks == PanelSizes::default().decks {
        sizes.decks = contents / window.y.max(1.0);
    }
    clamp_sizes(sizes, window, contents)
}

/// `shares` (each at least its `mins`) shrunk together, above their minimum, until they
/// add up to at most `cap`.
fn fit<const N: usize>(shares: [f32; N], mins: [f32; N], cap: f32) -> [f32; N] {
    let sum: f32 = shares.iter().sum();
    if sum <= cap {
        return shares;
    }
    let floor: f32 = mins.iter().sum();
    // A hair under the cap so rounding cannot put the sum over it.
    let keep = ((cap - 1e-5 - floor) / (sum - floor)).max(0.0);
    std::array::from_fn(|i| mins[i] + (shares[i] - mins[i]) * keep)
}

/// The sections the window shows for `view`: the 4-Decks view's or the layout's, with the
/// overview only when `view` has it on.
pub fn shown_sections(view: &ViewState) -> Sections {
    let mut sections = if view.four_decks {
        four_deck_sections()
    } else {
        sections(view.layout)
    };
    sections.overview &= view.overview;
    sections
}

/// The sections of the 4-Decks view, whatever the layout: compact panels with the
/// overview, the loop, the EQ and the meters.
pub fn four_deck_sections() -> Sections {
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

/// The decks the 4-channel mixer has a strip for, left to right: 3, 1, 2, 4.
pub fn mixer_order() -> [DeckId; 4] {
    [2, 0, 1, 3].map(|i| DeckId::ALL[i])
}

#[cfg(test)]
mod tests {
    use egui::vec2;

    use super::*;

    fn deck(number: usize) -> DeckId {
        DeckId::new(number - 1).unwrap()
    }

    #[test]
    fn sizes_are_clamped() {
        for window in [vec2(1440.0, 900.0), vec2(1024.0, 700.0)] {
            for value in [0.0, 2.0, f32::NAN] {
                let sizes = clamp_sizes(
                    PanelSizes {
                        lanes: value,
                        decks: value,
                        tree: value,
                        info: value,
                        sideview: value,
                    },
                    window,
                    0.0,
                );
                let rows = [sizes.lanes, sizes.decks];
                let columns = [sizes.tree, sizes.info, sizes.sideview];
                for (share, extent) in rows
                    .iter()
                    .map(|s| (s, window.y))
                    .chain(columns.iter().map(|s| (s, window.x)))
                {
                    assert!(*share >= 0.05 && share * extent >= 80.0 - 1e-3, "{sizes:?}");
                }
                assert!(rows.iter().sum::<f32>() <= 0.85, "{sizes:?}");
                assert!(columns.iter().sum::<f32>() <= 0.8, "{sizes:?}");
            }
        }
        // Sizes already within the limits are left alone.
        let fine = PanelSizes::default();
        assert_eq!(clamp_sizes(fine, vec2(1440.0, 900.0), 0.0), fine);
    }

    #[test]
    fn default_deck_row_fits_its_contents() {
        let window = vec2(1440.0, 900.0);
        for layout in Layout::ALL {
            for sections in [sections(*layout), four_deck_sections()] {
                let drawn = drawn_sizes(PanelSizes::default(), &sections, window).decks;
                let height = crate::deck::row_height(&sections);
                assert!(
                    (drawn * window.y - height).abs() <= 1.0,
                    "{layout:?}: {} against {height}",
                    drawn * window.y
                );
            }
        }
    }

    #[test]
    fn a_saved_deck_share_still_applies() {
        let window = vec2(1440.0, 900.0);
        let sizes = PanelSizes {
            decks: 0.5,
            ..PanelSizes::default()
        };
        for layout in Layout::ALL {
            let drawn = drawn_sizes(sizes, &sections(*layout), window);
            assert!((drawn.decks * window.y - 450.0).abs() <= 1.0, "{drawn:?}");
        }
    }

    #[test]
    fn four_deck_slots_are_fixed() {
        let numbers = |slots: Vec<DeckId>| slots.iter().map(|d| d.index() + 1).collect::<Vec<_>>();
        assert_eq!(numbers(deck_slots(true, [deck(3), deck(2)])), [1, 3, 2, 4]);
        assert_eq!(numbers(deck_slots(false, [deck(3), deck(2)])), [3, 2]);
    }

    #[test]
    fn the_mixer_orders_its_strips_3_1_2_4() {
        let numbers: Vec<usize> = mixer_order().iter().map(|d| d.index() + 1).collect();
        assert_eq!(numbers, [3, 1, 2, 4]);
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
    }
}
