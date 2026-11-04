//! Compiles a TorqueScript source, runs it on the virtual machine with a native function the host registers, and prints
//! what the script printed and returned.
//!
//! `cargo run -p torque --example script`

use std::rc::Rc;

use torque::script::compiler::{self, Purpose};
use torque::script::text::atof;
use torque::script::vm::code::{CodeBlock, torque3d_format};
use torque::script::vm::vm::{Native, Vm};

const SOURCE: &str = r#"
function fib(%n)
{
    if (%n < 2)
        return %n;
    return fib(%n - 1) + fib(%n - 2);
}

function Greeter::greet(%this, %who)
{
    %this.count++;
    echo(%this.greeting SPC %who @ "!");
}

echo("fib(15) is" SPC fib(15));

%g = new ScriptObject(MyGreeter) { class = Greeter; greeting = "Hello"; count = 0; };
%g.greet("world");
%g.greet("again");
echo("greeted" SPC %g.count SPC "times");
%g.delete();

$answer = hostSquare(6) + 6;
return "the answer is" SPC $answer;
"#;

fn main() {
    // Compile the source to DSO bytecode, then load it as a code block.
    let file = b"example.cs";
    let out = compiler::compile(
        SOURCE.as_bytes(),
        Some(file),
        Purpose::Run,
        torque3d_format(),
    );
    assert!(!out.syntax_error, "syntax error: {:?}", out.diagnostics);
    let dso = out.dso.expect("compiled code");
    let block = CodeBlock::from_dso(&dso, Some(file), torque3d_format()).expect("a valid DSO");

    let mut vm = Vm::new();
    // A native function the script can call: `hostSquare(n)` (the name counts as the first argument).
    vm.register("hostSquare", 2, 2, "(n) the square of n", |_vm, call| {
        let n = atof(&call.argv[1]);
        Native::Float(n * n)
    });

    let result = vm.run(&Rc::new(block)).expect("the script runs");
    for line in vm.drain_log() {
        println!("console: {}", String::from_utf8_lossy(&line.text));
    }
    println!("returned: {}", String::from_utf8_lossy(&result));
    // The host reads a global the script set.
    let answer = vm.global("$answer").unwrap_or_default();
    println!("$answer = {}", String::from_utf8_lossy(&answer));
}
