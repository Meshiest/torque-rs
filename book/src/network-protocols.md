# Network protocols

A game speaks one of two network protocols, and every server and client of it speaks the same one:

- **Torque's own** (`tge`, the default for now): TGE's handshake, packet header and event layer, byte for byte, so a server and a
  client of the engine can meet a real TGE peer. It is an optional compatibility mode; nothing in the engine's own protocol is
  chosen to fit it.
- **The engine's own** (`engine`): its own framing over a link contract. It owes TGE nothing on the wire, and every peer is the
  engine's own.

Above the framing both protocols carry the same things: the ghosts and their records, the net strings, the datablocks, the
moves, the bulk channels, the events and the prediction. A game's scripts see the same callbacks in both (`onConnectRequest`,
`onConnect`, `onConnectionAccepted`, `onConnectRequestRejected`, `onConnectionDropped`, `onConnectionTimedOut`, `onDrop`) with
the same arguments.

## Choosing one

The protocol is an exclusive slot of the engine (`engine.net().protocol(..)`), set by one of two plugins:
`TgeProtocolPlugin` (`torque.net.tge`) or `EngineProtocolPlugin` (`torque.net.engine`). `DefaultPlugins` and `MinimalPlugins`
carry both through `ProtocolPlugins`, with one of them off:

```toml
[net]
protocol = "engine"   # or "tge"
```

In `game.toml`, `[net] protocol` turns the other plugin off, as `[runner]` does for runners. In code,
`torque_app::plugins::set_protocol(&mut builder, ProtocolKind::Engine)` does the same. The variable `TORQUE_NET_PROTOCOL`
(`tge` or `engine`) picks which plugin the groups turn on for a whole run, which is how the `net-matrix` recipe runs the same tests in
both; a game's `[net] protocol` and a test that sets the protocol itself keep theirs. A browser build reads the variable when it
is compiled.

## What the engine's protocol does

- **The connect is one message.** The client's first message carries the game id, the framing versions it speaks, the manifest
  (always: a mismatch is refused with one line before any ghost), the auth ticket, the mods section, the password and the connect
  arguments. The server checks them in TGE mode's order (game, versions, password, player count, manifest and mods, ticket), then
  `onConnectRequest` decides. The accept carries the packet size, the packet interval, the byte rate, the id widths and the
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
| WebRTC data channel (rooms, the relay) | ICE | DTLS | SCTP | 1,200 |
| WebSocket (a game socket) | TCP | `wss` only | TCP | 1,200 |

**Tickets are bound to the server's challenge.** A server with an auth provider answers every connect with a challenge, a
nonce it draws for that session; the client's auth provider makes the ticket then, for the server's address, the challenge
and the server identity the link verified, and the server checks it against the same. A ticket in the connect itself counts
for nothing, so a ticket captured on one connection fails on any other whatever the thief's own nonce. A ticket whose
connection the scripts accepted is refused for two minutes after, by its provider's canonical token (`AuthProvider::ticket_id`),
so a bearer token works at most once in any two minutes. What the server remembers is shared out: at most 64 tickets from one
network and 16 for one identity in two minutes, and 32 accepted a second in all (the 4,096 it holds can then never fill), so a
flood of valid tickets from one network locks out only that network. A server with no auth provider
sends no challenge. The server identity is part of what a ticket is made for once a link verifies one (QUIC, later). Tickets are checked over unencrypted links too (the memory link, plain UDP, `ws`), all of
which listen on private addresses only; the server says so once on its console.

**Plain UDP is for LAN and development only.** It has no encryption, so a server in the engine's protocol refuses to listen on plain UDP at an address outside the private set (10/8,
172.16/12, 192.168/16, 169.254/16, 127/8): `GameServer::start_engine` fails with "0.0.0.0 is not a private address; the engine's
protocol takes plain UDP on private addresses only (LAN and development)", and a server whose socket was bound to one anyway
drops plain datagrams and warns once. An encrypted native link (QUIC) comes later; the link contract keeps it replaceable.

A plain link starts with a stateless cookie round trip: the client's `Hello` is padded to 1,200 bytes, the server answers with a
cookie and keeps nothing, and every opening packet carries the cookie. A validated link starts with the connect itself.

## Not changed

A local connection (the listen server's own client, `connectLocal`) stays in process and is the same in both protocols. The LAN
list's answers stay TGE's for now. The plugins' class ids, the move layout and the ghost records are the same in both; they may
change in the engine's protocol later with a new framing version, never in Torque's.

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
  the send pass) and `conn.rs` (the drop). `rpc.rs`: `RpcLanes`, the RPC lanes' carrier. `transport.rs`: `Transport::link_kind`
  and `check_plain_engine_bind`.
- `torque-client/src/conn/engine_wire.rs`: the client's session under `TgeFeed` (`tge.rs`: `over_engine`, `engine_pump`,
  `engine_send`); `net.rs`: `Datagrams::link_kind`.
- `torque-app-server/src/plugins.rs`: `EngineProtocolPlugin`, `ProtocolPlugins`, `set_protocol`; `server.rs`: the gate, the
  digests, the RPC lanes and the public-bind refusal at start.

**Data flow.** Server: transport, `pump::receive`, `engine::receive`, `SessionTable::receive`, the session's events: a connect
becomes a `PendingRequest` for `handle_request`, the move lane goes to `handle_game`, the RPC lane to `RpcLanes`, notifies through
`PacketOrder` to `PayloadLayer::on_notify`. Send pass: `engine::send_conn` polls the session with the payload layer as its ghost
lane's writer. The client mirrors it: `EngineWire` events become the feed's.

**Invariants and their tests.**
- Fates reach the layers above in send order, and a packet not sent is lost at once
  (`engine::order::tests::fates_come_back_in_send_order_whatever_order_they_resolve_in`, `a_packet_built_again_after_no_room_gets_a_new_number`).
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
- TGE mode's bytes do not move (`cargo test -p torque-net --test tge_golden`, `cargo test -p torque-trace-hash --test wire_golden`).

**Extension points.** A transport says what its links are with `Transport::link_kind` (server) and `Datagrams::link_kind`
(client); both ends must agree. A new link (QUIC) is a transport and a `LinkKind`. A new lane is a framing version.

**Checks.** `cargo test -p torque-net --lib engine`, `cargo test -p torque-server --lib net::engine`, the `net-matrix` recipe,
`just trace-hash`.
