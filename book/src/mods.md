# Mods

A **mod** is a folder, or a `.zip` file, of scripts and assets that changes a game without changing its program. Players put mods in
the game's `mods` folder; the game finds them when it starts, puts their files over its own, and runs their scripts. Disabling a mod
takes its scripts out again. You do not write any Rust to make one.

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
| `[scripts]` | A stage name and the script (or list of scripts) it runs, paths inside the mod. Without `[scripts]`, a mod runs `server.cs` on servers and `client.cs` on clients, when it has them. |
| `[extra]` | Free fields (text, numbers, true or false). The engine ignores them. |

A key the engine does not know is a warning, not an error. A `mod.toml` that is not valid, has no `id` or `version`, or
names a bad version skips the mod, and the console says why (`dumpMods()` lists every mod with its reason).

## What a mod's scripts can do

A mod's scripts are TorqueScript, run like the game's own, with one difference: every function the script declares goes into a
**package** named `Mod_<id>` (`Mod_bigger-bombs`), and the package is active while the mod is enabled. Only the mod's own files declare into it: a game script the mod `exec`s declares its functions as it always does. That is how a mod
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
- No matter what the files are called, `exec` refuses a file that came from a server.
- Its pictures, models, sounds and other content are mounted like a mod's files, but marked **untrusted**: a loader that was not
  written for hostile files (its author did not mark it so) is never given them.
- Paths and sizes are held to the same limits as mods (below).

A server therefore cannot make a client run code. Mods the player chose, from their own `mods` folder, are trusted like the
game's own scripts: a script mod can do whatever a script of the game can, so install mods from people you trust.

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

Whatever the rules, the limits and path rules above are applied first, below the layout: a layout is handed the files of a
mod that has already passed them, and what it returns is checked (ids, lengths, list sizes) before it is used. A layout cannot
load what the limits refuse. A stage is a name a mod's layout lists scripts under; a script listed under a stage the game did
not declare is skipped with a warning.

`game.toml` has a `[mods]` section for the defaults:

```toml
[mods]
enabled = true            # false turns the mods plugin off
folder = "mods"           # the folder of the game's files to search
disable = ["some-mod"]    # ids that start disabled
```

`ModsPlugin::new().disk(path)` also searches a folder on the player's disk (not on the web, where a game's files come from its
pack or a fetched zip), and `.limits(ModLimits { .. })` changes the caps.
