//! The parts of the runtime's hostile sweeps that other crates' tests share (they include this file): a case runner that
//! bounds a case's time by the steps it was charged, the natives of a `Vm`, hostile argument values, and the calls of one
//! native from script.
#![allow(dead_code)]

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::time::{Duration, Instant};

use torque_ts_vm::vm::Vm;

/// The variable that makes a sweep long: that many rounds, and long arguments.
pub const ROUNDS_VAR: &str = "TORQUE_FUZZ_VM";
pub const SEED_VAR: &str = "TORQUE_FUZZ_SEED";

/// What a case may take besides its steps: the start of a run, a compile, a debug build on a busy machine.
const BASE: Duration = Duration::from_millis(300);

/// What one charged step may take. A step is an instruction or 64 bytes of work; work the budget does not see (a save of a
/// long string charged one step, a set scanned once per byte of text) costs microseconds to milliseconds a step.
const PER_STEP: Duration = Duration::from_micros(5);

fn env_number(var: &str) -> Option<u64> {
    std::env::var(var).ok()?.trim().parse().ok()
}

/// The rounds to run (`default` unless [`ROUNDS_VAR`] says) and whether this is a long run.
pub fn rounds(default: u64) -> (u64, bool) {
    match env_number(ROUNDS_VAR) {
        Some(n) => (n, true),
        None => (default, false),
    }
}

pub fn seed() -> u64 {
    env_number(SEED_VAR).unwrap_or(0x5eed_f022)
}

/// One case: its `Vm` and the steps of every run it made.
pub struct Case<'a> {
    pub vm: &'a mut Vm,
    steps: u64,
}

impl Case<'_> {
    /// Runs `f`, an outermost run of the `Vm` (a run, an `eval`, an `invoke`), and counts its steps.
    pub fn run<T>(&mut self, f: impl FnOnce(&mut Vm) -> T) -> T {
        let out = f(self.vm);
        self.steps = self.steps.saturating_add(self.vm.steps());
        out
    }

    /// Advances the clock by `ms`, counting the steps of the events it fired.
    pub fn advance(&mut self, ms: u32) {
        let advance = self.vm.advance_time_counted(ms);
        self.steps = self.steps.saturating_add(advance.steps);
    }
}

/// Runs `f` on `vm` with a step limit of `limit` a run; panics with `what` when it panicked, or took longer than the steps
/// of its runs allow.
pub fn hammer(what: &dyn Fn() -> String, vm: &mut Vm, limit: u64, f: impl FnOnce(&mut Case)) {
    vm.set_step_limit(Some(limit));
    let mut case = Case { vm, steps: 0 };
    let start = Instant::now();
    let outcome = catch_unwind(AssertUnwindSafe(|| f(&mut case)));
    let took = start.elapsed();
    if let Err(panic) = outcome {
        let message = panic
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| panic.downcast_ref::<&str>().map(|s| s.to_string()))
            .unwrap_or_default();
        panic!("{} panicked: {message}", what());
    }
    let steps = case.steps;
    let bound = BASE + PER_STEP * u32::try_from(steps).unwrap_or(u32::MAX);
    assert!(
        took <= bound,
        "{} took {took:?} for {steps} steps (at most {bound:?})",
        what()
    );
}

/// A native as the listing names it.
#[derive(Clone, Debug)]
pub struct Native {
    /// None for a global function.
    pub namespace: Option<String>,
    pub name: String,
    /// The argument counts, the name included; a `max` of 0 has no limit.
    pub min: usize,
    pub max: usize,
}

/// The natives of `vm`.
pub fn natives(vm: &Vm) -> Vec<Native> {
    vm.native_listing()
        .lines()
        .filter_map(|line| {
            let (head, rest) = line.split_once(' ')?;
            let (min, max) = rest.split(' ').next()?.split_once("..")?;
            let min = min.parse().ok()?;
            let max = if max == "*" { 0 } else { max.parse().ok()? };
            let (namespace, name) = match head.rsplit_once("::") {
                Some((namespace, name)) => (Some(namespace.to_string()), name.to_string()),
                None => (None, head.to_string()),
            };
            Some(Native {
                namespace,
                name,
                min,
                max,
            })
        })
        .collect()
}

/// The argument values: empty, negative, past 32 bits, not numbers, markup, and two `long` texts that share no byte.
pub fn hostile_values(long: usize) -> Vec<Vec<u8>> {
    let mut words = b"a b\tc\nd ".repeat(long / 8 + 1);
    words.truncate(long);
    vec![
        Vec::new(),
        b"-1".to_vec(),
        b"4294967296".to_vec(),
        b"-2147483649".to_vec(),
        b"nan".to_vec(),
        b"inf".to_vec(),
        b"-inf".to_vec(),
        b"1e308".to_vec(),
        b"<font:<br>\x01\x02".to_vec(),
        b"*".to_vec(),
        vec![b'x'; long],
        words,
    ]
}

/// The argument lists a native is called with: each value in every count it takes (up to three more than its least),
/// the two long texts against each other (a text against a set, a haystack against a needle), 64 arguments and none.
pub fn argument_lists(native: &Native, values: &[Vec<u8>]) -> Vec<Vec<Vec<u8>>> {
    let least = native.min.saturating_sub(1);
    let most = if native.max == 0 {
        least + 3
    } else {
        native.max - 1
    };
    let mut lists = Vec::new();
    for value in values {
        for count in least..=most.min(least + 2) {
            lists.push(vec![value.clone(); count]);
        }
    }
    let (text, other) = (&values[values.len() - 2], &values[values.len() - 1]);
    for count in least.max(2)..=most.min(least + 2).max(2) {
        let mut args = vec![other.clone(); count];
        args[0] = text.clone();
        lists.push(args);
    }
    lists.push(vec![b"1".to_vec(); 64]);
    lists.push(Vec::new());
    lists
}

/// Calls `native` with `args` from script, so the arguments are charged as a script's call charges them: the values are
/// globals, and a method is called on a new object of its namespace when script can make one, else through the
/// namespace.
pub fn call_from_script(case: &mut Case, native: &Native, args: &[Vec<u8>]) {
    let mut list = String::new();
    for (i, value) in args.iter().enumerate() {
        let name = format!("$FuzzArg{i}");
        case.vm.set_global_bytes(&name, value);
        if i > 0 {
            list.push_str(", ");
        }
        list.push_str(&name);
    }
    let name = &native.name;
    let src = match &native.namespace {
        None => format!("{name}({list});"),
        Some(ns) => format!(
            "%o = new {ns}(); if (isObject(%o)) %o.{name}({list}); else {ns}::{name}({list});"
        ),
    };
    case.run(|vm| {
        let _ = vm.eval(src.as_bytes());
    });
}
