# The mods demo

A game mode written in Lua and shipped as a mod that the server requires, played headless: a server and two clients over the
memory transport. It puts the guide's [Mods](../../book/src/mods.md), [Lua](../../book/src/lua.md), [Native
extensions](../../book/src/native-extensions.md) and [Networked plugins](../../book/src/networked-plugins.md) chapters to work
together.

**The game.** A round of *orbs*: each player who joins gets a runner that chases the nearest orb, a clock counts the round down,
and a score is kept. A rope (the rope plugin's networked rope) runs from each runner to the orb it chases, and the leader hangs
from a gold cable. The rules are `game/mods/orbs/server.lua` (with `lib/rules.lua`); the datablocks it needs, which Lua cannot
declare, are `server.cs` beside it. The game's own scripts (`game/server.cs`, `game/client.cs`) know nothing of orbs: the server
script has empty `onEnterGame` and `onLeaveGame` hooks that the mod replaces, and the client script shows a board of whatever
state the server sends.

**Three mods, three rules** (`game/mods`):

| Mod | `mod.toml` | What it shows |
|---|---|---|
| `orbs` | `share = "download"`, `requires_plugins = ["torque.lua"]` | The game mode. Its content (two runners, an orb, a rope picture and a sound, made from code by `src/assets.rs`) is offered to every player: a client that lacks it downloads, verifies, mounts and caches it before the server runs `onConnect`. The Lua runs on the server only. |
| `golden-orbs` | `requires = ["orbs >= 1.0"]`, `share = "none"` | Load order: its name sorts before `orbs`, but it requires it, so it loads after it, and its Lua definitions of `orbsValue` and `orbsBeamColor` sit over the first mod's (every third orb is gold). Players need nothing from it. |
| `confetti` | content and a script | Disabled by `game/prefs.cs` (`$Pref::Mods::Disabled`): found, never mounted, its script never runs and its content is never offered. |

**What a client does.** It has none of the mods (`client_files()` leaves `mods/` out). On connect the server's offer lists `orbs`;
the client fetches its content over the `torque.mods` channel, checks every block and file against the hashes, mounts it as
untrusted content and tells the server it is ready. The orb and runner shapes the server ghosts to it are read from that mount, and
the rope's beam picture too. A second join from the same cache fetches nothing. The server's Lua cannot run on the client: the
mod's scripts are never offered, and a `.lua` file that did arrive would be data (`exec` and `require` refuse it).

**Run it** (the Lua extension is built apart, so it needs a C++ compiler the first time):

| | Recipe | Command |
|---|---|---|
| Play two rounds | `just examples/mods-demo` | `just lua`, then `cargo run -p torque-example-mods-demo --release --bin mods-demo` |
| Pictures of a round, from the first client | `just examples/mods-demo-shots` | `... --bin mods-demo -- --shots <dir>` (offscreen, no window) |
| The tests | `just examples/mods-demo-test` | `cargo test -p torque-example-mods-demo --test demo -- --include-ignored` |
| Package it for this computer (a folder that runs on its own, with `plugins/`, licenses and a manifest) | `just bundle examples/mods-demo` | `cargo run -p torque-bundle --release -- examples/mods-demo` (`bundle.toml` says what ships; the bundle's check plays a round from its folder) |

The demo needs the extension's library: `just lua` leaves it in the build folder's `release/`, `--lua <library>` or
`TORQUE_LUA_LIBRARY` names another. The tests build it themselves. They are `#[ignore]`d so that `cargo test` needs no C++
compiler; `tests/natives.rs` (the script engines' natives, snapshotted in `tests/natives/`) runs with the rest.

**Files.** `src/lib.rs` (the files and the engines: the mods plugin as a server that requires its mods and as a client with a
cache, the rope plugin, the Lua extension loaded from a `plugins/` folder), `src/session.rs` (a server and its clients, stepped by
hand), `src/viewer.rs` (a client drawn offscreen, reading its files through its own script engine), `src/assets.rs` (the mods'
content), `game/` (scripts and mods), `tests/demo.rs`.
