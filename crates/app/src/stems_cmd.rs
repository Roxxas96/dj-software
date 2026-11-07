//! The `stems` command: `stems setup`, the one-time model download. Progress goes to
//! stderr, the model's location to stdout.

use std::cell::Cell;

use anyhow::Context;
use stems::SetupEvent;

/// The weights' provenance, printed before anything is downloaded.
const NOTICE: &str = "\
Stems use the htdemucs model (Demucs v4, MIT-licensed code). Its weights are fetched from
their author, not from Open Jockey: the license of the weights is unclear, because the
author trained them partly on data usable for research only.";

/// Prints `event`s, the download every 10 % and the CoreML compile every 10 s.
#[derive(Default)]
struct Progress {
    /// The last download step (tens of percent) or compile step (tens of seconds) printed.
    last_step: Cell<Option<u64>>,
}

impl Progress {
    /// The line for `event`, if one is due.
    fn line(&self, event: SetupEvent) -> Option<String> {
        let due = |step: u64| self.last_step.replace(Some(step)) != Some(step);
        match event {
            SetupEvent::AlreadyInstalled => {
                Some("the model is already installed and verified".into())
            }
            SetupEvent::Downloading {
                done,
                total: Some(total),
            } if total > 0 => {
                let percent = (done * 100 / total).min(100) / 10 * 10;
                due(percent).then(|| {
                    format!(
                        "downloading the weights: {percent:3} % ({:.1} of {:.1} MB)",
                        done as f64 / 1e6,
                        total as f64 / 1e6
                    )
                })
            }
            SetupEvent::Downloading { done, .. } => {
                (done == 0).then(|| "downloading the weights".into())
            }
            SetupEvent::Filling => {
                self.last_step.set(None);
                Some("filling the model".into())
            }
            SetupEvent::TestVector => Some("checking the model with its test vector".into()),
            SetupEvent::Warming { seconds } => due(seconds / 10).then(|| {
                format!("compiling the model for this computer (about a minute): {seconds} s")
            }),
        }
    }
}

pub fn setup() -> anyhow::Result<()> {
    eprintln!("{NOTICE}");
    eprintln!("source: {}", stems::WEIGHTS_URL);
    let progress = Progress::default();
    stems::setup(&|event| {
        if let Some(line) = progress.line(event) {
            eprintln!("{line}");
        }
    })
    .context("stems setup")?;
    println!("model: {}", stems::model_path().display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_prints_every_ten_percent_then_every_ten_seconds() {
        let progress = Progress::default();
        let lines: Vec<String> = (0..=100)
            .map(|done| SetupEvent::Downloading {
                done,
                total: Some(100),
            })
            .chain([SetupEvent::Filling])
            .chain((0..25).map(|seconds| SetupEvent::Warming { seconds }))
            .filter_map(|event| progress.line(event))
            .collect();
        assert_eq!(lines.len(), 11 + 1 + 3, "{lines:#?}");
        assert!(lines[0].contains("  0 % (0.0 of 0.0 MB)"), "{}", lines[0]);
        assert!(lines[10].contains("100 %"), "{}", lines[10]);
        assert_eq!(lines[11], "filling the model");
        assert!(lines[14].ends_with(": 20 s"), "{}", lines[14]);
    }
}
