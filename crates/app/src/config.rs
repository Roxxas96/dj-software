//! `config.toml`: the mixer, sampler and pad-size settings, in the per-user configuration
//! folder (or the file given with `play --config`). Every section and key is optional;
//! an invalid value is an error naming the file, the key and the problem.

use std::path::{Path, PathBuf};

use anyhow::{Context, anyhow};
use toml::{Table, Value};

use crate::format::safe;

/// How many sampler pads (and pad-size entries) there are.
const PADS: usize = 8;
/// The most beats a beat jump pad may move.
const MAX_JUMP_BEATS: f64 = 512.0;
/// The shortest and longest loop roll, in beats.
const ROLL_RANGE: (f64, f64) = (1.0 / 32.0, 64.0);

/// The parsed configuration; a key left out keeps its default.
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    /// Whether the crossfader acts; off, every channel plays at full level.
    pub crossfader: bool,
    pub sampler: SamplerConfig,
    /// Beats for the beat jump pads, when the file sets them.
    pub beat_jump_pads: Option<[f64; PADS]>,
    /// Beats for the beat jump pads with SHIFT held, when the file sets them.
    pub beat_jump_shift_pads: Option<[f64; PADS]>,
    /// Beats for the loop roll pads, when the file sets them.
    pub loop_roll_pads: Option<[f64; PADS]>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            crossfader: true,
            sampler: SamplerConfig::default(),
            beat_jump_pads: None,
            beat_jump_shift_pads: None,
            loop_roll_pads: None,
        }
    }
}

/// The `[sampler]` section.
#[derive(Debug, Clone, PartialEq)]
pub struct SamplerConfig {
    /// Sampler volume, 0 to 1.
    pub volume: f32,
    /// What pads 1 to 8 load, in order; `None` (a `{}` entry) and pads past the end stay
    /// empty.
    pub pads: Vec<Option<PadSource>>,
}

impl Default for SamplerConfig {
    fn default() -> Self {
        Self {
            volume: 1.0,
            pads: Vec::new(),
        }
    }
}

/// Where a sampler pad's sample comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PadSource {
    /// An audio file (`~/` already expanded).
    File(PathBuf),
    /// A library track id.
    Track(i64),
}

/// The default configuration file, in the per-user configuration folder.
fn default_path() -> Option<PathBuf> {
    crate::legacy_dirs::migrate();
    controller::builtin::config_dir().map(|dir| dir.join("config.toml"))
}

/// Loads the configuration from `path`, or from the default file when `None`; a missing
/// default file means [`Config::default`], a missing explicit one is an error.
pub fn load(path: Option<&Path>) -> anyhow::Result<Config> {
    let (path, explicit) = match path {
        Some(path) => (path.to_owned(), true),
        None => match default_path() {
            Some(path) => (path, false),
            None => return Ok(Config::default()),
        },
    };
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && !explicit => {
            return Ok(Config::default());
        }
        Err(e) => {
            return Err(e).with_context(|| format!("reading {}", safe(path.display())));
        }
    };
    parse(&text, &safe(path.display()), home_dir().as_deref())
}

fn home_dir() -> Option<PathBuf> {
    directories::BaseDirs::new().map(|dirs| dirs.home_dir().to_path_buf())
}

/// An error `<file>: <key>: <problem>`.
fn invalid(file: &str, key: &str, problem: impl std::fmt::Display) -> anyhow::Error {
    anyhow!("{file}: {}: {problem}", safe(key))
}

/// Parses `text`; `file` names it in errors and `home` is what `~/` stands for.
fn parse(text: &str, file: &str, home: Option<&Path>) -> anyhow::Result<Config> {
    let root: Table = toml::from_str(text).map_err(|e| {
        let line = e
            .span()
            .map(|span| text[..span.start.min(text.len())].matches('\n').count() + 1);
        match line {
            Some(line) => anyhow!("{file}: line {line}: {}", safe(e.message())),
            None => anyhow!("{file}: {}", safe(e.message())),
        }
    })?;
    let mut config = Config::default();
    for (name, value) in &root {
        match name.as_str() {
            "mixer" => {
                let mixer = section(file, name, value)?;
                for (key, value) in mixer {
                    match key.as_str() {
                        "crossfader" => {
                            config.crossfader = value.as_bool().ok_or_else(|| {
                                invalid(file, "mixer.crossfader", "expected true or false")
                            })?;
                        }
                        _ => return Err(unknown(file, "mixer", key)),
                    }
                }
            }
            "sampler" => config.sampler = parse_sampler(section(file, name, value)?, file, home)?,
            "beat_jump" => {
                for (key, value) in section(file, name, value)? {
                    let path = format!("beat_jump.{key}");
                    match key.as_str() {
                        "pads" => {
                            config.beat_jump_pads = Some(sizes(file, &path, value, jump_beats)?);
                        }
                        "shift_pads" => {
                            config.beat_jump_shift_pads =
                                Some(sizes(file, &path, value, jump_beats)?);
                        }
                        _ => return Err(unknown(file, "beat_jump", key)),
                    }
                }
            }
            "loop_roll" => {
                for (key, value) in section(file, name, value)? {
                    match key.as_str() {
                        "pads" => {
                            config.loop_roll_pads =
                                Some(sizes(file, "loop_roll.pads", value, roll_beats)?);
                        }
                        _ => return Err(unknown(file, "loop_roll", key)),
                    }
                }
            }
            _ => return Err(invalid(file, name, "unknown key")),
        }
    }
    Ok(config)
}

fn unknown(file: &str, section: &str, key: &str) -> anyhow::Error {
    invalid(file, &format!("{section}.{key}"), "unknown key")
}

fn section<'a>(file: &str, name: &str, value: &'a Value) -> anyhow::Result<&'a Table> {
    value
        .as_table()
        .ok_or_else(|| invalid(file, name, "expected a table"))
}

/// A TOML integer or float.
fn number(value: &Value) -> Option<f64> {
    match value {
        Value::Integer(i) => Some(*i as f64),
        Value::Float(f) => Some(*f),
        _ => None,
    }
}

fn jump_beats(beats: f64) -> Result<(), String> {
    if !beats.is_finite() || beats == 0.0 || beats.abs() > MAX_JUMP_BEATS {
        return Err(format!(
            "expected beats that are not zero, at most {MAX_JUMP_BEATS} either way"
        ));
    }
    Ok(())
}

fn roll_beats(beats: f64) -> Result<(), String> {
    if !(ROLL_RANGE.0..=ROLL_RANGE.1).contains(&beats) {
        return Err(format!(
            "expected beats from 1/32 (0.03125) to {}",
            ROLL_RANGE.1
        ));
    }
    Ok(())
}

/// Exactly 8 numbers, each accepted by `check`.
fn sizes(
    file: &str,
    key: &str,
    value: &Value,
    check: fn(f64) -> Result<(), String>,
) -> anyhow::Result<[f64; PADS]> {
    let items = value
        .as_array()
        .ok_or_else(|| invalid(file, key, "expected an array of 8 numbers"))?;
    if items.len() != PADS {
        return Err(invalid(
            file,
            key,
            format!("expected exactly {PADS} numbers, got {}", items.len()),
        ));
    }
    let mut out = [0.0; PADS];
    for (i, item) in items.iter().enumerate() {
        let at = format!("{key}[{i}]");
        let beats = number(item).ok_or_else(|| invalid(file, &at, "expected a number"))?;
        check(beats).map_err(|problem| invalid(file, &at, problem))?;
        out[i] = beats;
    }
    Ok(out)
}

fn parse_sampler(table: &Table, file: &str, home: Option<&Path>) -> anyhow::Result<SamplerConfig> {
    let mut sampler = SamplerConfig::default();
    for (key, value) in table {
        match key.as_str() {
            "volume" => {
                let volume = number(value)
                    .filter(|v| (0.0..=1.0).contains(v))
                    .ok_or_else(|| {
                        invalid(file, "sampler.volume", "expected a number from 0 to 1")
                    })?;
                sampler.volume = volume as f32;
            }
            "pads" => sampler.pads = parse_pads(value, file, home)?,
            _ => return Err(unknown(file, "sampler", key)),
        }
    }
    Ok(sampler)
}

fn parse_pads(
    value: &Value,
    file: &str,
    home: Option<&Path>,
) -> anyhow::Result<Vec<Option<PadSource>>> {
    let items = value
        .as_array()
        .ok_or_else(|| invalid(file, "sampler.pads", "expected an array of tables"))?;
    if items.len() > PADS {
        return Err(invalid(
            file,
            "sampler.pads",
            format!("at most {PADS} pads, got {}", items.len()),
        ));
    }
    items
        .iter()
        .enumerate()
        .map(|(i, item)| {
            let at = format!("sampler.pads[{i}]");
            let table = item.as_table().ok_or_else(|| {
                invalid(
                    file,
                    &at,
                    "expected a table like { file = \"...\" }, { track = 1 } or {}",
                )
            })?;
            if let Some(key) = table
                .keys()
                .find(|k| !matches!(k.as_str(), "file" | "track"))
            {
                return Err(invalid(file, &format!("{at}.{key}"), "unknown key"));
            }
            match (table.get("file"), table.get("track")) {
                (Some(_), Some(_)) => Err(invalid(file, &at, "has both `file` and `track`")),
                (None, None) => Ok(None),
                (Some(file_value), None) => {
                    let key = format!("{at}.file");
                    let text = file_value
                        .as_str()
                        .ok_or_else(|| invalid(file, &key, "expected a path string"))?;
                    expand_path(text, home)
                        .map(|path| Some(PadSource::File(path)))
                        .map_err(|problem| invalid(file, &key, problem))
                }
                (None, Some(track)) => track
                    .as_integer()
                    .filter(|id| *id >= 0)
                    .map(|id| Some(PadSource::Track(id)))
                    .ok_or_else(|| {
                        invalid(
                            file,
                            &format!("{at}.track"),
                            "expected a track id (an integer)",
                        )
                    }),
            }
        })
        .collect()
}

/// A sample path: absolute, or starting with `~/`, which stands for `home`.
fn expand_path(text: &str, home: Option<&Path>) -> Result<PathBuf, String> {
    if let Some(rest) = text.strip_prefix("~/") {
        let home = home.ok_or("cannot find the home folder for `~/`")?;
        return Ok(home.join(rest));
    }
    let path = Path::new(text);
    if path.is_absolute() {
        Ok(path.to_owned())
    } else {
        Err("expected an absolute path or one starting with ~/".to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The example in docs/config.md.
    const EXAMPLE: &str = r#"
[mixer]
crossfader = true            # false: the crossfader is ignored, every channel at full level

[sampler]
volume = 1.0                 # 0 to 1
pads = [
  { file = "~/Samples/airhorn.wav" },
  { track = 1234 },
]

[beat_jump]
pads = [-4, 4, -8, 8, -16, 16, -32, 32]
shift_pads = [-1, 1, -2, 2, -4, 4, -64, 64]

[loop_roll]
pads = [0.0625, 0.125, 0.25, 0.5, 1, 2, 4, 8]
"#;

    fn home() -> PathBuf {
        PathBuf::from("/home/dj")
    }

    fn parse_text(text: &str) -> anyhow::Result<Config> {
        parse(text, "config.toml", Some(&home()))
    }

    fn error_of(text: &str) -> String {
        parse_text(text).unwrap_err().to_string()
    }

    #[test]
    fn missing_config_is_default() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(parse_text("").unwrap(), Config::default());
        // A missing default file is the default; an explicit one must exist.
        let error = load(Some(&dir.path().join("none.toml"))).unwrap_err();
        assert!(error.to_string().contains("none.toml"), "{error}");
    }

    #[test]
    fn full_example_parses() {
        let config = parse_text(EXAMPLE).unwrap();
        assert!(config.crossfader);
        assert_eq!(config.sampler.volume, 1.0);
        assert_eq!(
            config.sampler.pads,
            [
                Some(PadSource::File(home().join("Samples/airhorn.wav"))),
                Some(PadSource::Track(1234))
            ]
        );
        assert_eq!(
            config.beat_jump_pads,
            Some([-4.0, 4.0, -8.0, 8.0, -16.0, 16.0, -32.0, 32.0])
        );
        assert_eq!(
            config.beat_jump_shift_pads,
            Some([-1.0, 1.0, -2.0, 2.0, -4.0, 4.0, -64.0, 64.0])
        );
        assert_eq!(
            config.loop_roll_pads,
            Some([0.0625, 0.125, 0.25, 0.5, 1.0, 2.0, 4.0, 8.0])
        );
    }

    #[test]
    fn an_empty_table_is_an_empty_pad() {
        let config = parse_text("[sampler]\npads = [{}, {}, {}, {}, { track = 3 }]").unwrap();
        assert_eq!(
            config.sampler.pads,
            [None, None, None, None, Some(PadSource::Track(3))]
        );
    }

    #[test]
    fn bad_values_name_the_key() {
        let cases: &[(&str, &str)] = &[
            ("mystery = 1", "config.toml: mystery: unknown key"),
            ("[mixer]\nfade = 1", "config.toml: mixer.fade: unknown key"),
            (
                "[mixer]\ncrossfader = 1",
                "config.toml: mixer.crossfader: expected true or false",
            ),
            (
                "[sampler]\nvolume = 1.5",
                "config.toml: sampler.volume: expected a number from 0 to 1",
            ),
            (
                "[sampler]\nvolume = \"loud\"",
                "config.toml: sampler.volume: expected a number from 0 to 1",
            ),
            (
                "[sampler]\npads = [{ file = \"/a.wav\", track = 1 }]",
                "config.toml: sampler.pads[0]: has both `file` and `track`",
            ),
            (
                "[sampler]\npads = [{ file = \"relative.wav\" }]",
                "config.toml: sampler.pads[0].file: expected an absolute path",
            ),
            (
                "[sampler]\npads = [{ track = \"x\" }]",
                "config.toml: sampler.pads[0].track: expected a track id",
            ),
            (
                "[sampler]\npads = [{ track = 1, loud = 1 }]",
                "config.toml: sampler.pads[0].loud: unknown key",
            ),
            (
                "[sampler]\npads = [{track=1},{track=1},{track=1},{track=1},{track=1},{track=1},{track=1},{track=1},{track=1}]",
                "config.toml: sampler.pads: at most 8 pads, got 9",
            ),
            (
                "[beat_jump]\npads = [1, 2]",
                "config.toml: beat_jump.pads: expected exactly 8 numbers, got 2",
            ),
            (
                "[beat_jump]\npads = [-4, 4, 0, 8, -16, 16, -32, 32]",
                "config.toml: beat_jump.pads[2]: expected beats that are not zero",
            ),
            (
                "[beat_jump]\nshift_pads = [-4, 4, 513, 8, -16, 16, -32, 32]",
                "config.toml: beat_jump.shift_pads[2]:",
            ),
            (
                "[beat_jump]\npads = [-4, 4, \"a\", 8, -16, 16, -32, 32]",
                "config.toml: beat_jump.pads[2]: expected a number",
            ),
            (
                "[loop_roll]\npads = [0.01, 1, 1, 1, 1, 1, 1, 1]",
                "config.toml: loop_roll.pads[0]: expected beats from 1/32",
            ),
            (
                "[loop_roll]\npads = [1, 1, 1, 1, 1, 1, 1, 65]",
                "config.toml: loop_roll.pads[7]:",
            ),
        ];
        for (text, expected) in cases {
            let error = error_of(text);
            assert!(error.starts_with(expected), "{text:?}: {error}");
        }
        let e = error_of("[mixer]\n\ncrossfader = \"open");
        assert!(e.starts_with("config.toml: line 3: "), "{e}");
    }

    #[test]
    fn tilde_paths_expand() {
        assert_eq!(
            expand_path("~/a/b.wav", Some(&home())).unwrap(),
            PathBuf::from("/home/dj/a/b.wav")
        );
        assert!(expand_path("~/a.wav", None).is_err());
        assert!(expand_path("~other/a.wav", Some(&home())).is_err());
    }
}
