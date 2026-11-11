//! The clock the endpoints read: the system's for real sockets, a hand-stepped one for the deterministic tests and the fuzz.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use quinn_proto::{StdSystemTime, TimeSource};

/// Where an endpoint reads the time: every `now` quinn-proto is handed, and the wall-clock source its tokens are stamped by.
pub trait Clock {
    fn now(&self) -> Instant;
    /// The wall-clock source the server's handshake tokens are stamped and checked by (the same clock, so a token's age
    /// moves with the test's steps).
    fn time_source(&self) -> Arc<dyn TimeSource>;
}

/// The operating system's clocks.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }

    fn time_source(&self) -> Arc<dyn TimeSource> {
        Arc::new(StdSystemTime)
    }
}

/// A clock that moves only when a test steps it ([`FakeClock::advance`]). Clones share the time.
#[derive(Debug, Clone)]
pub struct FakeClock {
    base: Instant,
    wall: SystemTime,
    elapsed: Arc<Mutex<Duration>>,
}

impl Default for FakeClock {
    fn default() -> FakeClock {
        FakeClock {
            base: Instant::now(),
            wall: SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000),
            elapsed: Arc::new(Mutex::new(Duration::ZERO)),
        }
    }
}

impl FakeClock {
    #[must_use]
    pub fn new() -> FakeClock {
        FakeClock::default()
    }

    /// Moves the clock forward by `by`.
    pub fn advance(&self, by: Duration) {
        let mut elapsed = self.elapsed.lock().expect("the fake clock is not poisoned");
        *elapsed += by;
    }

    /// How far the clock has been moved.
    #[must_use]
    pub fn elapsed(&self) -> Duration {
        *self.elapsed.lock().expect("the fake clock is not poisoned")
    }
}

impl Clock for FakeClock {
    fn now(&self) -> Instant {
        self.base + self.elapsed()
    }

    fn time_source(&self) -> Arc<dyn TimeSource> {
        Arc::new(self.clone())
    }
}

impl TimeSource for FakeClock {
    fn now(&self) -> SystemTime {
        self.wall + self.elapsed()
    }
}
