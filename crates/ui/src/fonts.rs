//! Fallback fonts for text egui's own fonts can't draw (Arabic, CJK, accents beyond
//! Latin): the first of a few system fonts that exist, read at start-up. Nothing is shipped.

use std::sync::Arc;

use egui::{Context, FontData, FontDefinitions, FontFamily};

/// Candidate system fonts, tried in order; every one that loads is added.
const CANDIDATES: &[&str] = &[
    "/System/Library/Fonts/Supplemental/Arial Unicode.ttf",
    "/System/Library/Fonts/PingFang.ttc",
    r"C:\Windows\Fonts\arialuni.ttf",
    r"C:\Windows\Fonts\seguisym.ttf",
    r"C:\Windows\Fonts\msyh.ttc",
    "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf",
    "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
];

/// Appends the system fallback fonts after egui's defaults. Missing files are skipped.
pub fn install(ctx: &Context) {
    let mut fonts = FontDefinitions::default();
    for (n, path) in CANDIDATES.iter().enumerate() {
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        let name = format!("system fallback {n}");
        fonts
            .font_data
            .insert(name.clone(), Arc::new(FontData::from_owned(bytes)));
        for family in [FontFamily::Proportional, FontFamily::Monospace] {
            fonts.families.entry(family).or_default().push(name.clone());
        }
    }
    ctx.set_fonts(fonts);
}
