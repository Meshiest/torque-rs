# Server and client

torque-rs keeps Torque's client/server split: the server runs the scripts and the simulation and ghosts objects to
its clients; a client reads its ghosts into a world it draws. The `torque-example-minimal` crate
(`examples/minimal`) puts the engine crates together into a small dedicated server and a client that connect over the
loopback address. It is meant to be read: each piece below is a few dozen lines there.

```
just examples minimal server       # the bundled game on 127.0.0.1:28000; --ticks N stops it after N ticks
just examples minimal client       # in another terminal: connects and prints the ghosts it is sent
```

```
Serving 2 ghosted objects on 127.0.0.1:28000
console: Mission loaded: 2 objects
console: Client 1 entered with camera 16403
client 1 connected from 127.0.0.1:52259
client 1 is ghosting
```

```
Accepted { ghost_bits: 12 }
GhostingStarted
3 ghosts after 12 packets:
  ghost 0 Camera datablock 5 at (0.00, -12.00, 6.00), 0 updates
  ghost 1 StaticShape datablock 3 at (0.00, 0.00, 0.00), 0 updates
  ghost 2 Item datablock 4 at (-3.70, 1.53, 1.50), 9 updates
```

The test `examples/minimal/tests/loopback.rs` runs both in one process for about a second and checks that the
mission's StaticShape and Item and the client's own Camera arrive, and that the Item's moves do.

## The server's parts

**The scripts.** A `torque_ts_vm::vm::Vm` with its files from the game folder through `torque_resource`'s
`ResourceFiles` (the folders the resource manager scans are the host's choice: here `example/`), and TGE's game
classes declared by `torque_game::specs::install`, so `new StaticShape()`, `new Item()` and `datablock ItemData(...)`
work with their typed fields. The server execs `example/main.cs`, then calls the script function
`loadExampleMission`, which execs the mission file. The game folder holds only the example's own scripts.

A game folder read into memory (`GameFiles::load`, `add_dir`) holds plain files only: a link anywhere inside it, to a file or
to a folder, fails the load and the error names the path, so a folder cannot lead the reader outside itself. The folder you
point it at may itself be a link. On Windows every reparse point counts as a link, not only junctions and symbolic links:
a folder kept in OneDrive or another cloud-file service (files that are placeholders until downloaded) or on a deduplicated
volume is refused until its files are local plain files. A file over `files::MAX_FILE_BYTES` fails the load too.

**The socket.** `torque_server::net::transport::UdpTransport` under a `BindPolicy`: a server binds the one address its
host names, and a public address is refused unless the host allows it. `MemoryTransport` is the in-memory one for tests.

**The handshake and the connections.** `torque_net::handshake::Server::new(ServerConfig::new(max_players), key)` answers
the challenge and connect requests and keeps the connection table, each connection with its packet layer
(`Protocol`). A connect request comes back to the host as `Handled::Request`; the host decides (`Decision::Accept`) with
`finish_request`, which is where a game runs its `onConnectRequest`.

**Per connection.** An `EventQueue` of the ghost layer's events, an `EventReceiver` for the client's, and a
`GhostManager` built from `GhostConfig` (the ghost limit and the class-id width of the net class table). Ghosting starts
with `GhostManager::activate`, whose events (`ConnectionMessage`s and one `GhostAlwaysObjectEvent` per ghost-always
object) the host posts; normal ghosting begins when the client answers with `ConnectionMessage` 1.

**The objects.** The host implements `ObjectSource` over its world: which objects exist and are ghostable, their class
ids, the scope query, priorities, and `pack_update`, which packs a record with `TgeGhosts::pack` from an `ObjectView` of
the object and writes it with `TgeGhosts::encode`. The example reads its objects' script fields as the view; it
compares their placement every tick and marks a moved object dirty with the class's mask bit (`set_mask`), and
detaches a deleted one (`detach_object`). A game built on torque-game's scene classes marks its objects as they change
instead, through torque-game's `GhostLink`.

**One tick.** Drain the socket and hand each datagram to the handshake; for a game packet, apply its acks to the event
queue and the ghost manager (`on_notify`), then read the client's head (`ClientBlock`) and its events. Run the
connections' timeouts (`poll_timeouts`), advance the scripts' clock (`Vm::advance_time`, which runs `schedule`d
calls), and for every connection whose send gate is open (`Protocol::may_send`) write a data packet:
`begin_data_packet` (header and rate), the server head (`ServerHead`), the event section
(`write_section_with`, so a ghost-always event packs its object as it is written) and the ghost section
(`GhostManager::write_section`). `torque_server::pacing::PassClock` paces the ticks as TGE's dedicated main loop does.

## Hooks

The example's game logic is in its scripts, which the server calls at fixed points:

| Script function | When |
|---|---|
| `loadExampleMission(%file)` | at start-up, after `main.cs` |
| `onClientEnterGame(%client)` | a client was accepted; returns the camera object the client's scope follows |
| `onClientLeaveGame(%client, %camera)` | the client left, timed out or was dropped |

On the Rust side the seams are the engine's traits: `ObjectSource` and `ObjectView` (what the server's objects are),
`GhostCodecSet` and `DatablockCodecSet` (what their records are, see [Networked objects](networked-objects.md)),
`GhostScripts` (the script call when ghosting begins), and on the client `ClientSet` and `GhostHooks`. Torque's
defaults fill every one; a game replaces only the ones it changes.

## The client's parts

**The handshake and the packets.** `torque_net::client::Client::new(sequence, ConnectFields::standard(password, args))`
writes the challenge request and answers the server's replies (`on_datagram`). Once accepted it has a `Protocol`;
`begin_data_packet` opens a packet when the send gate allows one.

**Reading a server packet.** The head (`ServerHead::read`), the events through `GhostEventReader` (which hands a
ghost-always event's record to the ghost table at once), then the ghost section (`GhostClient::read_section`), whose log
`ClientWorld::apply_ghosts` takes. When the server's `ConnectionMessage` 0 arrives (the ghost-always objects are all
sent), the client posts `ConnectionMessage` 1 with the same sequence.

**Its own packets.** The client's head (`ClientBlock`: the control checksum, the moves, the first-person and
field-of-view pairs) and its event section. They also carry the acks the server's notifies come from.

## A conn game of the game's own

The client's half of a connection is a `ConnGame`: its feed (what it reads from the server into its world and what it
sends back), its own part of the natives' state (`Parts`), the names its local test answers to, the natives it adds, and
the answers it gives the connection at fixed points. Torque's is `TgeConn`, and every client type in `torque::app` takes it
as the default, so `ClientBuilder::from_engine(..)` and `GameClient` mean a client of `TgeConn`.

A game with its own conn game makes its client from the same plugins with
`ClientBuilder::<MyConn>::from_engine_with_conn(&engine, files, net)`. The plugins' client recipes run at the same stages
in the same order; they call the engine's installs through `ClientParts::installs`, which the builder makes for `MyConn`, so
no recipe names a conn game. A recipe of the game's own reads the typed state with `ClientWiring::<MyConn>::of(vm, "my
plugin")` (plain `ClientWiring` is TGE's; on a client of another conn game the error names both), and the socket kind and
the GUI from `ClientParts`, which every client has. The per-tick pass is the engine's slot of `dyn ClientPass<MyConn>` when
the game sets one, else the engine's own pass. A plugin sets it in its `build`:

```rust,ignore
let pass: Rc<dyn ClientPass<MyConn>> = Rc::new(MyPass);
engine.set_slot::<dyn ClientPass<MyConn>>(pass);
```

The slot is exclusive: a second plugin that sets it fails the build, naming both. The game's app
implements `GameApp` with `type Conn = MyConn` and is named to the engine with `EngineRun::app_for`, so the window, the
page and the offscreen runner drive it as its own type.

A conn game whose feed is Torque's (`type Feed = TgeFeed`) starts it with `torque_client::conn::tge::start_feed`, which
does what `TgeConn` does: the engine's protocol when the engine speaks it, the engine's channels, plugin classes and
per-ghost blocks. Such a client keeps `GameClient::with_world`, `ghost_count`, `sky_distances` and `predict_frame`. A feed
of the game's own does those things itself, and fills the view from its own world in its frame.

## What the example leaves out

A game adds these on the same pieces: the mission phases and the datablocks a client is sent before ghosting starts
(here ghosting starts as soon as a client is accepted, and records carry datablock ids only); a control object (the
server head sends none, so the client's camera is only its scope object); moves and their prediction; remote commands
and net strings; the server query (the LAN list's ping and info packets, `torque_net::serverquery`).

`torque-server` does not yet have a `Server` type that owns the scripts, the simulation and the network together; the
example's `DedicatedServer` shows how the parts fit until it does.
