# The mods demo

A small soccer match whose game mode is a Lua mod. The server requires the mod and sends it to players who lack it, and the client shows it downloading before the match starts.

Needs a C++ compiler the first time: each recipe builds the Lua extension first (`just lua`).

## Play

```text
just examples mods-demo play              # host a match and join it in a window
```

Or run a server and join it:

```text
just examples mods-demo server            # --bind <your LAN address> lets others join
just examples mods-demo client [address]  # a window; menu: host, join a listed match, or type an address
```

Browsers and desktops in one match:

```text
just examples mods-demo web               # a browser joins at http://127.0.0.1:28000/
```

A browser keeps the mod in its storage, so a reload downloads nothing.

Open to the internet: `just examples mods-demo server-public` (UDP port 28000).

**Controls:** W A S D run, Esc menu.

Without players, bots take the empty places, so one person always has a match.

## Headless

```text
just examples mods-demo run               # two bot clients play two rounds
```
