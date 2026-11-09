//! The two rules that watch a realm over time: the strike rule (too many limit stops turn it off) and the slow rule
//! (a realm over its frame budget too often runs every other frame).

use std::collections::VecDeque;

use crate::settings::{SLOW_WINDOW, StrikeRule};

/// The limit stops within the strike rule's window.
#[derive(Debug)]
pub(crate) struct Strikes {
    rule: StrikeRule,
    at: VecDeque<u64>,
}

impl Strikes {
    pub(crate) fn new(rule: StrikeRule) -> Strikes {
        Strikes {
            rule,
            at: VecDeque::new(),
        }
    }

    /// Counts a stop at `now_us`: `true` when it is the one that turns the realm off.
    pub(crate) fn strike(&mut self, now_us: u64) -> bool {
        while self
            .at
            .front()
            .is_some_and(|&t| now_us.saturating_sub(t) >= self.rule.window_us)
        {
            self.at.pop_front();
        }
        self.at.push_back(now_us);
        self.at.len() >= self.rule.count.max(1)
    }
}

/// Which of the last [`SLOW_WINDOW`] measured frames went over the budget.
#[derive(Debug)]
pub(crate) struct Pace {
    slow_after: u32,
    /// Bit 0 the newest frame: set when it was over.
    history: u64,
    slow: bool,
    /// While slow: whether this frame is one the realm sits out.
    rest: bool,
}

impl Pace {
    pub(crate) fn new(slow_after: u32) -> Pace {
        Pace {
            slow_after: slow_after.clamp(1, SLOW_WINDOW),
            history: 0,
            slow: false,
            rest: false,
        }
    }

    /// Counts a frame the realm ran in.
    pub(crate) fn record(&mut self, over: bool) {
        let window = (1u64 << SLOW_WINDOW) - 1;
        self.history = ((self.history << 1) | u64::from(over)) & window;
        self.slow = self.history.count_ones() >= self.slow_after;
    }

    /// Whether the realm sits out this frame: every other frame while it is slow. Frames sat out are not counted.
    pub(crate) fn sits_out(&mut self) -> bool {
        if !self.slow {
            self.rest = false;
            return false;
        }
        self.rest = !self.rest;
        self.rest
    }

    pub(crate) fn is_slow(&self) -> bool {
        self.slow
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn three_strikes_within_the_window_turn_it_off_and_older_ones_fall_away() {
        let rule = StrikeRule {
            count: 3,
            window_us: 10_000_000,
        };
        let mut strikes = Strikes::new(rule);
        assert!(!strikes.strike(0));
        assert!(!strikes.strike(6_000_000));
        // The first is 10 s old: two in the window.
        assert!(!strikes.strike(10_000_000));
        assert!(strikes.strike(12_000_000));
    }

    #[test]
    fn slow_after_30_of_60_frames_over_and_every_other_frame_sat_out() {
        let mut pace = Pace::new(30);
        for i in 0..59 {
            pace.record(i % 2 == 1);
        }
        assert!(!pace.is_slow(), "29 of 59 over");
        pace.record(false);
        pace.record(true);
        assert!(pace.is_slow(), "30 of the last 60 over");
        let sat: Vec<bool> = (0..4).map(|_| pace.sits_out()).collect();
        assert_eq!(sat, [true, false, true, false]);
        for _ in 0..60 {
            pace.record(false);
        }
        assert!(!pace.is_slow());
        assert!(!pace.sits_out());
    }
}
