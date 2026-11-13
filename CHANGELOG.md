# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

The first development version; nothing is published yet. torque-rs follows the engine of TGE and Torque3D (the script
engine, networking, simulation, the GUI and rendering) and is credited where it does. What is listed below is new or
different.

### Added

#### Datablocks

- A datablock edited while the server runs (a field written, or the datablock declared again) refreshes the objects built from
  it at the next tick, where TGE leaves them stale until the mission loads again: one coalesced refresh per user per tick, light
  for a write (the fields an object copied) and whole for a redeclare (`onNewDataBlock` again, script threads kept by sequence
  name and position), for ShapeBase, StaticShape, Item, Player and the vehicles (the rigid body's words with the pose and momenta
  kept, the wheel records, the fittings kept); a game's class registers its own on `EngineClass::datablock_changed`
  (`#[class(refresh = ..)]` on a plugin class; `PropData` refits its props at their exact pose and velocities), and
  `%db.onReloaded()` runs after a refresh. A redeclared `ExplosionData`, `DebrisData`, `ParticleData`, `ParticleEmitterData`,
  `AudioDescription` or `AudioProfile` runs its range checks again (TGE runs them in `onAdd` only), and one the checks refuse
  stays, with the checks' line, instead of being deleted under its users; a Projectile whose datablock is deleted deletes
  itself. The
  book's "Live datablocks" chapter.
- A client refreshes what it built from a datablock when a new record for it arrives mid-game, in place: the predicted
  Player's and vehicles' tuning (position, velocity and state kept to the bit), the tires and springs, a Player's live pose,
  the preloaded shapes and a plugin's copies of a datablock object's fields (the props' numbers), each keyed on a per-datablock
  generation that moves only when the record differs, at most 64 refreshes a frame. A local client over its server leaves the
  server's own datablock objects unwritten. TGE rebuilds nothing on a client once a datablock is received.
- Live datablock rounds: `updateDataBlocks()` sends every connected client the datablocks added or changed since it last got
  them, mid-game and with no wire change in either protocol (each connection remembers the key it was sent per datablock, the
  round sends new datablocks before changed ones so a referrer follows what it names, and the client's answer runs
  `GameConnection::onDataBlocksUpdated(%client, %count)`, never `onDataBlocksDone` again); a client runs `onDataBlocksChanged()`
  after a round when it defines it, caps its datablocks at `$Pref::Client::MaxDataBlocks` under the wire's id range and
  rate-limits bodies after its first download. TGE sends datablocks only at a mission's start.
- A development server (a debug build, or `$Pref::Dev`) reloads a shape whose `.dts` changed on disk and refreshes the objects of
  every datablock on it at the next tick (box, hulls, threads kept), where TGE reads a shape once per mission; a file that no
  longer loads keeps the old shape. Off, nothing reads a file time.
- `reloadDataBlocks(%file [, %removeMissing])`, a development command: runs a script file again, refreshes the objects built
  from its datablocks, sends the changed datablocks to the connected clients in one live round and reports the datablocks the
  file no longer declares (removed only when asked and only while unused); in development the server also watches every
  script file that declared a datablock and reloads the ones that change on disk, keeping every datablock when the new text
  does not compile. In TGE a changed datablock file waits for the next mission load.
- Datablocks are reference counted on the server, where TGE deletes one under its objects and tells nobody: `%db.delete()` with
  users retires the datablock (its users keep working, no new object, `setDataBlock`, datablock field or declaration finds it,
  and it is freed with its last user); `removeDataBlock(%db [, %fallback | "deleteUsers"])` refuses over users and over
  datablocks whose fields hold the id, or moves them to a fallback of the same class, or deletes the users first;
  `%db.getUserCount()`, `%db.getUsers([%max])` and `getDataBlockUsers(%db [, %max])` are bounded queries. Datablock ids are
  never reused inside a mission, a Player keeps its last products when its datablock is gone, and a ghost never carries a dead
  datablock id.
- In the engine's protocol a removed datablock leaves the clients too: the event table has a `DataBlockRemoveEvent`
  (engine framing 2), posted on the ordered queue to every connection that was sent the datablock when the server deletes it
  (a retiring datablock's when its last user is gone), and the client retires the datablock and purges its record and script
  object once no ghost, mounted image or predictor names it, at most 256 retired at once (past that the connection ends), an
  unknown id ignored and counted, a body for a retired id refused. Torque's protocol stays byte for byte: a stock client keeps
  a removed datablock until the mission reloads.
- `ParticleData` has the fields `turbulence`, `turbulenceScale`, `collide`, `bounceElasticity`, `bounceFriction`, `lit`,
  `softDistance`, `flipbookBlend` and `sizeByVelocity`, and `ParticleEmitterData` has `trail` and `budget`; each defaults to
  the behavior TGE has. They travel in a block after TGE's bits on the engine's protocol only (a Torque-protocol client is
  sent TGE's record unchanged; the engine framing is now 3), and a client holds every value to its range as it reads it. A
  `ParticleEmitterNodeData` that sets no `timeMultiple` now reads 1, as in TGE.

#### Mods

- `torque-mods`: servers that require mods (`ModsPlugin::required_mods`). The server sends the mods it requires (id, display name, version,
  package hash and, for a mod it serves, per-file and per-64 KiB-block hashes); the client matches them against what it has
  installed or cached, builds an updated mod from files and blocks of an older copy, asks for only the blocks it lacks, and verifies
  every block, file and the whole mod before it mounts it and keeps it in a per-user cache keyed by hash and by the server it came from, so a
  server cannot probe what the player downloaded elsewhere (`$Pref::Mods::ShareCache` turns that off). The client's connect block
  lists none of its mods. The server holds `onConnect` until the
  client is ready, in a burst (the whole packet, every tick), with caps on held clients, requests, time and idle time. TorqueScript
  is never sent or run; Lua files are kept as untrusted data. The offer and data messages carry a version, and a peer of another one
  is refused with both versions named. `getRequiredMod*`, `getModDownloadState`, `onModDownload*`.
  In a browser the cache is the page's IndexedDB database (`open_browser_cache`, with the same keys, per-server scoping, hash
  check on every read and least-recently-used cap, at most 128 MiB), so a reload finds the mods; it falls back to memory when the
  browser gives no storage or the page does not name the server (`ModsPlugin::cache_server`: a browser's link has one address for
  every server). `web_exports!` takes an optional async `prepare` the page awaits before it starts the game.
- Late content: a required mod's files can change while clients play. `refreshRequiredMods()` re-reads the mods and, like
  `enableMod` and `disableMod`, sends every connected client the new offer on the mods channel (no new message); the client
  fetches what it lacks under what is left of its connection's caps (downloaded, assembled, mounted and base bytes all count
  across offers), keeps what it has by hash, drops the mounts of mods no longer listed, takes a repeated offer once and at most
  64 late offers, and says ready again; the server holds each such client's live datablock rounds until that ready word,
  under the join's time and idle limits, so a datablock that names a file a client lacks is never sent to it. TGE moves content
  only before a mission.

#### Rendering

- A ShapeBase can dissolve away or materialize: `%obj.dissolve(%ms, %out, %edgeColor, %edgeWidth, %emitter, %tint)` on the
  server (with `setDissolveLook`, `clearDissolve` and `getDissolve`) is sent to clients of the engine's protocol in the
  object's cloak group (engine framing 4; Torque's protocol is unchanged and carries none). The enhanced renderer clips the
  object by a noise pattern fixed on the object in the depth prepass, the opaque pass and the shadows, the front glowing in
  HDR, and the front emits the emitter's particles from the surface it crosses, through the GPU particle pool or the CPU's,
  optionally in the surface's colors. Objects that do not dissolve draw with the same shaders as before.

- Screen-space distortion in the enhanced renderer: a shape or a decal whose datablock sets `distortion` (with
  `distortionEdge`, `distortionMap`, `distortionMapScale`, `distortionScroll`, `distortionTint`, `distortionChroma` and
  `distortionOnly`) bends what is behind it, for glass, water, heat haze, force fields and shockwaves; what is in front is not
  bent. Applied after the TAA, skipped at Low, and free in a frame with no distorter in sight. The fields travel in the engine
  protocol only (framing 5); `HdScene::distorters` takes them from code.
- Projected decals in the enhanced renderer at Medium and up: each bullet hole or mark is a box the GPU projects onto the
  opaque frame's depth (the CPU path's clip angle as a fade, its texture coordinates, look, fade and lifetime), in one
  instanced draw a texture, in place of the CPU's triangle clipping. They land on the static geometry and on the objects a
  host marks (`DynamicObject::takes_decals`), never on Players, vehicles or other moving shapes. Up to 4096 are kept by
  default; `$Pref::Decals::MaxProjected` raises or lowers it and is held to 16384. Low keeps TGE's clipped decals and its cap.
  A bending decal's distorter is culled to the view and capped with the shapes' distorters. A frame's decals cover at most
  eight screens of area (the nearest kept). For extensions and hosts: `DynamicObject::takes_decals` (false by default), the
  prepass's `PrepassOut.id` is a `vec2<f32>` (x the object's id, y 1 where decals do not land; extension WGSL that builds one
  sets both) and its normal's `w` says the same (1 or 0.5), and `DecalManager::distorters` takes a `sees` test of the view.
- `torque-app`: `WorldView::release_gpu` drops the enhanced renderer's GPU world and `restore_gpu` makes it again with the
  scene, the settings and the slot's passes as they were, so a game can draw another look for a while and come back to an
  identical picture (`has_gpu`, `gpu_geometry_bytes`).
- `torque-app`: `WorldView::released` and `released_with_extension` make a view that starts released (no `HdWorld`, no warm-up
  thread, the GUI pass made at the first `draw`), for a game that draws the world with its own renderer; `restore_gpu` and
  `adopt_gpu` build its first world. `GameApp::warming` (and `App::warming`) lets a game with its own warm-up worker say when
  the window may save the pipeline cache.
- `torque-render-scene`: `hd::ViewScene` (the scene a world view fills) lives here, free of wgpu, so a crate built on the scene
  types can implement it; `torque_render_hd::ext::ViewScene` re-exports it.
- `torque-app`: `WorldView::adopt_gpu(world)` takes an `HdWorld` built and warmed on a worker thread in place of the view's own,
  as `restore_gpu` does without building a cold one on the window's thread; `window::run_with(game, event_loop)` is `window::run`
  on an event loop the caller built, for a client run off the main thread.
- `torque::app`: `GameApp` gains `type Ext`, the render extension its world view draws with (`WorldView<E>`).
- `torque-render-hd`: passes at the opaque and translucent stages can sample the sun's shadows (`PassCx::sun_shadows`,
  `SUN_SHADOW_WGSL`, a bind group layout and group), with the lookup the forward pass shades with.
- `torque-render-hd`: the HD renderer, beside the fixed-function one that keeps the original look. Cascaded sun shadows
  and contact shadows; a shadow cube for every point and spot light in view, sized by its importance; a physical sky
  with volumetric clouds (`CloudLook`) that water reflects; water with waves, refraction, reflections and sun glint;
  particles lit by the sun, the sky and lights; TAA, ambient occlusion, bloom and automatic exposure; four quality
  levels, Low to Ultra. Optional ground textures that hide tiling (`HdScene::ground_variation`) and a cloud clock of
  their own (`HdScene::cloud_time`). A ghosting check example measures how fast each history settles.
- `torque-render-hd`: global illumination (alpha) at High and Ultra behind `Gi::enabled` (off by default): a voxel clipmap of the
  static triangles about the eye, lit by the sun and the point, spot and area lights with the forward pass's own light terms,
  and cascades of irradiance probes whose rays march those voxels; the forward pass's diffuse ambient comes from the probes in
  place of the light capture's bounce. Moving shapes receive it and cast none.
- `torque-render-hd`: area lights (`HdScene::area_lights`): rectangles and tubes with soft highlights the shape of the
  light, by linearly transformed cosines (after Heitz, Dupuy, Hill and Neubelt 2016) over a GGX table the crate fits itself
  (`torque_render_scene::hd::ltc`), at High and Ultra; Low and Medium draw them as point lights. Their shadows are a point
  light's from their center.
- `torque-render-hd`: particles are stepped and drawn by a GPU pool at Medium and up (`HdFeatures::gpu_particles`, `TORQUE_GPU_PARTICLES=0`
  or `1` forces it off or on); Low keeps the CPU path's quads. The emitters keep TGE's emission and random numbers and hand the pool one
  record per particle born. A flipbook's frames are one atlas texture and the fragment blends a frame into the next with
  `flipbookBlend`; a lit particle finds the sun's visibility once at its center; `softDistance`, `lit` and `sizeByVelocity` are
  honored, and fast thin blended particles (`sizeByVelocity`) are drawn after the TAA. A fill governor holds the summed area the
  sprites cover to a budget of the screen (2, 4, 8 and 16 frames from Low to Ultra), shrinking and thinning them past it.
- `torque-render-hd`: an additive decal is an emitter, as an additive particle is, so the bloom picks it up (a glowing mark
  on a surface).
- `torque-render-hd`: physically based materials for shapes: a `.mat.toml` sidecar beside a texture (one material) or a
  shape (one a material name) names base color, normal (OpenGL convention, a flag for DirectX maps), ORM or separate
  occlusion, roughness and metalness maps, height and emissive maps with their factors, alpha mode and UV scale
  (`torque_modelcore::pbr`). Normal mapping on MikkTSpace tangents, roughness and metalness per texel, the occlusion map
  on the ambient, emission, cutout and blended materials, and parallax occlusion mapping from the height map at Ultra.
  Mips filtered in linear light, with normal maps' lost detail kept as roughness. Draws without a material draw as
  before.
- `torque-render-scene`: `HdDraw` has a new `pbr` field (its material and tangents; `None` for a plain draw).
- `torque-render-hd`: compressed material maps. A material's maps bake into KTX 2.0 files with Zstandard levels (BC7 for
  color and ORM, BC5 for normals, BC4 for height, the whole mip chain as the loader makes it) that the loader takes beside
  the images while they are up to date, or that a sidecar names itself; they go to the GPU as BC textures, or are decoded on
  the CPU on a device without them. OpenEXR height and other single-channel maps load too. Cutout materials keep their
  coverage down the mip chain. KTX 2.0, Zstandard and OpenEXR files are read as untrusted input, bounded from their headers.
- `torque-render-scene`: `Texture` has a new `format` field (`TexelFormat::Rgba8` for every texture but a block-compressed
  material map).
- `torque::app`: interiors as static meshes kept on the GPU and culled there, with their shadows cached, so interiors
  that stay put cost nothing on the CPU each frame.
- `torque-render-core::pipeline_cache`: compiled pipelines kept in a per-user wgpu pipeline cache on Vulkan, keyed by
  adapter and driver and size-capped, so a start no longer compiles every pipeline again.
- `torque-render-scene`: tire tracks (TGE has none) and vehicle dust.
- `torque-render-scene`: the own Player's arms in its first-person view (`ClientWorld::first_person_nodes`), posed by the
  body's animation and kept out of the middle of a level view (`first_person_clear`), where TGE draws none of the body and
  Torque3D draws separate arm shapes.
- `torque-render-hd`, `torque::app`: the view under water (`HdScene::water_zone`, set by `WorldView::set_water` from the
  water's bottom and its `underwater` share): the water's tint and absorption, split at the waterline when the camera
  straddles it.
- `torque-gui`: buttons, popups and slider thumbs drawn as rounded boxes; `GuiExtension::classic_boxes` keeps TGE's
  beveled ones.
- `torque-gui`: smooth scrolling (off; `$Pref::Gui::SmoothScroll`, `setSmoothScroll`): a scroll control's content is at its
  final place at once and its drawing glides there over 150 ms of the simulation clock, from the wheel (a high-resolution
  wheel or touchpad by its share of a notch; `ClientHost::wheel_part`) and from revealing the focused control; a press ends the
  glide. With `$Pref::Gui::RevealFocus` (`setRevealFocus`; off) a control that takes the key focus scrolls its nearest scroll
  control to show it. A profile's `keyActivate` has Return and Space press the focused button. `registerFont(face, path)` registers a `.ttf` of the game's files as a font face (a name the game's fonts
  already have is refused; `FontSource::register_face`; `torque::app::layout_fonts` is now a `FaceCache`). A control's `opacity` dims it and what is in
  it; a profile's `cornerRadius` gives fills and style 1 borders rounded corners and buttons the same radius.
- `torque-fonts`: system fonts on Linux and macOS, with metric-compatible free faces in place of missing ones.
- `torque::app::config`: `game.toml`, a small config in a game's files read by the prebuilt engine: the plugins to turn off or on by id, the runner, and the scripts a server and a client run first (TGE fixes them in the executable). Strict: unknown keys are warnings, a size cap, errors with line and column.
- `torque::app::gizmos`: debug lines, boxes, spheres, arrows, crosses and text labels in world space (`GizmosPlugin`, not in
  `DefaultPlugins`; the `gizmos` feature), from Rust and as console functions (`drawDebugLine` and kin); TGE has none.
- `torque-gui`: GUI control classes written in Rust and added by plugins (`GuiControlClass`, `add_gui_control`): their fields
  are console fields, they draw, lay themselves out and take the mouse and keys by their own class, and any number of
  plugins can each add some; supersedes `GuiExtension::draw_control`, now deprecated. A control class can also run once a
  frame (`update`), place other controls (`ControlCx::set_bounds_of`, `resize_control`) and keep data on any control
  (`Gui::control_data`).
- `torque-gui`: more than one `GuiExtension` on a GUI: a primary one and any number added after it (`gui_primary`,
  `gui_extension`, `Gui::add_extension`), each method joining their answers by a rule; with one extension the GUI asks it directly.
- `torque-plugin-gui-layout`: flex and grid layout containers (`GuiFlexCtrl`, `GuiGridCtrl`; taffy) whose children are any
  controls, read through dynamic layout fields, inside GUIs that keep TGE's absolute positions (`GuiLayoutPlugin`, not in
  `DefaultPlugins`); TGE has only absolute positions and sizing flags. A server's client scripts get them as the `flex` and
  `grid` kinds: the plugin registers a `RealmLayout` (`GuiBuilderExt::add_realm_layout`) naming the two classes and the field of
  each layout prop, so `torque-gui` has no dependency on it, and an engine without the plugin keeps a container as a plain group.
  The format adds the two kinds and twelve item props (`grow`, `margin`, `column`, ...) every control has, with caps on nesting
  (8), children (256), tracks (32) and every number.
- `torque-gui` `client_script`: the GUI side of client scripts, the scripts a server sends to run on the player's client. A
  server's `ui` ops make controls of a closed set of Rust classes (`ClientScriptPanel`, `ClientScriptButton`, ...) through a
  typed applier that checks every op against the op format, the grants and the caps (a violation stops the realm). The
  controls have no names, commands, variables, accelerators or script callbacks, and scripts cannot read or write them. They
  live in a server layer the canvas does not own: drawn after the content and before the dialogs, only over the screens the
  game names, never moved by `setContent`, `pushDialog` or `popLayer`. An engine layer and a chip ("Server scripts on",
  and the address, with a check box for each ability) are drawn over the dialogs and get input first; Escape is the host's, and the realm is
  given text and navigation only in its focused text field. `Gui::client_script_controls` lists what the
  realm's controls show (kind, text, image path, rectangle), read-only and out of reach of scripts, for a game's tests. The consent prompt turns the cursor on while it is up and draws real check boxes; a host that takes input before its action maps asks the Escape filter first (`Gui::client_script_escape`), and a game's own key for a server's menu is told to the engine with `Gui::client_script_player_asked` (Rust only).
- `torque-client`: input sources (`InputSource`, `add_input_source`): devices a plugin adds that the client polls once per
  drawn frame (once per client pass without a window) and that reach the action maps as TGE's `joystick<N>` buttons, axes and hats (`bind(joystick0, xaxis, f)`); a device
  that goes lets go of what it held. TGE reads only DirectInput devices. A source can stamp events (`InputQueue`, `axis_at`) and the
  client then gives each axis as its time-weighted mean over the poll's span.
- `torque-plugin-gamepad`: gamepads as `joystick<N>` (gilrs natively, the browser's Gamepad API on the web; pads plugged in and out
  while the game runs; the sticks, triggers, buttons and cross), with the prefs `$Pref::Gamepad::DeadZone` and
  `$Pref::Gamepad::InvertY`; natively the pads are read on a thread of their own (500 Hz, `$Pref::Gamepad::PollRate`). Not in `DefaultPlugins`. The vehicle demo plays with it (`--features gamepad`, `--gamepad`).
- `torque-plugin-rope`: ropes a client script hangs between two points or from an object (`RopeData`, `ropeCreate` and kin), simulated
  with position-based dynamics in fixed substeps (sagging, swinging, a floor, stiffness, capped, hostile numbers made safe) and drawn as
  lit, fogged tubes that cast a shadow, by an opaque pass and a shadows pass; a datablock's `style` picks `"ribbon"` instead (a flat
  strip that always faces the camera along a smooth curve through the simulated points) or `"line"` (a straight, unsimulated strip
  between the ends, for lasers and tripwires), both depth tested, soft edged, never thinner than two pixels and without a shadow,
  with an `emissive` flag for a glow. Ropes take a `texture` (`.png` or `.jpg` through the asset loaders; arc-length u, a tile every
  `textureLength`, `textureScroll`, `textureFilter`; alpha blended on strips, cut out on tubes), and `ropeSetColor`,
  `ropeSetTexture`, `ropeSetTextureScroll` and `ropeSetEmissive` (and getters) change one rope without a datablock. A rope can be driven
  (`sim = "none"`, `ropeSetCurve`: a Bezier from the start along the aim or a direction, evenly sampled, following both ends and the mounts'
  directions each frame) with a taper (`endThickness`), a gradient (`colorEnd`), `additive` blending and an `emissiveIntensity` above 1,
  for beams; `MuzzleAnchors` and `MuzzleTable` attach an end to a weapon's muzzle as the frame drew it (`GhostScene::muzzles`), the first-person
  weapon's for the player whose view it is. The arena example has it behind `--features rope` (`--beam-shots`). The rope runs per drawn frame (240 substeps a second, at most eight a frame, shown between
  its last two states) instead of on the 32 ms tick, and an end that hangs from a ghost follows where the frame draws it
  (`AnchorResolver::ghost`, `FrameAnchors`, `RopeWorld::step_frame`, `pin_ghost`). Every style can cast (`castShadow`; a ribbon's or line's shadow is a band that faces the sun)
  and take (`receiveShadow`) the sun's shadows, as the renderer's own surfaces do. Not in `DefaultPlugins`. The level demo hangs one across the plaza
  (`--features rope`).
- `torque-plugin-rope` over the network, and `engine.net().on_ghost` (a client's handler for each ghost of a plugin class that appeared,
  changed or went): a server makes ropes every engine client shows (`ropeNetCreate` and kin, a ghost class with a mask bit for each group
  so an update sends only what changed, and `RopeData` as a networked datablock), the points stay on each client; `ropeSetLength`
  reels a rope over a time and a rope held farther apart than it is long is drawn straight (visual only); temporary ropes fade out
  (`ropeSetLifetime`); `ropeNetFx` sends a one-shot rope as a remote command with no ghost; `ropePredict` shows a client's own rope at once
  and the server's ghost adopts it by a bounded per-client id (or it fades out unconfirmed). The book's "Networked plugins" chapter.
- `engine.net().ghost_share`: a plugin ghost class's updates held to a share of each packet (an update past it waits for a later
  packet) and ordered by the plugin's own priority for each connection; TGE's objects and TGE peers are unchanged.
- `engine.net().scope_control`: a plugin keeps an object out of one connection's ghosts, or every connection's, and lets it back in
  (`ScopeControl`, `Audience`). No scope walk makes a ghost of a hidden object (an always-in-scope one and a script's `scopeToClient`
  included), the ghost it had is killed at the next packet, and what the scene hierarchy hangs on it is sent as roots; a server that
  never asks for it, and TGE peers, are unchanged. A hide never blinds a connection (its own control object is not hidden from it), only
  a `GameConnection` is an audience, each audience has its own room of the pool (a refused hide is counted), and a connection looks again
  only when its own hides changed. Each plugin hides under its own name (`hide_as`, `show_as`, `is_hidden_by_other`) and can read a
  log of the changes (`changes_since`), so plugins do not undo one another. A plugin collision source that passes its cap, or a set that does, is told on the console once. `engine.net().stills`: shapes a plugin keeps still without a ghost, drawn by the
  engine's ghost scene as resting ghosts are.
- `torque-plugin-props`: frozen props past `$Pref::Props::FrozenGhosts` per client go to it over the `props-frozen` bulk channel
  instead of as ghosts, with a handover that keeps one picture of a prop at every tick, and only to a client whose scope takes the prop
  and that no other plugin hides it from (a prop leaving the scope is removed, one entering is sent); a server may hold up to 4,096 props
  (collision sources take up to 4,096 shapes each, 8,192 in a set).
- `engine.net().drawn_shape`: a plugin ghost class drawn as a shape (a shape file, scale and skin from its record and datablock) by the
  engine's renderer, lit, shadowed and culled as a StaticShape is and in the frame's list of ghosts that ropes and passes anchor to;
  a class marked `Replicate::Interpolated` is shown between the server's updates (a warp of one to three ticks, as a vehicle's).
- `torque-plugin-rope` offsets and drivers: an end can hang from a point in a ghost's own frame (`pin_ghost_at`, `ropeNetAttachAt`; it
  rotates with the ghost and is at the drawn point every frame), and another plugin can steer a rope from Rust (`RopeWorld::drive`,
  `Drive`, `DriveEnd`, a `RopeDriver` registered by name with `RopePlugin::with_driver` or `RopeDrivers`, `ropeNetDrive` for a
  networked rope): the two ends, the length, whether the rope is at its limit (drawn straight between its ends) and its curve, each
  frame or tick. The rope stays visual. The rope plugin's network version is 2.
- `torque::app::camera_modes`: camera modes (`CameraMode`, `add_camera_mode`) that plugins add and the client picks by name
  (`setCameraMode`, `getCameraMode`, `getCameraModes`; `CameraModes::set` from Rust), eased from one to the next, with the first
  person, the third person on a boom and the orbit built in; `torque::app::camera_behind` is the helpers of the camera behind a
  figure, moved from the examples' shared crate. TGE has one camera at a time and no modes.
- `torque::app::render`, `torque-render-hd::passes`: rendering from plugins (`engine.render()`): passes at named stages
  of the HD renderer's frame (`Stage::{Shadows, Prepass, Opaque, Translucent, AfterTaa, Post, Overlay}`, `HdPass` handed a
  `PassCx` with the device, the encoder, the stage's targets, the camera and the scene), static batches of many copies of a
  mesh kept on the GPU in chunks and culled there (`StaticBatchSource`, `WorldView::set_static_batch`), and particle
  emitter kinds drawn with TGE's particles (`EmitterKind`). A stage with no pass costs nothing.
- `torque-engine::assets`: asset loaders from plugins (`engine.assets().loader::<T>(extensions, Spec { max_bytes,
  untrusted_ok }, load)`) and a typed cache by normalized path for each script engine (`Assets`), reading through the
  resource layer; a missing, refused, over-cap, untrusted or unreadable file is an `AssetError`, never a panic. A class's
  `Filename` field can name an asset type (`#[field(asset = T)]`) that each datablock's preload loads, failing the preload
  with the reason. Hot reload in debug builds or with `$Pref::Dev`: changed files load again once per drawn frame,
  `Assets::changed` lists them and the renderer's caches reset. TGE's own shape and texture loading is unchanged.
  A path is trusted only when a classifier says so and none says otherwise: layers add classifiers that answer `Some(origin)` for
  their own paths and `None` for the rest (`Assets::add_origins`), a host declares its own folders (`Assets::trust_folder`), a
  path nobody speaks for is untrusted, and an asset cached before a classifier was added is checked again.

#### Web and networking

- `torque-plugin-tcp`: TGE's `TCPObject` and `HTTPObject` (`TcpPlugin`, not in `DefaultPlugins`; nothing in web builds). Lookups,
  connects and TLS handshakes never block a tick, and each script engine is capped (`TcpLimits`: connections, listen sockets,
  accepted connections waiting, connects under way, bytes read and sent per tick, send queue, line length, callbacks per tick);
  a refused connect reports
  `onConnectFailed`, a peer that sends an over-long line is disconnected. HTTPS for `https://` addresses through rustls, and
  `post` sends a real POST.
- `torque-transfer` and `NetRegistry::upload`: files a client sends a server, taken by a plugin's sink under limits the server
  sets (size, quota per player, uploads at once, staged bytes, rate, a permission). The server pulls the file in 64 KiB blocks,
  asks for each run and checks every block and the whole content against hashes before the sink sees it as read-only untrusted
  bytes; data nobody asked for or a bad block drops the connection. Larger uploads stage in a server-private folder, and a cut-off
  upload resumes with only the missing blocks. The mods download shares the block hashing.
- `torque-client`: the client's prediction is a slot (`predict::slot`): one predictor at a time sees the server's heads, the ticks
  and the views and decides what is drawn; TGE's scheme is the default and is unchanged. Plugins register predictors with
  `engine.net().prediction` and `add_prediction`, and scripts swap with `setPredictor`. The client's vehicles step with the body
  solver the game picks per datablock (`PredictGame::body_solver`).
  Plugins also add prediction parts (`prediction_part`) that run after whichever predictor is chosen, to predict objects of
  their own and draw ghosts where they put them.
- `torque-plugin-smooth-prediction`: a predictor that corrects as TGE's does but eases the correction out of what is drawn (body,
  vehicle, eye and camera), with a snap distance and per-class opt-in, sending TGE's bytes.
- Web builds on WebAssembly and WebGPU: a game folder packed as one block or fetched from a file list (`files.json`,
  with sizes and SHA-256 hashes, checked on arrival), the page's one thread and the browser's clock and fonts, a render
  scale for the canvas, and a bridge between the page and the game's scripts (`torque::app::page_bridge`). The
  workspace's `web` build profile.
- `torque-server`'s `web` feature: browsers join over WebSocket, served the game's page over HTTP or TLS (rustls) on the
  same port, beside UDP players in the same game.
- `torque-server`'s and `torque-client`'s `webrtc` features: WebRTC data channels (str0m) carry a browser's datagrams,
  falling back on the WebSocket when the channel fails.
- `torque-rooms`: a room service for player-hosted games. A host gets a room code; the service carries only the WebRTC
  signaling and hands out STUN and TURN credentials. The `p2p` features let a player host a game with no port forwarded
  (`net::web::p2p::P2pHost`) and others join it through a room (`net::room_join`).
- Hardening against hostile input across the web stack: caps on requests, headers, frames, messages, bytes and
  connections (in all, per address and per second), timeouts, `Host` and origin checks, ICE checks and candidates
  filtered by source, and peers given stand-in addresses so per-address limits and bans hold.
- Bulk channels for plugins (`torque::net::chunk`, `torque::server::net::channels`): large, mostly static data (a map's static
  data, say) goes from the server to each client in chunks under a per-packet byte budget, reliable
  and ordered per channel, with a snapshot and deltas (`chunk::patch`), what each client gets chosen per client, caps on
  message size and backlog that refuse cleanly, and a fuzzed reader. They are turned on when the server's gate matches the
  channels in the client's manifest (the accept says so); TGE's packets are unchanged for a connection without channels.
- Client-to-server channels: the same chunks go from a client to the server in a section at the front of its data packets, under a
  budget that leaves its moves and events their room, on the connections where channels are on and the manifest lists the channel
  (`up:` names, so a peer without it is refused). The server reads every byte as hostile, with caps on held bytes (per connection
  and in all), message size, messages in flight and rate, and drops a connection that passes one; `NetRegistry::channel_to_server`
  adds one and `ClientChannels::send` sends on it. A connection that lists none keeps TGE's packets.
- Plugin RPCs (`torque-engine`'s `rpc`, `NetRegistry::rpc`): typed requests a client makes of the server, with a reply or as
  a notification, and notifications the server sends a client, declared with a `#[derive(NetMessage)]` message (a `String` or
  `Vec` must say its `max_len`, a float its `finite` check, a ghost is named by `GhostRef` and checked against what the server sent
  that client), a direction (a server-to-client RPC has no reply), `Delivery`, a per-connection token bucket counted in ticks, a
  size cap, `Allow` (anyone, a permission, a check) and the script's `onRpcCheck`, and a send queue whose cap fails a call
  with `Busy`. Each is listed by name, direction and layout digest in the manifest (format 2, only when RPCs exist). In TGE mode
  they ride a reserved priority channel in each direction, so the packets of a game with none are unchanged. A client reaches
  them through `ClientRpc`, the server through `ServerRpc`, and scripts through `rpcToServer`, `rpcToClient`, `rpcDrops` and
  handlers named `rpcServer_<name>` and `rpcClient_<name>`.
  A client resolves and checks the ghosts a server notification names against its own world, a malformed batch from the server
  ends the connection with a reason, the server holds received batches per connection and drops only a client that goes over its
  share, `onRpcCheck` must answer true and a call with no caller object is refused, and a server command may ask for a permission
  (`CommandSpec::requires`).
  A script handler gets objects and a script call names them (the ghost indexes stay on the wire), a message a peer must refuse
  (a float that is not finite) is never sent, a reply that comes after its call timed out is ignored, what a side sends is held
  to the same rates the other side applies, frames a carrier refuses stay queued, and two RPCs may not share a script handler.
  An RPC may be marked for client scripts (`RpcBuilder::realm`): only those can be called or heard from a server's Lua. An RPC's
  message may be as large as a batch (`MAX_RPC_BYTES`, 16 KiB less its length); an RPC asks for what it needs, 256 bytes by default.
- Permissions (`torque-permissions`, `torque-engine`'s `permissions`): dotted permission names (`props.spawn`), wildcard grants
  (`props.*`, `*`), and flat roles a game defines (the engine defines none); a connection holds roles and direct grants, with caps on
  each, and a check is true when any grant matches. Plugins declare the permissions they check, with a line of description
  (`engine.permissions().declare`), so tools can list them. A `PermissionProvider` slot keeps what players hold, by auth identity or
  else connection (in memory by default; connection-keyed grants end with the connection), server-side only. Scripts use
  `%client.addRole`, `removeRole`, `hasRole`, `getRoles`, `grant`, `revoke`, `hasPermission`, `definePermissionRole` and
  `declarePermission`.
- `torque-net::manifest` and `torque-net::connect`: a connection manifest (the plugins that have a network version, the
  handshake's protocol versions, the sorted ghost class, datablock class and channel names, the mods a server requires)
  and an auth ticket ride after the connect request's arguments, where a Torque server never reads. A server with a gate
  (`ServerConfig::gate`, `torque-engine`'s `Engine::connect_gate`) refuses a client whose build differs, with the reason
  in the reject and before the host's scripts, any ghost or any datablock; a request with no manifest is judged as a peer
  with nothing. A peer with no plugins sends and reads nothing new, so its handshake is Torque's own. Every list is
  bounded and the decoder is fuzzed.
- `torque-engine`: `engine.net().auth(provider)` (`AuthProvider`: the client makes a ticket, the server verifies it, after
  the manifest check and before `onConnectRequest`; a test provider, `SharedSecret`), and `engine.net()` entries for the
  names the manifest lists. A ticket is made after the server's challenge response arrives and is bound to one server and
  one connection (`AuthContext`: the address the client dialed and the challenge), so a server that relays it, or anyone
  who sniffs it, cannot use it elsewhere. A server tells its provider its own address with
  `GameServer::set_public_address` (by default the address it is bound to): behind a NAT, or bound to a private address
  players do not dial, the public address must be set, and a server that can tell it does not know one (bound to every
  interface, port 0, a relay) warns at start-up that it will refuse every sign-in. The handshake's challenge digest now
  changes every two minutes (the current and the previous one verify), so a sniffed connect request stops working for
  anyone after two to four minutes; within that window an on-path attacker who forges the client's address can still replay
  it, which `AuthContext` documents.
- `torque-net::connect::Identity`: who an auth provider verified a client as, an opaque account id (never empty, at most 128
  bytes) and an optional display name that takes no part in equality. `AuthProvider::verify` and `ConnectGate::check` return
  it, and it is kept on the connection (`Connection::identity`). It is made only on the server from the provider's check, never
  from text a client sends; a connection with none is anonymous and held by the connection itself, a new player each time.
  A real provider gives the same account the same id on every connection, so roles (`SubjectKey::of`), a prop's owner key and an
  upload's resume follow a player across a reconnect; a game's `propOwnerKey` still wins. An account that connects twice ends
  its older connection by default (`$Pref::Server::OneSessionPerAccount = 0` allows several). Script: `%client.getIdentity()`
  (`id:<hex>` or `conn:<n>`) and `%client.getIdentityName()`. Nothing on the wire changes.
- A network protocol slot: a game speaks one protocol, set by one plugin (`TgeProtocolPlugin`, `torque.net.tge`, in
  `DefaultPlugins` and `MinimalPlugins`; a second setter fails the build and names both) or by `[net] protocol` in `game.toml`.
  Torque's own protocol is the only one so far and its bytes are unchanged. `Engine::protocol` reads the slot, and the server
  decides what each connection's peer is in one place (`PeerKind`) instead of in each feature. The trace-hash lines are labeled
  with the protocol.
- The engine's own network protocol (`EngineProtocolPlugin`, `torque.net.engine`), the default of `DefaultPlugins` and
  `MinimalPlugins`; Torque's protocol, byte for byte, is an opt-in (`TgeProtocolPlugin`, `[net] protocol = "tge"`,
  `set_protocol(.., ProtocolKind::Tge)`), and an engine with neither plugin still speaks it. Its own framing over the QUIC
  link, WebTransport, WebRTC data channels, WebSockets, plain UDP on private addresses and the memory link, the manifest always
  sent with the connect, every peer the engine's own, one packet a tick at a byte-rate ceiling, plugin RPCs in their own
  lanes, and per-address and per-network caps on a server's sessions. Plain engine UDP refuses a public bind. A server with an
  auth provider challenges each connect and takes only a ticket made for that challenge, once (bound to the QUIC connection's
  TLS exporter on that link); the mods download asks for blocks with the `torque.mods.get` RPC.
- The QUIC link (`torque-net-quic`, the `quic` feature, on by default natively): TLS 1.3, QUIC's address validation with Retry
  under load, congestion control and connection migration, the server known by the hash of its identity key (pinned, or
  trusted on first use in `known_servers`), and WebTransport sessions for browsers on the same port. In the engine's protocol
  it is what UDP means: `Listen::Udp` opens it, a client's UDP socket dials it for the connection and keeps plain UDP for the
  LAN list, and the server answers the LAN list on the QUIC port. TGE has no encrypted transport.
- In the engine's protocol a packet with ghost or move data is counted lost only by the peer's acks (a packet it read is never
  counted lost, however long its frame), a receiver reads none 3 or more behind its newest, and a sender with 32 unresolved
  sends nothing until acks come, closing the session as timed out when none does. TGE's rule and bytes are unchanged.
- Server tick hooks and server commands for plugins. `engine.push(ServerTick::before(..))` and `ServerTick::after(..)` run a
  function of injected parameters once per 32 ms server tick around the process list's pass, in the order pushed, with the tick's
  number and time (`TickInfo`); a server with none ticks as before. `engine.server_command(name, CommandSpec, handler)` declares a
  `serverCmd*` with typed arguments, a per-client token bucket counted in ticks, and an argument size cap: a call past the limit or
  with a wrong argument is refused before the function runs, counted (`commandDrops(%client, name)`) and reported once a
  second; `server_command_limits` holds a command a script defines. TGE's commands and bytes are unchanged.
- Plugins set a server's main-loop hooks and mission loader (`ServerSlots`): `engine.server_hooks(make)` adds hooks (a list, run in
  add order; `pass_slice` is the first answer) and `engine.mission_loader(make)` sets how `GameServer::load_mission` loads
  a mission (exclusive; none `exec`s the `.mis`). Each `make` runs once per server, after the network host is attached and
  before the entry script, so it can read what the server's recipes set up.
- In the engine's protocol `commandToServer` and `commandToClient` are carried as two built-in RPCs (`torque.command`,
  `torque.command.client`) instead of events; scripts see the same callbacks, arguments, tags and order. A command no plugin
  declared is held to default limits there (32 a second, burst 64, at most 20 arguments of 255 bytes, one bucket for the
  connection), and `commandDrops(%client, "*")` counts what was refused. Torque's protocol has no limit on such a command and
  its bytes are unchanged. `RpcBuilder::outside_connection_rate` leaves an RPC out of the connection's bucket for all RPCs.
- `NetMode::host_port`: a game hosted under `NetMode::Loopback` (every harness) binds the system's free port whatever the options ask, and its client reads the bound port back from the server's address, so a harness game never depends on which ports are free. The examples use it.

#### Gameplay

- Sound: the client mixes through its own mixer (no OpenAL), on the simulation clock until a window opens the sound card
  (`audio-device`); without a device, or headless, every `alx` function answers as with sound and nothing is heard, where TGE's
  `OpenALInitDriver` fails. A ShapeBase's sound slots, audio events and image state sounds play on it as TGE's client plays
  them, in both network protocols.

- `torque-hierarchy`: parent and child transforms for any scene object, unlike TGE's one-level mounting: a tree with depth,
  children and size caps, cycle checks, parents-first resolve, keep-world or keep-local reparenting, a recursive delete and
  a per-child flag to go with the parent, and a quantizer for local poses on the wire. `torque-plugin-hierarchy` (on in engine
  mode, absent with TGE's protocol) puts it on the server: any class may be a child (a prop on a dynamic prop joins its body, on
  anything else is driven; a prop with a joint is refused), Players and vehicles included,
  moving in their parent's frame, and mounts never close a loop with parents; children resolved before every query, turn and read; `setParent`, `deleteRecursive` and kin, a server
  API with no client entry point. In the engine's protocol each child's link and quantized local pose ride its own update
  in a per-ghost block after the class record (a parent's motion sends nothing for its children; a link waits for its
  parent's ghost), whole trees are scoped together, and clients check every link and draw children on their parents; a
  predicting client collides with each child where its parents' latest server poses and its local pose put it, and predicts
  a child it controls (a turret on another player's tank) in its parent's frame, from packet data sent in that frame, so the
  parent's motion never corrects it; a Player child keeps its yaw in its parent's frame. A rope end hung
  from a child ghost follows the child's resolved pose (`ClientGhostLookup` answers through `ClientWorld::world_pose`), and a joint's
  anchor on a Player child is read after the tree's pending moves are written.
- `torque-game`: hitboxes with lag compensation (opt-in): a Player can be hit on its posed shape, checked against where
  the shooter saw it.
- `torque::app::camera_blend` and `camera_boom`: a camera that moves smoothly between first and third person and a
  third-person boom, both kept out of walls and the ground.
- `torque::app::ghosts`: a steady first-person view (`GhostOptions::steady_eye`) carried by the Player's motion rather
  than its animation.
- `torque-sim`, `torque-game`, `torque-client`: Torque3D's crouch, prone and sprint poses on TGE's Player, with timed
  getting down and up, a `walk` action below running pace, and eased transitions between sequences.
- `torque-client`: other players' vehicles drawn where they are now, coasting from their last update, and steering,
  springs and wheel contacts eased between ticks.
- `torque-vehicle`, `torque-game`: a seam under a vehicle's sub-step (`BodySolver`) for whatever resolves the body against
  the world and integrates it, picked per datablock (`SimClasses::body_solver`); TGE's solver stays the default and a
  datablock that picks none runs exactly as before. A criterion bench for the vehicle step (`benches/tick.rs`).
- `torque-physics-rapier` (optional, in no default plugin set): a deterministic Rapier world stepped at the engine's tick
  against the map's terrain (cut along the engine's own diagonals) and interior hulls, built by region on first use, with
  every input checked and every count capped; native and wasm32 step it to the same bits.
- `torque-collide`, `torque-game`, `torque-client`: a plugin can add collision (boxes and convex hulls at a pose, each with a
  type bit of its own taken from the free bits by name) that the server's container and the predicting client's world both
  answer for, so what a plugin owns blocks Players and vehicles on both sides alike (`engine.push(PluginCollision::server(..))`,
  `PluginCollision::client(..)`), listed in the handshake manifest so peers with other sources are refused. Capped in sources, shapes, hull size and work per query; an engine with none is unchanged.
- `torque-plugin-props` (optional): props, rigid bodies a server simulates in one persistent deterministic Rapier world and every
  client is sent and draws as shapes (`PropData`, `propSpawn` and kin, a server API with no client entry point, and
  `propCheckPlacement` for a game's own spawn commands), their state quantized
  to the wire's precision each tick so clients hold the server's state exactly; players and vehicles collide with them and push
  them; caps per player, per remote address, per server and awake (the farthest sleep first), a step budget, sleeping props that cost nothing, owners
  whose props outlive them for a while. Joints between props, the world and players (tethers hard or soft, winches, welds, hinges
  with limits and motors, grapples), server-authoritative, breakable, capped per player, per remote address, per prop and per
  server, sent to every client as `Joint` ghosts, with ropes that follow them (feature `rope`).
  A prop can collide as its shape's collision meshes (`collision = "shape"`: one convex hull a mesh, in one rigid body), built
  the same on the server and in a client's prediction, with caps on hulls, points and the server's total.
  Projectiles push the props they hit and explosions the props around them. With `PropPredictionPlugin` a client predicts
  the props its own Player holds, stands on or pushes (and the joints between them), restored to the server's state and
  replayed, drawn through a smoothed correction, with predicted impulses, spawns and shots of its own that are never sent.
- `torque-sim`, `torque-game`, `torque-client`: a plugin can tether a Player to a point (`player_tether`), applied inside the
  Player's own tick on the server and in the client's prediction alike, so the prediction replays it exactly; with no tether the
  step is Torque's.

#### Content import

- `torque-import`: glTF 2.0 (read and write), Wavefront OBJ and Blockbench (`.bbmodel`) readers, the model layer
  (`torque-modelcore`) into `.dts` and back. A datablock's `shapeFile` may name a `.glb` directly. Shape materials can
  be nearest-sampled (a material flag bit TGE leaves unused), as Blockbench models and glTF `NEAREST` samplers ask.
  glTF materials keep their normal map's scale and their occlusion map and strength.
- `torque-interior-builder`: `.dif` interiors built from convex brushes in code, Quake or Valve 220 `.map` files, or
  glTF, OBJ and Blockbench meshes, with hulls made or authored and a `.map`'s water, triggers and spawn points as
  mission objects. Interior materials can be nearest-sampled or see-through glass.
- `torque-shape-builder`: shapes built from code and written as `.dts`.
- `torque-convert`: the converter command for models and `.map` files.
- `torque-content`: the content formats as asset-loader plugins with caps and trust (`ContentFormatPlugins`, in
  `DefaultPlugins` and `MinimalPlugins`): glTF, OBJ and Blockbench as shapes, `.dif` and `.ter` as data, the map layer
  through an importer, and a `.map` built into an interior; OBJ, Blockbench and `.map` are added off and take trusted files
  only. A game or its `game.toml` turns each on or off by id.
- `torque-mods`: local mods as a plugin (`ModsPlugin`, in `DefaultPlugins` and `MinimalPlugins`): folders and zips with a
  `mod.toml`, mounted over the game's files within caps on size, count, depth and compression ratio, their scripts run by load
  stage as TorqueScript packages (disabling a mod deactivates it), `getModList`, `enableMod`, `disableMod` and `loadMods` from
  script, and files a server sends mounted as content only (never script). How mods are found, named, ordered and enabled are
  slots a game replaces (`engine.mods().layout(..)`); the `[mods]` section of `game.toml` and `$Pref::Mods::Disabled` are the
  default settings. Adds a `torque::mods` module, the `Entry` stage of servers and clients, and `Vm::wrap_file_host`,
  `Namespaces::set_declare_package` and `Vm::exec_policy`.
- `torque-format-valve` (not in `DefaultPlugins`): Valve's Source formats as asset-loader plugins that take untrusted files
  (`ValveFormatPlugins`): VTF textures, VMT materials, MDL/VVD/VTX models with their sequences (made into the model layer),
  VBSP maps (brush faces, displacements, static props, entities, the pak lump, lightmaps) and PCF particle systems with an
  approximate simulation; a game's VPK archives and folders mounted as one tree from its `gameinfo.txt`. A headless viewer
  example draws them from a game folder the player names. Physics files (`.phy`) read as one convex hull per piece with
  the solids' settings and ragdoll joints, falling back to the model's hull box and then its mesh's hull; a `collide`
  feature turns a map's static props into collision hulls.
- `torque-map-logic` (not in `DefaultPlugins`): the entity input/output logic of Hammer-made maps on the server: a
  deterministic, capped event queue (one per-tick work budget; the queue keeps its last place for an event cut short)
  with delays, wildcards and the special target names; logic entities, triggers and
  filters; doors, buttons and movers as the `MapMover` ghost class; `ambient_generic` as sound events; console functions
  for scripts. Entity definitions can be read at run time from a player's installed FGD files over coded defaults. A
  map's brush models read as convex hulls (`torque-format-valve`).
- Every reader bounds its counts, offsets, sizes and paths, and font and secret files are checked before they are read.
- `torque-images`: a JPEG's decode work is tied to its file size (samples per byte, block visits per block and byte), as a
  PNG's is. The Source file readers open a file once and cap its read, and the bundle tool's leak scan looks inside zips, gzip
  and PNG text and in more spellings of a path.

#### Tooling

- `torque`: the umbrella crate, re-exporting the engine crates behind features; `torque-app` (`torque::app`) holds what
  every game shares (its files, a server, a client, a window, the world view and offscreen pictures).
- Math on `glam`.
- `torque-app`: a client can run over its server, on that server's script engine (`ClientBuilder::over_server`), as TGE's
  client holds its local server; the client host then ticks the server. `GameApp` gains `type Server`, the server host of
  the client's script engine: a game that implements `GameApp` itself must add it (every `App` has
  `torque_server::server::Server`; stable Rust has no default for an associated type).
- `torque-script-reference` (`just script-reference`): writes the guide's script reference (every console function, method, class and pref of a default engine, with the opt-in plugins' marked) from the script engines it makes; CI fails when the checked-in pages are stale.
- Native extensions (`torque-plugin-loader`, the `torque` crate's `native-extensions` feature): libraries loaded at run time
  from a local `plugins/` folder over a versioned C ABI (`torque-plugin-abi`, the generated header `torque_plugin.h`), off
  by default, each file approved by its SHA-256 hash (kept per user, never in the plugins folder) and asked about again
  when it changes, loaded from a private read-only copy of the bytes that were hashed, refused before any of its code runs
  when its exported ABI version (read from the file) is another, and disabled instead of crashing the game when it
  panics; `torque-plugin-sdk`
  writes them in Rust with the engine's `#[function]` and render passes. TGE has none. Extensions can also run script
  languages: `exec` hands them the files of their extension, and they call scripts back (nested), define console functions,
  read and write objects and charge the step budget (`Vm::set_script_language`, `Vm::call_depth`).
- Natives, globals and prefs declare their effects where they are registered (`Effects`: files, code, the engine as a whole,
  the network, permissions, a variable an argument names; `ScriptRegistry::with_effects`, `effects = "..."` on
  `#[function]`, the ABI's `TORQUE_NATIVE_FILES` and siblings): the default call policy refuses a script language every
  native and every global write with a files, code, engine or permissions effect, and a test lists every native a full
  engine registers against a reviewed list. TGE has no such policy.
- A variable's name ends at its first NUL byte, as TGE's C strings do: `$a` and `$a` followed by a NUL and more are one
  variable, under one spelling.
- `export` writes a string too long for one line as whole statements (`$x = "..."; $x = $x @ "...";`) where TGE cuts the
  line and leaves its string open.
- Lua for mods (`torque-plugin-lua`, a native extension built on its own: Luau): a mod's `.lua` files run at its stages in
  sandboxed server and client realms (`ts.call`, objects, `ts.define`, `require` within the mod), held to the script
  engine's step limit and call depth and a memory cap, never from a server. TGE has none. In a browser build the same
  extension is a WebAssembly module of its own (`just lua-web`, built with the WASI SDK; it needs WebAssembly exception
  handling), loaded by the page only when its game uses Lua (`build.mjs --lua`, `web/lua.js`) and bridged to the engine's
  `web_lua::WebLuaPlugin`, which makes the C ABI's checks; the same tests run natively and on the web (`just lua-web-check`).
- `torque-client-script-host`: the host of server/client scripts, Lua a server sends run on its players' clients in a realm
  (a WebAssembly guest) per connection, under Wasmtime natively and in a Worker on the web: a wall-clock deadline per entry,
  a linear-memory cap, every op checked against the format and the granted tiers (a violation stops the realm), revocation
  that removes a tier's effects in the same frame, a strike rule, teardown on disconnect and a per-frame budget. In no
  default plugin set. `ui` and `net` have passed their security review (`REVIEWED_TIERS`) and are on by default (the `ui` and `net` cargo features); no other tier is offered. TGE has none.
- `torque-plugin-client-script`: server scripts for a game, the part a game fills in around the realm. The player's consent comes
  first: a global setting (ask by default, or never), a policy for each ability (ask, always, never), and a
  per-server choice remembered in a file the engine owns, so that no TorqueScript or Lua can read it; nothing of the realm exists,
  and no file of the server's is read, before the answer. The question is a slot (`ConsentPrompt`) with stable ability keys and
  English lines, and the engine ships a plain prompt (`torque-gui`, in the engine layer: it ignores the pointer for half a second
  and takes no key). A realm's `net.send` and the server's messages are plugin RPCs with every size and rate capped where the
  bytes arrive; `net.call` reaches only the RPCs a server marked with `RpcBuilder::realm`. The server sees only `declined`,
  `unavailable` or the abilities granted, and gets `clientScriptOnMessage`, `clientScriptSend`, `clientScriptOnStatus` and an
  offer built from the mods' `[client_script]` manifest tables. `torque-mods` reads a server's script files for a realm only from
  the mount that server offered, verified against its hash and not found through cache sharing. The guest is run only if its hash
  is listed in `ACCEPTED_CLIENT_GUESTS`: the guest built with WASI SDK 34.0 is listed, and `just lua-client-web` gives the same bytes
  whatever the folders it is built in (the build names them by fixed stand-ins). The justfile's `client-script-check` is the hardening suite. In no default plugin set. TGE has none.
- Web builds run server scripts: `web/build.mjs --client-scripts` adds the page's runner and the realm guest, and the page starts
  the runner before the game only if the guest's hash is on the module's own list (`web_exports!`'s `client_guests`, exported as
  `acceptedClientGuests`); `page_runner_ready` tells a game whether to offer `WebRunner`.
- `torque-bundle` (`just bundle <game crate>`): packages a game for the computer it runs on: its binaries built in release
  with only the plugin features its `game.toml` keeps (debug gizmos left out), its game folder without dev files, its native
  extensions in `plugins/`, the license texts of every crate it links, and a manifest of every file's SHA-256. Local folders
  are remapped out of the builds and a bundle holding one is refused; the same inputs give the same manifest. TGE has none.
  `--archive` also writes one file, a zip on Windows and a gzipped tar on Linux and macOS, with fixed times, owner, order and
  modes, so the same inputs give the same SHA-256; a game maker can run `--twice` to verify that for their game (it builds two
  times from clean folders and fails unless the archives are equal). `--server` packages a game's dedicated server and
  refuses it if it links a renderer, window, audio or similar crate or imports a GPU, window or audio system library.
  Every file of the game folder that a loader of the engine owns (shapes, interiors, terrains, images, sounds, fonts, scripts, zips,
  settings) is opened by that loader under its caps before anything is built, and a refused file fails the bundle with its path
  and the reason (`--no-validate` skips this, `--validate` does only this); `--precompile` ships the `.cs` and `.gui` scripts compiled.
  `--web` packages the browser build as a folder a static host serves: its pack's files opened by their loaders, `files.json` checked against the folder and the manifest, then a headless check.
  Two checkouts of one commit in different folders give byte-identical archives (`just bundle-repro-check`), and `--compare` names the
  files that differ between two bundles' archives. The builds run under the tool as cargo's rustc wrapper, so a `RUSTC_WRAPPER` of the
  user's is not used by bundle builds. A game's own extension libraries load without a question when their hashes, compiled into
  its executable by the bundler, match (`NativeExtensionsPlugin::shipped`); any other library asks as usual. A packaged game can
  also read its `manifest.toml` when it starts (`NativeExtensionsPlugin::bundle`) and warn about a listed file that changed or is
  missing; that report is advisory and approves nothing.
- Continuous integration on Linux (formatting, clippy, tests, rustdoc, the guide, the MSRV and cargo-deny),
  and `unsafe` code only in the pipeline cache and the native extensions' C ABI (one `ffi` module each in
  `torque-plugin-sdk` and `torque-plugin-loader`).

[Unreleased]: https://github.com/Meshiest/torque-rs/commits/main
