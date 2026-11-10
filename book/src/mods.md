# Mods

A **mod** is a folder, or a `.zip` file, of scripts and assets that changes a game without changing its program. Players put mods in
the game's `mods` folder; the game finds them when it starts, puts their files over its own, and runs their scripts. Disabling a mod
takes its scripts out again. You do not write any Rust to make one.

For all of it at once, `examples/mods-demo` is a game mode written in Lua and shipped as a mod that its server requires: clients
download its content, a second mod requires the first, a third is switched off by the prefs, and networked ropes show the round
(see its README; `just examples mods-demo run` plays it headless).

## What a mod looks like

```
mods/
  bigger-bombs/              a folder, or bigger-bombs.zip with the same contents
    mod.toml                 what the mod is (the only required file)
    server.cs                runs on a server when it starts
    client.cs                runs on a client when it starts
    art/bomb.png             a file at the same path as the game's replaces it
    art/spark.glb            a new file the scripts can name
```

A zip may hold the folder's contents directly, or one folder with the contents inside it (a folder zipped as it is): both read
the same. A folder or file whose name starts with `.` or `_` is skipped.

## `mod.toml`

```toml
id = "bigger-bombs"                     # required: letters, digits, _ - . (at most 64)
version = "1.2.0"                       # required: 1 to 4 numbers with dots
name = "Bigger Bombs"                   # shown in lists (default: the id)
description = "Bombs with more bang."
author = "Ann"

requires = ["core >= 1.0", "sounds"]    # mods that must be there; they load first
load_after = ["balance"]                # load after these, if they are there
load_before = ["cosmetics"]             # load before these, if they are there
requires_plugins = ["torque.audio"]     # engine plugins the mod needs (their ids)
client_side = false                     # true: only clients load it
share = "download"                      # what a server asks of players: none, required or download

[scripts]                               # entry scripts for each load stage (see below)
server = "server.cs"
client = ["client.cs", "gui/hud.cs"]

[extra]                                 # your own fields; a game may read them
priority = 10
```

| Key | Meaning |
|---|---|
| `id` | The mod's name everywhere: in lists, in `requires`, in the console functions. Two mods with one id: the first found loads, the other is skipped with a warning. |
| `version` | Compared number by number (`1.10` is newer than `1.9`). |
| `requires` | Ids, each optionally with `>=`, `>`, `<=`, `<` or `=` and a version (`"core >= 1.2"`). A mod whose requirement is missing, or at the wrong version, is skipped with a warning that says which; so are the mods that require it. |
| `load_after`, `load_before` | Order hints between mods that do not need each other. They only count when the other mod is there. |
| `requires_plugins` | Ids of engine plugins (`torque.audio`). A mod that needs a plugin the game does not have is skipped. |
| `client_side` | A mod for clients only (a menu, a skin). A dedicated server does not load it and never asks players to have it. |
| `share` | What a server with this mod asks of players: `none`, `required` or `download` (see "Servers that require mods"). Without it, required when the mod has content. |
| `[scripts]` | A stage name and the script (or list of scripts) it runs, paths inside the mod. Without `[scripts]`, a mod runs `server.cs` on servers and `client.cs` on clients, when it has them. |
| `[extra]` | Free fields (text, numbers, true or false). The engine ignores them. |

A key the engine does not know is a warning, not an error. A `mod.toml` that is not valid, has no `id` or `version`, or
names a bad version skips the mod, and the console says why (`dumpMods()` lists every mod with its reason).

## What a mod's scripts can do

A mod's scripts are TorqueScript (or Lua, when the game has the Lua extension: see [Lua](lua.md)), run like the game's own,
with one difference: every function the script declares goes into a
**package** named `Mod_<id>` with every character of the id that is not a letter, digit or `_` made `_` (`bigger-bombs` is `Mod_bigger_bombs`; two ids that map to one name cannot both load: the one found later, in folder and zip name order, is skipped), and the package is active while the mod is enabled. Only the mod's own files declare into it: a game script the mod `exec`s declares its functions as it always does. That is how a mod
replaces a function of the game: it declares one of the same name, and the game's is still there under `Parent::`.

```
// server.cs of the mod
function Bomb::onExplode(%this) {
    Parent::onExplode(%this);
    echo("bigger bang");
}

datablock ItemData(BigBomb) { shapeFile = "art/spark.glb"; };
```

Everything else a script does (creating objects and datablocks, setting globals) happens once when it runs and stays when the mod
is disabled; what disabling undoes is the package, so the game's functions come back, and the mod's files, which stop being
there. A mod that needs to clean up after itself can hold its changes in functions of its package.

Scripts of a mod run **after the game's own entry script** (`server.cs` or `main.cs`), so everything the game defines is there.
A mod's files are not visible to the entry script either: mods are found and checked when the engine starts, but mounted only once
the entry script has finished, so the game's own files are the ones it runs and the prefs it loads (`$Pref::Mods::Disabled`)
say which mods come on. Until then a mod's status is `pending`.
The game says when each *stage* runs; by default there are two: `server` (on a server, once its entry script has run) and
`client` (on a client). A game may declare more (`loadMods("gui")` from a script runs a stage by hand).

### Where files go

A mod's files are laid over the game's: a mod's `art/bomb.png` is the file the game finds at `art/bomb.png` while the mod is
enabled, replacing the game's. When several mods have the same path, the one loaded last wins. Every file is also at
`mods/<id>/...` (`mods/bigger-bombs/art/bomb.png`), which is where the entry scripts run from, so two mods can both have a
`server.cs`. The entry scripts and `mod.toml` are only there; they never replace a game file.

Compiled scripts (`.dso`) are never read from a mod: ship the source.

## Load order

Mods are loaded in an order that follows these rules:

1. A mod loads after every mod it `requires` and every mod it names in `load_after`, and before every mod that names it in `load_before`.
2. Among mods the rules leave free, the smaller id goes first. The order does not depend on how the files are listed.

Later mods win: their files replace earlier files at the same path, and their package sits above the earlier packages. If the
rules ask for a loop (A requires B, B requires A), the mods in the loop and the ones waiting on them are skipped, and the
console names the loop.

## Switching mods on and off

From the console or a script:

| Function | What it does |
|---|---|
| `getModList()` | Ids of the loaded mods in load order, separated by spaces (`getModList(true)` adds the skipped ones). |
| `getModField(id, field)` | `version`, `name`, `description`, `author`, `status` (`pending`, `enabled`, `disabled`, `skipped`), `reason`, `source`, `package`, `files`, `bytes`, or a field of `[extra]`. |
| `isModEnabled(id)` | 1 when the mod is loaded and on. |
| `disableMod(id)` | Switches the mod off, and every mod that requires it. The mod's package is deactivated and its files stop being there. |
| `enableMod(id)` | (From the entry script on; before that mods are pending and it refuses.) Switches it on: its files come back, and its scripts run for the stages that have already run. The mods it requires must be on. |
| `loadMods(stage)` | Runs the scripts of a stage for every enabled mod, in load order. A stage runs once for a mod. |
| `dumpMods()` | Lists every mod and where it came from. |

The list of disabled mods is kept in the pref `$Pref::Mods::Disabled` (ids separated by spaces), which the game saves with its
other prefs, and a game's `game.toml` can start mods off (`[mods] disable = ["some-mod"]`). The game's prefs file is read by its
entry script, and mods are mounted after it, so a mod the pref disables is never mounted, not even while the entry script runs.

## What servers may send

A server may tell a client which files to have, and send them. What it sends is **content, never script**:

- Anything that could be script (`.cs`, `.dso`, `.gui`, `.mis`, and the names that contain one of them, such as `.ed.cs`) is
  dropped when the files arrive. It cannot be read, listed or run.
- No matter what the files are called, `exec` refuses a file that came from a server. A `.lua` file may arrive as content,
  but it never runs, and Lua's `require` cannot load it either.
- Its pictures, models, sounds and other content are mounted like a mod's files, but marked **untrusted**: a loader that was not
  written for hostile files (its author did not mark it so) is never given them.
- Paths and sizes are held to the same limits as mods (below).

A server therefore cannot make a client run code. Mods the player chose, from their own `mods` folder, are trusted like the
game's own scripts: a script mod can do whatever a script of the game can, so install mods from people you trust.

## Servers that require mods

A server can ask every player to have some of its mods, and can serve a mod's files to a player who lacks them. This is for mods
that add pictures, models, sounds and maps the game's datablocks use: a crate that is a model the player does not have is a crate
nobody sees. It is off until the game turns it on (see "For game makers" below), and then it works like this.

### The short version

1. You connect. Your connect block says only that you can take content and the most you will take. It says nothing about the mods
   you have installed or have downloaded before. (A server can still learn something from how you answer its offer: see "For
   players".)
2. The server accepts you into a **downloading** state and sends its list of required mods. For each: the id, the version and a
   hash of the content, and, for a mod it serves, every file's path, size and hash and the hash of every 64 KiB block of it.
3. You compare. A mod you have installed, or downloaded from this same server before, with the same hash needs nothing. For the
   rest you build the new version from files and blocks of an older copy of the same mod, installed or in your cache from this
   server, that match, and ask the server for only the blocks you still lack, a few at a time.
4. You check every block, every file and the whole mod against the hashes, mount it, keep it in your cache and tell the server you are
   ready. Only then does the server run its `onConnect`, so the datablocks and ghosts it sends never reach you before the content
   they use.

A mod updated in one file sends only the blocks of that file that changed, and a mod you already have sends nothing.

### For modders

Add one line to your `mod.toml`:

```toml
share = "download"      # none, required or download
```

| `share` | What a server with this mod asks of players |
|---|---|
| `none` | Nothing. The mod is the server's own (rules, admin tools). |
| `required` | Players must have the mod's content, exactly as the server has it, to join. The server does not serve it. |
| `download` | As `required`, and the server serves the content to a player who lacks it, when the server allows downloads and the content fits the limits below. |

With no `share` line a mod is `required` when it has content (a file that is not a script) and `none` when it is only scripts. A
mod's **content** is every file that is not TorqueScript or code a loader takes, other than `mod.toml` and the mod's entry
scripts. The server and the player compare the content, not the scripts: a player whose copy has other scripts but the same content
has the same mod. Keep files that change often small, or split them: an update costs the blocks that differ.

TorqueScript (`.cs`, `.dso`, `.gui`, `.mis`, `.ed`) and WebAssembly or native code (`.wasm`, `.wat`, `.dll`, `.so`, `.dylib`) are
never sent: they are left out of the offer. A downloadable mod's `server.cs` runs on the server, as always. **Lua files are
content**: they are sent, checked like the rest, and kept, but a file that came from a server is marked untrusted and nothing
runs it (`exec` refuses it, and a loader asks `Mods::is_untrusted(path)`); only a sandboxed realm made for such files may opt in.

### For server hosts

The server lists the mods it asks for when its mods start, and the list follows `enableMod` and `disableMod`. The script functions
`getRequiredModCount()`, `getRequiredModList()` and `getRequiredModField(id, field)` (`version`, `hash`, `bytes`, `files`,
`download`) read it. A player is held while it checks, fetches and mounts, and then joins as usual. A mod over the limits below
is required but not served: a player who lacks it leaves with a line that says so.

While a player is held, its connection is in `ClientGroup` but your `onConnect` has not run. The held connection sends packets
every tick and gives the whole packet to the transfer (a **burst**), then goes back to the connection's rate. A server holds only
`DownloadLimits::max_clients` players at a time, and `max_clients_per_network` of them from one network (an IPv4 /24, an IPv6
/64): the next is refused with `Server busy, try again.` A player is dropped when it takes less than `min_bytes_per_s` of what
it asked for over `idle_seconds`, or has not finished within `seconds`. A player may only ask for runs of blocks the offer
lists, a few at a time and no more often than it can take them; any other request drops it.

A player's word that it has the mods (`serverCmdTorqueModsReady`) is taken as given: a server cannot check a client's files, so
holding a player only keeps an honest client from joining before its content is mounted. A modified client can join without
the mods, and the server's scripts must not count on a client having them.

Measured through the engine's memory link, 400 KB takes about 11 seconds, 33 KB a second. The same download before the burst, at
Torque's default rates (10 packets of 200 bytes a second, half of each for the channel), ran at under 1 KB a second.

| Limit | Default | Set with |
|---|---|---|
| Bytes a client takes from one server (the bytes sent, not the content that matched) | 32 MiB | `DownloadLimits::total_bytes` |
| One mod's content | 16 MiB | `DownloadLimits::mod_bytes` |
| One file | 8 MiB | `DownloadLimits::file_bytes` |
| Files of the mods a client takes | 2,000 | `DownloadLimits::files` |
| Time to finish | 300 s | `DownloadLimits::seconds` |
| Time without a request | 20 s | `DownloadLimits::idle_seconds` |
| Players held at once | 6 | `DownloadLimits::max_clients` |
| Of those, from one network | 2 | `DownloadLimits::max_clients_per_network` |
| The least rate a held player takes what it asked for at | 1 KiB a second | `DownloadLimits::min_bytes_per_s` |
| The player's cache of downloads | 512 MiB | `DownloadLimits::cache_bytes` |

The client sends its requests as the remote command `TorqueModsGet`, so its connection object must be named `ServerConnection`
(as in every Torque client script), and the server answers on the `torque.mods` bulk channel.

### For players

A mod a server sends is saved in a cache folder of your own (`%LOCALAPPDATA%\torque-rs\mod-content`,
`~/Library/Caches/torque-rs/mod-content` or `~/.cache/torque-rs/mod-content`; on the web it lasts the session). It is outside the
game's folder and outside `plugins/`, and it holds only content. Each entry is named for its hash and for the server it came from
(the address you dialed), and is checked again when it is used: an entry that does not match is deleted. One server holds at
most half the cache: past that, what it sends evicts its own oldest entries first, so a server cannot flush the others'. A
server's offer names each mod once, with an id a mod of your own could have, and the older copies the client builds an update
from are read only up to twice what the offer would download.

What a server can learn from you. The client sends no list of its mods, but the blocks it asks for, and the blocks it does not ask
for, show what it already has. So:

- **Downloaded mods are kept per server.** A server can use a cached entry, or build an update from one, only when the same server
  sent it. A server that offers another server's mod gets a full download from you and learns nothing of where you have been. The
  same mod from two servers is downloaded twice; that is the cost.
- **Installed mods match for every server.** A mod you have installed (a folder or zip in your `mods` folder) is compared against
  every offer, so a server can find out whether you have a given mod installed, and which version, by offering it and seeing that you
  ask for nothing.
- **Entries from an older cache** have no server recorded and count as another server's.
- `$Pref::Mods::ShareCache` (off by default) lets every server use every cached entry. You then download less, and a server can
  tell which mods you have from any server by offering them. Leave it off if that matters to you.

| Pref | Meaning |
|---|---|
| `$Pref::Mods::Download` | `0` turns downloads off: a server that needs a mod you lack ends your join with a line naming the mod, its version and size and saying that downloads are turned off. A mod you have installed or cached still needs nothing. Default 1. |
| `$Pref::Mods::DownloadMegabytes` | The most to take from one server (0: the game's limit). It never raises the game's limit. |
| `$Pref::Mods::CacheMegabytes` | The most downloaded content to keep (0: the game's limit). The oldest entries go first. |
| `$Pref::Mods::ShareCache` | `1` lets a server use mods you downloaded from other servers, so they are not downloaded again. A server can then tell, by what you skip, which mods you have from any server, and so which servers you have joined. Default 0. |

`clearModDownloadCache()` deletes the cache. A download ends, and the connection with it, with the reason, when a block, a file or
the mod does not match its hash (nothing is kept), when the server lists a script, when something is over a limit, or when it takes
too long. A game's script can follow a download:

| Function the game defines | Called with |
|---|---|
| `onModDownloadStarted(%mods, %bytes)` | the mods to fetch and the bytes to receive, when the list arrives |
| `onModDownloadProgress(%id, %received, %total, %index, %count)` | the mod being received, and the bytes received of the whole |
| `onModDownloadDone()` | everything the server requires is mounted |
| `onModDownloadFailed(%reason)` | the download ended; the connection is closed |

`getModDownloadState()` is `idle`, `waiting`, `downloading`, `done` or `failed`, and `getModDownloadProgress()` gives the bytes
received, the bytes to receive, the mods mounted and the mods fetched, as four numbers.

## Safety limits

Folders and zips come from anywhere, so each is read under hard limits. A mod over one is skipped (the others load), with the
reason in the console and in `getModField(id, "reason")`:

| Limit | Default |
|---|---|
| Mods looked at | 64 |
| Files in one mod | 20,000 |
| One file, unpacked | 32 MiB |
| All mods together, held in memory | 512 MiB (a zip counts its own size; a mod's unpacked size may not pass it either) |
| Folders in a path | 16 deep, and 240 bytes |
| A zip file | 64 MiB |
| A zip's unpacked size against its size | 100 to 1 (checked once it unpacks past 4 MiB) |

Paths are checked the same for folders and zips. A mod is refused when a path is empty, absolute (`/x`), has `..`, a drive letter
or `:`, a backslash, a control character or NUL, or a name Windows reserves (`con`, `nul.txt`); when two files differ only in
case; or when a zip uses a compression method or encryption the reader does not handle (store or deflate your files). A damaged or
truncated zip is refused, never half read. Links in a mods folder on the disk are not followed.

## For game makers: changing the rules

Everything above is the default. A game can change how mods are found and loaded without touching the engine, by filling **slots**
when it builds its engine. Each slot holds one rule; a second plugin that fills the same slot fails the build and names both.

```rust
use torque_mods::{EngineMods, ModStages, ModsPlugin, PrereqPolicy};

let mut builder = EngineBuilder::new();
builder
    .mods()
    .layout(MyLayout)                                   // how mods are found and what a mod is
    .order(ByPriority)                                  // the load order
    .prereqs(PrereqPolicy::Error)                       // what a missing requirement does
    .stages(ModStages::new(["boot", "gui", "game"]).server(["boot", "game"]).client(["boot", "gui"]))
    .enable_list(MyEnableList);                         // where on/off is kept
let engine = builder.add_plugins(DefaultPlugins).build()?;
```

| Slot | Trait | Default | Decides |
|---|---|---|---|
| `layout` | `ModLayout` | `ModTomlLayout`: `mod.toml` in the folders and zips of `mods` | which roots are searched (several, in the game's files or on the disk, folders and zips), and how a folder or zip is recognized as a mod and read into a `ModInfo`: id, version, requirements, load hints, required plugins, client-side, entry scripts per stage and free fields. A layout may use a different manifest (JSON, say) or none at all, finding entry scripts by name. |
| `names` | `ModNames` | every name except those starting `.` or `_` | maps a folder's or zip's name to the name and category a layout works with, or skips it. |
| `order` | `ModOrder` | `TopologicalOrder`: requirements first, ties by id | the load order. Order by a list, by a priority field, by name. |
| `prereqs` | `ModPrereqs` | `PrereqPolicy::Skip` | what a missing mod, a wrong version, a missing plugin or a loop does: skip the mod with a warning, fail the start, or ignore it. May answer per problem. |
| `stages` | `ModStages` | `server` and `client` | the names of the load stages, and which run by themselves on a server and on a client after the entry script. A stage not run by itself runs when the game calls `loadMods("name")` or `Mods::load_stage`. |
| `enable_list` | `ModEnableList` | game.toml's `[mods] disable` and `$Pref::Mods::Disabled` | which mods start disabled, and where a change is kept (a script file of `$Mod__name = 1;` lines, say). |
| `share` | `ModShare` | the mod's `share` in `mod.toml`, else required when it has content | what a server asks of players for each of its mods: nothing, the mod's content, or the content and a download. Decide from a list, a game mode or a prefs file. |

Whatever the rules, the limits and path rules above are applied first, below the layout: a layout is handed the files of a
mod that has already passed them, and what it returns is checked (ids, lengths, list sizes) before it is used. A layout cannot
load what the limits refuse. A stage is a name a mod's layout lists scripts under; a script listed under a stage the game did
not declare is skipped with a warning.

### Turning on required mods

Requiring mods is off until the game's plugin says so, because it changes what the game puts on the wire: a client and a server
of the game must both have it (a plain Torque client cannot join such a server, as with any networked plugin), and the plugin
adds the bulk channel `torque.mods` to the connection.

```rust
use torque_mods::{CacheChoice, DownloadLimits, ModsPlugin};

let mods = ModsPlugin::new()
    .required_mods()                                   // the server lists mods, clients check and may download
    .downloads(DownloadLimits { seconds: 600, ..DownloadLimits::default() })
    .cache(CacheChoice::PerUser);                      // where a client keeps what it downloaded (the default)
let engine = EngineBuilder::new().add_plugins(DefaultPlugins.set(mods)).build()?;
```

`allow_downloads(false)` makes a server require mods without ever serving them. The `share` slot (`engine.mods().share(..)`)
replaces the rule that says which mods are required, and which of those are served (the default reads `share` in `mod.toml`). A
game that wants to show progress defines `onModDownloadProgress` and the other hooks above in its client scripts.

`game.toml` has a `[mods]` section for the defaults:

```toml
[mods]
enabled = true            # false turns the mods plugin off
folder = "mods"           # the folder of the game's files to search
disable = ["some-mod"]    # ids that start disabled
```

`ModsPlugin::new().disk(path)` also searches a folder on the player's disk (not on the web, where a game's files come from its
pack or a fetched zip), and `.limits(ModLimits { .. })` changes the caps.
