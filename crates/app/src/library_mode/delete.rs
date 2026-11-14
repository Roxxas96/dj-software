//! Deleting from the browser, always after the window asked: entries out of a playlist
//! and whole playlists through the playlist edits, files and folders to the system's
//! Trash, never deleted outright.
//!
//! Every file or folder is checked by [`check_delete`] when the window asks and again on
//! the worker just before it moves: only what lies inside a library folder (compared as
//! canonical paths, so a link cannot point a delete outside), never a library folder
//! itself or a folder holding one, never a folder holding Open Jockey's own data, and
//! nothing loaded on a deck. Folders and files are also compared by identity (device and
//! inode, volume and file index), so a name typed in another case on a drive that ignores
//! case is still recognised. A failed Trash move
//! deletes nothing more: there is no fallback to deleting for good. The library tracks
//! whose files went are marked missing, as a rescan would; their hotcues stay.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::{Path, PathBuf};

use same_file::Handle;

use engine::DeckId;
use library::{Browser, LibraryError, Reader, WriteOp, Writer};
use ui::DeleteTarget;

use super::playlist_edits;
use super::worker::PLAYLIST_CHANGED;
use crate::format::safe;

/// The event when a whole-list delete finds the list changed since the window selected it.
pub const LIST_CHANGED: &str = "The list changed: nothing was deleted";

/// How many tracks one [`WriteOp::MarkMissing`] flags.
const MISSING_BATCH: usize = 500;

/// Moves files and folders to the Trash.
pub trait Trash: Send + Sync {
    /// Moves each of `paths` to the Trash, in order, stopping at the first that cannot go.
    ///
    /// # Errors
    ///
    /// Why a path could not go; the ones before it went.
    fn trash(&self, paths: &[PathBuf]) -> Result<(), TrashError>;
}

/// Why a Trash move failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrashError {
    /// The drive has no Trash or Recycle Bin (some USB sticks and network drives), as the
    /// system said it.
    NoTrash(String),
    /// Anything else (a permission, a file in use), as the system said it.
    Other(String),
}

impl TrashError {
    /// The error the system gave moving `path`: [`TrashError::NoTrash`] when its code says
    /// the drive has no Trash or Recycle Bin. Only the code (or, where the system gives no
    /// code, its own words with every part of `path` taken out) is looked at, so a file
    /// whose name reads like such an error is not taken for one.
    pub fn from_trash(error: &trash::Error, path: &Path) -> Self {
        let description = error.to_string();
        let no_trash = match error {
            // Windows: the copy engine's "the Recycle Bin is not available on this drive"
            // (COPYENGINE_E_RECYCLE_FORCE_NUKE) and "could not be found"
            // (COPYENGINE_E_RECYCLE_BIN_NOT_FOUND).
            trash::Error::Os { code, .. } => {
                matches!(*code as u32, 0x8027_0036 | 0x8027_003A)
            }
            // macOS: `trashItemAtURL` gives NSFeatureUnsupportedError (3328) on a volume
            // without a Trash, in words only. freedesktop: no home trash nor a usable one
            // on the mount, in fixed words.
            trash::Error::Unknown { description } => no_trash_words(description, path),
            _ => false,
        };
        if no_trash {
            Self::NoTrash(description)
        } else {
            Self::Other(description)
        }
    }

    /// What the system said.
    pub fn reason(&self) -> &str {
        match self {
            Self::NoTrash(reason) | Self::Other(reason) => reason,
        }
    }
}

/// Whether the system's own words in `description` say the drive has no Trash. Only the
/// part after the crate's "failed:" is the system's (the crate puts the path before it),
/// and every part of `path` is taken out of it first.
fn no_trash_words(description: &str, path: &Path) -> bool {
    const NO_TRASH: [&str; 5] = [
        "3328",
        "feature unsupported",
        "not supported",
        "valid 'home trash'",
        "no trash",
    ];
    let system = description
        .rsplit_once("failed:")
        .map_or(description, |(_, words)| words);
    let mut words = system.to_lowercase();
    let mut parts: Vec<String> = path
        .components()
        .map(|part| part.as_os_str().to_string_lossy().to_lowercase())
        .filter(|part| part.len() > 1)
        .collect();
    parts.push(path.to_string_lossy().to_lowercase());
    // Longest first, so a whole path goes before its parts.
    parts.sort_by_key(|part| std::cmp::Reverse(part.len()));
    for part in &parts {
        words = words.replace(part.as_str(), " ");
    }
    NO_TRASH.iter().any(|sign| words.contains(sign))
}

/// The system's Trash (the Recycle Bin on Windows), through the `trash` crate.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemTrash;

impl Trash for SystemTrash {
    #[cfg(not(test))]
    fn trash(&self, paths: &[PathBuf]) -> Result<(), TrashError> {
        #[cfg_attr(not(target_os = "macos"), allow(unused_mut))]
        let mut context = trash::TrashContext::default();
        // Through the file manager, not by scripting the Finder: no permission prompt and
        // no sound. The item can still be dragged back out of the Trash.
        #[cfg(target_os = "macos")]
        trash::macos::TrashContextExtMacos::set_delete_method(
            &mut context,
            trash::macos::DeleteMethod::NsFileManager,
        );
        for path in paths {
            context
                .delete(path)
                .map_err(|e| TrashError::from_trash(&e, path))?;
        }
        Ok(())
    }

    /// Tests never reach the real Trash: they use [`FakeTrash`].
    #[cfg(test)]
    fn trash(&self, _: &[PathBuf]) -> Result<(), TrashError> {
        panic!("a test reached the system Trash");
    }
}

/// What a check found would go to the Trash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// The paths to move: each in its canonical folder, under its own name, so a link
    /// moves itself, never what it points to.
    pub paths: Vec<PathBuf>,
    /// How many files go: the files themselves, or every file in the folder.
    pub files: usize,
    what: What,
}

/// What a [`Plan`] moves, for its question.
#[derive(Debug, Clone, PartialEq, Eq)]
enum What {
    Files,
    /// A folder, by name.
    Folder(String),
    /// A link to a folder, by name: only the link goes.
    Link(String),
}

impl Plan {
    /// The question the window asks before the move.
    pub fn question(&self) -> String {
        match (&self.what, self.paths.as_slice()) {
            (What::Folder(name), _) => format!(
                "Move the folder {name} and its {} to the Trash?",
                count(self.files, "file")
            ),
            (What::Link(name), _) => {
                format!("Move the link {name} to the Trash? The folder it points to stays.")
            }
            (What::Files, [one]) => format!("Move {} to the Trash?", name_of(one)),
            (What::Files, _) => format!("Move {} to the Trash?", count(self.files, "file")),
        }
    }
}

/// Why something cannot go to the Trash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// The file `.0` is loaded on deck `.1`.
    Loaded(PathBuf, DeckId),
    /// The folder `.0` is a library folder.
    IsRoot(PathBuf),
    /// The folder `.0` holds a library folder.
    HoldsRoot(PathBuf),
    /// The folder `.0` is or holds the folder of the library's database or of the
    /// configuration.
    HoldsOwn(PathBuf),
    /// `.0` is inside the folder of the library's database or of the configuration.
    InOwn(PathBuf),
    /// `.0`, or a place it is compared with, cannot be identified on disk: whether it is
    /// safe to move cannot be said.
    Unchecked(PathBuf),
    /// `.0` is not inside any library folder.
    Outside(PathBuf),
    /// `.0` is not on disk.
    Gone(PathBuf),
    /// `.0`, named as a file, is a folder.
    Folder(PathBuf),
    /// There is nothing to move.
    Nothing,
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Loaded(file, deck) => write!(
                f,
                "{} is loaded on deck {}: unload it first.",
                name_of(file),
                deck.index() + 1
            ),
            Self::IsRoot(folder) => write!(
                f,
                "{} is a library folder: remove it from the library first.",
                name_of(folder)
            ),
            Self::HoldsRoot(folder) => write!(
                f,
                "{} holds a library folder: remove it from the library first.",
                name_of(folder)
            ),
            Self::HoldsOwn(folder) => write!(
                f,
                "{} holds Open Jockey's library: it can't be deleted from here.",
                name_of(folder)
            ),
            Self::InOwn(path) => write!(
                f,
                "{} is part of Open Jockey's library: it can't be deleted from here.",
                name_of(path)
            ),
            Self::Unchecked(path) => {
                write!(
                    f,
                    "{} can't be checked: nothing was deleted.",
                    name_of(path)
                )
            }
            Self::Outside(path) => {
                write!(
                    f,
                    "{} is outside the library folders.",
                    safe(path.display())
                )
            }
            Self::Gone(path) => write!(f, "{} is not on disk any more.", name_of(path)),
            Self::Folder(path) => {
                write!(f, "{} is a folder: delete it from the tree.", name_of(path))
            }
            Self::Nothing => write!(f, "There is nothing to delete."),
        }
    }
}

/// What goes to the Trash, as the library or the window named it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Doomed {
    /// These files.
    Files(Vec<PathBuf>),
    /// This folder and everything in it.
    Folder(PathBuf),
}

/// Whether moving `doomed` to the Trash is allowed with `folders` the library folders,
/// `loaded` the files on the decks and `own` the folders of Open Jockey's own data (the
/// library database's and the configuration's), and if so what moves. Reads the disk (to
/// resolve the paths, compare them by identity and count a folder's files) but changes
/// nothing.
///
/// # Errors
///
/// Why it is refused: a path outside the library folders, a library folder or a folder
/// holding one, a folder holding Open Jockey's own data, a loaded file or a folder holding
/// one, a path no longer on disk.
pub fn check_delete(
    doomed: &Doomed,
    folders: &[PathBuf],
    loaded: &[(DeckId, PathBuf)],
    own: &[PathBuf],
) -> Result<Plan, Refusal> {
    let roots: Vec<PathBuf> = folders.iter().map(|f| canonical(f)).collect();
    let own: Vec<PathBuf> = own.iter().map(|f| canonical(f)).collect();
    let loaded: Vec<(DeckId, PathBuf)> = loaded
        .iter()
        .map(|(deck, path)| (*deck, canonical(path)))
        .collect();
    // The same places by identity, opened once: the library folders, the folders holding
    // one, Open Jockey's own folders and the folders holding them, and each loaded file
    // with the folders holding it. A delete is only checked with all of them: one that
    // cannot be identified refuses every delete rather than fall back to names. One that
    // is not on disk at all (a library folder's drive unplugged, a loaded file gone) is
    // left out: no target can be it or hold it.
    let root_ids = identities(roots.iter().map(PathBuf::as_path));
    let above_roots = identities(roots.iter().flat_map(|r| r.ancestors().skip(1)));
    let own_self = identities(own.iter().map(PathBuf::as_path));
    let own_ids = identities(own.iter().flat_map(|o| o.ancestors()));
    let loaded_ids: Option<Vec<Loaded<'_>>> = loaded
        .iter()
        .filter_map(|(deck, file)| {
            let id = match identity(file) {
                Ok(Some(id)) => id,
                Ok(None) => return None,
                Err(()) => return Some(None),
            };
            Some(Some(Loaded {
                deck: *deck,
                file,
                id,
                within: identities(file.ancestors().skip(1))?,
            }))
        })
        .collect();
    let ids = (|| Some((root_ids?, above_roots?, own_self?, own_ids?, loaded_ids?)))();
    let inside = |path: &Path| roots.iter().any(|r| path.starts_with(r) && path != r);
    // Per folder, whether it is inside one of Open Jockey's own, by identity; `None` when
    // a folder on the way up cannot be identified.
    let in_own_folders: RefCell<HashMap<PathBuf, Option<bool>>> = RefCell::default();
    let in_own = |folder: &Path, own_self: &HashSet<Handle>| -> Option<bool> {
        if let Some(known) = in_own_folders.borrow().get(folder) {
            return *known;
        }
        let found = folder
            .ancestors()
            .map(|up| Handle::from_path(up).ok().map(|id| own_self.contains(&id)))
            .try_fold(false, |found, up| Some(found || up?));
        in_own_folders.borrow_mut().insert(folder.to_owned(), found);
        found
    };
    // Where `path` is (what moves), what it resolves to, both inside a library folder so a
    // link can neither be moved from outside nor point a delete outside, and what it is on
    // disk: nothing for a link, which moves alone (its own place is checked by name).
    let place = |path: &Path| -> Result<Place, Refusal> {
        let at = location(path).ok_or_else(|| Refusal::Gone(path.to_owned()))?;
        let resolved = dunce::canonicalize(&at).map_err(|_| Refusal::Gone(path.to_owned()))?;
        let link = std::fs::symlink_metadata(&at).is_ok_and(|m| m.file_type().is_symlink());
        let unchecked = || Refusal::Unchecked(at.clone());
        let (root_ids, above_roots, own_self, own_ids, _) = ids.as_ref().ok_or_else(unchecked)?;
        let id = if link {
            None
        } else {
            Some(Handle::from_path(&at).map_err(|_| unchecked())?)
        };
        let known = |ids: &HashSet<Handle>| id.as_ref().is_some_and(|id| ids.contains(id));
        if roots.contains(&at) || known(root_ids) {
            return Err(Refusal::IsRoot(at));
        }
        if roots.iter().any(|r| r.starts_with(&at)) || known(above_roots) {
            return Err(Refusal::HoldsRoot(at));
        }
        if own.iter().any(|o| o.starts_with(&at)) || known(own_ids) {
            return Err(Refusal::HoldsOwn(at));
        }
        let folder = at.parent().ok_or_else(unchecked)?;
        let within = in_own(folder, own_self).ok_or_else(unchecked)?;
        if own.iter().any(|o| at.starts_with(o)) || within {
            return Err(Refusal::InOwn(at));
        }
        if !inside(&at) || !inside(&resolved) {
            return Err(Refusal::Outside(path.to_owned()));
        }
        Ok(Place { at, link, id })
    };
    let no_loaded = Vec::new();
    let loaded_ids = ids.as_ref().map_or(&no_loaded, |ids| &ids.4);
    match doomed {
        Doomed::Files(files) => {
            let mut paths: Vec<PathBuf> = Vec::with_capacity(files.len());
            for file in files {
                let Place { at, id, .. } = place(file)?;
                if std::fs::symlink_metadata(&at).is_ok_and(|m| m.is_dir()) {
                    return Err(Refusal::Folder(at));
                }
                let is_loaded = |l: &&Loaded<'_>| *l.file == at || id.as_ref() == Some(&l.id);
                if let Some(l) = loaded_ids.iter().find(is_loaded) {
                    return Err(Refusal::Loaded(at, l.deck));
                }
                if !paths.contains(&at) {
                    paths.push(at);
                }
            }
            if paths.is_empty() {
                return Err(Refusal::Nothing);
            }
            Ok(Plan {
                files: paths.len(),
                paths,
                what: What::Files,
            })
        }
        Doomed::Folder(folder) => {
            let Place { at, link, id } = place(folder)?;
            let holds = |l: &&Loaded<'_>| {
                l.file.starts_with(&at) || id.as_ref().is_some_and(|id| l.within.contains(id))
            };
            if let Some(l) = loaded_ids.iter().find(holds) {
                return Err(Refusal::Loaded(l.file.clone(), l.deck));
            }
            let (files, what) = if link {
                (1, What::Link(name_of(&at)))
            } else {
                (count_files(&at), What::Folder(name_of(&at)))
            };
            Ok(Plan {
                files,
                what,
                paths: vec![at],
            })
        }
    }
}

/// A path checked by [`check_delete`].
struct Place {
    /// Where it is: its folder resolved, then its own name.
    at: PathBuf,
    /// It is a link (which moves alone).
    link: bool,
    /// What it is on disk; `None` for a link.
    id: Option<Handle>,
}

/// A file loaded on a deck, for [`check_delete`].
struct Loaded<'a> {
    deck: DeckId,
    /// Resolved.
    file: &'a PathBuf,
    /// What it is on disk.
    id: Handle,
    /// What each folder holding it is on disk.
    within: HashSet<Handle>,
}

/// The identities of `paths` on disk, each once, leaving out those not on disk; `None`
/// when one is on disk but cannot be opened.
fn identities<'a>(mut paths: impl Iterator<Item = &'a Path>) -> Option<HashSet<Handle>> {
    paths.try_fold(HashSet::new(), |mut ids, path| {
        ids.extend(identity(path).ok()?);
        Some(ids)
    })
}

/// What `path` is on disk: `Ok(None)` when nothing is there, `Err` when it cannot be opened.
fn identity(path: &Path) -> Result<Option<Handle>, ()> {
    match Handle::from_path(path) {
        Ok(id) => Ok(Some(id)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(()),
    }
}

/// The folders Open Jockey keeps its own data in, which a delete never takes: the library
/// database's (with its backups and covers) and the configuration's.
pub(super) fn own_folders(db: &Path) -> Vec<PathBuf> {
    db.parent()
        .map(Path::to_owned)
        .into_iter()
        .chain(controller::builtin::config_dir())
        .collect()
}

/// Where `path` is on disk: its folder with links resolved, then its own name, which may
/// be a link. `None` unless it is an absolute path without `.` or `..` that is there (a
/// link counts, even one pointing nowhere).
fn location(path: &Path) -> Option<PathBuf> {
    use std::path::Component;
    let plain = path
        .components()
        .all(|c| !matches!(c, Component::CurDir | Component::ParentDir));
    if !path.is_absolute() || !plain {
        return None;
    }
    let at = dunce::canonicalize(path.parent()?)
        .ok()?
        .join(path.file_name()?);
    std::fs::symlink_metadata(&at).ok()?;
    Some(at)
}

/// `path` with links resolved, or as given when it cannot be (it is gone).
fn canonical(path: &Path) -> PathBuf {
    dunce::canonicalize(path).unwrap_or_else(|_| path.to_owned())
}

/// Every file under `dir`, hidden ones and links too, not following links to folders.
/// What cannot be read counts as empty.
fn count_files(dir: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .map(|entry| match entry.file_type() {
            Ok(kind) if kind.is_dir() => count_files(&entry.path()),
            _ => 1,
        })
        .sum()
}

/// `path`'s last part, for the window.
fn name_of(path: &Path) -> String {
    safe(
        path.file_name()
            .unwrap_or(path.as_os_str())
            .to_string_lossy(),
    )
}

/// "1 file", "3 files".
fn count(n: usize, what: &str) -> String {
    if n == 1 {
        format!("1 {what}")
    } else {
        format!("{n} {what}s")
    }
}

/// True only when `path` is known not to exist.
fn is_gone(path: &Path) -> bool {
    matches!(std::fs::symlink_metadata(path), Err(e) if e.kind() == std::io::ErrorKind::NotFound)
}

/// What a delete names, read from the library and the browser.
enum Resolved {
    /// Entries of the playlist called `name`.
    Entries {
        playlist: i64,
        name: String,
        entries: Vec<(usize, i64)>,
    },
    /// The playlist `id`, called `name`.
    Playlist { id: i64, name: String },
    /// Files or a folder, for the Trash.
    Disk(Doomed),
}

/// Reads what `target` names. The window's whole-list selections must still be the
/// browser's.
fn resolve(target: &DeleteTarget, browser: &Browser, reader: &Reader) -> Result<Resolved, String> {
    let warn = |e: LibraryError| format!("warning: library browser: {}", safe(e));
    let name = |id: i64| -> Result<String, String> {
        let playlists = reader.playlists().map_err(warn)?;
        playlists
            .into_iter()
            .find(|p| p.id == id)
            .map(|p| p.name)
            .ok_or_else(|| PLAYLIST_CHANGED.to_owned())
    };
    let whole = |list: &library::ListIdentity, playlist: bool| {
        browser.identity() == *list && browser.all_selected() && list.playlist.is_some() == playlist
    };
    Ok(match target {
        DeleteTarget::Entries { playlist, entries } => Resolved::Entries {
            playlist: *playlist,
            name: name(*playlist)?,
            entries: entries.clone(),
        },
        DeleteTarget::AllEntries { list } => {
            let playlist = list.playlist.filter(|_| whole(list, true));
            let Some(playlist) = playlist else {
                return Err(LIST_CHANGED.to_owned());
            };
            Resolved::Entries {
                playlist,
                name: name(playlist)?,
                entries: browser.list_entries().map_err(warn)?,
            }
        }
        DeleteTarget::Files { paths } => Resolved::Disk(Doomed::Files(paths.clone())),
        DeleteTarget::AllFiles { list } => {
            if !whole(list, false) {
                return Err(LIST_CHANGED.to_owned());
            }
            Resolved::Disk(Doomed::Files(browser.list_files()))
        }
        DeleteTarget::Playlist { id } => Resolved::Playlist {
            id: *id,
            name: name(*id)?,
        },
        DeleteTarget::Folder { path } => Resolved::Disk(Doomed::Folder(path.clone())),
    })
}

/// The question to ask before deleting `target`, or why it cannot be deleted, with
/// `loaded` the files on the decks and `own` Open Jockey's own folders.
pub(super) fn ask(
    target: &DeleteTarget,
    browser: &Browser,
    reader: &Reader,
    loaded: &[(DeckId, PathBuf)],
    own: &[PathBuf],
) -> Result<String, String> {
    match resolve(target, browser, reader)? {
        Resolved::Entries { name, entries, .. } => Ok(format!(
            "Remove {} from {}? The files stay.",
            count(entries.len(), "track"),
            safe(name)
        )),
        Resolved::Playlist { name, .. } => Ok(format!(
            "Delete the playlist {}? Its tracks stay in the library.",
            safe(name)
        )),
        Resolved::Disk(doomed) => {
            let folders = reader.folders().map_err(safe)?;
            check_delete(&doomed, &folders, loaded, own)
                .map(|plan| plan.question())
                .map_err(|refusal| refusal.to_string())
        }
    }
}

/// What a delete did, for the worker to report.
pub(super) struct Done {
    /// The library changed: the browser is to be read again.
    pub refresh: bool,
    /// The events to show, in order, after the browser is read again.
    pub messages: Vec<String>,
}

/// Where the worker deletes: the library, the browser (for whole lists and drive names)
/// and the Trash, never in Open Jockey's own folders (`own`).
pub(super) struct Deleter<'a> {
    pub reader: &'a Reader,
    pub writer: &'a Writer,
    pub browser: &'a Browser,
    pub trash: &'a dyn Trash,
    pub own: &'a [PathBuf],
}

impl Deleter<'_> {
    /// Deletes `target`, checked again now, with `loaded` the files on the decks.
    pub(super) fn delete(&self, target: &DeleteTarget, loaded: &[(DeckId, PathBuf)]) -> Done {
        let refused = |message: String| Done {
            refresh: false,
            messages: vec![message],
        };
        let resolved = match resolve(target, self.browser, self.reader) {
            Ok(resolved) => resolved,
            Err(message) => return refused(message),
        };
        let op = match resolved {
            Resolved::Entries {
                playlist, entries, ..
            } => playlist_edits::remove(playlist, &entries),
            Resolved::Playlist { id, .. } => WriteOp::DeletePlaylist(id),
            Resolved::Disk(doomed) => {
                let folders = match self.reader.folders() {
                    Ok(folders) => folders,
                    Err(e) => return refused(format!("warning: {}", safe(e))),
                };
                return match check_delete(&doomed, &folders, loaded, self.own) {
                    Ok(plan) => self.to_trash(&plan),
                    Err(refusal) => refused(refusal.to_string()),
                };
            }
        };
        let message = match self.writer.submit_wait(op) {
            Ok(_) => None,
            Err(LibraryError::PlaylistChanged(_) | LibraryError::NoSuchPlaylist(_)) => {
                Some(PLAYLIST_CHANGED.to_owned())
            }
            Err(e) => Some(format!(
                "warning: could not change the playlist: {}",
                safe(e)
            )),
        };
        Done {
            refresh: true,
            messages: message.into_iter().collect(),
        }
    }

    /// Moves what `plan` holds to the Trash, then marks the library tracks whose files
    /// went missing. A failed move marks only what did go.
    fn to_trash(&self, plan: &Plan) -> Done {
        let failed = self.trash.trash(&plan.paths).err();
        let gone: Vec<&PathBuf> = match failed {
            None => plan.paths.iter().collect(),
            Some(_) => plan.paths.iter().filter(|p| is_gone(p)).collect(),
        };
        let marked = self.mark_missing(&gone);
        let mut warning = None;
        let message = match &failed {
            None => format!("Moved {} to the Trash", count(plan.files, "file")),
            Some(error) => {
                let stuck = plan.paths.iter().find(|p| !is_gone(p));
                let stuck = stuck.unwrap_or(&plan.paths[0]);
                warning = Some(format!(
                    "warning: could not move {} to the Trash: {}",
                    safe(stuck.display()),
                    safe(error.reason())
                ));
                let why = match error {
                    TrashError::NoTrash(_) => format!("{} has no Trash", self.drive(stuck)),
                    TrashError::Other(reason) => format!(
                        "Could not move {} to the Trash ({})",
                        name_of(stuck),
                        safe(reason)
                    ),
                };
                if gone.is_empty() {
                    format!("{why}: nothing was deleted")
                } else {
                    format!(
                        "{why}: moved {} to the Trash, the rest stay",
                        count(gone.len(), "file")
                    )
                }
            }
        };
        let message = match marked {
            Ok(()) => message,
            Err(e) => format!(
                "{message}; warning: could not mark their tracks missing: {}",
                safe(e)
            ),
        };
        Done {
            refresh: !gone.is_empty(),
            messages: std::iter::once(message).chain(warning).collect(),
        }
    }

    /// Marks the library tracks under `paths` missing, as a rescan would.
    fn mark_missing(&self, paths: &[&PathBuf]) -> Result<(), LibraryError> {
        let mut ids = Vec::new();
        for path in paths {
            ids.extend(self.reader.tracks_under(path)?);
        }
        ids.sort_unstable();
        ids.dedup();
        for batch in ids.chunks(MISSING_BATCH) {
            self.writer
                .submit_wait(WriteOp::MarkMissing(batch.to_vec()))?;
        }
        Ok(())
    }

    /// The name of the drive `path` is on, as the browser shows it.
    fn drive(&self, path: &Path) -> String {
        self.browser.drive_name(path).map_or_else(
            || safe(path.ancestors().last().unwrap_or(path).display()),
            safe,
        )
    }
}

/// A Trash for tests: moves paths into a folder of its own, and fails when told to.
#[cfg(test)]
pub struct FakeTrash {
    bin: PathBuf,
    moved: std::sync::Mutex<Vec<PathBuf>>,
    fail: std::sync::Mutex<Option<TrashError>>,
}

#[cfg(test)]
impl FakeTrash {
    /// A Trash that moves what it is given into `bin` (made on the first move).
    pub fn new(bin: &Path) -> Self {
        Self {
            bin: bin.to_owned(),
            moved: std::sync::Mutex::default(),
            fail: std::sync::Mutex::default(),
        }
    }

    /// From now on every move fails with `error`, moving nothing.
    pub fn fail_with(&self, error: TrashError) {
        *self.fail.lock().unwrap() = Some(error);
    }

    /// From now on every move fails as on a drive without a Trash, moving nothing.
    pub fn fail(&self) {
        self.fail_with(TrashError::NoTrash("no Trash on this drive".into()));
    }

    /// What it moved, in order.
    pub fn moved(&self) -> Vec<PathBuf> {
        self.moved.lock().unwrap().clone()
    }
}

#[cfg(test)]
impl Trash for FakeTrash {
    fn trash(&self, paths: &[PathBuf]) -> Result<(), TrashError> {
        if let Some(error) = self.fail.lock().unwrap().clone() {
            return Err(error);
        }
        let mut moved = self.moved.lock().unwrap();
        let other = |e: std::io::Error| TrashError::Other(e.to_string());
        std::fs::create_dir_all(&self.bin).map_err(other)?;
        for path in paths {
            let to = self.bin.join(format!("{}-{}", moved.len(), name_of(path)));
            std::fs::rename(path, to).map_err(other)?;
            moved.push(path.clone());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::mpsc::{Receiver, Sender, channel};
    use std::time::Duration;

    use library::{BrowserEdit, BrowserView, Drive, Library, ScanProgress, WriteOutcome};

    use super::super::worker::{self, DeckFiles, Event, Job};
    use super::*;

    const FIXTURE: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../decode/tests/fixtures/sine440.wav"
    );

    /// A library in a temp folder whose one library folder, `music`, is the drive "Stick"
    /// the browser shows, with a worker that moves deletes to a [`FakeTrash`].
    struct Setup {
        dir: tempfile::TempDir,
        music: PathBuf,
        lib: Arc<Library>,
        trash: Arc<FakeTrash>,
        jobs: Sender<Job>,
        events: Receiver<Event>,
    }

    /// [`Setup`] with `files` (paths under `music`) made and scanned: `.wav` ones a short
    /// sine, the others a line of text.
    fn setup(files: &[&str]) -> Setup {
        let dir = tempfile::tempdir().unwrap();
        let music = dunce::canonicalize(dir.path()).unwrap().join("music");
        std::fs::create_dir_all(&music).unwrap();
        for file in files {
            let path = music.join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            if file.ends_with(".wav") {
                std::fs::copy(FIXTURE, &path).unwrap();
            } else {
                std::fs::write(&path, "not music").unwrap();
            }
        }
        // The database in a folder of its own, beside `music`: a delete never goes inside
        // it.
        std::fs::create_dir_all(dir.path().join("data")).unwrap();
        let lib = Library::open(&dir.path().join("data/library.sqlite3")).unwrap();
        library::add_folder(&lib, &music).unwrap();
        library::scan_folder(&lib, &music, &mut |_: ScanProgress| {}).unwrap();
        let lib = Arc::new(lib);
        let trash = Arc::new(FakeTrash::new(&dir.path().join("trash")));
        let drives = vec![Drive {
            name: "Stick".into(),
            path: music.clone(),
        }];
        let (tx, events) = channel();
        let (jobs, _worker) = worker::spawn(
            Arc::clone(&lib),
            tx,
            Some(drives),
            Arc::clone(&trash) as Arc<dyn Trash>,
        )
        .unwrap();
        let setup = Setup {
            dir,
            music,
            lib,
            trash,
            jobs,
            events,
        };
        setup.view();
        setup
    }

    impl Setup {
        /// The library track of `file` (under `music`).
        fn track(&self, file: &str) -> i64 {
            let reader = self.lib.reader().unwrap();
            // One join per part: "sub/a.wav" joined whole keeps its "/" on Windows.
            let path = file
                .split('/')
                .fold(self.music.clone(), |p, part| p.join(part));
            reader.tracks_under(&path).unwrap()[0]
        }

        fn missing(&self, track: i64) -> bool {
            let reader = self.lib.reader().unwrap();
            reader.track(track).unwrap().unwrap().row.missing
        }

        /// The next event `matches` accepts, skipping others, within 10 s.
        fn wait_for<T>(&self, mut matches: impl FnMut(Event) -> Option<T>) -> T {
            loop {
                let event = self
                    .events
                    .recv_timeout(Duration::from_secs(10))
                    .expect("no matching event within 10 s");
                if let Some(found) = matches(event) {
                    return found;
                }
            }
        }

        fn view(&self) -> BrowserView {
            self.wait_for(|e| match e {
                Event::Browser(view) => Some(view),
                _ => None,
            })
        }

        /// What the worker answers when asked about deleting `target` with `decks`.
        fn ask_with(&self, target: DeleteTarget, decks: DeckFiles) -> Result<String, String> {
            let seq = 7;
            self.jobs
                .send(Job::AskDelete { seq, target, decks })
                .unwrap();
            self.wait_for(|e| match e {
                Event::DeleteCheck(check) => {
                    assert_eq!(check.seq, seq);
                    Some(check.answer)
                }
                _ => None,
            })
        }

        fn ask(&self, target: DeleteTarget) -> Result<String, String> {
            self.ask_with(target, DeckFiles::default())
        }

        /// Deletes `target` with `decks` and returns the event that says how it went, and
        /// whether the browser was read again before it. Warnings (which follow the event)
        /// are skipped.
        fn delete_with(&self, target: DeleteTarget, decks: DeckFiles) -> (String, bool) {
            self.jobs.send(Job::Delete { target, decks }).unwrap();
            let mut reread = false;
            let message = self.wait_for(|e| match e {
                Event::Browser(_) => {
                    reread = true;
                    None
                }
                Event::Message(text) if !text.starts_with("warning: ") => Some(text),
                _ => None,
            });
            (message, reread)
        }

        /// The next warning event.
        fn warning(&self) -> String {
            self.wait_for(|e| match e {
                Event::Message(text) if text.starts_with("warning: ") => Some(text),
                _ => None,
            })
        }

        fn delete(&self, target: DeleteTarget) -> (String, bool) {
            self.delete_with(target, DeckFiles::default())
        }

        /// A playlist "Peak" holding `tracks`.
        fn peak(&self, tracks: &[i64]) -> i64 {
            let made = self.lib.writer().submit_wait(WriteOp::CreatePlaylistWith {
                name: "Peak".into(),
                tracks: tracks.to_vec(),
            });
            match made.unwrap() {
                WriteOutcome::Created { id } => id,
                other => panic!("unexpected outcome {other:?}"),
            }
        }
    }

    fn deck(n: usize) -> DeckId {
        DeckId::new(n - 1).unwrap()
    }

    #[test]
    fn a_file_goes_to_the_trash_and_is_marked_missing() {
        let s = setup(&["a.wav", "b.wav"]);
        let (a, b) = (s.track("a.wav"), s.track("b.wav"));
        s.lib
            .writer()
            .submit_wait(WriteOp::SetHotcue {
                track: a,
                slot: 1,
                frames: 100.0,
                loop_frames: None,
            })
            .unwrap();
        let target = DeleteTarget::Files {
            paths: vec![s.music.join("a.wav")],
        };
        assert_eq!(s.ask(target.clone()), Ok("Move a.wav to the Trash?".into()));
        // Asking moved nothing.
        assert!(s.music.join("a.wav").exists());
        assert!(s.trash.moved().is_empty());

        let (message, reread) = s.delete(target);
        assert_eq!(message, "Moved 1 file to the Trash");
        assert!(reread, "the browser is read again before the event");
        assert_eq!(s.trash.moved(), [s.music.join("a.wav")]);
        assert!(!s.music.join("a.wav").exists());
        assert!(s.missing(a));
        assert!(!s.missing(b));
        // Its hotcues stay, as for any missing file.
        let reader = s.lib.reader().unwrap();
        assert!(reader.hotcue_points(a).unwrap()[0].is_some());
    }

    #[test]
    fn entries_leave_the_playlist_and_the_files_stay() {
        let s = setup(&["a.wav", "b.wav"]);
        let (a, b) = (s.track("a.wav"), s.track("b.wav"));
        let playlist = s.peak(&[a, b]);
        let target = DeleteTarget::Entries {
            playlist,
            entries: vec![(0, a)],
        };
        assert_eq!(
            s.ask(target.clone()),
            Ok("Remove 1 track from Peak? The files stay.".into())
        );
        s.jobs
            .send(Job::Delete {
                target,
                decks: DeckFiles::default(),
            })
            .unwrap();
        s.view();
        let reader = s.lib.reader().unwrap();
        let left: Vec<i64> = reader
            .playlist_tracks(playlist, 0, 10)
            .unwrap()
            .iter()
            .map(|t| t.id)
            .collect();
        assert_eq!(left, [b]);
        assert!(s.music.join("a.wav").exists());
        assert!(s.trash.moved().is_empty());
        // Entries that moved since the window saw them: nothing is removed.
        let stale = DeleteTarget::Entries {
            playlist,
            entries: vec![(0, a)],
        };
        assert_eq!(s.delete(stale).0, PLAYLIST_CHANGED);
    }

    #[test]
    fn a_playlist_is_deleted_after_asking() {
        let s = setup(&["a.wav"]);
        let a = s.track("a.wav");
        let id = s.peak(&[a]);
        let target = DeleteTarget::Playlist { id };
        assert_eq!(
            s.ask(target.clone()),
            Ok("Delete the playlist Peak? Its tracks stay in the library.".into())
        );
        let reader = s.lib.reader().unwrap();
        assert_eq!(reader.playlists().unwrap().len(), 1);
        s.jobs
            .send(Job::Delete {
                target,
                decks: DeckFiles::default(),
            })
            .unwrap();
        let view = s.view();
        assert!(view.tree.iter().all(|row| row.playlist.is_none()));
        assert!(reader.playlists().unwrap().is_empty());
        // The track and its file stay.
        assert!(!s.missing(a));
        assert!(s.music.join("a.wav").exists());
        assert!(s.trash.moved().is_empty());
    }

    #[test]
    fn a_folder_goes_to_the_trash_with_its_count() {
        let files = [
            "sub/a.wav",
            "sub/b.wav",
            "sub/cover.jpg",
            "sub/.notes",
            "sub/deeper/readme.txt",
            "keep.wav",
        ];
        let s = setup(&files);
        let (a, b, keep) = (
            s.track("sub/a.wav"),
            s.track("sub/b.wav"),
            s.track("keep.wav"),
        );
        let target = DeleteTarget::Folder {
            path: s.music.join("sub"),
        };
        // Every file counts, music or not.
        assert_eq!(
            s.ask(target.clone()),
            Ok("Move the folder sub and its 5 files to the Trash?".into())
        );
        let (message, _) = s.delete(target);
        assert_eq!(message, "Moved 5 files to the Trash");
        assert_eq!(s.trash.moved(), [s.music.join("sub")]);
        assert!(!s.music.join("sub").exists());
        assert!(s.missing(a) && s.missing(b));
        assert!(!s.missing(keep));
    }

    #[test]
    fn a_folder_holding_a_loaded_track_cannot_be_deleted() {
        let s = setup(&["sub/a.wav", "sub/b.wav"]);
        let target = DeleteTarget::Folder {
            path: s.music.join("sub"),
        };
        let decks = DeckFiles {
            loaded: vec![(deck(2), s.music.join("sub/b.wav"))],
            taken: [0; 4],
        };
        let refused = "b.wav is loaded on deck 2: unload it first.";
        assert_eq!(
            s.ask_with(target.clone(), decks.clone()),
            Err(refused.into())
        );
        // Checked again when deleting: nothing moves.
        assert_eq!(
            s.delete_with(target.clone(), decks),
            (refused.into(), false)
        );
        assert!(s.trash.moved().is_empty());
        assert!(s.music.join("sub/b.wav").exists());

        // A load on its way to a deck counts as loaded, until the session takes it.
        let a = s.track("sub/a.wav");
        s.jobs
            .send(Job::LoadTrack {
                deck: deck(1),
                track: a,
                seq: 3,
                refuse_if_playing: false,
            })
            .unwrap();
        let flying = "a.wav is loaded on deck 1: unload it first.";
        assert_eq!(s.delete(target.clone()).0, flying);
        assert!(s.trash.moved().is_empty());
        // Taken (a later load replaced it on the deck): the folder can go.
        let decks = DeckFiles {
            loaded: Vec::new(),
            taken: [3, 0, 0, 0],
        };
        assert_eq!(s.delete_with(target, decks).0, "Moved 2 files to the Trash");

        // The same for a file on its own.
        let s = setup(&["a.wav"]);
        let a = s.track("a.wav");
        let decks = DeckFiles {
            loaded: vec![(deck(4), s.music.join("a.wav"))],
            taken: [0; 4],
        };
        let target = DeleteTarget::Files {
            paths: vec![s.music.join("a.wav")],
        };
        let refused = "a.wav is loaded on deck 4: unload it first.";
        assert_eq!(s.delete_with(target, decks), (refused.into(), false));
        assert!(s.music.join("a.wav").exists());
        assert!(!s.missing(a));
    }

    #[test]
    fn a_library_root_cannot_be_deleted() {
        let s = setup(&["a.wav"]);
        let ask = |path: &Path| {
            s.ask(DeleteTarget::Folder {
                path: path.to_owned(),
            })
        };
        assert_eq!(
            ask(&s.music),
            Err("music is a library folder: remove it from the library first.".into())
        );
        let above = dunce::canonicalize(s.dir.path()).unwrap();
        let name = name_of(&above);
        assert_eq!(
            ask(&above),
            Err(format!(
                "{name} holds a library folder: remove it from the library first."
            ))
        );
        // Also when deleting, and nothing moves.
        let (message, _) = s.delete(DeleteTarget::Folder {
            path: s.music.clone(),
        });
        assert_eq!(
            message,
            "music is a library folder: remove it from the library first."
        );
        assert!(s.trash.moved().is_empty());
        assert!(s.music.join("a.wav").exists());
        // A folder outside every library folder.
        let elsewhere = s.dir.path().join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        assert_eq!(
            ask(&elsewhere),
            Err(format!(
                "{} is outside the library folders.",
                elsewhere.display()
            ))
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_link_out_of_the_library_is_refused() {
        let s = setup(&["a.wav"]);
        let outside = s.dir.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::copy(FIXTURE, outside.join("x.wav")).unwrap();
        let link = s.music.join("link");
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        let file_link = s.music.join("x.wav");
        std::os::unix::fs::symlink(outside.join("x.wav"), &file_link).unwrap();
        let refused = |path: &Path| {
            Err(format!(
                "{} is outside the library folders.",
                path.display()
            ))
        };
        let folder = DeleteTarget::Folder { path: link.clone() };
        assert_eq!(s.ask(folder.clone()), refused(&link));
        assert_eq!(s.delete(folder).0, refused(&link).unwrap_err());
        // A file through a link, as the window could name it in a folder's list.
        let doomed = Doomed::Files(vec![file_link.clone()]);
        let folders = [s.music.clone()];
        assert_eq!(
            check_delete(&doomed, &folders, &[], &[]),
            Err(Refusal::Outside(file_link))
        );
        assert!(s.trash.moved().is_empty());
        assert!(outside.join("x.wav").exists());
    }

    #[test]
    fn a_failed_trash_deletes_nothing() {
        let s = setup(&["a.wav", "sub/b.wav"]);
        let a = s.track("a.wav");
        s.trash.fail();
        let (message, reread) = s.delete(DeleteTarget::Files {
            paths: vec![s.music.join("a.wav")],
        });
        assert_eq!(message, "Stick has no Trash: nothing was deleted");
        assert!(!reread);
        // The system's own words are kept, as a warning.
        let a_path = s.music.join("a.wav");
        assert_eq!(
            s.warning(),
            format!(
                "warning: could not move {} to the Trash: no Trash on this drive",
                a_path.display()
            )
        );
        assert!(s.music.join("a.wav").exists());
        assert!(!s.missing(a));
        let folder = DeleteTarget::Folder {
            path: s.music.join("sub"),
        };
        assert_eq!(
            s.delete(folder).0,
            "Stick has no Trash: nothing was deleted"
        );
        assert!(s.music.join("sub/b.wav").exists());
        assert!(!s.missing(s.track("sub/b.wav")));

        // Any other failure says what it was, and that nothing was deleted.
        s.trash
            .fail_with(TrashError::Other("permission denied".into()));
        let (message, reread) = s.delete(DeleteTarget::Files {
            paths: vec![s.music.join("a.wav")],
        });
        assert_eq!(
            message,
            "Could not move a.wav to the Trash (permission denied): nothing was deleted"
        );
        assert!(!reread);
        assert!(s.warning().ends_with("to the Trash: permission denied"));
        assert!(s.music.join("a.wav").exists());
        assert!(!s.missing(a));
    }

    #[test]
    fn only_a_missing_trash_is_called_one() {
        let path = Path::new("/Volumes/X/a.mp3");
        let unknown = |text: &str| {
            let error = trash::Error::Unknown {
                description: text.to_owned(),
            };
            TrashError::from_trash(&error, path)
        };
        let os = |code: u32| {
            let error = trash::Error::Os {
                code: code as i32,
                description: "windows error".into(),
            };
            TrashError::from_trash(&error, path)
        };
        for no_trash in [
            "While deleting '\"/Volumes/X/a.mp3\"', `trashItemAtURL` failed: Error Domain=NSCocoaErrorDomain Code=3328",
            "Could not find a valid 'home trash' nor valid trashes on other mount points",
        ] {
            assert!(
                matches!(unknown(no_trash), TrashError::NoTrash(_)),
                "{no_trash}"
            );
        }
        // Windows: the copy engine's codes for a drive without a Recycle Bin.
        assert!(matches!(os(0x8027_0036), TrashError::NoTrash(_)));
        assert!(matches!(os(0x8027_003A), TrashError::NoTrash(_)));
        for other in [0x8027_0022_u32, 0x8027_0037, 0x8007_0005] {
            assert!(matches!(os(other), TrashError::Other(_)), "{other:#x}");
        }
        assert!(matches!(
            unknown("Permission denied (os error 13)"),
            TrashError::Other(_)
        ));
    }

    #[test]
    fn a_file_name_that_reads_like_a_missing_trash_is_not_one() {
        // The path is in the crate's text, and the system may name the file too: neither
        // counts.
        let path = Path::new("/Music/3328 not supported/no trash 3328.mp3");
        let error = trash::Error::Unknown {
            description: format!(
                "While deleting '{path:?}', `trashItemAtURL` failed: \u{201c}no trash 3328.mp3\u{201d} couldn\u{2019}t be moved because you don\u{2019}t have permission to access \u{201c}3328 not supported\u{201d}."
            ),
        };
        assert!(matches!(
            TrashError::from_trash(&error, path),
            TrashError::Other(_)
        ));
        let error = trash::Error::Os {
            code: 0x8027_0022_u32 as i32,
            description: format!("windows error: {}: not supported 3328", path.display()),
        };
        assert!(matches!(
            TrashError::from_trash(&error, path),
            TrashError::Other(_)
        ));
    }

    #[test]
    fn a_folder_holding_open_jockeys_own_data_is_refused() {
        // The library folder is the home folder, which holds the app's data folder.
        let s = setup(&["a.wav"]);
        let home = s.music.clone();
        let data = home.join("Library/Application Support/open-jockey");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(data.join("library.sqlite3"), "db").unwrap();
        let folders = [home.clone()];
        let own = [data.clone()];
        for folder in [home.join("Library"), data.clone()] {
            let name = folder.file_name().unwrap().to_string_lossy().into_owned();
            let refusal = check_delete(&Doomed::Folder(folder.clone()), &folders, &[], &own);
            assert_eq!(refusal, Err(Refusal::HoldsOwn(folder)));
            assert_eq!(
                refusal.unwrap_err().to_string(),
                format!("{name} holds Open Jockey's library: it can't be deleted from here.")
            );
        }
        // Nor anything inside it: the database, its backups, its covers.
        std::fs::create_dir_all(data.join("covers")).unwrap();
        for inside in [data.join("library.sqlite3"), data.join("covers")] {
            let doomed = match inside.is_dir() {
                true => Doomed::Folder(inside.clone()),
                false => Doomed::Files(vec![inside.clone()]),
            };
            let refusal = check_delete(&doomed, &folders, &[], &own).unwrap_err();
            assert_eq!(refusal, Refusal::InOwn(inside.clone()));
            assert!(
                refusal
                    .to_string()
                    .ends_with("is part of Open Jockey's library: it can't be deleted from here."),
                "{refusal}"
            );
        }
        // Elsewhere in the folder, a delete goes on.
        let doomed = Doomed::Files(vec![home.join("a.wav")]);
        assert!(check_delete(&doomed, &folders, &[], &own).is_ok());
        // The worker knows the library's own folder: the database is next to `music`.
        let worker_own = own_folders(s.lib.path());
        assert_eq!(
            canonical(&worker_own[0]),
            canonical(s.lib.path().parent().unwrap())
        );
    }

    /// Sets `path`'s permission bits.
    #[cfg(unix)]
    fn chmod(path: &Path, mode: u32) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_target_that_cannot_be_identified_is_refused() {
        let s = setup(&["sub/a.wav", "b.wav"]);
        let sub = s.music.join("sub");
        chmod(&sub, 0o000);
        let folders = [s.music.clone()];
        let refusal = check_delete(&Doomed::Folder(sub.clone()), &folders, &[], &[]);
        chmod(&sub, 0o755);
        let refusal = refusal.unwrap_err();
        assert_eq!(refusal, Refusal::Unchecked(sub));
        assert_eq!(
            refusal.to_string(),
            "sub can't be checked: nothing was deleted."
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_library_folder_that_cannot_be_identified_refuses_every_delete() {
        let s = setup(&["b.wav"]);
        // A second library folder, now out of reach: a delete could be it or hold it.
        let locked = s.dir.path().join("locked");
        std::fs::create_dir_all(&locked).unwrap();
        let folders = [s.music.clone(), locked.clone()];
        chmod(&locked, 0o000);
        let doomed = Doomed::Files(vec![s.music.join("b.wav")]);
        let refusal = check_delete(&doomed, &folders, &[], &[]);
        chmod(&locked, 0o755);
        assert_eq!(refusal, Err(Refusal::Unchecked(s.music.join("b.wav"))));
        // Once it can be read again, the delete goes on.
        assert!(check_delete(&doomed, &folders, &[], &[]).is_ok());
    }

    #[test]
    fn an_unplugged_library_folder_or_a_gone_loaded_file_blocks_nothing() {
        let s = setup(&["b.wav"]);
        // A library folder whose drive is not plugged in, and a loaded file since gone.
        let unplugged = s.dir.path().join("unplugged/Music");
        let folders = [s.music.clone(), unplugged.clone()];
        let loaded = [(deck(1), unplugged.join("gone.wav"))];
        let doomed = Doomed::Files(vec![s.music.join("b.wav")]);
        assert!(check_delete(&doomed, &folders, &loaded, &[]).is_ok());
    }

    /// Whether the folder `dir` is on a drive that ignores case.
    fn ignores_case(dir: &Path) -> bool {
        let probe = dir.join("CaseProbe");
        std::fs::write(&probe, "").unwrap();
        let ignores = dir.join("caseprobe").exists();
        std::fs::remove_file(&probe).unwrap();
        ignores
    }

    #[test]
    fn names_in_another_case_are_the_same_places() {
        let s = setup(&["House/a.wav", "b.wav"]);
        if !ignores_case(&s.music) {
            // Only the comparison by identity can be checked here: names differ.
            return;
        }
        // A library folder inside another, added in the right case: named in another case
        // it is still a library folder, and its parent still holds one.
        let nested = s.music.join("House");
        let folders = [s.music.clone(), nested.clone()];
        let refusal = check_delete(&Doomed::Folder(s.music.join("house")), &folders, &[], &[]);
        assert!(matches!(refusal, Err(Refusal::IsRoot(_))), "{refusal:?}");
        // Added in the wrong case, it is the same.
        let folders = [s.music.clone(), s.music.join("HOUSE")];
        let refusal = check_delete(&Doomed::Folder(nested.clone()), &folders, &[], &[]);
        assert!(matches!(refusal, Err(Refusal::IsRoot(_))), "{refusal:?}");
        // A loaded file named in another case is still loaded, alone or in its folder.
        let folders = [s.music.clone()];
        let loaded = [(deck(2), s.music.join("b.wav"))];
        let doomed = Doomed::Files(vec![s.music.join("B.WAV")]);
        let refusal = check_delete(&doomed, &folders, &loaded, &[]);
        assert!(matches!(refusal, Err(Refusal::Loaded(_, _))), "{refusal:?}");
        let loaded = [(deck(1), nested.join("a.wav"))];
        let refusal = check_delete(
            &Doomed::Folder(s.music.join("hOuSe")),
            &folders,
            &loaded,
            &[],
        );
        assert!(matches!(refusal, Err(Refusal::Loaded(_, _))), "{refusal:?}");
        // Open Jockey's folder too.
        let own = [nested.clone()];
        let refusal = check_delete(&Doomed::Folder(s.music.join("house")), &folders, &[], &own);
        assert!(matches!(refusal, Err(Refusal::HoldsOwn(_))), "{refusal:?}");
    }

    #[test]
    fn a_file_the_library_does_not_have_goes_to_the_trash_too() {
        let s = setup(&["a.wav", "notes.txt"]);
        let notes = s.music.join("notes.txt");
        let target = DeleteTarget::Files {
            paths: vec![notes.clone(), s.music.join("a.wav")],
        };
        assert_eq!(
            s.ask(target.clone()),
            Ok("Move 2 files to the Trash?".into())
        );
        let (message, _) = s.delete(target);
        assert_eq!(message, "Moved 2 files to the Trash");
        assert!(!notes.exists());
        // Still checked as before: a path outside the library is refused.
        let outside = s.dir.path().join("elsewhere.txt");
        std::fs::write(&outside, "kept").unwrap();
        let target = DeleteTarget::Files {
            paths: vec![outside.clone()],
        };
        assert!(
            s.ask(target)
                .unwrap_err()
                .ends_with("is outside the library folders.")
        );
        assert!(outside.exists());
    }

    #[test]
    fn a_folder_named_as_a_file_is_refused() {
        let s = setup(&["sub/a.wav"]);
        let doomed = Doomed::Files(vec![s.music.join("sub")]);
        let refusal = check_delete(&doomed, std::slice::from_ref(&s.music), &[], &[]).unwrap_err();
        assert_eq!(
            refusal.to_string(),
            "sub is a folder: delete it from the tree."
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_link_to_a_file_in_the_library_moves_only_the_link() {
        let s = setup(&["a.wav"]);
        let link = s.music.join("alias.wav");
        std::os::unix::fs::symlink(s.music.join("a.wav"), &link).unwrap();
        let folders = [s.music.clone()];
        let plan = check_delete(&Doomed::Files(vec![link.clone()]), &folders, &[], &[]).unwrap();
        assert_eq!(plan.paths, std::slice::from_ref(&link));
        // Even with its target on a deck: the target stays where it is.
        let loaded = [(deck(1), s.music.join("a.wav"))];
        let plan =
            check_delete(&Doomed::Files(vec![link.clone()]), &folders, &loaded, &[]).unwrap();
        assert_eq!(plan.paths, std::slice::from_ref(&link));
        let trash = FakeTrash::new(&s.dir.path().join("bin"));
        trash.trash(&plan.paths).unwrap();
        assert!(std::fs::symlink_metadata(&link).is_err());
        assert!(s.music.join("a.wav").exists());
    }

    #[cfg(unix)]
    #[test]
    fn a_link_to_a_folder_in_the_library_moves_only_the_link() {
        let s = setup(&["real/a.wav", "real/b.wav"]);
        let link = s.music.join("alias");
        std::os::unix::fs::symlink(s.music.join("real"), &link).unwrap();
        let target = DeleteTarget::Folder { path: link.clone() };
        assert_eq!(
            s.ask(target.clone()),
            Ok("Move the link alias to the Trash? The folder it points to stays.".into())
        );
        let (message, _) = s.delete(target);
        assert_eq!(message, "Moved 1 file to the Trash");
        assert_eq!(s.trash.moved(), std::slice::from_ref(&link));
        assert!(std::fs::symlink_metadata(&link).is_err());
        assert!(s.music.join("real/a.wav").exists());
        assert!(!s.missing(s.track("real/a.wav")));
    }

    #[test]
    fn select_all_then_delete_in_a_folder_counts_every_file() {
        let s = setup(&["a.wav", "b.wav", "c.wav", "cover.jpg"]);
        s.jobs.send(Job::Edit(BrowserEdit::SelectAll)).unwrap();
        let view = s.view();
        assert!(view.all_selected);
        let list = view.identity();
        let target = DeleteTarget::AllFiles { list };
        // The library's count: every file of the list, not the page the window holds.
        assert_eq!(
            s.ask(target.clone()),
            Ok("Move 3 files to the Trash?".into())
        );
        // A pick since then: the selection is gone, and so is the delete.
        s.jobs.send(Job::Edit(BrowserEdit::SelectRow(1))).unwrap();
        s.view();
        assert_eq!(s.ask(target.clone()), Err(LIST_CHANGED.into()));
        assert_eq!(s.delete(target).0, LIST_CHANGED);
        assert!(s.trash.moved().is_empty());

        s.jobs.send(Job::Edit(BrowserEdit::SelectAll)).unwrap();
        let list = s.view().identity();
        let (message, _) = s.delete(DeleteTarget::AllFiles { list });
        assert_eq!(message, "Moved 3 files to the Trash");
        assert_eq!(s.trash.moved().len(), 3);
        assert!(s.music.join("cover.jpg").exists());
    }

    #[test]
    fn select_all_then_delete_in_a_playlist_removes_every_entry() {
        let s = setup(&["a.wav", "b.wav"]);
        let (a, b) = (s.track("a.wav"), s.track("b.wav"));
        let playlist = s.peak(&[a, b, a]);
        s.jobs
            .send(Job::Refresh {
                decks: Vec::new(),
                find_new_tracks: false,
            })
            .unwrap();
        let view = s.view();
        let row = view
            .tree
            .iter()
            .position(|r| r.playlist == Some(playlist))
            .unwrap();
        s.jobs
            .send(Job::Edit(BrowserEdit::SelectTree(row)))
            .unwrap();
        s.view();
        s.jobs.send(Job::Edit(BrowserEdit::SelectAll)).unwrap();
        let list = s.view().identity();
        let target = DeleteTarget::AllEntries { list };
        assert_eq!(
            s.ask(target.clone()),
            Ok("Remove 3 tracks from Peak? The files stay.".into())
        );
        s.jobs
            .send(Job::Delete {
                target,
                decks: DeckFiles::default(),
            })
            .unwrap();
        s.view();
        let reader = s.lib.reader().unwrap();
        assert!(reader.playlist_tracks(playlist, 0, 10).unwrap().is_empty());
        assert!(s.music.join("a.wav").exists());
    }

    #[test]
    fn the_questions_name_what_goes() {
        let plan = |paths: &[&str], files, folder: Option<&str>| Plan {
            paths: paths.iter().map(PathBuf::from).collect(),
            files,
            what: folder.map_or(What::Files, |name| What::Folder(name.to_owned())),
        };
        assert_eq!(
            plan(&["/m/a.mp3", "/m/b.mp3"], 2, None).question(),
            "Move 2 files to the Trash?"
        );
        assert_eq!(
            plan(&["/m/Intro.mp3"], 1, None).question(),
            "Move Intro.mp3 to the Trash?"
        );
        assert_eq!(
            plan(&["/m/Old"], 1, Some("Old")).question(),
            "Move the folder Old and its 1 file to the Trash?"
        );
    }
}
