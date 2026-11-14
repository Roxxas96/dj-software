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

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

pub use actions::BrowserRequest;
pub use drives::{Drive, drives};
pub use tree::{TreeKind, TreeRow};

use crate::scan::{is_hidden, is_supported, normalize_path};
use crate::{LibraryError, Reader, TrackFilter, TrackRow, TrackSort, TrackSortKey};
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

/// A change to the browser that names its target, as a click in the window does (the
/// BROWSE knob's [`BrowserRequest`]s move from where the cursor is instead).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrowserEdit {
    /// Highlights tree row `0..` (the last row past the end) and focuses the tree.
    SelectTree(usize),
    /// Puts the list cursor on row `0..` of the whole list (the last row past the end),
    /// loading its page, and focuses the list.
    SelectRow(usize),
    /// Loads the page of the list around row `offset` (the last row past the end), as the
    /// wheel scrolling past the loaded rows asks; the cursor, its track (what LOAD loads)
    /// and the focus stay.
    ShowPage { offset: usize },
    /// Opens or closes the drive or folder at tree row `0..`; the highlight stays on its
    /// entry, or moves to the folder when the folder closes over it.
    ToggleTreeAt(usize),
    /// Selects every row of the list, whatever its length, and focuses the list. The
    /// browser only notes it ([`BrowserView::all_selected`]); no row ids are built until
    /// something needs them ([`Browser::list_track_ids`]). Any other edit that picks or
    /// changes the list (not [`BrowserEdit::ShowPage`]) takes it back.
    SelectAll,
    /// Filters the list by `text` (`None`: no filter). `seq` grows with each query, so the
    /// window can drop views of older ones.
    Search { text: Option<String>, seq: u64 },
    /// Orders the list by `key`, or in its natural order with `None` (a playlist's own
    /// order, a folder's by file name). `seq` as for [`BrowserEdit::Search`].
    Sort {
        key: Option<TrackSortKey>,
        descending: bool,
        seq: u64,
    },
}

/// One row of the list: an audio file, with its library track when it has one.
#[derive(Debug, Clone, PartialEq)]
pub struct ListRow {
    pub path: PathBuf,
    /// `None` for a file the library doesn't have.
    pub track: Option<TrackRow>,
    pub file_name: String,
    /// The row's position in its playlist, from 0, whatever the list's search and sort;
    /// `None` outside a playlist.
    pub position: Option<usize>,
    /// The cover the library kept for the track (found by an online lookup). The browser
    /// leaves it `None`: a lookup is no reason to read the list again, so the app fills it
    /// in for the row it shows.
    pub cover_file: Option<PathBuf>,
}

/// Which list the browser shows: a whole-list selection is only good for the list it was
/// made on, so what acts on it carries this and the browser compares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListIdentity {
    /// The highlighted tree row.
    pub tree_cursor: usize,
    /// The latest search or sort the list reflects.
    pub seq: u64,
    /// How many rows the list has.
    pub count: usize,
    /// The library id of the playlist shown; `None` for a folder.
    pub playlist: Option<i64>,
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
    /// The text the list is filtered by.
    pub search: Option<String>,
    /// The column the list is ordered by, and whether descending; `None` for the list's
    /// natural order.
    pub sort: Option<(TrackSortKey, bool)>,
    /// The `seq` of the latest [`BrowserEdit::Search`] or [`BrowserEdit::Sort`] this view
    /// reflects (0 before any).
    pub seq: u64,
    /// The library id of the playlist the list shows; `None` for a folder's list.
    pub playlist: Option<i64>,
    /// Every row of the list is selected ([`BrowserEdit::SelectAll`]); `list_count` is how
    /// many.
    pub all_selected: bool,
}

impl BrowserView {
    /// The list this view shows.
    #[must_use]
    pub fn identity(&self) -> ListIdentity {
        ListIdentity {
            tree_cursor: self.tree_cursor,
            seq: self.seq,
            count: self.list_count,
            playlist: self.playlist,
        }
    }
}

/// What every list is filtered and ordered by, whichever tree entry it belongs to.
#[derive(Debug, Clone, Default, PartialEq)]
struct Query {
    /// As [`TrackFilter::text`]; never empty.
    text: Option<String>,
    /// `None`: the list's natural order.
    sort: Option<TrackSort>,
}

/// Where a list's rows come from.
#[derive(Debug, Clone, Default)]
enum Items {
    #[default]
    None,
    /// A folder's audio files that match the query, in its order (by file name ignoring
    /// case when unsorted), and the library's tracks among all its files.
    Files {
        files: Vec<PathBuf>,
        tracks: HashMap<PathBuf, TrackRow>,
        /// Some of the folder's files, matching or not, are not in the library.
        unknown: bool,
    },
    /// A playlist's entries that match `query`, in its order.
    Playlist { id: i64, query: Query },
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
    /// The row at the cursor, kept while the loaded page is elsewhere.
    selected: Option<ListRow>,
}

impl List {
    /// The list of `target` filtered and ordered by `query`, with the cursor on its first
    /// row and that row's page loaded. A folder's files are read here, once.
    fn open(reader: &Reader, target: &Target, query: &Query) -> Result<Self, LibraryError> {
        let (items, count) = match target {
            Target::Dir(dir) => {
                let items = folder_items(reader, dir, query)?;
                let count = match &items {
                    Items::Files { files, .. } => files.len(),
                    _ => 0,
                };
                (items, count)
            }
            Target::Playlists => (Items::None, 0),
            Target::Playlist { id, len } => {
                let count = match &query.text {
                    Some(text) => reader.playlist_match_count(*id, text)?,
                    None => *len,
                };
                let query = query.clone();
                (Items::Playlist { id: *id, query }, count)
            }
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
        self.show(reader, to)?;
        self.cursor = to;
        self.selected = self.row(to).cloned();
        Ok(())
    }

    /// Loads the page around `index` (or the last row) unless it is loaded; the cursor
    /// stays. On an error the list is unchanged.
    fn show(&mut self, reader: &Reader, index: usize) -> Result<(), LibraryError> {
        let to = index.min(self.count.saturating_sub(1));
        if !self.covers(to) && self.count > 0 {
            let offset = to.saturating_sub(BROWSER_PAGE_ROWS / 2);
            self.rows = page(reader, &self.items, offset)?;
            self.offset = offset;
        }
        Ok(())
    }

    /// Row `index`, if its page is loaded.
    fn row(&self, index: usize) -> Option<&ListRow> {
        if self.covers(index) {
            self.rows.get(index - self.offset)
        } else {
            None
        }
    }

    /// The row at the cursor, wherever the loaded page is.
    fn selected(&self) -> Option<&ListRow> {
        self.selected.as_ref()
    }
}

/// The browser state: the tree, its cursor, the focused pane and the highlighted entry's
/// list.
pub struct Browser {
    reader: Reader,
    /// Reads the drives at the top of the tree.
    roots: Box<dyn Fn() -> Vec<Drive> + Send>,
    tree: Tree,
    tree_cursor: usize,
    list: List,
    focus: Focus,
    /// The search and sort every list opens with.
    query: Query,
    /// The `seq` of the latest search or sort.
    seq: u64,
    /// Every row of the list is selected.
    all_selected: bool,
}

impl std::fmt::Debug for Browser {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Browser")
            .field("reader", &self.reader)
            .field("tree", &self.tree)
            .field("tree_cursor", &self.tree_cursor)
            .field("list", &self.list)
            .field("focus", &self.focus)
            .field("query", &self.query)
            .field("seq", &self.seq)
            .finish_non_exhaustive()
    }
}

impl Browser {
    /// [`Browser::with_roots`] on the computer's [`drives`].
    ///
    /// # Errors
    ///
    /// Any database error.
    pub fn new(reader: Reader) -> Result<Self, LibraryError> {
        Self::with_roots(reader, drives)
    }

    /// A browser whose tree starts with the drives `roots` gives, closed, then the
    /// playlists, with the first row highlighted, its list at the top and the tree
    /// focused. Each [`refresh`](Self::refresh) asks `roots` again.
    ///
    /// # Errors
    ///
    /// Any database error.
    pub fn with_roots(
        reader: Reader,
        roots: impl Fn() -> Vec<Drive> + Send + 'static,
    ) -> Result<Self, LibraryError> {
        let tree = Tree::new(roots(), &reader.playlists()?);
        let query = Query::default();
        let list = List::open(&reader, tree.target(0), &query)?;
        Ok(Self {
            reader,
            roots: Box::new(roots),
            tree,
            tree_cursor: 0,
            list,
            focus: Focus::Tree,
            query,
            seq: 0,
            all_selected: false,
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
            BrowserRequest::Scroll(steps) => {
                self.all_selected = false;
                match self.focus {
                    Focus::Tree => self.move_tree(steps)?,
                    Focus::List => {
                        let to = step(self.list.cursor, steps, self.list.count);
                        self.list.seek(&self.reader, to)?;
                    }
                }
            }
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

    /// Makes `edit` and returns the new view.
    ///
    /// A search or a sort reads the highlighted entry's list again with the cursor on its
    /// first row, and stays set for every list after it, until the next one changes it.
    /// An empty search is no search. In a folder, a search keeps the library tracks whose
    /// title or artist holds the text and the other files whose name does; a sort puts
    /// those other files with the tracks whose value is unknown: first ascending, last
    /// descending, as the library's track queries do.
    ///
    /// # Errors
    ///
    /// Any database error; the browser is then unchanged, except that a failed search or
    /// sort still takes its `seq`.
    pub fn edit(&mut self, edit: BrowserEdit) -> Result<BrowserView, LibraryError> {
        if !matches!(edit, BrowserEdit::SelectAll | BrowserEdit::ShowPage { .. }) {
            self.all_selected = false;
        }
        match edit {
            BrowserEdit::SelectAll => {
                self.all_selected = self.list.count > 0;
                self.focus = Focus::List;
            }
            BrowserEdit::SelectTree(at) => {
                let to = at.min(self.tree.len() - 1);
                if to != self.tree_cursor {
                    self.list = List::open(&self.reader, self.tree.target(to), &self.query)?;
                    self.tree_cursor = to;
                }
                self.focus = Focus::Tree;
            }
            BrowserEdit::SelectRow(at) => {
                self.list.seek(&self.reader, at)?;
                self.focus = Focus::List;
            }
            BrowserEdit::ShowPage { offset } => self.list.show(&self.reader, offset)?,
            BrowserEdit::ToggleTreeAt(at) if at < self.tree.len() => self.toggle_at(at)?,
            BrowserEdit::ToggleTreeAt(_) => {}
            BrowserEdit::Search { text, seq } => {
                let text = text.filter(|t| !t.is_empty());
                let query = Query {
                    text,
                    ..self.query.clone()
                };
                self.requery(query, seq)?;
            }
            BrowserEdit::Sort {
                key,
                descending,
                seq,
            } => {
                let sort = key.map(|key| TrackSort { key, descending });
                let query = Query {
                    sort,
                    ..self.query.clone()
                };
                self.requery(query, seq)?;
            }
        }
        Ok(self.view())
    }

    /// Reads the highlighted entry's list again under `query`, the latest one (`seq`).
    /// The browser takes `seq` even when the read fails, keeping its list and query, so
    /// that its views stay newer than every query the window sent.
    fn requery(&mut self, query: Query, seq: u64) -> Result<(), LibraryError> {
        self.seq = seq;
        self.list = List::open(&self.reader, self.tree.target(self.tree_cursor), &query)?;
        self.query = query;
        Ok(())
    }

    /// Opens or closes tree row `at`, keeping the highlight on its entry: below the
    /// row it moves by the rows added or removed, and inside a branch that closes it
    /// moves to the closed folder, whose list is then read.
    fn toggle_at(&mut self, at: usize) -> Result<(), LibraryError> {
        let cursor = self.tree_cursor;
        let end = self.tree.subtree_end(at);
        let mut tree = self.tree.clone();
        tree.toggle(at);
        if cursor > at && cursor < end {
            self.list = List::open(&self.reader, tree.target(at), &self.query)?;
            self.tree_cursor = at;
        } else if cursor > at {
            self.tree_cursor = (cursor + tree.len()).saturating_sub(self.tree.len());
        }
        self.tree = tree;
        Ok(())
    }

    /// Reads the drives, the playlists and the highlighted list again, to see library and
    /// disk changes (a drive plugged in or out, new playlists, scanned tracks, files added
    /// to the folder). Open folders stay open, on the drives still there. The highlighted entry stays highlighted and so does the selected row,
    /// wherever it moved in the list (in a playlist, the occurrence of its track nearest
    /// the old cursor); if the row left the list, the cursor stays on the same row where
    /// it fits. If the entry is gone, the cursor stays at the same tree row.
    ///
    /// # Errors
    ///
    /// Any database error; the browser is then unchanged.
    pub fn refresh(&mut self) -> Result<BrowserView, LibraryError> {
        let mut tree = self.tree.clone();
        tree.set_drives((self.roots)());
        tree.drop_gone_folders();
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
                let list = List::open(&self.reader, tree.target(i), &self.query)?;
                let at = match self.list.selected() {
                    Some(row) => find_row(&self.reader, &list, row, self.list.cursor)?,
                    None => None,
                };
                (i, list, at.unwrap_or(self.list.cursor))
            }
            None => {
                let i = self.tree_cursor.min(tree.len() - 1);
                (i, List::open(&self.reader, tree.target(i), &self.query)?, 0)
            }
        };
        list.seek(&self.reader, list_cursor)?;
        self.all_selected = false;
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
            search: self.query.text.clone(),
            sort: self.query.sort.map(|s| (s.key, s.descending)),
            seq: self.seq,
            playlist: match self.list.items {
                Items::Playlist { id, .. } => Some(id),
                _ => None,
            },
            all_selected: self.all_selected,
        }
    }

    /// The library track at the list cursor (what LOAD loads), whichever pane has focus.
    /// `None` when the list is empty or the file is not in the library.
    #[must_use]
    pub fn selected_track(&self) -> Option<TrackRow> {
        self.list.selected().and_then(|r| r.track.clone())
    }

    /// The list shown now.
    #[must_use]
    pub fn identity(&self) -> ListIdentity {
        ListIdentity {
            tree_cursor: self.tree_cursor,
            seq: self.seq,
            count: self.list.count,
            playlist: match self.list.items {
                Items::Playlist { id, .. } => Some(id),
                _ => None,
            },
        }
    }

    /// Every row of the list is selected ([`BrowserEdit::SelectAll`]) and nothing has
    /// changed the pick or the list since.
    #[must_use]
    pub fn all_selected(&self) -> bool {
        self.all_selected
    }

    /// The library ids of the list's tracks, in list order: what a whole-list selection
    /// ([`BrowserEdit::SelectAll`]) stands for. Rows the library does not have (files of a
    /// folder) are left out. Built when asked, not kept: a long playlist reads its rows
    /// from the database here.
    ///
    /// # Errors
    ///
    /// Any database error.
    pub fn list_track_ids(&self) -> Result<Vec<i64>, LibraryError> {
        Ok(match &self.list.items {
            Items::None => Vec::new(),
            Items::Files { files, tracks, .. } => files
                .iter()
                .filter_map(|path| tracks.get(path).map(|t| t.id))
                .collect(),
            Items::Playlist { id, query } => self
                .reader
                .playlist_page(*id, query.text.as_deref(), query.sort, 0, self.list.count)?
                .into_iter()
                .map(|(_, track, _)| track.id)
                .collect(),
        })
    }

    /// Every file of a folder's list, in list order (its search and sort applied): what a
    /// whole-list selection of a folder stands for. Empty for a playlist's list.
    #[must_use]
    pub fn list_files(&self) -> Vec<PathBuf> {
        match &self.list.items {
            Items::Files { files, .. } => files.clone(),
            Items::None | Items::Playlist { .. } => Vec::new(),
        }
    }

    /// Every entry of a playlist's list as (position in the playlist, track), in list order
    /// (its search and sort applied): what a whole-list selection of a playlist stands for.
    /// Empty for a folder's list.
    ///
    /// # Errors
    ///
    /// Any database error.
    pub fn list_entries(&self) -> Result<Vec<(usize, i64)>, LibraryError> {
        Ok(match &self.list.items {
            Items::Playlist { id, query } => self
                .reader
                .playlist_page(*id, query.text.as_deref(), query.sort, 0, self.list.count)?
                .into_iter()
                .map(|(_, track, position)| (position, track.id))
                .collect(),
            Items::None | Items::Files { .. } => Vec::new(),
        })
    }

    /// The name the tree gives the drive `path` is on, if it shows that drive.
    #[must_use]
    pub fn drive_name(&self, path: &Path) -> Option<String> {
        self.tree.drive_of(path).map(str::to_owned)
    }

    /// The highlighted folder, when its list has audio files the library doesn't have.
    #[must_use]
    pub fn folder_with_unknown_files(&self) -> Option<PathBuf> {
        let Target::Dir(dir) = self.tree.target(self.tree_cursor) else {
            return None;
        };
        match &self.list.items {
            Items::Files { unknown: true, .. } => Some(dir.clone()),
            _ => None,
        }
    }

    fn move_tree(&mut self, steps: i32) -> Result<(), LibraryError> {
        let to = step(self.tree_cursor, steps, self.tree.len());
        if to != self.tree_cursor {
            self.list = List::open(&self.reader, self.tree.target(to), &self.query)?;
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

/// `dir`'s audio files that match `query`, in its order, and the library's tracks among
/// all of them. A folder that can't be read has no files. Paths are canonical, as the
/// library stores them.
fn folder_items(reader: &Reader, dir: &Path, query: &Query) -> Result<Items, LibraryError> {
    let dir = normalize_path(dir).unwrap_or_else(|_| dir.to_owned());
    let mut files = audio_files(&dir).unwrap_or_default();
    let filter = TrackFilter {
        folder: Some(dir),
        folder_only: true,
        ..TrackFilter::default()
    };
    let tracks: HashMap<PathBuf, TrackRow> = if files.is_empty() {
        HashMap::new()
    } else {
        utf8_only(reader.track_files(&filter))?
            .into_iter()
            .collect()
    };
    let unknown = files.iter().any(|f| !tracks.contains_key(f));
    if files.is_empty() || *query == Query::default() {
        return Ok(Items::Files {
            files,
            tracks,
            unknown,
        });
    }
    if let Some(text) = &query.text {
        let filter = TrackFilter {
            text: Some(text.clone()),
            ..filter.clone()
        };
        let matching: HashSet<PathBuf> = utf8_only(reader.track_files(&filter))?
            .into_iter()
            .map(|(path, _)| path)
            .collect();
        // SQLite's LIKE ignores ASCII case only; so does this.
        let needle = text.to_ascii_lowercase();
        files.retain(|f| {
            matching.contains(f)
                || !tracks.contains_key(f) && file_name(f).to_ascii_lowercase().contains(&needle)
        });
    }
    if let Some(sort) = query.sort {
        let order = utf8_only(reader.track_paths_sorted(&filter, sort))?;
        let rank: HashMap<&Path, usize> = order
            .iter()
            .enumerate()
            .map(|(at, path)| (path.as_path(), at))
            .collect();
        // Files outside the library count as unknown values: first ascending, last
        // descending. The sort is stable, so they keep their name order.
        files.sort_by_key(|f| match rank.get(f.as_path()) {
            Some(&at) => (!sort.descending, at),
            None => (sort.descending, 0),
        });
    }
    Ok(Items::Files {
        files,
        tracks,
        unknown,
    })
}

/// `result`, with nothing for a folder whose path is not UTF-8: the library holds only
/// UTF-8 paths.
fn utf8_only<T>(result: Result<Vec<T>, LibraryError>) -> Result<Vec<T>, LibraryError> {
    match result {
        Err(LibraryError::NonUtf8Path(_)) => Ok(Vec::new()),
        other => other,
    }
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
        Items::Files { files, tracks, .. } => files
            .iter()
            .skip(offset)
            .take(BROWSER_PAGE_ROWS)
            .map(|path| ListRow {
                path: path.clone(),
                track: tracks.get(path).cloned(),
                file_name: file_name(path),
                position: None,
                cover_file: None,
            })
            .collect(),
        Items::Playlist { id, query } => reader
            .playlist_page(
                *id,
                query.text.as_deref(),
                query.sort,
                offset,
                BROWSER_PAGE_ROWS,
            )?
            .into_iter()
            .map(|(path, track, position)| ListRow {
                file_name: file_name(&path),
                path,
                track: Some(track),
                position: Some(position),
                cover_file: None,
            })
            .collect(),
    })
}

/// Where `row` is in `list`, if it is there: a folder's file by path; in a playlist
/// (where a track may appear more than once), its track's occurrence nearest `near`,
/// found with one query.
fn find_row(
    reader: &Reader,
    list: &List,
    row: &ListRow,
    near: usize,
) -> Result<Option<usize>, LibraryError> {
    match &list.items {
        Items::None => Ok(None),
        Items::Files { files, .. } => Ok(files.iter().position(|p| *p == row.path)),
        Items::Playlist { id, query } => {
            let Some(track) = row.track.as_ref().map(|t| t.id) else {
                return Ok(None);
            };
            let ranks = reader.playlist_ranks(*id, query.text.as_deref(), query.sort, track)?;
            // The first of two equally near keeps the earlier one, as a scan from the top.
            Ok(ranks.into_iter().min_by_key(|at| at.abs_diff(near)))
        }
    }
}
