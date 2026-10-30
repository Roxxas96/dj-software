//! The `midi` commands: list ports and monitor raw messages.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{RecvTimeoutError, channel};
use std::time::Duration;

use anyhow::{Context, anyhow};
use controller::{clock, input_port_names, open_input, output_port_names};

const POLL: Duration = Duration::from_millis(100);

pub fn ports() -> anyhow::Result<()> {
    for name in input_port_names().context("listing MIDI inputs")? {
        println!("in:  {name}");
    }
    for name in output_port_names().context("listing MIDI outputs")? {
        println!("out: {name}");
    }
    Ok(())
}

/// Picks the port to monitor: the requested name, else the first input containing "DDJ".
fn choose_port(requested: Option<&str>, inputs: &[String]) -> anyhow::Result<String> {
    if let Some(name) = requested {
        return Ok(name.to_owned());
    }
    if let Some(name) = inputs.iter().find(|n| n.contains("DDJ")) {
        return Ok(name.clone());
    }
    let list = if inputs.is_empty() {
        "none".to_owned()
    } else {
        inputs.join(", ")
    };
    Err(anyhow!(
        "no MIDI input containing \"DDJ\" found; pass --port <NAME>. Available inputs: {list}"
    ))
}

pub fn monitor(port: Option<&str>) -> anyhow::Result<()> {
    let inputs = input_port_names().context("listing MIDI inputs")?;
    let wanted = choose_port(port, &inputs)?;

    let interrupted = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&interrupted);
    ctrlc::set_handler(move || flag.store(true, Ordering::Relaxed))
        .context("installing the Ctrl-C handler")?;

    let (tx, rx) = channel();
    let started = clock::now_micros();
    let input = open_input(&wanted, tx).context("opening MIDI input")?;
    println!("Monitoring {} (Ctrl-C to stop)", input.name());
    while !interrupted.load(Ordering::Relaxed) {
        match rx.recv_timeout(POLL) {
            Ok(timed) => {
                let ms = timed.micros.saturating_sub(started) / 1000;
                println!("{ms:>8} ms  {}", timed.message);
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn explicit_port_wins() {
        let inputs = names(&["DDJ-SB3"]);
        assert_eq!(choose_port(Some("IAC"), &inputs).unwrap(), "IAC");
    }

    #[test]
    fn defaults_to_first_ddj_input() {
        let inputs = names(&["IAC Driver", "DDJ-SB3", "DDJ-400"]);
        assert_eq!(choose_port(None, &inputs).unwrap(), "DDJ-SB3");
    }

    #[test]
    fn no_ddj_lists_available_ports() {
        let error = choose_port(None, &names(&["IAC Driver"])).unwrap_err();
        assert!(error.to_string().contains("IAC Driver"));
    }
}
