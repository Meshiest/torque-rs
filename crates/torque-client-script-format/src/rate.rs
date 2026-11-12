//! The rates both sides of a realm keep, on the entry calls' `now_us`: the guest so a script gets a clear answer, the host
//! as the enforcement point. Both count with these, so an honest guest never emits what its host would refuse or drop.

use crate::LOG_LINES_PER_SECOND;
use crate::net::{SEND_BURST, SEND_BYTES_PER_SECOND, SENDS_PER_SECOND};

/// One message (or one byte) in a [`SendBucket`]'s units: millionths, so a microsecond's refill is never lost.
const UNIT: u64 = 1_000_000;

/// The rate of `net.send` and `net.call` together: [`SENDS_PER_SECOND`] (burst [`SEND_BURST`]) and
/// [`SEND_BYTES_PER_SECOND`] (a second's worth in the burst). It starts full.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SendBucket {
    messages: u64,
    bytes: u64,
    refilled: Option<u64>,
}

impl Default for SendBucket {
    fn default() -> SendBucket {
        SendBucket::new()
    }
}

impl SendBucket {
    /// A full bucket.
    #[must_use]
    pub const fn new() -> SendBucket {
        SendBucket {
            messages: SEND_BURST as u64 * UNIT,
            bytes: SEND_BYTES_PER_SECOND as u64 * UNIT,
            refilled: None,
        }
    }

    /// Takes one message of `len` bytes at `now_us`: false, and nothing taken from either, when the messages or the bytes
    /// are not there.
    pub fn take(&mut self, now_us: u64, len: usize) -> bool {
        let elapsed = self.refilled.map_or(0, |at| now_us.saturating_sub(at));
        self.refilled = Some(now_us);
        let refill = |left: u64, per_second: u32, cap: u64| {
            left.saturating_add(elapsed.saturating_mul(u64::from(per_second)))
                .min(cap)
        };
        self.messages = refill(
            self.messages,
            SENDS_PER_SECOND,
            u64::from(SEND_BURST) * UNIT,
        );
        self.bytes = refill(
            self.bytes,
            SEND_BYTES_PER_SECOND,
            u64::from(SEND_BYTES_PER_SECOND) * UNIT,
        );
        let cost = (len as u64).saturating_mul(UNIT);
        if self.messages < UNIT || self.bytes < cost {
            return false;
        }
        self.messages -= UNIT;
        self.bytes -= cost;
        true
    }
}

/// Lines counted in a second that begins at the first line after the last second passed: the log cap of
/// [`LOG_LINES_PER_SECOND`] lines, then one warning ([`LineWindow::log_line`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LineWindow {
    second: Option<u64>,
    lines: u32,
}

/// What a [`LineWindow`] says of a log line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLine {
    /// One of the second's [`LOG_LINES_PER_SECOND`].
    Shown,
    /// The one past them: the warning that the rest of the second's lines are dropped takes its place.
    Warning,
    /// Dropped.
    Dropped,
}

impl LineWindow {
    /// A window that begins its first second at its first line.
    #[must_use]
    pub const fn new() -> LineWindow {
        LineWindow {
            second: None,
            lines: 0,
        }
    }

    /// Begins a new second at `now_us` when none is under way or the one under way has passed: true when it did.
    pub fn roll(&mut self, now_us: u64) -> bool {
        let passed = self
            .second
            .is_none_or(|start| now_us.saturating_sub(start) >= 1_000_000);
        if passed {
            self.second = Some(now_us);
            self.lines = 0;
        }
        passed
    }

    /// Counts one line at `now_us` (the second rolls first): its place in the second, 1 for the first.
    pub fn count(&mut self, now_us: u64) -> u32 {
        self.roll(now_us);
        self.lines = self.lines.saturating_add(1);
        self.lines
    }

    /// Counts one log line at `now_us` under the log cap.
    pub fn log_line(&mut self, now_us: u64) -> LogLine {
        match self.count(now_us) {
            n if n <= LOG_LINES_PER_SECOND => LogLine::Shown,
            n if n == LOG_LINES_PER_SECOND + 1 => LogLine::Warning,
            _ => LogLine::Dropped,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_send_bucket_spends_a_burst_then_refills_at_its_rate_and_a_refusal_takes_nothing() {
        let mut bucket = SendBucket::new();
        assert_eq!((0..100).filter(|_| bucket.take(0, 0)).count(), 64);
        // Messages the message count refuses take none of the bytes: a second later all 64 KiB are still there.
        for _ in 0..100 {
            assert!(!bucket.take(0, 1 << 10));
        }
        assert!(bucket.take(1_000_000, 64 << 10));
        assert!(!bucket.take(1_000_000, 1), "the bytes are spent");
        // Bytes the bytes refuse take no message.
        let mut bucket = SendBucket::new();
        for _ in 0..100 {
            assert!(!bucket.take(0, (64 << 10) + 1));
        }
        assert_eq!((0..100).filter(|_| bucket.take(0, 0)).count(), 64);
        // 32 messages a second.
        assert_eq!((0..100).filter(|_| bucket.take(1_000_000, 0)).count(), 32);
    }

    #[test]
    fn a_line_window_is_a_second_from_its_first_line() {
        let mut w = LineWindow::new();
        let limit = LOG_LINES_PER_SECOND;
        for _ in 0..limit {
            assert_eq!(w.log_line(900_000), LogLine::Shown);
        }
        assert_eq!(w.log_line(1_000_000), LogLine::Warning);
        assert_eq!(w.log_line(1_899_999), LogLine::Dropped);
        assert_eq!(w.log_line(1_900_000), LogLine::Shown);
        assert!(!w.roll(2_899_999));
        assert!(w.roll(2_900_000));
    }
}
