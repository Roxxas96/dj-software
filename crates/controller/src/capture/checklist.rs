//! The capture checklist: a TOML file of sections and steps, expanded per deck
//! and per SHIFT variant into one flat, ordered list of [`Step`]s.
//!
//! Step keys: `id`, `prompt`, `kind`, optional `prepare` (done before listening
//! starts), `shift` (also capture a SHIFT variant) and `held_shift` (this step is
//! itself done with SHIFT held). A section's optional `shift_control` (an id or a
//! list of ids, one per layer) names the earlier SHIFT button steps whose notes
//! are the modifier in its SHIFT steps.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use super::fixture::Source;

/// How a control behaves, which decides when its capture step is complete.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StepKind {
    /// Done after a 400 ms quiet gap.
    Button,
    /// Eight pads: done after 8 distinct note-on notes and a 1500 ms quiet gap.
    Pads,
    /// Knob, fader or slider: done after a 1500 ms quiet gap.
    Continuous,
    /// Jog wheel or endless encoder: done after a 1500 ms quiet gap.
    Jog,
}

impl StepKind {
    pub fn as_str(self) -> &'static str {
        match self {
            StepKind::Button => "button",
            StepKind::Pads => "pads",
            StepKind::Continuous => "continuous",
            StepKind::Jog => "jog",
        }
    }
}

/// One expanded capture step. A SHIFT variant is its own step with id suffix `.shift`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub id: String,
    /// Section name, with `{deck}` substituted.
    pub section: String,
    pub prompt: String,
    /// Shown before the first step of a section (and again when it changes).
    pub setup: Option<String>,
    pub kind: StepKind,
    /// Something to do before listening starts (e.g. select a pad mode); the
    /// CLI waits for Enter first so that action is not recorded.
    pub prepare: Option<String>,
    /// True when the step is captured with SHIFT held (a SHIFT variant or a
    /// `held_shift` step).
    pub shift: bool,
    /// For SHIFT steps: id of the earlier step that captured this side's SHIFT
    /// button, whose note is excluded from the record.
    pub shift_control: Vec<String>,
}

/// A loaded checklist: an optional controller name and the expanded steps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checklist {
    pub controller: Option<String>,
    /// Sources never recorded (e.g. a controller's parasitic messages).
    pub ignore: Vec<Source>,
    pub steps: Vec<Step>,
}

#[derive(Debug, thiserror::Error)]
pub enum ChecklistError {
    #[error("invalid checklist TOML: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("duplicate step id {0:?}")]
    DuplicateId(String),
    #[error("step {step:?}: shift_control {shift_control:?} {problem}")]
    ShiftControl {
        step: String,
        shift_control: String,
        problem: &'static str,
    },
    #[error("ignore source {ignored}: {problem}")]
    Ignore {
        ignored: Source,
        problem: &'static str,
    },
    #[error("section {section:?}: {problem}")]
    Section { section: String, problem: String },
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileDef {
    controller: Option<String>,
    #[serde(default)]
    ignore: IgnoreDef,
    #[serde(rename = "section", default)]
    sections: Vec<SectionDef>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SectionDef {
    name: String,
    decks: Option<Vec<u8>>,
    setup: Option<String>,
    /// Id(s) of the SHIFT button steps whose notes are the modifier in this
    /// section's SHIFT steps; each must be captured earlier in the checklist.
    #[serde(default)]
    shift_control: OneOrMany,
    #[serde(rename = "step", default)]
    steps: Vec<StepDef>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct IgnoreDef {
    #[serde(default)]
    sources: Vec<Source>,
}

/// A string or a list of strings.
#[derive(Debug, Default, Deserialize)]
#[serde(untagged)]
enum OneOrMany {
    #[default]
    None,
    One(String),
    Many(Vec<String>),
}

impl OneOrMany {
    fn as_slice(&self) -> &[String] {
        match self {
            OneOrMany::None => &[],
            OneOrMany::One(one) => std::slice::from_ref(one),
            OneOrMany::Many(many) => many,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StepDef {
    id: String,
    prompt: String,
    kind: StepKind,
    prepare: Option<String>,
    /// Also capture a SHIFT variant of this step.
    #[serde(default)]
    shift: bool,
    /// This step itself is done with SHIFT held (prompt says so); no variant.
    #[serde(default)]
    held_shift: bool,
}

/// Parses a checklist and returns its expanded, ordered steps.
pub fn load_checklist(text: &str) -> Result<Vec<Step>, ChecklistError> {
    parse_checklist(text).map(|c| c.steps)
}

/// Parses a checklist, keeping the optional top-level `controller` name.
pub fn parse_checklist(text: &str) -> Result<Checklist, ChecklistError> {
    let file: FileDef = toml::from_str(text)?;
    for &source in &file.ignore.sources {
        let problem = if source.status < 0x80 || source.status >= 0xF0 {
            Some("status must be a channel message status (0x80..=0xEF)")
        } else if source.data1 > 0x7F {
            Some("data1 must be 0..=127")
        } else {
            None
        };
        if let Some(problem) = problem {
            return Err(ChecklistError::Ignore {
                ignored: source,
                problem,
            });
        }
    }
    let mut steps = Vec::new();
    for section in &file.sections {
        expand_section(section, &mut steps)?;
    }
    let mut seen = HashSet::new();
    for step in &steps {
        if !seen.insert(step.id.as_str()) {
            return Err(ChecklistError::DuplicateId(step.id.clone()));
        }
    }
    check_shift_controls(&steps)?;
    Ok(Checklist {
        controller: file.controller,
        ignore: file.ignore.sources,
        steps,
    })
}

/// Each `shift_control` id must name an unshifted button step that comes earlier,
/// so its note is known when the SHIFT step is captured.
fn check_shift_controls(steps: &[Step]) -> Result<(), ChecklistError> {
    for (i, step) in steps.iter().enumerate() {
        for shift_control in &step.shift_control {
            let error = |problem| ChecklistError::ShiftControl {
                step: step.id.clone(),
                shift_control: shift_control.clone(),
                problem,
            };
            let Some(j) = steps.iter().position(|s| &s.id == shift_control) else {
                return Err(error("is not a step in this checklist"));
            };
            if j > i {
                return Err(error("must come before this step"));
            }
            let target = &steps[j];
            if target.shift || target.kind != StepKind::Button {
                return Err(error("must be an unshifted button step"));
            }
        }
    }
    Ok(())
}

const DECK: &str = "{deck}";

fn expand_section(section: &SectionDef, out: &mut Vec<Step>) -> Result<(), ChecklistError> {
    let problem = |problem: &str| ChecklistError::Section {
        section: section.name.clone(),
        problem: problem.to_owned(),
    };
    let decks: Vec<Option<u8>> = match &section.decks {
        Some(decks) if decks.is_empty() => return Err(problem("`decks` is empty")),
        Some(decks) => decks.iter().copied().map(Some).collect(),
        None => {
            let uses_deck = section.name.contains(DECK)
                || section.setup.as_deref().is_some_and(|s| s.contains(DECK))
                || section
                    .shift_control
                    .as_slice()
                    .iter()
                    .any(|s| s.contains(DECK))
                || section.steps.iter().any(|s| {
                    s.id.contains(DECK)
                        || s.prompt.contains(DECK)
                        || s.prepare.as_deref().is_some_and(|p| p.contains(DECK))
                });
            if uses_deck {
                return Err(problem("uses {deck} but has no `decks`"));
            }
            vec![None]
        }
    };
    for deck in decks {
        let sub = |text: &str| match deck {
            Some(deck) => text.replace(DECK, &deck.to_string()),
            None => text.to_owned(),
        };
        let name = sub(&section.name);
        let setup = section.setup.as_deref().map(sub);
        let shift_controls: Vec<String> = section
            .shift_control
            .as_slice()
            .iter()
            .map(|s| sub(s))
            .collect();
        for def in &section.steps {
            if def.id.trim().is_empty() {
                return Err(problem("a step has an empty id"));
            }
            if def.shift && def.held_shift {
                return Err(problem(&format!(
                    "step {:?} sets both `shift` and `held_shift`",
                    def.id
                )));
            }
            let base = Step {
                id: sub(&def.id),
                section: name.clone(),
                prompt: sub(&def.prompt),
                setup: setup.clone(),
                prepare: def.prepare.as_deref().map(sub),
                kind: def.kind,
                shift: def.held_shift,
                shift_control: if def.held_shift {
                    shift_controls.clone()
                } else {
                    Vec::new()
                },
            };
            let shifted = def.shift.then(|| Step {
                id: format!("{}.shift", base.id),
                prompt: format!("Hold SHIFT and keep holding it, then: {}", base.prompt),
                shift: true,
                shift_control: shift_controls.clone(),
                ..base.clone()
            });
            out.push(base);
            out.extend(shifted);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const TWO_STEPS: &str = r#"
[[section]]
name = "Left deck {deck}"
decks = [1, 3]
setup = "Switch the LEFT side to deck {deck}."
[[section.step]]
id = "deck{deck}.play"
prompt = "Press PLAY/PAUSE"
kind = "button"
shift = true
[[section.step]]
id = "deck{deck}.cue"
prompt = "Press CUE"
kind = "button"
"#;

    #[test]
    fn checklist_expands_decks_and_shift() {
        let steps = load_checklist(TWO_STEPS).unwrap();
        let ids: Vec<&str> = steps.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "deck1.play",
                "deck1.play.shift",
                "deck1.cue",
                "deck3.play",
                "deck3.play.shift",
                "deck3.cue",
            ]
        );
        assert!(!steps[0].shift);
        assert!(steps[1].shift);
        assert!(steps[1].prompt.contains("SHIFT"));
        assert!(steps[1].prompt.contains("Press PLAY/PAUSE"));
        assert_eq!(
            steps[3].setup.as_deref(),
            Some("Switch the LEFT side to deck 3.")
        );
        assert_eq!(steps[3].section, "Left deck 3");
        assert_eq!(steps[0].kind, StepKind::Button);
    }

    #[test]
    fn checklist_without_decks_keeps_ids() {
        let text = r#"
controller = "Test"
[[section]]
name = "Global"
[[section.step]]
id = "crossfader"
prompt = "Move it"
kind = "continuous"
"#;
        let checklist = parse_checklist(text).unwrap();
        assert_eq!(checklist.controller.as_deref(), Some("Test"));
        assert_eq!(checklist.steps.len(), 1);
        assert_eq!(checklist.steps[0].id, "crossfader");
        assert_eq!(checklist.steps[0].setup, None);
    }

    #[test]
    fn checklist_rejects_duplicate_ids_and_unknown_keys() {
        let duplicate = r#"
[[section]]
name = "A"
[[section.step]]
id = "x"
prompt = "p"
kind = "button"
shift = true
[[section.step]]
id = "x.shift"
prompt = "p"
kind = "button"
"#;
        assert!(matches!(
            load_checklist(duplicate),
            Err(ChecklistError::DuplicateId(id)) if id == "x.shift"
        ));

        // A section with decks whose id lacks {deck} repeats the same id.
        let repeated = r#"
[[section]]
name = "A"
decks = [1, 2]
[[section.step]]
id = "x"
prompt = "p"
kind = "button"
"#;
        assert!(matches!(
            load_checklist(repeated),
            Err(ChecklistError::DuplicateId(_))
        ));

        let unknown_step_key = r#"
[[section]]
name = "A"
[[section.step]]
id = "x"
prompt = "p"
kind = "button"
colour = "red"
"#;
        assert!(matches!(
            load_checklist(unknown_step_key),
            Err(ChecklistError::Parse(_))
        ));

        let unknown_top_key = "sections = []\n";
        assert!(matches!(
            load_checklist(unknown_top_key),
            Err(ChecklistError::Parse(_))
        ));

        let bad_kind = r#"
[[section]]
name = "A"
[[section.step]]
id = "x"
prompt = "p"
kind = "slider"
"#;
        assert!(load_checklist(bad_kind).is_err());
    }

    #[test]
    fn checklist_rejects_deck_placeholder_without_decks() {
        let text = r#"
[[section]]
name = "A"
[[section.step]]
id = "deck{deck}.x"
prompt = "p"
kind = "button"
"#;
        assert!(matches!(
            load_checklist(text),
            Err(ChecklistError::Section { .. })
        ));
        let empty_decks = r#"
[[section]]
name = "A"
decks = []
[[section.step]]
id = "x"
prompt = "p"
kind = "button"
"#;
        assert!(matches!(
            load_checklist(empty_decks),
            Err(ChecklistError::Section { .. })
        ));
    }

    #[test]
    fn real_sb3_checklist_loads() {
        let text = include_str!("../../../../controllers/ddj-sb3/checklist.toml");
        let checklist = parse_checklist(text).unwrap();
        assert!(checklist.steps.len() > 80, "{}", checklist.steps.len());
        assert_eq!(checklist.controller.as_deref(), Some("Pioneer DDJ-SB3"));
        // The SB3's filter-position dump is knob data, handled by the
        // capture rules: nothing is ignored.
        assert!(checklist.ignore.is_empty());
        for deck in 1..=4 {
            for id in ["play", "cue", "jog_top", "tempo", "pads_hot_cue"] {
                let id = format!("deck{deck}.{id}");
                assert!(checklist.steps.iter().any(|s| s.id == id), "missing {id}");
            }
            let id = format!("channel{deck}.fader");
            assert!(checklist.steps.iter().any(|s| s.id == id), "missing {id}");
        }
        assert!(checklist.steps.iter().any(|s| s.id == "crossfader"));
        for side in ["left", "right"] {
            for id in ["1", "1.shift", "2", "3", "level"] {
                let id = format!("fx.{side}.{id}");
                assert!(checklist.steps.iter().any(|s| s.id == id), "missing {id}");
            }
        }
        for step in &checklist.steps {
            if step.kind == StepKind::Pads {
                // Pad mode is selected before listening, except the SHIFT pass
                // that stays in HOT CUE mode.
                assert_eq!(step.prepare.is_some(), !step.shift, "{}", step.id);
            }
        }
        let shifted = checklist
            .steps
            .iter()
            .find(|s| s.id == "deck2.pads_hot_cue.shift")
            .unwrap();
        assert!(shifted.shift);
        // Every SHIFT step knows its side's SHIFT button (load checks the order).
        for step in checklist.steps.iter().filter(|s| s.shift) {
            assert!(!step.shift_control.is_empty(), "{}", step.id);
        }
        assert_eq!(shifted.shift_control, ["deck2.shift"]);
        // Outside the deck sections a side may be on either layer, so SHIFT
        // steps exclude both of that side's SHIFT captures.
        for step in checklist.steps.iter().filter(|s| s.shift) {
            if step.id.starts_with("deck") || step.id.starts_with("channel") {
                continue;
            }
            let both = |a: &str, b: &str| {
                step.shift_control.contains(&a.to_owned())
                    && step.shift_control.contains(&b.to_owned())
            };
            assert!(
                both("deck1.shift", "deck3.shift") || both("deck2.shift", "deck4.shift"),
                "{}: {:?}",
                step.id,
                step.shift_control
            );
        }
        // After the mixer section the sides are on decks 3 and 4: the first
        // global section sends them back to 1 and 2 before any SHIFT step.
        let global = checklist
            .steps
            .iter()
            .find(|s| s.id == "crossfader")
            .unwrap();
        let setup = global.setup.as_deref().unwrap();
        assert!(
            setup.contains("deck 1") && setup.contains("deck 2"),
            "{setup}"
        );
        // TEMPO starts at the far end, so `first` gives the polarity.
        for deck in 1..=4 {
            let id = format!("deck{deck}.tempo");
            let tempo = checklist.steps.iter().find(|s| s.id == id).unwrap();
            assert!(
                tempo.prepare.as_deref().is_some_and(|p| p.contains("AWAY")),
                "{id}"
            );
        }
        // DECK is pressed once, as the last step of each deck section.
        for deck in 1..=4 {
            let section: Vec<&Step> = checklist
                .steps
                .iter()
                .filter(|s| s.id.starts_with(&format!("deck{deck}.")))
                .collect();
            assert_eq!(
                section.last().unwrap().id,
                format!("deck{deck}.deck_select")
            );
            assert_eq!(section.first().unwrap().id, format!("deck{deck}.shift"));
        }
        assert!(
            checklist
                .steps
                .iter()
                .all(|s| !s.id.contains('{') && !s.prompt.contains('{'))
        );
    }

    #[test]
    fn checklist_prepare_and_held_shift() {
        let text = r#"
[[section]]
name = "Left deck {deck}"
decks = [1]
[[section.step]]
id = "deck{deck}.pads"
prompt = "Tap the pads."
prepare = "Press HOT CUE on deck {deck}."
kind = "pads"
[[section.step]]
id = "deck{deck}.pads.shift"
prompt = "Hold SHIFT and tap the pads."
kind = "pads"
held_shift = true
"#;
        let steps = load_checklist(text).unwrap();
        assert_eq!(steps.len(), 2);
        assert_eq!(
            steps[0].prepare.as_deref(),
            Some("Press HOT CUE on deck 1.")
        );
        assert!(!steps[0].shift);
        assert_eq!(steps[1].id, "deck1.pads.shift");
        assert!(steps[1].shift);
        assert_eq!(steps[1].prompt, "Hold SHIFT and tap the pads.");
        assert_eq!(steps[1].prepare, None);

        let both = text.replace("held_shift = true", "held_shift = true\nshift = true");
        assert!(matches!(
            load_checklist(&both),
            Err(ChecklistError::Section { .. })
        ));
    }

    #[test]
    fn checklist_shift_control_must_precede() {
        let ok = r#"
[[section]]
name = "Left deck {deck}"
decks = [1, 3]
shift_control = "deck{deck}.shift"
[[section.step]]
id = "deck{deck}.shift"
prompt = "Press SHIFT."
kind = "button"
[[section.step]]
id = "deck{deck}.play"
prompt = "Press PLAY."
kind = "button"
shift = true
[[section.step]]
id = "deck{deck}.pads.shift"
prompt = "Hold SHIFT and tap the pads."
kind = "pads"
held_shift = true
"#;
        let steps = load_checklist(ok).unwrap();
        let find = |id: &str| steps.iter().find(|s| s.id == id).unwrap();
        assert!(find("deck3.play").shift_control.is_empty());
        assert_eq!(find("deck3.play.shift").shift_control, ["deck3.shift"]);
        assert_eq!(find("deck1.pads.shift").shift_control, ["deck1.shift"]);

        // List form: every listed id must come earlier.
        let list = r#"
[[section]]
name = "Left deck {deck}"
decks = [1, 3]
[[section.step]]
id = "deck{deck}.shift"
prompt = "Press SHIFT."
kind = "button"

[[section]]
name = "Browser"
shift_control = ["deck1.shift", "deck3.shift"]
[[section.step]]
id = "load.left"
prompt = "Press LOAD."
kind = "button"
shift = true
"#;
        let steps = load_checklist(list).unwrap();
        assert_eq!(steps[3].id, "load.left.shift");
        assert_eq!(steps[3].shift_control, ["deck1.shift", "deck3.shift"]);
        let bad = list.replace("\"deck3.shift\"]", "\"deck5.shift\"]");
        assert!(matches!(
            load_checklist(&bad),
            Err(ChecklistError::ShiftControl { shift_control, .. }) if shift_control == "deck5.shift"
        ));

        // SHIFT captured after the step that needs it.
        let late = ok.replace(
            "id = \"deck{deck}.shift\"\nprompt = \"Press SHIFT.\"\nkind = \"button\"\n[[section.step]]\nid = \"deck{deck}.play\"",
            "id = \"deck{deck}.play\"",
        ) + "[[section.step]]\nid = \"deck{deck}.shift\"\nprompt = \"Press SHIFT.\"\nkind = \"button\"\n";
        assert!(matches!(
            load_checklist(&late),
            Err(ChecklistError::ShiftControl { problem, .. }) if problem.contains("before")
        ));

        let missing = ok.replace(
            "shift_control = \"deck{deck}.shift\"",
            "shift_control = \"nope\"",
        );
        assert!(matches!(
            load_checklist(&missing),
            Err(ChecklistError::ShiftControl { .. })
        ));

        let not_button = ok.replace(
            "shift_control = \"deck{deck}.shift\"",
            "shift_control = \"deck{deck}.pads.shift\"",
        );
        assert!(matches!(
            load_checklist(&not_button),
            Err(ChecklistError::ShiftControl { .. })
        ));
    }

    #[test]
    fn checklist_ignore_sources() {
        let text = r#"
[ignore]
sources = [{ status = 0xB5, data1 = 0x02 }]
[[section]]
name = "A"
[[section.step]]
id = "x"
prompt = "p"
kind = "button"
"#;
        let checklist = parse_checklist(text).unwrap();
        assert_eq!(
            checklist.ignore,
            [Source {
                status: 0xB5,
                data1: 0x02
            }]
        );
        assert!(parse_checklist(&text.replace("data1 = 0x02", "data1 = 0x02, value = 1")).is_err());
        assert!(parse_checklist(&text.replace("sources", "source")).is_err());
        assert!(matches!(
            parse_checklist(&text.replace("0xB5", "0x35")),
            Err(ChecklistError::Ignore { .. })
        ));
        assert!(matches!(
            parse_checklist(&text.replace("0x02", "0x80")),
            Err(ChecklistError::Ignore { .. })
        ));
        assert!(parse_checklist(TWO_STEPS).unwrap().ignore.is_empty());
    }
}
