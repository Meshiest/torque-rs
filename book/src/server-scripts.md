# Server scripts

A server can send Lua that runs on its players' machines: menus, a HUD, and messages back to the server. The player decides,
server by server and ability by ability, whether that Lua runs at all. The Lua runs in a **realm**, a sandbox of its own (Luau
inside WebAssembly, one for each connection), and it can only do what the player allowed. This chapter is about the consent, the
messages and what a game has to wire up; the sandbox and its caps are in the [Lua](lua.md) chapter and the crates'
documentation.

The first two abilities, `ui` and `net`, have passed the engine's security review, so a build offers them and the global setting
starts at ask: the player is asked before any script of a server runs, and nothing of the realm exists until they allow it. A game
that wants no server scripts sets the setting to never (`Realms::setting`), and a build with the default cargo features of
`torque-plugin-client-script` turned off offers no ability. The other abilities are not offered by any build until each has passed
its own review. The `unreviewed-tiers` feature, for debug builds only, is for tests and for working on an ability before its review;
today it changes nothing, as every ability with a host module has passed.

## What a server asks for

A mod that the server requires of its clients can ask for client scripts in its `mod.toml`:

```toml
[client_script]
entry = "client/main.lua"       # a Lua file of the mod's own content
tiers = ["ui", "net"]           # the abilities it asks for
```

The entry is part of the mod's content, so the hash clients check covers it: a client runs only what the server offered. A
request that is not one (an entry that is also a stage script, a path that leaves the mod, a name that is not an ability) is a
warning and the server sends no client scripts for that mod. A name the client does not know is ignored, so a newer server works
with an older client.

The abilities are `ui`, `net`, `state`, `draw`, `effects`, `input`, `camera`, `local` and `predict`. A build offers an ability
only after it passed its security review and its host module is compiled in. A request for anything else gets nothing, and the
server hears `unavailable`.

## What the player sees

Before anything of the realm exists, the engine decides from three things: the global setting (ask, or never for a
player who wants no server scripts), the player's policy for each ability (ask, always or never), and what the player decided before for this
server.

- Never for the setting, or never remembered for this server: nothing runs and nothing is asked.
- Every ability asked for is decided already: the realm runs with the ones that were allowed, and nothing is asked.
- Otherwise a prompt lists the abilities the server asks for that are not decided, each with a box. An ability whose policy is
  always is granted without a line, one whose policy is never shows grayed and unticked, and one decided for this server before
  shows as decided. The buttons are Allow, Not now and Never for this server, and "Remember my choice" is ticked unless the player clears it.

The "allow without asking" option grants the basic abilities whenever a server asks for them: `ui` (menus and HUD), `net`
(messages to the script's own server and its replies), `state` (read-only views of the players and objects the game already
shows), `draw` (markers and lines in the world) and `effects` (sounds and effects). It does not cover the abilities that act on
the player's input, camera, local objects or movement. A game that shows the option says all of that, not only "menus and
talking to the server": `torque_plugin_client_script::abilities::BASICS_ENGLISH` is the English line and `BASICS_KEY` its key. The engine ships no settings screen: a game's own settings UI shows this line beside the option, and the library does not draw it.

The prompt ignores the pointer for half a second after it appears, needs the press and the release on the same button, and takes
no key, so a click the player was making or a Return a game dialog was waiting for cannot answer it. It is drawn in the engine
layer above every dialog and above the server's own controls. Until the player allows something, no instance of the sandbox
exists and none of the server's files has been read.

What is remembered lives in a file the engine owns, in the player's own cache folder. It is not a preference, a script variable or
a file in the game's folders: no TorqueScript function, console function or Lua table can read or change it. The options menu
lists the servers it remembers and forgets them (`Realms::choices_mut`).

The server learns one thing about the answer: `declined`, `unavailable` or the abilities granted (`clientScriptOnStatus`).

## The Lua a server writes

With `net` granted, a realm has:

```lua
net.send({ action = "drink", item = 3 })      -- a free-form message to the server; false if over a cap
net.on(function(value) ... end)               -- messages from the server

net.call("props.spawn", { data = "crate", at = { 1, 2, 3 } }, function(ok, result) ... end)
net.handle("hud.note", function(value) ... end)
```

A value is `nil`, a boolean, a number, a string, or a table nested at most 8 deep with at most 256 entries. A message is at most
4 KiB up and 15 KiB down; up to 32 a second (a burst of 64) and 64 KiB a second go to the server, and the server's messages to the
realm queue 256 deep, the oldest dropped. A call names the RPC and a table of its fields; a table that names a field the RPC does
not have, a string past 255 bytes, a vector past 16 numbers or a value that is not a number is refused on the client with a Lua
error. An answer is `ok, result`, or `false` and a short text: `refused`, `rate limited`, `too large`, `timeout`,
`disconnected`, or the server's own words.

Only RPCs the server marked for realms can be called or heard:

```rust
engine.net().rpc::<Spawn, Spawned>("props.spawn").to_server().realm()
    .allow(Allow::permission("props.spawn")).server(spawn);
```

An RPC without `.realm()` is refused on the client, and so is every one of this crate's own. Marking an RPC lets the server's
scripts, which are code from the network, call it, so its handler treats the call exactly as it treats any client's.

### Menus and layout

With `ui` granted a realm has the `ui` table: `ui.new(kind, props, parent)` makes a control (`panel`, `text`, `richtext`, `image`,
`button`, `toggle`, `textfield`, `list`, `scroll`, `slider`, `progress`, and the containers `flex` and `grid`), `h:set`, `h:get`,
`h:parent`, `h:destroy` and `h:on` change and read it. Controls are placed by `pos` and `size` and the sizing rules, or by a
container: a `flex` puts its children in a row or a column and a `grid` in cells, by the rules of CSS, if the game's engine has
the layout plugin (`GuiLayoutPlugin`); the [layout chapter](gui-layout.md#in-a-servers-client-scripts) lists the props and the
caps. A container is capped like everything else the server sends: 8 nested, 256 children, 32 tracks an axis, finite numbers in
range only. Without the plugin a container is a plain group and its children keep their own `pos` and `size`.

## The server is the authority

Nothing a realm does changes the server's state except through a message or an RPC that the server decodes, rate-limits, checks
the permissions of and then decides about. A realm sends nothing a modified client could not send, so it adds no way in: a server
has to validate what players send it anyway. The classic example is a potion:

```lua
-- On the client: the realm says what the player wants.
net.call("potion.drink", { slot = 3 }, function(ok, result) ... end)
```

```rust
// On the server: the client says "I drank the potion"; the server checks the inventory.
fn drink(cx: &mut RpcCx, req: Drink) -> Result<Drunk, RpcError> {
    let player = cx.caller().and_then(|c| players.get(c)).ok_or(RpcError::Refused)?;
    if !player.inventory.has_potion(req.slot) { return Err(RpcError::Handler("no potion there".into())); }
    // ... heal the player, remove the potion ...
    Ok(Drunk { health: player.health })
}
```

A server never waits for a realm: a message to a realm is a notification, and the realm's answer is a new call.

On the server, TorqueScript gets a player's message as text (a table as JSON, so no script text is ever built from it) in
`clientScriptOnMessage(%client, %text)`, answers a player with `clientScriptSend(%client, %text)`, and hears the player's decision
in `clientScriptOnStatus(%client, %state, %tiers)`. Rust adds `ClientScriptsPlugin::on_message`. The offer goes to each client
once it has an RPC link (`clientScriptOffer(%client)` sends it again).

## What a game wires up

1. Add `ClientScriptsPlugin` to the server's and the client's engines.
2. On the client, make a `Realms` with the choices (`Choices::open(choices::default_path())`) and a `RunnerFactory` that instances the
   guest (`NativeRealm` under Wasmtime natively, `WebRunner` on the web), set its setting, and call `Realms::frame` once a frame
   with the script engine and the GUI and `Realms::disconnect` when a connection ends.
3. Name the screens that show server controls with `Gui::set_client_script_screens`. To let a server lay its controls out, add
   `GuiLayoutPlugin` too (`GuiPlugin` hands its layout to the GUI); leave it out and containers are plain groups.
4. The prompt is a slot: the engine's own is `EnginePrompt`, and a game draws its own with `ConsentPrompt` (the abilities come with
   stable keys such as `client_script.ability.ui` and English lines, so a game can translate them; the prompt may look as it likes
   but sits in the engine layer, and the realm cannot draw over it).
5. The guest is a data file the engine checks by hash. A release lists the hash of the guest it ships in
   `ACCEPTED_CLIENT_GUESTS` (the justfile's `client-script-guest-hash` prints it); an unlisted guest makes server scripts unavailable.
   The next section says how to build the file the list names.

## Building the guest

`just lua-client-web` builds `torque_lua_client.wasm` into the build folder's `lua-web`. The built file is not committed; its hash
is, in `ACCEPTED_CLIENT_GUESTS` (`crates/torque-client-script-host/src/guests.rs`), and the justfile's `client-script-guest-check` builds the
guest and fails unless its hash is listed. The build is reproducible: the same sources and the same toolchain give the same bytes, whatever the folders of the checkout, the
build and the cargo home are. `web/lua-build.mjs` builds the crates from a copy of them laid out as in the checkout (cargo hashes
the absolute path of a path dependency outside its workspace into the module, which would put the checkout's folder in the hash),
and names the folders that are left by fixed stand-ins in the file's panic and assertion messages, so the player's folder names are
not in it either.

| Input | What the listed hash was built with |
|---|---|
| WASI SDK | 34.0 (`wasi-sdk-34.0-x86_64-windows.tar.gz` from the [WebAssembly/wasi-sdk releases](https://github.com/WebAssembly/wasi-sdk/releases/tag/wasi-sdk-34), clang 23.1.0-rc3, wasi-libc 2e6fb9d8ee0c) |
| Rust | 1.99.1 (58cf8f9c3 2026-10-13), target `wasm32-wasip1` |
| Host | Windows x86_64 |
| Luau | the `luau0-src` the lockfile of `crates/torque-plugin-lua` names |
| Command | `WASI_SDK_PATH=<the SDK's folder> just lua-client-web` |

The release publishes no checksum for its archives; the SDK archive above has the SHA-256
`cccb5c323a9b34f0349a9b09e8804a0a7632c68c3310f4b5f437ed57d7e71d8f`. Another host or another SDK or Rust version is a different guest
with a hash of its own (a panic message names its file with the host's path separator, and the compiler's own version and the
SDK's libraries are in the bytes): list the hash of each guest a release ships, one line each, with the toolchain in a comment
above it. Never change a listed guest's sources without building again and listing the new hash.

## How the realm is tested

The justfile's `client-script-check` is the whole suite, and its output is the evidence:

- **Escape, fuel and memory** (`just lua-client-test`, and the same cases under Wasmtime, Node and Chrome): every way out the
  plan lists is refused or absent (over 160 cases), a runaway loop stops in every entry (a file run, an event, a timer, a message
  handler, a frame callback, a coroutine), memory bombs stop at the cap, and a hand-written guest that loops without imports or
  grows past its maximum is stopped by the deadline or the memory trap while other realms run on.
- **Spoofing and lockout** (`cargo test -p torque-gui client_script`): over random operations mixed with the game's own dialogs and
  screen changes, the realm is drawn only between the content and the game's dialogs, never over a screen the game did not mark,
  never without the chip, and nothing is drawn after the chip; Escape never reaches the realm.
- **Authority** (`tests/authority.rs`): a hostile realm that makes no calls leaves the bytes the client sends the server over 600
  frames, and the server's script state, equal to an idle realm's, for every set of granted abilities; one that makes calls changes
  the server only inside the handlers the game registered, which the test counts.
- **End to end** (torque-test-app's `client_script_end_to_end`): a whole server and client of the engine's own over the memory
  transport: a realm's message arrives in `clientScriptOnMessage` as text that nothing runs, `clientScriptSend` arrives in the
  realm, and the answer is heard in `clientScriptOnStatus`.
- **Fuzzing** (seeded, `TORQUE_CLIENT_SCRIPT_FUZZ` set for the full counts): the message decoder 1,000,000 cases both ways, the
  Lua-to-arguments converter 300,000, the rich-text reader 300,000, the format's props reader 300,000 and the applier 100,000 sequences (with and without a layout, half
  the controls containers), and a seeded run of random container ops against the layout plugin's real classes, all with no panic and every
  accepted result within its caps.
- **Parity** (`parity.txt` in the host crate): each case's stream (log lines, applied `ui` records, messages, calls, revocations and
  the stop) is folded into one number and the number must be the same natively, under Node and in Chrome. A case that reads the
  wall clock has none. A change that moves a number on purpose is written back with `TORQUE_PARITY_BLESS=1`.
- **Budgets** (the justfile's `client-script-bench ui,net`): a hostile realm at its caps keeps the frame within 3 ms of the scene alone at
  p99, and an idle realm with 1,024 controls costs at most 0.3 ms at p50, both counting the canvas drawing the realm's controls.

## An example: the office's prop catalog

`examples/level-demo` has one (`just examples level-demo catalog` plays it headless; `src/catalog.rs` and the mod's folder `catalog/` are the
files). Its server serves the office with a mod, `catalog`, that it requires of its clients; the mod is a menu of the office's
furniture and desk items, each with a picture and a Spawn button, and a Spawn puts a prop down in front of the player.

**The manifest** asks for the two abilities the menu needs:

```toml
share = "download"

[client_script]
entry = "client/main.lua"
tiers = ["ui", "net"]
```

**Download.** The mod is in the server's files (`mods/catalog/`) and not in the clients', so each client downloads it, the Lua and a
picture for each kind of prop, before its realm can ask the player; none of it runs or is read until the player allows it. (A client
that has the mod installed does not use it for scripts.)

**The server's half** (`catalog/server.cs`) keeps the catalog (the kinds the map has a datablock for, with display names), a short
list of recent spawns, and every rule. A message from a menu is `hello`, `spawn|kind|label` or `clear`. A request of any kind (even `hello`) is refused, with the
reason, when the player has asked more than 3 times in 5 seconds; a spawn is refused too when the kind is not one the map has, the label is over 32 characters
or has characters outside a short list, the player has 4 props out already, or `propCheckPlacement` finds no room in front of them.
Otherwise the server's own script spawns the prop with `propSpawn`, owned by the player, keeps the label on it as a field of the
object, remembers it in the recent list and sends the new list to every menu. Nothing about the menu is trusted: a modified client
could send anything, so the rules are the server's, and the menu only asks.

**The menu** (`catalog/client/main.lua`) is a `grid` of cards, each a `button` (a picture from the mod's content and a name; the whole
card is the button, with the button's hover and pressed looks); a `textfield` for the label; a status line that shows only what the server refused; and the recent list under it. A click calls
`net.send`:

```lua
card:on("press", function()
    net.send("spawn|" .. entry.kind .. "|" .. label:get("text"))
end)

net.on(function(message)
    -- catalog|kind|name|..., recent|player|kind|label|..., reply|ok|text, reply|refused|reason, or toggle
end)
```

**Keys.** A server's menu binds no keys. Escape closes it: the engine takes the first Escape press before any action map of the game, drops
the menu's hold on the cursor and tells it `closed`, which the menu answers by hiding itself (Shift+Escape stops server scripts for the
connection, as ever). The key that opens it again is the game's: the level demo binds Q (`game/client.cs`), tells the server with a
command (`Catalog`), and tells the engine the player asked (`Gui::client_script_player_asked`, which the game's Rust calls and no script
can), so the menu may take the cursor again; the server's script answers the command with a `toggle` message to the menu.

**Cursor.** The consent prompt turns the cursor on while it is up, even in a world played with the mouse, and puts it back as the game had
it when the player answers.

**Names.** A player's name in the recent list comes from their connect request and nothing verifies it, so the server cuts it to 16
characters and uses it only if it has letters, numbers, spaces, `-`, `_` and `.`; else the list says "Player".

The example's tests (`level-demo`'s `tests/catalog.rs`) play the menu through the GUI's own input; they need the guest and are run with
`--include-ignored` (`just examples level-demo catalog-test`).

## What servers cannot do

The realm has no files, network, clipboard, devices, TorqueScript, console or other server's data. It never reads a key the player
types elsewhere, never draws over a game dialog or the engine's prompts, and is always identified by a chip that it cannot cover.
Escape drops everything it holds, and Shift+Escape turns server scripts off for the connection. A fake menu or a fake "log in
again" box drawn from the server's own images under the chip is possible: **a game never asks for a password while the chip is
showing.**
