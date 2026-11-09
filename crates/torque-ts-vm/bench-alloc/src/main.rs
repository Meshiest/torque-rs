//! The VM's bench (`examples/bench.rs`) with mimalloc as the global allocator.

#[path = "../../examples/bench.rs"]
mod bench;

#[global_allocator]
static ALLOC: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() {
    bench::main();
}
