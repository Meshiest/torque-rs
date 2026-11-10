# Networked plugins

A plugin that has something to say over the network has four ways to say it: a **ghost class** (an object the server keeps and
every client is shown), a **datablock class** (the numbers a kind of object is made from), a **bulk channel** (a large, mostly
static blob sent in pieces) and an **event** (one message, once). It can also **sign its clients in** (auth), and it says in a
**manifest** which of these it needs, so a client that is not built for the game is turned away before anything is sent.

This chapter takes them one at a time and uses one plugin throughout: `torque-plugin-rope`, whose ropes a server makes and
every client shows. Read [Plugins](plugins.md) first for what a plugin is, [Writing a plugin](writing-a-plugin.md) for the rope
as a client-only plugin, and [Networked objects](networked-objects.md) for how Torque's own records are written; a plugin's are
written the same way.

| You want to send | Use | The rope's |
|---|---|---|
| An object that exists on the server and that each client shows | a ghost class | `Rope`: its anchors, length, look, curve, lifetime |
| The numbers a kind of thing is made from | a datablock class | `RopeData` |
| A large blob, sent once and then in changes | a bulk channel | none: a rope is a few hundred bits |
| Something that happens once and leaves nothing behind | an event | `RopeFx`: a grapple shot, an arc |
| Who is connecting | an auth provider | none |

What a plugin must keep in mind throughout: a TGE client reads none of it, so everything here is for the engine's own clients,
and the manifest is what keeps the two apart.

## Ghost classes and the derive

A ghost class is the console class of the objects it ghosts. The server's object is an ordinary script object with ordinary
console fields; the class says how those fields travel:

```rust
{{#include ../../crates/torque-plugin-rope/src/net/records.rs:ghost}}
```

`#[derive(GhostRecord)]` writes the record's layout (one list of codec calls, run by the encoder, by the decoder of hostile
bytes and by the logging encoder alike), how an update merges into what the client kept, and the **packer**: it reads each field
from the object by its console field name (`data_block` is `dataBlock`; `#[net(name = "...")]` says another) as the connection
sees it. A field takes `#[net(bits = N)]` for an integer's width, `#[net(max_len = N)]` for the longest string or list, and
`#[net(mask = BITS)]` for the groups below.

The field types are the ones a record can be made of (`NetField`): `bool`, `u8`/`u16`/`u32`, `i32`, `f32`, `[f32; N]`, `[u8; N]`,
`String`, `DatablockRef`, `Xf`, `Option<T>` and `Vec<T>`. A type the engine does not know does not compile. A plugin that needs
more writes a `NetField` of its own: the rope's anchor, look, curve, lifetime and length are small types of the plugin
(`AnchorWire`, `LookWire`, ...), each a `layout` (how it travels, with its caps), a `from_view` (how it is read from the object:
an anchor reads `startMode`, `startPoint`, `startObject` and `startNode`) and the script fields it gives a datablock. A type that
reads fields of its own sets `COMPOUND`, so an `Option` of it is present whenever it is packed.

### Mask bits: only what changed

A server marks an object dirty by bits, and a field with `mask = BITS` is an `Option` that is sent only when one of its bits is
set (a new ghost has them all set). The rope has one bit for each group, so a change goes out as a few bits:

| Bit | Group | Marked by |
|---|---|---|
| `MASK_START`, `MASK_END` | what holds an end (a point, a ghost and a place in its own frame) | `ropeNetPin`, `ropeNetAttach`, `ropeNetAttachAt`, `ropeNetRelease` |
| `MASK_LENGTH` | the length to reel to, and the time left | `ropeNetSetLength` |
| `MASK_LOOK` | color, texture, scroll, glow | `ropeNetSetColor` and kin |
| `MASK_CURVE` | a beam's driven curve | `ropeNetSetCurve` |
| `MASK_LIFE` | the time left and the fade | `ropeNetSetLifetime` |
| `MASK_DRIVER` | the name of a driver and the ghost it is asked about | `ropeNetDrive`, `ropeNetUndrive` |
| `MASK_INIT` | what a new ghost is told once: the predict id | never again |

The server's function sets the console field and marks the group (`torque_game::scene::ghost::mark(vm, object, MASK_LOOK)`).
The client keeps the merged record, so the record it is handed is always the whole state; the rope compares it with what it
applied and puts only the groups that differ on its rope. The record's own tests (`net::records`) pack with one bit and check
that every other group is none, and that the bits are a small part of the whole.

### What the packer can ask of the connection

The packer sees the object **as one connection sees it**. Besides the object's console fields it can read, from the view:

- a **link** to another object's ghost (`view.link("endObject")`): the ghost's index on this connection, `Link::NotGhosted`
  while the object has none for it yet, or null. A field that is `pending` (the rope's anchor hung from an object the client has
  no ghost of yet) **keeps its mask bit**, so the next packet sends it again; the client sees a free end meanwhile;
- `@now`, the server's simulation clock in milliseconds, so a lifetime can be sent as the time that is left;
- `@connection`, the connection object the record is packed for, so a record can say one thing to its owner and another to
  everyone else (the predict id below).

### Ids, marks and the console class

A class's id on the wire is the place of its name in the sorted list of the engine's plugin classes, past Torque's own, so both
sides agree from the names alone and Torque's ids and records are unchanged. The registry takes the mark that says how the client
shows the class between updates:

```rust
{{#include ../../crates/torque-plugin-rope/src/net/mod.rs:register}}
```

`Replicate::Predicted` is for a class whose client runs the same simulation ahead of the server and must be bit exact on every
platform (the engine's determinism rules apply, and `torque-trace-hash` checks them); `Interpolated` shows the server's last states.
A rope is **cosmetic**: each client simulates its own points, they differ in the last digits from client to client, nothing
depends on them, and a rope never applies a force to anything. Pulling is for a physics joint that a rope can follow later.

### A share of each packet

A class with many objects that change often (props, debris) can hold its updates to a share of each packet and order them itself:

```rust,ignore
engine.net().ghost_share(
    "Prop",
    |_vm| 0.4,                                    // at most 40% of each packet's bits
    |vm, prop, scope, waited| priority(vm, prop, scope, waited),
);
```

The manager writes the class's first records before its updates, and the updates in the order of the priority, higher first, for
each connection (the function gets the connection's scope object and how many packets the update has waited; the priority is held
below half of a kill's, so a ghost's removal always goes first). The share is of the room the packet has left when its ghost
section starts. An update that would take the class past its share waits for a later packet, and the class's other updates wait
with it (the group is closed for that packet). Other classes are untouched, and only the engine's
peers see plugin classes, so a TGE peer's packets do not change. Both functions run for each update of each packet, so they
should only read. The props plugin uses it (see [Props and joints](props-and-joints.md)).

### Keeping an object out of a connection's ghosts

A plugin that has a better way to send an object than a ghost (a bulk channel, a message of its own) asks the server not to ghost it,
to one connection or to all:

```rust,ignore
engine.net().scope_control();                       // when the plugin is built
// in a server hook:
let scope = ScopeControl::of(vm).expect("asked for above");
scope.hide(vm, object, Audience::Connection(client))?;  // client: the connection's GameConnection object
scope.show(object, Audience::Connection(client));   // and back
```

The server asks the control before it makes a ghost of an object for a connection, so every way an object gets scoped is refused:
the full query, an object's relatives, the feed of new objects, an always-in-scope object, the ghost-always set and a script's
`scopeToClient`. A ghost the connection has when the object is hidden is killed at the next packet (no update need be pending), and
what the scene hierarchy hangs on it is sent again as roots with their own poses; shown again, the object is ghosted by the next
scan, always-in-scope ones included. Only the server's engine has the control, so a client cannot reach it, and a hide only takes
something away from what a client is sent, and never the connection's own control object (or what it rides): a hide cannot
blind a player. A deleted object and a gone connection take their entries with them. A connection is a `GameConnection` (a hide for
any other object is refused, `HideError::NotAConnection`), and each audience (one connection, or everyone) has up to 16,384 entries
of the pool's 262,144, so one plugin's hoard cannot take another connection's room; a hide past a cap is refused (`HideError::Full`,
counted by `ScopeControl::refused`) and changes nothing. A connection's ghosts look again only when what concerns that connection
changed (`ScopeControl::stamp_for`). The control costs a server that never asks for it nothing, and a
Torque-protocol peer's packets are as they were. Each plugin hides and shows under its own name (`hide_as`, `show_as`), so one
plugin's show never undoes another's hide; a plugin that sends an object another way (a bulk channel) asks `is_hidden_by_other`
and reads `changes_since` to follow the hides of the others without a walk. Handing an object over to the plugin's own channel without a gap is the plugin's
to do: send the object by the channel first, hide the ghost when the channel's message was acknowledged
(`ChannelHub::queued_message_len`), and the other way round show the ghost first and remove the channel's copy once the ghost's
first record was acknowledged; props do exactly this for frozen props ([Props and joints](props-and-joints.md#frozen-props-over-a-channel)).

### Shapes without a ghost

A plugin that sends objects another way can still have the client's renderer draw them: `engine.net().stills()` gives each client a
`PluginStills`, a set of shapes (file, one scale, position, rotation) the plugin replaces from its client tick hook
(`vm.extension::<PluginStills>()`). The engine's ghost scene draws each as it draws a resting ghost, as a still copy of its shape's
static batch (kept on the GPU, culled there, in the cached shadows), so thousands of them cost a frame little. A scene that does
not keep resting shapes draws none, and a still that is not drawable (no shape, a scale that is not a number or is past 1024) is left
out; at most 8,192 are kept.

## The client's side of a ghost

The ghost reaches the client as a record in the connection's world; a plugin that shows or simulates it names a handler for the
class, which hears each ghost that appeared, changed (with the whole record, the updates merged) and went, once a tick:

```rust
{{#include ../../crates/torque-plugin-rope/src/net/client.rs:heard}}
```

The rope makes a rope of the client's own world from the ghost and keeps simulating it: the **points never go on the wire**. An
end hung from a ghost follows where the client draws that ghost each frame, and, for a client that draws nothing, where the
connection's `ClientGhosts` says the ghost is. An end whose ghost goes lets go. All of this is client-local: a client's frame does
not wait for the server.

### A ghost drawn as a shape

A ghost class whose objects are things to look at can leave the drawing to the engine. `engine.net().drawn_shape::<G>(class, draw)`
names a function from the record (and the datablocks, to read the one it names) to the shape file, scale and skin the ghost is drawn
as:

```rust
engine
    .net()
    .ghost::<PropGhost>("Prop", Replicate::Interpolated)
    .drawn_shape::<PropGhost>("Prop", |prop, datablocks| {
        let model = datablocks.text(prop.data_block.0?, "model")?;
        Some(ShapeDraw::new(model).scaled([2.0; 3]).skinned("art/prop_red"))
    });
```

The client's renderer then draws the ghost as it draws a StaticShape: lit, shadowed, culled and in the frame's list of ghosts, so a
rope or a pass of your own can hang from it (the rope plugin's default anchors find its origin and meshes as they do a TGE shape's). It stands
where the record's first transform puts it. A class marked `Replicate::Interpolated` is shown between the server's updates: from
where the last frame drew it to where the update put it, over the time the updates are apart (one to three ticks), and held there
until the next. A server that sends nothing for a while leaves it where it is. The function runs every frame the ghost is in view,
so it only reads; a scale that is not a number, or past 1024 on an axis, draws nothing (the server is not trusted). A class's own
ghost is marked dirty by the plugin that moves it (`torque_game::scene::ghost::mark`): a script's `setTransform` marks only the
classes the engine itself knows.

## Datablocks

A datablock class is a record of the numbers a kind of thing is made from, sent once with the mission's datablocks. The client
makes a script object of the console class for it and runs its preload, so a plugin reads it by the typed `RopeData::read`. The
record is a struct with `#[derive(DatablockCodec)]`:

```rust
{{#include ../../crates/torque-plugin-rope/src/net/records.rs:datablock}}
```

The record need not be the console class itself: `RopeData` has a `Filename`, which is not a record field, so the record names the
same console fields with a `String`; a test keeps the two lists in step. A datablock goes out whole and has no masks. A client
has no *names* for the datablocks it is sent (a client script reaches one by the id the server tells it).

## The handshake manifest

A server and a client must agree on more than the protocol: the plugins that change what is on the wire, the ghost and datablock
classes, the channels. Each is listed in the **manifest** the client sends with its connect request, and a server whose engine
is networked judges the request against its own before it sends anything:

```rust
{{#include ../../crates/torque-plugin-rope/src/lib.rs:net_version}}
```

A plugin that sets `net_version` is listed with it; a class registered with `ghost`, `datablock` or `channel` is listed by
name. A mismatch is refused with the reason, which the client's script hears in `onConnectRequestRejected`:

| What differs | The client is told |
|---|---|
| The client has no such plugin | `server needs plugin torque.rope 1, you do not have it` |
| The plugin's version is not the server's | `server needs plugin torque.rope 2, you have 1` |
| The client has a plugin the server does not | `you have plugin torque.rope 1, which this server does not` |
| A class or channel one side does not have | `server has ghost class Rope, you do not` |
| The tables differ in another way | `ghost class tables differ (server 2, you 3)` |
| The protocols have no version in common | `server speaks protocol 12 to 12, you speak ...` |

The test is `tests/net.rs`, which gives a server and a client without the plugin: it ends with the first line, and no ghost was
sent. In Torque's protocol an engine with nothing networked sends no manifest and judges none, so its handshake is Torque's own;
in the engine's protocol ([Network protocols](network-protocols.md)) the manifest is part of every connect and always judged. A plain TGE client
meeting a networked server is refused as one that has none of the plugins ("you do not have it"); a server that wants both kinds
of client leaves its plugins without a `net_version`, and the engine's classes then go only to the clients a game says are the
engine's own (`$Server::EngineClasses`, which a server whose gate admitted every client sets for itself). Over a connection that
did not read plugin classes a TGE client is never sent them: `tests/tge_client.rs` runs a client with Torque's codec alone against
a server with a rope, and it reads its packets and sees no rope.

## Permissions

A server decides who may do what with **permissions**, not with fixed levels. A permission is a dotted name (`props.spawn`,
`rpc.example.chat`): 1 to 8 segments of 1 to 32 bytes from `a-z`, `0-9`, `_` and `-`. A **grant** is a permission, or one with a
trailing wildcard (`props.*` covers every permission under `props`, `*` all of them). A **role** is a named set of grants. The
engine defines no roles: a game does, in Rust, in script or from a file, and roles are flat (no role includes another; a game that
wants "admin implies moderator" grants both). A connection holds roles and direct grants, and a check is true when any grant
matches.

| Cap | Limit |
|---|---|
| Roles on a server, grants in a role | 256, 64 |
| Roles on one player, direct grants on one player | 32, 64 |
| Segments of a name, bytes of a segment or a role name | 8, 32 |
| Declared permissions | 1,024 |

Anything over a cap, an unknown role or a name that does not parse is refused with a reason, and nothing changes.

```rust
let mut perms = engine.permissions();
perms.declare("props.spawn", "Spawn a prop");      // the permissions the plugin checks, so tools can list them
perms.role("builder", &["props.*", "tools.paint"]); // a game's role
// perms.provider(MyDatabase::new());                // where grants are kept (below)
```

A check of a permission nobody declared is a debug assertion (and a warning in a release build). **Script**, on a
`GameConnection`: `%client.addRole("builder")`, `removeRole`, `hasRole`, `getRoles()` (a word list), `grant("props.*")`, `revoke`,
`hasPermission("props.spawn")`; and globally `definePermissionRole("builder", "props.* tools.paint")` and
`declarePermission("props.spawn", "Spawn a prop")`. The calls that change something answer 1 when they did, 0 when there was nothing to
change, and print the reason when they were refused.

**Where it is kept.** A `PermissionProvider` (a slot: one plugin sets it) keeps what each player holds, keyed by the player's
[identity](#who-a-connection-is) (the same one a game's prop ownership uses): the account id the auth provider verified, else the
connection. A role granted to a signed-in player is theirs on every later connection, as long as the server (or the provider you set)
keeps it; one granted to an anonymous player ends with the connection. The default keeps it in memory; a game with accounts backs it
with its database, and its grants then outlive the server. Grants kept under a connection are forgotten when it ends. All of it is on the server: a client never sets its own permissions and nothing is sent to a client unless the game
sends it. The model itself, with no engine behind it, is the `torque-permissions` crate.

## Bulk channels

A ghost is for what changes a little at a time. A **channel** carries large, mostly static data, such as a terrain or a map of
tiles, in chunks that ride in the packets a connection sends anyway, under a byte budget that keeps the events and ghosts
flowing. A message is a snapshot (the whole state) or a delta (what changed since); delivery is reliable and ordered, the
receiver assembles chunks by range and every length is checked against the channel's limits before anything is allocated.

```rust
engine.net().channel::<TerrainChannel>("terrain", 100);   // 100 bytes of each packet at most
```

`ChunkChannel` is the plugin's logic on both sides (`snapshot` on the server, `receive` on the client); the name is listed in the
manifest, and the server turns the channels on for a connection when it accepts the manifest that lists them. The rope has none:
its ghost is a few hundred bits. (`examples/minimal-engine/tests/channels.rs` sends 30 KB while the example's ghosts go on.) The props plugin sends frozen props past a count this way, instead of as ghosts, with [scope control](#keeping-an-object-out-of-a-connections-ghosts) to keep them out of the ghosts.

### From a client to the server

A channel can also go the other way, for what a client has to hand the server (an upload, a large message from a client-side
page):

```rust
engine.net().channel_to_server::<UploadChannel>("uploads", 200);   // 200 bytes of each client packet at most
// on the client: channels.send("uploads", Kind::Delta, bytes)
```

`ChunkChannel::receive_from` is the server's logic: it gets each message with the connection it came from, whole and in the order
that client queued them. The chunks are the same and so is the delivery; they sit at the front of the client's data packets, ahead
of its moves, and never take more than half of one (every fourth packet carries none), so moves and events keep their room. The
name goes in the manifest with an `up:` prefix, and the section exists only on a connection whose manifest listed it.

The server treats every byte as hostile (`UpstreamCaps`): what a connection holds for half-received messages (2 MiB by default,
16 MiB at most) and what all connections hold together (64 MiB), message size, half-received messages, and a rate in bytes and
messages a second. A connection that passes a cap is dropped with the packet error, and nothing from the packet that passed it is
delivered. A client queues no more than the server holds for it (`ClientChannels::set_upstream_backlog` follows a raised cap).

## RPCs

A **plugin RPC** is a typed request a client makes of the server (with a reply, or as a notification) or a notification the
server sends a client. A plugin declares it with a message type, a direction, limits and a handler; the engine does the framing,
the checks and the matching of replies to calls.

```rust
#[derive(Debug, Clone, PartialEq, Default, NetMessage)]
struct SpawnProp {
    #[net(max_len = 64)] data: String,      // a String or a Vec must say its cap
    #[net(finite)] at: Xf,                  // a float must say it is checked
    #[net(bits = 8)] predict_id: u8,
}

#[derive(Debug, Clone, PartialEq, Default, NetMessage)]
struct Spawned { prop: GhostRef }           // a ghost index, resolved per connection

engine.net().rpc::<SpawnProp, Spawned>("props.spawn")
    .to_server()                            // `.to_client()` for a notification the server sends
    .delivery(Delivery::Reliable)           // or Unreliable (a notification only)
    .rate(4, 8)                             // calls a second, burst: a token bucket counted in server ticks
    .max_bytes(256)                         // encoded message, header included; at most 16,382 (a batch less its length)
    .allow(Allow::Anyone)                   // Allow::permission("props.spawn"), or Allow::check(|cx| ..)|cx| ..)
    .server(|cx: &mut RpcCx, req| -> Result<Spawned, RpcError> { .. });
```

A message is a struct with `#[derive(NetMessage)]`: the fields of a ghost record, laid out by the same codec, with caps a peer
cannot get around. A `String` or `Vec` without a `max_len`, a float without `finite` (the decoder then refuses a NaN or an
infinity), a raw object id and a type the wire has no form for do not compile. Name an object with `GhostRef`: the server
resolves it only if that object is ghosted to the caller, so a client can name only what it can see, and a call that names
another is answered `Refused` before the handler runs. A script handler gets the object a ghost stands for (an object id), not the connection's ghost index, and a script that
calls (`rpcToClient`, `rpcToServer`) names objects: they go out as the ghost indexes the peer has, and an object the peer has
no ghost of is refused locally. A client checks the ghosts a server's notification names the same way,
against its own world: `cx.resolve(ghost)` gives the object, and a notification that names a ghost the client does not have is
refused and counted, never handed to the handler as a raw index.

**Directions.** A `to_server` RPC has a reply type; when it is `()` the call is a notification and nothing waits. A `to_client`
RPC is always a notification (a reply-bearing one does not compile): a server never waits on a client that may fail, leave or
never answer. A server that needs an answer sends a notification and the client answers with an RPC of its own. The direction
is enforced on both ends: a client that sends a `to_client` RPC, or a server that sends a `to_server` one, is a protocol error and
the connection is dropped.

**Calling.** The client's side of a connection is a link (`ClientRpc::with_link`), the server's one for each client
(`ServerRpc::with_link`, `ServerRpc::notify`). `link.rpc::<Req, Resp>("props.spawn")` gives a typed handle, checking the layouts
against the table; `link.call(&handle, request, |result| ..)` runs the closure in a later frame with the reply or an error, and
`link.notify(&handle, request)` sends a notification. At most 16 calls of one RPC wait for a reply at once.

| Error | Meaning |
|---|---|
| `Refused` | the server refused: permission, the script's `onRpcCheck`, or a ghost the caller was not sent |
| `RateLimited` | the call went over its rate on the server |
| `TooLarge`, `Busy` | local, before sending: the message is over its `max_bytes`; the send queue (64 KiB of reliable bytes not yet acknowledged) or the RPC's 16 outstanding calls are full: try again |
| `Handler(text)` | the handler's own error, at most 255 bytes: nothing else about a handler's failure crosses |
| `Disconnected`, `Timeout` | the connection ended, or no reply came in `$Pref::Net::RpcTimeout` (10 seconds) |
| `Unsupported` | local: no such RPC, not this side's to call, or other message types |

**Late replies.** A call that times out leaves a short-lived tombstone for its id: a reply that comes after the timeout is ignored
(the callback ran once, with `Timeout`) and the id is not reused meanwhile. A reply to a call that never was is still a protocol
error.

**Limits.** Each RPC has a token bucket for each connection, and each connection one for all of them (64 a second, a burst of
128), both counted in ticks, so they are deterministic. A call over a limit is dropped before it is decoded, counted
(`rpcDrops(%client, "name")`) and reported on the console once a second; a caller waiting for a reply gets `RateLimited`. What a side sends is held to the same buckets, so a client or server
that sends too much hears `RateLimited` (or `Busy`) locally and the peer never has to drop anything an honest caller sent. A
float that is not finite (a script's `1e999`, `inf`, `nan`) is `Invalid` locally and never sent. Frames the carrier cannot take
now (its queue, or the budget the bulk channels share, is full) stay queued in order for the next tick; the reserved channel
is not held up by that shared budget, and a client that never takes its replies is dropped.
`$Pref::Server::RpcKickDrops` (default 0, off) drops a client that has more than that many calls dropped in a second. The
decoder checks `max_bytes` and every field's cap against the bits left before it allocates, and an over-cap message drops
the connection like any malformed packet. On a client, a server's notifications are held to the same caps and rates, and a malformed batch from the server ends the
connection with a reason, as any malformed server packet does. On the server the batches received between two ticks are held per
connection (128 KiB each); a client that goes over its share is dropped with the packet error, and no other client's calls are lost.

**Permissions.** `Allow::Anyone` (the default), `Allow::permission("props.spawn")` and `Allow::check(|cx| ..)` are checked on the
server before the handler runs; `cx.has_permission("..")`, `cx.caller()` and `cx.resolve(ghost)` are what a check and a handler
ask. A permission named in `allow` is validated when the RPC is registered and declared for the plugin if it is not already (see
[Permissions](#permissions)); `.default_permission()` asks for `rpc.<plugin>.<name>` instead (the plugin's id and the RPC's name,
lowercased, other characters made `_`, the plugin's `.` too, so it is one segment; two RPCs that end up with the same one are refused). After that the server script's `onRpcCheck(%client, %name)`, if the game defines it, must
answer true for the call to run: `0`, `false`, an empty answer and a function that fails all deny it (a game that defines none leaves
the decision to `Allow`). A call whose caller has no connection object on the server is refused. A server command takes the same
check: `CommandSpec::requires("chat.mute")`. A client has no permissions of its own: `has_permission` is false there.

**Script.** `rpcToServer("props.spawn", %data, ..)` on a client and `rpcToClient(%client, "props.spawned", ..)` on the server send
a call whose arguments are the message's fields in order (a list field is not filled from a script call); they answer nothing
when the call was queued and the reason when not. A notification's handler can be the game's script: `.server_script()` and
`.client_script()` call `rpcServer_<name>(%client, fields..)` and `rpcClient_<name>(fields..)`, the name's punctuation turned
into `_` (`rpcServer_chat_note`).

**On the wire.** Every RPC is listed in the manifest by name, direction and a digest of its message layouts (manifest format 2,
written only when RPCs exist), so a peer whose RPCs differ is refused with "server and client disagree on RPC props.spawn". In
TGE mode the calls ride a reserved channel in each direction (`torque.rpc`), batched once a tick into one reliable, ordered
channel message that is written first in the packet (within its own budget), so a bulk transfer does not delay a call; this keeps
TGE's packets as they are for every peer that registers no RPC. An unreliable RPC is sent reliably in TGE mode. Handlers run in the
server's tick after the packets were received, and on a client in its tick after the packets of the frame were applied, so a call
that names a ghost from the same packet finds it. The framing and the checks are in `RpcLink`, which hands frames to an
`RpcCarrier`; the channel is the TGE-mode carrier. In the engine's protocol the carrier is the session's own RPC lane
(`RpcLanes`): reliable batches ride it, unreliable ones that fit ride the unreliable lane and are dropped when lost, and the
session's unacknowledged bytes are the backpressure.

**The engine's own RPCs.** In the engine's protocol two RPCs are always registered, `torque.command` and `torque.command.client`: they
carry `commandToServer` and `commandToClient` (see [Network protocols](network-protocols.md)), so those names are taken, and they
are in every manifest of that protocol. They are outside the connection's bucket for all RPCs and have their own, so a plugin's
calls and the game's commands do not starve each other; `onRpcCheck` is not asked about them.

(`examples/minimal-engine/tests/rpc.rs` runs a call with a reply, a notification each way, the script natives, a script
handler, a flood, and the permission checks through the engine's own server and client over the memory transport.)

## Auth

An **auth provider** signs a client in: the client's half makes a ticket for the connection it is making, the server's half
verifies it, after the manifest check and before the game's `onConnectRequest`:

```rust
engine.net().auth(SharedSecret::new(key, "player"));   // for a test or a private game
```

One plugin sets it. A ticket is made for a *connection*, not for a client: the provider is handed an `AuthContext` (the server's
address and the connection's challenge) and **binds the ticket to it**, so a ticket cannot be replayed to another server or on
another connection. A provider that ignores the context lets any server a client joins use its ticket elsewhere. A client with no
ticket (not signed in) stops with the provider's own reason ("not signed in"); a refused ticket is "your sign-in was not accepted",
whatever the provider's reason was.

### Who a connection is

Every connection has an identity, and the server keeps what it holds per player (roles, props, an upload's resume) under it:

| Provider | The identity | It is the same player when |
|---|---|---|
| none (the default) | none: the connection itself, `conn:<n>` | never: each connection is a new player, and a reconnect starts over |
| a real provider (a platform account, a signed token) | the account id the provider verified, `id:<hex>`, and the display name it gave | the same account signs in, on any connection, session or restart |
| `SharedSecret` (tests and private games) | the name the ticket is signed for, as id and as name | the same name |

The server's half of the provider returns it from `verify`, as an `Identity`: an opaque account id (1 to 128 bytes, never empty, so a
provider cannot put everyone under one key) and an optional name to show (control characters dropped, at most 64 bytes). Two
identities are the same player when their ids are equal; the name is only for display, and a player may change it. The id must be
what the provider checked (a signature, a platform's answer), never unchecked text out of the ticket, and the same for the same
account every time. A client cannot choose its identity: it is made on the server from the provider's check, and the connect
arguments, password and ticket text a client sends reach the provider only to be checked.

```rust
let identity = handshake.connection(conn).and_then(|c| c.identity.as_ref());   // None: anonymous
let key = SubjectKey::of(identity, conn.0);                                      // what the server keeps per player
```

**One session per account.** An account that connects while it is already connected is one player on two connections, so by
default the server ends the older connection (reason "You signed in from another place.") before the newer one's `onConnect`, and
the roles, props and upload the account holds follow the live session. A game that wants several sessions per account sets
`$Pref::Server::OneSessionPerAccount = 0`; the connections then share the account's roles, and props orphaned by one are claimed
by neither while two connections hold the key. Anonymous connections are never joined: each is its own player.

In script, `%client.getIdentity()` is the key as text (`id:616461` or `conn:7`; compare it, do not parse it) and
`%client.getIdentityName()` the display name, or empty. Both exist with no provider (every connection is anonymous), and nothing
about them is sent to a client. `getIdentity()` is the raw account id: a game that echoes it to other clients (a scoreboard, chat)
shows them the provider's id, so it should not if the provider's ids are meant to be private. The server's own plugins never call
the method (a game may redefine it); they ask the host from Rust. On the wire nothing changes: a ticket is sent only when a provider is set, as before, in both
protocols.

## Events

An event is one message with nothing left behind: a grapple shot, an arc of lightning. A ghost would be wrong for it (an
object to create, scope, update and delete for something that lasts a second); a plugin sends an event over the **remote
command** path that Torque already has, reliable and ordered with the rest of the connection's events. The rope's server function
builds a `commandToClient(%client, 'RopeFx', ...)`, and the client's `clientCmdRopeFx` is a console function of the plugin:

```
ropeNetFx("", "0 0 1.5", %hit, Beam, 0.8, 0.3);   // every client; start, end, datablock, seconds, bend
```

Everything on the receiving side is checked, because the sender may be hostile: each point must be three finite numbers inside
the world, the datablock a `RopeData` the client has, the time a positive number (held to 30 seconds), the bend held to its range
(a bend that is not a number is straight), and a client shows at most 32 at a time. A call that fails a check does nothing and says
nothing. The one-shot rope is a temporary rope: it fades over its last quarter second (`tests/net_events.rs` throws hostile values at it).

## Predicted spawns

A client that fires a beam should not wait a round trip to see it. It makes its rope at once and tags it for the server:

```
%rope = ropeCreate(%muzzle, %target, $Beam::Block);   // the client's own rope, at once (the datablock's id)
%id = ropePredict(%rope);                            // an id from 1 to 255
commandToServer('FireBeam', %id);
```

The server's `serverCmdFireBeam` makes the rope for everyone and gives the id, with the sender's connection, to `ropeNetCreate`.
The ghost for the owner's connection carries the id and every other connection's carries 0, because the packer compares the
owner with `@connection`. The owner's client finds the id in its waiting list and **adopts** its own rope as the ghost's, so there
is exactly one; the others make theirs from the ghost. A prediction the server does not confirm within two seconds fades out over
a quarter second and is taken away.

The scheme is small and bounded on purpose: an id is one byte; a client has at most 16 waiting, each with its own id (a
seventeenth gets 0 and is not tagged); the ids are the client's alone, since the server hands one out only to the connection that
sent it. A hostile client can send any id it likes, and gets the rope the server would have made for it anyway; it cannot make
another client adopt, drop or fade a rope, because no other client is ever told the id (`tests/net_events.rs` has two clients pick
the same id).

## Choosing a predictor

The client predicts its own control object, as TGE's client does: each tick it runs its new move, and when the server's state
differs from what it predicted (the checksums do not match), it takes the server's state and runs again every move the server has
not acknowledged. That correction lands in one frame, so the player sees the figure, the vehicle and the camera jump.

How the client predicts is a slot, like the renderer's: one predictor runs at a time. It sees the three moments of the
prediction (a server head, a tick, a view), the local moves, the server's acks and states, and the body solver the game picks for
each vehicle datablock, and it decides what is drawn (`torque_client::predict::slot`). The default is TGE's scheme, unchanged:
the same moves, checksums and bytes, and the same trace hashes. An engine that registers no other predictor has no slot at all:
its clients predict as before and gain no console functions.

The engine ships one other predictor, `torque-plugin-smooth-prediction`. Its simulation corrects exactly as TGE's does, so a TGE
server sees nothing new, but what is drawn eases the jump out: the body, its vehicle, the eye and the camera are drawn offset by
the correction, the offset shrinking each tick (by default a quarter of it goes each tick, 5% is left after about a third of a
second) and interpolated between ticks like the body. A jump longer than its snap distance (5 units by default: a teleport, a
respawn) is drawn at once, and a change of what the client controls (a mount, a death's camera) drops the offset. Only the
classes listed are smoothed (`Player` and `WheeledVehicle` by default); the rotation is drawn as the simulation has it.

```rust
use torque_plugin_smooth_prediction::{SmoothPrediction, SmoothPredictionPlugin};

// The game's predictor: every client starts with it.
engine.add_plugins(SmoothPredictionPlugin::new(SmoothPrediction::default().keep(0.7)));
// Or registered only, for a script to choose: clients start with TGE's.
engine.add_plugins(SmoothPredictionPlugin::optional(SmoothPrediction::default()));
```

A plugin with a predictor of its own calls `engine.net().prediction(P)` (the game's: one plugin may, a second fails the build) or
`engine.net().add_prediction(P)` (for a swap only), with `torque_client::predict::NetPrediction` in scope. With either, each client
has `setPredictor(id)` and `getPredictor()` (TGE's is always there as `tge`); a swap takes effect at the next tick.

Which to pick:

| | TGE's (`tge`) | Smoothed (`smooth`) |
|---|---|---|
| What a correction looks like | a jump in one frame | an ease over a few ticks |
| What is drawn right after it | where the server says | where the old prediction was, moving to the new |
| On the wire | TGE's bytes | TGE's bytes |
| Servers it joins | any | any |

Smoothing hides small corrections well: a server that moved the player a little, a missed jump, a bump the client did not see.
It cannot hide a prediction that is wrong all the time (the drawn object then trails the simulation by its share of each
correction); fix that in the simulation. A predictor that needs more on the wire than TGE sends registers plugin ghost classes or
bulk channels, which put it in the handshake manifest, so a peer without it is refused before play and TGE peers keep TGE's bytes.

**Predicting more.** Running nearby vehicles and props ahead of the server and rolling them back to its state, instead of
drawing them between updates, fits the same three moments: a head keeps the server's states of the predicted ghosts, a tick
restores them and runs the ticks since, and a view draws them. Such a predictor reaches the client's simulation through the step
(`PredictState::predictor`, the client's `Predictor` for its world type) and the vehicles' solvers through
`PredictState::body_solver`. It would need per-class snapshots and restores, a work budget per frame for the replays, and a
determinism test per predicted class on native and wasm32; it is not built yet.

**Testing a predictor.** `torque-trace-hash` runs each predictor's scenarios after TGE's (`figure-smooth`: the figure walking
while the server moves it every 75 ticks, four frames drawn a tick and folded in) and `just trace-hash` checks native against
wasm32. Its `forced_corrections` helper moves a standing figure or buggy on the server and gives back every frame drawn after;
`crates/torque-trace-hash/tests/forced_correction.rs` uses it to check that the smoothed body never moves more than a sixteenth
of the jump in a frame while the simulation jumps the whole of it. `just examples/level-demo-correction-shots` draws the same
with both predictors, frame by frame.

## Limits

| What | Limit |
|---|---|
| Networked ropes on a server | 256 |
| Ropes in a client's world, and their segments | 256, 4,096 |
| Node name in an anchor, texture file in a look | 24 bytes, 96 bytes |
| A rope's lifetime or reel | 3,600 seconds |
| One-shot ropes a client shows, their time, and sent to a client | 32, 30 seconds, 32 a second |
| Predicted ropes waiting on a client, and their wait | 16, 2 seconds |
| Plugin ghost classes, datablock classes | 16, 29 (the room Torque's class id widths leave) |
| Entries in the scope control (an object hidden from a connection, or from all): in all, for one audience; stills a client keeps | 262,144, 16,384; 8,192 |
| RPCs an engine registers, a message (the most an RPC may ask for), a call's name | 256, 16,382 bytes (256 unless it asks), 40 bytes |
| Calls of one RPC waiting for a reply, reliable RPC bytes not yet acknowledged | 16, 64 KiB |
| An RPC's rate unless it says another, all of a connection's | 8 a second (burst 16), 64 a second (burst 128) |

## Testing a networked plugin

Run a real server and engine clients in one process over the memory transport and step them by hand, one 32 ms tick of the server
and one of each client, the datagrams carried across around each: nothing reads a clock and nothing opens a socket
(`crates/torque-plugin-rope/tests/net/rig.rs`; a networked test uses `127.0.0.1` or memory and nothing else). Make one engine for
each participant, since a plugin's resources are the engine's. Check what the clients end up with (the rope's anchors, its
length, whether it is taut), what each group costs (pack with one bit and count), and what a hostile peer can do (random bytes
into the record's decode must never panic and never allocate past a cap). A determinism check that runs on native and on
wasm32 (`just trace-hash`) is for a plugin whose simulation runs ahead of the server; the rope's is cosmetic and its crate draws
with the renderer, which does not build for wasm32.
