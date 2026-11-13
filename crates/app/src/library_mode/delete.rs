//! Deleting from the browser, always after the window asked: entries out of a playlist
//! and whole playlists through the playlist edits, files and folders to the system's
//! Trash, never deleted outright.
//!
//! Every file or folder is checked by [`check_delete`] when the window asks and again on
//! the worker just before it moves: only what lies inside a library folder (compared as
//! canonical paths, so a link cannot point a delete outside), never a library folder
//! itself or a folder holding one, and nothing loaded on a deck. A failed Trash move
//! deletes nothing more: there is no fallback to deleting for good. The library tracks
//! whose files went are marked missing, as a rescan would; their hotcues stay.

use std::fmt;
use std::path::{Path, PathBuf};

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
pub struct TrashError(pub String);

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
                .map_err(|e| TrashError(e.to_string()))?;
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
    /// The canonical paths to move.
    pub paths: Vec<PathBuf>,
    /// How many files go: the files themselves, or every file in the folder.
    pub files: usize,
    /// The folder's name, for a folder.
    folder: Option<String>,
}

impl Plan {
    /// The question the window asks before the move.
    pub fn question(&self) -> String {
        match (&self.folder, self.paths.as_slice()) {
            (Some(name), _) => format!(
                "Move the folder {name} and its {} to the Trash?",
                count(self.files, "file")
            ),
            (None, [one]) => format!("Move {} to the Trash?", name_of(one)),
            (None, _) => format!("Move {} to the Trash?", count(self.files, "file")),
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
    /// `.0` is not inside any library folder.
    Outside(PathBuf),
    /// `.0` is not on disk.
    Gone(PathBuf),
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
            Self::Outside(path) => {
                write!(
                    f,
                    "{} is outside the library folders.",
                    safe(path.display())
                )
            }
            Self::Gone(path) => write!(f, "{} is not on disk any more.", name_of(path)),
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

/// Whether moving `doomed` to the Trash is allowed with `folders` the library folders and
/// `loaded` the files on the decks, and if so what moves. Reads the disk (to resolve the
/// paths and count a folder's files) but changes nothing.
///
/// # Errors
///
/// Why it is refused: a path outside the library folders, a library folder or a folder
/// holding one, a loaded file or a folder holding one, a path no longer on disk.
pub fn check_delete(
    doomed: &Doomed,
    folders: &[PathBuf],
    loaded: &[(DeckId, PathBuf)],
) -> Result<Plan, Refusal> {
    let roots: Vec<PathBuf> = folders.iter().map(|f| canonical(f)).collect();
    let loaded: Vec<(DeckId, PathBuf)> = loaded
        .iter()
        .map(|(deck, path)| (*deck, canonical(path)))
        .collect();
    let inside = |path: &Path| roots.iter().any(|r| path.starts_with(r) && path != r);
    match doomed {
        Doomed::Files(files) => {
            let mut paths: Vec<PathBuf> = Vec::with_capacity(files.len());
            for file in files {
                let path = dunce::canonicalize(file).map_err(|_| Refusal::Gone(file.clone()))?;
                if !inside(&path) {
                    return Err(Refusal::Outside(file.clone()));
                }
                if let Some((deck, _)) = loaded.iter().find(|(_, l)| *l == path) {
                    return Err(Refusal::Loaded(path, *deck));
                }
                if !paths.contains(&path) {
                    paths.push(path);
                }
            }
            if paths.is_empty() {
                return Err(Refusal::Nothing);
            }
            Ok(Plan {
                files: paths.len(),
                paths,
                folder: None,
            })
        }
        Doomed::Folder(folder) => {
            let path = dunce::canonicalize(folder).map_err(|_| Refusal::Gone(folder.clone()))?;
            if roots.contains(&path) {
                return Err(Refusal::IsRoot(path));
            }
            if roots.iter().any(|r| r.starts_with(&path)) {
                return Err(Refusal::HoldsRoot(path));
            }
            if !inside(&path) {
                return Err(Refusal::Outside(folder.clone()));
            }
            if let Some((deck, file)) = loaded.iter().find(|(_, l)| l.starts_with(&path)) {
                return Err(Refusal::Loaded(file.clone(), *deck));
            }
            Ok(Plan {
                files: count_files(&path),
                folder: Some(name_of(&path)),
                paths: vec![path],
            })
        }
    }
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
        DeleteTarget::Files { tracks } => {
            let mut paths = Vec::with_capacity(tracks.len());
            for &track in tracks {
                if let Some(details) = reader.track(track).map_err(warn)? {
                    paths.push(details.path);
                }
            }
            Resolved::Disk(Doomed::Files(paths))
        }
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
/// `loaded` the files on the decks.
pub(super) fn ask(
    target: &DeleteTarget,
    browser: &Browser,
    reader: &Reader,
    loaded: &[(DeckId, PathBuf)],
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
            check_delete(&doomed, &folders, loaded)
                .map(|plan| plan.question())
                .map_err(|refusal| refusal.to_string())
        }
    }
}

/// What a delete did, for the worker to report.
pub(super) struct Done {
    /// The library changed: the browser is to be read again.
    pub refresh: bool,
    /// The event to show, after the browser is read again.
    pub message: Option<String>,
}

/// Where the worker deletes: the library, the browser (for whole lists and drive names)
/// and the Trash.
pub(super) struct Deleter<'a> {
    pub reader: &'a Reader,
    pub writer: &'a Writer,
    pub browser: &'a Browser,
    pub trash: &'a dyn Trash,
}

impl Deleter<'_> {
    /// Deletes `target`, checked again now, with `loaded` the files on the decks.
    pub(super) fn delete(&self, target: &DeleteTarget, loaded: &[(DeckId, PathBuf)]) -> Done {
        let refused = |message: String| Done {
            refresh: false,
            message: Some(message),
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
                return match check_delete(&doomed, &folders, loaded) {
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
            message,
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
        let message = match failed {
            None => format!("Moved {} to the Trash", count(plan.files, "file")),
            Some(_) => {
                let stuck = plan.paths.iter().find(|p| !is_gone(p));
                let drive = self.drive(stuck.unwrap_or(&plan.paths[0]));
                if gone.is_empty() {
                    format!("{drive} has no Trash: nothing was deleted")
                } else {
                    format!(
                        "{drive} has no Trash: moved {} to the Trash, the rest stay",
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
            message: Some(message),
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
    fail: std::sync::atomic::AtomicBool,
}

#[cfg(test)]
impl FakeTrash {
    /// A Trash that moves what it is given into `bin` (made on the first move).
    pub fn new(bin: &Path) -> Self {
        Self {
            bin: bin.to_owned(),
            moved: std::sync::Mutex::default(),
            fail: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// From now on every move fails, moving nothing.
    pub fn fail(&self) {
        self.fail.store(true, std::sync::atomic::Ordering::Relaxed);
    }

    /// What it moved, in order.
    pub fn moved(&self) -> Vec<PathBuf> {
        self.moved.lock().unwrap().clone()
    }
}

#[cfg(test)]
impl Trash for FakeTrash {
    fn trash(&self, paths: &[PathBuf]) -> Result<(), TrashError> {
        if self.fail.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(TrashError("no Trash on this drive".into()));
        }
        let mut moved = self.moved.lock().unwrap();
        std::fs::create_dir_all(&self.bin).map_err(|e| TrashError(e.to_string()))?;
        for path in paths {
            let to = self.bin.join(format!("{}-{}", moved.len(), name_of(path)));
            std::fs::rename(path, to).map_err(|e| TrashError(e.to_string()))?;
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
        let lib = Library::open(&dir.path().join("library.sqlite3")).unwrap();
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
            reader.tracks_under(&self.music.join(file)).unwrap()[0]
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
        /// whether the browser was read again before it.
        fn delete_with(&self, target: DeleteTarget, decks: DeckFiles) -> (String, bool) {
            self.jobs.send(Job::Delete { target, decks }).unwrap();
            let mut reread = false;
            let message = self.wait_for(|e| match e {
                Event::Browser(_) => {
                    reread = true;
                    None
                }
                Event::Message(text) => Some(text),
                _ => None,
            });
            (message, reread)
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
        let target = DeleteTarget::Files { tracks: vec![a] };
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
        let target = DeleteTarget::Files { tracks: vec![a] };
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
            check_delete(&doomed, &folders, &[]),
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
        let (message, reread) = s.delete(DeleteTarget::Files { tracks: vec![a] });
        assert_eq!(message, "Stick has no Trash: nothing was deleted");
        assert!(!reread);
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
            folder: folder.map(str::to_owned),
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
