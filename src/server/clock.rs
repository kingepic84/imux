//! `clock-mode`: a big clock in the active pane until a key is pressed or the button clicked.

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use super::Server;
use super::modes::Mode;
use crate::config::format;

/// Milliseconds from `now_ms` (since the epoch) to a moment just after the next full minute.
pub(super) fn millis_to_next_minute(now_ms: u128) -> u64 {
    (60_000 - (now_ms % 60_000)) as u64 + 50
}

/// When the clock has to be drawn again.
pub(super) fn next_tick() -> Instant {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis();
    Instant::now() + Duration::from_millis(millis_to_next_minute(now))
}

impl Server {
    pub(super) fn open_clock(&mut self, id: usize) {
        if let Some(c) = self.clients.get_mut(&id) {
            c.mode = Mode::Clock(next_tick());
        }
        self.dirty = true;
    }

    /// The digits to draw and, for the 12 hour style, `AM` or `PM` under them.
    pub(super) fn clock_face(&self) -> (String, String) {
        if self.options.text("clock-mode-style") == "12" {
            (format::strftime("%I:%M"), format::strftime("%p"))
        } else {
            (format::strftime("%H:%M"), String::new())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_clock_wakes_just_after_each_minute() {
        assert_eq!(millis_to_next_minute(0), 60_050);
        assert_eq!(millis_to_next_minute(59_999), 51);
        assert_eq!(millis_to_next_minute(60_000), 60_050);
        let now: u128 = 1_700_000_012_345;
        assert_eq!(u128::from(millis_to_next_minute(now)) + now % 60_000, 60_050);
        assert!((0..120_000u128).step_by(997).all(|t| (51..=60_050).contains(&millis_to_next_minute(t))));
    }
}
