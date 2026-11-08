//! The thread that reads the pads at its own rate.

use std::any::Any;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use torque_client::input_source::Instant;

use crate::pad::PadBackend;
use crate::sampler::{PadSamples, Sampler};

/// What makes a backend on the thread (the backend itself need not be `Send`: gilrs is, but a platform's may not be).
pub type MakeBackend = Arc<dyn Fn() -> Box<dyn PadBackend> + Send + Sync>;

/// The lowest and highest rate the thread reads at (reads a second).
pub const RATES: std::ops::RangeInclusive<u32> = 30..=2_000;

/// How many times the thread starts its backend again after a panic before it gives up.
pub const RESTARTS: u32 = 1;

/// A thread that reads a backend, made on the thread, `rate` times a second into a queue. Dropping it stops the thread and waits
/// for it, so the backend (and any device it holds) is gone when the drop returns.
///
/// If the backend panics, the thread says so on stderr, lets go of the pads it had seen, and starts a new backend once
/// ([`RESTARTS`]); a second panic ends the thread, says so, and [`failed`](PadThread::failed) is true from then on.
pub struct PadThread {
    stop: Arc<AtomicBool>,
    state: Arc<Health>,
    handle: Option<JoinHandle<()>>,
}

#[derive(Default)]
struct Health {
    restarts: AtomicU32,
    failed: AtomicBool,
}

impl PadThread {
    /// Starts the thread. `rate` is the reads a second; change it while the thread runs and the next read follows it.
    pub fn spawn(make: MakeBackend, samples: PadSamples, rate: Arc<AtomicU32>) -> PadThread {
        let stop = Arc::new(AtomicBool::new(false));
        let state = Arc::new(Health::default());
        let (flag, health) = (Arc::clone(&stop), Arc::clone(&state));
        let handle = thread::Builder::new()
            .name("gamepad".to_owned())
            .spawn(move || supervise(&make, &samples, &rate, &flag, &health))
            .inspect_err(|error| eprintln!("gamepad: no thread to read pads: {error}"))
            .ok();
        if handle.is_none() {
            state.failed.store(true, Ordering::Release);
        }
        PadThread {
            stop,
            state,
            handle,
        }
    }

    /// Whether the thread was started and has not ended.
    pub fn running(&self) -> bool {
        self.handle.as_ref().is_some_and(|h| !h.is_finished())
    }

    /// How many times the backend panicked and was started again.
    pub fn restarts(&self) -> u32 {
        self.state.restarts.load(Ordering::Acquire)
    }

    /// Whether the pads stopped for good: the thread could not start, or its backend panicked past [`RESTARTS`].
    pub fn failed(&self) -> bool {
        self.state.failed.load(Ordering::Acquire)
    }
}

impl Drop for PadThread {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(handle) = self.handle.take() {
            // A panic is caught and reported on the thread, so a join error is only a panic in that reporting.
            let _ = handle.join();
        }
    }
}

/// Runs the reading loop, and runs it again once if its backend panics.
fn supervise(
    make: &MakeBackend,
    samples: &PadSamples,
    rate: &AtomicU32,
    stop: &AtomicBool,
    health: &Health,
) {
    loop {
        let mut sampler = None;
        let ended = catch_unwind(AssertUnwindSafe(|| {
            let sampler = sampler.insert(Sampler::new(make(), samples.clone()));
            read(sampler, rate, stop);
        }));
        let Err(panic) = ended else {
            return;
        };
        let reason = describe(panic.as_ref());
        // Let go of what the pads held, and drop the backend that panicked (which may panic again).
        let _ = catch_unwind(AssertUnwindSafe(|| {
            if let Some(mut sampler) = sampler {
                sampler.release_all();
            }
        }));
        let restarts = health.restarts.load(Ordering::Acquire);
        if restarts >= RESTARTS || stop.load(Ordering::Acquire) {
            health.failed.store(true, Ordering::Release);
            eprintln!(
                "gamepad: the pad thread panicked ({reason}) and will not be started again; gamepads are off"
            );
            return;
        }
        health.restarts.store(restarts + 1, Ordering::Release);
        eprintln!("gamepad: the pad thread panicked ({reason}); starting it again");
    }
}

fn describe(panic: &(dyn Any + Send)) -> String {
    panic
        .downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| panic.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "no message".to_owned())
}

fn read(sampler: &mut Sampler, rate: &AtomicU32, stop: &AtomicBool) {
    let mut next = Instant::now();
    while !stop.load(Ordering::Acquire) {
        sampler.sample();
        let hz = rate
            .load(Ordering::Relaxed)
            .clamp(*RATES.start(), *RATES.end());
        next += Duration::from_secs_f64(1.0 / f64::from(hz));
        let now = Instant::now();
        match next.checked_duration_since(now) {
            Some(wait) => thread::sleep(wait),
            // Behind (a stalled machine): start the next period from now, not a burst of reads to catch up.
            None => next = now,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;

    use super::*;
    use crate::pad::{FakePads, PadButton, PadState};
    use crate::sampler::PadSample;

    /// A backend that panics at its first read, in each of the first `panics` that are made.
    struct Flaky {
        pads: FakePads,
        panic: bool,
    }

    impl PadBackend for Flaky {
        fn poll(&mut self) -> Vec<(u32, PadState)> {
            assert!(!self.panic, "the backend broke");
            self.pads.poll()
        }
    }

    fn flaky(pads: &FakePads, panics: usize) -> MakeBackend {
        let (pads, made) = (pads.clone(), AtomicUsize::new(0));
        Arc::new(move || {
            let panic = made.fetch_add(1, Ordering::SeqCst) < panics;
            Box::new(Flaky {
                pads: pads.clone(),
                panic,
            })
        })
    }

    fn wait_for(what: &str, mut ready: impl FnMut() -> bool) {
        let until = std::time::Instant::now() + Duration::from_secs(10);
        while !ready() {
            assert!(std::time::Instant::now() < until, "timed out: {what}");
            thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn a_backend_that_panics_once_is_started_again() {
        let pads = FakePads::default();
        pads.press(5, PadButton::South, true);
        let samples = PadSamples::default();
        let thread = PadThread::spawn(
            flaky(&pads, 1),
            samples.clone(),
            Arc::new(AtomicU32::new(1_000)),
        );
        wait_for("the second backend's sample", || !samples.is_empty());
        assert_eq!(thread.restarts(), 1);
        assert!(thread.running() && !thread.failed());
        let first = samples.take().into_iter().next();
        assert!(matches!(first, Some(PadSample::State { id: 5, .. })));
    }

    #[test]
    fn a_backend_that_keeps_panicking_ends_the_thread_and_lets_go_of_the_pads() {
        let pads = FakePads::default();
        let samples = PadSamples::default();
        // The first backend sees a pad before it breaks; the second breaks at once.
        let made = Arc::new(AtomicUsize::new(0));
        let make: MakeBackend = {
            let (pads, made) = (pads.clone(), Arc::clone(&made));
            Arc::new(move || {
                let n = made.fetch_add(1, Ordering::SeqCst);
                Box::new(Late {
                    pads: pads.clone(),
                    reads: 0,
                    panic_at: if n == 0 { 3 } else { 0 },
                })
            })
        };
        pads.plug_in(2);
        let thread = PadThread::spawn(make, samples.clone(), Arc::new(AtomicU32::new(1_000)));
        wait_for("the thread to give up", || thread.failed());
        assert_eq!(thread.restarts(), RESTARTS);
        wait_for("the thread to end", || !thread.running());
        let seen = samples.take();
        assert!(matches!(seen.first(), Some(PadSample::State { id: 2, .. })));
        assert!(matches!(seen.last(), Some(PadSample::Gone { id: 2, .. })));
        // Dropping a thread that already ended joins at once.
        drop(thread);
    }

    /// A backend that panics at its `panic_at`th read.
    struct Late {
        pads: FakePads,
        reads: usize,
        panic_at: usize,
    }

    impl PadBackend for Late {
        fn poll(&mut self) -> Vec<(u32, PadState)> {
            assert!(self.reads != self.panic_at, "the backend broke");
            self.reads += 1;
            self.pads.poll()
        }
    }
}
