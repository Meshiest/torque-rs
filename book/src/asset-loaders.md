# Assets from plugins

A plugin that brings its own kind of file (a word list, a rope's profile, a new model format) adds a **loader** for its file
extensions while the engine is built. Every server and client script engine then has an asset cache that reads those files
through the engine's files (the game folder, its mounted zips and the files scripts wrote, with the resource layer's refusals
and caps), gives each one to its loader once, and keeps the result by path and type. A datablock can name such a file in a
`Filename` field: its preload loads it, and a file that is missing or too big fails the preload with the reason.

TGE's own shapes, textures, interiors and terrains still load the way they always have; this registry is for what plugins add.

## A loader

```rust
use torque::engine::assets::{LoadCx, Spec};
use torque::engine::{EngineBuilder, Plugin, PluginId};

/// A rope's profile: one width a line, from the top down.
pub struct Profile(pub Vec<f32>);

fn load_profile(bytes: &[u8], _cx: &LoadCx<'_>) -> Result<Profile, String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "not UTF-8 text".to_string())?;
    text.lines()
        .map(|line| line.trim().parse().map_err(|_| format!("\"{line}\" is not a width")))
        .collect::<Result<_, _>>()
        .map(Profile)
}

#[derive(PluginId)]
#[plugin(id = "example.rope")]
struct RopePlugin;

impl Plugin for RopePlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        engine.assets().loader(&["profile"], Spec::untrusted(16 * 1024), load_profile);
    }
}
```

`loader::<T>(extensions, spec, load)` takes:

- **extensions**: `"profile"` or `".profile"`, in any case; a file's extension is matched without case. Two plugins may read one
  extension into different types (a load asks for its type), but a second loader of one type for one extension fails the build.
- **a `Spec`**: `max_bytes`, the largest file the loader is ever given (a longer one is not read at all), and `untrusted_ok`,
  whether it is given files from untrusted sources: what a server sent, a mod. `Spec::trusted(n)` is for a loader that was not
  written to face hostile bytes; `Spec::untrusted(n)` for one that was.
- **the loader**: the file's bytes and a `LoadCx` (its normalized path and where it came from) to the asset, or a message. The
  asset is any `'static` type.

## Loading

The cache of a script engine is `engine.assets(&mut vm)` (attached the first time) or `Assets::of(&vm)`:

```rust
let assets = engine.assets(&mut vm);
let profile = assets.load::<Profile>(vm.file_host(), "art/ropes/hemp.profile")?;   // Rc<Profile>
let again = assets.get::<Profile>("Art/Ropes/./hemp.profile");                      // the cached one, read nothing
```

Paths are normalized before they are looked up: `\` reads as `/`, `.` and empty parts go, `..` takes off the part before it,
and case does not matter (as for the files themselves). The registry never panics; what can go wrong is an `AssetError`
(its text escapes control characters in the path and the loader's message, so it prints as one line):

| Error | When |
|---|---|
| `BadPath` | the path is empty, ends in `/`, or climbs out of the game folder |
| `NoLoader` | no loader of the type asked for reads the extension |
| `Missing` | there is no such file |
| `Refused` | the files' rules refuse the path |
| `OverCap` | the file is longer than the loader's `max_bytes`; it was not read |
| `Untrusted` | the file came from an untrusted source and the loader's `Spec` does not take one; it was not read |
| `Parse` | the loader returned a message (or panicked) |

A loader's panic is caught natively and becomes a `Parse` error, but not on the web, where a panic aborts the module:
a loader that takes untrusted files must not panic on any bytes.

A failed load is not cached, so the next load tries again.

**Until the host calls `set_origins`, every path counts as trusted**, so a `Spec::trusted` loader is given any file.
Where a file comes from is the host's to say: `assets.set_origins(|path| ...)` returns `Origin::Trusted` or
`Origin::Untrusted` for a normalized path. A host that mounts mods or the files a server sent must set it.

The cache keeps what it loaded until `assets.forget(path)` or `assets.clear()`: it is bounded only by the files there are,
each at most its loader's `max_bytes` (plus what the loader makes of it). A host whose files keep growing (a server's
downloads, a mod folder) forgets a map's or a server's assets when it leaves it, or clears the cache between them.

## Datablocks that name assets

A `Filename` field of a class declared with `#[derive(EngineClass)]` can name the asset type its file is:

```rust
#[derive(EngineClass)]
#[class(name = "RopeData", parent = "SimDataBlock")]
struct RopeData {
    #[field(range = 2..=64, default = 8)]
    segments: i32,
    /// The rope's widths from the top down.
    #[field(asset = Profile)]
    profile: Filename,
}
```

Each `RopeData` datablock's preload then loads the file its `profile` names into the server's cache (before the class's own
`preload` hook runs, so the hook can `get` it). A file that does not load fails the preload, and the console prints why:

```
datablock RopeData(Hemp) { profile = "art/ropes/hemp.profile"; };    // loads, and Hemp is kept
datablock RopeData(Thin) { profile = "art/ropes/none.profile"; };
// preload failed for Thin: profile: art/ropes/none.profile is missing.
datablock RopeData(Huge) { profile = "art/ropes/huge.profile"; };
// preload failed for Huge: profile: art/ropes/huge.profile is 70000 bytes, over its loader's 16384.
```

A failed datablock is deleted, as TGE does with any failed preload. An empty name names nothing. `#[field(asset = ..)]` on a
field that is not a `Filename` does not compile.

## Hot reload

While developing, an edited file reloads its assets without a restart. In a debug build, or with `$Pref::Dev` true, the client
looks at the change times of the files its cache holds once per drawn frame (at most every 250 ms). A file that changed is read
and loaded again: the new asset replaces the old in the cache, `assets.changed()` lists its path for that frame, and the
renderer's caches are reset (`RenderResets::files_replaced`), so whatever looked a name up does it again. A file whose new
contents do not load keeps its old asset; the console prints the reason, and the file is tried again when it changes again.

```rust
for path in assets.changed() {
    // rebuild whatever was made from the asset at `path`
}
```

A holder of an old `Rc` keeps the old asset until it asks again: `changed()` is the cue. A release build does not look unless
`$Pref::Dev` is true, and a host can turn it on or off whatever the build with `assets.set_hot_reload(Some(on))`. A host with no
window (a dedicated server, a test) calls `assets.poll_in(&vm)` itself, or `assets.poll(files, now_ms)` with its own clock.

## Testing

A loader needs no window and no game folder: an `EngineBuilder` with the plugin, `Assets::new(&engine)`, and `MemFiles` holding
the files (`insert_at` gives a change time, so hot reload is tested by changing it and calling `poll` with a later time).
`torque-engine`'s asset tests load a tiny text format through a plugin, check the cache, the caps, the refusals and a datablock's
preload, and give a loader 20,000 random files under its cap without the registry panicking.
