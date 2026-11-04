# Console and scripting

torque-rs runs TorqueScript, the scripting language of the Torque engines, on a virtual machine written in Rust. Three
crates make up the scripting stack:

| Crate | What it does |
|---|---|
| `torque-ts-bytecode` | Reads and writes compiled script (DSO) files; knows the instruction set. |
| `torque-ts-compiler` | Turns TorqueScript source text into a DSO. |
| `torque-ts-vm` | The runtime: variables, namespaces, objects, natives, events, file loading, and the loop that runs code. |

A fourth small crate, `torque-ts-text`, holds the C-library behaviour the other three share (`atoi`, `atof`, `%g`
printing), so the compiler and the runtime parse and print numbers the same way.

## The runtime

`torque-ts-vm` is built around one type, `Vm`. It owns everything a script can see:

- global variables and function-local frames;
- namespaces and packages (`activatePackage`, `Parent::`, method lookup up a class chain);
- SimObjects, SimSets and SimGroups, datablocks, ids and names;
- the event queue behind `schedule`, with a virtual clock the host can drive;
- tagged strings, console types for fields, and the console log;
- a step budget and a call-depth limit, so a broken or hostile script ends with an error instead of hanging or
  overflowing the stack.

Embedding it takes a few lines:

```rust,ignore
use torque_ts_vm::vm::Vm;

let mut vm = Vm::new();
vm.eval(b"echo(1 + 2);").unwrap();
for line in vm.drain_log() {
    println!("{}", String::from_utf8_lossy(&line.text));
}
```

`Vm::invoke` calls a script function from Rust, `Vm::call_method` calls a method on an object, and `Vm::exec_file`
does what the `exec` function does. Files come from a `FileHost` (the in-memory `MemFiles`, or `DirHost` over real
folders) and time from a `Clock`; the host picks both.

## Executors

A `Vm` runs one dialect of compiled code. The loop that runs it is an `Executor`, chosen when the `Vm` is made. The
executor decides how code is evaluated: what stacks it keeps, how locals are stored, how a call passes its
arguments. Everything else is the same for every dialect, so natives, console classes and hosts are written once.

`Vm::new()` uses the default executor, `Torque3dExecutor`. A game with another instruction set passes its own
executor to `Vm::with_executor`. The trait (in `torque_ts_vm::exec`) is small: run a block from an offset, report
the last result, save and restore evaluation state around a run started from Rust, and report work done for the step
budget. An executor reaches back into the `Vm` through its public methods: frames, variables, call resolution,
object creation, typed field reads and writes, the budgets and the log. The Torque3D executor uses only that
interface, which keeps it open to other implementations.

### The default Torque3D executor

It follows the semantics of Torque3D's interpreter (MIT licence; the code here is this project's own):

- values live on one typed stack (integer, float or string) and are converted as each operation needs;
- function locals are registers, and each function has a section for its parameter defaults;
- script functions return typed values; natives receive text arguments and return a typed result;
- every block (a function, a file's top level, evaluated text) runs in a frame of its own.

Where it differs on purpose, hostile code is an error rather than undefined behaviour: registers are bounded, a stack
underflow stops the run, and a failed `assert` logs its message and the script carries on.

## The dialect

The default dialect is Torque3D's TorqueScript:

- compiled files are DSO version 50, with a local-variable table;
- `foreach` and `foreach$` loop over a set or a word list;
- `assert(condition, "message")`;
- internal names on objects (set when the object is declared, looked up with `->`, and read with `getInternalName`
  and `findObjectByInternalName`);
- `singleton` declarations;
- typed dynamic fields, where a field's value keeps the type it was written with.

The shape of a dialect is a `Dialect` value in `torque-ts-bytecode`: the version word, the file layout and the opcode
numbering. `Dialect::torque3d()` is the built-in profile; another build of the same format is another value.

## Natives and console classes

A native is a Rust function a script can call, registered by name with `Vm::register` (global) or `Vm::register_in`
(in a namespace, which makes it a method). It gets a `NativeCall` (the arguments as text and the current object) and
returns a `Native`: a string, an integer, a float, a bool, nothing, or a failure that ends the run. Registering a name
again replaces the native in place, so a game can override one the engine ships, even after a script has already
called it.

The shipped natives cover the console basics (`echo`, `warn`, `error`), strings, words, math, vectors and matrices,
files, tagged strings, variables, sets and objects, scheduling, and the profiler.

A console class is described by a `ClassSpec`: its name, parent, fields (with their types) and methods.
`install_specs(vm, &[ClassSpec])` registers a table of them and stubs any method that has no native yet. A class can
also have Rust behaviour through the `EngineClass` trait, set with `Vm::set_class_behaviour`; it hooks object
registration (`on_add`), removal, datablock preload and construction. `FileObject` is implemented in the crate, and a
game or another engine crate adds the rest of its classes the same way.

## Policies a game sets

The defaults are Torque3D's. A game that behaves differently installs its rules on the `Vm` once, when it makes it.
Each is a trait or a plain value.

| Setting | What it decides |
|---|---|
| `ResourceRules` | The resource manager's view of paths: which are refused, which folders may be written, which are scanned. Given to `MemFiles` and `DirHost`. |
| `ExecPolicy` | A check on a file `exec` is asked to run, made before the file is looked up. |
| `SimConfig` | How the object tables start: the sets and groups made at start-up, and where dynamic ids begin and wrap. |
| `ConsoleRules` | How a few failures are reported: unknown commands, `new` refusals, backtraces. |
| `Messages` | The text of the runtime's own console lines. |
| `FieldRules` | How console types parse and print, such as `%g` against `%f`, and the rotation maths. |
| `TagRules` | The tagged-string table's checks on dead entries and out-of-range ids. |
| `Limits` | Numeric limits: active packages, the datablock id range, the event batch size. |
| `BuildInfo` | What `getBuildString`, `getVersionNumber` and the other version natives answer. |
| `AllocPolicy` | What code may allocate. It can refuse an object registration or a counted `schedule`, and it is told when frames, objects and events come and go. |
| `DeclarePolicy` | Whether a script may declare a given function (to protect names, for example). |

Two more hooks complete the set: `FileRules` (recorded file lengths, name expansion) and `Extensions` (state a game
keeps beside the `Vm`, one value per type).

## The compiler

`torque-ts-compiler` is a scanner, an LALR(1) parser and a code generator. Every compile takes a `Dialect` and returns
an `Output`: the diagnostics as the console would print them, whether a syntax error was seen, the `DsoFile`, and the
work done in units a VM can charge to its step budget. The syntax tree (`ast`) is public, so another front end can feed
the code generator, and a code generator for another instruction set can walk the tree the parser builds. A compile
runs on a thread with a large stack, so a script that calls `eval` at the bottom of a deep recursion is safe.

Inside a `Vm` the compiler is a `Compiler` the host installs (`Vm::set_compiler`). `Vm::new()` installs the Torque3D
one, which `eval`, `compile` and `exec` of a source file use.

## The bytecode

`torque-ts-bytecode` is the file format. `DsoFile` reads and writes a compiled file: the global and per-function string
and float tables, the identifier table, the code words and the local-variable table. `decode` turns code words into
instructions and checks them for consistency. `dsotext` is a lossless, readable text form of a DSO. The compiler's
tests use it, and it is handy for writing a program by hand when the compiler cannot emit an instruction yet.
