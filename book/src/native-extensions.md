# Native extensions

A native extension is a plugin built on its own, as a library (`.dll` on Windows, `.so` on Linux, `.dylib` on macOS),
that the engine loads while it starts. It adds console functions, methods, globals and render passes, as a compiled
plugin does, without the game being rebuilt. Node's native addons and Godot's GDExtension work the same way.

Extensions talk to the engine through a small, versioned C interface (the ABI). Write one in Rust with the
`torque-plugin-sdk` crate, or in C, C++, Zig or anything else that can export C functions, with the header
`torque_plugin.h`.

## Read this first: an extension can do anything

A native extension is an ordinary compiled program running inside the game. It has every right the game has. It can read,
change and delete your files, send anything over the network, watch what you type, and install other programs. There is
no sandbox. A bad extension can do all the harm any program you download and run can do.

So the engine is careful about which extensions it loads:

- **Off by default.** Nothing loads unless the game turns extensions on.
- **Only from your own folder.** The engine loads extensions from a local `plugins/` folder that you put the files in.
  It never loads one that a server sends. A server can say it needs an extension (by its id and version), but it can
  never send one.
- **You approve each file.** The first time an extension loads, the engine shows its id, its version and the SHA-256
  hash of its file, and asks you. It remembers the hash you approved, in a folder of your own (not in `plugins/`, so a
  download cannot approve itself). If the file changes in any way, it asks again.
- **A game's own extensions are part of the game.** A packaged game (made with `torque-bundle`) has the SHA-256 of each
  extension it ships compiled into its executable, the way the executable itself is trusted, and gives them to
  `NativeExtensionsPlugin::shipped`. A library whose bytes have one of those hashes loads without a question. Any other
  library (one dropped into `plugins/` later, or a shipped one that changed) asks like before. Nothing in the game's folder
  can approve a library: a file there is as writable as the library itself.
- **Changed files are reported, not trusted.** A packaged game also ships a `manifest.toml` listing every file with its
  SHA-256. A game that sets `NativeExtensionsPlugin::bundle(folder)` reads it at start and warns about each listed file that
  changed or is missing. This is advisory: someone who can write the game's folder can edit the manifest to hide a change, so
  a clean report proves nothing, and the manifest approves no library. It reads every listed file once at start (at most
  the sizes the manifest lists, within 16 GiB), so a game with a very large folder may leave it out.
- **It loads exactly what you approved.** The engine reads the file once, hashes those bytes, and loads a private,
  read-only copy of them. Changing or swapping the file in `plugins/` afterwards changes nothing that runs.
- **The wrong version never runs.** The engine reads the ABI version from the file's export table without loading it,
  and refuses another version before any of the extension's code runs.
- **A crash in Rust is contained.** A panic inside an extension is caught where it happens. The engine disables the
  extension and goes on. (A C extension that writes over memory can still crash the game: nothing can stop that.)

Load an extension only if you trust where it came from, as you would any program. If you want mods that cannot harm the
player, use scripts and assets: TorqueScript mods run inside the script engine.

## Turning extensions on

A game adds the loader's plugin group, which adds `torque.native-extensions` turned off:

```rust
use torque::engine::EngineBuilder;
use torque::extensions::NativeExtensions;

let mut engine = EngineBuilder::new();
engine.add_plugins((DefaultPlugins, MyGame, NativeExtensions::default()));
```

(`torque::extensions` is the `torque` crate's `native-extensions` feature, or the `torque-plugin-loader` crate on its
own. Its `render` feature, on with the `torque` crate's `app` features, draws the extensions' passes.)

Then either the game's `game.toml` turns it on:

```toml
[plugins]
enable = ["torque.native-extensions"]
```

or a command-line flag does, if the game passes its arguments on:

```rust
torque::extensions::enable_by_flag(&mut engine, std::env::args()); // --native-extensions
```

`NativeExtensionsPlugin::new(folder)` loads from another folder than `plugins/`.

## The plugins folder

Each extension is two files in `plugins/`: its library and a manifest named after it.

```text
plugins/
  native_sample.dll        (libnative_sample.so on Linux, libnative_sample.dylib on macOS)
  native_sample.toml
```

What the engine writes itself goes in a per-user folder, `torque-rs/native-extensions` in the platform's per-user cache
folder (`%LOCALAPPDATA%` on Windows, `~/Library/Caches` on macOS, `~/.cache` elsewhere), the base the engine's other
per-user files use:

```text
torque-rs/native-extensions/
  approved.txt             (the hashes you approved)
  cache/<sha256>.dll       (the private copies that are loaded)
```

An `approved.txt` found in `plugins/` is ignored, with a warning. `NativeExtensionsPlugin::user_folder(path)` puts the
per-user folder elsewhere; one inside the plugins folder loads nothing.

The manifest says what the library is, so the engine can show you before any of its code runs:

```toml
id = "example.native-sample"   # the id the library reports: its plugin id
version = "0.1.0"              # the version it reports
library = "native_sample"      # the library's name; the manifest's own name when left out
```

The engine then:

1. reads the manifest, then reads the library once and hashes those bytes (SHA-256);
2. asks whether to load it, unless `approved.txt` already has that id, version and hash;
3. reads the ABI version the bytes export, refusing another version;
4. writes the bytes to `cache/<sha256>.<ext>`, makes the copy read-only, checks its hash again, and loads the copy by its
   full path;
5. calls its entry point, which registers what it adds;
6. checks that the id and version it reports are the manifest's, and adds it as a plugin of that id.

Copies no longer in use are removed from the cache each time.

**What an approval covers.** Exactly one file: the library with that hash, under that id and version. Nothing the
library reads or loads later is checked. So an extension must be linked statically: the private copy finds its own
dependencies only among the system's libraries (Windows' System32, the system library paths elsewhere), never in
`plugins/`, the game's folder, the working folder or `PATH`. A library that needs another DLL or `.so` of its own does
not load. (Rust's `cdylib` and a C library that uses only the C runtime are fine.)

A game that set `.shipped(hashes)` treats a library whose hash is in that list as approved in step 2, and writes nothing to
`approved.txt` for it. The hash that counts is the one of the bytes that are then loaded, and the list must be compiled into
the executable, never read from a file in the game's folder. A game that set `.bundle(folder)` also reads
`<folder>/manifest.toml` first, only to warn about listed files that changed or are missing. The manifest is untrusted input:
it is read within a cap, a path that is absolute, holds `..` or a drive, or is listed twice is refused and never read, and
files are read without following a link.

Every step that fails is a warning on the engine (`engine.warnings()`), and the `LoadReport` resource says what happened
to each manifest. The question goes to the terminal when there is one. With none (a launcher, a service, a test) it is
answered "no". A host can put in its own: `NativeExtensionsPlugin::new(folder).approver(|request| ...)` gets the id, the
version, the file and its hash, and whether the file changed since an earlier approval
(`ApprovalRequest::question()` is the text to show). `.deny_unknown()` loads only what `approved.txt` already has, and
never asks.

`approved.txt` holds one line for each approved file:

```text
# Native extensions approved to load: <sha256> <id> <version>. Each runs with the game's full rights.
3f2a...c9 example.native-sample 0.1.0
```

## Writing an extension in Rust

An extension is a `cdylib` crate that depends on `torque-plugin-sdk` and nothing else of the engine. The repository's
sample is `examples/native-extension`, which has its own Cargo workspace so it builds apart from the engine:

```toml
[package]
name = "native-sample"
version = "0.1.0"
edition = "2024"

[lib]
crate-type = ["cdylib"]

[dependencies]
torque-plugin-sdk = { path = "../../crates/torque-plugin-sdk" }
```

The code looks like a compiled plugin. `#[function]` is the engine's own attribute: in a crate that depends on the SDK
it declares the function through the SDK, with the same name, argument counts and usage text a compiled plugin would
get. `export!` writes the library's two exports.

```rust
use torque_plugin_sdk::{Call, Extension, PassFrame, Registrar, Space, Stage, Triangles, function, natives};

/// greets someone by name
#[function]
fn sample_greet(name: &str) -> String {
    format!("Hello, {name}!")
}

/// adds two or three whole numbers
#[function(ns = "Sample")]
fn add(a: i32, b: i32, c: Option<i32>) -> i32 {
    a.wrapping_add(b).wrapping_add(c.unwrap_or(0))
}

/// the greeting global, said twice
#[function(ns = "Sample", name = "greetingTwice")]
fn greeting_twice(call: Call) -> String {
    let greeting = String::from_utf8_lossy(&call.global("$Sample::Greeting")).into_owned();
    format!("{greeting} {greeting}")
}

/// A badge in the picture's top left corner, over everything.
fn badge(_frame: &PassFrame, out: &mut Triangles) {
    let corners = [[-0.95, 0.6, 0.0], [-0.6, 0.6, 0.0], [-0.6, 0.95, 0.0], [-0.95, 0.95, 0.0]];
    out.quad(Space::Clip, corners, [0.1, 0.6, 1.0, 0.8]);
}

#[derive(Default)]
struct Sample;

impl Extension for Sample {
    const ID: &'static str = "example.native-sample";
    const VERSION: &'static str = "0.1.0";

    fn build(&self, ext: &mut Registrar) {
        ext.script()
            .natives(natives![sample_greet, add, greeting_twice])
            .global("$Sample::Greeting", "Hello");
        ext.render().pass(Stage::Overlay, || badge);
    }
}

torque_plugin_sdk::export!(Sample);
```

Build it with `cargo build --release` (or `just examples native-extension build` for the repository's sample), and copy the library
and its manifest into the game's `plugins/` folder.

What an extension can declare:

- **Console functions and methods**: `ext.script().natives(natives![...])` for `#[function]`s, or
  `.function(namespace, name, args, description, f)` and `.method(class, name, args, description, f)` with a closure.
  `ext.server_script()` and `ext.client_script()` declare for one side only. Parameters are script arguments (`i32`,
  `u32`, `f32`, `bool`, `&str`, `String`, `&[u8]`, `Option<T>`, `Rest`), read by the engine's own rules, and the
  injected `Call` (the console, globals) and `This` (a method's object). A function returns nothing, a number, a truth
  value, text, or a `Result` whose error is printed on the console with the function's usage.
- **Globals**: `.global("$Sample::Greeting", "Hello")`, set before scripts run.
- **Render passes**: `ext.render().pass(stage, make)`. Each view of the world makes a pass with `make` and asks it for
  triangles each frame: in the world (`Space::World`, seen through the frame's camera) or on the picture
  (`Space::Clip`). At `Stage::Opaque` and `Stage::Translucent` they draw into the scene, hidden behind what is nearer;
  at `Stage::Overlay` they draw over the finished picture. The engine draws them flat colored and alpha blended.

A panic anywhere in the extension's code is caught at the boundary and disables the extension: its functions then print
an error and return the empty string, and its passes stop drawing. Build extensions with the default
`panic = "unwind"`. With `panic = "abort"` a panic ends the game.

An extension is never unloaded while the game runs: its library stays loaded until the process ends.

**Threads.** The engine may call an extension's functions and passes from any of its threads, but never two at once: it
makes one call into an extension at a time, across all its threads, and refuses a call that would re-enter the extension
on the thread already inside it. So an extension's own state needs no lock against the engine (the SDK still asks for
`Send + Sync` natives and `Send` passes, since the thread may change between calls), but it may not count on one thread,
or on thread-local storage, staying the same from call to call.

The one re-entry the engine allows is a call back: during one of its calls an extension may run the script engine's
functions and methods (`Call::call_function`, `Call::call_method`), and a script run that way may call the extension again
on the same thread. The extension must hold no lock of its own across such a call. The nesting counts against the script
engine's call depth (the stack the extension used is charged as script levels), and at most 64 calls back nest.

### Script languages

An extension can run another language's files: `Registrar::script_language(side, "lua", language)` declares a
`ScriptLanguage`, and from then on `exec` hands every file with that extension to it, after the checks a script's file
gets (so a file a server sent is never handed over). During its calls a language can:

- run console functions and methods (`call_function`, `call_method`, `has_method`), find objects (`find_object`) and read
  and write their fields (`field`, `set_field`), and globals (`global`, `set_global`);
- define console functions and methods that call it back (`define_function`, with an id the engine hands back to
  `ScriptLanguage::call_defined`), in the package of the file it is running (`script_package`: a mod's), so that switching
  the mod off hides them;
- (the functions and methods a language's code runs are checked against the script engine's call policy, which by
  default refuses those that run code from text or write files, under any namespace);
- charge its work to the step budget (`charge`): `Err(Limit)` once it is spent, which the language hands back so that the
  script that started it stops too;
- read its own files (`read_script`: a relative path, no `..`, its extension) through the checks `exec` makes;
- learn which script engine it is in (`vm_info`: a number and a side) and hear when one goes (`ScriptLanguage::vm_dropped`).

The [Lua](lua.md) extension is built on this.

### The same plugin compiled in

An extension's functions have the same names, argument counts and usage texts as the same functions in a compiled
plugin, so a game can move a plugin between the two forms without its scripts noticing. The loader's tests check this
on the sample: its natives listing is the compiled twin's line for line, and its picture is the compiled twin's (`just examples
native-extension shots` draws both and compares them with `shots-diff`). A compiled plugin draws the same triangles with
`torque::extensions::render::TrianglesPass`.

## Writing an extension in C

`crates/torque-plugin-abi/include/torque_plugin.h` declares the whole interface. An extension defines its two exports:

```c
#include "torque_plugin.h"

static const TorqueApi *api;

static TorqueStr text(const char *s) {
    TorqueStr t;
    size_t n = 0;
    while (s[n] != 0) {
        n++;
    }
    t.ptr = (const uint8_t *)s;
    t.len = n;
    return t;
}

/* C::twice(text): the text twice. */
static TorqueStatus twice(void *user, TorqueCall call) {
    static uint8_t out[1024];
    TorqueStr arg;
    TorqueStr result;
    size_t i;
    (void)user;
    if (api->arg_str(call, 0, &arg) != TORQUE_OK || arg.len > sizeof out / 2) {
        return api->return_error(call, text("the text is too long"));
    }
    for (i = 0; i < arg.len; i++) {
        out[i] = arg.ptr[i];
        out[arg.len + i] = arg.ptr[i];
    }
    result.ptr = out;
    result.len = 2 * arg.len;
    return api->return_str(call, result);
}

TORQUE_EXPORT const uint32_t torque_plugin_abi_version = TORQUE_ABI_VERSION;

TORQUE_EXPORT TorqueStatus torque_plugin_init(const TorqueApi *engine, TorquePluginInfo *info) {
    TorqueArg args[1];
    TorqueNativeDecl decl = {0};
    /* The first two fields are the same in every version: check them first. */
    if (engine->abi_version != TORQUE_ABI_VERSION || engine->size < sizeof(TorqueApi)) {
        return TORQUE_ERR_ABI;
    }
    api = engine;
    info->abi_version = TORQUE_ABI_VERSION;
    info->id = text("example.c-sample");
    info->version = text("0.1.0");

    args[0].name = text("text");
    args[0].kind = TORQUE_ARG_REQUIRED;
    decl.side = TORQUE_SIDE_BOTH;
    decl.ns = text("C");
    decl.name = text("twice");
    decl.args = args;
    decl.arg_count = 1;
    decl.description = text("the text twice");
    decl.call = twice;
    return api->register_function(api->init, &decl);
}
```

Build it as a shared library: `cc -shared -fPIC -I crates/torque-plugin-abi/include -o libc_sample.so sample.c` (on
Windows, `clang -shared ... -o c_sample.dll`). The full example is `examples/native-extension/c/sample.c`; the loader's
tests build and load it when a C compiler is found.

The rules of the interface:

- Text is a pointer and a length (`TorqueStr`), never NUL-terminated. Names and ids are UTF-8; script values are bytes.
- The engine's objects are handles (`TorqueInit`, `TorqueCall`, `TorqueDraw`): numbers that are valid only during the
  call that hands them over. A handle used later, or on another thread, is refused with `TORQUE_ERR_HANDLE`.
- What the engine hands over (an argument's text, a global's value) is valid until the call returns; the engine copies
  everything the extension hands it before the function returns.
- Every function returns a `TorqueStatus`, and checks what it is given: a null pointer, a length over the limits
  (`TORQUE_MAX_*`), text that is not UTF-8 or a name with the wrong characters is an error status, not a crash.
- The table (`TorqueApi`) stays valid while the extension is loaded. Keep the pointer.
- Calls come from any of the engine's threads, one at a time (see Threads above).
- A text result the table hands back (`get_global`, `call_function`, `get_field`, `read_script`) is valid until the next
  function of the table called with the same call handle, or the call's end: copy it.
- `TORQUE_ERR_LIMIT` means a limit stopped the run (the step budget, the call depth). Return it from your own function, so
  the script that called you stops too.

## ABI versioning

`TORQUE_ABI_VERSION` is a single number. Any change to the interface raises it: a type's fields, a function's
signature, a constant's meaning, or a function added to the table. The engine loads only extensions built for its own
version, so there is no partial compatibility to get wrong. An extension built for another version is refused with a
message naming both versions, and none of its code runs (the engine reads the exported `torque_plugin_abi_version`
before it calls anything). To update, rebuild the extension against the new SDK or header.

`TorqueApi` starts with `abi_version` and `size` in every version. An extension checks those two fields before it reads
anything else; the SDK does it for you.

The header is generated from the Rust types (`crates/torque-plugin-abi`) by cbindgen: `just c-header` writes it, and
`just c-header-check` (run by CI) fails when the checked-in header is out of date.

## Servers and extensions

An extension with a network version (`Extension::NET_VERSION`, or `has_net_version` in C) is a networked plugin like
any other: it is in the handshake manifest, and a server refuses a client without it at the same version. A server never
sends a library. The player has to install the extension and approve it, as any other.
