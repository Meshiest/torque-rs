# The mods demo

A game mode written in Lua and shipped as a mod that the server requires: a small soccer match people join from a window or a
browser, and see the mod download before they play. It also runs headless (a server and two client bots over the memory
transport). It puts the guide's [Mods](../../book/src/mods.md), [Lua](../../book/src/lua.md), [Native
extensions](../../book/src/native-extensions.md) and [Networked plugins](../../book/src/networked-plugins.md) chapters to work
together.

## Playing it

| | Recipe | Command |
|---|---|---|
| The server | `just examples mods-demo server` | `cargo run -p torque-example-mods-demo --release --bin mods-demo-server` |
| The window client | `just examples mods-demo client [address]` | `... --bin mods-demo-client -- [address]` |
| A match hosted here, in a window | `just examples mods-demo play` | `... --bin mods-demo-client -- --host` |
| Browsers and desktops in one match | `just examples mods-demo web` | `node web/build.mjs mods-demo`, then `mods-demo-server --transport both` |
| A room service for a desktop host | `just examples mods-demo web-rooms` | `torque-rooms --web-root <target>/web/mods-demo`; the host runs `mods-demo-server --room ws://127.0.0.1:28100` |
| The window client's pictures | `just examples mods-demo client-shots` | `... --bin mods-demo-client -- --shots <dir>` (offscreen, no window) |
| The browser checks | `just examples mods-demo web-check`, `web-rtc-check`, `web-p2p-check` | `node web/soccer-check.mjs <web build> <picture> 120 join\|rtc\|room` |

**The server** (`mods-demo-server`, as the arena's `fps-server`) listens on UDP 127.0.0.1:28000; `--bind <LAN address>` lets
others on the network join, `--bind 0.0.0.0` (`just examples mods-demo server-public`) players outside it too. It answers the
network's list of games. `--transport both` adds the web on the same port (the page and the browsers' game sockets),
`--webrtc` a WebRTC data channel beside each browser's socket, `--room <ws://...>` hosts through a room service and prints
the room's code. It requires the soccer mod and sends it to a player who lacks it; it needs the Lua extension (`just lua`).

**Bots.** The game mode has bots of its own (`lib/bots.lua`, `AIPlayer`s): they take the places nobody has taken, at most
`--bots` of them (two by default, so a server nobody has joined plays a match on its own and one person always has someone to
play), and the newest leaves when a person joins. They run a little slower than a person.

**The window client** (`mods-demo-client`) opens on its menu: Host a match (LAN only or public; the server runs in the same
process, with its Lua), Join a match (the network's matches, with their players and bots, or an address typed in), Quit. A
join shows the **download screen**: the server, the mod it plays (`soccer 1.0.0`), its files and size, a bar as it comes in,
then "Joining the match"; a join with the mod already in the cache says there is nothing to download. Mods are kept in the
per-user cache (`--cache <dir>` names another). **Controls:** W A S D run; Esc opens the menu (back to the
match, or leave it).

**The camera** is fixed beside the pitch, the whole pitch in view, the player's team attacking to the right (the camera
crosses to the other side for Red). It shows the ball, both goals and every player at all times, the keys move the player
across the screen, and it needs no mouse capture, so a browser plays it the same way.

**Browsers** join only, as the arena's page does: the server that served the page, over WebSocket with a data channel beside it
when the server offers one, or a desktop host by its room's code (`?room=<code>`). The page shows the same download screen;
the mod is kept for as long as the page is open, so joining again from its menu downloads nothing, and a new page downloads it
again. Hosting from a page is not offered: the server's game mode needs the Lua extension, which a page cannot run as a server.

**The game.** A small soccer match, one player a team: a walled pitch with a goal at each end (white posts and crossbar, a
see-through net, each team's color only on its posts' band and its net's tint), a ball, a score and a clock. The ball is a prop
of the props plugin (`torque-plugin-props`): the players' boxes push it, and a player that comes up to the ball kicks it toward
the goal it attacks. At every kick the server asks each client to play the mod's kick sound where the ball is
(`clientCmdPlaySound`, a 3D sound through the engine's mixer, silent in the headless demo; the client plays only that file, at a
place on the pitch). A ball in a goal scores; after a short pause the players go back to their places, the ball is put back on
the center spot and the team that conceded kicks off. A round ends at three goals or after a minute, and the next starts at 0 -
0. The rules are `game/mods/soccer/server.lua` (with `lib/rules.lua`, which also lays out the pitch); the datablocks it needs,
which Lua cannot declare, are `server.cs` beside it. The game's own scripts (`game/server.cs`, `game/client.cs`) know nothing of
soccer: the server script has empty `onEnterGame` and `onLeaveGame` hooks that the mod replaces, and the client script shows a
board of whatever the server sends: at the top middle the score (the player's own team first, each number in its team's color)
over the clock, with no band behind them, and a goal or the end of a round in large text in the middle of the screen while its
pause lasts.

**Three mods, three rules** (`game/mods`):

| Mod | `mod.toml` | What it shows |
|---|---|---|
| `soccer` | `share = "download"`, `requires_plugins = ["torque.lua"]` | The game mode. Its content (the pitch's floor, walls and goals, the two teams' players, the ball and a kick sound, made from code by `src/assets.rs`) is offered to every player: a client that lacks it downloads, verifies, mounts and caches it before the server runs `onConnect`. The Lua runs on the server only. |
| `big-ball` | `requires = ["soccer >= 1.0"]`, `share = "none"` | Load order: its name sorts before `soccer`, but it requires it, so it loads after it. Its `server.cs` declares a bigger, heavier ball copied from the soccer mod's (`BigBallData : BallData`), and its Lua definition of `soccerBallData` sits over the first mod's, so every kickoff spawns the big ball. Players need nothing from it: the ball's shape is the soccer mod's, drawn larger. |
| `confetti` | content and a script | Disabled by `game/prefs.cs` (`$Pref::Mods::Disabled`): found, never mounted, its script never runs and its content is never offered. |

**What a client does.** It has none of the mods (`client_files()` leaves `mods/` out). On connect the server's offer lists
`soccer`; the client fetches its content over the `torque.mods` channel, checks every block and file against the hashes, mounts
it as untrusted content and tells the server it is ready. The pitch, the players and the ball the server ghosts to it are drawn
from that mount. A second join from the same cache fetches nothing. The server's Lua cannot run on the client: the mod's scripts
are never offered, and a `.lua` file that did arrive would be data (`exec` and `require` refuse it).

**How the headless clients play.** Each client steers its own player with the move globals (`$mvForwardAction` and the others), as held
keys would. A small bot (`src/bot.rs`) sets them every step from what that client sees: its player's position and the ball. With
the ball in the half it attacks (or close in front of it) the bot runs round behind the ball and then through it toward the goal;
with the ball in its own half it keeps goal. Kicks aim at the far post and go a little off either side, so goals happen.

**The pitch's numbers** (its size, the goals', the ball's radius) are `src/assets.rs`'s, which the art and the bots use; the
mod's scripts repeat them, and `the_mods_scripts_agree_with_the_pitchs_numbers` checks they agree.

**The protocol.** Props travel only in the engine's own protocol, so every engine of the demo speaks it whatever
`TORQUE_NET_PROTOCOL` says.

**Run it headless** (the Lua extension is built apart, so it needs a C++ compiler the first time; the headless server has no
bots of its own, its two clients are the players):

| | Recipe | Command |
|---|---|---|
| Play two rounds | `just examples mods-demo run` | `just lua`, then `cargo run -p torque-example-mods-demo --release --bin mods-demo` |
| Pictures of a match (the kickoff, a shot on goal, a goal, the end of the round, a goal close up), from the first client | `just examples mods-demo shots` | `... --bin mods-demo -- --shots <dir>` (offscreen, no window) |
| The tests (the headless match, and the game people play: `tests/multiplayer.rs`) | `just examples mods-demo test` | `cargo test -p torque-example-mods-demo --test demo --test multiplayer -- --include-ignored` |
| Package it for this computer (a folder that runs on its own, with `plugins/`, licenses and a manifest) | `just bundle examples/mods-demo` | `cargo run -p torque-bundle --release -- examples/mods-demo` (`bundle.toml` says what ships; the bundle's check plays a round from its folder) |

The demo needs the extension's library: `just lua` leaves it in the build folder's `release/`, `--lua <library>` or
`TORQUE_LUA_LIBRARY` names another. The tests build it themselves. They are `#[ignore]`d so that `cargo test` needs no C++
compiler; `tests/natives.rs` (the script engines' natives, snapshotted in `tests/natives/`) runs with the rest.

**Files.** `src/lib.rs` (the files and the engines: the mods plugin as a server that requires its mods and as a client with a
cache, the props plugin), `src/lua.rs` (the Lua extension loaded from a `plugins/` folder), `src/server.rs` (the server people
join), `src/game.rs` (the client people play, in a window or a page), `src/web.rs` (the page's client), `src/draw.rs` (a
client's world drawn from its own files), `src/shots.rs` (the window client's pictures), `src/session.rs` (the headless server
and its clients, stepped by hand), `src/bot.rs` (what steers each headless client's player), `src/viewer.rs` (a headless client
drawn offscreen), `src/assets.rs` (the mods' content), `game/` (scripts and mods: `client.cs` has the menus, the download
screen and the keys, `mods/soccer/lib/bots.lua` the server's bots), `tests/demo.rs`, `tests/multiplayer.rs`,
`web/soccer-check.mjs` (the browser checks).
