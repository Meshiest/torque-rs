//! The wall-clock pacing of a realtime server: the main loop of a dedicated server, as TGE runs it.
//!
//! The pacing follows TGE's main loop (`DemoGame::main` and `DemoGame::processTimeEvent`), which Torque3D, the MIT release of
//! that engine, keeps as `StandardMainLoop` (`Engine/source/app/mainLoop.cpp`, `Copyright (c) 2012 GarageGames, LLC`); the code
//! here is this crate's own.
//!
//! **The loop.** Each turn drains the socket, pumps the window messages, then sleeps 1 ms (a dedicated server has no window),
//! then reads the time and posts a time event when at least a millisecond passed. The time event caps the elapsed time at 1,024
//! ms, ticks the server once per 32 ms boundary of its time and advances the Sim events by it. On a system timer left at its
//! default 64 Hz, a 1 ms sleep returns at the next timer interrupt: a turn every 15.625 ms.
//!
//! **Here.** [`PassClock`] sleeps to the next point of that 64 Hz grid (counted from the clock's start) and gives the pass the
//! wall milliseconds since the last one (whole milliseconds of the wall since the start, so the fraction is carried); the host
//! hands them to the server, and the pass drains the socket, ticks and advances the events. Nothing runs between two passes
//! (the loop reads its socket once a turn), so an idle server wakes 64 times a second.

use std::time::{Duration, Instant};

/// One turn of the dedicated loop: a 1 ms sleep on the default 64 Hz system timer (15.625 ms).
pub const PASS_PERIOD: Duration = Duration::from_micros(15_625);

/// The wall clock of a realtime host: when each pass of the loop runs, and the milliseconds it covers.
#[derive(Debug, Clone)]
pub struct PassClock {
    start: Instant,
    period: Duration,
    /// The whole milliseconds of the wall at the last pass (since `start`).
    last_ms: u64,
    passes: u64,
}

impl Default for PassClock {
    fn default() -> PassClock {
        PassClock::new()
    }
}

impl PassClock {
    /// A clock that starts now, with the default period ([`PASS_PERIOD`]).
    pub fn new() -> PassClock {
        PassClock::with_period(PASS_PERIOD)
    }

    /// A clock with another period (tests; at least a millisecond).
    pub fn with_period(period: Duration) -> PassClock {
        PassClock {
            start: Instant::now(),
            period: period.max(Duration::from_millis(1)),
            last_ms: 0,
            passes: 0,
        }
    }

    /// The whole milliseconds of the wall since the clock started.
    pub fn wall_ms(&self) -> u64 {
        self.start.elapsed().as_millis() as u64
    }

    /// How many passes the clock has paced.
    pub fn passes(&self) -> u64 {
        self.passes
    }

    /// Sleeps until the wall has run `ms` (a host's first main-loop pass) and counts that pass: the next
    /// [`wait_pass`](Self::wait_pass) covers the time from here.
    pub fn wait_until_ms(&mut self, ms: u64) {
        let due = Duration::from_millis(ms);
        if let Some(wait) = due.checked_sub(self.start.elapsed()) {
            std::thread::sleep(wait);
        }
        self.last_ms = self.wall_ms();
        self.passes += 1;
    }

    /// Sleeps to the next point of the pass grid after now (a pass whose work ran past one or more points waits for the next,
    /// as a 1 ms sleep does) and returns the whole milliseconds since the last pass (at least 1: no time event is posted for
    /// less, and the next turn waits again). Saturates at `u32::MAX`.
    pub fn wait_pass(&mut self) -> u32 {
        loop {
            let now = self.start.elapsed();
            let period = self.period.as_nanos();
            let next = (now.as_nanos() / period + 1) * period;
            let wait = Duration::from_nanos((next - now.as_nanos()) as u64);
            std::thread::sleep(wait);
            let ms = self.wall_ms();
            let elapsed = ms.saturating_sub(self.last_ms);
            if elapsed >= 1 {
                self.last_ms = ms;
                self.passes += 1;
                return u32::try_from(elapsed).unwrap_or(u32::MAX);
            }
        }
    }
}

#[cfg(test)]
#[path = "pacing_tests.rs"]
mod tests;
