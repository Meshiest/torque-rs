# Games in a browser

A game made on `torque::app` runs in a browser on WebGPU: `node web/build.mjs <example>` builds it for the web (the module,
its page and the game's files), and `just examples level-demo web` or `just examples vehicle-demo web` builds and serves one
on `http://127.0.0.1:8080/`. Every web build shares one page (`web/index.html`) and one entry (`torque::app::web_page`): the
page shows each loading stage, and its gear opens the settings (the renderer's quality, the render scale, the frame-rate
display). The game's canvas fills the browser window and follows it. The game draws at the canvas's device pixels under
the render scale (Auto, the default, draws all of them up to a 1920x1080 screen's worth and fewer past it; or 100%, 75%,
50%), and lays its GUI out at that size; the browser stretches the drawing over the canvas. The page tells the game the
scale on its root element's `data-torque-render-scale`, and the game writes the size it draws at on `data-torque-size`.

## Building while you edit

`node web/build.mjs <example> --dev` (or `just examples level-demo web-dev`) is the build for the edit-and-reload loop. It builds
the module in the `web-dev` profile, the dev profile with the third-party crates optimized: no link-time optimization, incremental
compiles, and no wasm-opt, so a change to Rust takes a rebuild of the changed crates and one link. The module is larger and runs
slower than the release one, which stays the default and is what the site is built from. The game's files from the last build
(`game.pack` and `files.json`, or the embedded pack) are kept, so the native release build that packs them is not run again; give
`--repack` after a change to the files, or `--keep-pack` to keep the pack in a release build. The first `--dev` build of a folder
with no pack packs it as the release build does.

The Lua module's build (`web/lua-build.mjs`) keeps its copy of the crates in `<target>/lua-web/repo` between runs and writes a file
only when its bytes changed, so a run with no change compiles nothing.

The page compiles the module while it downloads (`WebAssembly.compileStreaming` over one branch of the body, the progress bar
following the other) and compiles the finished download in a browser without it.

## Storage the game keeps in the browser

A game that downloads mods keeps them in the page's IndexedDB database, so a reload finds them. Its `web_exports!` names a
`prepare` function (`prepare: my_game::web::prepare`) that calls `torque::mods::open_browser_cache().await`; the page awaits
it before `startFetched` (and starts the game even if it fails), because the browser's storage answers later than a call is made.
`open_browser_cache` never fails: any error, and any open and read that takes over five seconds, leaves the cache in memory for the
page's life. The engine's `ModsPlugin` then keeps what it downloads there (`CacheChoice::PerUser`), but only when the game names the
server it joins with `ModsPlugin::cache_server` (`torque::mods::server_name_of_url` of the server's address, or
`server_name_of_room` of the room service's address and the room's code; a code names a host only within one service): a
page's link dials one placeholder address for every server, so without a name the entries of every server would share one key and
a server could see what the others sent. An address a browser might read differently (a backslash, an `@`, a control character, a
host with anything but letters, digits, `-`, `.` and `_`) gets no name, so that page keeps its mods in memory. A browser that gives no storage (some
private windows) and one that refuses a write (a full quota) also leave the cache in memory. It holds at most 128 MiB, all of it in
the page's memory too; what the database holds is checked before it is copied (a name that is not an entry's, a value that is not
bytes or is too large, and a stamp with no entry are deleted). See [Mods](mods.md) for what the cache is keyed by and checked
against.

## The page and the scripts

The page and the game's scripts call each other through the page bridge (`torque::app::page_bridge`, `web/bridge.js`).

From the page, `torque` (also `window.torque`) calls the client's script engine. Each call is a promise, served at the game's
next frame; calls made before the game runs wait for it.

```js
const sum = await torque.call('add', 2, 3);        // a script function's result: "5"
await torque.setVar('$pref::Player::Name', 'Ann');  // a global (the `$` may be left out)
const name = await torque.getVar('$pref::Player::Name');
try {
  await torque.eval('$a = 1;\n$b = 2 +;');
} catch (e) {
  console.log(e.kind, e.line, e.column);            // "syntax" 2 9: nothing of the text ran
}
```

From script, `callPage(name, args...)` calls a handler the page registered and gives what it returned. A name the page did
not register reaches nothing and gives `""`, so a game can emit events whether or not a page listens:

```js
torque.on('mapLoaded', (title) => {
  document.title = title;
});
```

```
function clientCmdWorldStarted(%title)
{
   if (isFunction("callPage"))
      callPage("mapLoaded", %title);
   ...
}
```

The settings panel uses the bridge too: it calls the script function `setRenderQuality("low")` (also open to a game's own
options screen), and the renderer draws the next frame at that quality.

**The caps.** Arguments are strings: at most 16 a call, each at most 4096 bytes. Function, handler and variable names are
identifiers (a variable's parts joined by `::`). An `eval` runs at most 64 KiB of text. A frame serves at most 64 of the
page's calls (the rest wait, at most 256; past that they are refused), and script calls the page at most 32 times a frame.

**What may reach the page.** Script reaches only the handlers the page registered, with strings; there is no way for script to
run the page's own code. `torque.eval` exists only on the page's side: the page's author already controls the page. A remote
server runs script on a client only through the game's `clientCmd` functions, so it reaches the page only where the game wrote
a `clientCmd` that calls `callPage` with a fixed, registered name, as above. Such a function passes what the server sent only
as arguments: it never hands the server's text to `callPage` as a name, and the page never registers a handler that runs what
it is given.

## The website

`just site` (`node web/site.mjs [folder]`) builds the project's website into `<target>/site`: the gallery
(`web/gallery.html`) as its front page, each web example in its own folder (`level-demo/`, `fps/`, `vehicle-demo/`, files
fetched by URL), the guide in `book/` and the API reference in `api/`. Every link is relative, so the folder works under a
project site's path; `just site-serve` serves it on `http://127.0.0.1:8080/torque-rs/`, and `just site-check` loads the
gallery, its links and each card's game in headless Chrome. `.github/workflows/pages.yml` builds it and deploys it to
GitHub Pages on each push to main.

A static host has no game server, so the site's examples are built with `--solo` (`node web/build.mjs <example> --solo`, into
`<target>/web/<example>-solo`): the page's root element carries `data-torque-solo="1"`, and the game is played alone. The
arena runs its server in the page's Web Worker, as a player's host does, with no room service. Both games wait at their
main menu, which offers "Play alone", "Host with a link" and "Join with Link". The arena's "Host with a link" takes the
host into its game with the panel open. The vehicle demo plays alone in the page itself; its "Host with a link" starts
its server in the page's worker then and takes the host into it, on the same page. Neither offers the room service's hosting
or joining there, and both Esc menus offer "Invite players". The native builds and the local web builds keep their
multiplayer.

### Invite links

With no server of ours, two players trade connection codes by hand (`web/invites.js`, `web/codes.js`). "Host with a
link" opens a panel: the player picks the STUN server (Cloudflare, Google, or same network only, which uses none), reads
what that server sees, and presses "Create Invite Offer" once per friend for an invite offer link
(`<page>#join=<code>`). An invite belongs to one peer connection, so each works once. The friend's page shows which
server the invite uses before it contacts it, then makes a reply code for the friend to send back. The host pastes it
into their panel under "A reply came back?", and the data channel opens straight between the two browsers. The
friend's game joins once the channel opens, however long the trade takes. The page never loads again for any of this: a
page opened on an invite starts its game as one played alone, with
the friend's panel over its menu, and switches that game to the host's once the channel opens (and joining with a pasted
link works the same way). Every state of the friend's panel has "Back to the menu", which closes the panel over the
game's own menu; a join that failed or was refused also takes a new invite offer link pasted in. Leaving the host's
game, or the host leaving, brings the page back to its own menu, played alone. On the host's list, an invite that failed,
expired or whose player left has "Dismiss". The invite's code rides in the link's fragment, which a browser never sends
to the server, and the page clears it from the address bar once read. The invite link and the reply code each carry their
sender's IP address, which the panels say beside every copy button.

A code is the SDP cut to what a data channel needs (ICE user fragment and password, the DTLS fingerprint and setup, the
candidates as type, address and port), deflated, in base64url, with a version byte, a kind byte and a CRC-32; about 230
characters with three candidates. The page refuses a code longer than 1024 characters, one that inflates past 2 KiB, a
line out of its form or order, and a STUN server key not in its own fixed list; a code never names a server by address.
The host takes a joiner's candidates by the same rules as a room's joiners (canonical IPv4, no `.local` names, private
addresses only on the same-network server, or with `?lan=1` on a page whose code allows it, never its own). On any server, a host candidate may also be a
browser's mDNS name, strictly a lowercase version 4 UUID and `.local`, handed to the browser's own ICE unresolved: only
the browser resolves it, and only on its own network. So pressing Connect (or adding a reply) also makes the browser
ask its local network, by multicast, for the other player's `.local` name; only a device that registered that random
name answers. That is how two tabs of one browser, or two computers on one
network, reach each other through Cloudflare's or Google's server, whose public address alone needs a router that loops
it back, which most home routers do not. The host counts the joiner by the address its traffic
comes from (or by a stand-in of its invite's own when the browser hides that address, as it does for a peer-reflexive
pair), and hands the open channel to the worker's relay with every cap. With no TURN server, some networks can't connect;
the page says so when the channel does not open in time.

### Room codes

A player who hosts through a room service (`torque-rooms`) is given a code of 16 characters, shown in four groups of four
(`ABCD-EFGH-JKMN-PQRS`). Each character is one of 32 letters and digits that do not read alike, so a code holds 80 bits.
The code is the room's only secret, and it is long enough that guessing it is hopeless whatever the guesser does: with at
most 1,024 rooms live, a guess hits about once in 2^70 tries. So the service needs no stop that refuses every player at
once; its limits are kept per network (an IPv4 /24, an IPv6 /48). A network's join attempts a minute and its misses a
second are counted, and past either of them every join from that network is refused as `busy` until the window passes,
whether its code names a room or not, so the reply never tells a guesser which codes are live. A joiner refused `busy` tries
again by itself, up to three times, waiting 1.5 s, then 3 s, then 6 s. Codes from before the codes were lengthened (eight
characters) no longer read.
