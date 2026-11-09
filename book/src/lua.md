# Lua

Mods can be written in Lua as well as TorqueScript. The engine runs [Luau](https://luau.org), the Lua that Roblox uses:
Lua 5.1 with optional types, `+=`, `continue` and string interpolation (`` `hp: {hp}` ``). Lua comes as a native
extension, `torque-plugin-lua`, so a game that wants it puts the library in its `plugins/` folder (see
[Native extensions](native-extensions.md)); the engine itself never needs a C++ compiler to build.

A mod's `.lua` files run at the mod's stages, the way its `.cs` files do, and a game's own `.lua` files run with `exec`,
like any script. A `.luau` file is the same language.

## A first Lua mod

```text
mods/greeter/
  mod.toml
  server.lua
  lib/words.lua
```

```toml
# mod.toml
id = "greeter"
version = "1.0.0"
requires_plugins = ["torque.lua"]   # optional: skip the mod when the game has no Lua
```

Without a `[scripts]` table, a stage runs `server.cs` and then `server.lua` (and `client.cs`, `client.lua` on clients)
when the mod has them. With one, name the files: `server = ["start.lua", "extra.cs"]`.

```lua
-- server.lua
local words = require("lib.words")

ts.define("GameConnection::onClientEnterGame", function(client)
    local conn = ts.object(client)
    ts.call("messageClient", conn, "", words.hello .. ", " .. conn:getPlayerName())
end)

print("greeter loaded in the", ts.realm, "realm")
```

```lua
-- lib/words.lua
return { hello = "Welcome" }
```

Switching the mod off (`disableMod("greeter")`, or the mods menu) takes its functions away again, as it does a
TorqueScript mod's: `ts.define` puts them in the mod's package.

## The API

Everything in TorqueScript is text, and so is everything that crosses from Lua to TorqueScript: a number goes as a script
writes it (`3`, `0.5`), `true` and `false` as `1` and `0`, `nil` as the empty text, an object as its id. Results come
back as Lua strings (`tonumber` turns one into a number; Luau also does arithmetic on numeric strings).

| Lua | What it does |
|---|---|
| `ts.call(name, ...)` | Calls the console function `name` (`"echo"`, `"Namespace::name"`) and returns its result. A function that does not exist gives `""`. |
| `ts.object(nameOrId)` | The object with that name or id (or a `/` path), or `nil`. |
| `obj.field`, `obj.field = value` | Reads or writes a field, as `%obj.field` does in a script. |
| `obj:method(...)` | Calls a method of the object (with the colon). |
| `obj.id` | The object's id, a number. |
| `ts.field(obj, name, index)`, `ts.setField(obj, name, value, index)` | The same field access, for a name that is also a method's, or an array field (`name[index]`). |
| `ts.method(obj, name, ...)` | Calls a method by its name. |
| `ts.exists(obj)` | Whether the object still exists. |
| `ts.global(name)`, `ts.setGlobal(name, value)` | Reads and sets a global (`"$Pref::Server::Name"`; the `$` is optional). |
| `ts.define(name, fn)` | Defines a console function (`"luaHello"`) or method (`"Player::onLand"`) that calls `fn`. Its arguments are text; a method's object comes first. What `fn` returns goes back to the script. |
| `print(...)`, `ts.warn(...)` | Print on the console. |
| `ts.realm`, `SERVER`, `CLIENT` | Which realm the script runs in. |
| `require(name)` | Loads a module of the mod's own folder (`"lib.words"` is `lib/words.lua`), once. |

Events in the engine are script functions and methods the engine calls (`onAdd`, `onDamage`, `GameConnection::onConnect`),
so a Lua mod hooks one by defining it with `ts.define`. To keep the original, call it before or after yours from a
TorqueScript wrapper; a Lua definition replaces the function for as long as the mod is on.

An error in a script (a typo, a failed `assert`, a wrong argument) is printed on the console with the file and line, and
the game goes on. An error inside a function `ts.define` made prints the same way and the call returns `""`.

## Realms

Each script engine has its own Lua state, its realm: the server's and every client's. A script runs in one realm and sees
only that realm: its console functions, objects and globals. A client's Lua cannot call a function only the server
has (`ts.call` gives `""` and the console says `Unknown command`), and nothing a server's Lua sets is visible to a
client's.

Inside a realm, each mod has its own global table: a global one mod's script sets (`count = 1`) is not another mod's.
Every mod reads the same built-ins (`string`, `math`, `ts`, ...), which are read-only.

A mod that has both `server.lua` and `client.lua` runs each in its own realm, so the two share nothing but what goes over
the network.

## The sandbox

Lua runs inside the same limits as a script, and less is allowed:

- **No files, processes or environment.** There is no `io`, and `os` keeps only `os.clock`. Lua reads files only through
  `require`, which loads `.lua` files of the requiring mod's own folder and nothing else (no `..`, no absolute paths, no
  other mod's folder).
- **No code from text, no bytecode.** There is no `load`, `loadstring`, `dofile` or `string.dump`, and a file of
  compiled Lua is refused: ship the source.
- **No debug library and no environments.** There is no `debug`, `getfenv`, `setfenv` or `package`.
- **Read-only built-ins.** The libraries, the strings' metatable and the global table cannot be changed
  (`string.upper = ...`, `rawset(_G, ...)` and `setmetatable(string, ...)` are errors), so one mod cannot change what
  another sees. Objects' metatables are hidden.
- **The step limit.** Lua's work counts against the script engine's step limit, as a script's does: Luau stops at every
  call and loop, and each stop is a step. A loop that never ends stops with an error, like a TorqueScript one, and so
  does the script that called it, however the Lua hides the error (`pcall`, coroutines). The builtins that work through
  a whole string, table or buffer in one call (`string.rep`, `string.find`, `table.concat`, `table.insert` in the middle,
  `buffer.fill`, `buffer.copy`, ...) are charged a step for every 64 bytes they read or write (a table slot counts as 8),
  as TorqueScript's natives are: `s:sub(i, i)` of a long string or an append with `table.insert` costs almost nothing, a
  copy of the whole string costs its length. A pattern search is charged the part of the subject it scans, once more for
  each quantifier, and every step of the matcher, going back over the subject included, is a step of the budget too, so
  even a pattern that backtracks a lot stops at the limit. A pattern with more than 6 of `*`, `+`, `-` and `?` is refused
  outright (`%-` and the `-` of a set like `[a-z]` do not count). When the script engine has no
  limit of its own, one call into Lua may take 50,000,000 steps.
- **A memory cap.** Each realm may use at most `$Pref::Lua::MemoryLimit` MiB (64 by default, read when the realm is
  made). The realm's garbage is collected as it nears the cap, and a builtin refused by the cap gets a full collection and
  a second try, so a loop over large temporary strings keeps running; an allocation the live data cannot make room for is
  an error in the script that made it (`not enough memory`), and the game goes on.
- **The call depth.** Lua and TorqueScript can call each other, but the nesting counts against the script engine's call
  depth, so recursion through both stops with an error long before it could crash the game.
- **A few console functions are refused.** The engine will not let Lua run `exec`, `eval`, `call`, `compile`, `export`,
  `fileDelete`, `deleteVariables`, `deleteDataBlocks`, `quit`, `enableMod`, `disableMod`, `loadMods` or `setModPaths`, nor
  the methods that write files or bind text to run later (`save`, `openForWrite`, `writeLine`, `bindCmd`, ...), under any
  namespace (`SimObject::save` too), nor a `schedule` that names one of them. Nor may Lua write a field whose text the
  engine runs as script (`command`, `altCommand`, `closeCommand`, `escapeCommand`, `expression`, `validate`), directly
  or with `setFieldValue` (its `"name[index]"` read as the engine reads a field spec): the check is on the name the
  engine builds from the name and the index, so any field that
  starts with one of them (`command0`, `command[1]`, `COMMAND`) is refused, and so is a field name with a NUL byte. The
  console says why. This is the script engine's call policy
  (`torque_plugin_loader::call_policy`), which a game can replace for a realm of its own (an allowlist).

### What can run later

Every way a Lua call can make code run later, or write a field the engine runs, and what holds it:

| Path | Held by |
|---|---|
| `schedule`, `obj:schedule`, `SimObject::schedule` | the policy, twice: the queued call is checked when it is queued and again, with the policy of that moment, when it fires (skipped with a warning) |
| `call`, `eval`, `exec`, `compile` | refused |
| Action map binds: `bind`, `bindObj` (a key calls a function by its name) | the policy: a bind naming a refused function is refused |
| `bindCmd` (text run on a key press) | refused |
| Fields the engine runs: `command`, `altCommand`, `closeCommand`, `escapeCommand`, `expression`, `validate` | refused, on the name the engine builds, through `obj.field`, `ts.setField`, `setFieldValue` and `setDataField` |
| `ts.define` (TorqueScript and the engine call Lua back, as a callback or in place of a function) | safe by design: the Lua that runs is held to the same policy on every call it makes |
| `commandToServer`, `commandToClient` | safe by design: the other side calls only its own `serverCmd...` or `clientCmd...` script functions |
| Object callbacks (`onAdd`, `onDamage`, ...) | safe by design: the engine calls methods by fixed names, never a name a field holds |
| Globals a game's own script evaluates later | not held: a game that runs a global's text as script should not let Lua set it |

## What servers may send

Nothing a server sends runs as Lua, exactly as for TorqueScript. A server's files are mounted as content: a `.lua` among
them can be read as data by a game's loaders, but `exec` refuses it, and `require` cannot load it, whatever its name.

The one way a server's Lua runs on a player's client is a server/client script, in a realm of its own
(`torque-client-script-host`, not yet in any default plugin set): a WebAssembly guest per connection with only the
abilities the player allowed, none of this page's `ts` bridge. Its time is held to the frame: each callback of a frame
(an event, a timer, `realm.onFrame`) gets 20,000 steps (about 2 ms) and a file run (a mod's entry and what it
`require`s) 100,000 (about 11 ms); an entry past 25 ms, or a callback stopped at its steps, is a strike, and three in
10 s turn the scripts off for that connection. A server with a bigger setup than a file run allows spreads it over
frames, a piece in each `realm.onFrame` or `realm.after` callback.

## Installing Lua in a game

Build the library with `just lua` (it needs a C++ compiler, which only the Lua extension does) and put it in the game's
`plugins/` folder with its manifest, `crates/torque-plugin-lua/torque_lua.toml`:

```text
plugins/
  torque_lua.dll        (libtorque_lua.so on Linux, libtorque_lua.dylib on macOS)
  torque_lua.toml
```

Turn native extensions on (`[plugins] enable = ["torque.native-extensions"]` in `game.toml`) and approve the file the
first time it loads. On Windows `just lua` links the C++ runtime in, so the one file is all the folder needs.

A game can also link the extension into its own build (`torque_lua::INIT` with `ExtensionPlugin::from_init`), which the
extension's tests do. Its crate is a workspace of its own: `cargo test --manifest-path crates/torque-plugin-lua/Cargo.toml`
(`just lua-test`).

## In a browser

A page cannot load native extensions, so in a browser build Lua is a WebAssembly module of its own: the same extension,
built from the same Rust and the same Luau for `wasm32-wasip1` with the WASI SDK's clang, so its sandbox, limits and
messages are the native library's. The page loads it and bridges it to the engine through its JavaScript (`web/lua.js`).
The engine's side is `WebLuaPlugin` (`torque-plugin-loader`'s `web_lua` module), which makes every check the C ABI makes
for a native extension (the names it takes, the call policy, `exec`'s checks for every file, the step budget, the call
depth) on each request the module sends.

To give a web build Lua:

1. Build the module with `just lua-web`. It needs the `wasm32-wasip1` target (`rustup target add wasm32-wasip1`) and a
   [WASI SDK](https://github.com/WebAssembly/wasi-sdk/releases) release unpacked anywhere, its folder named by the
   `WASI_SDK_PATH` environment variable. The module lands in the build folder's `lua-web/torque_lua.wasm` (about
   1.3 MiB, 420 KiB gzipped).
2. Add `WebLuaPlugin::default()` to the game's web build, where the native build has the extension.
3. Build the page with `--lua` (`node web/build.mjs <example> --lua`). The page then loads `lua.js` and the module before
   the game starts. A page built without it never fetches either, and its Lua files print that they need the page's Lua
   module.

**Browser requirements.** Luau's errors are C++ exceptions, and the module is built for WebAssembly's exception handling in
its standard form (`exnref`): Chrome and Edge 137 or later, Firefox 131 or later, Safari 18.4 or later (and Node 24 or
later). On a browser without it the game still plays, but its Lua files cannot run: the page's console says the module did
not load.

**What the module can reach.** Only the few functions its C library imports, which `web/lua.js` answers without reaching
anything: no files (no folder is opened and every file call fails), no environment or arguments, no exit. Its clock is the
page's monotonic one, for `os.clock`; the time of day is refused. Random bytes, for its hash tables' seeds, come from the
browser's `crypto.getRandomValues`.

**If the module breaks.** A trap in the module (a bug, not a script error) stops Lua for the rest of the page's life, and
the console says so; the game goes on.

**Tests.** `just lua-web-check` runs the Lua tests (`crates/torque-plugin-lua/tests/lua.rs` and its shared scripts)
natively, then builds the same cases for a browser (`examples/lua-web-check`) and runs them under Node and in headless
Chrome, each with the page's module. Every case, the sandbox's escapes and the step, memory and policy limits among them,
must pass in all three. CI does not run it: it needs the WASI SDK and builds Luau's C++.

**Building.** mlua 0.12.2 does not build for WASI as released; until a release has upstream's one-line fix (the Luau heap
dump left empty on WebAssembly), `web/lua-build.mjs` applies it to a copy of mlua's sources in the build folder and builds
from a copy of the extension's manifest, never changing the repository's own.

## Limits at a glance

| What | Limit |
|---|---|
| Steps | the script engine's step limit; 50,000,000 a call when it has none |
| Memory | `$Pref::Lua::MemoryLimit` MiB per realm (64) |
| Nesting of Lua and TorqueScript | the script engine's call depth (and at most 64 calls back) |
| Arguments of a call | 32 |
| A `require`d file | 16 MiB |
