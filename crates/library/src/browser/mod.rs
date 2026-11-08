//! The library browser the controller's BROWSE knob drives, laid out like VirtualDJ's: a
//! tree of the drives and their folders on disk, then the playlists, and the list of the
//! highlighted tree entry (a folder's own audio files, or a playlist's tracks).
//!
//! The disk is read on the thread that runs the browser, never on the control loop: a
//! folder's subfolders when it opens, and its files once each time the highlight moves to
//! it. Lists are held a page at a time around the cursor, so a 5,000-file folder or a
//! 50,000-track playlist never builds all its rows.

mod drives;
mod tree;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub use actions::BrowserRequest;
pub use drives::{Drive, drives};
pub use tree::{TreeKind, TreeRow};

use crate::scan::{is_hidden, is_supported, normalize_path};
use crate::{LibraryError, Reader, TrackFilter, TrackRow};
use tree::{Target, Tree};

/// How many rows of a list the browser holds at once: one page around the cursor.
pub const BROWSER_PAGE_ROWS: usize = 200;

/// The browser pane the BROWSE knob moves.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Focus {
    /// The drives, folders and playlists.
    #[default]
    Tree,
    /// The highlighted tree entry's files or tracks.
    List,
}

/// One row of the list: an audio file, with its library track when it has one.
#[derive(Debug, Clone, PartialEq)]
pub struct ListRow {
    pub path: PathBuf,
    /// `None` for a file the library doesn't have.
    pub track: Option<TrackRow>,
    pub file_name: String,
}

/// What the browser shows.
#[derive(Debug, Clone, PartialEq)]
pub struct BrowserView {
    /// The highlighted tree entry after its parents, such as `"Playlists ▸ Peak time"` or
    /// `"Macintosh HD ▸ Users ▸ me ▸ Music"`.
    pub path: String,
    /// The cursor in the focused pane, from 0.
    pub index: usize,
    /// How many rows the focused pane has.
    pub count: usize,
    /// The selected track: the library track at the list cursor, whichever pane has focus.
    pub track: Option<TrackRow>,
    pub focus: Focus,
    /// Every visible tree row.
    pub tree: Vec<TreeRow>,
    pub tree_cursor: usize,
    /// The loaded page of the list: rows `list_offset..list_offset + list.len()`.
    pub list: Vec<ListRow>,
    pub list_offset: usize,
    /// How many rows the whole list has.
    pub list_count: usize,
    pub list_cursor: usize,
}

/// Where a list's rows come from.
#[derive(Debug, Clone, Default)]
enum Items {
    #[default]
    None,
    /// A folder's audio files sorted by name, and the library's tracks among them.
    Files {
        files: Vec<PathBuf>,
        tracks: HashMap<PathBuf, TrackRow>,
    },
    Playlist(i64),
}

/// The highlighted entry's list: its rows' source, length, cursor and loaded page.
#[derive(Debug, Clone, Default)]
struct List {
    items: Items,
    count: usize,
    cursor: usize,
    /// The list position of `rows[0]`.
    offset: usize,
    rows: Vec<ListRow>,
}

impl List {
    /// The list of `target`, with the cursor on its first row and that row's page loaded.
    /// A folder's files are read here, once.
    fn open(reader: &Reader, target: &Target) -> Result<Self, LibraryError> {
        let (items, count) = match target {
            Target::Dir(dir) => {
                let items = folder_items(reader, dir)?;
                let count = match &items {
                    Items::Files { files, .. } => files.len(),
                    _ => 0,
                };
                (items, count)
            }
            Target::Playlists => (Items::None, 0),
            Target::Playlist { id, len } => (Items::Playlist(*id), *len),
        };
        let mut list = Self {
            items,
            count,
            ..Self::default()
        };
        list.seek(reader, 0)?;
        Ok(list)
    }

    fn covers(&self, index: usize) -> bool {
        index >= self.offset && index - self.offset < self.rows.len()
    }

    /// Puts the cursor on `index` (or the last row), loading its page if needed. On an
    /// error the list is unchanged.
    fn seek(&mut self, reader: &Reader, index: usize) -> Result<(), LibraryError> {
        let to = index.min(self.count.saturating_sub(1));
        if !self.covers(to) && self.count > 0 {
            let offset = to.saturating_sub(BROWSER_PAGE_ROWS / 2);
            self.rows = page(reader, &self.items, offset)?;
            self.offset = offset;
        }
        self.cursor = to;
        Ok(())
    }

    fn selected(&self) -> Option<&ListRow> {
        if self.covers(self.cursor) {
            self.rows.get(self.cursor - self.offset)
        } else {
            None
        }
    }
}

/// The browser state: the tree, its cursor, the focused pane and the highlighted entry's
/// list.
#[derive(Debug)]
pub struct Browser {
    reader: Reader,
    tree: Tree,
    tree_cursor: usize,
    list: List,
    focus: Focus,
}

impl Browser {
    /// [`Browser::with_roots`] on the computer's [`drives`].
    ///
    /// # Errors
    ///
    /// Any database error.
    pub fn new(reader: Reader) -> Result<Self, LibraryError> {
        Self::with_roots(reader, drives())
    }

    /// A browser whose tree starts with `roots`, closed, then the playlists, with the
    /// first row highlighted, its list at the top and the tree focused.
    ///
    /// # Errors
    ///
    /// Any database error.
    pub fn with_roots(reader: Reader, roots: Vec<Drive>) -> Result<Self, LibraryError> {
        let tree = Tree::new(roots, &reader.playlists()?);
        let list = List::open(&reader, tree.target(0))?;
        Ok(Self {
            reader,
            tree,
            tree_cursor: 0,
            list,
            focus: Focus::Tree,
        })
    }

    /// Scrolls the focused pane, switches panes, or opens or closes the highlighted
    /// folder, and returns the new view.
    ///
    /// A scroll stops at the first and last rows (it does not wrap) and moves over the
    /// visible tree rows only. Moving to another tree entry reads its list and puts the
    /// cursor on its first row. Opening or closing a folder works from either pane; on a
    /// playlist, or the Playlists node, it does nothing.
    ///
    /// # Errors
    ///
    /// Any database error; the browser is then unchanged.
    pub fn apply(&mut self, req: BrowserRequest) -> Result<BrowserView, LibraryError> {
        match req {
            BrowserRequest::Scroll(steps) => match self.focus {
                Focus::Tree => self.move_tree(steps)?,
                Focus::List => {
                    let to = step(self.list.cursor, steps, self.list.count);
                    self.list.seek(&self.reader, to)?;
                }
            },
            BrowserRequest::ToggleFolder => self.tree.toggle(self.tree_cursor),
            BrowserRequest::ToggleFocus => {
                self.focus = match self.focus {
                    Focus::Tree => Focus::List,
                    Focus::List => Focus::Tree,
                };
            }
        }
        Ok(self.view())
    }

    /// Reads the playlists and the highlighted list again, to see library and disk
    /// changes (new playlists, scanned tracks, files added to the folder). Open folders
    /// stay open. The highlighted entry stays highlighted and so does the selected row,
    /// wherever it moved in the list (in a playlist, the occurrence of its track nearest
    /// the old cursor); if the row left the list, the cursor stays on the same row where
    /// it fits. If the entry is gone, the cursor stays at the same tree row.
    ///
    /// # Errors
    ///
    /// Any database error; the browser is then unchanged.
    pub fn refresh(&mut self) -> Result<BrowserView, LibraryError> {
        let mut tree = self.tree.clone();
        tree.set_playlists(&self.reader.playlists()?);
        let current = self.tree.target(self.tree_cursor);
        let same_row =
            self.tree_cursor < tree.len() && tree.target(self.tree_cursor).same_entry(current);
        let found = if same_row {
            Some(self.tree_cursor)
        } else {
            tree.position(current)
        };
        let (tree_cursor, mut list, list_cursor) = match found {
            Some(i) => {
                let list = List::open(&self.reader, tree.target(i))?;
                let at = match self.list.selected() {
                    Some(row) => find_row(&self.reader, &list, row, self.list.cursor)?,
                    None => None,
                };
                (i, list, at.unwrap_or(self.list.cursor))
            }
            None => {
                let i = self.tree_cursor.min(tree.len() - 1);
                (i, List::open(&self.reader, tree.target(i))?, 0)
            }
        };
        list.seek(&self.reader, list_cursor)?;
        self.tree = tree;
        self.tree_cursor = tree_cursor;
        self.list = list;
        Ok(self.view())
    }

    /// What the browser shows now.
    #[must_use]
    pub fn view(&self) -> BrowserView {
        let (index, count) = match self.focus {
            Focus::Tree => (self.tree_cursor, self.tree.len()),
            Focus::List => (self.list.cursor, self.list.count),
        };
        BrowserView {
            path: self.tree.path(self.tree_cursor),
            index,
            count,
            track: self.selected_track(),
            focus: self.focus,
            tree: self.tree.rows(),
            tree_cursor: self.tree_cursor,
            list: self.list.rows.clone(),
            list_offset: self.list.offset,
            list_count: self.list.count,
            list_cursor: self.list.cursor,
        }
    }

    /// The library track at the list cursor (what LOAD loads), whichever pane has focus.
    /// `None` when the list is empty or the file is not in the library.
    #[must_use]
    pub fn selected_track(&self) -> Option<TrackRow> {
        self.list.selected().and_then(|r| r.track.clone())
    }

    /// The highlighted folder, when its list has audio files the library doesn't have.
    #[must_use]
    pub fn folder_with_unknown_files(&self) -> Option<PathBuf> {
        let Target::Dir(dir) = self.tree.target(self.tree_cursor) else {
            return None;
        };
        match &self.list.items {
            Items::Files { files, tracks } if files.iter().any(|f| !tracks.contains_key(f)) => {
                Some(dir.clone())
            }
            _ => None,
        }
    }

    fn move_tree(&mut self, steps: i32) -> Result<(), LibraryError> {
        let to = step(self.tree_cursor, steps, self.tree.len());
        if to != self.tree_cursor {
            self.list = List::open(&self.reader, self.tree.target(to))?;
            self.tree_cursor = to;
        }
        Ok(())
    }
}

/// The text in the last `(...)` or `[...]` of a title, trimmed: the mix name VirtualDJ
/// shows in its Remix column, such as "Extended Mix" for `"Just Us (Extended Mix)"`.
/// `None` when there is no such group or it is empty.
#[must_use]
pub fn remix_of(title: &str) -> Option<&str> {
    let close = title.rfind([')', ']'])?;
    let open = if title[close..].starts_with(')') {
        '('
    } else {
        '['
    };
    let start = title[..close].rfind(open)?;
    let inner = title[start + 1..close].trim();
    (!inner.is_empty()).then_some(inner)
}

/// `cursor` moved by `steps`, kept within `0..count` (0 when `count` is 0).
fn step(cursor: usize, steps: i32, count: usize) -> usize {
    let last = count.saturating_sub(1);
    let by = usize::try_from(steps.unsigned_abs()).unwrap_or(usize::MAX);
    if steps >= 0 {
        cursor.saturating_add(by).min(last)
    } else {
        cursor.saturating_sub(by).min(last)
    }
}

/// `dir`'s audio files and the library's tracks among them. A folder that can't be read
/// has no files. Paths are canonical, as the library stores them.
fn folder_items(reader: &Reader, dir: &Path) -> Result<Items, LibraryError> {
    let dir = normalize_path(dir).unwrap_or_else(|_| dir.to_owned());
    let files = audio_files(&dir).unwrap_or_default();
    let tracks = if files.is_empty() {
        HashMap::new()
    } else {
        let filter = TrackFilter {
            folder: Some(dir),
            folder_only: true,
            ..TrackFilter::default()
        };
        match reader.track_files(&filter) {
            Ok(tracks) => tracks.into_iter().collect(),
            // The library holds only UTF-8 paths.
            Err(LibraryError::NonUtf8Path(_)) => HashMap::new(),
            Err(e) => return Err(e),
        }
    };
    Ok(Items::Files { files, tracks })
}

/// `dir`'s visible audio files (not its subfolders'), sorted by file name ignoring case.
fn audio_files(dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)?
        .flatten()
        .filter(|e| !is_hidden(&e.file_name()))
        .map(|e| e.path())
        .filter(|p| is_supported(p) && p.is_file())
        .collect();
    files.sort_by_cached_key(|p| file_name(p).to_lowercase());
    Ok(files)
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// The page of `items` starting at `offset`.
fn page(reader: &Reader, items: &Items, offset: usize) -> Result<Vec<ListRow>, LibraryError> {
    Ok(match items {
        Items::None => Vec::new(),
        Items::Files { files, tracks } => files
            .iter()
            .skip(offset)
            .take(BROWSER_PAGE_ROWS)
            .map(|path| ListRow {
                path: path.clone(),
                track: tracks.get(path).cloned(),
                file_name: file_name(path),
            })
            .collect(),
        Items::Playlist(id) => reader
            .playlist_files(*id, offset, BROWSER_PAGE_ROWS)?
            .into_iter()
            .map(|(path, track)| ListRow {
                file_name: file_name(&path),
                path,
                track: Some(track),
            })
            .collect(),
    })
}

/// Where `row` is in `list`, if it is there: a folder's file by path; in a playlist
/// (where a track may appear more than once), its track's occurrence nearest `near`,
/// read a page at a time.
fn find_row(
    reader: &Reader,
    list: &List,
    row: &ListRow,
    near: usize,
) -> Result<Option<usize>, LibraryError> {
    match &list.items {
        Items::None => Ok(None),
        Items::Files { files, .. } => Ok(files.iter().position(|p| *p == row.path)),
        Items::Playlist(playlist) => {
            let Some(id) = row.track.as_ref().map(|t| t.id) else {
                return Ok(None);
            };
            let mut best: Option<usize> = None;
            let mut offset = 0;
            while offset < list.count {
                let rows = reader.playlist_tracks(*playlist, offset, BROWSER_PAGE_ROWS)?;
                if rows.is_empty() {
                    break;
                }
                for (i, row) in rows.iter().enumerate() {
                    let at = offset + i;
                    if row.id == id && best.is_none_or(|b| at.abs_diff(near) < b.abs_diff(near)) {
                        best = Some(at);
                    }
                }
                offset += rows.len();
            }
            Ok(best)
        }
    }
}
