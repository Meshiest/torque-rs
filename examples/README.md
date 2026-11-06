# Examples

Small games made on the engine. Each uses the asset kit (shapes, textures, terrains and trees made by code at startup, no
files in the repository) and the app layer (`torque::app`: the files in memory, a server, a client, a window). Every
picture here is drawn offscreen by the example itself, the world alone with no HUD (`just examples/showcase` and
`just examples/showcase <dir> --examples`, see [The pictures](#the-pictures)).

Run the recipes from the repository's root (`just examples/<recipe>`) or from this folder (`just <recipe>`). Each recipe is
a plain `cargo run`, shown beside it. The web builds need rustup's `wasm32-unknown-unknown` target and Node; the first build
installs wasm-bindgen's command line into the target folder.

<table>
<tr>
<td align="center"><a href="#the-level-demo"><img src="screenshots/level-demo-plaza.webp" width="200" alt="The level demo"><br>The level demo</a></td>
<td align="center"><a href="#the-level-demo"><img src="screenshots/level-demo-office.webp" width="200" alt="Its office"><br>Its office</a></td>
<td align="center"><a href="#the-valley"><img src="screenshots/level-demo-valley.webp" width="200" alt="Its valley"><br>Its valley</a></td>
<td align="center"><a href="#the-arena-shooter"><img src="screenshots/fps-shot.webp" width="200" alt="The arena shooter"><br>The arena shooter</a></td>
</tr>
<tr>
<td align="center"><a href="#the-vehicle-demo"><img src="screenshots/vehicle-demo-turn.webp" width="200" alt="The vehicle demo"><br>The vehicle demo</a></td>
<td align="center"><a href="#the-character"><img src="screenshots/character-walk.webp" width="200" alt="The character"><br>The character</a></td>
<td align="center"><a href="#the-asset-kit"><img src="screenshots/assets-figure.webp" width="200" alt="The asset kit"><br>The asset kit</a></td>
</tr>
</table>

Also: [the minimal server and client](#the-minimal-server-and-client), a dedicated server and a console client that prints its ghosts.

## The level demo

![The level demo's plaza from above](screenshots/level-demo-plaza.webp)
![The level demo's office: desks under their ceiling lights](screenshots/level-demo-office.webp)

A singleplayer world: a menu of maps, then the box figure walking and jumping on them. The server and the client run in
one process and talk over an in-memory link, so it is the engine's whole network path with no socket. The maps are
TorqueScript mission files (`level-demo/game/missions/`): the plaza, the maze, the ramps and the towers are built of cubes;
the office is a Blockbench model (`game/levels/office.bbmodel`) built into interiors when the game starts, with its glass
and ceiling lights; the valley is a terrain with a lake (below).

**Controls:** 1 to 9 pick a map on the menu; W A S D walk, Shift sprint, Ctrl or C crouch, Z lie down or get up, Space
jump, mouse look, Tab first or third person (the camera eases between them), R restart, Esc back to the menu.

**Run it:**

| | Recipe | Command |
|---|---|---|
| Native | `just examples/level-demo` | `cargo run -p torque-example-level-demo --release` |
| Web | `just examples/web-level-demo` (serves it on http://127.0.0.1:8080/; `?map=office` starts a map at once) | `node web/build.mjs level-demo` |
| Web, files inside the module | `just examples/web-level-demo-embedded` | `node web/build.mjs level-demo --embed` |
| Pictures | `just examples/level-demo-shots`, `just examples/level-demo-valley-shots` | `cargo run -p torque-example-level-demo --release -- --shots <dir>` |

**Shows:** a server and a client in one process; missions loaded from TorqueScript; the client's prediction against cubes,
interiors and terrain; interiors built from a Blockbench model (nearest-sampled textures, see-through glass); spot lights
with shadows; the camera blend; the web build with its files fetched by a list and kept in the browser's cache.

### The valley

![Low over the valley's sea at golden hour, the clouds reflected in the water](screenshots/level-demo-valley.webp)
![The valley from high on its eastern slope](screenshots/level-demo-valley-wide.webp)

A level demo map (6 on the menu): a lake in a long valley between jagged mountains, running out as a fjord to the sea and
the horizon, the sun held low over the water, woods and groves on the lower slopes, and sunlit clouds racing over in a
timelapse (150 times as fast as the clock), their shadows sweeping the slopes. Walk down from the meadow to the shore. The terrain, its grass, rock and sand,
and the trees are made by the asset kit's code (`assets/src/valley.rs`); the map is `level-demo/game/missions/valley.mis`.

**Run it:** as the level demo, then 6; in the browser `just examples/web-level-demo` and open
http://127.0.0.1:8080/?map=valley. Its pictures and their GPU frame times: `just examples/level-demo-valley-shots`;
eight seconds of its sky moving: `cargo run -p torque-example-level-demo --release -- --valley-clip <dir>`.

**Shows:** a legacy terrain (`TerrainBlock`) with blended materials, drawn and collided with on the client too; a
`WaterBlock` drawn as the enhanced renderer's water (waves, reflection, refraction, the sun's glint); a `TimeOfDay` held at
golden hour under a `ScatterSky`, with volumetric sunlit cumulus on their own faster clock; the ground's textures tiled
stochastically and varied at large scale against visible repeats (both opt-in, per map); a few tree and grove interiors
placed hundreds of times.

## The arena shooter

![A shot in the arena: the bolt in flight, the muzzle's flare, the other player hit](screenshots/fps-shot.webp)

An arena shooter for a few players: a main menu, host or join (the list of games on the network, or an address), the box
figure with a pistol in first person, bolts that hurt, rounds and chat. A dedicated server takes desktop players on UDP and
browsers over WebSocket and WebRTC data channels in the same game.

**Controls:** W A S D walk, Shift sprint, Ctrl or C crouch, Z lie down or get up, Space jump, mouse look, left button
shoot, right button aim down the sights, R reload, Tab first or third person, T chat, Esc pause menu.

**Run it:**

| | Recipe | Command |
|---|---|---|
| Native client (host or join from its menu) | `just examples/fps-client` | `cargo run -p torque-example-fps --release --bin fps-client` |
| Dedicated server (LAN) | `just examples/fps-server` | `cargo run -p torque-example-fps --release --bin fps-server` |
| Dedicated server (internet) | `just examples/fps-server-public` | `... --bin fps-server -- --bind 0.0.0.0 --port 28000` |
| Web (browsers and desktops in one game) | `just examples/web-fps` (a browser joins at http://127.0.0.1:28000/) | `node web/build.mjs fps`, then `fps-server --transport both` |
| Pictures | `just examples/fps-shots` and the other `fps-*-shots` recipes | `... --bin fps-client -- --shots <dir>` |

**Shows:** the network game end to end (the LAN server query, ghosting, the client's prediction); posed hitboxes and lag
compensation; projectiles with ribbons, muzzle flares and smoke, bolt holes as decals; cross play between UDP, WebSocket
and WebRTC.

## The vehicle demo

![The buggy in a turn of the race track, dust and tire tracks behind it](screenshots/vehicle-demo-turn.webp)

A drive for one player or a few: the box figure walks up to a dune buggy, clicks to take a seat (the driver's or the
passenger's) and drives it round a dirt race track over checkerboard hills, through forests of low-poly trees, under a day
that turns.

**Controls:** on foot W A S D walk, Shift sprint, Ctrl or C crouch, Z lie down or get up, Space jump, mouse look, Tab first
or third person, click the buggy to get in; at the wheel W throttle, S reverse, A D steer, Space brake, mouse turns the
camera, wheel zooms, F headlight, Tab the driver's eyes, E get out;
`[` and `]` slow down or speed up the day, `\` its normal speed; R restart, Esc menu.

**Run it:**

| | Recipe | Command |
|---|---|---|
| Native (play alone, host or join from its menu) | `just examples/vehicle-demo` | `cargo run -p torque-example-vehicle-demo --release` |
| At once | `just examples/vehicle-demo --solo` (or `--host`, `--join <address>`) | `... --release -- --solo` |
| Pictures | `just examples/vehicle-demo-shots`, `vehicle-demo-track-shots`, `vehicle-demo-cloud-shots` | `... --release -- --shots <dir>` |

**Shows:** wheeled vehicles (springs, tires, steering) predicted on the client; mounting; tire tracks and wheel dust; other
players' vehicles drawn smoothly between updates; a terrain with a blended dirt layer; forests built as interiors; the day
cycle with volumetric clouds and their shadows.

## The character

![The box figure mid-stride](screenshots/character-walk.webp)

An animated box figure in a window, client only: idle, walk, run and jump as a small state machine over blended sequence
threads, with an orbit camera.

**Controls:** W A S D walk, Shift sprint, Ctrl or C crouch, Z lie down or get up, Space jump, F pistol, mouse or Q / E
orbit, wheel zoom, Esc quit.

**Run it:** `just examples/character` (`cargo run -p torque-example-character --release`); its pictures:
`just examples/character-shots`.

**Shows:** DTS shapes made from code, sequence threads blended by script, action maps.

## The asset kit

![The box figure holding its pistol, from the kit's review pictures](screenshots/assets-figure.webp)

The shapes and textures every example uses, made by code (`assets/src/`): a cube, the box figure with its sequences, a
pistol and its projectile, the buggy and its wheel, the hills and the valley as terrains, low-poly trees, the arena's
interiors, and their textures. Nothing generated is kept in the repository: each example makes the kit when it starts.

**Run it:** `just examples/assets <dir>` writes the kit's files (`cargo run -p torque-example-assets -- <dir>`);
`just examples/kit-shots` draws every shape for review.

**Shows:** the shape builder (`torque-shape-builder`), the `.dts` writer, the interior builder (`.dif` from brushes) and
the legacy terrain format.

## The minimal server and client

A dedicated server of the bundled game and a client that connects to it and prints the ghosts it is sent, both in the
console. `just examples/min-server` (`cargo run -p torque-example --bin min-server`) and `just examples/min-client`
(`cargo run -p torque-example --bin min-client`); `just examples/headless` runs the umbrella crate's headless examples.

## The pictures

`just examples/showcase <dir>` draws the four pictures at the top of the repository's README (each the world alone at the
enhanced renderer's highest quality) and sets them in a 2 x 2 grid (`hero.png`, `hero@2x.png`); `--examples` draws the
other pictures on this page; `--only <name>` draws one; `--size WxH` sets their size. The WebP files here were made from
those PNGs.
