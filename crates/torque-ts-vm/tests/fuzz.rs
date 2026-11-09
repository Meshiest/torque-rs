//! Hostile sweeps of the runtime, from a fixed seed: random programs and their mutated bytecode run through both
//! executors, long strings up to the size cap through what copies or scans them, and every native of the runtime called
//! from script with hostile arguments. Each case must end in `Ok` or `Err`, never a panic, and in time in proportion to the
//! steps its runs were charged (`sweep::hammer`).
//!
//! The default run is short. `TORQUE_FUZZ_VM=<rounds>` runs that many rounds, every long-string case at the size cap and
//! 1 MiB arguments for the natives, and `TORQUE_FUZZ_SEED=<n>` starts from another seed; a failure names the round and its
//! seed, so it can be run again.

#[path = "../../torque-ts-compiler/tests/common/mod.rs"]
mod common;
mod sweep;

use std::rc::Rc;

use common::Rng;
use common::program::random_program;
use sweep::{argument_lists, call_from_script, hammer, hostile_values, natives, rounds, seed};
use torque_ts_bytecode::DsoFile;
use torque_ts_compiler::classic::{Profile, compile as classic_compile};
use torque_ts_vm::code::{CodeBlock, DsoFormat, torque3d_format};
use torque_ts_vm::exec::classic::{self, ClassicExecutor, TGE14_DIALECT};
use torque_ts_vm::host::{MemFiles, Purpose};
use torque_ts_vm::vm::{MAX_STRING_BYTES, Vm, VmError};

/// The steps one run of a program case may take.
const STEPS: u64 = 200_000;

/// The steps one run of a long-string case may take: making the longest string takes about two million.
const LONG_STEPS: u64 = 6_000_000;

#[derive(Clone, Copy, Debug)]
enum Dialect {
    Torque3d,
    Classic,
}

impl Dialect {
    fn vm(self) -> Vm {
        let mut vm = match self {
            Dialect::Torque3d => Vm::new(),
            Dialect::Classic => {
                let mut vm = Vm::with_executor(Rc::new(ClassicExecutor::new()));
                classic::install(&mut vm);
                vm
            }
        };
        vm.set_file_host(MemFiles::new());
        vm
    }

    /// The code of `src`, and whether it had a syntax error.
    fn compile_checked(self, src: &[u8]) -> (Option<DsoFile>, bool) {
        let name = Some(&b"fuzz.cs"[..]);
        match self {
            Dialect::Torque3d => {
                let out = torque_ts_compiler::compile(src, name, Purpose::Run, torque3d_format());
                (out.dso, out.syntax_error)
            }
            Dialect::Classic => {
                let out = classic_compile(src, name, Purpose::Run, &Profile::tge14());
                (out.dso, out.syntax_error)
            }
        }
    }

    fn compile(self, src: &[u8]) -> Option<DsoFile> {
        self.compile_checked(src).0
    }

    fn format(self) -> &'static dyn DsoFormat {
        match self {
            Dialect::Torque3d => torque3d_format(),
            Dialect::Classic => &TGE14_DIALECT,
        }
    }
}

/// Changes one to four places of `dso`: code words flipped, replaced, inserted or deleted, string table bytes changed,
/// identifier positions moved.
fn mutate(rng: &mut Rng, dso: &mut DsoFile) {
    for _ in 0..1 + rng.below(4) {
        match rng.below(6) {
            0 | 1 if !dso.code.is_empty() => {
                let at = rng.below(dso.code.len());
                dso.code[at] ^= 1 << rng.below(32);
            }
            2 if !dso.code.is_empty() => {
                let at = rng.below(dso.code.len());
                dso.code[at] = {
                    let any = rng.next_u64() as u32;
                    *rng.pick(&[0, 1, 2, 0xffff, u32::MAX, any])
                };
            }
            3 => {
                let at = rng.below(dso.code.len() + 1);
                dso.code.insert(at, rng.next_u64() as u32 % 128);
            }
            4 if !dso.code.is_empty() => {
                let at = rng.below(dso.code.len());
                dso.code.remove(at);
            }
            _ if !dso.global_strings.is_empty() => {
                let at = rng.below(dso.global_strings.len());
                dso.global_strings[at] = rng.below(256) as u8;
                if let Some(entry) = dso.identifiers.first_mut()
                    && let Some(position) = entry.positions.first_mut()
                {
                    *position = position.wrapping_add(rng.below(3) as u32);
                }
            }
            _ => {}
        }
    }
}

#[test]
fn mutated_code_never_panics_the_executor() {
    let (rounds, _) = rounds(150);
    let seed = seed();
    for round in 0..rounds {
        let mut rng = Rng::new(seed ^ round.wrapping_mul(0x9e37_79b9));
        let (decls, depth) = (1 + rng.below(4), 1 + rng.below(4) as u32);
        let src = random_program(&mut rng, decls, depth);
        for dialect in [Dialect::Torque3d, Dialect::Classic] {
            let Some(clean) = dialect.compile(src.as_bytes()) else {
                continue;
            };
            for variant in 0..4 {
                let mut dso = clean.clone();
                if variant > 0 {
                    mutate(&mut rng, &mut dso);
                }
                let what = || {
                    format!("round {round} (seed {seed:#x}) {dialect:?} variant {variant}:\n{src}")
                };
                let mut vm = dialect.vm();
                hammer(&what, &mut vm, STEPS, |case| {
                    let Ok(block) = CodeBlock::from_dso(&dso, Some(b"fuzz.cs"), dialect.format())
                    else {
                        return;
                    };
                    case.run(|vm| {
                        let _ = vm.run(&Rc::new(block));
                    });
                    // The functions it declared, called with no arguments and with odd ones.
                    for name in ["foo", "Bar", "f", "x"] {
                        case.run(|vm| {
                            let _ = vm.invoke(None, name.as_bytes(), &[]);
                        });
                        case.run(|vm| {
                            let odd = [b"-1".to_vec(), b"nan".to_vec()];
                            let _ = vm.invoke(None, name.as_bytes(), &odd);
                        });
                    }
                });
            }
        }
    }
}

#[test]
fn random_programs_run_through_both_executors() {
    let (rounds, _) = rounds(150);
    let seed = seed() ^ 0x0bad_c0de;
    for round in 0..rounds {
        let mut rng = Rng::new(seed ^ round.wrapping_mul(0x9e37_79b9));
        let (decls, depth) = (1 + rng.below(5), 1 + rng.below(5) as u32);
        let src = random_program(&mut rng, decls, depth);
        for dialect in [Dialect::Torque3d, Dialect::Classic] {
            let what = || format!("round {round} (seed {seed:#x}) {dialect:?}:\n{src}");
            let mut vm = dialect.vm();
            hammer(&what, &mut vm, STEPS, |case| {
                case.run(|vm| {
                    let _ = vm.eval(src.as_bytes());
                });
                case.advance(100);
            });
        }
    }
}

/// What the long-string cases do with `%s`, a string of the case's length, and `%t`, one as long that shares no byte with
/// it: each repeats an operation that copies or scans a value it leaves in place, or keeps it, until the step limit.
const LONG_CASES: &[&str] = &[
    "while (1) { CHAIN%s; }",
    "%d = 1 @ %s; while (%d && 1) {}",
    "while (1) { foreach$ (%w in %t) break; }",
    "%o = new ScriptObject(); while (1) { %o.setInternalName(%s); %o.f[%s] = %s; %o.class = %s; }",
    "while (1) { $x = stripChars(%s, %t); }",
    "while (1) { $x = nextToken(%s, \"tok\", %t); }",
    "while (1) { $x = strstr(%s, %t) + getWordCount(%t) + strlen(getWord(%t, 1000)); }",
    "while (1) { $x = %s $= %t; $y = strcmp(%s, %s); $z = StripMLControlChars(%s); }",
    "while (1) { $g[%s] = 1; %u = $g[%s]; }",
    "while (1) { echo(%s, %t, %s); }",
];

/// `%s` and `%t` of `len` bytes (doubled up from a few bytes, then cut), then `body`.
fn long_case(len: usize, body: &str) -> String {
    let doublings = usize::BITS - len.max(1).leading_zeros();
    format!(
        "function long() {{ %s = \"x\"; %t = \"a b\"; \
         for (%i = 0; %i < {doublings}; %i++) {{ %s = %s @ %s; %t = %t @ %t; }} \
         %s = getSubStr(%s, 0, {len}); %t = getSubStr(%t, 0, {len}); {body} }} long();"
    )
}

#[test]
fn long_strings_stay_in_proportion() {
    let (_, long_run) = rounds(0);
    let seed = seed();
    let mut rng = Rng::new(seed);
    // Every case at 1 MiB; the first and a few chosen by the seed at about half the size cap (its doubling stops at the
    // cap itself), all of them in a long run.
    // The first case is an assignment chain as long as the parser takes, each link a save of the same value (into one
    // variable: four thousand would pass the memory limit first).
    let chain = "$g = ".repeat(4000);
    let bodies: Vec<String> = LONG_CASES
        .iter()
        .map(|c| c.replace("CHAIN", &chain))
        .collect();
    let mut cases: Vec<(usize, &str)> = bodies.iter().map(|c| (1 << 20, c.as_str())).collect();
    for (i, case) in bodies.iter().enumerate() {
        if long_run || i == 0 || rng.below(LONG_CASES.len()) < 2 {
            cases.push((MAX_STRING_BYTES / 2 - 1, case.as_str()));
        }
    }
    for (len, body) in cases {
        let src = long_case(len, body);
        for dialect in [Dialect::Torque3d, Dialect::Classic] {
            // TGE 1.4's grammar has no `foreach$`.
            if dialect.compile_checked(src.as_bytes()).1 {
                continue;
            }
            let shown = &body[..body.len().min(120)];
            let what = || format!("{dialect:?} {len} bytes (seed {seed:#x}): {shown}");
            let mut vm = dialect.vm();
            hammer(&what, &mut vm, LONG_STEPS, |case| {
                case.run(|vm| {
                    let end = vm.eval(src.as_bytes());
                    // Each loop runs to a limit (TGE 1.4's conditions read some of these texts otherwise).
                    let stopped = matches!(end, Err(VmError::StepLimit | VmError::MemoryLimit));
                    assert!(
                        stopped || matches!(dialect, Dialect::Classic),
                        "{}: {end:?}",
                        what()
                    );
                });
            });
        }
    }
}

#[test]
fn every_native_survives_hostile_arguments() {
    let (_, long_run) = rounds(0);
    let long = if long_run { 1 << 20 } else { 64 << 10 };
    let values = hostile_values(long);
    for dialect in [Dialect::Torque3d, Dialect::Classic] {
        let listed = natives(&dialect.vm());
        assert!(listed.len() > 100, "{dialect:?}: {} natives", listed.len());
        for native in &listed {
            for (i, args) in argument_lists(native, &values).iter().enumerate() {
                let what = || {
                    let shown: Vec<String> = args
                        .iter()
                        .map(|a| String::from_utf8_lossy(&a[..a.len().min(16)]).into_owned())
                        .collect();
                    format!("{dialect:?} {native:?} call {i} {shown:?}")
                };
                let mut vm = dialect.vm();
                hammer(&what, &mut vm, STEPS * 10, |case| {
                    call_from_script(case, native, args);
                });
            }
        }
    }
}
