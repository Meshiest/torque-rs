# Plugins

A torque-rs game is built from plugins. A plugin is one part of the engine or of a game: the server game, the GUI,
the water, your own rules. The engine does not start with a fixed set of parts. You list the plugins you want, the engine
builds them in order, and the result is an `Engine` that makes a server or a client whenever you ask.

This chapter shows how to build an engine, write a plugin, order and switch plugins, and run the result. It ends with
the two minimal examples side by side: the same game, wired by hand and built from plugins.

The types are in `torque::engine` (the `torque-engine` crate). The engine's own plugins are in `torque::app::plugins`,
which needs the `app` feature of the `torque` crate.

## What a plugin is

A plugin is a value that implements the `Plugin` trait. When the engine is built, each plugin's `build` method runs once
and records what the plugin adds. There are five kinds of thing to record:

- **Recipes** are small functions that run on every new script engine, at a named stage. A recipe registers console
  classes and functions, sets globals, and so on. The engine keeps the recipes and replays them each time a server or a
  client is made, so one engine can start a server many times, and each start gets fresh state.
- **Resources** are values shared by everything the engine makes, one per type (a game's files, a configuration).
- **Slots** are values that exactly one plugin may set, such as the game's simulation classes or the runner. A second
  plugin that sets the same slot is a build error.
- **Lists** are values that any plugin may add to, in plugin order.
- **Other plugins.** A plugin may add the plugins it builds on.

A plugin does not own the script engine and does not run while the game does. It only records. That is why one engine
can start a server or a client over and over, and why a dedicated server can replay the server recipes alone.

Every plugin type has a stable id, such as `torque.water` or `my_game::ArenaPlugin`. The id is the plugin's identity:
ordering, duplicate checks and config files all go by it.

## Building an engine

An `EngineBuilder` collects plugins and builds them:

```rust
use torque::app::plugins::DefaultPlugins;
use torque::engine::EngineBuilder;

let engine = EngineBuilder::new()
    .add_plugins(DefaultPlugins)
    .build()?;
```

`add_plugins` takes a plugin, a **plugin group**, or a tuple of those (up to 12, and tuples nest). Everything is added
at the end of the list, in the order you write it:

```rust
EngineBuilder::new().add_plugins((DefaultPlugins, ArenaPlugin, HudPlugin));
```

`build` orders the plugins, runs every plugin's `build`, then runs every plugin's `finish` (a second method, after all
the `build`s, for a plugin that wants to read what the others recorded). It returns the `Engine`, or the first error.

### Groups and `DefaultPlugins`

A group is a named set of plugins added together. `DefaultPlugins` is the engine's own: it wires a server's and a
client's script engine the way a Torque game expects. Here is what it adds, in order.

| Plugin | Id | What it does |
|---|---|---|
| `GameClassesPlugin` | `torque.game-classes` | TGE's game classes as console classes (datablocks, shapes, players, items, vehicles), on the server first of all and on the client |
| `ServerGamePlugin` | `torque.server-game` | the server's simulation: its classes and natives over the scene container |
| `TerrainPlugin` | `torque.terrain` | `TerrainBlock` and its collision on the server |
| `WaterPlugin` | `torque.water` | `WaterBlock`, water the players swim in, on the server |
| `NetHostPlugin` | `torque.net-host` | the server's network host and the passes that drive it from the main loop |
| `GuiPlugin` | `torque.gui` | the GUI's classes and natives on the client |
| `ActionMapPlugin` | `torque.action-map` | the `ActionMap` class and its natives |
| `ScreenPlugin` | `torque.screen` | resolution, full screen and the canvas's natives |
| `ConnPlugin` | `torque.conn` | the client's connection to a server and the ghosts' natives |
| `ServerQueryPlugin` | `torque.server-query` | the server browser's natives |
| `ServerTransportPlugins`, `ClientTransportPlugins` | `torque.transport-udp-server`, ... | the transports a server listens on and a client connects over, a plugin each (see [Transports](#transports)) |
| `AudioPlugin` | `torque.audio` | the `alx` natives, the audio datablock classes (`AudioDescription`, `AudioProfile`) and a mixer of the client's own, moved once per tick; it plays through the null sink and opens no sound card |
| `ClientHostPlugin` | `torque.frame-host` | the client host that runs the frames |
| `RendererSlotPlugin` | `torque.renderer-slot` | the renderer slot each client shares with its world view, and `setRenderer`/`getRenderer` |
| `RendererPlugin` | `torque.renderer` | the engine's renderer, registered on the slot as `default` (the `renderer` feature) |
| `RenderFeaturePlugins` | `torque.render-shadows`, ... | the renderer's features, one plugin each (see [Renderers](#renderers)) |
| `WindowRunnerPlugin` | `torque.window-runner` | the runner of native builds: plays the game in a window |
| `WebPageRunnerPlugin` | `torque.web-page-runner` | the runner of web builds: plays the game on the page's canvas, and brings the page and the scripts calling each other (see [Games in a browser](web-pages.md)) |

Four more plugins are not in the group, because each game picks its own or wants them only sometimes:

- `ClientGamePlugin::new(game)` gives the client its game, the part that draws and predicts the game's objects.
- `SimClassesPlugin::new(classes)` gives the server a game's own subclasses of the simulated classes.
- `ZoomPlugin` adds `setFov`, `setDefaultFov`, `setZoomSpeed` and `$cameraFov`.
- `LivePerfPlugin::new(name, pref)` adds the console function `name([mode])` that sets when the window's frame breakdown prints.

A game that needs only part of this takes the group and turns plugins off, or adds the plugins one by one. A group can
also swap a plugin for a differently configured one of the same type (`GuiPlugin` has a `layout_fonts` option):

```rust
use torque::app::plugins::{DefaultPlugins, GuiPlugin, WaterPlugin};
use torque::engine::PluginGroup;

let engine = EngineBuilder::new()
    .add_plugins(
        DefaultPlugins
            .set(GuiPlugin { layout_fonts: true })
            .disable::<WaterPlugin>(),
    )
    .build()?;
assert!(!engine.is_plugin_added::<WaterPlugin>());
```

You can write a group of your own. It lists its plugins in order, and a game adds it like any plugin:

```rust
use torque::engine::{PluginGroup, PluginGroupBuilder};

/// The weather: a sky, and rain that a game turns on when it wants it.
struct WeatherPlugins;

impl PluginGroup for WeatherPlugins {
    fn build(self) -> PluginGroupBuilder {
        PluginGroupBuilder::new().add(Sky).add(Rain).disable::<Rain>()
    }
}

let engine = EngineBuilder::new()
    .add_plugins(WeatherPlugins)
    .enable::<Rain>()
    .build()?;
```

A group can also hold another group (`add_group`).

## Writing a plugin

Here is a complete plugin. It adds a console function `hello(name)` to every server and keeps its greeting as a resource:

```rust
use torque::engine::{EngineBuilder, Plugin, PluginId, ServerStage};
use torque::script::vm::vm::{Native, Vm};

/// What `hello` answers with, shared by every server the engine starts.
struct Greeting(String);

/// A plugin; its id is `example.hello` (by default it would come from the crate and type names).
#[derive(PluginId)]
#[plugin(id = "example.hello")]
struct HelloPlugin {
    greeting: String,
}

impl Plugin for HelloPlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        engine.insert_resource(Greeting(self.greeting.clone()));
        // A recipe: it runs on every server script engine the engine makes.
        engine.server(ServerStage::Game, |vm, engine| {
            let engine = engine.clone();
            vm.register("hello", 2, 2, "(name) a greeting", move |_vm, call| {
                let greeting = engine.resource::<Greeting>().expect("the hello plugin's greeting");
                let name = String::from_utf8_lossy(&call.argv[1]);
                Native::Str(format!("{}, {name}!", greeting.0).into_bytes())
            });
            Ok(())
        });
    }
}

let engine = EngineBuilder::new()
    .add_plugins(HelloPlugin { greeting: "Hello".into() })
    .build()?;

// What the host does each time it makes a server's script engine.
let mut vm = Vm::new();
for &stage in ServerStage::ALL {
    engine.replay(stage, &mut vm)?;
}
assert_eq!(vm.eval(b"return hello(\"world\");")?, b"Hello, world!");
```

A few things to notice.

**The id.** `#[derive(PluginId)]` gives the type its id. Without the `#[plugin(id = "...")]` line the id is the package
name and the type's path: `my_game::ArenaPlugin` for `ArenaPlugin` in the package `my-game`. An id is ASCII letters,
digits, `_`, `-`, `.` and `:`, and never empty. Pick one and keep it: config files and peers name plugins by it. If the
derive cannot find the engine crate through your manifest, add `#[plugin(crate = torque::engine)]`. A generic type cannot
derive an id, because each instance would share one.

**Options are fields.** A plugin carries its options as fields (`greeting` above). The inputs of one start of a game, such
as its files, sockets and globals, are not plugin state; they belong to the host that makes the server or client.

**Shared state.** The recipe runs for every server, so anything it keeps goes in a resource. A
resource is one value per type, read back with `engine.resource::<T>()` and changed with `engine.resource_mut::<T>()`.
Both lend it out checked at run time, as a `RefCell` does.

**Shorter, with parameters.** The same plugin can declare `hello` in the script registry, as a plain function whose
parameters say what it needs: script arguments (`name: &str`) and things the engine hands over (`Res<Greeting>`). The
argument counts and the usage text come from the signature:

```rust,ignore
fn hello(greeting: Res<Greeting>, name: &str) -> String {
    format!("{}, {name}!", greeting.0)
}

engine.script().function("", "hello", &["name"], "a greeting", hello);
// Its line in `native_listing()`: hello 2..2 hello(name) - a greeting
```

`engine.recipe(stage, f)` takes a recipe written the same way (`fn(vm: VmMut, motd: Res<Motd>)`). The parameters are
`VmMut` (the script engine to change, at most one), `VmRef` (to read, any number, never beside a `VmMut`), `Res<T>` and `ResMut<T>` (resources), `Slot<dyn T>`, `&Engine`, `Local<T>`
(state of the function's own, kept between runs) and, for a method, `This`. A native's script arguments are numbers,
truth values, text, `Option<T>` for one that may be left out, and `Rest` for the rest. Two plugins declaring one name
(compared without case) fail the build, naming both.

**Shortest, with the attribute.** `#[torque::function]` takes the names from the Rust signature (camelCase) and the
description from the doc comment's first line, and `torque::natives![...]` lists the functions for the registry:

```rust,ignore
/// a greeting
#[torque::function]
fn hello(greeting: Res<Greeting>, name: &str) -> String {
    format!("{}, {name}!", greeting.0)
}

engine.script().natives(torque::natives![hello]);
// The same line: hello 2..2 hello(name) - a greeting
```

`#[torque::function(ns = "Pad", name = "setDeadZone")]` sets the namespace and the name, `#[arg(name = "...")]` on a
parameter its name in the usage text, `usage = "..."` keeps a usage text exactly as written. `#[torque::methods(class =
"Player")]` on an impl block makes every `pub fn` a method (`#[torque(skip)]` leaves one out, `#[torque(name = "...")]`
renames it); a method with `&self` or `&mut self` is called on its object's Rust data, and `natives![Type]` lists the
block. A signature the engine cannot run (a parameter type it does not know, two `VmMut`s, an `Option` before a required
argument, `self` in a free function) does not compile.

[Script functions from Rust](script-functions.md) goes through the attributes, the parameters, the conversions, errors, prefs and
classes in full; the [script reference](reference/script/index.md) lists what the engine's own scripts already have.

### Prefs and classes

Two derives declare what a plugin keeps in the script engine, and `engine.script()` registers them in every script
engine of the side before its scripts run.

**Prefs** are typed `$Pref::...` globals. The struct's fields are the prefs: the prefix is the globals' common start, and
a field's global is the prefix, `::` and the field's name in PascalCase (`name = "..."` changes the last part). A default
is required, a literal of the field's type; `range = low..=high` clamps what is read.

```rust,ignore
#[derive(torque::Prefs)]
#[prefs(prefix = "$Pref::Pad")]
struct PadPrefs {
    /// How far the stick must move before it counts.
    #[pref(default = 0.15, range = 0.0..=0.9)]
    dead_zone: f32,
    #[pref(default = true)]
    invert_y: bool,
}

engine.script().prefs::<PadPrefs>();   // $Pref::Pad::DeadZone = 0.15 and $Pref::Pad::InvertY = 1, before any script

let pad = vm.prefs::<PadPrefs>();      // `use torque::engine::VmPrefs`: the current values, DeadZone clamped to 0..0.9
fn rumble(pad: Pref<PadPrefs>) {}      // or as a parameter of a native or a recipe (never beside a `VmMut`)
```

The field types are `i32`, `u32`, `f32`, `bool` and `String`, read by the rules of a native's arguments (`"12abc"` is 12).
Prefs are plain globals, so a game's `export("$Pref::*", "prefs.cs")` saves them, and `engine.export_prefs(&mut vm,
file)` saves exactly the registered ones. A script that sets a pref out of range keeps what it wrote in the global; the
clamp applies when Rust reads it. `engine.prefs()` lists the declarations (name, default, range, doc comment).

**Classes** are console classes with typed fields. `#[class(name, parent)]` gives the class and what it goes below
(`SimObject` when left out); each field is a console field of the Rust type's console type, named in camelCase.

```rust,ignore
#[derive(torque::EngineClass)]
#[class(name = "RopeData", parent = "SimDataBlock", preload = RopeData::check)]
struct RopeData {
    /// How many pieces the rope is cut into.
    #[field(range = 2..=64, default = 8)]
    segments: i32,
    length: f32,
    material: Filename,
}

impl RopeData {
    // Runs for each RopeData datablock once it is registered, with the fields read; an Err deletes the datablock.
    // Only datablocks are preloaded, so a class below SimDataBlock is the one that can have this hook.
    fn check(&self, vm: &mut Vm, id: ObjId) -> Result<(), String> { /* ... */ Ok(()) }
}

engine.script().class::<RopeData>();
let rope = RopeData::read(&vm, id)?;   // `use torque::engine::ScriptClass`: the object's fields as the struct
```

A script writes the fields by the console's rules for the type: `dAtoi` for `i32`, `dAtof` for `f32`, `./` expanded in a
`Filename`, and a write outside `range` is clamped with a warning. The field types are `i32`, `u8`, `u16`, `f32`, `bool`,
`String`, `Filename`, and the arrays `[f32; 3]` (a point), `[f32; 2]`, `[i32; 2]`, `[f32; 4]` (a color) and `[u8; 4]`.
`obj.dump()` prints a `Field Docs:` section for a class whose fields have doc comments: each field's name, its comment and
its range. The console's own classes are not declared this way and dump as they always did. A wrong attribute, a range or
default of the wrong type, and a field type that is not a console type do not compile.

### Recipes by stage

`engine.server(stage, recipe)` and `engine.client(stage, recipe)` record a recipe. A recipe gets the new script engine
(`&mut Vm`) and the `Engine` it came from, and returns `Result<(), String>`. An `Err` stops the replay, and the error
names the plugin and the stage.

A script engine is made in stages. The host replays them in the order below, at the moment the parts a stage needs
exist. Within a stage, recipes run in plugin order, and within a plugin in the order it recorded them. A recipe recorded
in `finish` comes after every one recorded in `build`.

| Server stage | Runs |
|---|---|
| `Classes` | first of all: the game classes' console classes |
| `Prepare` | a game's own preparations that the server game reads when it is installed (its simulation classes) |
| `Game` | the server game: its simulation and natives |
| `World` | the world's objects: terrain, water |
| `Net` | the network host and its passes |

| Client stage | Runs |
|---|---|
| `Classes` | the game classes' console classes, when the client has them |
| `Game` | the client game: what draws and predicts the game's objects |
| `Gui` | the GUI's classes and natives |
| `Input` | action maps and their natives |
| `Screen` | resolution, full screen and the canvas's natives |
| `Conn` | the connection to a server, when the client has one |
| `Query` | the server query (the server browser's natives) |
| `Natives` | the game's own natives and globals, just before its client script runs |
| `Host` | the client host that runs the frames |
| `Window` | once the window exists |

A dedicated server replays only the server stages. A client replays only the client stages. Pick the stage that matches
what you register: a native your game's scripts call belongs at `Game` (server) or `Natives` (client), where the
engine's own natives already exist.

A recipe that needs something an earlier stage made, such as the server game's parts, reads it from the wiring the host
puts on the script engine (`ServerWiring` and `ClientWiring` in `torque::app::plugins`). The engine's own plugins do this.

### Resources, slots and lists

These live in the builder while the engine is built, and in the `Engine` afterward.

```rust
use std::rc::Rc;

use torque::engine::{EngineBuilder, Plugin, PluginId};

/// A trait a game lets one plugin implement.
trait Greeter {
    fn greet(&self, name: &str) -> String;
}

/// A line a plugin adds to the server's welcome text. Any plugin may add some.
struct Welcome(&'static str);

struct Politely;
impl Greeter for Politely {
    fn greet(&self, name: &str) -> String {
        format!("Good day, {name}.")
    }
}

#[derive(PluginId)]
struct Manners;
impl Plugin for Manners {
    fn build(&self, engine: &mut EngineBuilder) {
        // A slot: one plugin sets it. A second setter fails the build.
        engine.set_slot::<dyn Greeter>(Rc::new(Politely));
        // A list: any plugin may push.
        engine.push(Welcome("Mind the water."));
    }
}

#[derive(PluginId)]
struct Rules;
impl Plugin for Rules {
    fn build(&self, engine: &mut EngineBuilder) {
        engine.push(Welcome("No running."));
    }

    fn finish(&self, engine: &mut EngineBuilder) {
        // After every `build`, a plugin can read what the others recorded.
        assert_eq!(engine.list::<Welcome>().len(), 2);
    }
}

let engine = EngineBuilder::new().add_plugins((Manners, Rules)).build()?;
let greeter = engine.slot::<dyn Greeter>().expect("Manners sets it");
assert_eq!(greeter.greet("Ada"), "Good day, Ada.");
assert_eq!(engine.list::<Welcome>().len(), 2);
```

- A **resource** is one value per type. `insert_resource` replaces one stored before; `init_resource` stores
  `T::default()` unless there is one. `resource` and `resource_mut` read it while the engine is built; the built `Engine`
  has `resource` and `resource_mut`, which lend it out checked at run time (`try_resource` and `try_resource_mut` give an
  error naming it instead of a panic).
- A **slot** is one value per type, often a `dyn Trait`, that exactly one plugin sets (`set_slot`, read with `slot`).
  Use a slot when the engine needs a single answer: the runner, the client game, the game's simulation classes.
- A **list** collects values of one type from any number of plugins (`push`, read with `list`), in plugin order. Use a list
  when the answer is "all of them".

### Requiring another plugin

A plugin that needs another says so in `requires`:

```rust
use torque::engine::{PluginType, Requires};

#[derive(Default, PluginId)]
struct Gui;
impl Plugin for Gui {
    fn build(&self, _engine: &mut EngineBuilder) {}
}

#[derive(PluginId)]
struct Menu;
impl Plugin for Menu {
    fn requires(&self, requires: &mut Requires) {
        requires.plugin::<Gui>();
    }
    fn build(&self, _engine: &mut EngineBuilder) {}
}

// `Gui` was not added: it is added as its default, and builds first.
let engine = EngineBuilder::new().add_plugins(Menu).build()?;
assert_eq!(engine.plugins(), [Gui::ID, Menu::ID]);
```

A required plugin always builds first. If the engine has none, `requires.plugin::<P>()` adds `P::default()` (so `P` must
implement `Default`). A plugin that knows another only by id, such as a loaded extension, uses `requires.id("...")`.

The engine's own plugins use this: `TerrainPlugin`, `WaterPlugin` and `NetHostPlugin` require `ServerGamePlugin`, and
`ScreenPlugin` and `ConnPlugin` require `GuiPlugin`.

### Plugins that add plugins

A plugin may add the plugins it builds on from its `build`:

```rust
impl Plugin for ArenaPlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        if !engine.is_plugin_added::<ScoreboardPlugin>() {
            engine.add_plugins(ScoreboardPlugin::default());
        }
        // ...
    }
}
```

The new plugins build at once, before the rest of this `build`, and their recipes sit between the earlier and later ones
of the plugin that added them. A plugin type can be added only once, so check `is_plugin_added` first, or use `requires`,
which adds the plugin only when it is missing.

## Ordering

The order is the same every time you build. It follows three rules:

1. **Add order.** Plugins build in the order they were added. Groups add theirs in the group's order.
2. **Before and after.** In a group, `add_before::<Target>(plugin)` and `add_after::<Target>(plugin)` put a plugin
   before or after another one, wherever that one is in the engine's list. (`add_before_id` and `add_after_id` do the
   same by id.)
3. **Requires.** A required plugin comes before the plugin that requires it.

```rust
PluginGroupBuilder::new()
    .add(GuiPlugin::default())
    .add_before::<GuiPlugin>(SplashPlugin)
```

The engine walks the list in add order, and before it places a plugin it places everything that must come before it. A
constraint that names a plugin that is not in the engine, or is turned off, is ignored.

`Engine::plugins()` lists the plugins in the order they built. Since recipes run in plugin order, this is also the order
of the recipes within a stage.

### Errors and warnings

A build that cannot make sense of the plugins returns a `BuildError`:

| Error | Cause |
|---|---|
| `DuplicatePlugin` | a plugin type was added twice, or two plugin types share one id |
| `OrderCycle` | the ordering rules form a loop; the error lists the plugins around it |
| `DuplicateSlot` | two plugins set one slot; the error names both |

A build that can go on does, and keeps a `Warning` for you to show. They are in `engine.warnings()`:

| Warning | Meaning |
|---|---|
| `UnknownPlugin` | `enable`, `disable` or `set` named a plugin that is not there; skipped |
| `ToggleDuringBuild` | `enable` or `disable` was called while plugins built; refused (toggle before the build) |
| `InvalidId` | a plugin reported an id that is not valid; it was not added |
| `RequiresDisabled` | a plugin was turned off because a plugin it requires is disabled |
| `RequiresMissing` | a plugin was turned off because the id it requires is not in the engine |
| `OrderNotKept` | a plugin added during a build was to come before one that had already built |

Both enums can grow, so match them with a catch-all arm. Every warning prints readably, so a host can write them to its
log:

```rust
for warning in engine.warnings() {
    eprintln!("warning: {warning}");
}
```

## Turning plugins on and off

**From code, by type.** `disable` and `enable` take a plugin type, or a tuple of them (they nest):

```rust
let engine = EngineBuilder::new()
    .add_plugins(DefaultPlugins)
    .disable::<(WaterPlugin, TerrainPlugin)>()
    .build()?;
```

A disabled plugin does not build and its recipes never run. The calls apply when the engine is built, so they may come
before the plugin is added. A group's plugin can start out disabled (`WeatherPlugins` above), and `enable` turns it on.

Turning off a plugin also turns off the plugins that require it, with a `RequiresDisabled` warning each. For example,
`DefaultPlugins.disable::<ServerGamePlugin>()` also turns off the terrain, the water and the network host. A plugin that
was added only as another plugin's default, and that no plugin still on requires, is dropped.

**From a config file, by id.** A config file cannot name a Rust type, so the builder has the same calls by id:
`disable_id` and `enable_id`. A game that lets its players or its `game.toml` (see [the game's config](#the-games-config-gametoml)) switch plugins off reads the ids and
passes them on:

```rust
let mut builder = EngineBuilder::new();
builder.add_plugins(DefaultPlugins);
for id in ["torque.water", "torque.watr"] {
    builder.disable_id(id);
}
let engine = builder.build()?;
assert!(!engine.is_plugin_added_id("torque.water"));
// The misspelled id is skipped, with a warning.
assert!(matches!(&engine.warnings()[0], Warning::UnknownPlugin { id, .. } if id == "torque.watr"));
```

`builder.contains(id)` tells whether an id is in the list, and `builder.ids()` lists the ids added so far (disabled ones
too), in add order. A config reader can use them to report a misspelled id itself, but it does not have to: the
build skips an unknown id with a warning and goes on.

Toggles are made before `build`, in call order. A plugin that does not want another one does not add or require it; a
plugin that tries to toggle during its own `build` gets a `ToggleDuringBuild` warning and nothing changes.

## Renderers

A client draws its world with one renderer at a time, chosen from a slot that holds every renderer its engine's plugins
registered, each under an id. `DefaultPlugins` registers the engine's own renderer, `RendererPlugin`, as `default`. A
client starts on `default` when its engine has it, else on the first renderer registered.

Any plugin can register a renderer. It names an id and a function that makes the renderer on the view's GPU; the renderer
draws the view's scene each frame it is chosen (`WorldRenderer`). A type with its own id (`NamedRenderer`) registers with
one call:

```rust
use torque::app::renderers::{EngineRenderers, NamedRenderer, RenderFrame, WorldRenderer};

struct Sketch { /* pipelines */ }

impl WorldRenderer for Sketch {
    fn draw(&mut self, frame: RenderFrame<'_>) -> Result<(), String> {
        // draw frame.scene through frame.camera into frame.rect of frame.target
        Ok(())
    }
}

impl NamedRenderer for Sketch {
    const ID: &'static str = "my_game.sketch";
    fn new(gpu: &Gpu) -> Result<Sketch, String> { /* ... */ }
}

#[derive(PluginId)]
struct SketchPlugin;

impl Plugin for SketchPlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        engine.add_renderer::<Sketch>();
    }
}
```

The game swaps renderers while it runs. A script calls `setRenderer("my_game.sketch")` (it answers 1 when it swapped
or already drew with that renderer), and `getRenderer()` names the one drawing. From Rust, `RendererSlot::switch` and
`WorldView::set_renderer` do the same. The world view swaps at its next frame; between swaps a frame only compares one
counter. An id no plugin registered is refused, and the error names the ones that are:
`no renderer "vulkan" (registered: default, my_game.sketch)`. A second renderer under an id already registered is left
out, with a warning on the client's console.

A renderer is made on the view's GPU the first time it is chosen. If that fails, `setRenderer` answers 0 with the reason
on the console, the renderer drawing stays, and choosing the id again tries again. A renderer chosen before the view
exists (a script at start-up) is made when the view attaches; if it fails there, the engine's own renderer draws
instead. A made renderer is kept while the client lives, so swapping back is immediate, and the GPU memory it holds is
bounded by the renderers registered.

The engine's renderer is the `renderer` Cargo feature of `torque-app` (on by default). Through the umbrella crate, `app`
has it and `app-core` does not, for a game that draws only with renderers of its own. With no renderer at all the GUI is
drawn alone and the client's console says why.

The engine's renderer's features are plugins too, in the group `RenderFeaturePlugins` (`DefaultPlugins` has it). Each
one turns its switches on, so a game drops a feature by turning its plugin off:

| Plugin | Id | Turns on |
|---|---|---|
| `ShadowsPlugin` | `torque.render-shadows` | the sun's cascaded shadows, the lights' shadows and the contact shadows |
| `AntiAliasingPlugin` | `torque.render-anti-aliasing` | the quality's anti-aliasing (TAA or the FXAA-style filter) |
| `AmbientOcclusionPlugin` | `torque.render-ambient-occlusion` | the ambient occlusion |
| `ReflectionsPlugin` | `torque.render-reflections` | screen-space reflections, the light capture, the planar reflection and the water drawn as water |
| `ClearWaterPlugin` | `torque.render-clear-water` | clear water: absorbing, scattering and foaming where it is shallow |
| `BloomPlugin` | `torque.render-bloom` | the bloom |
| `VolumetricsPlugin` | `torque.render-volumetrics` | the volumetric light |
| `CloudsPlugin` | `torque.render-clouds` | the volumetric clouds (off: the server's flat cloud layers) |
| `TransparencyPlugin` | `torque.render-transparency` | weighted blended transparency (off: sorted blending) |
| `GradingPlugin` | `torque.render-grading` | the tonemap through the grading table |
| `GlowLightsPlugin` | `torque.render-glow-lights` | the glowing surfaces' lights |
| `SurfaceDetailPlugin` | `torque.render-surface-detail` | an extension's fine surface detail |
| `ParticleLightPlugin` | `torque.render-particle-light` | smoke and dust lit by the sun, the sky and the lights |

```rust
use torque::app::plugins::{CloudsPlugin, DefaultPlugins, ReflectionsPlugin};

let engine = EngineBuilder::new()
    .add_plugins(DefaultPlugins.disable::<(CloudsPlugin, ReflectionsPlugin)>())
    .build()?;
```

With every feature plugin on, the renderer draws as it always has. Other renderers on the slot read the same scene and
decide for themselves what to draw of it.

## Transports

A server listens, and a client connects, over a transport picked by id. Each transport is a plugin that registers
itself under an id; the engine's own are in `DefaultPlugins` (and the server's in `MinimalPlugins`), one pair per
transport, a server half and a client half:

| Id | Server plugin | Client plugin | What it carries |
|---|---|---|---|
| `udp` | `UdpServerTransportPlugin` (`torque.transport-udp-server`) | `UdpClientTransportPlugin` (`torque.transport-udp-client`) | UDP datagrams on one socket, the LAN list's pings and queries on the same one (not in a browser) |
| `memory` | `MemoryServerTransportPlugin` (`torque.transport-memory-server`) | `MemoryClientTransportPlugin` (`torque.transport-memory-client`) | an in-process link: a game played alone, tests |
| `web` | `WebServerTransportPlugin` (`torque.transport-web-server`) | `WebClientTransportPlugin` (`torque.transport-web-client`) | WebSocket game sockets, the page served on the same port, a WebRTC data channel beside them (the `web` feature; `webrtc` for the channel) |
| `rooms` | `RoomsServerTransportPlugin` (`torque.transport-rooms-server`) | `RoomsClientTransportPlugin` (`torque.transport-rooms-client`) | a player's host found through a room service by its code (the `p2p` feature; the client half in a browser) |
| `relay` | `RelayServerTransportPlugin` (`torque.transport-relay-server`) | `RelayClientTransportPlugin` (`torque.transport-relay-client`) | a browser host's links, held by its page (the `p2p` feature; the client half in a browser) |

The server picks the transport by the id of the `Listen` it was started with (`udp` for `Listen::Udp`, `memory` for
`Listen::Memory`, `web` for `Listen::Web`, and the id of a `BoundTransport` or of `Listen::custom`). A client's
`ClientNet::Loopback`, `Lan` and `Memory` are `udp` and `memory`; any other transport is used with
`ClientBuilder::connect(id, &target)` (or `GameClient::connect`) on a client made with `ClientNet::External`, where the
target is of the type that transport takes.

A plugin adds a transport of its own with `add_server_transport(id, open)` and `add_client_transport(id, open)`:

```rust,ignore
impl Plugin for ChannelPlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        // The server side: a `Listen::custom("channel", ())` opens it.
        engine.add_server_transport("channel", |_listen| Ok(ServerLink::new(channel_transport())));
        // The client side: `client.connect("channel", &ChannelTarget)`.
        engine.add_client_transport("channel", |_: &ChannelTarget| {
            Ok(ClientLink::new(Box::new(channel_datagrams()), NetPolicy::Loopback))
        });
    }
}
```

A transport turned off is gone: `DefaultPlugins.disable::<UdpServerTransportPlugin>()` makes a server started on
`Listen::Udp` fail with a message that names the id, the plugin to add and the ids that are registered. A second
transport under an id already registered is skipped, with a warning on the console.

`HostingPlugin` (not in `DefaultPlugins`) adds the join screen's natives (`checkJoinAddress` and its kin) on top of the
rooms transport: with the `p2p` feature it adds that transport with it, and is turned off, with a warning, when the
transport is.

## Debug gizmos

`GizmosPlugin` (`torque.gizmos`, not in `DefaultPlugins`) draws lines, boxes, spheres, arrows, crosses and text labels in
world space, for one frame or a number of seconds, for debugging. Add it while you are developing:

```rust
use torque::app::gizmos::GizmosPlugin;
use torque::app::plugins::DefaultPlugins;
use torque::engine::EngineBuilder;

let engine = EngineBuilder::new()
    .add_plugins((DefaultPlugins, GizmosPlugin::default()))
    .build()?;
```

From Rust, each client has a `Gizmos` set (`GameClient::gizmos()`, or `Gizmos::of(vm)`). It is immediate mode: call the
drawing function each time the shape should be drawn, with a `Style` for the color, how long it stays and whether the scene
hides it.

```rust,ignore
use torque::app::gizmos::Style;

let gizmos = client.gizmos().expect("the engine has the gizmos plugin");
let red = Style::new([1.0, 0.0, 0.0, 1.0]);
gizmos.line([0.0, 0.0, 0.0], [0.0, 0.0, 5.0], red);              // this frame
gizmos.sphere([0.0, 0.0, 5.0], 0.5, red.seconds(2.0));           // for two seconds
gizmos.aabb([-1.0, -1.0, 0.0], [1.0, 1.0, 2.0], red.depth(false)); // drawn over everything
gizmos.label([0.0, 0.0, 5.6], "target", Style::default());
```

Scripts get `drawDebugLine`, `drawDebugBox`, `drawDebugOrientedBox`, `drawDebugSphere`, `drawDebugArrow`, `drawDebugCross`,
`drawDebugText` and `clearDebug`. A vector is `"x y z"`, a color `"r g b"` or `"r g b a"` (0 to 1), then optionally the
seconds and a depth switch:

```
drawDebugLine(%from, %to, "1 0 0");               // this frame
drawDebugSphere(%center, 0.5, "1 1 0", 2.5);      // for two and a half seconds
drawDebugBox(%min, %max, "0 1 0", 0, 0);          // this frame, not hidden by the scene
drawDebugText(%pos, "spawn", "1 1 1", 10);
```

The view draws what is held after the world and before the GUI: the lines in one batched draw, hidden where the engine's
renderer drew something nearer (with another renderer on the slot they are drawn over the picture), and the labels with the
GUI's fonts. Each drawn frame ages the shapes; a view drawing offscreen at a fixed step ages them by that step. With nothing
held, a frame does nothing on the GPU, and a client made without the plugin has no gizmos at all.

At most `GizmoSettings::max_lines` line segments (a box is twelve, a sphere ninety-six) and `max_labels` labels are held at
once. A shape that does not fit is dropped whole, and the console says so once for each run of frames that dropped any.

Leave the `gizmos` Cargo feature of `torque-app` off for a shipped build (`torque` has it in `app`; take `app-core` and
`renderer` instead). The plugin then stays, and registers the console functions as ones that do nothing, so the game's
scripts need no change.

## Running an engine

An engine has a runner, a slot that decides what "run" means. Each runner is a plugin that sets it, and the slot holds
one. `DefaultPlugins` has the platform's: `WindowRunnerPlugin` on a computer, `WebPageRunnerPlugin` in a browser. With it,
`run` builds the engine and starts the game:

```rust
use torque::app::plugins::DefaultPlugins;
use torque::app::run::EngineRun;
use torque::engine::EngineBuilder;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    EngineBuilder::new()
        .add_plugins((DefaultPlugins, ExamplePlugin(files()?)))
        .app(move |engine| ExampleGame::new(engine, server))
        .run()?;
    Ok(())
}
```

`run` returns when the game is over. A `RunError` says why it did not run to the end: the engine did not build, no
plugin set a runner, or the runner stopped with a message. `run_exit` is `run` for a program's `main`: it prints the error
to standard error and returns a failing exit code.

```rust
use std::process::ExitCode;

fn main() -> ExitCode {
    EngineBuilder::new().add_plugins(DefaultPlugins).run_exit()
}
```

`EngineRun` (from `torque::app::run`) adds `app`, which names the game: a function that makes your `App` (the client, a
title and what to draw) from the built engine, when a runner that needs one starts.

To run another way, turn the default runner off and add the one you want:

| Plugin | Runs |
|---|---|
| `WindowRunnerPlugin` | the game in a native window |
| `WebPageRunnerPlugin` | the game on the page's canvas, in a browser build |
| `OffscreenRunnerPlugin::new(size, ms)` | the game drawn without a window for `ms` of its clock, with a picture of the last frame at the end if you call `.picture(path)` |
| `DedicatedRunnerPlugin` | a dedicated server with no window and no app |

```rust
EngineBuilder::new()
    .add_plugins((
        DefaultPlugins.disable::<WindowRunnerPlugin>(),
        OffscreenRunnerPlugin::new((960, 540), 2000).picture("shot.png"),
    ))
    .app(move |engine| ExampleGame::new(engine, server))
    .run()?;
```

Two runners in one engine fail the build and the error names both plugins. A `Dedicated` (from `torque::app::run`) holds
what a dedicated server needs: its files, where it listens, the script function that starts the map, and when to stop
(after a number of ticks, or when a `Stop` you hold is requested); give it to the engine with `dedicated(server)`
(`EngineDedicated`). The window, web page and offscreen runners need the game from `app`; a dedicated server does not.

An engine does not need `DefaultPlugins` to run. It needs a plugin that sets the runner slot, and you can write that
plugin yourself (a type that implements `Runner`, set with `set_slot::<dyn Runner>`).

## The game's config: `game.toml`

A prebuilt engine runs a game that is only files: scripts, assets and a small `game.toml` that says how to run it. The
file sits at the top of the game's files, so it comes from a folder, a zip or the web pack alike. Every key is optional,
and a game with no `game.toml` runs as it always did.

```toml
[game]
name = "Minimal, on the engine"
server = "server.cs"      # the script a server runs first (default server.cs)
client = "client.cs"      # the script a client runs first (default main.cs)

[plugins]
disable = ["torque.water", "torque.terrain"]   # plugin ids to turn off
enable = []                                   # plugin ids to turn on

[runner]
id = "torque.dedicated-runner"   # which runner plugin runs the game
```

`GameConfig::load(&files)` reads it from the game's `GameFiles` (a missing file gives the default config), and
`.config(&config)` (`EngineConfig`, from `torque::app::config`) applies it to the builder:

```rust
let files = files()?;
let config = GameConfig::load(&files)?;
let engine = EngineBuilder::new()
    .add_plugins((DefaultPlugins, ExamplePlugin(files)))
    .config(&config)
    .build()?;
```

- **Plugins.** `disable` and `enable` go to `disable_id` and `enable_id`, so an id no plugin has is a warning, as above.
  Call `config` after the plugins are added: that is how it can tell the runners a game has. The warnings are in
  `engine.warnings()`, and a server or a client prints them first on its console.
- **Entry scripts.** A server runs `server` (`GameServer::start_engine`) and `ClientBuilder::exec_entry` runs `client`, in
  place of the fixed `server.cs`. The engine keeps the config as a resource; `server_entry(&engine)` and
  `client_entry(&engine)` read it. A script is a path inside the game's files (no leading `/`, no `..`).
- **Runner.** `[runner] id` names one of the runner plugins in the build. The others the engine ships are turned off,
  as the runner slot holds one. Naming a runner the build does not have is a warning, and the one already set stays.

The file may come from a game folder someone downloaded, so it is read strictly. It is at most 64 KiB and its lists
hold at most 256 ids. A key the engine does not know is a warning (`config.warnings`; they reach the engine's warnings
as `game.toml: the key `plugins.reload` is not known; skipped`). A file that is not valid TOML, or has a key of the
wrong type, is an error that says where: `game.toml:4:11: invalid type: string "torque.water", expected a sequence`.
Neither case panics, and a parser test throws damaged and random files at it to keep it so.

The minimal example does this: `examples/minimal-engine/game/game.toml` turns off the plugins that game never uses
(sound, water, terrain, the game list) and names the client's script, and its `builder` function applies the file for
the server, the client and the window alike.

## The two minimal examples

The guide's [Server and client](server-and-client.md) chapter describes `examples/minimal`, a dedicated server and a
client that connect over the loopback address. It wires everything by hand, on the lower crates. `examples/minimal-engine`
is the same game built the other way: `DefaultPlugins` and one small game plugin make the server, the client and the window.
It reads its scripts and mission from `examples/minimal`.

How much code each one holds (`wc -l`), without the game's own scripts and mission, which both share:

| | `examples/minimal` | `examples/minimal-engine` |
|---|---:|---:|
| Rust in `src/` | 1681 | 310 |
| of which code (no blank or comment lines) | 1386 | 238 |
| scripts of its own | 0 | 82 (`server.cs`, `client.cs`) |
| tests | 151 | 188 |
| server | 706 + 152 + 69 | 40 |
| console client | 345 + 45 | 41 |
| window client | 277 + 20 | 94 + 27 |
| library root | 22 | 108 |

The two are not the same size of program. The hand-wired one is a protocol layer: it keeps its own connection table, event
queues and ghost manager and reads TGE's record codecs, and it leaves the mission phases, datablock transfer and control
objects out. The one on the engine runs the engine's server and client, so it gets all of that, and the only game code is a
plugin and some short scripts.

The server, hand-wired (`examples/minimal/src/bin/min-server.rs`, with `DedicatedServer` behind it):

```rust
let mut server = DedicatedServer::start(&options)?;
server.run(ticks, |server| {
    for note in server.take_notes() {
        match note {
            ServerNote::Console(line) => println!("console: {line}"),
            ServerNote::Connected { conn, from } => println!("client {conn} connected from {from}"),
            ServerNote::Ghosting { conn } => println!("client {conn} is ghosting"),
            ServerNote::Left { conn, reason } => println!("client {conn} left: {reason}"),
        }
    }
});
server.shutdown();
```

The server on the engine (`examples/minimal-engine/src/bin/min-engine-server.rs`): the game is a plugin, the server is a
runner.

```rust
EngineBuilder::new()
    .add_plugins((
        DefaultPlugins.disable::<WindowRunnerPlugin>(),
        DedicatedRunnerPlugin,
        ExamplePlugin(files.clone()),
    ))
    .dedicated(Dedicated { ticks, ..dedicated(&files, listen) })
    .run()?;
```

The client builds the same engine and makes a client from it. The connection is a script call, as in any Torque game:

```rust
let engine = EngineBuilder::new()
    .add_plugins((DefaultPlugins, ExamplePlugin(files()?)))
    .build()?;
let mut client = client(&engine, ClientNet::Loopback)?;
client.call("connectTo", &[&server])?;
client.step(5);
```

The window client is the same engine with its window as the runner. The game is an `App`, and `.app(..)` names it:

```rust
EngineBuilder::new()
    .add_plugins((DefaultPlugins, ExamplePlugin(files()?)))
    .app(move |engine| ExampleGame::new(engine, server))
    .run()?;
```

The game plugin (`examples/minimal-engine/src/lib.rs`) is where the example's own pieces go in. It keeps the game's files
as a resource, for the server and the client to read, and it records one recipe that runs on every client script engine:

```rust
#[derive(PluginId)]
#[plugin(crate = torque::engine)]
pub struct ExamplePlugin(pub GameFiles);

impl Plugin for ExamplePlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        engine.insert_resource(self.0.clone());
        engine.client(ClientStage::Natives, |vm, _| {
            vm.set_global("$Example::Boxes", &MAX_BOXES.to_string());
            Ok(())
        });
    }
}
```

The example's README (`examples/minimal-engine/README.md`) lists the small differences between the two sessions, and its
tests check that the engine's script engines carry every native the hand-wired ones have.

## What's next

Plugins are new, and most of the engine is not plugins yet. The direction:

- **More of the engine as plugins.** Today `DefaultPlugins` wraps the installs that wire the script engines, and each one
  stays where it lives. Other built-in subsystems are candidates to become plugins of the same kind, so that leaving one
  out is a `disable::<..>()` call.
- **Macros for script functions.** Registering a console function takes a name, argument counts, a usage string and a
  closure that unpacks the arguments by hand. A macro could write that from an ordinary Rust function.
- **Extensions.** A plugin can report its own id at run time (`Plugin::id`) and name other plugins only by id
  (`Requires::id`, `add_before_id`), so that an extension loaded at run time can take part in the same ordering, config and
  version checks (`Plugin::net_version`) that built-in plugins do.

None of these has a date. The [roadmap](roadmap.md) lists what is planned.
