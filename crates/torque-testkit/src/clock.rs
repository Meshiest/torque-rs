//! A clock a test moves by hand, so a test never waits.

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime};

/// The wall-clock time a [`FakeClock`] starts at: seconds after the Unix epoch (November 2023), well inside any certificate's
/// validity.
const WALL_START_SECS: u64 = 1_700_000_000;

/// A clock that moves only when a test steps it ([`FakeClock::advance`]). Clones share the time, and the clock can be read
/// from other threads.
#[derive(Debug, Clone)]
pub struct FakeClock {
    origin: Instant,
    elapsed: Arc<Mutex<Duration>>,
}

impl Default for FakeClock {
    fn default() -> FakeClock {
        FakeClock {
            origin: Instant::now(),
            elapsed: Arc::new(Mutex::new(Duration::ZERO)),
        }
    }
}

impl FakeClock {
    /// A clock at its start.
    #[must_use]
    pub fn new() -> FakeClock {
        FakeClock::default()
    }

    /// Moves the clock forward by `by`.
    pub fn advance(&self, by: Duration) {
        *self.lock() += by;
    }

    /// How far the clock has been moved.
    #[must_use]
    pub fn elapsed(&self) -> Duration {
        *self.lock()
    }

    /// The time now.
    #[must_use]
    pub fn now(&self) -> Instant {
        self.at_duration(self.elapsed())
    }

    /// The time `ms` milliseconds after the clock began.
    #[must_use]
    pub fn at(&self, ms: u64) -> Instant {
        self.at_duration(Duration::from_millis(ms))
    }

    /// The time `since` after the clock began.
    #[must_use]
    pub fn at_duration(&self, since: Duration) -> Instant {
        self.origin + since
    }

    /// The wall clock: a fixed date that moves with the steps.
    #[must_use]
    pub fn wall(&self) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(WALL_START_SECS) + self.elapsed()
    }

    /// A reader of this clock, for code that takes a closure.
    pub fn reader(&self) -> impl Fn() -> Instant + Send + Sync + 'static {
        let clock = self.clone();
        move || clock.now()
    }

    fn lock(&self) -> MutexGuard<'_, Duration> {
        self.elapsed
            .lock()
            .expect("a panic never holds the fake clock")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_clock_moves_only_when_stepped_and_clones_share_it() {
        let clock = FakeClock::new();
        let other = clock.clone();
        let (start, wall) = (clock.now(), clock.wall());
        assert_eq!(clock.now(), start);
        other.advance(Duration::from_millis(1_500));
        assert_eq!(clock.now() - start, Duration::from_millis(1_500));
        let moved = clock
            .wall()
            .duration_since(wall)
            .expect("the wall clock moves forward");
        assert_eq!(moved, Duration::from_millis(1_500));
        assert_eq!(clock.at(40) - start, Duration::from_millis(40));
        let read = clock.reader();
        clock.advance(Duration::from_secs(1));
        assert_eq!(read() - start, Duration::from_millis(2_500));
    }
}
