//! The capture checklist: a TOML file of sections and steps, expanded per deck
//! and per SHIFT variant into one flat, ordered list of [`Step`]s.
//!
//! Step keys: `id`, `prompt`, `kind`, optional `prepare` (done before listening
//! starts), `shift` (also capture a SHIFT variant) and `held_shift` (this step is
//! itself done with SHIFT held).

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

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
    /// `held_shift` step); the first key pressed is then treated as SHIFT.
    pub shift: bool,
}

/// A loaded checklist: an optional controller name and the expanded steps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checklist {
    pub controller: Option<String>,
    pub steps: Vec<Step>,
}

#[derive(Debug, thiserror::Error)]
pub enum ChecklistError {
    #[error("invalid checklist TOML: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("duplicate step id {0:?}")]
    DuplicateId(String),
    #[error("section {section:?}: {problem}")]
    Section { section: String, problem: String },
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileDef {
    controller: Option<String>,
    #[serde(rename = "section", default)]
    sections: Vec<SectionDef>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SectionDef {
    name: String,
    decks: Option<Vec<u8>>,
    setup: Option<String>,
    #[serde(rename = "step", default)]
    steps: Vec<StepDef>,
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
    Ok(Checklist {
        controller: file.controller,
        steps,
    })
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
            };
            let shifted = def.shift.then(|| Step {
                id: format!("{}.shift", base.id),
                prompt: format!("Hold SHIFT and keep holding it, then: {}", base.prompt),
                shift: true,
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
}
