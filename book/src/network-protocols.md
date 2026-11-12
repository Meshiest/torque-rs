# Network protocols

A game speaks one of two network protocols, and every server and client of it speaks the same one:

- **The engine's own** (`engine`, the default): its own framing over a link contract. It owes TGE nothing on the wire, and every
  peer is the engine's own. Natively its UDP is the QUIC link, encrypted (below).
- **Torque's own** (`tge`): TGE's handshake, packet header and event layer, byte for byte, so a server and a client of the
  engine can meet a real TGE peer. It is an optional compatibility mode that a game turns on; nothing in the engine's own
  protocol is chosen to fit it.

Above the framing both protocols carry the same things: the ghosts and their records, the net strings, the datablocks, the
moves, the bulk channels, the events and the prediction. A game's scripts see the same callbacks in both (`onConnectRequest`,
`onConnect`, `onConnectionAccepted`, `onConnectRequestRejected`, `onConnectionDropped`, `onConnectionTimedOut`, `onDrop`) with
the same arguments.

## Choosing one

The protocol is an exclusive slot of the engine (`engine.net().protocol(..)`), set by one of two plugins:
`EngineProtocolPlugin` (`torque.net.engine`) or `TgeProtocolPlugin` (`torque.net.tge`). `DefaultPlugins` and `MinimalPlugins`
carry both through `ProtocolPlugins`, the engine's on and Torque's off. A game that must speak Torque's protocol says so:

```toml
[net]
protocol = "tge"   # or "engine", the default
```

In `game.toml`, `[net] protocol` turns the other plugin off, as `[runner]` does for runners. In code,
`torque_app::plugins::set_protocol(&mut builder, ProtocolKind::Tge)` does the same. An engine built without the groups and
without either plugin speaks Torque's protocol: the slot's own fallback, so a host that wires its own plugins keeps what it
had. The variable `TORQUE_NET_PROTOCOL` (`tge` or `engine`) picks which plugin the groups turn on for a whole run, which is how
the `net-matrix` recipe runs the same tests in both; a game's `[net] protocol` and a test that sets the protocol itself keep
theirs. A browser build reads the variable when it is compiled.

**UDP is the QUIC link.** Natively, with the `quic` feature (on by default in `torque-app` and in the umbrella crate's
`default` and `app`), a server in the engine's protocol opens `Listen::Udp` as the QUIC link on the same address and port, and
`--transport both` puts the QUIC link beside the web instead of plain UDP; a client's `ClientNet::Loopback` and `ClientNet::Lan`
sockets send the engine's datagrams over a QUIC connection to the server they connect to, dialed on the first one, and keep a
plain socket for the LAN list's requests only (an engine datagram read on it is dropped). The LAN list still finds the server:
its QUIC endpoint takes the list's pings and info requests (which are not QUIC) from private addresses on its port and answers
them there. A server's identity key is `identity.key` in its data folder, or, on the loopback address, a key made for the
server's life; a client checks a LAN or internet server's key against `known_servers` (trusted on first use), and a loopback
server's against the key it met first, pinned for the socket's own redials (a changed key is refused) until the scripts
connect to that server again (joining or hosting anew, when a new key is expected). A connection that ended is dialed again
after a wait that doubles from a quarter second to four seconds, so a server that refuses or is gone costs
one handshake per wait, not one per tick. There is no fallback to plain UDP: a client of a build with QUIC does not reach a server
that listens on plain engine UDP, so a game's server and client builds take the feature together. Plain engine UDP is what an
engine without the QUIC transports opens (`builder.disable_id("torque.transport-quic-server")` and
`"torque.transport-quic-client"`, or a build without the feature), on private addresses only. Browsers keep their links:
WebTransport where the server takes it, else WebRTC, else the WebSocket.

## What the engine's protocol does

- **The connect is one message.** The client's first message carries the game id, the framing versions it speaks, the manifest
  (always: a mismatch is refused with one line before any ghost), the auth ticket, the mods section, the password and the connect
  arguments. The server checks them in TGE mode's order (game, versions, password, player count, manifest and mods, ticket), then
  `onConnectRequest` decides. The accept carries the packet size (`max_packet`: the most the *server* sends; a client never sends a datagram over `NET_MAX_DATAGRAM`, 1,500 bytes, whatever it says, see *Datagram size* below), the packet interval, the byte rate, the id widths and the
  digests of the channel and RPC tables.
- **Links bring their security.** The same engine bytes run over every link; a link says what it already guarantees (whether it
  proved the peer's address, whether it encrypts, whether it runs congestion control) and how large a datagram it takes, and the
  engine adds sequence numbers, acks, lanes and fragmentation on top.
- **Lanes.** A packet carries a control lane (the connect and the accept), an RPC lane (plugin RPCs, reliable and ordered), an
  unreliable lane (unreliable RPCs), and the stream lanes the host writes: the server's ghost lane (events, ghosts and channels)
  and the client's move lane (its moves, events and upstream channels).
- **Rates.** One packet a tick each way, at most the link's datagram size and the connection's byte credit:
  `$Pref::Server::MaxClientBytesPerSecond` (256 KiB/s by default) is a ceiling on every link. Where a link has congestion control
  of its own, its "no room" skips a packet. Only plain UDP uses the engine's own backoff: halved when more than a tenth of a
  second's packets were lost, raised by a tenth otherwise, never under 16 KiB/s.
- **Stream packets.** A stream packet is counted lost only by the peer's acks (or an outage), never by the clock alone; a receiver reads no stream packet 3 or more behind its newest, so its sender's count of it lost is true, and the layers above resend what it held. A sender with 32 stream packets unresolved sends nothing until acks come, and the session times out if they do not.
- **Caps.** A server holds at most 16 sessions from one address and 64 from one network (an IPv4 /24, an IPv6 /64); of those not
  yet established, at most 4 from one address and 16 from one network on a validated link (2 and 4 on plain UDP), and the
  oldest of all is evicted past 64. They count the peer's own address: a browser's game socket and its data channel to a
  server go by stand-in addresses, and the server's web transport counts them by the TCP peer behind them. A data channel
  through a room service or a browser host's relay hides the peer's address from the host: there each peer counts as its own
  address and network, so only the per-address and total caps hold.

| Link | validated | secure | congestion control | datagram |
|---|---|---|---|---|
| memory (tests, the trace hashes) | yes | in process | no | 64 KiB |
| plain UDP | by the engine's cookie | no | the engine's backoff | 1,200 |
| QUIC (desktop players: `Listen::Udp` in this protocol, `Listen::Quic`) | QUIC's address validation, Retry under load | TLS 1.3 | QUIC's | the connection's, at most 1,200 |
| WebTransport (browsers, on the QUIC endpoint) | QUIC's | TLS 1.3 | QUIC's | the connection's less the session's one-byte prefix, at most 1,199 |
| WebRTC data channel (rooms, the relay) | ICE | DTLS | SCTP | 1,200 |
| WebSocket (a game socket) | TCP | `wss` only | TCP | 1,200 |

**The QUIC link.** A server listens on it with `Listen::Quic` (the `quic` feature, transport id `quic`, natively only), a
client connects with a `QuicTarget`. The engine's datagrams ride QUIC DATAGRAM frames on one UDP socket; no streams are opened
and no 0-RTT is accepted or sent (a connect carries a ticket and a password, and early data is replayable). A client whose
address changes keeps its connection, and the server runs its per-address checks again for the new address when the connection
switches to it (its first packet from there; quinn-proto reports no event for the path challenge completing). Only established
connections count against the server's caps (256 in all, 16 per address); handshakes in progress are capped on their own (64,
and a validated address may hold 4 of them, its network 16) and dropped if not finished within 5 seconds, so a flood of Initials
from forged addresses can neither fill the server nor lock an address out. Unvalidated handshakes may hold at most half of those
places (every unvalidated Initial past that gets a Retry), and at the cap a validated newcomer takes the place of the oldest
unvalidated one, whoever's, else of a validated one from its own address or network only, never another's. Each connection's backlog for the host is capped in datagrams
and bytes, and a datagram over 1,200 bytes is dropped on receipt. The server answers an unvalidated handshake with a Retry first, which costs it no state, under load (more than
32 new handshakes a second in all, or 8 from one network), when the handshakes in progress are at their cap, when the address
already holds a connection, or when the server is full; a validated handshake is refused when the server is full or its address
holds its share. Connection ids are random: a peer's address is never a connection's identity. The host sees each connection
at a stand-in address in `248.0.0.0/5` (the web transport's game sockets take `240.0.0.0/5`, so a server on both tells them
apart) and counts it by its real address; each connection's datagrams wait for the host in a backlog of its own (256), so a
flooding peer loses only its own.

**WebTransport for browsers.** Browsers cannot open a QUIC connection of their own, but they can open a WebTransport session,
which is HTTP/3 over QUIC: the same endpoint serves both when the listen says so (`QuicListen::web_transport`, or a web listen
with `WebListen::quic`; a dedicated server's `--webtransport`). The server offers two ALPNs, the raw one for desktops and `h3`,
and runs the least HTTP/3 a session needs (`torque-net-quic::h3`, sans-IO and fuzzed as hostile input): each side opens its
control and QPACK streams and sends SETTINGS (extended CONNECT, HTTP datagrams, one WebTransport session); the client's one
bidirectional stream carries a CONNECT with `:protocol webtransport`, which the server judges (the method, the scheme, the
`:authority` an IP address, `localhost` or one of the server's own names, the path, and the page's `origin`: the server's own
page by default, a list, or any; a request with no origin is a native client's) and answers `200`, or `400`, `403`, `404` or
`421` and nothing is established. The engine's datagrams then ride the connection's DATAGRAM frames behind the session's
quarter stream id (one byte), the same bytes as on the raw protocol; the server counts the connection, its backlog and its
bytes exactly as a raw one. QPACK runs on the static table alone (no dynamic table is ever advertised, so a reference to one
is an error), every frame, field section, name and value has a cap, a connection may open only the session's streams (one
bidirectional, three unidirectional, 16 KiB windows each) and any other stream is stopped unread, and a connection that opens
no session within the handshake timeout is closed. A desktop may speak the same session (`QuicTarget::web_transport`), so the
native tests exercise the path browsers take.

**Certificates browsers pin.** A browser with no certificate authority for the server takes its certificate by hash
(`serverCertificateHashes`), which the W3C allows only for an ECDSA P-256 certificate valid for under two weeks in all. So a
WebTransport server issues a short self-signed certificate on its identity key, valid thirteen days (backdated an hour for a
browser's clock), and re-issues it every six days; it publishes the hash of the one it presents and of the one that takes
over next, so a page loaded just before a rotation still joins after it (`torque-net-quic::certs`, tested over sixty fake
days). The web transport serving the page puts them in `webtransport.json` beside it (the QUIC port number, the path and the hashes,
written again at each rotation), and the page's `web/webtransport.js` opens the session with them and `requireUnreliable:
true` (a browser that would fall back to HTTP/2, which carries no datagrams, fails instead), from a secure context only
(`https`, or `localhost`); on any failure the page joins over its game socket and a data channel as before. Desktops never
look at the dates: they pin the key. A server with a certificate authority's chain on its identity key can present that
instead (`CertificateChoice::Chain`) and publishes no hash.

**Server identity and known servers.** A server has one long-lived ECDSA P-256 key, `identity.key` in its folder under the
user's data folder, made on first start (a host may name another file or hand the key over). Its identity is the SHA-256 of the
key's SubjectPublicKeyInfo, written `sha256:<64 hex digits>`, and it presents a self-signed certificate on that key. A client
checks the key, never a certificate authority: given an identity (a listing, an invite, a typed hash) it refuses any other with
a message naming both hashes; given none, it trusts the key it sees and remembers it for the address in `known_servers` under
its data folder, one `host:port sha256:<hex>` line each (at most 1,024; at the cap a new server is refused with a message naming
the file, and no pin is ever forgotten without the user), and refuses a later change with a message that names both hashes and
the file. The signature over the handshake is verified against the pinned key
either way.

**Script commands are two RPCs.** `commandToServer` and `commandToClient` keep their natives, their callbacks (`serverCmd<Name>(%client, ..)`,
`clientCmd<Name>(..)`), their tag expansion and their errors; only the carrier differs. Torque's protocol queues a command
and the NetString events of its new tags in order; the engine's protocol has no such queue, so a command is one call of the
built-in RPC `torque.command` (a client's) or `torque.command.client` (a server's), whose single field is the command with the
texts of its tags. A tag's text travels with every command that names it, so a command that is dropped at a limit leaves no
later one with a slot that means nothing. A script sees the same argument count, the same tagged strings (`detag` gives the
same text), the same order and the same `%client` in both protocols (`cargo test -p torque-test-app --test
server_commands_modes` runs one script suite in both and compares what the callbacks logged). What differs:

- **Limits.** A command is a client's input, so the server holds it twice. The RPC's own bucket (1,024 a second, burst 1,024,
  outside the connection's bucket for plugin RPCs) only bounds what a body costs to read; a call over it is dropped and counted
  (`rpcDrops(%client, "torque.command")`). Then each command meets its `CommandSpec` as in Torque's protocol: the command a
  plugin declared (`engine.server_command(..)`) is held to its own rate, arguments and permission, and every other command shares
  the default limits, one bucket for the connection: 32 a second, burst 64, at most 20 arguments of at most 255 bytes (a
  tag argument arrives expanded and is not held to the size). Refusals are counted for script: `commandDrops(%client, "Name")`
  for a name (the first 32 undeclared names a connection used), `commandDrops(%client, "*")` for every undeclared command
  together. Torque's protocol keeps its old behavior: an undeclared command has no limit.
- **Size.** A command is at most 20 arguments of 255 bytes each, as before, and its body, the texts of its tags included, must fit
  1,000 bytes; one that does not is not sent and the sender's console says so.
- **Permissions.** A command's `CommandSpec::requires` permission is checked by the server as in Torque's protocol (the caller's
  `torque-permissions` grants). The RPC itself is open to any connected client and `onRpcCheck` is not asked about it: the game's
  gates for commands (`command_allowed`, a held connection) and the spec's permission are the commands' own.
- **Before the accept.** Nothing is sent before the connection is accepted: `commandToServer` does nothing until then, and a
  server's calls ride the link of an accepted connection. A body the receiver cannot read ends the connection with `Packet Error.`,
  as a malformed packet does.
- **Datagram size.** A rule of the protocol for every client implementation: `max_packet` in the accept is the most the server
  sends, and a client never sends a datagram over `NET_MAX_DATAGRAM` (`torque_net::cap`, 1,500 bytes) whatever `max_packet` says,
  so its send limit is `min(max_packet, NET_MAX_DATAGRAM)` (the engine's client makes it in `engine_wire.rs`, `WireLink::max_datagram`).
  A server reads at most that (a source's share of a tick's work cannot take more) and drops a longer datagram as it is read,
  counting it in `Counters::oversize`. A body also carries exactly the tags its arguments name, each slot once.
- **Console.** The `Mapping string:` lines of Torque's NetString events are not printed: there is no NetString event.
- A local connection (`connectLocal`) keeps its commands as events in both protocols.

**Tickets are bound to the server's challenge.**
 A server with an auth provider answers every connect with a challenge, a
nonce it draws for that session; the client's auth provider makes the ticket then, for the server's address, the challenge,
the server identity the link verified and, on QUIC, the connection's channel binding (the TLS exporter of RFC 8446 section
7.5, label `EXPORTER-torque-auth`, 32 bytes, which both ends derive and no third party can), and the server checks it against
the same. So a ticket relayed through another server fails there, whatever that server tells its own clients. A ticket in the connect itself counts
for nothing, so a ticket captured on one connection fails on any other whatever the thief's own nonce. A ticket whose
connection the scripts accepted is refused for two minutes after, by its provider's canonical token (`AuthProvider::ticket_id`),
so a bearer token works at most once in any two minutes. What the server remembers is shared out: at most 64 tickets from one
network and 16 for one identity in two minutes, and 32 accepted a second in all (the 4,096 it holds can then never fill), so a
flood of valid tickets from one network locks out only that network. A server with no auth provider
sends no challenge. On the links without an identity or a binding (memory, plain UDP, WebRTC, WebSocket) the ticket context is
exactly what it was before them. Tickets are checked over unencrypted links too (the memory link, plain UDP, `ws`), all of
which listen on private addresses only; the server says so once on its console.

**Plain UDP is for LAN and development only.** It has no encryption, so a server in the engine's protocol refuses to listen on plain UDP at an address outside the private set (10/8,
172.16/12, 192.168/16, 169.254/16, 127/8): `GameServer::start_engine` fails with "0.0.0.0 is not a private address; the engine's
protocol takes plain UDP on private addresses only (LAN and development)", and a server whose socket was bound to one anyway
drops plain datagrams and warns once. The QUIC link is the encrypted native one; it may listen on a public address.

A plain link starts with a stateless cookie round trip: the client's `Hello` is padded to 1,200 bytes, the server answers with a
cookie and keeps nothing, and every opening packet carries the cookie. A validated link starts with the connect itself.

## A game's own net game

Above either protocol's framing, a server's network host is made of a net game (`torque_server::net::NetGame`): its events,
its ghost sources (`GhostGame`), its part of each connection, its string table and its rules at a few points of the passes.
The engine's servers speak Torque's (`TorqueNet`) unless a plugin sets the engine's net game slot:

```rust,ignore
use torque_app::net_game::{NetGameParts, NetGamePlugin};

builder.add_plugins(NetGamePlugin::<MyNet>::new(|vm: &mut Vm, game: &GameParts| {
    Ok(NetGameParts {
        game: MyNet::new(vm)?,
        ghosts: MyGhosts::over(&game.container),
        static_writes: None,
        // Wraps (or replaces) Torque's event layer before the bulk channels wrap it.
        payload: None,
    })
}));
let server = GameServer::<MyNet>::start_with_net(&engine, files, &start)?;
```

The factory runs once per server, in the `Net` stage, after the server game. A net game is a type, not a trait object, so the
server is `GameServer<MyNet>` and the code that knows the type starts it; the engine keeps a start for it too, which the
dedicated runner uses (`net_game::start_net_game`). `GameServer::start_engine` is the server of `TorqueNet` and is refused,
naming both types, on an engine set for another net game. One plugin sets each of these slots; a second fails the build
naming both:

| Slot | Plugin | Builder call (`ServerSlots`) |
|---|---|---|
| net game | `NetGamePlugin<G>` (`torque.net-game`) | `net_game(factory)` |
| codecs (`NetCodecs`, and with them the handshake's form) | `NetCodecsPlugin` (`torque.net-codecs`) | `net_codecs(make)`, run per server before the `Net` stage |
| server game (the container and shapes the world and the host build on) | the game's own | `server_game(make)`, run in the `Game` stage in place of TGE's install |

The payload wrapper is handed Torque's event layer and returns the layer the bulk channels wrap. **Its contract:** it forwards
every call to that layer (`on_game_packet`, `on_notify`, `write`, `busy`, `on_accept`, `mark_peer`, `mark_channels`,
`take_calls`, `take_work`, `on_drop`), which keeps TGE's event caps and the ghost layer's accounting. There is no separate
replacement: a wrapper that ignores the layer it is handed replaces it, and must then drive the host's event layer
(`NetShared::events`) itself and enforce caps of its own.

Some limits stay with the engine whatever the net game: the ghost hub's budgets, the connect, request and reject rates and
the handshake's per-source caps, and the pump's script budgets. The net game sets its own player default
(`NetGame::DEFAULT_MAX_PLAYERS`) and, through its codecs, `NetCodecs::server_config`: the max players and the handshake's form.

The plugins' net classes reach only the engine's own peers, and only through a net game that carries them: its ghost game
(`GhostGame::carries_plugin_classes`; `TgeGhostGame` does, and so does a game that wraps it) and its codecs
(`NetCodecs::plugin_classes`, as the engine's own `EngineCodecs` answer). A server whose engine has plugin classes warns at
start, naming them, when either does not.

## A host attached later

A server started on `Listen::Late` has no transport and no network host: none of the host's natives (`commandToClient`, the
permission natives, the `GameConnection` identity methods, the RPCs) and none of its passes. The rest of the start runs as
usual. `GameServer::attach_net` gives it both later, running the net game's factory then and attaching the same host, in the
same order, as a server started on a transport. Its game's scripts that make datablocks run after it:

```rust,ignore
let mut server = GameServer::<MyNet>::start_with_net_files(&engine, my_files, &ServerStart {
    listen: Listen::Late,
    ..start
})?;
server.attach_net(ServerLink::memory(address)?)?; // or any `SharedTransport`
```

The attach is refused (`AttachNetError::Refused`) when the server has a host already (every server started any other way
has one), when a datablock exists (a datablock's key starts with the host, so one made before it would never be sent), and
for a real socket when the scripts do not read the system clock (`server.server.use_system_clock()`). In the engine's
protocol a real plain socket outside the private addresses is refused too (`AttachNetError::PlainEngineBind`), as a start on
`Listen::Udp` refuses it. The host's passes go on last, so an attach that fails (`AttachNetError::Failed`) leaves a server
that reads and sends nothing, and a later `attach_net` can try again. It leaves `allowConnections` as the start left it. `GameServer::shared` is none until then. `start_with_net_files` is `start_with_net`
over a file host of the game's own, as `start_engine_with_files` is `start_engine` over one.

## Not changed

A local connection (the listen server's own client, `connectLocal`) stays in process and is the same in both protocols. The LAN
list's answers stay TGE's for now, on the QUIC endpoint's port in the engine's protocol. The plugins' class ids, the move layout and the ghost records are the same in both; they may
change in the engine's protocol later with a new framing version, never in Torque's.

## Datablock records in the engine's protocol

A datablock record is Torque's `packData` bit for bit, and in the engine's protocol a class may end it with a block TGE's lacks,
behind a presence flag: `ParticleData` and `ParticleEmitterData` carry the fields Torque has no name for (`turbulence`,
`collide`, `lit`, `softDistance`, `sizeByVelocity`, `trail`, `budget` and the rest). The block made the engine framing
version 3: a peer of framing 2 reads those records without it, so the connect refuses it
instead of letting its stream drift. A reader holds every value of a block to its range as it reads it. Version 5 added the
screen-space distortion's block to `ShapeBaseData` (and so to every class built on it, after its part) and to `DecalData`
(`distortion`, `distortionEdge`, `distortionMap`, `distortionMapScale`, `distortionScroll`, `distortionTint`,
`distortionChroma`, `distortionOnly`).

## Ghost records in the engine's protocol

A ghost record is Torque's `packUpdate` bit for bit, and in the engine's protocol a class may end a group with a block TGE's
lacks, behind a presence flag, by the same rule as the datablocks' blocks: a ShapeBase's cloak group ends with its dissolve (a
serial, the way, the time and how far the server was into it, the edge's color, glow and width, the emitter, the look). The
flag made the engine framing 4: a peer of framing 3 is refused at the connect.

## Determinism

`just trace-hash` prints each scenario once per protocol: `figure tge ...` and `figure engine ...`. Every line is pinned in
`crates/torque-trace-hash/expected.txt` with the reason of its last change, and the native and wasm32 runs must print the same
lines. The `tge` lines never move.

## Maintaining

**Module map.**
- `torque-net/src/engine/`: the framing and sessions (`framing.rs`, `session.rs`, `lanes.rs`, `gate.rs`); `table.rs`, a server's
  sessions with the per-source caps; `order.rs`, the wide packet numbers in send order; `bridge.rs`, the connect as the connect
  fields, block and ticket context both protocols judge; `link.rs`, the link contract and `LinkKind`.
- `torque-server/src/net/engine.rs`: the server's host wiring (`EngineHost`), called from `pump.rs` (receive, `handle_request`,
  the send pass) and `conn.rs` (the drop). `rpc.rs`: `RpcLanes`, the RPC lanes' carrier. `transport.rs`: `Transport::link_kind`,
  `Transport::channel_binding` and `check_plain_engine_bind`.
- `torque-net-quic`: the QUIC link (`server.rs`, `client.rs` over `quinn-proto`; `identity.rs`, the key and the identity hash;
  `tls.rs`, the configs and the pinning verifier; `known.rs`, the known-servers file; `h3/`, the WebTransport session:
  `frames.rs` the HTTP/3 frames and settings, `qpack.rs` and `huffman.rs` the field sections, `session.rs` the state machine
  and the CONNECT's judge, `drive.rs` its driver on a connection; `certs.rs`, the rotating short certificates and their
  hashes). `torque-app-server/src/transports.rs` and `torque-app/src/transports.rs` register it as `quic`;
  `WebQuicTransport` there pairs it with the web transport and writes `webtransport.json`; `torque-app/src/web_join.rs` and
  `web/webtransport.js` are the page's side.
- `torque-server/src/net/command_rpc.rs`: the two command RPCs and the inbox their handlers fill; `events.rs` (`EventLayer::encode_command`,
  `decode_command`) makes and reads a body against the connection's own tables of command tags; `pump.rs` (`run_command_bodies`) runs
  the commands as a packet's run; `rpc.rs` (`ServerRpc::send_command`, `ClientRpc::queue_command`) moves them; `cmdlimit.rs` holds the
  default limits. `torque-net/src/remote/body.rs`: the body's bytes and their hostile-input rules. `torque-client/src/conn.rs`:
  `command_to_server_rpc`, `receive_commands`.
- `torque-client/src/conn/engine_wire.rs`: the client's session under `TgeFeed` (`tge.rs`: `over_engine`, `engine_pump`,
  `engine_send`); `net.rs`: `Datagrams::link_kind`, `Datagrams::channel_binding`.
- `torque-app-server/src/plugins.rs`: `EngineProtocolPlugin`, `ProtocolPlugins`, `set_protocol`, `ServerWiring` (untyped,
  with the typed attach of the server's net game); `server.rs`: the gate, the digests, the RPC lanes and the public-bind
  refusal at start, and `wire_host`, the host's parts after its attach, which a start on a transport and `attach_net` both call;
  `net_game.rs`: the net game and codecs slots, `NetGameParts`, `RunServer`, the plugin-class warning.

**Data flow.** Server: transport, `pump::receive`, `engine::receive`, `SessionTable::receive`, the session's events: a connect
becomes a `PendingRequest` for `handle_request`, the move lane goes to `handle_game`, the RPC lane to `RpcLanes`, notifies through
`PacketOrder` to `PayloadLayer::on_notify`. Send pass: `engine::send_conn` polls the session with the payload layer as its ghost
lane's writer. The client mirrors it: `EngineWire` events become the feed's.

**Invariants and their tests.**
- The accept's `max_packet` is the server's own send limit; a client's is `min(max_packet, NET_MAX_DATAGRAM)`, made in
  `WireLink::max_datagram`, and the server drops a longer datagram as it is read (`commands_modes::a_datagram_over_the_limit_is_dropped_as_it_is_read_and_counted`).
- Fates reach the layers above in send order, and a packet not sent is lost at once
  (`engine::order::tests::fates_come_back_in_send_order_whatever_order_they_resolve_in`, `a_packet_built_again_after_no_room_gets_a_new_number`).
- A stream packet's fate is the peer's: never counted lost by the clock alone, so a peer that stops reading for a long frame
  reads no packet its sender counted lost; an outage is counted lost and the stream goes on; a packet three behind the newest
  is not read (`engine_link`: `a_peer_that_stalls_reads_no_packet_its_sender_counted_lost`,
  `an_outage_is_counted_lost_and_the_stream_goes_on`, `a_packet_three_behind_the_newest_is_not_read`).
- Sessions per address, per network and pending are capped; an evicted one is dropped by the host
  (`engine::table::tests::established_sessions_are_capped_per_address_and_per_network`,
  `pending_sessions_from_one_address_are_capped_and_the_oldest_of_all_is_evicted`).
- Plain engine UDP never listens on a public address (`net::engine::tests::plain_engine_udp_listens_only_on_private_addresses`;
  `transports_tests::a_public_bind_of_plain_engine_udp_is_refused` in `torque-app-server`).
- The RPC lanes hold a link to the send queue and a peer to its inbox share
  (`net::engine::tests::the_rpc_lanes_hold_a_link_to_the_send_queue_and_a_peer_to_its_inbox_share`).
- The byte credit and the plain link's backoff (`the_byte_credit_follows_..._eighth_of_a_second`, `a_plain_link_backs_off_...`).
- A ticket counts only for the challenge of its own session, once (`engine_tickets_tests` in `torque-app-server`:
  `over_a_validated_link_a_ticket_captured_on_one_connection_fails_on_another`, `..._presented_again_within_the_window_is_refused`,
  `over_plain_udp_a_captured_ticket_fails_and_a_replay_is_refused`; `sources_that_never_answer_their_challenge_keep_the_waiting_connects_bounded`; `net::engine::tests::a_flood_of_valid_tickets_from_one_network_locks_out_only_that_network`).
- A command body is read with every count and length checked against its caps and the bytes left, and a body with bytes left is refused
  (`remote::body::tests::a_body_that_is_not_one_is_an_error_and_never_a_panic`); a client that sends one is dropped
  (`a_command_body_the_server_cannot_read_ends_the_connection_in_the_engines_protocol`).
- A source's share of work holds the largest datagram (a const assert in `net::host`), and a client on a link that takes big datagrams
  still sends a command burst without stalling (`commands_modes::a_command_burst_over_a_link_that_takes_big_datagrams_never_stalls`,
  `a_datagram_over_the_limit_is_dropped_as_it_is_read_and_counted`).
- Both protocols give a script the same commands (`commands_modes::both_protocols_give_the_scripts_the_same_commands`), and the engine's
  default limits hold: 1,000 undeclared commands in a tick run at most 64 and are counted
  (`a_command_no_plugin_declared_is_held_to_the_default_limits_in_the_engines_protocol_only`). Nothing is sent before the accept
  (`nothing_is_sent_before_the_connection_is_accepted_in_either_protocol`).
- A server started on `Listen::Late` has none of the host's natives, and after `attach_net` a normal server's; a client's
  session against it writes the same datagrams, in both protocols; each refusal holds (`late_net_*` in `torque-app-server`'s
  `late_net_tests.rs`, torque-trace-hash's `tests/late_net.rs`).
- The command RPCs' tag tables are the connection's own, apart from the ghost records' (`EventLayer`'s `command_send` and `command_remote`):
  a command re-keying a slot never changes what a ghost record's string means.
- TGE mode's bytes do not move (`cargo test -p torque-net --test tge_golden`, `cargo test -p torque-trace-hash --test wire_golden`).
- A net game of Torque's parts with its own connection part and a payload layer over Torque's writes the goldens' bytes
  (`net_game_a_net_game_of_torques_parts_writes_the_goldens_bytes` in `wire_golden`); the slots' rules are `net_game_*` in
  `torque-app-server`.

- Speed is measured, not guessed: `cargo bench -p torque-net --bench ghost_section|bits|engine_session`, `-p torque-server --bench ghost_tick`, `-p torque-client --bench ghost_read` (what each guards is in the crates' Maintaining sections); the speed-ups change no byte, so the goldens and `just trace-hash` stay as they are.

**Extension points.** A game's own host is a `NetGamePlugin<G>`; a new per-server part of the host is a field of
`NetGameParts`. A transport says what its links are with `Transport::link_kind` (server) and `Datagrams::link_kind`
(client); both ends must agree. A new link (QUIC) is a transport and a `LinkKind`. A new lane is a framing version.

**Checks.** `cargo test -p torque-net --lib engine`, `cargo test -p torque-server --lib net::engine`, the `net-matrix` recipe,
`just trace-hash`.
