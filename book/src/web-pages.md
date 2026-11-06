# Games in a browser

A game made on `torque::app` runs in a browser on WebGPU: `node web/build.mjs <example>` builds it for the web (the module,
its page and the game's files), and `just examples/web-level-demo` or `just examples/web-vehicle-demo` builds and serves one
on `http://127.0.0.1:8080/`. Every web build shares one page (`web/index.html`) and one entry (`torque::app::web_page`): the
page shows each loading stage, and its gear opens the settings (the renderer's quality, the render scale, the frame-rate
display). The game's canvas fills the browser window and follows it. The game draws at the canvas's device pixels under
the render scale (Auto, the default, draws all of them up to a 1920x1080 screen's worth and fewer past it; or 100%, 75%,
50%), and lays its GUI out at that size; the browser stretches the drawing over the canvas. The page tells the game the
scale on its root element's `data-torque-render-scale`, and the game writes the size it draws at on `data-torque-size`.

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
arena runs its server in the page's Web Worker, as a player's host does, with no room service, and its menu offers "Play
alone", "Host with a link" and "Join with Link". The vehicle demo plays alone in the page itself; its "Host with a link"
opens the page again with `?invites=1`, where its server runs in the worker. Neither offers the room service's hosting
or joining there, and both Esc menus offer "Invite players". The native builds and the local web builds keep their
multiplayer.

### Invite links

With no server of ours, two players trade connection codes by hand (`web/invites.js`, `web/codes.js`). "Host with a
link" opens a panel: the player picks the STUN server (Cloudflare, Google, or same network only, which uses none), reads
what that server sees, and presses "Create Invite Offer" once per friend for an invite offer link
(`<page>#join=<code>`). An invite belongs to one peer connection, so each works once. The friend's page shows which
server the invite uses before it contacts it, then makes a reply link (`<page>#reply=<code>`). The host opens the reply
link in the same browser: that tab hands the reply to the game's tab over a `BroadcastChannel` and closes, and the data
channel opens straight between the two browsers. The reply can also be pasted into the host's panel. The codes ride in
the fragment, which a browser never sends to the server, and the page clears it from the address bar once read. Each
link carries its sender's IP address, which the panels say beside every copy button.

A code is the SDP cut to what a data channel needs (ICE user fragment and password, the DTLS fingerprint and setup, the
candidates as type, address and port), deflated, in base64url, with a version byte, a kind byte and a CRC-32; about 230
characters with three candidates. The page refuses a code longer than 1024 characters, one that inflates past 2 KiB, a
line out of its form or order, and a STUN server key not in its own fixed list; a code never names a server by address.
The host takes a joiner's candidates by the same rules as a room's joiners (canonical IPv4, no `.local` names, private
addresses only on the same-network server or with `?lan=1`, never its own). On the same-network server only, a host
candidate may also be a browser's mDNS name, strictly a lowercase version 4 UUID and `.local`, handed to the browser's
own ICE unresolved. The host counts the joiner by the address its traffic
comes from (or by a stand-in of its invite's own when the browser hides that address, as it does for a peer-reflexive
pair), and hands the open channel to the worker's relay with every cap. With no TURN server, some networks can't connect;
the page says so when the channel does not open in time.
