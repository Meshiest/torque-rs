# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

The first development version. Nothing is published yet; this is what exists.

### Added

- `torque-ts-text`: the C runtime text and number conversions TorqueScript relies on (`atoi`, `atof`, the `printf`
  formats).
- `torque-ts-bytecode`: compiled TorqueScript (DSO) files: reading, writing and decoding, with dialect profiles.
- `torque-ts-compiler`: the TorqueScript compiler, from source text to DSO bytecode.
- `torque-ts-vm`: the TorqueScript runtime: the console's variables, namespaces, packages, objects, datablocks,
  natives and events, with a pluggable executor.
- `torque-collide`: convexes, GJK, polygon lists, working sets and object type masks.
- `torque-shapes`: DTS shapes and DSQ sequences (reading, writing, collision meshes, animated poses).
- `torque-shape-builder`: a builder of shape models from code (nodes, box and prism meshes, details, materials, keyframed
  sequences), with every derived figure filled in, written as a `.dts` by `torque-shapes`; behind the umbrella's
  `shape-builder` feature.
- `torque-sim`: the process list, players, AI players, items, projectiles and mounted images.
- `torque-vehicle`: rigid bodies, wheeled and flying vehicles.
- `torque-scene`: the scene container (bins, `findObjects` and `castRay` walks, the container searches) and a scene object's
  transform maths, behind a table a host may replace.
- `torque-resource`: the resource layer: a game folder read-only, zips mounted as folders, the scripts' writes in memory.
- `torque-game`: the TGE game classes as console classes: the scene objects and their container, the shapes the datablocks
  load, the simulated classes' hosts and the datablocks' computed values.
- `torque-server`: a dedicated server's main-loop pacing, the platform values its natives report and the datagram
  transport with its bind policy (its errors implement `std::error::Error`).
- `torque-client`: the client's engine half: the connection to a server over a game's feed (TGE's feed and records
  included), the control object's prediction, action maps and input routing, the LAN server query, the eye, zoom, frame
  rate and HUD feed, the console log file, and the window glue behind the `window` feature.
- `torque-mapcore` and `torque-maps`: the format-neutral map layer and readers for Torque interior, terrain and
  material-list files, and a `.dif` writer for every interior version the reader takes.
- `InteriorInstance` hosted and ghosted: the server reads its `.dif` and collides with its hulls (rays, movers, projectiles,
  lag-compensated shots), its record is TGE 1.4's, the examples' client predicts against it, and `torque-app`'s ghost scene
  draws it (its surfaces as static mesh objects of the world view, `WorldView::mesh_object`, each material's draw through the
  style's `interior_draw`) and puts projectile holes on it.
- `torque-interior-builder`: interiors (`.dif`, version 0 in TGE's form) built from convex brushes made in code or read from a
  Quake or Valve 220 `.map`: hidden faces removed, a solid BSP, the brushes as collision hulls with emit strings and polygon
  lists, texture planes and lightmap rectangles; a `.map`'s water, triggers, physical zones and spawn points as a `.mis`
  fragment. Meshes too (glTF, OBJ, through the model layer): triangles joined into convex polygons of one plane, material and
  texture map, drawn as they are; hulls authored (`col_*`, `hull_*`, `Col-N`, each its mesh's convex hull), cut from a closed
  part's solid BSP leaves, or slabs under an open part's polygons; sets over a version-0 interior's caps cut into pieces;
  `water*`, `trigger*` and `physical_zone*` meshes as mission objects.
- `torque-modelcore`: the format-neutral model layer (nodes with rest transforms, meshes with two UV sets, colors and up to
  four weights a vertex, skins, materials with their PBR textures, detail levels, animations sampled at even keys), in
  Torque's frame, with the naming conventions of Torque's shape loader (`detailN`, `Col-N`, `LOSCol-N`, `col_*` and `hull_*`
  hulls, mission volumes).
- `torque-import`: a glTF 2.0 reader (`.glb`, or `.gltf` with side files or `data:` URIs; meshes, skins, materials, images,
  step, linear and cubic-spline animations; every count, offset, stride, URI and image header bounded) and writer, and the
  model layer as a `.dts` (through `torque-shape-builder`) and a `.dts` as the model layer. A shape written as a `.glb` and
  read back makes the same `.dts`, byte for byte.
- Native glTF shapes: a datablock's `shapeFile` may name a `.glb` (or a `.gltf` with `data:` URIs); the server's preload, the
  client's prediction and eye and the shape drawing read it as the `.dts` it makes.
- `torque-import`: a Wavefront OBJ reader (`.obj` with its `.mtl` files; every count, number, index and path bounded) and
  a Blockbench reader (`.bbmodel`: cubes with their turns, origins and inflation, meshes, groups as nodes, per-face
  coordinates and turns, embedded PNG and JPEG textures; the JSON size, element and texture counts and sizes bounded), so a
  `.bbmodel` converts to a `.dts` or builds into an interior (each cube a closed part: one hull).
- Interiors carry how their materials are drawn as this engine's own surface flags (two bits TGE's tools never set and its
  reader ignores): nearest-neighbor filtering (a Blockbench model's textures, a glTF `NEAREST` sampler: sharp texels, the
  mipmaps still blended so a far wall does not shimmer) and see-through glass (a blended material, or a texture with partly
  see-through texels), drawn alpha blended in the translucent pass, each pane sorted on its own, and solid to collision as
  before (`torque_interior_builder::materials`, the importer's `Surfaces::material_flags`). Other interiors draw as they did.
- `torque-convert`: the converter command, its own crate: models (`.glb`, `.gltf`, `.obj`, `.bbmodel`, `.dts`) into `.dts` (with the
  images the model carries) or `.glb`, and `.map` brushes or meshes into interiors (`--hulls auto|authored|slabs`).
- `torque-shape-builder`: skin meshes, objects with a mesh in several details, primitives of several materials, aligned
  scale keys and blend flags in owned sequence specs; the name table is made at build time (nodes, details, objects,
  sequences).
- The example kit also writes `cube.glb`, `figure.glb` and `pistol.glb`; the arena loads them instead of the `.dts` files when
  `ARENA_SHAPES=glb`.
- `torque-net`: connections, the packet layer, events, ghosting, bit streams and string compression.
- `torque-ghosts`: the networked objects' record codecs: the layout framework, the class registries, the parent parts, and
  TGE 1.4's ghost and datablock layouts (`TgeGhosts`, `TgeDatablocks`).
- `torque-ghost-world`: the client's world of ghosts over any set of ghost classes, with TGE's set: the records merged, the
  datablocks, the control object and the eye, the mounted images' state machine, the script threads and the effects.
- `torque-images`: PNG and JPEG decoding and mip chains.
- `torque-fonts`: font caches, glyph metrics and text layout.
- `torque-gui`: the GUI object model, layout and the script methods of GUI classes.
- `torque-gui`: buttons, popups and their open lists, and slider thumbs drawn as rounded boxes (a solid fill, a one-pixel
  border, hover, pressed and disabled colors from the profile); a host keeps TGE's beveled boxes for a profile with
  `GuiExtension::classic_boxes`.
- `torque-audio`: WAV and Ogg Vorbis decoding, sources, channels, the listener and a mixer, with device output behind
  the `device` feature.
- `torque`: the umbrella crate, re-exporting the engine crates behind features, with headless examples (`script`,
  `shape`, `collide`, `net`).
- `torque-app`, re-exported as `torque::app` (the umbrella's `app` feature, off by default: it needs a GPU): what a game on the engine shares: its game
  folder in memory, a server on UDP or an in-memory link (with a dedicated server's pacing), a client with the server it hosts,
  the world view under the GUI, an orbit camera, a window, offscreen pictures and the command line's flags. The example games
  run on it, with their shared scripts (connection lifecycle, mission loading, figures, falls, notes) in
  `examples/common/game`.
- `torque-example`: a minimal dedicated server (the bundled script mission, TGE's records) and client, `min-server` and
  `min-client` (`just examples/min-server`, `just examples/min-client`), that connect over the loopback address and print the
  ghosts the client is sent. The example recipes are in `examples/justfile`.
- `torque-example-assets` (`examples/assets`): the example games' asset kit, made from code at run time: a unit cube with a
  collision hull, a box figure (eye, camera and mount nodes; idle, walk, run and jump sequences), a pistol with its image
  nodes and a fire sequence, a projectile, and their PNG textures.
- `torque-example-level-demo` (`examples/level-demo`): a singleplayer world in a window: a menu of maps (the `.mis` files in its
  `game/missions` folder), then the kit's box figure walking and jumping on the map's cubes, a server (torque-server with
  TGE's game classes) and a client (TGE's connection half) joined in one process by an in-memory datagram link.
- `torque-server`: `game::install` (TGE's game classes on one container and the process list, `GameBase::onAdd`), the
  GameConnection's ghosting and control methods (`activateGhosting`, `resetGhosting`, `setControlObject`,
  `getControlObject`), the `NetObject` behaviour that enters ghostable objects in the hub, the command natives registered
  with the host, and records packed from the simulated Player's state.
- `torque-client`: TGE's feed reads datablock events and answers `DataBlocksDone`.
- `torque-client`: named keys and mouse buttons (`Key`, `MouseButton`, also `torque::client::Key`) with the client host's
  `key`, `press`, `type_text` and `mouse_button`; the window maps winit's keys through `Key` (F13 to F24, the ISO key next
  to the left Shift and the side buttons now reach the platform layer).
- `torque-gui`: the US layout's scan codes (`map_virtual_key`) cover F13 to F24, so F13 to F15 give their key codes
  (`f13`..`f15`); F16 to F24 have none in the scan-code table.
- `torque-example-vehicle-demo` (`examples/vehicle-demo`): the kit's figure beside a dune buggy on green checkerboard hills (a
  `TerrainBlock`), for one player or a few on a LAN (host, or join from the list or an address); a click on the buggy takes the
  nearest free seat (the driver's or the passenger's), W A S D drive it, Space brakes, E gets out, Tab switches between the eyes
  and the chase camera; its tires throw dust and leave tracks. The kit adds the buggy and its
  wheel, the `sit` and `ride` sequences, the hills' `.ter` and their checkerboard.
- `torque::app::hosting`: hosting a game and joining one (the client's network mode, the address a hosted game binds and is
  joined by, a typed address, the join screen's natives), moved out of the arena shooter; `GameServer::start_with` runs a
  game's setup (its simulated classes) before the game classes are installed.
- `torque-render-scene`: the dust a vehicle kicks up (`vehicle_dust`: TGE's tire emitter by speed and slip, and its lift-off
  dust) and tire tracks a game may ask for (`tracks`: strips clipped to the ground's triangles, multiplied over it, fading);
  `torque::app`'s ghost scene runs both (`GhostScene::set_tire_tracks`). A vehicle look's wheels carry their slip.
- `torque::app::terrain`: a `TerrainBlock` over a legacy `.ter` with its collision on the server (`TerrainBlock::onAdd`), the
  same block as a static object of the client's prediction, and `WorldView::set_terrain` to draw it.
- `torque::app::terrain`: a terrain's material layers drawn (`Terrain::layer_draws`, `WorldView::set_terrain_layers`): the
  first material opaque, each other one blended over it where its weight is, each with its own texture and texture
  coordinates (repeated over the ground, or the host's for each triangle, such as a road's along it); the enhanced renderer's
  terrain layer (`material::LAYER`) sharpens a layer's coarse weight about one half into a narrow edge, broken up by its
  texture's alpha.
- `torque::app`: the world view's interiors are static meshes (`WorldView::set_static_mesh`, the enhanced renderer's static
  cells): uploaded once and kept on the GPU, culled there in parts by their boxes, their shadows in the cached cascades, so a
  frame costs nothing on the CPU for the interiors that stay put (a map of fifty forest interiors drew its lap at 21.6 ms a
  frame, now 4.3 ms, the same as without them).
- `torque-server`: Torque's net game ghosts the simulated vehicles (their records from the vehicle's state, its step's dirty
  bits as TGE's), and a Player that drives a WheeledVehicle sends the vehicle's packet data after its own.
- `torque-client`: TGE's feed reads a WheeledVehicleData's wheel count from its shape (the game's files,
  `ClientGame::file_reader`, which the prediction reads the driven vehicle's shape from too).
- `torque::app::lights`: a client's engine spot lights and its day (TimeOfDay, ScatterSky) into the world view
  (`WorldView::set_spots`, `WorldView::set_day`: the sun, ambient, fog, sky and clouds follow the day).
- `torque-render-hd`: a scene's cloud look (`HdScene::cloud_look`, `CloudLook`, `WorldView::set_cloud_look`): the layers,
  their light, coverage and ground shadow, sunlight reddened by the air at each layer's height, and finer steps inside the
  clouds. `CloudLook::SUNLIT_CUMULUS` gives sunlit cumulus; without a look the clouds are as before. The vehicle demo uses
  it.
- `torque::app::camera_blend`: `CameraBlend`, a camera that moves from one mode's eye to another's on a switch (first person
  to third and back): a critically damped spring on the offset from the new eye (95% of the way in `SETTLE_SECONDS`, 0.24 s,
  never past it), worked out exactly for any frame time, carrying the camera's velocity through a switch and a switch
  mid-way, kept out of walls and ground by the game's own rule; the level demo's, the arena's and the vehicle demo's Tab use
  it.
- `torque::app::camera_boom`: `CameraBoom`, a third-person camera's boom: five lines look for what is in the way (so a wall the
  boom grazes keeps off the near plane), the camera comes in fast (`PULL_IN_SECONDS`) and goes out slowly
  (`EASE_OUT_SECONDS`), and never moves through anything. The level demo's and the arena's camera behind the figure ride
  it.
- The examples' camera logs (`just examples/camera-log`): Tab in and out standing, walking, running, mid-turn and pressed
  again, the camera behind the figure past walls and glass, and Tab while driving, frame by frame at 60 and 144 frames a
  second as CSV (the camera's position, velocity and turn rate, the mode's eye and the figure), with `--render` the frames
  too.
- The level demo's camera behind the figure stays in front of interior walls and under their ceilings (its line of sight
  meets the interiors' hulls as it meets the cubes), and a map's SpotLights light the level demo's world (its server ghosts
  the engine's own classes to every client).
- The level demo's office map: a Blockbench model (`game/levels/office.bbmodel`, made by `tools/office.py`) built into an
  interior when the demo starts, with pixel textures drawn nearest, glass windows, a roof and ceiling lights.
- `torque-example-fps`: Tab eases the camera behind the figure (over the shoulder, kept in front of cubes and interior walls)
  and back to its eyes; backed against a wall it draws the first person's view. `torque-client`'s
  `Predictor::cast_ray` casts a segment in the client's container (the static index's objects near it and the ghosts).
- `torque::app::client`: `GameClient::start_local` runs a game alone over an in-memory link, the client's socket set aside
  while it runs.
- `torque-example-vehicle-demo`: the buggy's headlight (F, the driver's) from the lamps on its roof, which glow while it is on, a day that runs through sunrise, noon, sunset and
  night, "Play alone" (and `--solo`: no socket for the game, nobody on the network sees it), and Tab easing between the
  eyes and the chase camera.
- `torque-example-fps`: a map rotation. A round lasts `$Pref::Server::RoundSeconds` (five minutes); then every player sees the
  round's scores and the next map's name, and the next map of `$Pref::Server::MapList` (or of `missions/`) loads under the same
  connections, the scores back at 0; the view shows the round's clock. The examples' `loadMission` ends the running map for
  the clients in the game (`endMission`, `'MissionEnd'`, their ghosting reset) and sends them the new one's datablocks.
- `torque-example-fps`: a round also ends when a player's score reaches `$Pref::Server::ScoreLimit` (10; 0 turns it off), and
  the host of a game picks the next map from the pause menu's Change map entry (`serverCmdChangeMap`): only the connection that
  presents the key its own process gave the server may ask, only for a map of the server's list, and not twice within three
  seconds; the map loads at once under the same connections, the scores back at 0. The examples' `connectTo` takes that key.
- `torque-example-vehicle-demo`: a brown dirt race track of about 2 km round the start field and over the hill's shoulder (the
  hills' second terrain material, its texture laid along the track with tire-worn bands), the ground shaped along it (crests,
  dips, a shallow valley, a ridge and banked turns), and forests of low-poly trees round it (tall redwoods and firs, pines,
  and young spruces, oaks and birches under them, built from brushes into interiors, one a square of the map);
  `just examples/vehicle-demo-track-shots` draws them.
- The examples' pause menu (`examples/common/game/pause.cs`: Resume, Leave game and Quit over the view, the cursor on and the
  game's keys off), the FPS example's moved there and shared; in the vehicle demo Esc opens it on foot and at the wheel (it
  left the game at once). `cargo run -p torque-example-vehicle-demo --release --example steer-clip -- <dir>` draws frame
  sequences of the steering and of the tracks at speed on the driver's client and another's.
- The level demo in a browser: `just examples/web-level-demo` builds it for the web (WebGPU) and serves it on
  `http://127.0.0.1:8080/` (`?map=office` starts a map at once). The page fetches the game's files by their list
  (`files.json`) with a progress bar, keeps them in the browser's Cache Storage under their hashes (a second load requests
  none) and refuses a file whose hash is not the list's, with a message on the page; `web-level-demo-embedded` builds the
  module with its files inside it instead (the `embed-pack` feature). `just examples/web-level-demo-check` loads it in
  headless Chrome, walks the figure, counts the second load's requests, checks the refusal and fails on any console error. The engine's parts it needs run on the page's one thread, the
  browser's clock and the browser's fonts; `torque::app::GameFiles::pack` and `unpack` carry a game folder as one block of
  bytes.
- `torque-server`'s web transport (the `web` feature): browsers join over WebSocket and are served the game's page over HTTP
  on the same port, through the `Transport` trait as UDP is (`net::web::WebTransport`; `MultiTransport` takes UDP and the web
  on one port number). TLS through rustls with certificate and key files; plain HTTP and WebSocket listen on private
  addresses only. Hostile input is capped: the request line, the head and its headers, `GET` only, no body, paths
  normalized with no traversal, files served from memory; per connection a TLS handshake deadline and a budget of
  handshakes per pass, a head timeout, an idle timeout on game messages (pings do not count), a stall timeout, a deadline
  for each response at a slowest read rate, and a cap on unsent bytes; a game socket's bytes metered beneath the WebSocket
  layer: messages no larger than a datagram, no more frames a second (continuations and control frames included) than UDP's
  per-connection packet cap, as many full datagrams' bytes a second, at most 16 frames a message and a byte budget per
  pass; every connection's wire bytes (encrypted ones under TLS, so records that carry no plaintext count) metered per pass
  and per second too, and TLS driven one bounded step per read; connections capped in all (the longest-idle HTTP connection makes room when full), per address and per second.
  `Transport::service` does a transport's work between ticks, and `Transport::take_gone` reports closed sockets, whose
  connections the host drops at once.
  Requests must name an IP address, `localhost` or one of the server's names in `Host` (421 otherwise, against DNS
  rebinding), and a page's game socket must come from the server's own page or a listed origin (`net::web::Access`, 403
  otherwise). Game sockets go by stand-in addresses in `240.0.0.0/4`, one fixed IP per peer address (a peer that comes
  back has the same one, so the host's limits and bans hold), so they never share an address with a UDP peer; a socket's
  place under the per-address cap is reserved when its upgrade is answered.
- `torque-server`'s `webrtc` feature: WebRTC data channels for the web transport's browsers (`net::web::rtc`, str0m with
  no I/O of its own, driven by the transport's passes). A game socket carries the signaling as text messages (the server's
  hello, an SDP offer and answer); the channel (`ordered: false, maxRetransmits: 0`) then carries the socket's datagrams
  both ways under the socket's stand-in address, so the host's limits and bans hold for both, and the socket carries them
  again when ICE fails or the channel closes. One UDP socket of its own (the game's datagrams share first bytes with STUN
  and DTLS), ICE-lite, candidates from the bind address and any public addresses named. Routing: a binding request goes to
  the peer its USERNAME names, only from that peer's game socket's IP address and at a rate per source; any other datagram
  goes to the peer whose stack has validated its source by an authenticated check, and a mapping moves only when that
  peer no longer accepts the address, so another peer cannot take a player's traffic. Every cap counts datagrams as they
  come off the wire (per pass in all, per peer and for the peers still connecting; outputs per peer per pass; datagrams
  and bytes a second; datagrams before the channel opens), and what breaks them, or what the stack refuses, is dropped
  and counted: a forged source address never closes anything. Only authenticated misuse closes a peer (messages larger
  than a datagram or more a second than UDP's per-connection cap, a second channel, text or media), and then only its
  data channel: the browser goes on over its game socket. Peers capped in all, per address and per second, offers per
  socket, a connect deadline. `cargo test -p torque-server --features webrtc --test rtc`.
- `torque-client`'s `web-socket` feature: a client's datagrams over WebSocket (`net::web_socket::WebSocketDatagrams`: the
  page's `WebSocket` in a browser, a plain `ws` connection natively). `torque::app`'s `web` feature: a server on
  `Listen::Web` (the web alone, or UDP beside it), whose dedicated loop services the transport between ticks, and
  `ClientNet::External` with `ClientBuilder::set_socket` for a client that brings its own socket. `torque`'s `web` feature
  turns both on.
- `torque-client`'s `webrtc` feature: a client's datagrams over a WebRTC data channel beside its game socket
  (`net::web_rtc::WebRtcDatagrams`: the page's `RTCPeerConnection` in a browser, str0m over UDP natively). It starts on the
  socket, answers the server's hello with an offer, moves to the channel when it opens, and stays on the socket when the
  channel does not open in 10 s, closes, or is refused; `LinkWatch` shows which link carries the datagrams and the
  channel's longest gap. The game socket keeps the server's text messages (`take_text`, `send_text`).
- `torque::app`'s and `torque`'s `webrtc` features (the server's data channels and the client's side);
  `Transport::status` lines (a web transport's data channels and each peer's traffic), which a dedicated server prints
  every so often with `GameServer::set_status_every` and `GameServer::transport_status` returns.
- `torque::app::manifest`: the list of a web build's files (`files.json`: each file's path, size and SHA-256), read with caps
  on its size, its file count, the files' total bytes, the paths and the hashes' form; a fetched file is taken only when its
  size and hash are the list's. The examples' `GameFiles::from_fetched` reads the fetched files (packs and loose files).
- The arena for browsers: `fps-server --transport udp|web|both` (default `udp`) serves the browser build's folder over HTTP
  (`--web-root`, or `--no-files`) and takes game sockets over WebSocket on the same port, with TLS from `--tls-cert` and
  `--tls-key`, host names from `--web-host` and other pages' origins from `--allow-origin`; `node web/build.mjs fps` builds
  the join-only browser client (files fetched by `files.json`, or carried with `--embed`), which joins the server that
  served its page. `just examples/web-fps` builds and serves it; `just examples/web-fps-check` puts a desktop player with
  no window and headless Chrome in one game. `cargo test -p torque-example-fps --test web`: a UDP and a WebSocket client
  see each other, chat and hit each other; hostile peers are closed with the ticks' time unchanged; a banned browser that
  comes back is still refused. The web switches without `--transport web` or `both` are refused with an error.
- The arena on WebRTC data channels: `fps-server --webrtc` (with `--transport web` or `both`) gives browsers a data
  channel on a UDP port of its own (`--rtc-port`, default the game's port plus one), with candidates from the bind address
  and `--rtc-public-ip` (a host behind a 1:1 NAT). On `0.0.0.0` a `--rtc-public-ip` or `--rtc-lan` is required (without either the
  server refuses to start the channels) and the computer's private LAN addresses are left out unless `--rtc-lan` adds them
  (alone, only players on the network get a data channel, and a note says so at startup);
  `--rtc-loss` and `--rtc-stats` are for checks and print a warning at startup. The browser build moves to the channel when the server offers one and shows its link on the page
  (`data-torque-link`). `cargo test -p torque-example-fps --test rtc`: a browser on a data channel and a desktop on UDP
  see each other, chat and hit each other; with the channel's datagrams all dropped the browser plays over its socket.
  `just examples/web-fps-rtc-check`: headless Chrome joins on the data channel, keeps getting messages with 10% of them
  dropped, and plays over its game socket with UDP blocked.
- The website (`just site`, `node web/site.mjs`; deployed to GitHub Pages by `.github/workflows/pages.yml`): a gallery
  of the web examples with their pictures, each example in its own folder with its files fetched by URL, the guide in
  `book/` and the API reference in `api/`, every link relative. `node web/build.mjs <example> --solo` marks a page to be
  played alone (a site with no game server): the arena plays on its own server in the page's worker with no room, and
  neither it nor the vehicle demo offers to host or join. `web/serve.mjs` serves under a path (`/torque-rs/`);
  `just site-check` loads the gallery, its links and each card's game in headless Chrome from the cursor guard.
- `torque-rooms`: a room service for player-hosted games (library and `torque-rooms` binary). A host opens a room and
  gets a code (8 characters, 40 bits); joiners name the code, and the service carries only the signaling of one WebRTC
  data channel between each joiner and the host (a body must be an offer or answer that is an SDP of at most 4 KiB in
  printable ASCII, one candidate line of at most 512 bytes, or a busy or closed message with no body: the game's datagrams
  never go through it, though an SDP could still carry a little other text under the message caps). It runs on torque-server's web transport with its TLS, Host and Origin checks, per-address
  caps and wire meters, and adds caps on rooms in all and per address, joins per address a minute and failed joins a
  second (code guessing), joiners per room, signaling messages per joiner, and room, joiner and idle lifetimes. It hands
  each room's players STUN URLs and TURN credentials in the TURN REST API form (HMAC-SHA1 of `<expiry>:<id>` under a
  secret shared with the TURN server). torque-server's `rooms` feature: the room protocol (`net::room`); the web
  transport's `close_socket`, and its sends capped by its message limit (unchanged by default).
- `torque-rooms`' TURN credentials: a lifetime of 1 minute to 48 hours (`--turn-ttl`), an expiry that cannot wrap, and
  the secret file (`--turn-secret-file`) read only when it is a regular file of at most a little over a kilobyte and, on
  Unix, closed to other users. Tests: the credential against HMAC-SHA1 vectors computed outside Rust, the expiry, the
  ICE JSON, and credentials given only to a room's host (on `room`) and joiners (on `joined`), never on a refusal.
- `torque-server`'s `p2p` feature: a player's host (`net::web::p2p::P2pHost`), a `Transport` that opens a room on a
  room service and takes joiners over WebRTC data channels with no port forwarded: the web transport's data channels in a host mode
  (full ICE, so the host's own checks open its NAT; a server-reflexive candidate from the room's STUN server). A
  joiner's checks are taken only from the address the service saw it at or from a candidate's the host took (a check
  with USE-CANDIDATE from anywhere else would aim the joiner's traffic there), from at most 16 source addresses a joiner,
  under the per-source rate; an address counts as verified only when a response to the host's own check comes from it. A joiner cannot aim the host's checks: its candidates, offered or trickled, are
  taken only as IPv4 UDP (no `.local` names), at most 16 lines a joiner (refused ones counted), with distinct
  addresses capped across joiners, and never
  loopback, unspecified, broadcast, multicast, link-local, documentation, benchmarking, shared, reserved or the host's
  own; private ones only with `--rtc-lan`. Checks to addresses nothing authenticated came from are rated per joiner and
  in all.
  Each joiner goes by a stand-in of the IP the service saw, so per-address limits and bans hold; every signaling message
  goes through the data channels' caps, and a joiner whose channel closes is reported gone. `net::relay::RelayTransport`:
  the same caps (links in all, per address and a second; messages and wire bytes a second; oversize) for a server whose
  links someone else holds (a browser host's worker). `cargo test -p torque-rooms --test p2p`: a joiner reaches a host
  through a live service, datagrams both ways, the channel outlives the room socket and closes when the host leaves;
  media offers, broken offers and candidate floods are turned away with the open channel untouched and passes short.
- The arena hosted by a player: `fps-server --room ws://<service>` opens a room and prints its code (no UDP game
  socket); the browser build joins with `?room=<code>` (torque-client's `p2p` feature, `net::room_join`: signaling
  through the room, every datagram on the channel, none through the service) and ends the game with "The host left the
  game." when the channel closes. torque-app's `Listen::Bound` (a transport made and bound by the caller); the
  `torque` and `torque-app` `p2p` features. `just examples/web-fps-rooms` runs a service serving the page;
  `just examples/web-fps-p2p-check`: headless Chrome joins `fps-server --room` by code on a data channel, walks, and
  sees the host leave.
- A browser hosts the arena (`?host=1`): the page opens a room and shows its code (`data-torque-room`), runs the
  server in a Web Worker (`web/host-worker.js`, on `net::relay`, which holds every cap on what joiners send), plays in it
  over a local link, and holds each joiner's peer connection and data channel (`web/host.js`). A joiner's candidates go
  through the desktop host's rules (IPv4 UDP, no `.local`, no reserved or private addresses unless `?lan=1`, at most 16
  lines); at most 4 peer connections come from one address, and the room's peer numbers and ICE servers are read
  strictly. `web-fps-p2p-check`'s browser part: a second page
  joins by code, each page holds the other's figure, the joiner walks, and it says the host left when the host page
  closes. The p2p check launches its browsers through `web/guard.mjs` with `?nolock`.
- Invite links for the arena and the vehicle demo on their pages played alone (a static site, no server of ours): the
  menu's "Host with a link" (and the Esc menu's "Invite players") creates one invite offer link per player ("Create
  Invite Offer") (`<page>#join=<code>`, each works once), the player's page makes a reply link (`#reply=<code>`), and
  the host opens it in the same browser, where it hands the reply to the game's tab over a BroadcastChannel and closes
  (or it is pasted into the host's panel). The host's server runs in the page's worker as for `?host=1` (the vehicle
  demo, played alone in the page, opens it again with `?invites=1`); the joiner's data channel goes to its relay with
  every cap, and the joiner takes its host's messages within the same caps (`host.js` `meteredReceiver`). torque-app's
  `web_join`: `Target::Page` (a link the page holds), `link_opened`, `link_ended`, `ask_page`; `web_host_exports!` takes
  `start_link_client`. torque-client's `net::held::HeldDatagrams` holds what a page hands the client (none over the most
  bytes for a datagram, the count and the bytes capped). Codes (`web/codes.js`): the SDP cut to what a data channel
  needs, deflated, base64url, a version byte, a kind byte and a CRC-32 (about 230 characters with three candidates); a
  cap on the code's length and on its inflated size, strict lines, and the STUN server named by a key into the page's
  own list (Cloudflare, Google, or same network only), never by a URL. On any server, a host candidate may be a
  browser's mDNS name (a lowercase version 4 UUID and `.local`, nothing else), handed to the browser's ICE
  unresolved; neither side takes the other's code at its own names. The panels say which server is asked for the
  player's public address, and that the links carry it. `node --test web/codes.test.mjs`;
  `just examples/web-fps-invite-check` and `web-vehicle-demo-invite-check`: two guarded
  browsers trade the links on this computer's own addresses, the joiner walks, each holds the other's figure, and bad,
  cut short, oversized and used codes are refused with a message. The examples' pause menu takes an optional Invite
  players entry (`$Pause::CanInvite`, the game's `invitePlayers()`).
- The web pages' canvas fills the browser window and follows it. The game draws at the canvas's device pixels under
  the settings' Render scale (Auto, the default: all of them up to a 1920x1080 screen's worth, fewer past it; or 100%,
  75%, 50%), kept with the other settings, and lays its GUI out at that size (torque-client's
  `platform::web_input::RenderScale`; the page's `data-torque-render-scale`, and `data-torque-size` for the size drawn).
  The arena's page has the page bridge (`torque.call`), so its quality setting works too.
- The web builds' page shows each loading stage: the module's download (its bytes and percent as they come), its
  compile, the game's files, the start, the GPU, the map, then the game (`web/loading.js`). Its settings panel (the gear)
  picks the renderer's quality, applied at the next frame (the scripts' `setRenderQuality`), and shows the frames a second and the frame time
  (also F8); both are kept in the browser's storage, and Firefox starts at Low until the player picks
  (`web/settings.js`). `just examples/web-unit` tests both in Node. The module is built in a `web` profile (thin LTO) and
  run through `wasm-opt -O2` when it is installed.
- `torque::app::web_page`: the entry web builds share: the page's calls (`web_exports!`), the files the page fetches by
  its list, its settings, the loading stages and the frame times.
- `torque::app::page_bridge` and `web/bridge.js`: a web build's page and the game's scripts call each other. The page's
  `torque.call`, `eval` (a syntax error comes back with its line and column, and nothing runs), `getVar` and `setVar` are
  served at the next frame; script's `callPage(name, args...)` reaches only the handlers the page registered
  (`torque.on`). Arguments are strings within caps (count, size, calls a frame, requests waiting). The level demo emits
  `mapLoaded` and `playerFell`. The guide's "Games in a browser" shows it and the rule that a server reaches the page only
  through the game's own `clientCmd` functions.
- The vehicle demo in a browser, played alone (the server on an in-memory link in the page): `just
  examples/web-vehicle-demo` builds and serves it, `vehicle-demo --web-files <dir>` writes its pack and list,
  `just examples/web-vehicle-demo-check` walks its figure in headless Chrome.
- The vehicle demo for browsers: `vehicle-demo-server`, its dedicated server, takes the arena server's switches
  (`--transport both`, `--webrtc`, `--room`, TLS, the `--rtc-*` ones), now shared as `torque::app::serve`. The page
  still plays alone; `?join` joins the server that served it (`just examples/web-vehicle-demo-server`), `?room=<code>` a
  player's host, and `?host=1` hosts with the server in the page's Web Worker (`just examples/web-vehicle-demo-rooms`).
  Its menu offers to host and to join a room by its code, and shows a hosted game's code; a page built `--solo` offers
  only to play alone.
  The browser's join targets, its local link and its link's marks (`torque::app::web_join`) and the worker's server with
  its page calls (`torque::app::web_host`, `web_host_exports!`) moved out of the arena and are shared. A browser whose
  game socket closes now ends the game with "The server closed the connection." `just
  examples/web-vehicle-demo-mp-check`: two headless pages (served by the server, joined through a room, and a host page
  with a joiner) take turns driving, riding as the passenger and walking, and the second says so when the host goes.
- Pictures of the world alone: `Seat::world_picture` draws a game's world without its GUI (no HUD, hints or menus) over the
  whole picture, and the character example's `Step::WorldShot` does the same. `WorldView::set_water` draws a sheet of
  water (`Water`: its height, rectangle, texture and tint) as the enhanced renderer's water; `WorldView::clear_ground` and
  `WorldView::clear_day` take a map's ground and its day's light away again.
- The level demo draws a map's `TerrainBlock` and `WaterBlock` (read from the map the server runs, as the server reads
  them): the terrain also the own figure's ground in the prediction (`CubeClient::set_terrain`, changed as each map
  starts) and the floor of the camera behind it. A map's `TimeOfDay` under its `ScatterSky` lights its world, with sunlit
  clouds.
- The examples' `showcase` modules (the vehicle demo, the arena, the level demo): the buggy taken through the race
  track's tightest turn, its dust and tracks behind it; a shot in the arena from the side, frame by frame from its flash
  (the muzzle's flare, the bolt, the hit); the office's desks; each the world alone at the Ultra quality.
- The level demo's valley map (`missions/valley.mis`, 6 on the menu, `?map=valley` on the web): a lake in a long valley
  between mountains, the sun held low over a pass at its western end, woods on the lower slopes and sunlit clouds. The
  asset kit makes it (`torque_example_assets::valley`: the terrain with grass, rock and sand, and woods of the hills'
  trees; `trees::chunks_of` and `trees::build_chunks` cut any trees into interiors); the level demo adds it to the kit
  when it starts and to its web pack. `just examples/level-demo-valley-shots` draws it from a few places.
- `just examples/showcase` (`torque-example-showcase`): the front page's four pictures (the buggy in a turn, a shot in the
  arena, the valley at golden hour, the office), each the world alone at the Ultra quality, set in a 2 x 2 grid with thin
  transparent gutters (`hero.png`, 1920 x 1080, and `hero@2x.png`); `--examples` the examples' other pictures.
- `HdScene::ground_variation` (opt-in, `WorldView::set_ground_variation`): the ground's textures varied against visible
  tiling: every terrain layer's texture tiled stochastically and tinted by a large-scale noise. Off, the planes are drawn as
  before. `HdScene::cloud_time` (opt-in, `WorldView::set_cloud_time`): the volumetric clouds' own clock (their drift,
  change of shape and shadows) in place of the effect time.
- The valley opens onto the sea: west of the lake it sinks under the water, its mountains running out as headlands, and
  one water block runs from the lake out past the terrain's western edge into the haze (the sky drawn to 6,000 units), the
  low sun's glint along it. Its clouds run 150 times as fast as the clock; `--valley-clip` also writes its last frame
  moving and held still (the moving clouds keep their sharpness: no trail from their history).
- The valley made larger and wilder: jagged mountains of up to about 500 m from ridged noise (crests, crags and gullies)
  with the block's rim raised so no edge shows, its haze from 1,400 units and its sky to 3,500. Its woods are a few trees
  and groves of three to five trees built once (`valley_tree_*.dif`, `valley_grove_*.dif`) and placed, turned and scaled
  by about 500 `InteriorInstance`s (`trees::tree_sunk` sinks a grove's trunks deeper): the level demo's web pack is
  3.4 MB with the valley. Its map asks for the ground's variation (`groundVariation = 1`) and runs its clouds 30 times as
  fast as the clock (`cloudTimeScale = 30`, a timelapse over the held golden hour), both read by the level demo from the
  map's `MapInfo`; the level demo asks for a packet every tick at the largest size (the valley's places ghost in about
  7 s). `--valley-shots` writes each view's GPU frame time, and `--valley-clip` eight seconds of the sky moving.
- The showcase's buggy drives through the turn in the late afternoon with its roof lamps on, their beam on the track
  ahead; its valley is seen from over the lake's eastern end, the lake in the middle; `--examples` adds the valley's
  ground close up and the whole valley from far up its eastern end.
- Water reflects the volumetric clouds: where its reflection is the sky (the planar reflection's sky, which its target now
  marks with an alpha of -1; a screen-space reflection's miss; the sky cube at Low) the clouds' image of this frame is laid
  over it along the reflected direction, off a calmer surface than the waves' (a third of their slope), and past the view's
  top edge the image's edge at the same heading stands in for it, read blurrier. No history of its own: fast clouds leave no
  trail in it.
- The valley's far ranges: three rows of jagged mountains on each side of the sea (a summit and its shoulders each, one
  interior a mountain: 84 small interiors), farther, broader and taller row by row, fading into the haze toward the
  horizon with the sea open between them; the haze starts at 2,400 units. The level demo's web pack is 4.5 MB with the
  valley. A sixth valley view, low over the sea with the clouds in its reflection (`valley-sea`, also in the showcase's
  `--examples`).
- The water's mirrored view fogs toward the atmosphere's own sky (its light toward the point, turned up), as the main view
  fogs toward the sky behind a point, instead of the horizon ban's flat color: a far shape fogged into the sky reflects
  in the color it is seen in. Capture faces, the sky cube's faces and a server's sky box keep the ban's color.
- The showcase's valley is the view low over the sea (`SEA_PICTURE_MS` into the timelapse, when the clouds' reflections read
  best), the far ranges' peaks with more faces, and a valley view's eye kept above the water where the ground is under it.
- A map's numbers are not trusted: the level demo refuses a `WaterBlock` whose position, scale or opacity is not finite
  numbers (`nan` and `inf` parse as numbers) and holds `cloudTimeScale` to 0 to 1,000; its clouds' clock starts over when it
  is not finite; `WorldView::set_water` draws no water of non-finite numbers; the renderer sends the effect time in place of
  a cloud clock that is not finite.
- The example kit's figure walks (`walk`, worked out from where its feet stand: `torque_example_assets::gait`): each foot
  lands on its heel with the toe up, rolls flat, stays put while the body passes over it and leaves from its toe, both feet
  down at each landing and the hips dipping a little, at 2.1 units a second (a held-Shift pace in the demos), the left heel
  landing where `run`'s left leg reaches furthest forward. The feet turn at new `ankle_l` and `ankle_r` nodes. A test
  holds a planted foot to under 5% of the ground's speed.
- The example kit's figure's `run` is worked out the same way and bounces: a short flight between steps, the hips sinking as
  the body passes over the planted foot and rising into the air, the heel kicked up and the knee driven forward through the
  swing, its planted feet held by the same test.
- `torque-client`: a Player walks at walking pace (`player_anim`, the engine's own `walk` action): a shape with a forward
  `walk` plays it instead of `run` under the geometric mean of the two speeds, and backward instead of `back` under that of
  the walk's and the back's, with a 5% margin either way; walk and run keep their place in the loop as they blend. A shape
  without `walk` keeps TGE's pick.
- `torque-game`: a synced Player (one with hitboxes) walks by the same rule (`synced::walk_or`), its walk stepped by the
  ground covered and keeping its place in the loop as it changes to and from the run, so its hitboxes follow the walk the
  clients draw; a Player without hitboxes keeps TGE's pick.
- `just examples/walk-clip`: a clip of the kit's figure walking, running and walking backward, its action chosen by the
  client's Player animation, drawn offscreen from the side and three-quarters (an mp4 through ffmpeg, and a strip).
- `torque-render-hd`: a ghosting check (`cargo run -p torque-render-hd --release --example ghosting`): scripted scenes (a
  moving shape's shadow under a still camera, a fast sun, a light switched, a moving camera, smoke) drawn with every history
  and against a reference of each instant drawn with none and settled, printing per scene the share of pixels off and the
  frames until the image settles after each change (`--max-settle`, `--max-trail` fail past a limit); `--audit` turns each
  pass's history off on its own (`HdWorld::histories`, all on by default). The vehicle demo's `ghost-shots` example measures
  the buggy driving past a still camera at golden hour the same way.
- `torque::app::ghosts`: a steady first-person view (`GhostOptions::steady_eye`, the engine's own; TGE stands the view at
  the drawn `eye` node): the own Player's view at its `eye` node at the rest pose, carried by its real motion, while the
  body and its hitboxes keep their animation; `eye_sway` keeps the sideways sway, and `OwnView::drawn_eye` gives TGE's eye.
  On in the level, vehicle and arena demos; `just examples/eye-clip` draws a before-and-after clip.
- The example kit's figure crouches and lies down (`torque_example_assets::poses`): `crouch_root`, `crouch_forward`,
  `crouch_backward`, `crouch_side`, `prone_root`, `prone_forward` and `prone_backward`, posed from where its feet and hands
  stand (each leg and arm turned to reach there), the crouched walk from the walk's model with the hips low, a crouched
  side step and a crawl whose standing feet and hands stay put (the slide test holds them under 6% of the ground's speed);
  `prone_hold` and `prone_aim` hold the pistol ahead lying down. Each pose fits its box (1 x 1 x 1.6 crouched, 2 x 2 x 0.8
  prone), and a crouch lowers the body over feet that stay where they stand.
- The example kit's figure gets down to prone and up again (`prone_down`, 0.768 s; `prone_up`, 0.864 s): from the crouch it drops
  to its right knee, puts its left hand down where it lies, comes onto both knees, lowers its chest and slides its legs back
  to lie flat, the head up and looking ahead; up is the same played back (it pushes up on the hand, brings its knees under and
  rises). Its feet, knees and hand touch the floor without sinking, the planted hand stays put, the right arm reaches ahead
  (over a held pistol's arm), and each starts and ends in the pose it joins. Crouched still, the figure is on the balls of
  its feet, the feet where the standing ones are.
- The example kit's figure reacts to hits: blend sequences played once over any pose or gait, `flinch_front`, `flinch_back`,
  `flinch_left` and `flinch_right` (the body punched away from the side hit, the hips pushed back and a small step) and
  `flinch_head_front` and the rest (a sharp snap of the head, the torso barely moving), each at rest at its start and end.
- `torque-sim`: poses after Torque3D's (`player::pose`, the engine's own): standing, sprinting, crouched and prone, each with
  its own box (`crouchBoundingBox`, `proneBoundingBox`), speeds (`maxCrouchForwardSpeed`, `maxProneForwardSpeed`,
  `maxSprintForwardSpeed` and the rest) and run force, picked from the move's pose triggers (`$player::crouchTrigger`,
  `$player::proneTrigger`, `$player::sprintTrigger`). A pose with no room for its box is refused (no standing up under a low
  ceiling), between standing and prone the Player kneels in the crouch for 10 ticks, it cannot jump lying down, and a mounted
  Player stands. A datablock without the fields is TGE's.
- `torque-game`: a `PlayerData`'s pose fields (`sim::player::poses`), and each pose's own actions (`crouch_root`,
  `crouch_forward`, `prone_root`, ...) picked on the server by the rule its clients use, an action of another pose blending
  in over a longer, eased transition; a pose's own arm action (`prone_hold` for `hold`) on both sides. A Player record's move
  shows the pose its server chose in its pose triggers, so clients draw the pose taken, not the one asked for. Hitboxes and
  lag compensation follow the pose, and a script thread's reaction.
- `torque-shapes`: an eased transition (`ShapeInstance::transition_to_sequence_eased`, the engine's own).
- `torque-client`: a Player's animation plays its pose's actions (`PlayerMotion::pose`, `record_pose`) and the script
  threads its server runs on it over its own (`PlayerAnimation::script_threads`); the prediction keeps the own Player's pose
  (`ControlState::pose`), from the pose fields a game gives in `PredictGame::player_preload`.
- `torque::app::ghosts`: other Players drawn in the pose their server chose (`GhostOptions::poses`), and the script threads on
  every Player, so a hit's reaction drawn is the one its hitboxes have; the steady first-person eye lowers with crouching and
  lying down and rises again, eased with the body (a hit's reaction never moves it).
- The demos walk by default and sprint with Shift held (was: run, and walk with Shift); Ctrl held crouches and Z lies down or
  gets up, in the level, vehicle and arena demos and the character example, their hints and the justfile saying so. A hit in
  the arena plays a reaction away from the shot (`playerFlinch`, common/server.cs), the head's for a head shot.
- Getting down and up between the crouch and prone (the engine's own): the step holds the Player still for 24 ticks lying down
  and 27 pushing up (`pose::settling`), and crouched two ticks more before it stands; a shape's `prone_down` and `prone_up`
  play to their end on the server and the clients alike (`PoseSets::way`), so the hitboxes and the eased first-person eye
  follow the same timing. C crouches too in every demo (a browser closes its tab on Ctrl+W), and the hints say so.
- While a Player's body moves between two poses (the blend, or the way down or up), shots reach it through the union of
  both poses' boxes (`pose::reach_box`: the scene's world box and the lag samples), so its drawn head and chest stay hittable;
  the movement box changes at once as before. A pose with another box is kept at least 8 ticks, so a client cannot flip its
  box every tick.
- `just examples/poses-clip`: a clip of the figure crouching, lying down, walking crouched, crawling, sprinting and reacting
  to hits from four sides and to the head, as another player's client draws it.
- `torque-render-core::pipeline_cache`: on Vulkan a `Gpu` keeps its compiled pipelines in a wgpu pipeline cache
  (`Gpu::pipeline_cache`), loaded from a per-user file (`pipeline_cache::set_dir`, `user_dir`) and saved by
  `Gpu::save_pipeline_cache`; every pipeline the GUI pass, the blur and the enhanced renderer make uses it
  (`HdWorld::pipeline_cache` for an extension's own). A windowed app saves it once the view's warm-up is done and at the
  close, so a start after a driver update no longer compiles every pipeline again. The file is keyed by adapter and
  driver, at most 64 MiB, written whole and renamed into place, and a missing, damaged or foreign one starts an empty
  cache. This is the workspace's one `unsafe` block (`Device::create_pipeline_cache`, with a `SAFETY` comment):
  torque-render-core denies `unsafe_code` instead of forbidding it and expects it on that function only.
- torque-app's `web_join::switch_to` (`switchTo` in `web_host_exports!`, with `linkDatagram` and `setLinkSender`): a
  page switches its running game in place between playing alone, an invite link's game and hosting.
- The guide (mdBook) and the API reference with no rustdoc warnings.
- Continuous integration on Linux and Windows (formatting, clippy, tests, examples, rustdoc, the guide, the MSRV and
  cargo-deny), workspace lints, and the contributing, security and notice files.

### Fixed

- `torque-vehicle`: a vehicle held at full throttle against a tree (or any interior) crept into it and drove through, and its
  body could sink into the terrain under a steady push. The body now stays out of map pieces, kept by the separating axis
  test (`torque_collide::sat`) after TGE's contact responses (`Body::hold_out`).
- `torque::app`, `torque-render-hd`: the effect clock could not be advanced, so water, the volumetric clouds of a map
  without a clock of their own and the stars' twinkle stood still. `WorldView::advance_time` runs it (`WorldView::time`,
  wrapping at `EFFECT_TIME_WRAP`). The film grain no longer follows the effect time: its pattern is fixed (`GRAIN_SEED`),
  so it never shimmers.
- `torque-fonts`: the system fonts on Linux and macOS (the menus drew no text outside Windows). The font folders (XDG's, or
  macOS's three) are scanned once, with depth, entry, file and size limits, and every file is checked (`ttf::check`) before
  it is read. A face missing there falls back on a metric-compatible free face (Arial on Liberation Sans, then DejaVu Sans;
  Times New Roman on Liberation Serif; Courier New and Lucida Console on Liberation Mono), then on any sans face found. On
  Windows, too, a face's file is checked and size-capped, and a face name is looked up in the fonts folder's listing, never
  joined into a path.
- Examples' pose keys: Ctrl holds the crouch and C toggles it; C or Ctrl lying down gets up to a crouch, Space lying down
  or crouched by C stands rather than jumping, and a respawn or a restart starts the figure standing (it kept the prone
  toggle).
- `torque-client`: in a browser the window's loop no longer spins between frames: the clock steps and the frame is drawn
  once per animation frame (a hidden page still runs its clock five times a second).
- `torque-client`: in a browser, the keys and buttons held are let go when the page loses the focus, is hidden or loses
  the pointer lock (a key let go meanwhile stayed held in the game). A pointer lock the browser ends while the game holds
  the mouse (its Esc) is the game's Esc, so the level demo's menu opens; in full screen, Chromium passes Esc on to the
  game (the page's keyboard lock).
- The third-person Tab camera: a switch while walking, running or driving no longer stops the camera dead and lunges after
  the figure (the camera's speed fell from the figure's to near 0 at the switch: 17 to 2 m/s in the buggy, 7 to 1 m/s in the
  arena), and Tab pressed again mid-way no longer stops it at full speed (54 m/s to 0 in one frame); the move eases from
  smoothstep's slow start and late peak to a quick start and a soft arrival, relative to the figure, so it never lags it.
  The figure is hidden while the camera is within `CLOSE` of its eyes on the way in and out, so its head no longer fills
  the view.
- The level demo's camera behind the figure looks through glass instead of stopping in front of it, and comes in fast and
  goes out slowly past walls, posts and furniture instead of jumping in and out (walking along the office's desks: 10 jumps
  of up to a meter a frame before, 2 of 0.7 m after, where a hanging light crosses the line right by the camera).

- `torque-render-hd`: the sun disk no longer shows as a dull orange dot darker than the sky about it at golden hour. The
  neutral tonemap keeps every color's hue past white, so a saturated disk 100 times over white showed as the same orange as
  the sky about it and darker than a paler sky. The disk now takes its own path to white (white-hot overhead; cream, peach
  and orange toward the horizon; the tonemap itself is unchanged) and dims with the air it shines through (the atmosphere's
  transmittance), so it stays the brightest thing in the sky at every elevation.
- `torque-shape-builder`: a sequence with ground frames is a path-making one (`MakePath`, as TGE's exporter marks it), so a
  Player finds the example figure's gait speeds and plays its gaits at the rate it moves (they played at one rate at any
  speed).
- `torque-render-hd`: a change of light no longer leaves a smeared copy behind under a still camera (the tree shadows
  stepping with the running day, a shape's shadow sweeping past, a light switched). The TAA clips its history into the
  neighborhood's box every frame and starts it again where the neighborhood's mean changes, with no rules for moving shadows,
  water, the sky or see-through shapes; the ambient occlusion keeps no history (a pattern fixed per pixel, always filtered in
  space) and the contact shadows' march has a fixed pattern. The ghosting check: the fast sun settles in 7 frames, not
  never (71 % of the frame off before), the vehicle demo's buggy pass at once, not never.
- The level demo's office: textures redrawn as clean pixel art (flat areas, a few shading steps, lit and shaded edges, no
  noise), the office chairs' bases on the floor (they hovered 4 cm over it), and the screens drawn unlit (monitors, two new
  laptops on the meeting table, a chart on the meeting room's screen).
- The level demo: a quick tap of Space held the jump until the next tap (the script counted only the press); now it jumps
  once. Tab switches the camera (it was V), as in the other demos, and every map draws its textures nearest (cubes, tiles,
  figures, interiors; the mipmaps kept).
- `torque-render-hd`: every point or spot light whose light reaches the view has a shadow cube, sized by its importance (its
  brightness, the share of the view its sphere takes, its cone): three tiers of cube arrays at the quality's face size, a half
  and a quarter of it (High: 4 x 512, 8 x 256, 16 x 128, less memory than its 8 equal cubes), where a fixed 8 cubes were
  traded among the lights in view and a light without one shone unshadowed, so shadows popped on and off as the camera
  turned or walked. A light that changes tier crossfades from one cube to the other over 12 frames, a light that holds a
  cube counts 1.25 times as much, and off-screen casters keep shadowing the view (a test with a point light and a low sun).
- Four HD shaders mixed `*` and `^` without parentheses, which WebGPU's WGSL rules (Chrome's compiler) refuse.
- `torque::app`: an interior ghost that takes the index of another map's interior at the same place is drawn from its own
  file (the old one stayed).
- `torque-client`: the end of the server's ghosting (a mission's end) keeps the connection's datablocks and net strings, as
  TGE's client does (the next mission's figures came without their mounted images).
- `torque-net`: a ghost-always object's update keeps the mask bits its pack could not send (a light mounted on an object
  the connection had no ghost of yet), so a later update sends them.
- `torque-server`: Torque's net game sends a ShapeBase's script threads (`playThread`; it sent none).
- `torque::app::view`: a day whose ScatterSky names no shadow color keeps the daylight's (the shadows were black).
- `torque-client`: a Player that gets off its mount plays `root` and picks its actions by its moves again (it went on
  showing the seated action the server had set).
- `torque-game`: a shape with fewer than eight collision details (the bounds report and the hull slots read past its
  lists); the class specs name TGE's console methods (the crate's natives were refused without them), and `SceneObject`,
  `ShapeBaseData` and `PlayerData` read their constructors' values (a Player made from a script datablock could not move).
- `torque-client`: the steering a vehicle is drawn with (its front wheels, its `steering` thread and the hands of a figure
  on it) eases each frame toward the ticks' steering (`look::SteeringEase`, a critically damped follow, the same at any frame
  rate; `seconds: None` draws the last tick's, as TGE does); another's vehicle eases toward its newest value run on at
  the rate its updates bring. It turned in a tick's steps, and another's in its updates' steps.
- `torque-client`: a vehicle's drawn wheel contacts, where its tire tracks start and its tires' dust rises, are under the
  tires the frame draws; they were a tick's travel behind them at speed.
- `torque-client`: the client's ticks fall on their 32 ms grid at any frame rate (`conn::tick_time`). A frame that did not end
  on the grid ran the tick late and counted the next tick from the frame, so at 144 frames a second the client ticked every
  35 ms (fewer moves than the server's ticks) and drew the Players and vehicles unevenly between ticks.
- `torque-client`: a vehicle's springs are drawn between its last two ticks' extensions, as its body is
  (`look::SpringLooks`); the tires, and the tracks and dust under them, stepped once a tick.
- `torque-client`: another player's vehicle is drawn where it is now: through the warp its update's state coasts on at its
  velocities (and by the update's age, measured and eased over updates, at most 0.1 s), and the drawn pose eases to it
  (TGE holds the state while the drawn pose warps to the update's position, the past by then). At 15 to 20 m/s it
  was drawn 1.7 m behind the server's on average, 2 m at most; now 0.15 m ahead, as the driver's own (0.06 m behind at most).
- `torque-client`: a ghost vehicle's update age never turns into NaN and is capped at 3 units of travel and half a radian of
  turn; an update's rotation of no length is left out, momenta that give velocities that are not finite leave the body at
  rest, and a tick that makes anything not finite is undone, so a hostile update never draws a vehicle at NaN and the next
  one heals it. The drawn offset from an update shrinks by a share each tick (an update coming before the last one's ease
  ended slowed it), and past 10 units it is drawn there at once. A warp or first-sight update draws it at its pose with no
  offset left of an earlier update.
- `torque-example-vehicle-demo`: the seated look stops at the edge of its range as the mouse moves (the mouse turned back
  had to undo what went past the edge first), and Tab puts it in the new camera's range (a look behind the buggy becomes the
  head turned as far as it goes).
- `torque-client`: a window takes the system cursor (lock, confine, center) only while it is focused and shown. It lets go the
  moment it loses focus or is hidden, never moves the cursor then, and takes it again when it is focused and shown and the
  game still wants the hold (a window made in the background held it before). In a browser the pointer lock is asked for
  only right after a click, never renewed on a timer or after Esc.
- The web page: `?nolock` stops every pointer-lock request (a non-writable, non-configurable no-op). The browser checks get
  their browser only from `launchGuarded` (`web/guard.mjs`), whose pages (and popups and frames) cannot take the pointer
  lock whatever they load, and also load with `?nolock`. `web/guard-scan.mjs` fails any script in the repository that names a
  browser driver or program or loads a module by a computed name, and `web/guard.test.mjs` shows a page that locks on click
  never gets it; both run in `just examples/web-guard-test` (first in each check recipe) and in CI. Headless Chrome on
  Windows confined the real cursor when a check clicked the canvas.
- `torque-render-hd`, `torque::app`: the arena's first shot no longer stops the game for a moment. The effects'
  pipelines (particles, trails, decals) are made on a worker thread when the world view is made
  (`HdWorld::warm_effects`), and the ghost scene loads the shapes and textures its datablocks name as they arrive; a
  first shot with a cold shader cache took 0.44-0.68 s (12 pipelines made), now within a millisecond of the median frame
  (the fps example `first_use` measures it).
- `examples/fps`: aiming down the sights eases both ways on a critically damped spring (0.18 s, the same at any frame
  rate, no overshoot; a press let go mid-way turns back from where it is at the speed it had), the pistol arcs in and
  up, and a synced Player's arm moves between its arm actions over 0.18 s (`ARM_BLEND`) instead of switching at once.
- `torque::app`: a client draws a projectile on the server's line, from the raw floats every first update carries (the
  bounce block) instead of the initial block's 10-bit direction and centimeter start (up to 0.18 degrees off: at a
  wall's corner one screen showed a graze and another a miss, and the trail jumped back when the server's explosion
  came); with `GhostOptions::bolt_stops` it stops a bolt where that line meets the world (the arena: within a millimeter
  of the server's hit on both clients, `tests/bolt_line.rs`). The arena fires from `getSteadyEyePoint` (engine addition:
  the eye at the shape's rest pose), so it keeps the steady first-person eye on with its shots on the crosshair.
  Crouched or prone `getSteadyEyePoint` is the eye of the pose's root held at its start, eased from the last pose's over the
  body's blend as the client's view is, so shots leave from the lowered view in every pose (`tests/aim.rs`).
- The camera behind the figure rides lower with a crouched or prone figure: the arena's boom from the lowered eye, the level
  demo's looking no higher than the eye; lying down and looking up, the boom stops above the floor.
- Invite links: two tabs of one browser, or two computers on one network, connect on any server (Cloudflare and Google
  too): a code carries the browsers' mDNS names (`<uuid v4>.local`, which only the browser resolves, on its own network)
  beside the public address, which most home routers do not loop back. The joiner's game joins once its data channel
  opens; it asked at once and gave up ("The game did not answer.") about 12 s later, before the host had the reply.
  Every state of the Join with Link panel, and a reply tab whose reply was not taken, has Back to the menu (the page
  again with no invite, its game at its menu), and the failures say what the player can try. `invite-check.mjs same`:
  two tabs of one guarded browser, a Cloudflare invite with no STUN server contacted, the reply opened 20 s later.
- The arena and the vehicle demo on a page played alone wait at their main menu (Play alone, Host with a link, Join with
  Link) instead of starting at once; the arena's Host with a link takes the host into its game with the panel open, and
  a page joined by an invite link offers Play alone instead.

[Unreleased]: https://github.com/Meshiest/torque-rs/commits/main
