//! The `config.toml` settings the window lists and edits. [`Settings`] keeps the file's
//! text, edits a key in place (its comments, order and layout kept), checks the result
//! with the same parser as `play` and saves it on a short-lived thread, atomically. Which
//! keys apply at once is in [`REGISTRY`]; the others apply at the next launch.
//!
//! `library` settings (the key notation) live in the library database, not here.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;

use toml_edit::{DocumentMut, Item, Value};
use ui::SettingRow;

use crate::config::{self, Config};

/// One `config.toml` key the window can change.
struct Entry {
    /// `table.key`.
    key: &'static str,
    category: &'static str,
    /// As TOML text; empty when the default is to leave it unset.
    default: &'static str,
    help: &'static str,
    /// Whether a change applies at once.
    live: bool,
}

const fn entry(
    key: &'static str,
    category: &'static str,
    default: &'static str,
    help: &'static str,
    live: bool,
) -> Entry {
    Entry {
        key,
        category,
        default,
        help,
        live,
    }
}

/// Every key in `docs/config.md`'s table, in its order.
const REGISTRY: &[Entry] = &[
    entry(
        "mixer.crossfader",
        "Mixer",
        "true",
        "false: the crossfader is ignored and every channel plays at full level",
        true,
    ),
    entry(
        "fx.slots",
        "Effects",
        r#"["echo", "reverb", "flanger"]"#,
        "The 3 effects of every deck without its own: echo, reverb, flanger or filter",
        true,
    ),
    entry(
        "fx.deck1",
        "Effects",
        "",
        "Deck 1's own 3 effects, in place of fx.slots",
        true,
    ),
    entry(
        "fx.deck2",
        "Effects",
        "",
        "Deck 2's own 3 effects, in place of fx.slots",
        true,
    ),
    entry(
        "fx.deck3",
        "Effects",
        "",
        "Deck 3's own 3 effects, in place of fx.slots",
        true,
    ),
    entry(
        "fx.deck4",
        "Effects",
        "",
        "Deck 4's own 3 effects, in place of fx.slots",
        true,
    ),
    entry(
        "sampler.volume",
        "Sampler",
        "1.0",
        "Sampler level, 0 to 1, on the master level's curve (0.5 is about -12 dB)",
        true,
    ),
    entry(
        "sampler.pads",
        "Sampler",
        "",
        "Up to 8 pads: { file = \"...\" }, { track = <id> } or {} for an empty pad",
        false,
    ),
    entry(
        "beat_jump.pads",
        "Pads",
        "[-4, 4, -8, 8, -16, 16, -32, 32]",
        "Beats the beat jump pads move; exactly 8, each non-zero with abs <= 512",
        true,
    ),
    entry(
        "beat_jump.shift_pads",
        "Pads",
        "[-1, 1, -2, 2, -4, 4, -64, 64]",
        "The same with SHIFT held",
        true,
    ),
    entry(
        "loop_roll.pads",
        "Pads",
        "[0.0625, 0.125, 0.25, 0.5, 1, 2, 4, 8]",
        "Beats the loop roll pads roll; exactly 8, each from 1/32 to 64",
        true,
    ),
    entry(
        "backspin.short",
        "Pads",
        "1.0",
        "Seconds of the short backspin, from 0.2 to 10",
        true,
    ),
    entry(
        "backspin.long",
        "Pads",
        "2.0",
        "Seconds of the long backspin, from 0.2 to 10",
        true,
    ),
    entry(
        "stems.enabled",
        "Stems",
        "false",
        "true: library tracks are separated into stems (and cached)",
        false,
    ),
    entry(
        "stems.eq_mode",
        "Stems",
        r#""eq""#,
        "What the EQ knobs control: \"eq\" (the bands) or \"stems\" (needs stems enabled)",
        true,
    ),
    entry(
        "stems.knobs",
        "Stems",
        r#""vdj""#,
        "Stems mode's knobs: \"vdj\" (kicks, melody, hats) or \"raw\" (bass, instruments, vocals)",
        true,
    ),
    entry(
        "stems.kicks_hz",
        "Stems",
        "300",
        "Where kicks hand over to hats in the \"vdj\" layout, in Hz, from 50 to 1000",
        false,
    ),
    entry(
        "stems.cache_gb",
        "Stems",
        "5",
        "The most disk the stems cache may use, in gigabytes, from 0.5 to 1000",
        false,
    ),
    entry(
        "audio.device",
        "Audio",
        "",
        "The output device: a name or part of one; unset, the system default",
        false,
    ),
    entry(
        "audio.master",
        "Audio",
        "[1, 2]",
        "The 1-based output channels carrying the master, left then right",
        false,
    ),
    entry(
        "audio.headphones",
        "Audio",
        "",
        "The 1-based output channels carrying the headphones; unset, none",
        false,
    ),
    entry(
        "controller.mapping",
        "Controller",
        r#""ddj-sb3""#,
        "The controller mapping: a built-in name or a mapping file path",
        false,
    ),
    entry(
        "ui.waveform_beats",
        "Interface",
        "16",
        "The beats across a waveform lane before the window's zoom is saved",
        true,
    ),
    entry(
        "ui.waveform_min_beats",
        "Interface",
        "4",
        "The most the waveforms zoom in: the fewest beats across a lane, 1 to 512",
        true,
    ),
    entry(
        "ui.waveform_max_beats",
        "Interface",
        "64",
        "The most the waveforms zoom out: the most beats across a lane, 1 to 512",
        true,
    ),
];

/// A change that was saved.
#[derive(Debug)]
pub struct Applied {
    /// The configuration the file now holds.
    pub config: Config,
    /// Whether every key that changed applies at once.
    pub live: bool,
}

/// The configuration file's settings, and their saving.
pub struct Settings {
    path: PathBuf,
    /// The file's name in errors.
    file: String,
    doc: DocumentMut,
    config: Config,
    /// Per [`REGISTRY`] entry, its value at launch: a next-launch key that differs is
    /// pending.
    launched: Vec<String>,
    /// Per [`REGISTRY`] entry, why its last change was refused.
    errors: Vec<Option<String>>,
    /// Why the file cannot be edited: it could not be read, so saving would lose it.
    unreadable: Option<String>,
    saver: Saver,
}

impl Settings {
    /// The settings in the file at `path` (a missing file holds none), which parsed as
    /// `config`.
    pub fn load(path: PathBuf, config: &Config) -> Self {
        let file = crate::format::safe(path.display());
        let (doc, unreadable) = match std::fs::read_to_string(&path) {
            Ok(text) => match text.parse::<DocumentMut>() {
                Ok(doc) => (doc, None),
                Err(e) => (DocumentMut::new(), Some(format!("{file}: {e}"))),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (DocumentMut::new(), None),
            Err(e) => (DocumentMut::new(), Some(format!("reading {file}: {e}"))),
        };
        let launched = REGISTRY.iter().map(|entry| value_of(&doc, entry)).collect();
        Self {
            path,
            file,
            doc,
            config: config.clone(),
            launched,
            errors: vec![None; REGISTRY.len()],
            unreadable,
            saver: Saver::new(),
        }
    }

    /// The configuration the file holds now.
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// One row per key in `docs/config.md`'s table, in its order.
    pub fn rows(&self) -> Vec<SettingRow> {
        REGISTRY
            .iter()
            .enumerate()
            .map(|(i, entry)| {
                let value = value_of(&self.doc, entry);
                SettingRow {
                    key: entry.key.to_owned(),
                    category: entry.category,
                    pending: !entry.live && value != self.launched[i],
                    value,
                    default: entry.default.to_owned(),
                    help: entry.help,
                    live: entry.live,
                    error: self.errors[i].clone(),
                }
            })
            .collect()
    }

    /// Sets `key` to `value`, TOML value text (`"vdj"`, `300`, `[3, 4]`, `true`); an empty
    /// `value` removes the key, so it is back to its default. The file is saved only if
    /// the whole of it is still valid; otherwise the key's row shows why.
    pub fn set(&mut self, key: &str, value: &str) -> Result<Applied, String> {
        let Some(index) = REGISTRY.iter().position(|entry| entry.key == key) else {
            return Err(format!("{}: unknown setting", crate::format::safe(key)));
        };
        let edited = self.edit(index, value);
        let result = edited.and_then(|doc| self.commit(doc));
        self.errors[index] = result.as_ref().err().cloned();
        result
    }

    /// Removes every key the registry knows from the file, keeping its comments and
    /// anything else it holds.
    pub fn reset_all(&mut self) -> Result<Applied, String> {
        if let Some(why) = &self.unreadable {
            return Err(why.clone());
        }
        let mut doc = self.doc.clone();
        for entry in REGISTRY {
            remove(&mut doc, entry.key);
        }
        let result = self.commit(doc);
        if result.is_ok() {
            self.errors.fill(None);
        }
        result
    }

    /// Why saves failed since the last call.
    pub fn save_errors(&self) -> Vec<String> {
        self.saver.errors.try_iter().collect()
    }

    /// Waits for the last save to be written.
    pub fn wait_for_saves(&mut self) {
        if let Some(handle) = self.saver.handle.take() {
            let _ = handle.join();
        }
    }

    /// The file's text with `REGISTRY[index]` set to `value`.
    fn edit(&self, index: usize, value: &str) -> Result<DocumentMut, String> {
        if let Some(why) = &self.unreadable {
            return Err(why.clone());
        }
        let key = REGISTRY[index].key;
        let mut doc = self.doc.clone();
        if value.trim().is_empty() {
            remove(&mut doc, key);
            return Ok(doc);
        }
        let mut value = value.parse::<Value>().map_err(|_| {
            format!(
                "{}: {key}: expected a TOML value, such as 300, \"text\" or [1, 2]",
                self.file
            )
        })?;
        value.decor_mut().clear();
        let (table, name) = split(key);
        let root = doc.as_table_mut();
        if !root.contains_key(table) {
            root.insert(table, Item::Table(toml_edit::Table::new()));
        }
        let Some(table) = root.get_mut(table).and_then(Item::as_table_like_mut) else {
            return Err(format!("{}: {table}: expected a table", self.file));
        };
        match table.get_mut(name) {
            Some(Item::Value(old)) => {
                // The comment after the old value stays after the new one.
                *value.decor_mut() = old.decor().clone();
                *old = value;
            }
            _ => {
                table.insert(name, Item::Value(value));
            }
        }
        Ok(doc)
    }

    /// Checks `doc` with `play`'s parser and, if it is valid, makes it the file's text and
    /// saves it.
    fn commit(&mut self, doc: DocumentMut) -> Result<Applied, String> {
        let text = doc.to_string();
        let config = config::parse_text(&text, &self.file).map_err(|e| format!("{e:#}"))?;
        let live = REGISTRY
            .iter()
            .all(|entry| entry.live || value_of(&doc, entry) == value_of(&self.doc, entry));
        self.doc = doc;
        self.config = config.clone();
        self.saver.save(&self.path, text);
        Ok(Applied { config, live })
    }
}

impl Drop for Settings {
    fn drop(&mut self) {
        self.wait_for_saves();
    }
}

/// Writes the file on short-lived threads, the newest text last.
struct Saver {
    /// The number of the last save asked for.
    asked: u64,
    /// The number of the last save written: an older one still waiting is not written.
    written: Arc<Mutex<u64>>,
    handle: Option<JoinHandle<()>>,
    errors_tx: Sender<String>,
    errors: Receiver<String>,
}

impl Saver {
    fn new() -> Self {
        let (errors_tx, errors) = channel();
        Self {
            asked: 0,
            written: Arc::new(Mutex::new(0)),
            handle: None,
            errors_tx,
            errors,
        }
    }

    fn save(&mut self, path: &Path, text: String) {
        self.asked += 1;
        let number = self.asked;
        let written = Arc::clone(&self.written);
        let errors = self.errors_tx.clone();
        let path = path.to_owned();
        let spawned = std::thread::Builder::new()
            .name("config save".to_owned())
            .spawn(move || {
                let mut last = written.lock().unwrap_or_else(PoisonError::into_inner);
                if *last > number {
                    return;
                }
                *last = number;
                if let Err(e) = write(&path, &text) {
                    let file = crate::format::safe(path.display());
                    let _ = errors.send(format!("cannot save {file}: {e}"));
                }
            });
        match spawned {
            Ok(handle) => self.handle = Some(handle),
            Err(e) => {
                let _ = self
                    .errors_tx
                    .send(format!("cannot save the settings: {e}"));
            }
        }
    }
}

/// Writes `text` beside the file and renames it over it, so the file is never left half
/// written. A link is followed: the file it points to is replaced, not the link.
fn write(path: &Path, text: &str) -> std::io::Result<()> {
    let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut partial = path.clone().into_os_string();
    partial.push(".partial");
    let partial = PathBuf::from(partial);
    std::fs::write(&partial, text)?;
    std::fs::rename(&partial, &path).inspect_err(|_| {
        let _ = std::fs::remove_file(&partial);
    })
}

/// `table.key` as `(table, key)`.
fn split(key: &'static str) -> (&'static str, &'static str) {
    key.split_once('.').expect("registry keys are table.key")
}

/// `entry`'s value in `doc` as TOML text, else its default.
fn value_of(doc: &DocumentMut, entry: &Entry) -> String {
    let (table, name) = split(entry.key);
    let value = doc
        .get(table)
        .and_then(Item::as_table_like)
        .and_then(|table| table.get(name))
        .and_then(Item::as_value);
    match value {
        Some(value) => {
            let mut value = value.clone();
            value.decor_mut().clear();
            value.to_string()
        }
        None => entry.default.to_owned(),
    }
}

/// Removes `key` from `doc`. The comment lines above it are kept: they move to what
/// follows it.
fn remove(doc: &mut DocumentMut, key: &'static str) {
    let (table_name, name) = split(key);
    let root = doc.as_table_mut();
    let Some(item) = root.get_mut(table_name) else {
        return;
    };
    let Some(table) = item.as_table_mut() else {
        // An inline table (`stems = { knobs = "raw" }`) has no comment lines in it.
        if let Some(table) = item.as_table_like_mut() {
            table.remove(name);
        }
        return;
    };
    let next = table
        .iter()
        .map(|(key, _)| key)
        .skip_while(|key| *key != name)
        .nth(1)
        .map(str::to_owned);
    let Some((removed, _)) = table.remove_entry(name) else {
        return;
    };
    let comments = removed
        .leaf_decor()
        .prefix()
        .and_then(|prefix| prefix.as_str())
        .filter(|prefix| prefix.contains('#'))
        .map(str::to_owned);
    let Some(comments) = comments else {
        return;
    };
    if let Some(mut next) = next.and_then(|next| table.key_mut(&next)) {
        let decor = next.leaf_decor_mut();
        let own = decor.prefix().and_then(|p| p.as_str()).unwrap_or("");
        decor.set_prefix(format!("{comments}{own}"));
        return;
    }
    // The last key of its table: the comments go before the next table, else at the end.
    let position = table.position();
    let next_table = root
        .iter_mut()
        .filter_map(|(_, item)| item.as_table_mut())
        .filter(|table| table.position() > position)
        .min_by_key(|table| table.position());
    match next_table {
        Some(next) => {
            let decor = next.decor_mut();
            let own = decor.prefix().and_then(|p| p.as_str()).unwrap_or("");
            decor.set_prefix(format!("{comments}{own}"));
        }
        None => {
            let own = doc.trailing().as_str().unwrap_or("").to_owned();
            doc.set_trailing(format!("{comments}{own}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = "\
# my setup
[mixer]
crossfader = false   # no fader on the booth mixer

# stems, on the laptop
[stems]
enabled = true
knobs = \"vdj\"       # the VirtualDJ layout
kicks_hz = 300
";

    /// A settings file at a temporary path holding `text`.
    fn settings(text: &str) -> (tempfile::TempDir, PathBuf, Settings) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, text).unwrap();
        let config = config::parse_text(text, "config.toml").unwrap();
        let settings = Settings::load(path.clone(), &config);
        (dir, path, settings)
    }

    fn saved(path: &Path, settings: &mut Settings) -> String {
        settings.wait_for_saves();
        std::fs::read_to_string(path).unwrap()
    }

    #[test]
    fn edits_keep_comments() {
        let (_dir, path, mut settings) = settings(FILE);
        let applied = settings.set("stems.knobs", "\"raw\"").unwrap();
        assert_eq!(applied.config.stems.knobs, engine::StemKnobs::Raw);
        let text = saved(&path, &mut settings);
        let expected = FILE.replace("knobs = \"vdj\"", "knobs = \"raw\"");
        assert_eq!(text, expected);
        assert!(text.contains("# my setup\n") && text.contains("# stems, on the laptop\n"));
        assert!(!path.with_file_name("config.toml.partial").exists());
    }

    #[test]
    fn a_new_key_is_inserted_in_its_table() {
        let (_dir, path, mut settings) = settings(FILE);
        let applied = settings.set("ui.waveform_beats", "24").unwrap();
        assert_eq!(applied.config.ui.waveform_beats, 24.0);
        let text = saved(&path, &mut settings);
        assert!(text.starts_with(FILE), "{text}");
        let doc: toml::Table = text.parse().unwrap();
        assert_eq!(doc["ui"]["waveform_beats"].as_integer(), Some(24));
        // A key of a table already there goes in it.
        settings.set("mixer.crossfader", "true").unwrap();
        settings.set("stems.cache_gb", "20").unwrap();
        let text = saved(&path, &mut settings);
        let config = config::parse_text(&text, "config.toml").unwrap();
        assert!(config.crossfader);
        assert_eq!(config.stems.cache_gb, 20.0);
        assert_eq!(text.matches("[stems]").count(), 1);
        assert!(text.contains("crossfader = true   # no fader on the booth mixer\n"));
    }

    #[test]
    fn a_bad_value_is_refused() {
        let (_dir, path, mut settings) = settings(FILE);
        let error = settings.set("stems.kicks_hz", "10").unwrap_err();
        assert!(
            error.contains("stems.kicks_hz: expected a number from 50 to 1000"),
            "{error}"
        );
        assert_eq!(saved(&path, &mut settings), FILE);
        let row = settings
            .rows()
            .into_iter()
            .find(|row| row.key == "stems.kicks_hz")
            .unwrap();
        assert_eq!(row.error.as_deref(), Some(error.as_str()));
        assert_eq!(row.value, "300");
        // Not TOML at all, and an unknown key.
        assert!(settings.set("stems.knobs", "raw").is_err());
        assert!(settings.set("stems.nope", "1").is_err());
        // A good value clears the row's error.
        settings.set("stems.kicks_hz", "200").unwrap();
        let rows = settings.rows();
        let row = rows.iter().find(|row| row.key == "stems.kicks_hz").unwrap();
        assert_eq!(row.error, None);
    }

    #[test]
    fn live_and_next_launch() {
        let (_dir, _path, mut settings) = settings(FILE);
        assert!(settings.set("stems.knobs", "\"raw\"").unwrap().live);
        let row = |settings: &Settings, key: &str| {
            settings
                .rows()
                .into_iter()
                .find(|row| row.key == key)
                .unwrap()
        };
        assert!(!row(&settings, "stems.knobs").pending);
        let applied = settings.set("audio.device", "\"DDJ-SB3\"").unwrap();
        assert!(!applied.live);
        assert_eq!(applied.config.audio.device.as_deref(), Some("DDJ-SB3"));
        let device = row(&settings, "audio.device");
        assert!(device.pending && !device.live);
        assert_eq!(device.value, "\"DDJ-SB3\"");
        // Back to what it was at launch, it is no longer pending.
        settings.set("audio.device", "").unwrap();
        assert!(!row(&settings, "audio.device").pending);
    }

    #[test]
    fn reset_all_removes_known_keys() {
        let text = format!("# keep me\n{FILE}\n[ui]\n# zoom\nwaveform_beats = 32\n");
        let (_dir, path, mut settings) = settings(&text);
        let applied = settings.reset_all().unwrap();
        assert!(!applied.live, "stems.enabled applies at the next launch");
        assert_eq!(applied.config, Config::default());
        let text = saved(&path, &mut settings);
        assert_eq!(
            config::parse_text(&text, "config.toml").unwrap(),
            Config::default()
        );
        for comment in [
            "# keep me",
            "# my setup",
            "# stems, on the laptop",
            "# zoom",
        ] {
            assert!(text.contains(comment), "{comment} lost: {text}");
        }
        assert!(settings.rows().iter().all(|row| row.value == row.default));
    }

    #[test]
    fn a_missing_file_is_created() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("open-jockey").join("config.toml");
        let mut settings = Settings::load(path.clone(), &Config::default());
        settings.set("sampler.volume", "0.5").unwrap();
        let text = saved(&path, &mut settings);
        let config = config::parse_text(&text, "config.toml").unwrap();
        assert_eq!(config.sampler.volume, 0.5);
    }

    #[test]
    fn registry_matches_the_docs() {
        let docs = include_str!("../../../../docs/config.md");
        let mut documented = Vec::new();
        for line in docs.lines().filter(|line| line.starts_with("| `")) {
            let cell = line.split('|').nth(1).unwrap().trim();
            let keys: Vec<&str> = cell.split('`').skip(1).step_by(2).collect();
            match keys.as_slice() {
                [key] => documented.push((*key).to_owned()),
                // `fx.deck1` to `fx.deck4`.
                [first, last] => {
                    let stem = first.trim_end_matches(|c: char| c.is_ascii_digit());
                    let from: u32 = first[stem.len()..].parse().unwrap();
                    let to: u32 = last[stem.len()..].parse().unwrap();
                    documented.extend((from..=to).map(|n| format!("{stem}{n}")));
                }
                _ => panic!("unexpected key cell {cell}"),
            }
        }
        let registry: Vec<String> = REGISTRY.iter().map(|e| e.key.to_owned()).collect();
        assert_eq!(registry, documented);
        // Each default parses as its key's value.
        for entry in REGISTRY.iter().filter(|entry| !entry.default.is_empty()) {
            let (table, name) = split(entry.key);
            let text = format!("[{table}]\n{name} = {}\n", entry.default);
            assert!(
                config::parse_text(&text, "config.toml").is_ok(),
                "{}",
                entry.key
            );
        }
    }
}
