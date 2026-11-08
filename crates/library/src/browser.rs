//! The library browser the controller's BROWSE knob drives: a tree of playlists, library
//! folders and all tracks, and the track list of the selected tree entry.
//!
//! Track lists are read a page at a time around the cursor, so a 50,000-track library
//! never loads all its rows, and their lengths come from `count` queries (or the playlist
//! summaries).

use std::path::PathBuf;

pub use actions::BrowserRequest;

use crate::{LibraryError, Reader, TrackFilter, TrackRow, TrackSort};

/// How many rows of a track list the browser holds at once: one page around the cursor.
pub const BROWSER_PAGE_ROWS: usize = 200;

/// The browser pane the BROWSE knob moves.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Focus {
    /// Playlists, folders and "All tracks".
    #[default]
    Tree,
    /// The tracks of the selected tree entry.
    List,
}

/// What the browser shows.
#[derive(Debug, Clone, PartialEq)]
pub struct BrowserView {
    /// The selected tree entry, such as `"Playlists ▸ Peak time"`, `"Folders ▸ /music"`
    /// or `"All tracks"`.
    pub path: String,
    /// The cursor in the focused pane, from 0.
    pub index: usize,
    /// How many rows the focused pane has.
    pub count: usize,
    /// The selected track: the one at the list cursor, whichever pane has focus.
    pub track: Option<TrackRow>,
    pub focus: Focus,
}

/// Where a tree entry's tracks come from.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Source {
    /// A playlist and its entry count.
    Playlist {
        id: i64,
        len: usize,
    },
    Folder(PathBuf),
    All,
}

impl Source {
    /// Whether `self` and `other` are the same entry, even if its contents changed.
    fn same_entry(&self, other: &Source) -> bool {
        match (self, other) {
            (Source::Playlist { id: a, .. }, Source::Playlist { id: b, .. }) => a == b,
            _ => self == other,
        }
    }
}

#[derive(Debug, Clone)]
struct Entry {
    path: String,
    source: Source,
}

/// The track list of the selected entry: its length, the cursor and the loaded page.
#[derive(Debug, Clone, Default)]
struct List {
    count: usize,
    cursor: usize,
    /// The list position of `rows[0]`.
    offset: usize,
    rows: Vec<TrackRow>,
}

impl List {
    fn covers(&self, index: usize) -> bool {
        index >= self.offset && index - self.offset < self.rows.len()
    }
}

/// The browser state: the tree, its cursor, the focused pane and the selected entry's
/// track list.
#[derive(Debug)]
pub struct Browser {
    reader: Reader,
    /// Never empty: "All tracks" is always there.
    entries: Vec<Entry>,
    tree_cursor: usize,
    list: List,
    focus: Focus,
}

impl Browser {
    /// A browser on the first tree entry and the top of its list, with the tree focused.
    ///
    /// # Errors
    ///
    /// Any database error.
    pub fn new(reader: Reader) -> Result<Self, LibraryError> {
        let entries = entries(&reader)?;
        let list = open_list(&reader, &entries[0].source, 0)?;
        Ok(Self {
            reader,
            entries,
            tree_cursor: 0,
            list,
            focus: Focus::Tree,
        })
    }

    /// Scrolls the focused pane or switches panes, and returns the new view.
    ///
    /// A scroll stops at the first and last rows (it does not wrap). Moving to another
    /// tree entry puts its list cursor on the first track.
    ///
    /// # Errors
    ///
    /// Any database error; the browser is then unchanged.
    pub fn apply(&mut self, req: BrowserRequest) -> Result<BrowserView, LibraryError> {
        match req {
            BrowserRequest::Scroll(steps) => match self.focus {
                Focus::Tree => self.move_tree(steps)?,
                Focus::List => self.move_list(steps)?,
            },
            // Task 3 opens and closes folders.
            BrowserRequest::ToggleFolder => {}
            BrowserRequest::ToggleFocus => {
                self.focus = match self.focus {
                    Focus::Tree => Focus::List,
                    Focus::List => Focus::Tree,
                };
            }
        }
        Ok(self.view())
    }

    /// Reads the tree and the selected list again, to see library changes (new
    /// playlists, scanned tracks). The selected entry stays selected and so does the
    /// selected track, wherever it moved in the list (in a playlist, the occurrence
    /// nearest the old cursor); if the track left the list, the cursor stays on the same
    /// row where it fits. If the entry is gone, the cursor stays at the same tree row.
    ///
    /// # Errors
    ///
    /// Any database error; the browser is then unchanged.
    pub fn refresh(&mut self) -> Result<BrowserView, LibraryError> {
        let entries = entries(&self.reader)?;
        let current = &self.entries[self.tree_cursor].source;
        let (tree_cursor, list_cursor) = match entries
            .iter()
            .position(|e| e.source.same_entry(current))
        {
            Some(i) => {
                let selected = self.selected_track().map(|t| t.id);
                let found = match selected {
                    Some(id) => find_track(&self.reader, &entries[i].source, id, self.list.cursor)?,
                    None => None,
                };
                (i, found.unwrap_or(self.list.cursor))
            }
            None => (self.tree_cursor.min(entries.len() - 1), 0),
        };
        let list = open_list(&self.reader, &entries[tree_cursor].source, list_cursor)?;
        self.entries = entries;
        self.tree_cursor = tree_cursor;
        self.list = list;
        Ok(self.view())
    }

    /// What the browser shows now.
    #[must_use]
    pub fn view(&self) -> BrowserView {
        let (index, count) = match self.focus {
            Focus::Tree => (self.tree_cursor, self.entries.len()),
            Focus::List => (self.list.cursor, self.list.count),
        };
        BrowserView {
            path: self.entries[self.tree_cursor].path.clone(),
            index,
            count,
            track: self.selected_track(),
            focus: self.focus,
        }
    }

    /// The track at the list cursor (what LOAD loads), whichever pane has focus. `None`
    /// when the selected entry has no tracks.
    #[must_use]
    pub fn selected_track(&self) -> Option<TrackRow> {
        let list = &self.list;
        if list.covers(list.cursor) {
            list.rows.get(list.cursor - list.offset).cloned()
        } else {
            None
        }
    }

    /// How many track rows the browser holds, to test that lists are paged.
    #[doc(hidden)]
    #[must_use]
    pub fn loaded_rows(&self) -> usize {
        self.list.rows.len()
    }

    fn move_tree(&mut self, steps: i32) -> Result<(), LibraryError> {
        let to = step(self.tree_cursor, steps, self.entries.len());
        if to != self.tree_cursor {
            self.list = open_list(&self.reader, &self.entries[to].source, 0)?;
            self.tree_cursor = to;
        }
        Ok(())
    }

    fn move_list(&mut self, steps: i32) -> Result<(), LibraryError> {
        let to = step(self.list.cursor, steps, self.list.count);
        if !self.list.covers(to) && self.list.count > 0 {
            let source = &self.entries[self.tree_cursor].source;
            let (offset, rows) = page(&self.reader, source, to)?;
            self.list.offset = offset;
            self.list.rows = rows;
        }
        self.list.cursor = to;
        Ok(())
    }
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

/// The tree: every playlist in list order, every library folder by path, then
/// "All tracks".
fn entries(reader: &Reader) -> Result<Vec<Entry>, LibraryError> {
    let playlists = reader.playlists()?.into_iter().map(|p| Entry {
        path: format!("Playlists ▸ {}", p.name),
        source: Source::Playlist {
            id: p.id,
            len: usize::try_from(p.track_count).unwrap_or(usize::MAX),
        },
    });
    let folders = reader.folders()?.into_iter().map(|f| Entry {
        path: format!("Folders ▸ {}", f.display()),
        source: Source::Folder(f),
    });
    let all = Entry {
        path: "All tracks".to_owned(),
        source: Source::All,
    };
    Ok(playlists.chain(folders).chain([all]).collect())
}

/// The list of `source` with the cursor at `cursor` (or its last row), and the page
/// around it loaded.
fn open_list(reader: &Reader, source: &Source, cursor: usize) -> Result<List, LibraryError> {
    let count = match source {
        Source::Playlist { len, .. } => *len,
        Source::Folder(folder) => reader.track_count(&folder_filter(folder))?,
        Source::All => reader.track_count(&TrackFilter::default())?,
    };
    let cursor = cursor.min(count.saturating_sub(1));
    let (offset, rows) = if count == 0 {
        (0, Vec::new())
    } else {
        page(reader, source, cursor)?
    };
    Ok(List {
        count,
        cursor,
        offset,
        rows,
    })
}

/// Where track `id` is in `source`'s list, if it is there; in a playlist (where a track
/// may appear more than once), the occurrence nearest `near`. A playlist is read a page at
/// a time; a folder or "All tracks" takes one count query.
fn find_track(
    reader: &Reader,
    source: &Source,
    id: i64,
    near: usize,
) -> Result<Option<usize>, LibraryError> {
    match source {
        Source::Playlist { id: playlist, len } => {
            let mut best: Option<usize> = None;
            let mut offset = 0;
            while offset < *len {
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
        Source::Folder(folder) => {
            reader.track_position(&folder_filter(folder), TrackSort::default(), id)
        }
        Source::All => reader.track_position(&TrackFilter::default(), TrackSort::default(), id),
    }
}

/// The page of `source` around `index`: its offset and rows.
fn page(
    reader: &Reader,
    source: &Source,
    index: usize,
) -> Result<(usize, Vec<TrackRow>), LibraryError> {
    let offset = index.saturating_sub(BROWSER_PAGE_ROWS / 2);
    let rows = match source {
        Source::Playlist { id, .. } => reader.playlist_tracks(*id, offset, BROWSER_PAGE_ROWS)?,
        Source::Folder(folder) => reader.tracks(
            &folder_filter(folder),
            TrackSort::default(),
            offset,
            BROWSER_PAGE_ROWS,
        )?,
        Source::All => reader.tracks(
            &TrackFilter::default(),
            TrackSort::default(),
            offset,
            BROWSER_PAGE_ROWS,
        )?,
    };
    Ok((offset, rows))
}

fn folder_filter(folder: &std::path::Path) -> TrackFilter {
    TrackFilter {
        folder: Some(folder.to_path_buf()),
        ..TrackFilter::default()
    }
}
