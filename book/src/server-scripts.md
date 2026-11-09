# Server scripts

A server can send Lua that runs on its players' machines: menus, a HUD, and messages back to the server. The player decides,
server by server and ability by ability, whether that Lua runs at all. The Lua runs in a **realm**, a sandbox of its own (Luau
inside WebAssembly, one for each connection), and it can only do what the player allowed. This chapter is about the consent, the
messages and what a game has to wire up; the sandbox and its caps are in the [Lua](lua.md) chapter and the crates'
documentation.

Server scripts are off in every build until the engine's review of the first abilities (`ui` and `net`) is approved. A game that
turns them on today does so with the `ui` and `net` cargo features of `torque-client-script-host` and tests, not with a release.

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

Before anything of the realm exists, the engine decides from three things: the global setting (never, until the first abilities
are approved, then ask), the player's policy for each ability (ask, always or never), and what the player decided before for this
server.

- Never for the setting, or never remembered for this server: nothing runs and nothing is asked.
- Every ability asked for is decided already: the realm runs with the ones that were allowed, and nothing is asked.
- Otherwise a prompt lists the abilities the server asks for that are not decided, each with a box. An ability whose policy is
  always is granted without a line, one whose policy is never shows grayed and unticked, and one decided for this server before
  shows as decided. The buttons are Allow, Not now and Never for this server, and "Remember" is ticked unless the player clears it.

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
3. Name the screens that show server controls with `Gui::set_client_script_screens`.
4. The prompt is a slot: the engine's own is `EnginePrompt`, and a game draws its own with `ConsentPrompt` (the abilities come with
   stable keys such as `client_script.ability.ui` and English lines, so a game can translate them; the prompt may look as it likes
   but sits in the engine layer, and the realm cannot draw over it).
5. The guest is a data file the engine checks by hash. A release lists the hash of the guest it ships in
   `ACCEPTED_CLIENT_GUESTS` (the justfile's `client-script-guest-hash` prints it); an unlisted guest makes server scripts unavailable.

## What servers cannot do

The realm has no files, network, clipboard, devices, TorqueScript, console or other server's data. It never reads a key the player
types elsewhere, never draws over a game dialog or the engine's prompts, and is always identified by a chip that it cannot cover.
Escape drops everything it holds, and Shift+Escape turns server scripts off for the connection. A fake menu or a fake "log in
again" box drawn from the server's own images under the chip is possible: **a game never asks for a password while the chip is
showing.**
