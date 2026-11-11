# Examples

Small games made with the engine. Run each from the repository root with `just examples <example> <recipe>`. The web builds need rustup's `wasm32-unknown-unknown` target and Node.

<table>
<tr>
<td align="center"><a href="#the-level-demo"><img src="screenshots/level-demo-plaza.webp" width="200" alt="The level demo"><br>The level demo</a></td>
<td align="center"><a href="#the-level-demo"><img src="screenshots/level-demo-office.webp" width="200" alt="The office"><br>The office</a></td>
<td align="center"><a href="#the-valley"><img src="screenshots/level-demo-valley.webp" width="200" alt="The valley"><br>The valley</a></td>
<td align="center"><a href="#the-arena-shooter"><img src="screenshots/fps-shot.webp" width="200" alt="The arena shooter"><br>The arena shooter</a></td>
</tr>
<tr>
<td align="center"><a href="#the-vehicle-demo"><img src="screenshots/vehicle-demo-turn.webp" width="200" alt="The vehicle demo"><br>The vehicle demo</a></td>
<td align="center"><a href="#the-character"><img src="screenshots/character-walk.webp" width="200" alt="The character"><br>The character</a></td>
<td align="center"><a href="#the-asset-kit"><img src="screenshots/assets-figure.webp" width="200" alt="The asset kit"><br>The asset kit</a></td>
<td align="center"><a href="#the-prop-catalog"><img src="screenshots/level-demo-catalog.webp" width="200" alt="The prop catalog"><br>The prop catalog</a></td>
</tr>
</table>

## The level demo

![The level demo's plaza from above](screenshots/level-demo-plaza.webp)
![The level demo's office: desks under their ceiling lights](screenshots/level-demo-office.webp)

Pick a map from the menu, then walk and jump around it as the box figure. The office has props you can push, pick up and carry, and others can join you there: Host a match or Join a match from the menu, or open it in a browser.

```text
just examples level-demo run
just examples level-demo web           # http://127.0.0.1:8080/?map=office
just examples level-demo office-server # host the office for others
just examples level-demo office-web    # browsers join at http://127.0.0.1:28000/?join
```

**Controls:** 1 to 9 pick a map; W A S D walk, Shift sprint, C crouch, Z lie down, Space jump, mouse look, left click pick up or drop a prop, Q the prop catalog, Tab first or third person, R restart, Esc menu.

### The valley

![The valley's sea at sunset, with clouds reflected in the water](screenshots/level-demo-valley.webp)

Map 6 of the level demo: a lake in a valley that opens out to the sea, at sunset, with trees and fast clouds. In the browser, open `?map=valley`.

## The prop catalog

![The prop catalog open over the office: a grid of props with their pictures, a label field, and the recent spawns](screenshots/level-demo-catalog.webp)

A menu the office's server sends to its players, with a picture for each prop. Clicking a card puts that prop down in front of you. It plays a short session with two headless clients. It needs the WASI SDK (`WASI_SDK_PATH`, see the [Lua chapter](../book/src/lua.md)).

```text
just examples level-demo catalog
```

## The arena shooter

![A shot in the arena: the bolt in flight, the muzzle's flare, the other player hit](screenshots/fps-shot.webp)

An arena shooter for a few players, on a LAN, the internet, or in browsers. Host or join from the menu.

```text
just examples fps client
just examples fps web             # browsers join at http://127.0.0.1:28000/
```

**Controls:** W A S D walk, Shift sprint, C crouch, Space jump, mouse look, left button shoot, right button aim, R reload, Tab first or third person, T chat, Esc menu.

## The vehicle demo

![The buggy in a turn of the race track, dust and tire tracks behind it](screenshots/vehicle-demo-turn.webp)

Walk up to a dune buggy, click to get in, and drive a race track over hills and through forests. Play alone, host, or join.

```text
just examples vehicle-demo run
just examples vehicle-demo web    # http://127.0.0.1:8080/
```

**Controls:** on foot, W A S D walk, Space jump, click the buggy to get in. Driving: W throttle, S reverse, A D steer, Space brake, F headlight, E get out. Tab changes the camera, T chats, Esc opens the menu.

## The character

![The box figure mid-stride](screenshots/character-walk.webp)

An animated box figure in a window, with an orbit camera.

```text
just examples character run
```

**Controls:** W A S D walk, Shift sprint, Space jump, F pistol, mouse or Q / E orbit, wheel zoom, Esc quit.

## The asset kit

![The box figure holding its pistol, from the kit's review pictures](screenshots/assets-figure.webp)

The shapes and textures every example uses, made by code. Nothing is stored in the repository.

```text
just examples assets write <dir>
```

## The minimal examples

A dedicated server and a console client that prints the objects the server sends it. `minimal` wires it by hand on the lower crates; `minimal-engine` builds the same game on `Engine` and `DefaultPlugins`.

```text
just examples minimal server              # or: minimal-engine server
just examples minimal client              # or: minimal-engine client
just examples minimal-engine window       # the client in a window
```

## The mods demo

A small soccer match whose game mode is a Lua mod. The server sends the mod to players who join. See [`mods-demo/README.md`](mods-demo/README.md).

```text
just examples mods-demo server
just examples mods-demo client    # or: just examples mods-demo play
```

**Controls:** W A S D run, Esc menu. The server needs a C++ compiler the first time (it builds the Lua extension).

## The native extension

A plugin built apart from the engine, loaded at run time.

```text
just examples native-extension build
```

## The format viewer

Draws pictures of Source engine content from a game folder you already have. It opens no window.

```text
just examples format-viewer run <command> ...
```

Set `TORQUE_SOURCE_GAME_DIR` to the game folder first.
