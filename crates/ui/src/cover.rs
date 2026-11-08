//! Cover art: a track's embedded picture, read and decoded on a background thread and kept
//! as a texture for the [`COVERS_KEPT`] tracks last asked for.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use egui::{
    ColorImage, Context, CornerRadius, Rect, Sense, Stroke, TextureHandle, TextureOptions, Ui,
    Vec2, pos2, vec2,
};
use lofty::config::ParseOptions;
use lofty::file::TaggedFileExt;
use lofty::picture::PictureType;
use lofty::probe::Probe;

use crate::theme;

/// How many tracks' covers are kept, the most recently asked for.
pub const COVERS_KEPT: usize = 32;
/// The longest side of a decoded cover, in pixels: larger pictures are scaled down.
pub const COVER_PIXELS: u32 = 256;
/// The covers asked for, each loading or loaded. The thread that reads them starts at the
/// first request and ends once this is dropped.
#[derive(Default)]
pub struct Covers {
    entries: HashMap<PathBuf, Entry>,
    /// The paths in `entries`, least recently asked for first.
    recent: VecDeque<PathBuf>,
    worker: Option<Worker>,
}

impl std::fmt::Debug for Covers {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Covers")
            .field("kept", &self.entries.len())
            .finish_non_exhaustive()
    }
}

enum Entry {
    Loading,
    /// `None`: the file has no picture that decodes.
    Loaded(Option<TextureHandle>),
}

struct Worker {
    requests: Sender<PathBuf>,
    results: Receiver<(PathBuf, Option<ColorImage>)>,
}

impl Covers {
    /// The cover of the track at `path`: `None` while it loads and when it has none. The
    /// first call for a path starts loading it.
    pub fn get(&mut self, ctx: &Context, path: &Path) -> Option<TextureHandle> {
        self.collect(ctx);
        if let Some(entry) = self.entries.get(path) {
            let texture = match entry {
                Entry::Loading => None,
                Entry::Loaded(texture) => texture.clone(),
            };
            self.touch(path);
            return texture;
        }
        let entry = match self.worker() {
            Some(worker) if worker.requests.send(path.to_owned()).is_ok() => Entry::Loading,
            // Without the thread no cover can load: show the plain tile.
            _ => Entry::Loaded(None),
        };
        self.entries.insert(path.to_owned(), entry);
        self.recent.push_back(path.to_owned());
        while self.recent.len() > COVERS_KEPT {
            if let Some(oldest) = self.recent.pop_front() {
                self.entries.remove(&oldest);
            }
        }
        None
    }

    /// Turns the pictures the thread has decoded into textures. A picture whose path was
    /// dropped meanwhile is discarded.
    fn collect(&mut self, ctx: &Context) {
        let Some(worker) = &self.worker else { return };
        for (path, image) in worker.results.try_iter() {
            if let Some(entry @ Entry::Loading) = self.entries.get_mut(&path) {
                let name = format!("cover {}", path.display());
                let texture =
                    image.map(|image| ctx.load_texture(name, image, TextureOptions::LINEAR));
                *entry = Entry::Loaded(texture);
            }
        }
    }

    /// Moves `path` to the most recently asked for.
    fn touch(&mut self, path: &Path) {
        if let Some(at) = self.recent.iter().position(|p| p == path)
            && let Some(p) = self.recent.remove(at)
        {
            self.recent.push_back(p);
        }
    }

    /// The loading thread, started on first use; `None` if it could not start.
    fn worker(&mut self) -> Option<&Worker> {
        if self.worker.is_none() {
            let (requests, incoming) = mpsc::channel::<PathBuf>();
            let (done, results) = mpsc::channel();
            let spawned = thread::Builder::new()
                .name("covers".to_owned())
                .spawn(move || {
                    for path in incoming {
                        let image = read(&path);
                        if done.send((path, image)).is_err() {
                            break;
                        }
                    }
                });
            if spawned.is_ok() {
                self.worker = Some(Worker { requests, results });
            }
        }
        self.worker.as_ref()
    }
}

/// `path`'s front cover, else its first picture, decoded and scaled to at most
/// [`COVER_PIXELS`] a side. `None` when the file can't be read or has no picture that
/// decodes.
pub fn read(path: &Path) -> Option<ColorImage> {
    let options = ParseOptions::new().read_properties(false);
    let file = Probe::open(path).ok()?.options(options).read().ok()?;
    let pictures = || file.tags().iter().flat_map(|tag| tag.pictures());
    let picture = pictures()
        .find(|p| p.pic_type() == PictureType::CoverFront)
        .or_else(|| pictures().next())?;
    decode(picture.data())
}

/// An encoded JPEG or PNG as an image of at most [`COVER_PIXELS`] a side.
pub fn decode(bytes: &[u8]) -> Option<ColorImage> {
    let mut image = image::load_from_memory(bytes).ok()?;
    if image.width() > COVER_PIXELS || image.height() > COVER_PIXELS {
        image = image.thumbnail(COVER_PIXELS, COVER_PIXELS);
    }
    let rgba = image.to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    Some(ColorImage::from_rgba_unmultiplied(size, rgba.as_raw()))
}

/// Draws a square tile of `side` points: `cover`, or a plain tile with a note.
pub fn tile(ui: &mut Ui, cover: Option<&TextureHandle>, side: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(side), Sense::hover());
    match cover {
        Some(texture) => {
            egui::Image::from_texture(texture)
                .corner_radius(CornerRadius::same(2))
                .paint_at(ui, rect);
        }
        None => no_cover(ui, rect),
    }
}

/// A plain tile with an eighth note in its middle.
fn no_cover(ui: &Ui, rect: Rect) {
    let p = theme::palette(ui.ctx());
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(2), p.lane);
    let unit = rect.height() / 8.0;
    let head = rect.center() + vec2(-unit, 1.5 * unit);
    painter.circle_filled(head, unit, p.note);
    let stem_x = head.x + unit * 0.9;
    let stem = Stroke::new(unit * 0.35, p.note);
    let top = pos2(stem_x, head.y - 4.0 * unit);
    painter.line_segment([pos2(stem_x, head.y), top], stem);
    painter.line_segment([top, top + vec2(1.5 * unit, 1.5 * unit)], stem);
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;
    use std::time::{Duration, Instant};

    use super::*;

    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        image::RgbaImage::from_pixel(width, height, image::Rgba([255, 0, 0, 255]))
            .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
            .unwrap();
        bytes
    }

    #[test]
    fn a_cover_decodes_and_large_ones_shrink() {
        let small = decode(&png(4, 2)).unwrap();
        assert_eq!(small.size, [4, 2]);
        assert_eq!(small.pixels[0], egui::Color32::RED);
        let large = decode(&png(1024, 512)).unwrap();
        assert_eq!(large.size, [256, 128]);
        assert!(decode(b"not an image").is_none());
    }

    #[test]
    fn a_file_without_a_picture_loads_as_no_cover() {
        let ctx = Context::default();
        let mut covers = Covers::default();
        let path = Path::new("/nonexistent/open-jockey/track.mp3");
        assert!(covers.get(&ctx, path).is_none());
        let deadline = Instant::now() + Duration::from_secs(5);
        while matches!(covers.entries.get(path), Some(Entry::Loading)) {
            assert!(Instant::now() < deadline, "the cover never loaded");
            thread::sleep(Duration::from_millis(5));
            covers.collect(&ctx);
        }
        assert!(matches!(
            covers.entries.get(path),
            Some(Entry::Loaded(None))
        ));
        assert!(covers.get(&ctx, path).is_none());
    }

    #[test]
    fn only_the_most_recent_covers_are_kept() {
        let ctx = Context::default();
        let mut covers = Covers::default();
        let path = |n: usize| PathBuf::from(format!("/nonexistent/{n}.mp3"));
        covers.get(&ctx, &path(0));
        for n in 1..=COVERS_KEPT {
            // Asking for the first again keeps it the most recent.
            covers.get(&ctx, &path(0));
            covers.get(&ctx, &path(n));
        }
        assert_eq!(covers.entries.len(), COVERS_KEPT);
        assert!(covers.entries.contains_key(&path(0)));
        assert!(!covers.entries.contains_key(&path(1)));
        assert!(covers.entries.contains_key(&path(COVERS_KEPT)));
    }
}
