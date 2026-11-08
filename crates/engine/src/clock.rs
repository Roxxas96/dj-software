//! The clock the renderer stamps its published deck snapshots with: microseconds since a
//! process-wide epoch, so a stamp fits in an atomic and the UI can tell when a snapshot's
//! block was rendered.

use std::sync::OnceLock;
use std::time::{Duration, Instant};

static EPOCH: OnceLock<Instant> = OnceLock::new();

/// The epoch, fixed on first use. [`crate::engine`] fixes it before the audio thread runs,
/// so the renderer only ever reads it.
fn epoch() -> Instant {
    *EPOCH.get_or_init(Instant::now)
}

/// `at` in microseconds since the epoch (0 for an instant before it).
pub fn micros_at(at: Instant) -> u64 {
    u64::try_from(at.saturating_duration_since(epoch()).as_micros()).unwrap_or(u64::MAX)
}

/// The instant `micros` microseconds after the epoch.
pub fn instant_at(micros: u64) -> Instant {
    epoch() + Duration::from_micros(micros)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_instant_survives_the_round_trip_to_the_microsecond() {
        let _ = epoch();
        let now = Instant::now();
        let back = instant_at(micros_at(now));
        assert!(
            back <= now && now - back < Duration::from_micros(1),
            "{back:?} {now:?}"
        );
    }
}
