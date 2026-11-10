//! A small timing bench of the runtime: fixed scripts per workload, each run in a fresh `Vm`, the median of a few short
//! runs. Run it in release: `cargo run --release -p torque-ts-vm --example bench -- [--classic] [runs]`. The Torque3D
//! executor is the default; `--classic` runs the same workloads through the classic executor and compiler (the dialect
//! games ship).
//!
//! Each line prints the workload, its median and best time, the median per operation where the workload counts them,
//! and a digest of the console lines and the result, so a change that alters what a script prints shows up as a
//! changed digest. `BENCH_ONLY=<name>` runs one workload. `bench-alloc` (beside this crate, its own workspace) runs
//! the same bench with mimalloc as the allocator.

use std::rc::Rc;
use std::time::{Duration, Instant};

use torque_ts_bytecode::DsoFile;
use torque_ts_bytecode::classic::ClassicDialect;
use torque_ts_compiler::classic::Profile;
use torque_ts_vm::code::{CodeBlock, torque3d_format};
use torque_ts_vm::exec::classic::{self, ClassicExecutor, TGE14_DIALECT};
use torque_ts_vm::host::{MemFiles, Purpose};
use torque_ts_vm::vm::Vm;

/// The executor and compiler a run uses.
#[derive(Clone, Copy, PartialEq)]
enum Dialect {
    Torque3d,
    Classic,
}

impl Dialect {
    fn vm(self) -> Vm {
        match self {
            Dialect::Torque3d => Vm::new(),
            Dialect::Classic => {
                let mut vm = Vm::with_executor(Rc::new(ClassicExecutor::new()));
                classic::install(&mut vm);
                vm
            }
        }
    }

    /// The compiled bytes of `src` on the current thread, and the compile's work.
    fn compile_here(self, src: &str) -> (Vec<u8>, u64) {
        match self {
            Dialect::Torque3d => {
                let out = torque_ts_compiler::compile_here(
                    src.as_bytes(),
                    Some(b"bench.cs"),
                    Purpose::Run,
                    torque3d_format(),
                );
                let dso = out.dso.expect("code");
                (dso.to_bytes(torque3d_format()), out.work)
            }
            Dialect::Classic => {
                let out = torque_ts_compiler::classic::compile_here(
                    src.as_bytes(),
                    Some(b"bench.cs"),
                    Purpose::Run,
                    &Profile::tge14(),
                );
                let dso = out.dso.expect("code");
                (ClassicDialect::tge14().to_bytes(&dso), out.work)
            }
        }
    }

    fn block(self, src: &str) -> Rc<CodeBlock> {
        match self {
            Dialect::Torque3d => {
                let out = torque_ts_compiler::compile(
                    src.as_bytes(),
                    Some(b"bench.cs"),
                    Purpose::Run,
                    torque3d_format(),
                );
                assert!(!out.syntax_error, "{:?}", out.diagnostics);
                let dso = out.dso.expect("code");
                Rc::new(
                    CodeBlock::from_dso(&dso, Some(b"bench.cs"), torque3d_format()).expect("loads"),
                )
            }
            Dialect::Classic => {
                let out = torque_ts_compiler::classic::compile(
                    src.as_bytes(),
                    Some(b"bench.cs"),
                    Purpose::Run,
                    &Profile::tge14(),
                );
                assert!(!out.syntax_error, "{:?}", out.diagnostics);
                let dso: DsoFile = out.dso.expect("code");
                let dialect = ClassicDialect::tge14();
                let read = dialect.parse(&dialect.to_bytes(&dso)).expect("reads back");
                Rc::new(
                    CodeBlock::from_dso(&read, Some(b"bench.cs"), &TGE14_DIALECT).expect("loads"),
                )
            }
        }
    }
}

/// One timed run: how long it took, a digest of what it produced, and how many operations it did (0 when the
/// workload is not counted per operation).
struct Run {
    took: Duration,
    digest: u64,
    ops: usize,
}

/// Script calls: a two-argument function called in a loop, and a method on a script object.
const CALLS: &str = "
function add(%a, %b) { return %a + %b; }
function Counter::bump(%this, %by) { %this.n = %this.n + %by; return %this.n; }
function calls(%n) {
    %s = 0;
    for (%i = 0; %i < %n; %i++)
        %s = add(%s, %i);
    %c = new ScriptObject(BenchCounter) { class = Counter; n = 0; };
    for (%i = 0; %i < %n / 4; %i++)
        %c.bump(2);
    %r = %s SPC %c.n;
    %c.delete();
    return %r;
}
echo(calls(100000));
";

/// Loops over locals and globals: integer, float and comparison work.
const LOOPS: &str = "
function loops(%n) {
    %s = 0;
    %f = 0.5;
    for (%i = 0; %i < %n; %i++) {
        %s = (%s + %i * 3) % 1000003;
        %f = %f * 1.0001;
        if (%i & 1)
            $odd++;
        else
            $even = $even + 2;
    }
    return %s SPC %f SPC $odd SPC $even;
}
echo(loops(300000));
";

/// Text natives and concatenation.
const STRINGS: &str = "
function strs(%n) {
    %line = \"alpha beta gamma delta epsilon zeta eta theta\";
    %len = 0;
    for (%i = 0; %i < %n; %i++) {
        %w = getWord(%line, %i % 8);
        %t = strReplace(%line, \"gamma\", %w);
        %c = getWordCount(%t);
        %out = %w @ \"_\" @ %i SPC %c TAB getWords(%t, 2, 4);
        %len = %len + strlen(%out) + strstr(%t, %w);
        %u = strupr(getSubStr(%out, 1, 6));
    }
    return %len SPC %u;
}
echo(strs(40000));
";

/// Objects: `new` with fields, field reads and writes, a set's members walked by index, then deleted.
const OBJECTS: &str = "
function objs(%n) {
    %set = new SimSet();
    for (%i = 0; %i < %n; %i++) {
        %o = new ScriptObject() { a = %i; b = \"x\" @ %i; };
        %o.c = %o.a + 1;
        %o.d[%i % 3] = %o.b;
        %set.add(%o);
    }
    %sum = 0;
    for (%k = 0; %k < 4; %k++)
        for (%i = 0; %i < %set.getCount(); %i++) {
            %o = %set.getObject(%i);
            %sum = %sum + %o.c + strlen(%o.b) + strlen(%o.d[%i % 3]);
        }
    while (%set.getCount() > 0)
        %set.getObject(0).delete();
    %set.delete();
    return %sum;
}
echo(objs(8000));
";

/// Numbers become text: int, float and string locals passed to a function and concatenated.
const NUMSTR: &str = "
function nsfn(%a, %b) { return %a @ \"-\" @ %b; }
function numstr(%n) {
    %f = 1.5;
    %len = 0;
    for (%i = 0; %i < %n; %i++) {
        %s = \"s\" @ %i;
        %t = nsfn(%i, %f);
        %u = nsfn(%s, %i + 1);
        %len = %len + strlen(%t) + strlen(%u) + strlen(%i @ %f);
    }
    return %len;
}
echo(numstr(100000));
";

/// Dynamic field writes and reads, plain and arrayed.
const FIELDS: &str = "
function fieldsw(%n) {
    %o = new ScriptObject();
    for (%i = 0; %i < %n; %i++) {
        %o.count = %o.count + 1;
        %o.slot[%i % 4] = %o.slot[%i % 4] + %i;
    }
    %r = %o.count SPC %o.slot[0] SPC %o.slot[3];
    %o.delete();
    return %r;
}
echo(fieldsw(200000));
";

/// The events the schedule workload posts.
const SCHEDULE_EVENTS: usize = 100_000;
/// The one-line evals the eval workload runs.
const EVALS: usize = 2_000;
/// The files and lines per file of the exec workload.
const EXEC_FILES: usize = 200;
const EXEC_LINES: usize = 50;
/// The calls each host-to-script workload makes.
const HOST_CALLS: usize = 200_000;

/// The number of datablocks the datablock workload declares.
const DATABLOCKS: usize = 6000;
/// The copies of the function bodies the compile workload's script holds.
const COMPILE_COPIES: usize = 150;

fn datablocks() -> String {
    let mut src = String::new();
    for i in 0..DATABLOCKS {
        src.push_str(&format!(
            "datablock SimDataBlock(BenchData{i}) {{ category = \"bench\"; index = {i}; scale = {}.5; \
             label = \"data number {i}\"; }};\n",
            i % 17
        ));
    }
    src.push_str("echo(BenchData7.label SPC BenchData5999.index);\n");
    src
}

fn compile_source() -> String {
    let mut src = String::new();
    for copy in 0..COMPILE_COPIES {
        for body in [CALLS, LOOPS, STRINGS, OBJECTS] {
            // Each copy's functions get their own names, so the whole file declares distinct functions.
            src.push_str(
                &body
                    .replace("function ", &format!("function c{copy}_"))
                    .replace("echo(", "//echo("),
            );
        }
    }
    src
}

/// FNV-1a over the bytes, for the digest of a run's output.
fn fnv(hash: &mut u64, bytes: &[u8]) {
    for b in bytes {
        *hash ^= u64::from(*b);
        *hash = hash.wrapping_mul(0x100_0000_01b3);
    }
}

const FNV_START: u64 = 0xcbf2_9ce4_8422_2325;

/// Hashes the console lines of `vm` into `hash` (and prints them with `BENCH_SHOW`).
fn digest_log(vm: &mut Vm, hash: &mut u64) {
    for line in vm.drain_log() {
        if std::env::var_os("BENCH_SHOW").is_some() {
            println!("  | {}", String::from_utf8_lossy(&line.text));
        }
        fnv(hash, &line.text);
        fnv(hash, b"\n");
    }
}

fn run_script(dialect: Dialect, block: &Rc<CodeBlock>, ops: usize) -> Run {
    let mut vm = dialect.vm();
    let start = Instant::now();
    let result = vm.run(block).expect("runs");
    let took = start.elapsed();
    let mut digest = FNV_START;
    fnv(&mut digest, &result);
    digest_log(&mut vm, &mut digest);
    Run { took, digest, ops }
}

fn run_compile(dialect: Dialect, src: &str) -> Run {
    let start = Instant::now();
    let (bytes, work) = dialect.compile_here(src);
    let took = start.elapsed();
    let mut digest = FNV_START;
    fnv(&mut digest, &bytes);
    fnv(&mut digest, &work.to_le_bytes());
    Run {
        took,
        digest,
        ops: 0,
    }
}

/// `eval` of a one-line text, `EVALS` times: what an extension that evaluates text per event pays.
fn run_eval(dialect: Dialect) -> Run {
    let mut vm = dialect.vm();
    let texts: Vec<String> = (0..EVALS)
        .map(|i| format!("$bench_eval = {i} + 1;"))
        .collect();
    let start = Instant::now();
    for text in &texts {
        vm.eval(text.as_bytes()).expect("evals");
    }
    let took = start.elapsed();
    let mut digest = FNV_START;
    fnv(&mut digest, &vm.global("$bench_eval").unwrap_or_default());
    digest_log(&mut vm, &mut digest);
    Run {
        took,
        digest,
        ops: EVALS,
    }
}

/// A generated source file of `EXEC_LINES` lines: functions and globals.
fn exec_file(n: usize) -> String {
    let mut src = String::new();
    for k in 0..EXEC_LINES / 2 {
        src.push_str(&format!(
            "function f{n}_{k}(%a, %b) {{ return %a + %b * {k}; }}\n$g{n}_{k} = f{n}_{k}({k}, 2) @ \"x\";\n"
        ));
    }
    src
}

/// `exec` of `EXEC_FILES` generated source files from memory.
fn run_exec(dialect: Dialect) -> Run {
    let mut vm = dialect.vm();
    let files = MemFiles::new();
    let paths: Vec<String> = (0..EXEC_FILES).map(|n| format!("bench/f{n}.cs")).collect();
    for (n, path) in paths.iter().enumerate() {
        files.insert(path, exec_file(n).as_bytes());
    }
    vm.set_file_host(files);
    let start = Instant::now();
    for path in &paths {
        assert!(vm.exec_file(path.as_bytes(), false).expect("execs"));
    }
    let took = start.elapsed();
    let mut digest = FNV_START;
    digest_log(&mut vm, &mut digest);
    fnv(&mut digest, &vm.global("$g7_3").unwrap_or_default());
    Run {
        took,
        digest,
        ops: EXEC_FILES,
    }
}

/// The host calling in: `Vm::invoke` of a global function and `Vm::call_method` of a script object's method.
const HOST_PRELUDE: &str = "
function add(%a, %b) { return %a + %b; }
function Counter::bump(%this, %by) { %this.n = %this.n + %by; return %this.n; }
new ScriptObject(HostCounter) { class = Counter; n = 0; };
";

fn run_host_calls(dialect: Dialect, prelude: &Rc<CodeBlock>) -> Run {
    let mut vm = dialect.vm();
    vm.run(prelude).expect("runs");
    let counter = vm.objects().find(b"HostCounter").expect("the object");
    let args = [b"3".to_vec(), b"4".to_vec()];
    let by = [b"2".to_vec()];
    let start = Instant::now();
    let mut digest = FNV_START;
    for _ in 0..HOST_CALLS {
        let sum = vm.invoke(None, b"add", &args).expect("invokes");
        fnv(&mut digest, &sum);
    }
    for _ in 0..HOST_CALLS {
        let n = vm.call_method(counter, b"bump", &by).expect("calls");
        fnv(&mut digest, &n);
    }
    let took = start.elapsed();
    digest_log(&mut vm, &mut digest);
    Run {
        took,
        digest,
        ops: 2 * HOST_CALLS,
    }
}

/// `schedule` across 1,000 objects, half of the events cancelled, then fired by `advance_time`: the host drives
/// the clock, the events run script.
const SCHEDULE: &str = "
function add(%a, %b) { $sum = $sum + %a + %b; }
function post(%n) {
    for (%k = 0; %k < 1000; %k++)
        $obj[%k] = new ScriptObject();
    for (%i = 0; %i < %n; %i++)
        $ev[%i] = schedule(%i % 50, $obj[%i % 1000], add, %i, 7);
    for (%i = 0; %i < %n; %i += 2)
        cancel($ev[%i]);
    return %n;
}
";

fn run_schedule(dialect: Dialect, block: &Rc<CodeBlock>) -> Run {
    let mut vm = dialect.vm();
    vm.run(block).expect("runs");
    let start = Instant::now();
    vm.invoke(None, b"post", &[SCHEDULE_EVENTS.to_string().into_bytes()])
        .expect("posts");
    let errors = vm.advance_time(100);
    let took = start.elapsed();
    assert!(errors.is_empty(), "{errors:?}");
    let mut digest = FNV_START;
    fnv(&mut digest, &vm.global("$sum").unwrap_or_default());
    digest_log(&mut vm, &mut digest);
    Run {
        took,
        digest,
        ops: SCHEDULE_EVENTS,
    }
}

fn report(name: &str, runs: usize, mut one: impl FnMut() -> Run) {
    // `BENCH_ONLY=<name>` runs only that workload.
    if std::env::var("BENCH_ONLY").is_ok_and(|only| only != name) {
        return;
    }
    let mut times = Vec::new();
    let mut digest = None;
    let mut ops = 0;
    for _ in 0..runs {
        let run = one();
        assert!(
            digest.is_none_or(|d| d == run.digest),
            "{name}: output differs between runs"
        );
        digest = Some(run.digest);
        ops = run.ops;
        times.push(run.took);
    }
    times.sort();
    let median = times[times.len() / 2];
    let per_op = if ops == 0 {
        String::new()
    } else {
        format!("  {:>8.3} us/op", median.as_secs_f64() * 1e6 / ops as f64)
    };
    println!(
        "{name:<10} median {:>8.2} ms  best {:>8.2} ms{per_op}  digest {:016x}",
        median.as_secs_f64() * 1e3,
        times[0].as_secs_f64() * 1e3,
        digest.unwrap_or(0)
    );
}

pub fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dialect = if args.iter().any(|a| a == "--classic") {
        Dialect::Classic
    } else {
        Dialect::Torque3d
    };
    let args: Vec<&String> = args.iter().filter(|a| !a.starts_with("--")).collect();
    // `bench <file.cs> [runs]` times one script file instead of the workloads.
    let file = args.first().filter(|a| a.ends_with(".cs"));
    let runs = args
        .get(usize::from(file.is_some()))
        .and_then(|a| a.parse().ok())
        .unwrap_or(5);
    if let Some(file) = file {
        let src = std::fs::read_to_string(file).expect("reads the script");
        let code = dialect.block(&src);
        report(file, runs, || run_script(dialect, &code, 0));
        return;
    }
    for (name, src, ops) in [
        ("calls", CALLS.to_string(), 125_000),
        ("loops", LOOPS.to_string(), 300_000),
        ("strings", STRINGS.to_string(), 40_000),
        ("objects", OBJECTS.to_string(), 8_000),
        ("numstr", NUMSTR.to_string(), 100_000),
        ("fields", FIELDS.to_string(), 200_000),
        ("datablocks", datablocks(), DATABLOCKS),
    ] {
        let code = dialect.block(&src);
        report(name, runs, || run_script(dialect, &code, ops));
    }
    let prelude = dialect.block(HOST_PRELUDE);
    report("host-calls", runs, || run_host_calls(dialect, &prelude));
    let schedule = dialect.block(SCHEDULE);
    report("schedule", runs, || run_schedule(dialect, &schedule));
    report("eval", runs, || run_eval(dialect));
    report("exec-files", runs, || run_exec(dialect));
    let src = compile_source();
    report("compile", runs, || run_compile(dialect, &src));
}
