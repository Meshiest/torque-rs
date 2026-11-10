# Script functions from Rust

A console function is a Rust function. The engine reads its arguments from the text a script passes, calls it, and turns
what it returns back into a script value. This chapter is about writing those functions: the attributes that declare them,
what their parameters can be, how text becomes a Rust value, and what goes wrong at compile time and at run time. The
[Plugins](plugins.md) chapter says where a function is registered; the [script reference](reference/script/index.md) lists
every function, method, class and pref the engine already has.

There are three ways to put a native into a script engine, from the lowest level up:

| Way | What you write | What the engine makes for you |
|---|---|---|
| `vm.register(name, min, max, usage, f)` | A closure over `&mut Vm` and the raw call (`argv` as bytes). | Nothing: the counts, the usage text and every conversion are yours. |
| `engine.script().function(ns, name, &[args], usage, f)` | A plain function whose parameters are typed. | The counts, the usage text (from the names you give), the conversions, the injected parameters. |
| `#[torque::function]` and `#[torque::methods]` | The same function with an attribute. | All of the above, with the names, the description and the docs taken from the signature and its doc comment. |

Write new natives with the attributes. The first way stays for code that has no `Engine` and for a native that needs the
raw call.

## A first function

```rust
use torque::engine::{EngineBuilder, Plugin, PluginId, Res, ServerStage};
use torque::script::vm::vm::Vm;
use torque::{function, natives};

/// What `greet` answers with, shared by every server the engine starts.
struct Greeting(String);

/// greets someone, loudly when asked
///
/// The greeting is the engine's `Greeting` resource.
#[function(name = "greet")]
fn greeting(greeting: Res<Greeting>, #[arg(name = "who")] person: &str, loud: Option<bool>) -> String {
    let text = format!("{}, {person}", greeting.0);
    if loud.unwrap_or(false) { text.to_uppercase() } else { text }
}

#[derive(PluginId)]
struct GreetPlugin;

impl Plugin for GreetPlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        engine.insert_resource(Greeting("Hello".into()));
        engine.script().natives(natives![greeting]);
    }
}

let engine = EngineBuilder::new().add_plugins(GreetPlugin).build()?;
let mut vm = Vm::new();
for &stage in ServerStage::ALL {
    engine.replay(stage, &mut vm)?;
}
assert_eq!(vm.eval(b"return greet(\"Ann\");")?, b"Hello, Ann");
assert_eq!(vm.eval(b"return greet(\"Ann\", true);")?, b"HELLO, ANN");
assert!(vm.native_listing().contains("greet 2..3 greet(who [, loud]) - greets someone, loudly when asked\n"));
# Ok::<(), Box<dyn std::error::Error>>(())
```

The attribute takes four things from the function:

- **The name.** The script name is the function's name in camelCase (`set_dead_zone` is `setDeadZone`); `name = "..."` picks
  another. A name is ASCII letters, digits and `_`, not starting with a digit, and scripts compare names without case.
- **The namespace.** None by default, so the function is global. `ns = "Pad"` makes it `Pad::setDeadZone(..)`.
- **The arguments.** Each parameter that reads a script argument is one argument, named after the Rust parameter in
  camelCase (`#[arg(name = "who")]` renames one in the usage text). Parameters the engine fills in are invisible to scripts;
  see the next section.
- **The description.** The first line of the doc comment goes after the signature in the usage text. The whole comment is
  kept and appears on the function's page in the script reference. `description = "..."` replaces the first line (an empty
  one leaves it off).

The rest of the options are `usage = "..."`, which keeps a usage text exactly as written (for a native whose text must not
change), `dev`, which makes it a development command, and `crate = path`, for when the manifest does not name the engine
crate. A development command runs in a debug build, or in a release build once a script sets `$Pref::Dev` to true; in
any other case it prints an error and returns the empty string. It is registered either way, so the natives a build has
do not depend on its profile.

`natives![...]` lists functions, and impl blocks (below), for the registry. `engine.script()` registers them in every server
and client script engine; `engine.server_script()` and `engine.client_script()` do it for one side. Two natives with one
name on one side fail the build and name both plugins; two in one list fail to compile.

## Parameters

A parameter is either a **script argument** or an **injected** one.

**Script arguments** come from the text the script passes, one parameter for each argument, in order:

| Parameter type | Takes |
|---|---|
| `i32`, `u32`, `f32`, `bool` | one argument, read by the conversion rules below |
| `&str`, `String`, `Cow<str>`, `Filename` | one argument as text |
| `&[u8]` | one argument as bytes, as they are |
| `Option<T>` | one argument, or none; `T` is one of the types above |
| `Rest` | every argument left |

The argument counts come from the order. Required arguments come first, then `Option`s, then at most one `Rest` last. Put an
`Option` before a required argument and the crate does not compile. The usage text shows the optional ones in brackets
(`getWords(text, first [, last])`) and the rest with dots (`schedule(delay, method, args...)`), and the native is registered
with those counts, so a script that passes the wrong number of arguments gets the usual warning with the usage text and the
native never runs.

```rust,ignore
/// the words from first to last
#[function]
fn get_words(text: &str, first: i32, last: Option<i32>) -> String { /* ... */ }
// getWords 3..4 getWords(text, first [, last]) - the words from first to last

/// calls a method of the object, later
#[function(ns = "Sched")]
fn after(delay: i32, method: &str, args: Rest) { /* args.iter(), args.parse::<i32>() ... */ }
// Sched::after 3..* Sched::after(delay, method, args...) - calls a method of the object, later
```

**Injected parameters** are what the function needs from the engine. Their types say which, and a script never sees them:

| Parameter type | What it is |
|---|---|
| `VmMut` | The script engine, to change. At most one per function. |
| `VmRef` | The script engine, to read. Any number, but never beside a `VmMut`. |
| `Res<T>` | The engine's resource of type `T`, to read. |
| `ResMut<T>` | The resource, to change. |
| `Slot<dyn Trait>` | The value of an exclusive slot a plugin set. |
| `&Engine` or `Engine` | The engine itself (the second is a clone). |
| `Local<T>` | State of the function's own, kept between calls; `T: Default`. A native's lasts as long as its script engine. |
| `This` | A method's object. For methods only. |

Resources are checked when the function runs, as a `RefCell` checks them. Hold a `ResMut<T>` while running a script that
calls a native which wants the same resource and that second call gets an error naming `T`, not a panic. The same goes for
`Local<T>` when a function is called again from inside itself. An injected parameter needs no wiring beyond the plugin that inserts the resource or sets the slot.

```rust
use torque::engine::{EngineBuilder, Local, Plugin, PluginId, ResMut, ServerStage, VmMut};
use torque::script::vm::vm::Vm;
use torque::{function, natives};

struct Score(i32);

/// adds points to the score and returns the total
#[function(ns = "Game")]
fn add_score(mut score: ResMut<Score>, points: i32) -> i32 {
    score.0 += points;
    score.0
}

/// how many times this script engine asked
#[function(ns = "Game", description = "")]
fn calls(mut count: Local<u32>, mut vm: VmMut) -> u32 {
    *count += 1;
    vm.set_global("$Game::Calls", &count.to_string());
    *count
}

#[derive(PluginId)]
struct ScorePlugin;

impl Plugin for ScorePlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        engine.insert_resource(Score(0));
        engine.script().natives(natives![add_score, calls]);
    }
}

let engine = EngineBuilder::new().add_plugins(ScorePlugin).build()?;
let mut vm = Vm::new();
for &stage in ServerStage::ALL {
    engine.replay(stage, &mut vm)?;
}
assert_eq!(vm.eval(b"Game::addScore(5); return Game::addScore(3);")?, b"8");
assert_eq!(vm.eval(b"Game::calls(); return Game::calls();")?, b"2");
assert_eq!(vm.global("$Game::Calls").as_deref(), Some(&b"2"[..]));
# Ok::<(), Box<dyn std::error::Error>>(())
```

A recipe (`engine.recipe(stage, f)`) takes the same injected parameters, and no script arguments. Everything in this
section applies to it too.

## How text becomes a Rust value

A script passes text; TorqueScript's own rules turn it into numbers, and the engine's natives follow the same rules as the
ones written by hand, so a typed native behaves like one that reads `argv` itself.

| Rust type | Rule | Examples |
|---|---|---|
| `i32` | `dAtoi`: leading white space, a sign, decimal digits, saturating. Anything else reads as 0. | `"12abc"` is 12, `" -3"` is -3, `"0x10"` is 0, `"abc"` is 0 |
| `u32` | `dAtoi`, its bits read as unsigned. | `"-1"` is 4294967295 |
| `f32` | `dAtof`, rounded to a 32-bit float. | `"1.5e2"` is 150, `"x"` is 0 |
| `bool` | `dAtob`: `true` in any case, or a number that is not 0 once it is a float. | `"True"` and `"2"` are true, `"0"`, `"1e-50"` and `""` are false |
| `&str` | The text up to the first NUL. A text that is not UTF-8 is refused: the call fails with an error and the native does not run. | |
| `String`, `Cow<str>`, `Filename` | The same text, with each byte sequence that is not UTF-8 replaced by U+FFFD. | |
| `&[u8]` | The bytes up to the first NUL, as they are. | |
| `Option<T>` | `None` when the script left the argument out; otherwise `Some` of what `T` reads. | |
| `Rest` | Every remaining argument, as passed; `rest.parse::<T>()` reads each as `T`. | |

A number never fails to read: text that is not a number reads as 0, as it does in the original engine. Only `&str` can refuse.

What a function returns goes back the same way:

| Return type | The script gets |
|---|---|
| `()` | Nothing (the empty string). |
| `i32` | An integer. |
| `u32` | The 32 bits as a signed integer (the way a script prints a word it holds unsigned). |
| `f32` | A float, widened exactly. |
| `bool` | `1` or `0`. |
| `String`, `&'static str`, `Filename` | Text. |
| `Native` | The result as it is, for a native that picks the kind itself. |
| `Result<T, ScriptError>` | The `T`, or an error (below). |

Unsigned, 64-bit, `usize` and `f64` values have no conversion, on purpose: the console has 32-bit integers and floats, so
a parameter or result of another width does not compile. Convert at the edge (`i32::try_from(n)`).

## Errors

A native reports a failure by returning `Err(ScriptError::new("..."))` from a function that returns
`Result<T, ScriptError>`. The console prints an error line with the function's name and the message, a warning with the
usage text, and the call returns the empty string.

```rust
use torque::engine::{EngineBuilder, Plugin, PluginId, ScriptError, ServerStage};
use torque::script::vm::vm::Vm;
use torque::{function, natives};

/// the share each of `people` gets
#[function]
fn split(total: f32, people: i32) -> Result<f32, ScriptError> {
    if people <= 0 {
        return Err(ScriptError::new("people must be at least 1"));
    }
    Ok(total / people as f32)
}

#[derive(PluginId)]
struct SplitPlugin;

impl Plugin for SplitPlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        engine.script().natives(natives![split]);
    }
}

let engine = EngineBuilder::new().add_plugins(SplitPlugin).build()?;
let mut vm = Vm::new();
for &stage in ServerStage::ALL {
    engine.replay(stage, &mut vm)?;
}
assert_eq!(vm.eval(b"return split(9, 3);")?, b"3");
assert_eq!(vm.eval(b"return split(9, 0);")?, b"");
let log: Vec<String> = vm.drain_log().iter().map(|l| String::from_utf8_lossy(&l.text).into_owned()).collect();
assert!(log.iter().any(|l| l.contains("split: people must be at least 1")));
assert!(log.iter().any(|l| l.contains("usage: split(total, people) - the share each of `people` gets")));
# Ok::<(), Box<dyn std::error::Error>>(())
```

The same line format covers the failures the engine finds itself: an argument that is not UTF-8 text for a `&str`
(`argument 2: the argument is not valid UTF-8 text`), a resource no plugin inserted (``the resource `Score` is missing: no
plugin inserted it``), a resource that is borrowed (``the resource `Score` is already borrowed: ...``), a slot no plugin
set, and a `Local` used again from inside itself. They print as an error without the usage line, and the native does not run.

A panic in a native is a bug in the native, not a script error. Return an `Err` for anything a script can cause.

## Methods

`#[torque::methods]` goes on an impl block. Every `pub fn` of an inherent block becomes a method of the class the attribute
names (`class = "Player"`; the type's name when left out), and so does every fn of a trait impl for a concrete type. A method
is called on an object, `%obj.giveAmmo(10)`, and found up the class's parents.

```rust
use torque::engine::{EngineBuilder, ServerStage, This, VmRef};
use torque::script::vm::vm::Vm;
use torque::{methods, natives};

/// The Rust data an object keeps, in `SimObject::ext`.
struct Ammo {
    rounds: i32,
}

#[methods(class = "SimObject")]
impl Ammo {
    /// adds rounds, up to the most given, and returns the rounds now
    pub fn give_ammo(&mut self, amount: i32, max: Option<i32>) -> i32 {
        self.rounds = (self.rounds + amount).min(max.unwrap_or(i32::MAX));
        self.rounds
    }

    /// the object's name
    #[torque(name = "label")]
    pub fn name_of(vm: VmRef, this: This) -> String {
        let name = vm.objects().get(this.0).and_then(|o| o.name()).unwrap_or_default();
        String::from_utf8_lossy(name).into_owned()
    }

    #[torque(skip)]
    pub fn rounds(&self) -> i32 {
        self.rounds
    }
}

let mut builder = EngineBuilder::new();
builder.script().natives(natives![Ammo]);
let engine = builder.build()?;
let mut vm = Vm::new();
for &stage in ServerStage::ALL {
    engine.replay(stage, &mut vm)?;
}
vm.eval(b"new SimObject(Crate);")?;
let id = vm.objects().find(b"Crate").expect("the object");
vm.objects_mut().get_mut(id).expect("the object").ext = Some(Box::new(Ammo { rounds: 5 }));
assert_eq!(vm.eval(b"return Crate.giveAmmo(10, 12);")?, b"12");
assert_eq!(vm.eval(b"return Crate.label();")?, b"Crate");
# Ok::<(), Box<dyn std::error::Error>>(())
```

How a method reaches its object depends on its receiver:

- **`&self` or `&mut self`.** The method runs on the object's Rust data, its `SimObject::ext`, which must be the impl
  block's type. The engine takes the data out of the object for the call, so the method may also take `VmMut` or `VmRef`.
  An object without that data is a console error, not a panic.
- **No receiver.** The method takes `This` for the object's id, and finds what it needs through `VmRef` or `VmMut`.

On a method, `#[torque(skip)]` leaves a fn out, `#[torque(name = "...")]` renames it, and `usage` and `description` work as
they do on a function. Parameters work as for a function, and `natives![Ammo]` lists the whole block. A type takes one
`#[methods]` block, because the attribute implements the listing trait for it; a second block names its list:
`#[methods(class = "SimObject", list = AmmoTools)]`, listed as `natives![Ammo, AmmoTools]`. Two methods of a block with the
same name (without case) do not compile.

## Prefs

A pref is a typed `$Pref::...` global with a default. `#[derive(Prefs)]` declares a struct of them:

```rust
use torque::Prefs;
use torque::engine::{EngineBuilder, Plugin, PluginId, ServerStage, VmPrefs};
use torque::script::vm::vm::Vm;

#[derive(Prefs)]
#[prefs(prefix = "$Pref::Pad")]
struct PadPrefs {
    /// How far the stick must move before it counts.
    #[pref(default = 0.15, range = 0.0..=0.9)]
    dead_zone: f32,
    #[pref(default = true)]
    invert_y: bool,
    #[pref(default = "default", name = "Profile")]
    active_profile: String,
}

#[derive(PluginId)]
struct PadPlugin;

impl Plugin for PadPlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        engine.script().prefs::<PadPrefs>();
    }
}

let engine = EngineBuilder::new().add_plugins(PadPlugin).build()?;
let mut vm = Vm::new();
for &stage in ServerStage::ALL {
    engine.replay(stage, &mut vm)?;
}
// The defaults are set before any script runs.
assert_eq!(vm.global("$Pref::Pad::DeadZone").as_deref(), Some(&b"0.15"[..]));
// A script may write anything; Rust reads it clamped to the range.
vm.eval(b"$Pref::Pad::DeadZone = 5;")?;
assert_eq!(vm.prefs::<PadPrefs>().dead_zone, 0.9);
assert_eq!(vm.prefs::<PadPrefs>().active_profile, "default");
# Ok::<(), Box<dyn std::error::Error>>(())
```

- **`prefix`** is the globals' common start; a field's global is the prefix, `::` and the field's name in PascalCase
  (`dead_zone` is `$Pref::Pad::DeadZone`). `name = "..."` changes the last part.
- **`default`** is required, a literal of the field's type (`0.15`, `true`, `"text"`, `-1`). It is set before any script
  runs, so a game's `export("$Pref::*", "prefs.cs")` writes it and a saved file that sets the global again wins.
- **`range = low..=high`** clamps what Rust reads. The global itself keeps what the script wrote.
- **`doc = ".."`** says what a pref is for when the doc comment is not it. Doc comments and ranges appear in the script
  reference's prefs page.

The field types are `i32`, `u32`, `f32`, `bool` and `String`, read by the rules above. A native or a recipe can ask for
the current values as a parameter, `Pref<PadPrefs>` (never beside a `VmMut`), and `engine.export_prefs(&mut vm, file)`
writes exactly the registered prefs.

## Classes

A console class with typed fields is a struct with `#[derive(EngineClass)]`:

```rust
use torque::EngineClass;
use torque::engine::{EngineBuilder, Filename, Plugin, PluginId, ScriptClass, ServerStage};
use torque::script::vm::vm::Vm;

#[derive(EngineClass)]
#[class(name = "RopeData", parent = "SimDataBlock")]
struct RopeData {
    /// How many pieces the rope is cut into.
    #[field(range = 2..=64, default = 8)]
    segments: i32,
    length: f32,
    material: Filename,
}

#[derive(PluginId)]
struct RopePlugin;

impl Plugin for RopePlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        engine.script().class::<RopeData>();
    }
}

let engine = EngineBuilder::new().add_plugins(RopePlugin).build()?;
let mut vm = Vm::new();
for &stage in ServerStage::ALL {
    engine.replay(stage, &mut vm)?;
}
vm.eval(b"datablock RopeData(Hawser) { segments = 100; length = 2.5; };")?;
let id = vm.objects().find(b"Hawser").expect("the datablock");
let rope = RopeData::read(&vm, id)?;
assert_eq!(rope.segments, 64); // the write was clamped to the range
assert_eq!(rope.length, 2.5);
# Ok::<(), Box<dyn std::error::Error>>(())
```

`#[class(name, parent)]` gives the class and what it goes below (`SimObject` when left out). Each field is a console field
named in camelCase, with an optional `#[field(name, doc, range, default)]`. The field types are `i32`, `u8`, `u16`, `f32`,
`bool`, `String`, `Filename` (a path: `./` and `~/` are expanded against the running script) and the arrays `[f32; 3]` (a
point), `[f32; 2]`, `[i32; 2]`, `[f32; 4]` (a color) and `[u8; 4]`. A script writes them by the console's rules for the
console type. A write outside `range` is clamped with a warning. `obj.dump()` lists the documented fields under
`Field Docs:` with their ranges, and the script reference's classes page has every field with its type and range, and its default for a class declared this way.

`preload = path` names a function `fn(&Self, &mut Vm, ObjId) -> Result<(), String>` that runs for each datablock of the
class once it is registered, with its fields read into the struct. An `Err` prints the preload failure with its text and
deletes the datablock. Only datablocks are preloaded, so the hook belongs on a class below `SimDataBlock`.

## Compile errors you will see

The attributes check a signature when the crate is built, so a function the engine cannot run never reaches a script. Some of
the messages come from the macros; the ones that say `evaluation panicked` come from a constant that fails to evaluate (error
`E0080`), which `cargo build` reports and `cargo check` may not.

**A parameter that is neither a script argument nor injected** (`Vec<i32>`, `u64`, `f64`, a type of your own):

```text
error[E0277]: `fn(Vec<i32>) {f}` is not a native: each parameter must be a script argument or an injected `Param`
error[E0277]: `Vec<i32>` is neither a script argument nor an injected parameter
```

A result type that is not a script value (`f64`, a `Vec`) gets the first line only, with the function's type in it
(`fn() -> f64 {f}`). The line talks about parameters, but when they are all fine, look at the result.

**An `Option` before a required argument:**

```text
error[E0080]: evaluation panicked: a native's script arguments go required first, then `Option`s, then one `Rest` last
```

**Two `VmMut`s, or a `VmMut` beside a `VmRef`:**

```text
error[E0080]: evaluation panicked: a native takes at most one `VmMut`
error[E0080]: evaluation panicked: a native takes either one `VmMut` or any number of `VmRef`, never both
```

**`self` in a free function:**

```text
error: `self` in a free function: a native that is called on an object is a method, declared with `#[methods(class = "...")]` on its impl block
```

**Two natives with one name**, in one `natives![..]` list or in one `#[methods]` block (names compare without case):

```text
error[E0080]: evaluation panicked: two natives of this list have the same name (names compare without case): rename one with `name = "..."`
error: two methods of this block are called `IsArmed` (`isArmed` came first; names compare without case): rename one with `#[torque(name = "...")]` or leave one out with `#[torque(skip)]`
```

Two plugins that declare one name do not fail the compiler but the engine's build, with an error that names both plugins.

**A name that is not a script name, or an option the attribute does not have:**

```text
error: a script name is ASCII letters, digits and '_', not starting with a digit
error: expected `ns = "..."`, `name = "..."`, `usage = "..."`, `description = "..."`, `dev` or `crate = path`
```

**A method that is not `pub`, with a `#[torque(..)]` option, and `#[methods]` on a trait definition:**

```text
error: only `pub` fns of an inherent block become methods: make it `pub`, or drop the attribute
error: `#[methods]` goes on an impl block for a concrete type, not on a trait definition: put it on `impl Trait for Type`
```

**A function named like a type or module in scope** (`fn net()` beside `mod net`):

```text
error[E0428]: the name `net` is defined multiple times
```

This one needs a word. The `function` attribute adds a hidden type with the function's own name next to it (types and
functions have separate names, so both fit), and `natives![net]` finds the declaration through that type. A `mod net`, a
`struct net` or an import of that name in the same scope collides with it. Rename the function and give the script name with
`name = "net"`, or move the function into a module of its own.

**Prefs and classes** have their own checks, all at compile time:

```text
error: a pref needs a default: `#[pref(default = ..)]`
error[E0308]: mismatched types                       (a default of the wrong type: "fast" for an f32)
error[E0080]: evaluation panicked: the default is outside the range
error[E0277]: `Vec<u8>` cannot be a pref: use `i32`, `u32`, `f32`, `bool` or `String`
error[E0277]: `Vec<i32>` cannot be a console field: use `i32`, `u8`, `u16`, `f32`, `bool`, `String`, `Filename`, or a point or color array
error: a console class is a struct with named fields
```

A default that is not a literal, a range that is not `low..=high`, and two fields whose globals (or console fields) are the same
name without case are errors as well. The crate's `trybuild` tests keep these messages from drifting.

## Where the docs go

The doc comment on a native, a pref or a class field is data, not only documentation for Rust readers. The usage text has
its first line; the script reference has the rest. `just script-reference` rebuilds the reference from the script engines of
a default engine, so a native you add shows up there the next time it is run, and CI fails when the
checked-in pages are stale. Write the first line as a description that reads after a signature
(`getWords(text, first [, last]) - the words from first to last`), and put the detail below it.
