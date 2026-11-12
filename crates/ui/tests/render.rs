//! The whole window, drawn on the GPU to PNG files under `target/ui-render/`, for the owner
//! to set beside VirtualDJ. Ignored, as CI has no GPU: run it with
//! `cargo test -p ui --test render -- --ignored`.

mod support;

use std::collections::HashSet;
use std::path::PathBuf;

use egui::{Vec2, vec2};
use ui::{ColourScheme, Layout};

const WINDOW: Vec2 = vec2(1440.0, 900.0);
const SMALL_WINDOW: Vec2 = vec2(1024.0, 700.0);
/// A drawn window has more colours than this; a blank or broken one fewer.
const MIN_COLOURS: usize = 50;

/// Where the renders go: `target/ui-render/` at the workspace's root.
fn out_dir() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/ui-render");
    std::fs::create_dir_all(&dir).expect("creating target/ui-render");
    dir
}

/// Renders `layout` at `size` in `scheme` to `<name>.png`, and checks the image is the
/// window, drawn.
fn render(name: &str, layout: Layout, four_decks: bool, size: Vec2, scheme: ColourScheme) {
    let (mut frame, reads) = support::loaded(layout, four_decks);
    frame.view.colour_scheme = scheme;
    let image = support::render(frame, reads, size);
    assert_eq!(
        (image.width(), image.height()),
        (size.x as u32, size.y as u32),
        "{name} is not the window's size"
    );
    let colours: HashSet<[u8; 4]> = image.pixels().map(|p| p.0).collect();
    assert!(
        colours.len() > MIN_COLOURS,
        "{name} has only {} colours",
        colours.len()
    );
    let path = out_dir().join(format!("{name}.png"));
    image.save(&path).expect("writing the render");
    println!("wrote {}", path.display());
}

#[test]
#[ignore = "needs a GPU; writes target/ui-render/*.png"]
fn render_every_layout() {
    for (name, layout) in [
        ("starter", Layout::Starter),
        ("essentials", Layout::Essentials),
        ("pro", Layout::Pro),
        ("performance", Layout::Performance),
    ] {
        render(name, layout, false, WINDOW, ColourScheme::Dark);
    }
    render("pro-4decks", Layout::Pro, true, WINDOW, ColourScheme::Dark);
    render(
        "pro-small",
        Layout::Pro,
        false,
        SMALL_WINDOW,
        ColourScheme::Dark,
    );
    render("pro-light", Layout::Pro, false, WINDOW, ColourScheme::Light);
}
