//! The thread [`compile`](crate::compile) runs on: one per calling thread, started by its first compile and kept until
//! the caller exits (starting a thread with the big stack costs about 90 us, more than compiling a short script). Both
//! sides block on a channel, so an idle worker uses no CPU.

use std::cell::RefCell;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::mpsc::{self, Sender};

use crate::WORKER_STACK;

type Job = Box<dyn FnOnce() + Send>;

thread_local! {
    static WORKER: RefCell<Option<Sender<Job>>> = const { RefCell::new(None) };
}

fn spawn() -> Option<Sender<Job>> {
    let (jobs, queue) = mpsc::channel::<Job>();
    std::thread::Builder::new()
        .name("ts-compiler".into())
        .stack_size(WORKER_STACK)
        .spawn(move || {
            // Blocks while idle: no CPU is used between compiles.
            for job in queue {
                job();
            }
        })
        .ok()?;
    Some(jobs)
}

/// Runs `job` on the calling thread's worker and waits for its result. None when the worker could not be started or
/// the job panicked (the worker lives on after a panic).
pub(crate) fn run<T: Send + 'static>(job: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    let (reply, answer) = mpsc::sync_channel(1);
    let mut job: Job = Box::new(move || {
        let _ = reply.send(catch_unwind(AssertUnwindSafe(job)).ok());
    });
    WORKER.with(|slot| {
        let mut slot = slot.borrow_mut();
        // A second try covers a worker that is gone (its thread ended some other way).
        for _ in 0..2 {
            let jobs = match &*slot {
                Some(jobs) => jobs,
                None => slot.insert(spawn()?),
            };
            match jobs.send(job) {
                Ok(()) => return answer.recv().ok().flatten(),
                Err(mpsc::SendError(back)) => {
                    job = back;
                    *slot = None;
                }
            }
        }
        None
    })
}
