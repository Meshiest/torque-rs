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
- `torque::app::camera_blend`: `CameraBlend`, a camera that eases from one mode's eye to another's on a switch (first person
  to third and back), restartable mid-way, kept out of walls and ground by the game's own rule; the level demo's Tab uses it.
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
- `torque-example-fps`: a map rotation. A round lasts `$Pref::Server::RoundSeconds` (ten minutes); then every player sees the
  round's scores and the next map's name, and the next map of `$Pref::Server::MapList` (or of `missions/`) loads under the same
  connections, the scores back at 0; the view shows the round's clock. The examples' `loadMission` ends the running map for
  the clients in the game (`endMission`, `'MissionEnd'`, their ghosting reset) and sends them the new one's datablocks.
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
- `just examples/poses-clip`: a clip of the figure crouching, lying down, walking crouched, crawling, sprinting and reacting
  to hits from four sides and to the head, as another player's client draws it.
- The guide (mdBook) and the API reference with no rustdoc warnings.
- Continuous integration on Linux and Windows (formatting, clippy, tests, examples, rustdoc, the guide, the MSRV and
  cargo-deny), workspace lints, and the contributing, security and notice files.

### Fixed

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

[Unreleased]: https://github.com/TODO/torque-rs/commits/main
