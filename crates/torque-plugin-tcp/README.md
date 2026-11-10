# torque-plugin-tcp

TGE's `TCPObject` and `HTTPObject` script classes for a torque-rs game: a script connects to an address, listens on a port, sends
text and gets each line it receives as a callback; an HTTPObject sends a GET or a POST and gets the answer's body line by line.
Every server and every client script engine gets the classes and its own sockets.

It is not part of the default plugins, and stays opt-in: a game adds it (TGE always has the classes, but this plugin brings
rustls and the system's certificate store with it).

```rust
use torque::app::plugins::DefaultPlugins;
use torque::engine::EngineBuilder;
use torque_plugin_tcp::TcpPlugin;

let engine = EngineBuilder::new()
    .add_plugins((DefaultPlugins, TcpPlugin::default()))
    .build()?;
```

The scripts are TGE's:

```
function Feed::onLine(%this, %line) { echo("got " @ %line); }
function Feed::onDisconnect(%this) { echo("done"); }
new HTTPObject(Feed);
Feed.get("https://example.com", "/news.txt");

function Chat::onConnectRequest(%this, %address, %id) { new TCPObject(ChatPeer, %id); }
function ChatPeer::onLine(%this, %line) { %this.send("you said " @ %line @ "\n"); }
new TCPObject(Chat);
Chat.listen(28100);
```

| Method | Does |
|---|---|
| `TCPObject::connect(address)` | `[ip:]host[:port]` (28000 when no port): `onDNSResolved` or `onDNSFailed` for a name, then `onConnected` or `onConnectFailed` |
| `TCPObject::listen(port)` | `onConnectRequest(address, id)` per connection; `new TCPObject(name, id)` takes it (on `$Pref::Net::BindAddress` when set) |
| `TCPObject::send(text, ...)` | sends each argument as it is |
| `TCPObject::disconnect()` | closes whatever the object has open; no callback |
| `HTTPObject::get(address, path[, query])` | `GET path?query`; each body line is `onLine`, the end is `onDisconnect` |
| `HTTPObject::post(address, path, query, body)` | `POST` with the body as a form |

An HTTPObject's address may start with `http://` or `https://` (TLS through rustls, checked against the system's trusted
roots; port 443 when none is given).

## What differs from TGE

- **Nothing blocks a tick.** The name lookup, the connect and the TLS handshake run on a thread of their own; sockets are
  read and written without waiting.
- **Caps per script engine** (`TcpLimits`, set with `TcpPlugin::with_limits`): 32 of the script's own connections, 8 accepted
  connections waiting to be taken (counted apart, so peers cannot use up the script's own; no connection is accepted
  once both together reach 32, and an adoption past 32 closes the connection), 4 listen sockets, 8 connects
  under way (a disconnected connect whose lookup still hangs counts until its thread ends), 256 KiB read and 256 KiB sent per tick (64 KiB read from one connection), a 1 MiB send queue per connection,
  256 KiB lines, 4,096 callbacks and 8 accepts per tick, 10 s for a script to take an accepted connection, 20 s for a connect.
  A connect past a cap warns and reports `onConnectFailed` on the next tick (one callback however many connects were
  refused; a later connect drops it); a peer that sends a longer line is disconnected. A host name with a space, a control
  byte or a byte past ASCII is refused before any lookup (`onDNSFailed`), so it never reaches a request's `Host` header.
  Each tick's pass is charged to the script engine's work budget.
- **HTTPS** for `https://` addresses.
- **`post` sends a real POST** with its body as a form; TGE 1.4's sends a GET. This is kept on purpose.
- **No allowlist, and the client's network policy does not gate script sockets**: scripts reach any address, as TGE's do.
  `NetPolicy` (the game socket's policy, `Off` by default and `Loopback` in single player) is not consulted, so a mod's
  update check works in single player too.
- In a web build (wasm32) the plugin adds nothing.

## Maintaining

**Module map.**

| Module | Holds |
|---|---|
| `lib.rs` | `TcpPlugin`: the recipes that install the classes on each server and client, and the two tick hooks |
| `limits.rs` | `TcpLimits` and the pass's budget (`PASS_WORK`, `PASS_STEPS`, `MAX_CONNECT_THREADS`) |
| `host.rs` | `TcpHost` (one per script engine: the socket table, the objects' state) and `poll` |
| `natives.rs` | the classes' registration, their behaviours (`processArguments`, `onAdd`, the destructor) and the methods |
| `lines.rs` | `parseLine` and `finishLastLine` |
| `http.rs` | the request text (`expandPath`, GET and POST) and the answer's state machine (`onReceive`, `processLine`) |
| `address.rs` | the address text (`Net::openConnectTo`'s rules, the `http://` and `https://` forms) |
| `link.rs` | the connect threads and `Link`, an open plain or TLS connection |
| `tls.rs` | the rustls client settings and the non-blocking TLS reads and writes (feature `https`) |

**Flow.** A method (`connect`, `get`) closes the object's socket and starts a connect thread, which reports `Resolved` (a name
only), then `Connected` with the open `Link` or a failure. Each tick (`ServerTick::before` on a server, a `ClientTick` on a
client) runs `poll` under a host budget: the callbacks a cap deferred, then every socket, starting at another one each tick.
A connecting socket gives its reports as callbacks; a listen socket accepts; an open one writes its queue, then reads and
splits, one `onReceive` at a time, each line an `onLine`. The end of a connection gives the waiting bytes as a last line, then
`onDisconnect`. An HTTPObject's request goes out right after the script's `onConnected`.

**Invariants and the tests that hold them.**

- No borrow of the table is held while a script runs, and after each callback the socket is checked to still be the object's
  (`a_deleted_object_closes_its_connection`, `a_listening_object_offers_each_connection_and_a_new_object_takes_it`).
- TGE's order of callbacks (`a_name_is_looked_up_before_the_connect_and_a_missing_one_fails_alone`,
  `a_connect_sends_and_receives_lines_and_the_last_one_comes_at_the_disconnect`, `a_connect_to_a_closed_port_fails`).
- TGE's splitting and HTTP reading, whatever the read boundaries (`lines_tests.rs`, `http_tests.rs`:
  `a_chunked_body_joins_a_line_across_chunks_and_stops_at_the_last` reads the same answer a byte at a time).
- Every cap refuses past its limit and loses nothing it accepted (`tests/caps.rs`, one test per cap); refused connects keep
  one pending callback per object (`refused_connects_keep_one_callback_and_a_later_connect_drops_it`), and hanging lookups
  hold the lookup cap after a disconnect (`lookups_that_hang_hold_the_cap_after_a_disconnect`).
- A host name that could break a request line never reaches one
  (`a_host_name_with_a_line_break_is_refused_before_any_lookup`, `a_name_with_a_blank_a_control_or_a_byte_past_ascii_is_unusable`).
- A hostile peer never stalls a pass: a line that never ends, an endless stream, a slow drip, a flood of empty lines
  (`tests/hostile.rs`, each pass timed).
- HTTPS checks the certificate (`tests/https.rs`: a trusted test root passes, no roots fails as `onConnectFailed`).
- Both sides tick their own sockets (`tests/sides.rs`, a real server and client).
- The pages of the script reference list the classes (`just script-reference-check`), and the methods that reach the network
  declare `Effects::NETWORK` (torque-plugin-loader's `native_effects` test).

**Extension points.** `TcpPlugin::with_limits`, `with_resolver` (a test's or a game's own lookups) and `with_tls_config` (a
game's own roots, built with `client_config`). `poll` is public for a host that runs its own loop. A new callback is a
`Callback` variant and its `method` name; a new cap is a `TcpLimits` field checked where the work is taken, with a test in
`tests/caps.rs`.

**Checks.**

```
cargo test -p torque-plugin-tcp --all-features
cargo clippy -p torque-plugin-tcp --all-targets --all-features -- -D warnings
cargo clippy -p torque-plugin-tcp --all-targets --no-default-features -- -D warnings
cargo check -p torque-plugin-tcp --target wasm32-unknown-unknown
```

Every test stays on 127.0.0.1 with ports the system picks; names are answered by a test resolver, never by DNS.

Credit: follows TGE 1.4's `game/net/tcpObject.cc` and `game/net/httpObject.cc` as Torque3D carries them under the MIT license
(`Engine/source/app/net/tcpObject.cpp` and `Engine/source/app/net/httpObject.cpp`, each Copyright (c) 2012 GarageGames, LLC).
