//! The per-frame driver of a client's view of its world, whatever renderer draws it: when the scene is built again
//! ([`RebuildPace`]: when the world changed or the eye moved far from where it was built, at most every [`REBUILD_EVERY`]), how
//! often the lines of sight its flares are drawn by are checked between builds ([`SightWatch`]), the pace of the builds a kind
//! of change asks for ([`Pace`]), and a renderer made and warmed up off the window's thread ([`Warm`]: the window keeps drawing
//! its current look meanwhile). What a build and a draw are is the renderer's and the game's.

use std::sync::mpsc;
use web_time::{Duration, Instant};

/// The scene is built again at most this often (a window), and when the eye is this far (units) from where it was built.
pub const REBUILD_EVERY: Duration = Duration::from_millis(500);
pub const REBUILD_DISTANCE: f32 = 32.0;
/// How often the lines of sight are checked between builds ([`SightWatch`]).
pub const SIGHT_EVERY: Duration = Duration::from_millis(50);
/// The most builds line-of-sight changes ask for in one [`REBUILD_EVERY`] (a server cannot make every check a build).
pub const MAX_SIGHT_REBUILDS: u32 = 4;

/// What the scene was last built from: when, the eye, the world's signature (`S`: what changes the scene) and the frame size.
#[derive(Debug, Clone, PartialEq)]
pub struct RebuildPace<S> {
    pub built_at: Option<Instant>,
    pub built_eye: Option<[f32; 3]>,
    pub built_sig: Option<S>,
    pub built_size: Option<(u32, u32)>,
}

impl<S> Default for RebuildPace<S> {
    fn default() -> RebuildPace<S> {
        RebuildPace {
            built_at: None,
            built_eye: None,
            built_sig: None,
            built_size: None,
        }
    }
}

impl<S: PartialEq> RebuildPace<S> {
    /// Whether [`REBUILD_EVERY`] has passed since the last build at `now` (or there was none).
    pub fn due(&self, now: Instant) -> bool {
        self.built_at
            .is_none_or(|t| now.saturating_duration_since(t) >= REBUILD_EVERY)
    }

    /// Whether the scene would differ from the last build: the world's signature `sig` or the frame `size` changed, or the eye
    /// at `eye` is more than [`REBUILD_DISTANCE`] from where it was built.
    pub fn changed(&self, sig: &S, eye: [f32; 3], size: (u32, u32)) -> bool {
        let far_moved = self.built_eye.is_none_or(|b| {
            (0..3).map(|i| (b[i] - eye[i]).powi(2)).sum::<f32>().sqrt() > REBUILD_DISTANCE
        });
        self.built_sig.as_ref() != Some(sig) || far_moved || self.built_size != Some(size)
    }

    /// The scene was built at `now` for `eye`, `sig` and `size`.
    pub fn built(&mut self, now: Instant, eye: [f32; 3], sig: S, size: (u32, u32)) {
        self.built_at = Some(now);
        self.built_eye = Some(eye);
        self.built_sig = Some(sig);
        self.built_size = Some(size);
    }
}

/// The lines of sight a window checks between builds ([`SightWatch::changed_at`]): the answers the frame was built with, the last
/// check and the builds of the current [`REBUILD_EVERY`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SightWatch {
    built: Option<Vec<Option<bool>>>,
    checked_at: Option<Instant>,
    window: Option<(Instant, u32)>,
}

impl SightWatch {
    /// Whether the lines of sight now (`key`, asked for only when a check is due) differ from the ones the frame was built
    /// with: checked at most every [`SIGHT_EVERY`], and true at most [`MAX_SIGHT_REBUILDS`] times in a [`REBUILD_EVERY`]
    /// ([`SightWatch::due`], then [`SightWatch::differs`]).
    pub fn changed_at(&mut self, now: Instant, key: impl FnOnce() -> Vec<Option<bool>>) -> bool {
        self.due(now) && self.differs(&key())
    }

    /// Whether a check runs at `now` (counted as run when it does): the frame was built, [`SIGHT_EVERY`] has passed since the
    /// last check, and fewer than [`MAX_SIGHT_REBUILDS`] checks asked for a build in this [`REBUILD_EVERY`].
    pub fn due(&mut self, now: Instant) -> bool {
        if self.built.is_none()
            || self
                .checked_at
                .is_some_and(|t| now.saturating_duration_since(t) < SIGHT_EVERY)
        {
            return false;
        }
        self.checked_at = Some(now);
        let window = match self.window {
            Some((start, n)) if now.saturating_duration_since(start) < REBUILD_EVERY => (start, n),
            _ => (now, 0),
        };
        self.window = Some(window);
        window.1 < MAX_SIGHT_REBUILDS
    }

    /// Whether the lines of sight `key` of a due check differ from the ones the frame was built with (a test the world did not
    /// make, None, keeps the last answer); a difference counts as a build asked for.
    pub fn differs(&mut self, key: &[Option<bool>]) -> bool {
        let changed = self.built.as_ref().is_none_or(|b| {
            b.len() != key.len()
                || b.iter()
                    .zip(key)
                    .any(|(was, now)| now.is_some() && now != was)
        });
        if changed && let Some((start, n)) = self.window {
            self.window = Some((start, n + 1));
        }
        changed
    }

    /// The frame was built with the lines of sight `key`.
    pub fn built(&mut self, key: Vec<Option<bool>>) {
        self.built = Some(key);
    }
}

/// When a kind of change last built the whole scene: a change that comes every tick does not build every frame.
#[derive(Debug, Default, Clone, Copy)]
pub struct Pace {
    last: Option<Instant>,
}

impl Pace {
    /// Whether a build may run at `now`, at least `every` after the last one allowed (it is counted as run when it may).
    pub fn allow(&mut self, now: Instant, every: Duration) -> bool {
        if self
            .last
            .is_some_and(|t| now.saturating_duration_since(t) < every)
        {
            return false;
        }
        self.last = Some(now);
        true
    }
}

/// A renderer (or anything slow to make) made on a worker thread: the window keeps drawing its current look and takes the result
/// when [`Warm::poll`] has it.
pub struct Warm<T> {
    pub started: Instant,
    rx: mpsc::Receiver<Result<T, String>>,
}

impl<T: Send + 'static> Warm<T> {
    /// Starts `job` on a thread named `name`.
    pub fn start(name: &str, job: impl FnOnce() -> Result<T, String> + Send + 'static) -> Warm<T> {
        let (tx, rx) = mpsc::channel();
        // A browser has no threads: the job runs now, on the caller's.
        if cfg!(target_arch = "wasm32") {
            let _ = tx.send(job());
            return Warm {
                started: Instant::now(),
                rx,
            };
        }
        let spawned = std::thread::Builder::new()
            .name(name.into())
            .spawn(move || {
                let _ = tx.send(job());
            });
        if let Err(e) = spawned {
            let (tx, rx2) = mpsc::channel();
            let _ = tx.send(Err(format!("the warm-up thread: {e}")));
            return Warm {
                started: Instant::now(),
                rx: rx2,
            };
        }
        Warm {
            started: Instant::now(),
            rx,
        }
    }

    /// The result once it is ready (Err: the job failed, or its thread ended without one).
    pub fn poll(&self) -> Option<Result<T, String>> {
        match self.rx.try_recv() {
            Ok(r) => Some(r),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => Some(Err("the warm-up thread ended".into())),
        }
    }

    /// Waits for the result.
    pub fn wait(self) -> Result<T, String> {
        self.rx.recv().map_err(|e| e.to_string())?
    }
}

#[cfg(test)]
#[path = "view_tests.rs"]
mod tests;
