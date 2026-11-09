//! The process list's own cost: one pass over 2,000 objects (a fifth of them ticking after another), the walk a server's
//! `advanceObjects` makes each 32 ms step, and the same pass after one `processAfter` made the list dirty. The allocations of
//! a pass are printed ahead of the timings (a counting allocator).

use std::hint::black_box;

use criterion::{Criterion, criterion_group};
use stats_alloc::{INSTRUMENTED_SYSTEM, Region, StatsAlloc};
use torque_sim::process::ProcessList;

#[global_allocator]
static GLOBAL: &StatsAlloc<std::alloc::System> = &INSTRUMENTED_SYSTEM;

const OBJECTS: u32 = 2_000;

/// 2,000 objects with ids from 1,000 on, every fifth ticking after the object 7 ids before it.
fn list() -> ProcessList<u32> {
    let mut list = ProcessList::new();
    for k in 0..OBJECTS {
        let id = 1_000 + k;
        let after = (k % 5 == 4 && k >= 7).then(|| id - 7);
        list.add(id, "Bench", k, after);
    }
    list.order_list_if_dirty();
    list
}

/// One pass: each object taken off, its payload read and put back, as the host's tick does.
fn pass(list: &mut ProcessList<u32>) -> u64 {
    list.order_list_if_dirty();
    list.begin_pass();
    let mut sum = 0u64;
    while let Some(v) = list.next_in_pass() {
        if let Some(p) = v.payload {
            sum += u64::from(p);
            list.restore(v.key, v.serial, p);
        }
    }
    list.finish_pass();
    sum
}

fn bench(c: &mut Criterion) {
    let mut l = list();
    c.bench_function("process list pass, 2000 objects", |b| {
        b.iter(|| black_box(pass(&mut l)))
    });
    let mut l = list();
    let mut flip = false;
    c.bench_function("process list pass, 2000 objects, dirty", |b| {
        b.iter(|| {
            // One edge changes each pass, so the pass sorts first.
            flip = !flip;
            l.process_after(1_001, flip.then_some(1_500));
            black_box(pass(&mut l))
        })
    });
}

criterion_group!(benches, bench);

fn main() {
    let mut l = list();
    pass(&mut l);
    let region = Region::new(GLOBAL);
    let sum = pass(&mut l);
    eprintln!(
        "process list: {} allocations a pass of {OBJECTS} (payload sum {sum})",
        region.change().allocations
    );
    benches();
    Criterion::default().configure_from_args().final_summary();
}
