//! Reading pads faster than the client polls: a [`Sampler`] reads a backend and leaves stamped samples of what changed in a
//! [`PadSamples`] queue, which the source drains at each client poll.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};

use torque_client::input_source::Instant;

use crate::pad::{PadBackend, PadState};

/// How many samples a queue holds before it merges stick moves: past it, a pad's new sample replaces that pad's latest waiting
/// one when only the sticks and the depth of the triggers differ (the older sample's time is kept, so the merged span still
/// begins where it did).
pub const MAX_SAMPLES: usize = 16_384;

/// How many samples a queue holds before it drops the stick-only samples already waiting. Samples that press or let go of a
/// button, and pads that go, are never dropped, so a queue grows past this only with presses and unplugs.
pub const HARD_MAX_SAMPLES: usize = 4 * MAX_SAMPLES;

/// What a pad did, and when.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PadSample {
    /// Pad `id` held `state` from `at` on.
    State {
        at: Instant,
        id: u32,
        state: PadState,
    },
    /// Pad `id` was unplugged at `at`.
    Gone { at: Instant, id: u32 },
}

/// A queue of [`PadSample`]s that a thread (or a timer) fills and the source drains. Clones share the queue.
///
/// A client that does not poll (a minimized window) leaves it filling: past [`MAX_SAMPLES`] the stick moves of each pad are
/// merged into its latest waiting sample, past [`HARD_MAX_SAMPLES`] the stick-only samples are dropped, and one warning is
/// printed. Button samples keep their order and are never dropped.
#[derive(Debug, Clone)]
pub struct PadSamples {
    inner: Arc<Mutex<Waiting>>,
    soft: usize,
    hard: usize,
}

#[derive(Debug, Default)]
struct Waiting {
    samples: Vec<PadSample>,
    warned: bool,
}

impl Default for PadSamples {
    fn default() -> PadSamples {
        PadSamples::with_limits(MAX_SAMPLES, HARD_MAX_SAMPLES)
    }
}

impl PadSamples {
    /// A queue that merges past `soft` samples and drops stick-only samples past `hard`.
    pub fn with_limits(soft: usize, hard: usize) -> PadSamples {
        PadSamples {
            inner: Arc::default(),
            soft,
            hard,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Waiting> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Pad `id` holds `state` from `at` on.
    pub fn state(&self, at: Instant, id: u32, state: PadState) {
        let mut waiting = self.lock();
        let queue = &mut waiting.samples;
        if queue.len() >= self.soft {
            // The pad's latest waiting sample, if it is a state with the same buttons: the sticks move on in it.
            let latest = queue.iter_mut().rev().find(|s| match s {
                PadSample::State { id: other, .. } | PadSample::Gone { id: other, .. } => {
                    *other == id
                }
            });
            if let Some(PadSample::State { state: held, .. }) = latest
                && held.same_buttons(&state)
            {
                *held = state;
                if !std::mem::replace(&mut waiting.warned, true) {
                    eprintln!(
                        "gamepad: samples are piling up because the game is not polling; merging stick moves (buttons are kept)"
                    );
                }
                return;
            }
        }
        if queue.len() >= self.hard {
            // Only presses and unplugs are worth the room now.
            let mut before: BTreeMap<u32, PadState> = BTreeMap::new();
            queue.retain(|sample| match sample {
                PadSample::Gone { id, .. } => {
                    before.remove(id);
                    true
                }
                PadSample::State { id, state, .. } => before
                    .insert(*id, *state)
                    .is_none_or(|was| !was.same_buttons(state)),
            });
        }
        queue.push(PadSample::State { at, id, state });
    }

    /// Pad `id` was unplugged at `at`.
    pub fn gone(&self, at: Instant, id: u32) {
        self.lock().samples.push(PadSample::Gone { at, id });
    }

    /// How many samples wait.
    pub fn len(&self) -> usize {
        self.lock().samples.len()
    }

    /// Whether nothing waits.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Takes everything that waits, oldest first.
    pub fn take(&self) -> Vec<PadSample> {
        std::mem::take(&mut self.lock().samples)
    }
}

/// Reads a backend and queues what changed since its last read.
pub struct Sampler {
    backend: Box<dyn PadBackend>,
    samples: PadSamples,
    last: BTreeMap<u32, PadState>,
}

impl Sampler {
    /// A sampler that reads `backend` into `samples`.
    pub fn new(backend: Box<dyn PadBackend>, samples: PadSamples) -> Sampler {
        Sampler {
            backend,
            samples,
            last: BTreeMap::new(),
        }
    }

    /// Reads the backend once: a sample for each pad that is new or changed, stamped when the read finished, and one for each
    /// pad that went.
    pub fn sample(&mut self) {
        let pads = self.backend.poll();
        let at = Instant::now();
        let gone: Vec<u32> = self
            .last
            .keys()
            .filter(|id| !pads.iter().any(|(pad, _)| pad == *id))
            .copied()
            .collect();
        for id in gone {
            self.last.remove(&id);
            self.samples.gone(at, id);
        }
        for (id, state) in pads {
            if self.last.insert(id, state) != Some(state) {
                self.samples.state(at, id, state);
            }
        }
    }

    /// Says every pad it has seen is gone (a reader that failed lets go of what it held).
    pub fn release_all(&mut self) {
        let at = Instant::now();
        for id in std::mem::take(&mut self.last).into_keys() {
            self.samples.gone(at, id);
        }
    }

    /// The backend, to read it another way.
    #[cfg(target_arch = "wasm32")]
    pub fn into_backend(self) -> Box<dyn PadBackend> {
        self.backend
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pad::PadButton;

    fn pad(x: f32, south: bool) -> PadState {
        let mut state = PadState::default();
        state.left = (x, 0.0);
        state.set(PadButton::South, south);
        state
    }

    #[test]
    fn many_samples_of_several_pads_with_no_poll_stay_bounded_and_keep_button_order() {
        let queue = PadSamples::default();
        let at = Instant::now();
        let mut down = [false; 4];
        let mut expected = Vec::new();
        for i in 0..1_000_000u32 {
            let id = i % 4;
            // Every 24,999th sample presses or lets go of A on its pad (the step is odd, so it falls on every pad in turn).
            if i % 24_999 == 0 {
                down[id as usize] = !down[id as usize];
                expected.push((id, down[id as usize]));
            }
            queue.state(at, id, pad((i % 1_000) as f32 / 1_000.0, down[id as usize]));
            assert!(queue.len() <= MAX_SAMPLES + expected.len());
        }
        let samples = queue.take();
        assert!(samples.len() <= MAX_SAMPLES + expected.len());
        // The button changes, pad by pad and in the order they were made.
        let mut seen: BTreeMap<u32, bool> = BTreeMap::new();
        let mut changes = Vec::new();
        for sample in &samples {
            let PadSample::State { id, state, .. } = sample else {
                continue;
            };
            let now = state.pressed(PadButton::South);
            if seen.insert(*id, now).unwrap_or(false) != now {
                changes.push((*id, now));
            }
        }
        assert_eq!(changes, expected);
    }

    #[test]
    fn past_the_hard_limit_only_stick_samples_are_dropped() {
        let queue = PadSamples::with_limits(1_000_000, 100);
        let at = Instant::now();
        // The soft limit is out of reach, so nothing merges: the hard limit alone thins the queue.
        for i in 0..1_000u32 {
            queue.state(at, i % 2, pad(i as f32 / 1_000.0, false));
        }
        queue.state(at, 0, pad(0.0, true));
        queue.gone(at, 1);
        queue.state(at, 0, pad(0.0, false));
        let samples = queue.take();
        assert!(samples.len() <= 110, "{}", samples.len());
        let buttons: Vec<bool> = samples
            .iter()
            .filter_map(|s| match s {
                PadSample::State { id: 0, state, .. } => Some(state.pressed(PadButton::South)),
                _ => None,
            })
            .collect();
        assert!(buttons.windows(2).filter(|w| w[0] != w[1]).count() == 2);
        assert!(
            samples
                .iter()
                .any(|s| matches!(s, PadSample::Gone { id: 1, .. }))
        );
    }
}
