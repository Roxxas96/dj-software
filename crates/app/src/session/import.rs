//! The import from VirtualDJ in the window: the window's [`ImportStep`]s move it along and
//! the frame shows where it is ([`ImportView`]). Reading VirtualDJ's data with the plan,
//! and writing the import, run on a thread of their own: the control loop never waits.
//!
//! The decks must stay empty from the scan to the write: the plan read the tracks' hotcues
//! at the scan, and a loaded track would keep its old ones on the deck and could write
//! them back.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError, channel};

use interop::{ImportPlan, ImportReport, TrackImport};
use library::Library;
use ui::{ImportStep, ImportSummary, ImportView, TrackLabel};

use crate::format::{safe, track_name};
use crate::vdj::{Places, Scan};

/// The window's import from VirtualDJ.
pub(super) struct Import {
    lib: Arc<Library>,
    places: Places,
    /// Set once an import is written, for library mode to read the browser again.
    changed: Option<Arc<AtomicBool>>,
    state: State,
    view: ImportView,
}

/// What the import is doing, besides what it shows.
enum State {
    /// Nothing: not started, cancelled or finished.
    Rest,
    Scanning(Receiver<Result<Box<Planned>, String>>),
    /// Planned: the summary shows, then (`asking`) the questions.
    Planned(Box<Planned>),
    Applying(Receiver<Result<ImportReport, String>>),
}

/// A plan, and the answers to its questions so far.
struct Planned {
    plan: ImportPlan,
    summary: ImportSummary,
    /// The conflicting tracks' names, by track id.
    names: HashMap<i64, String>,
    answers: HashMap<i64, interop::Resolution>,
    /// The answer for every track not answered yet.
    rest: Option<interop::Resolution>,
    /// The import was asked for: the questions show in place of the summary.
    asking: bool,
}

impl Import {
    /// An import into `lib` from VirtualDJ's data in `places`; `changed` is set once it
    /// has written.
    pub fn new(lib: Arc<Library>, places: Places, changed: Option<Arc<AtomicBool>>) -> Self {
        Self {
            lib,
            places,
            changed,
            state: State::Rest,
            view: ImportView::Idle,
        }
    }

    /// What the window shows.
    pub fn view(&self) -> &ImportView {
        &self.view
    }

    /// Takes the window's `step`; `decks_loaded` says whether a deck holds a track. A
    /// step that does not fit where the import is (an answer after a cancel, anything
    /// while it writes) is ignored.
    pub fn step(&mut self, step: ImportStep, decks_loaded: bool) {
        if matches!(self.state, State::Applying(_)) {
            return;
        }
        match step {
            ImportStep::Scan => {
                if decks_loaded {
                    self.refuse();
                } else {
                    self.scan();
                }
            }
            ImportStep::Cancel => {
                self.state = State::Rest;
                self.view = ImportView::Idle;
            }
            ImportStep::Apply => {
                let State::Planned(planned) = &mut self.state else {
                    return;
                };
                if planned.asking {
                    return;
                }
                if decks_loaded {
                    self.refuse();
                    return;
                }
                planned.asking = true;
                self.next(decks_loaded);
            }
            ImportStep::Resolve {
                track,
                resolution,
                rest_too,
            } => {
                let State::Planned(planned) = &mut self.state else {
                    return;
                };
                if !planned.asking || planned.question().map(|t| t.track) != Some(track) {
                    return;
                }
                let resolution = match resolution {
                    ui::Resolution::Keep => interop::Resolution::Keep,
                    ui::Resolution::Replace => interop::Resolution::Replace,
                    ui::Resolution::Merge => interop::Resolution::Merge,
                };
                planned.answers.insert(track, resolution);
                if rest_too {
                    planned.rest = Some(resolution);
                }
                self.next(decks_loaded);
            }
        }
    }

    /// Takes the thread's result, if it came; `true` when an import was just written.
    pub fn poll(&mut self) -> bool {
        match &self.state {
            State::Scanning(done) => match done.try_recv() {
                Err(TryRecvError::Empty) => false,
                Ok(Ok(planned)) => {
                    self.view = ImportView::Summary(planned.summary.clone());
                    self.state = State::Planned(planned);
                    false
                }
                Ok(Err(why)) => self.fail(why),
                Err(TryRecvError::Disconnected) => self.fail(STOPPED.to_owned()),
            },
            State::Applying(done) => match done.try_recv() {
                Err(TryRecvError::Empty) => false,
                Ok(Ok(report)) => {
                    self.state = State::Rest;
                    self.view = ImportView::Done(format!(
                        "Imported {} tracks and {} playlists. Backup: {}",
                        report.tracks_changed,
                        report.playlists_created,
                        safe(report.backup.display())
                    ));
                    if let Some(changed) = &self.changed {
                        changed.store(true, Ordering::Relaxed);
                    }
                    true
                }
                Ok(Err(why)) => self.fail(why),
                Err(TryRecvError::Disconnected) => self.fail(STOPPED.to_owned()),
            },
            State::Rest | State::Planned(_) => false,
        }
    }

    /// Reads VirtualDJ's data and plans the import on a thread.
    fn scan(&mut self) {
        let (lib, places) = (Arc::clone(&self.lib), self.places.clone());
        self.start(
            move || plan(&lib, &places),
            State::Scanning,
            ImportView::Scanning,
        );
    }

    /// Asks the next question or, with none left, writes the import on a thread.
    fn next(&mut self, decks_loaded: bool) {
        let State::Planned(planned) = &self.state else {
            return;
        };
        if let Some(track) = planned.question() {
            self.view = planned.ask(track);
            return;
        }
        if decks_loaded {
            self.refuse();
            return;
        }
        let State::Planned(planned) = std::mem::replace(&mut self.state, State::Rest) else {
            return;
        };
        let lib = Arc::clone(&self.lib);
        let write = move || {
            let Planned {
                plan,
                answers,
                rest,
                ..
            } = *planned;
            let resolve = |t: &TrackImport| {
                answers
                    .get(&t.track)
                    .copied()
                    .or(rest)
                    .unwrap_or(interop::Resolution::Keep)
            };
            interop::apply(&lib, &plan, resolve)
                .map_err(|e| safe(format!("Nothing was imported: {e}")))
        };
        self.start(write, State::Applying, ImportView::Applying);
    }

    /// Runs `work` on a thread; the import is then in `state` with its receiver.
    fn start<T: Send + 'static>(
        &mut self,
        work: impl FnOnce() -> Result<T, String> + Send + 'static,
        state: impl FnOnce(Receiver<Result<T, String>>) -> State,
        view: ImportView,
    ) {
        let (tx, rx) = channel();
        let spawned = std::thread::Builder::new()
            .name("vdj-import".to_owned())
            .spawn(move || {
                // A cancelled scan has no one to send to.
                let _ = tx.send(work());
            });
        match spawned {
            Ok(_) => {
                self.state = state(rx);
                self.view = view;
            }
            Err(e) => {
                self.fail(format!("cannot start the import: {e}"));
            }
        }
    }

    /// Refuses the import while a deck holds a track; nothing is written.
    fn refuse(&mut self) {
        self.state = State::Rest;
        self.view = ImportView::DecksBusy;
    }

    /// Stops with `why`; nothing is written.
    fn fail(&mut self, why: String) -> bool {
        self.state = State::Rest;
        self.view = ImportView::Failed(why);
        false
    }
}

/// What the window shows when the import's thread died.
const STOPPED: &str = "The import stopped unexpectedly; nothing was imported.";

/// Reads VirtualDJ's data from `places` and plans the import into `lib`.
fn plan(lib: &Library, places: &Places) -> Result<Box<Planned>, String> {
    let fail = |e: anyhow::Error| safe(format!("{e:#}"));
    let reader = lib.reader().map_err(|e| fail(e.into()))?;
    let scanned = match crate::vdj::scan(&reader, places).map_err(fail)? {
        Scan::Found(scanned) => scanned,
        Scan::Nothing { searched } => {
            let searched: Vec<String> = searched.iter().map(|d| d.display().to_string()).collect();
            return Err(safe(if searched.is_empty() {
                "No VirtualDJ data found.".to_owned()
            } else {
                format!("No VirtualDJ data found in: {}", searched.join(", "))
            }));
        }
    };
    let mut names = HashMap::new();
    for t in scanned.plan.tracks.iter().filter(|t| t.conflicts()) {
        let name = match reader.track(t.track) {
            Ok(Some(details)) => track_name(&details.row),
            _ => safe(
                t.path
                    .file_name()
                    .unwrap_or(t.path.as_os_str())
                    .to_string_lossy()
                    .into_owned(),
            ),
        };
        names.insert(t.track, name);
    }
    Ok(Box::new(Planned {
        summary: scanned.summary(),
        plan: scanned.plan,
        names,
        answers: HashMap::new(),
        rest: None,
        asking: false,
    }))
}

impl Planned {
    /// The next conflicting track to ask about, if any is left.
    fn question(&self) -> Option<&TrackImport> {
        if self.rest.is_some() {
            return None;
        }
        self.unanswered().next()
    }

    fn unanswered(&self) -> impl Iterator<Item = &TrackImport> {
        self.plan
            .tracks
            .iter()
            .filter(|t| t.conflicts() && !self.answers.contains_key(&t.track))
    }

    /// The question about `t`.
    fn ask(&self, t: &TrackImport) -> ImportView {
        ImportView::Ask {
            track: TrackLabel {
                id: t.track,
                name: self.names.get(&t.track).cloned().unwrap_or_default(),
            },
            existing: t.existing.iter().flatten().count(),
            incoming: t.incoming.iter().flatten().count(),
            left: self.unanswered().count(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::mpsc::Sender;
    use std::time::{Duration, Instant};

    use controller::Runtime;
    use engine::{Command, DeckCommand, DeckId, EngineController, EngineRenderer};
    use library::{HotcuePoint, WriteOp};
    use ui::{Resolution, UiCommand};

    use super::super::commands;
    use super::super::frame::FrameParts;
    use super::*;
    use crate::config::Config;

    const STEREO: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../decode/tests/fixtures/sine440.wav"
    );
    const MONO: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../decode/tests/fixtures/sine440_mono.wav"
    );

    /// A library of two scanned tracks, a VirtualDJ folder with hotcues for both and a
    /// playlist of both, and the session's parts importing from that folder only.
    struct Setup {
        dir: tempfile::TempDir,
        lib: Arc<Library>,
        /// Tracks a.wav and b.wav.
        ids: [i64; 2],
        changed: Arc<AtomicBool>,
        parts: FrameParts,
        window: Sender<UiCommand>,
        commands: Receiver<UiCommand>,
    }

    fn setup() -> Setup {
        let dir = tempfile::tempdir().unwrap();
        // Library folders are stored canonical (/private/var on macOS).
        let root = dunce::canonicalize(dir.path()).unwrap();
        let music = root.join("music");
        std::fs::create_dir_all(&music).unwrap();
        std::fs::copy(STEREO, music.join("a.wav")).unwrap();
        std::fs::copy(MONO, music.join("b.wav")).unwrap();
        let lib = Arc::new(Library::open(&root.join("library.sqlite3")).unwrap());
        library::add_folder(&lib, &music).unwrap();
        library::scan_folder(&lib, &music, &mut |_| {}).unwrap();
        let vdj = root.join("vdj");
        std::fs::create_dir_all(vdj.join("MyLists")).unwrap();
        let (a, b) = (music.join("a.wav"), music.join("b.wav"));
        let xml = format!(
            "<VirtualDJ_Database Version=\"2025\">\n\
             <Song FilePath=\"{}\">\n\
             <Poi Pos=\"0.1\" Num=\"1\" Type=\"cue\" />\n\
             <Poi Pos=\"0.2\" Num=\"2\" Type=\"cue\" />\n\
             </Song>\n\
             <Song FilePath=\"{}\">\n\
             <Poi Pos=\"0.3\" Num=\"1\" Type=\"cue\" />\n\
             </Song>\n\
             </VirtualDJ_Database>",
            a.display(),
            b.display()
        );
        std::fs::write(vdj.join("database.xml"), xml).unwrap();
        let list = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<VirtualFolder>\n\
             <song path=\"{}\" />\n<song path=\"{}\" />\n</VirtualFolder>",
            a.display(),
            b.display()
        );
        std::fs::write(vdj.join("MyLists/Set.vdjfolder"), list).unwrap();
        let ids = [id_of(&lib, &a), id_of(&lib, &b)];
        let changed = Arc::new(AtomicBool::new(false));
        let mut parts = FrameParts::new(&Config::default());
        // The temp folder stands for `/Volumes`: nothing outside it is read.
        let places = Places {
            homes: vec![vdj],
            mounts: root,
        };
        parts.import = Some(Import::new(
            Arc::clone(&lib),
            places,
            Some(Arc::clone(&changed)),
        ));
        let (window, commands) = channel();
        Setup {
            dir,
            lib,
            ids,
            changed,
            parts,
            window,
            commands,
        }
    }

    fn id_of(lib: &Library, path: &Path) -> i64 {
        let reader = lib.reader().unwrap();
        (1..10)
            .find(|&id| reader.track(id).unwrap().is_some_and(|t| t.path == path))
            .expect("the track is in the library")
    }

    fn point(seconds: f64) -> HotcuePoint {
        HotcuePoint {
            frames: seconds * 44_100.0,
            loop_frames: None,
            color: None,
            name: None,
        }
    }

    /// Gives both tracks a hotcue in slot 3, so both conflict.
    fn hotcues_here(s: &Setup) {
        let ops = s
            .ids
            .map(|track| WriteOp::PutHotcue {
                track,
                slot: 3,
                point: point(0.5),
            })
            .into();
        s.lib.writer().submit_wait(WriteOp::Batch(ops)).unwrap();
    }

    fn slots(s: &Setup, track: i64) -> [Option<HotcuePoint>; 8] {
        s.lib.reader().unwrap().hotcue_points(track).unwrap()
    }

    impl Setup {
        /// Sends `step` and applies it as the session does, with no engine running.
        fn send(&mut self, step: ImportStep) -> ImportView {
            self.window.send(UiCommand::Import(step)).unwrap();
            commands::drain(&self.commands, &mut self.parts, None);
            self.view()
        }

        /// Sends `step` with the control loop's engine, whose decks are what it holds.
        fn send_with(
            &mut self,
            step: ImportStep,
            runtime: &mut Runtime,
            engine: &mut EngineController,
        ) -> ImportView {
            self.window.send(UiCommand::Import(step)).unwrap();
            commands::drain(&self.commands, &mut self.parts, Some((runtime, engine)));
            self.view()
        }

        fn view(&self) -> ImportView {
            self.parts.idle().import
        }

        /// The view once the thread's work is done.
        fn settled(&mut self) -> ImportView {
            let began = Instant::now();
            loop {
                commands::drain(&self.commands, &mut self.parts, None);
                let view = self.view();
                if !matches!(view, ImportView::Scanning | ImportView::Applying) {
                    return view;
                }
                assert!(
                    began.elapsed() < Duration::from_secs(10),
                    "the import never finished"
                );
                std::thread::sleep(Duration::from_millis(5));
            }
        }

        fn backups(&self) -> PathBuf {
            dunce::canonicalize(self.dir.path())
                .unwrap()
                .join("backups")
        }

        fn nothing_written(&self) {
            assert!(!self.backups().exists(), "no backup");
            assert!(self.lib.reader().unwrap().playlists().unwrap().is_empty());
            for track in self.ids {
                let slots = slots(self, track);
                assert_eq!(slots[0], None);
                assert_eq!(slots[2], Some(point(0.5)));
            }
            assert!(!self.changed.load(Ordering::Relaxed));
        }
    }

    #[test]
    fn scan_then_apply_imports_from_a_temp_vdj_dir() {
        let mut s = setup();
        assert_eq!(s.send(ImportStep::Scan), ImportView::Scanning);
        assert_eq!(
            s.settled(),
            ImportView::Summary(ImportSummary {
                databases: 1,
                tracks: 2,
                with_marks: 2,
                hotcues: 3,
                playlists_new: 1,
                ..ImportSummary::default()
            })
        );
        s.send(ImportStep::Apply);
        let ImportView::Done(text) = s.settled() else {
            panic!("not done: {:?}", s.view());
        };
        let backup = text
            .strip_prefix("Imported 2 tracks and 1 playlists. Backup: ")
            .unwrap_or_else(|| panic!("{text}"));
        assert!(Path::new(backup).starts_with(s.backups()), "{backup}");
        let [a, b] = s.ids;
        assert!(slots(&s, a)[0].is_some() && slots(&s, a)[1].is_some());
        assert!(slots(&s, b)[0].is_some());
        let playlists = s.lib.reader().unwrap().playlists().unwrap();
        assert_eq!(playlists.len(), 1);
        assert_eq!(
            (playlists[0].name.as_str(), playlists[0].track_count),
            ("Set", 2)
        );
        assert!(
            s.changed.load(Ordering::Relaxed),
            "the browser is read again"
        );
        assert_eq!(s.send(ImportStep::Cancel), ImportView::Idle);
    }

    #[test]
    fn each_conflicting_track_is_asked_about() {
        let mut s = setup();
        hotcues_here(&s);
        s.send(ImportStep::Scan);
        let ImportView::Summary(summary) = s.settled() else {
            panic!("no summary: {:?}", s.view());
        };
        assert_eq!(summary.conflicting, 2);
        let [a, b] = s.ids;
        let ImportView::Ask {
            track,
            existing,
            incoming,
            left,
        } = s.send(ImportStep::Apply)
        else {
            panic!("no question: {:?}", s.view());
        };
        assert_eq!((track.id, existing, incoming, left), (a, 1, 2, 2));
        assert_eq!(track.name, "a");
        let asked = s.send(ImportStep::Resolve {
            track: a,
            resolution: Resolution::Keep,
            rest_too: false,
        });
        assert!(
            matches!(&asked, ImportView::Ask { track, left: 1, .. } if track.id == b),
            "{asked:?}"
        );
        s.send(ImportStep::Resolve {
            track: b,
            resolution: Resolution::Merge,
            rest_too: false,
        });
        assert!(matches!(s.settled(), ImportView::Done(_)), "{:?}", s.view());
        assert_eq!(slots(&s, a)[0], None, "kept");
        assert!(slots(&s, b)[0].is_some());
        assert_eq!(slots(&s, b)[2], Some(point(0.5)), "merged");
    }

    #[test]
    fn choosing_for_all_remaining_asks_no_more() {
        let mut s = setup();
        hotcues_here(&s);
        s.send(ImportStep::Scan);
        s.settled();
        let [a, b] = s.ids;
        assert!(matches!(
            s.send(ImportStep::Apply),
            ImportView::Ask { left: 2, .. }
        ));
        let next = s.send(ImportStep::Resolve {
            track: a,
            resolution: Resolution::Replace,
            rest_too: true,
        });
        assert!(
            matches!(next, ImportView::Applying | ImportView::Done(_)),
            "{next:?}"
        );
        assert!(matches!(s.settled(), ImportView::Done(_)), "{:?}", s.view());
        for track in [a, b] {
            assert!(slots(&s, track)[0].is_some());
            assert_eq!(slots(&s, track)[2], None, "replaced");
        }
    }

    #[test]
    fn cancel_writes_nothing() {
        let mut s = setup();
        hotcues_here(&s);
        s.send(ImportStep::Scan);
        s.settled();
        s.send(ImportStep::Apply);
        assert_eq!(s.send(ImportStep::Cancel), ImportView::Idle);
        // Answers sent after the cancel are not taken.
        s.send(ImportStep::Resolve {
            track: s.ids[0],
            resolution: Resolution::Replace,
            rest_too: true,
        });
        assert_eq!(s.settled(), ImportView::Idle);
        s.nothing_written();
    }

    fn render(renderer: &mut EngineRenderer) {
        let (mut master, mut headphones) = (vec![0.0; 1024], vec![0.0; 1024]);
        renderer.render(&mut master, &mut headphones);
    }

    fn load(engine: &mut EngineController, renderer: &mut EngineRenderer) {
        let track = Arc::new(engine::TrackBuffer::new(44_100, vec![0.0; 2 * 4410]).unwrap());
        engine
            .send(Command::Deck(
                DeckId::new(1).unwrap(),
                DeckCommand::Load {
                    track,
                    memory: engine::TrackMemory::default(),
                },
            ))
            .unwrap();
        render(renderer);
    }

    #[test]
    fn import_refuses_while_a_deck_is_loaded() {
        let mut s = setup();
        hotcues_here(&s);
        let mapping = controller::builtin::load_mapping_from("ddj-sb3", None).unwrap();
        let mut runtime = Runtime::new(mapping);
        let (mut engine, mut renderer) = engine::engine(engine::EngineConfig {
            sample_rate: 44_100,
            max_block_frames: 512,
        });
        render(&mut renderer);
        // Empty decks: the scan runs.
        s.send_with(ImportStep::Scan, &mut runtime, &mut engine);
        assert!(matches!(s.settled(), ImportView::Summary(_)));
        // A track loaded since: the import is refused, before any question.
        load(&mut engine, &mut renderer);
        assert_eq!(
            s.send_with(ImportStep::Apply, &mut runtime, &mut engine),
            ImportView::DecksBusy
        );
        // And a new scan too.
        assert_eq!(
            s.send_with(ImportStep::Scan, &mut runtime, &mut engine),
            ImportView::DecksBusy
        );
        assert_eq!(s.settled(), ImportView::DecksBusy);
        s.nothing_written();
    }

    #[test]
    fn a_track_loaded_during_the_questions_stops_the_write() {
        let mut s = setup();
        hotcues_here(&s);
        let mapping = controller::builtin::load_mapping_from("ddj-sb3", None).unwrap();
        let mut runtime = Runtime::new(mapping);
        let (mut engine, mut renderer) = engine::engine(engine::EngineConfig {
            sample_rate: 44_100,
            max_block_frames: 512,
        });
        render(&mut renderer);
        s.send_with(ImportStep::Scan, &mut runtime, &mut engine);
        s.settled();
        let asked = s.send_with(ImportStep::Apply, &mut runtime, &mut engine);
        assert!(matches!(asked, ImportView::Ask { .. }), "{asked:?}");
        load(&mut engine, &mut renderer);
        let answer = ImportStep::Resolve {
            track: s.ids[0],
            resolution: Resolution::Replace,
            rest_too: true,
        };
        assert_eq!(
            s.send_with(answer, &mut runtime, &mut engine),
            ImportView::DecksBusy
        );
        s.nothing_written();
    }

    #[test]
    fn no_virtualdj_data_says_where_it_looked() {
        let mut s = setup();
        let empty = s.dir.path().join("empty");
        std::fs::create_dir_all(&empty).unwrap();
        let root = dunce::canonicalize(s.dir.path()).unwrap();
        s.parts.import = Some(Import::new(
            Arc::clone(&s.lib),
            Places {
                homes: vec![empty.clone()],
                mounts: root.clone(),
            },
            None,
        ));
        s.send(ImportStep::Scan);
        assert_eq!(
            s.settled(),
            ImportView::Failed(format!(
                "No VirtualDJ data found in: {}, {}",
                empty.display(),
                root.join("music").join("VirtualDJ").display()
            ))
        );
    }
}
