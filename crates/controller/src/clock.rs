//! Process-wide monotonic clock shared by every MIDI timestamp.

use std::sync::OnceLock;
use std::time::Instant;

static START: OnceLock<Instant> = OnceLock::new();

/// Microseconds since the first call in this process. Monotonic non-decreasing.
pub fn now_micros() -> u64 {
    let start = *START.get_or_init(Instant::now);
    u64::try_from(start.elapsed().as_micros()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn now_micros_is_monotonic_non_decreasing() {
        let mut last = now_micros();
        for _ in 0..1000 {
            let now = now_micros();
            assert!(now >= last);
            last = now;
        }
    }
}
