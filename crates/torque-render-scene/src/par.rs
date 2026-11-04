//! Pure work of one frame split over a few threads (a live window's moving shapes: their vertices made and converted). The
//! threads are a pool of their own, made at the first call and kept (starting a thread a frame cost more than the work); each
//! call's results come back in the items' order, so the outcome is the one thread's.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use rayon::prelude::*;

/// The most threads the pool has (the calling thread waits for them).
pub const MAX_THREADS: usize = 8;
/// Below this much work (the caller's units: vertices) a call runs on the calling thread alone.
pub const MIN_WORK_A_THREAD: usize = 2_000;

static OFF: AtomicBool = AtomicBool::new(false);
static POOL: OnceLock<Option<rayon::ThreadPool>> = OnceLock::new();

/// Every call on the calling thread alone (`false`), or split again (a test compares the two).
pub fn set_enabled(on: bool) {
    OFF.store(!on, Ordering::Relaxed);
}

/// The pool's threads: one fewer than the cores (the rest of the process: the decoder, the static cells, the driver), at most
/// [`MAX_THREADS`]; none on a machine of one or two cores.
fn pool() -> Option<&'static rayon::ThreadPool> {
    POOL.get_or_init(|| {
        let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
        let threads = cores.saturating_sub(1).min(MAX_THREADS);
        (threads >= 2)
            .then(|| {
                rayon::ThreadPoolBuilder::new()
                    .num_threads(threads)
                    .thread_name(|i| format!("moving shapes {i}"))
                    .build()
                    .ok()
            })
            .flatten()
    })
    .as_ref()
}

/// Where the runs of items with `weights` start, and the end: one run per thread their sum is worth (`[0, n]`: one).
fn runs(weights: &[usize]) -> Vec<usize> {
    let threads = if OFF.load(Ordering::Relaxed) {
        1
    } else {
        pool().map_or(1, rayon::ThreadPool::current_num_threads)
    };
    let total: usize = weights.iter().sum();
    let threads = (total / MIN_WORK_A_THREAD)
        .clamp(1, threads.max(1))
        .min(weights.len().max(1));
    let mut bounds = vec![0usize];
    if threads > 1 {
        let share = total.div_ceil(threads).max(1);
        let mut acc = 0usize;
        for (i, w) in weights.iter().enumerate() {
            acc += w;
            if acc >= share * bounds.len() && bounds.len() < threads && i + 1 < weights.len() {
                bounds.push(i + 1);
            }
        }
    }
    bounds.push(weights.len());
    bounds
}

/// `f` of each item in order, the items split into runs of about equal `weight`, a run a thread of the pool.
pub fn map<T: Sync, R: Send>(
    items: &[T],
    weight: impl Fn(&T) -> usize,
    f: impl Fn(&T) -> R + Sync,
) -> Vec<R> {
    let weights: Vec<usize> = items.iter().map(&weight).collect();
    let bounds = runs(&weights);
    match pool() {
        Some(pool) if bounds.len() > 2 => {
            let runs: Vec<&[T]> = bounds.windows(2).map(|w| &items[w[0]..w[1]]).collect();
            let parts: Vec<Vec<R>> = pool.install(|| {
                runs.into_par_iter()
                    .map(|run| run.iter().map(&f).collect())
                    .collect()
            });
            parts.into_iter().flatten().collect()
        }
        _ => items.iter().map(f).collect(),
    }
}

/// [`map`] of owned items (each moved to the thread that takes it).
pub fn map_owned<T: Send, R: Send>(
    mut items: Vec<T>,
    weight: impl Fn(&T) -> usize,
    f: impl Fn(T) -> R + Sync,
) -> Vec<R> {
    let weights: Vec<usize> = items.iter().map(&weight).collect();
    let bounds = runs(&weights);
    match pool() {
        Some(pool) if bounds.len() > 2 => {
            // The runs from the last one back (split off the end), then in order again.
            let mut runs: Vec<Vec<T>> = Vec::with_capacity(bounds.len() - 1);
            for w in bounds.windows(2).rev() {
                runs.push(items.split_off(w[0]));
            }
            runs.reverse();
            let parts: Vec<Vec<R>> = pool.install(|| {
                runs.into_par_iter()
                    .map(|run| run.into_iter().map(&f).collect())
                    .collect()
            });
            parts.into_iter().flatten().collect()
        }
        _ => items.into_iter().map(f).collect(),
    }
}

#[cfg(test)]
#[path = "par_tests.rs"]
mod tests;
