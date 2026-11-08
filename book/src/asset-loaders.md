# Assets from plugins

A plugin that brings its own kind of file (a word list, a rope's profile, a new model format) adds a **loader** for its file
extensions while the engine is built. Every server and client script engine then has an asset cache that reads those files
through the engine's files (the game folder, its mounted zips and the files scripts wrote, with the resource layer's refusals
and caps), gives each one to its loader once, and keeps the result by path and type. A datablock can name such a file in a
`Filename` field: its preload loads it, and a file that is missing or too big fails the preload with the reason.

TGE's own shapes, textures, interiors and terrains still load the way they always have; the engine's content formats are
loaders on this registry too ([Content formats](#content-formats)), beside those paths.

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

## Content formats

The formats the engine reads are plugins of `torque-content`, each adding its loaders with its cap and trust, and
`ContentFormatPlugins` is the group of them. `DefaultPlugins` and `MinimalPlugins` have the group.

| Plugin | Id | Extensions | Asset | Largest file | Untrusted files | In the group |
|---|---|---|---|---|---|---|
| `GltfFormatPlugin` | `torque.format-gltf` | `glb`, `gltf` | `ImportedShape` | 16 MiB | yes | on |
| `ObjFormatPlugin` | `torque.format-obj` | `obj` | `ImportedShape` | 64 MiB | no | off |
| `BlockbenchFormatPlugin` | `torque.format-blockbench` | `bbmodel` | `ImportedShape` | 64 MiB | no | off |
| `DifFormatPlugin` | `torque.format-dif` | `dif` | `torque_maps::dif::Dif` | 64 MiB | yes | on |
| `TerFormatPlugin` | `torque.format-ter` | `ter` | `torque_maps::ter::Terrain` | 16 MiB | yes | on |
| `MapLayerPlugin` | `torque.format-map-layer` | `dif`, `ter`, `dml` | `MapModel`, `Heightfield`, `SkyList` | the importer's | yes | on |
| `QuakeMapFormatPlugin` | `torque.format-quake-map` | `map` | `BuiltInterior` | 64 MiB | no | off (`map-builder` feature) |

- **Shapes.** An `ImportedShape` is the `.dts` bytes every shape reader takes. A glTF reads exactly as a datablock's glTF
  shape file does (the same bytes); an OBJ and a Blockbench model read as `torque-convert` reads them. An OBJ's `.mtl` files
  and a `.gltf`'s side files are not read (a `.gltf` loads when its buffers are `data:` URIs). Each import is paid from a work
  budget (`MAX_IMPORT_WORK`).
- **Interiors and terrains.** A `.dif` and a `.ter` are the files as data, read as the server reads them; a `.map` is built
  into an interior (`BuiltInterior`: the `.dif` bytes, what was built, the mission objects and the reader's warnings) as
  `torque-convert` builds it.
- **The map layer.** `MapLayerPlugin` reads a format's files through its `torque_mapcore::Importer` into the format-neutral
  types drawing and collision read: a model file as `MapModel` (its surfaces, convex hulls and the files it names), a
  heightfield and a sky list. Its default is Torque's legacy importer; `MapLayerPlugin { importer, extensions }` takes another.
- **Trust.** Only the readers that are hardened against hostile bytes and fuzzed today take untrusted files: glTF and Torque's
  `.dif`, `.ter` and `.dml`. OBJ, Blockbench and `.map` take trusted files only.

```rust
use torque::app::plugins::DefaultPlugins;
use torque::content::{DifFormatPlugin, ImportedShape, ObjFormatPlugin};

let engine = EngineBuilder::new()
    .add_plugins(DefaultPlugins.disable::<DifFormatPlugin>().enable::<ObjFormatPlugin>())
    .build()?;
let shape = engine.assets(&mut vm).load::<ImportedShape>(vm.file_host(), "shapes/rock.obj")?;
```

A `game.toml` does the same by id: `[plugins] disable = ["torque.format-dif"]`, `enable = ["torque.format-obj"]`.

The engine's own loading of datablock shapes, `InteriorInstance` and `TerrainBlock` files does not go through these loaders
yet: it pays each file from the script engine's budget and keeps it in its own cache, and stays as it was. The loaders read the
same bytes into the same results (the crate's tests compare each with the path it stands beside), for a plugin or a game that
loads a file itself, and turning a format off does not change what those paths read.

### Source formats

`torque-format-valve` is a separate crate, in no default group: a game that reads Valve's Source content adds
`ValveFormatPlugins` (or one of its plugins) itself. Every reader is hardened and fuzzed, so each takes untrusted files.

| Plugin | Id | Extensions | Asset | Largest file |
|---|---|---|---|---|
| `VtfFormatPlugin` | `torque.format-vtf` | `vtf` | `Texture` (RGBA8 levels, the sprite sheet) | 128 MiB |
| `VmtFormatPlugin` | `torque.format-vmt` | `vmt` | `vmt::Vmt` | 1 MiB |
| `MdlFormatPlugin` | `torque.format-mdl` | `mdl`, `vvd`, `vtx`, `ani` | `mdl::Mdl`, `vvd::Vvd`, `vtx::Vtx`, `AniFile` | 64 MiB each |
| `BspFormatPlugin` | `torque.format-vbsp` | `bsp` | `bsp::Bsp` | 512 MiB |
| `PcfFormatPlugin` | `torque.format-pcf` | `pcf` | `pcf::Pcf` | 32 MiB |

A model is three files: `load_model` loads them through the registry and makes a `torque-modelcore` model (bones, skinned
meshes, materials with the paths to try, the sequences as animations, the included models' too). A map's `Bsp` reads each
lump when asked; `Bsp::build` makes its drawn surfaces (brush faces, displacement grids). The files come from
`files::SourceFiles`, a file host over a game's VPK archives and folders as its `gameinfo.txt` lists them, with a map's pak
lump mounted first:

```rust
use torque::engine::assets::Assets;
use torque_format_valve::files::SourceFiles;
use torque_format_valve::{ValveFormatPlugins, load_model, model::Choice};

let engine = EngineBuilder::new().add_plugins(ValveFormatPlugins).build()?;
let files = SourceFiles::from_gameinfo(Path::new(game_folder))?;
let assets = Assets::new(&engine);
let model = load_model(&assets, &files, "models/props/crate.mdl", &Choice::default(), &mut |_| true)?;
```

The `format-viewer` example draws textures, models, animated sequences, maps and particle systems from the game folder
`TORQUE_SOURCE_GAME_DIR` names, offscreen.

## Testing

A loader needs no window and no game folder: an `EngineBuilder` with the plugin, `Assets::new(&engine)`, and `MemFiles` holding
the files (`insert_at` gives a change time, so hot reload is tested by changing it and calling `poll` with a later time).
`torque-engine`'s asset tests load a tiny text format through a plugin, check the cache, the caps, the refusals and a datablock's
preload, and give a loader 20,000 random files under its cap without the registry panicking.
