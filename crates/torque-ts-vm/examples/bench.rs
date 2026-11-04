//! A small timing bench of the runtime with the Torque3D executor: fixed scripts per workload, each run in a fresh `Vm`,
//! the median of a few short runs. Run it in release: `cargo run --release -p torque-ts-vm --example bench [-- runs]`.
//!
//! Each line prints the workload, its median and best time and a digest of the console lines and the result, so a
//! change that alters what a script prints shows up as a changed digest.

use std::rc::Rc;
use std::time::{Duration, Instant};

use torque_ts_vm::code::{CodeBlock, torque3d_format};
use torque_ts_vm::host::Purpose;
use torque_ts_vm::vm::Vm;

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

fn block(src: &str) -> Rc<CodeBlock> {
    let out = torque_ts_compiler::compile(
        src.as_bytes(),
        Some(b"bench.cs"),
        Purpose::Run,
        torque3d_format(),
    );
    assert!(!out.syntax_error, "{:?}", out.diagnostics);
    let dso = out.dso.expect("code");
    Rc::new(CodeBlock::from_dso(&dso, Some(b"bench.cs"), torque3d_format()).expect("loads"))
}

/// FNV-1a over the bytes, for the digest of a run's output.
fn fnv(hash: &mut u64, bytes: &[u8]) {
    for b in bytes {
        *hash ^= u64::from(*b);
        *hash = hash.wrapping_mul(0x100_0000_01b3);
    }
}

fn run_script(block: &Rc<CodeBlock>) -> (Duration, u64) {
    let mut vm = Vm::new();
    let start = Instant::now();
    let result = vm.run(block).expect("runs");
    let took = start.elapsed();
    let mut hash = 0xcbf2_9ce4_8422_2325;
    fnv(&mut hash, &result);
    for line in vm.drain_log() {
        if std::env::var_os("BENCH_SHOW").is_some() {
            println!("  | {}", String::from_utf8_lossy(&line.text));
        }
        fnv(&mut hash, &line.text);
        fnv(&mut hash, b"\n");
    }
    (took, hash)
}

fn run_compile(src: &str) -> (Duration, u64) {
    let start = Instant::now();
    let out = torque_ts_compiler::compile_here(
        src.as_bytes(),
        Some(b"bench.cs"),
        Purpose::Run,
        torque3d_format(),
    );
    let took = start.elapsed();
    let dso = out.dso.expect("code");
    let mut hash = 0xcbf2_9ce4_8422_2325;
    fnv(&mut hash, &dso.to_bytes(torque3d_format()));
    fnv(&mut hash, &out.work.to_le_bytes());
    (took, hash)
}

fn report(name: &str, runs: usize, mut one: impl FnMut() -> (Duration, u64)) {
    // `BENCH_ONLY=<name>` runs only that workload.
    if std::env::var("BENCH_ONLY").is_ok_and(|only| only != name) {
        return;
    }
    let mut times = Vec::new();
    let mut digest = None;
    for _ in 0..runs {
        let (took, hash) = one();
        assert!(
            digest.is_none_or(|d| d == hash),
            "{name}: output differs between runs"
        );
        digest = Some(hash);
        times.push(took);
    }
    times.sort();
    println!(
        "{name:<10} median {:>8.2} ms  best {:>8.2} ms  digest {:016x}",
        times[times.len() / 2].as_secs_f64() * 1e3,
        times[0].as_secs_f64() * 1e3,
        digest.unwrap_or(0)
    );
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // `bench <file.cs> [runs]` times one script file instead of the workloads.
    let file = args.first().filter(|a| a.ends_with(".cs"));
    let runs = args
        .get(usize::from(file.is_some()))
        .and_then(|a| a.parse().ok())
        .unwrap_or(5);
    if let Some(file) = file {
        let src = std::fs::read_to_string(file).expect("reads the script");
        let code = block(&src);
        report(file, runs, || run_script(&code));
        return;
    }
    for (name, src) in [
        ("calls", CALLS.to_string()),
        ("loops", LOOPS.to_string()),
        ("strings", STRINGS.to_string()),
        ("objects", OBJECTS.to_string()),
        ("datablocks", datablocks()),
    ] {
        let code = block(&src);
        report(name, runs, || run_script(&code));
    }
    let src = compile_source();
    report("compile", runs, || run_compile(&src));
}
