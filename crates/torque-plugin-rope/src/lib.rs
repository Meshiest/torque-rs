//! A rope plugin for torque-rs: hanging cables, tethers and washing lines, simulated and drawn on the client.
//!
//! [`RopePlugin`] adds
//!
//! - the datablock class `RopeData` ([`RopeData`]): how it is drawn, how many segments, how long, how thick, how stiff, what color;
//! - the console functions `ropeCreate`, `ropeAttach`, `ropePin`, `ropeRelease`, `ropeImpulse`, `ropeDelete`, `ropeClear`,
//!   `ropeCount`, `ropeFloor`, `ropeNoFloor`, `ropeSetLength` and `ropeSetLifetime` (for a client's own ropes), and a server's
//!   `ropeNet*` functions that make ropes every client shows ([`net`]);
//! - a simulation ([`RopeWorld`]) that runs once for each frame the ropes are drawn;
//! - three render passes ([`RopePass`]): the lit, fogged tubes into the opaque frame, their shadows into the sun's cascades, and
//!   the ribbons and lines over the frame's depth after the translucent surfaces.
//!
//! It is not in [`DefaultPlugins`](https://docs.rs/torque-app): a game adds it. A game that draws with another renderer than the
//! engine's sees no ropes (the passes run in the engine's renderer only), and one that never draws pays for the simulation alone.
//!
//! ```
//! use torque_app::plugins::DefaultPlugins;
//! use torque_engine::EngineBuilder;
//! use torque_plugin_rope::RopePlugin;
//!
//! // In the game's `main`, beside the default plugins:
//! let engine = EngineBuilder::new()
//!     .add_plugins((DefaultPlugins, RopePlugin::default()))
//!     .build()?;
//! assert!(engine.is_plugin_added::<RopePlugin>());
//! # Ok::<(), torque_engine::BuildError>(())
//! ```
//!
//! A client script then describes a rope and hangs it:
//!
//! ```text
//! datablock RopeData(WashingLine)
//! {
//!    segments = 16;
//!    thickness = 0.04;
//!    color = "200 200 190 255";
//! };
//!
//! %rope = ropeCreate("-4 0 3", "4 0 3", WashingLine);   // between two points, sagging
//! ropeAttach(%rope, 1, PostB);                          // the far end now follows the object PostB
//! ropeImpulse(%rope, "0 3 0");                          // a push
//! ropeDelete(%rope);
//! ```
//!
//! # How a rope behaves
//!
//! A rope is a chain of points ([`RopeData::segments`] pieces). In each substep the points move on by their own speed and
//! gravity, then the pieces are pulled back to their resting length, over and over, so the rope keeps its length whatever pulls on
//! it (position-based dynamics: see [`sim`]). Stiffness resists bending; a floor ([`RopeWorld::set_floor`], `ropeFloor(z)`) keeps
//! the rope off the ground.
//!
//! **Time.** A rope is a client-side visual, so it runs off the game's 32 ms tick: each drawn frame runs the substeps (240 a
//! second, at most eight a frame: a longer hitch is dropped, not caught up) that the time since the last frame holds
//! ([`RopeWorld::step_frame`], from the render passes; the same frame seen by a second view runs nothing), and shows the rope between
//! its last two states, so it moves smoothly at any frame rate. A client that draws nothing runs its ropes from its tick instead.
//!
//! **Anchors.** The ends are *pinned*: to a point, or to an object. An end that hangs from an object follows where the *frame draws*
//! that object when the game can say which ghost it is ([`AnchorResolver::ghost`]) and where [`FrameAnchors`] puts a `node` on the
//! shape as drawn (the shape's interpolated transform of this frame, so the end is exactly at the drawn anchor every frame); an
//! object with no drawn shape is followed by what the script reads once a tick (`position` or `transform`, or whatever a game's own
//! [`AnchorResolver`] reads), eased. A rope that hangs from a deleted object lets go of it. [`DrawnShapes`] puts an empty `node` at the
//! shape's origin and a number at the middle of that mesh of the shape as animated; any other node is the origin as well, so a name
//! the game's [`FrameAnchors`] does not know ties the rope to the origin.
//!
//! **Offsets.** An end can hang from a point in a ghost's own frame: [`RopeWorld::pin_ghost_at`] (and `ropeNetAttachAt` from a server
//! script) holds the point `offset` from the node, turned with the ghost's transform as the frame draws it
//! ([`FrameAnchors::mount_at`]), so the end rotates with the ghost and is at the drawn point every frame, interpolated as the ghost
//! is. A client that draws no frame adds the offset to the ghost's position unrotated.
//!
//! # Limits
//!
//! At most [`MAX_ROPES`] ropes and [`MAX_TOTAL_SEGMENTS`] segments over all of them; a datablock's numbers are clamped to their
//! ranges (a NaN reads as the default). A call past a cap prints a console error and makes nothing.
//!
//! The ropes are the engine's, not each client's: every client of one engine shares them, and each client's frames run the
//! simulation, so a game with several clients in one process should make one engine for each. A rope a client makes with
//! `ropeCreate` is its own; a rope a server makes with `ropeNetCreate` is every client's ([`net`]).
//!
//! # Length, taut ropes and temporary ropes
//!
//! `ropeSetLength(rope, length, seconds)` reels a rope in or out evenly over `seconds` (0: at once). When both ends are held
//! farther apart than the rope is long, it is drawn straight between them with its pieces evenly spaced, not stretched; it pulls
//! on nothing (pulling is for a physics joint a rope can follow), and it hangs again once the ends come closer or the length grows
//! ([`Rope::taut`]). `ropeSetLifetime(rope, seconds, fade)` makes a rope a temporary one: it is taken away after `seconds`, and over
//! its last `fade` seconds a tube thins and a ribbon or a line grows fainter ([`Rope::fade`]).
//!
//! # Drivers
//!
//! A rope normally follows what a script told it. Another plugin can instead *drive* it from Rust: [`RopeWorld::drive`] sets the two
//! ends ([`DriveEnd`]: a point, a place on a ghost at an offset, or free), the length, whether the rope is at its limit (drawn
//! straight and evenly spaced between its ends) and its curve, each frame or tick, and a [`RopeDriver`] registered under a name
//! ([`RopePlugin::with_driver`], [`RopeDrivers`]) is asked each client tick for every rope bound to it. A server binds a networked
//! rope with `ropeNetDrive(rope, name, object)`; each client then asks its driver about its own ghost of `object` (a joint, say), so
//! only the name and the object cross the network. A rope stays a visual: a driver says where it is drawn, and it never pushes
//! back. See [`driver`].
//!
//! # Over the network
//!
//! The plugin is networked ([`Plugin::net_version`]): a server with it refuses a client without
//! it at the connection, before any ghost is sent. A server script makes a rope with `ropeNetCreate(start, end, datablock)` and
//! changes it with `ropeNetPin`, `ropeNetAttach`, `ropeNetAttachAt`, `ropeNetRelease`, `ropeNetSetLength`, `ropeNetSetColor`, `ropeNetDrive` and the other `ropeNet*`
//! functions (each marks only the group it changed); every client of the engine then shows the rope, simulating its points itself.
//! `ropeNetFx` sends a short-lived rope as a remote command with no ghost; `ropePredict` lets a client show its own rope at once and
//! have the server's ghost adopt it. The book's "Networked plugins" chapter walks through this plugin.
//!
//! # Styles
//!
//! A datablock's `style` says how its ropes are drawn; the number of segments, the caps and the sharing of the world are the same
//! for all three.
//!
//! | `style` | Drawn as | Simulated | Casts a shadow | Takes a shadow |
//! |---|---|---|---|---|
//! | `"tube"` (the default) | a lit tube of eight sides | yes | yes | yes |
//! | `"ribbon"` | a flat strip that faces the camera, along a smooth (Catmull-Rom) curve through the simulated points | yes | no | yes |
//! | `"line"` | a straight strip between the two ends | no: the ends are where they are pinned, and a free end stays | no | yes, unless `emissive` |
//!
//! A ribbon reads as a smooth curve even with few segments, and is cheap: cables, vines, beams and tethers. A line is for a laser,
//! a tripwire or a power line seen from far away; it still counts as one rope and one segment against [`MAX_ROPES`] and
//! [`MAX_TOTAL_SEGMENTS`]. `thickness` is the strip's width, never less than two pixels on the screen (a far strip stays a thin
//! line); the `color`'s alpha makes a strip see-through, and `emissive = true` draws it without light or shade, like a laser (the
//! fog still covers it). A style that is none of the three prints a warning when a rope is made and makes a tube.
//!
//! ```text
//! datablock RopeData(Laser)
//! {
//!    style = "line";
//!    thickness = 0.04;
//!    color = "255 40 40 220";
//!    emissive = true;
//! };
//! %beam = ropeCreate("-4 0 1.2", "4 0 1.2", Laser);   // a straight strip; ropeAttach moves an end with an object
//! ```
//!
//! **Textures.** `texture` names a `.png` or `.jpg` (read through the engine's asset loaders: their size caps, their trust rules and
//! their hot reload; a file that changes on disk is drawn again), drawn along the rope: `u` runs along it by arc length, one repeat
//! every `textureLength` world units (the rope's width by default, a square tile), so the texture does not stretch as the rope sags
//! or lengthens and runs on across the segment joints and the ribbon's curve; `v` runs around a tube or across a strip. The
//! `color` multiplies the texture (white keeps its own colors), `textureScroll` runs it along the rope in repeats a second (the
//! effects' clock; a laser, a conveyor, an energy beam) and `textureFilter = "nearest"` keeps hard texels for pixel art (else
//! linear with mip levels). A ribbon or a line uses the texture's alpha, blended; a tube cuts out what is under half alpha, and the
//! cut-out casts no shadow. A file that is missing, too big or not an image warns once and the rope is drawn plain.
//!
//! **One rope's own look.** `ropeSetColor(rope, "r g b a")`, `ropeSetTexture(rope, file)`, `ropeSetTextureScroll(rope, speed)` and
//! `ropeSetEmissive(rope, on)` change one rope without a datablock of its own (`ropeGetColor` and kin read what the rope has now);
//! an empty string clears the color or the texture back to the datablock's, and `ropeClearLook` clears all four. They are kept
//! per rope in a [`RopeLook`], a small plain struct (each value `Some` over the datablock's, `None` its own) that a rope's ghost can
//! carry.
//!
//! **Driven shapes and beams.** A datablock with `sim = "none"` (or a rope given `ropeSetCurve`) is not simulated: its shape is set
//! each frame. Straight between its ends, or on the cubic Bezier `ropeSetCurve(rope, startDir, endDir, bend)` gives it: from the start
//! it leaves along `startDir`, into the far end along `endDir` (each `""`: toward the other end, `"mount"`: the way the end's mount
//! points as the frame draws it, or a direction `"x y z"`), the handles `bend` times the distance between the ends long; the samples
//! are evenly spaced along it, so a texture does not bunch. It works with every style, and both ends and the mounts' directions
//! follow what is drawn each frame ([`FrameAnchors::mount`]). A *tractor beam* from a weapon is a ribbon with `sim = "none"`,
//! `additive`, `emissive` with an `emissiveIntensity` above 1, a `texture` running with `textureScroll`, a taper (`endThickness`)
//! and a gradient (`colorEnd`), its start attached to the muzzle (`ropeAttach`, or [`RopeWorld::pin_ghost`] with the node `"muzzle"`
//! under [`MuzzleAnchors`]) and `ropeSetCurve(rope, "mount", "", 0.4)`.
//!
//! [`MuzzleAnchors`] read the muzzles a game drew this frame from a [`MuzzleTable`] (torque-app's `GhostScene::muzzles()` fills one):
//! `"muzzle"` or `"muzzleN"` is the muzzle of the image in slot N of the ghost, with its barrel's direction. For the player whose
//! view is first person it is the first-person weapon's muzzle, drawn before the eye with the client's predicted aim; for other
//! players, and in third person, the weapon's mount. An image whose shape has no `muzzlePoint` node has no muzzle in the table: the
//! end then goes to the shape's origin, pointing along its forward (+y). The own figure is not drawn in first person, so the
//! muzzle is found by the ghost's number ([`FrameAnchors::ghost_mount`]).
//!
//! **Shadows.** `castShadow` and `receiveShadow` are 1 or 0, or -1 (the default) for the style's own, the columns of the table.
//! A tube casts as a tube. A ribbon or a line that casts makes a band of its `thickness` that faces the sun (the strip itself faces
//! the camera, which would give a shadow that changes as the camera moves), so its shadow on the ground does not change with the
//! camera. A rope that takes shadows is darkened as the renderer's own surfaces are, through the same cascades and filter
//! ([`RopePass`] reads them from `PassCx::sun_shadows`); a laser (an emissive line) does not by default, since it should not
//! darken.
//!
//! Strips are drawn with the frame's depth, so things in front of them hide them and things behind them are hidden by them; they
//! have no collision. Looking along a strip it never turns into a sliver: its width always faces the camera, and its ends are
//! pushed out by half a width. See [`strip`].
//!
//! # Drawing
//!
//! The tubes are drawn at [`Stage::Opaque`] over the frame's depth, lit as the frame lights its
//! surfaces (the sun's diffuse and ambient light, darkened where a shadow falls on a rope that takes one), fogged as the frame fogs. The shadow pass draws
//! the tubes of the ropes that cast a shadow into each cascade at [`Stage::Shadows`]. The ribbons and lines are drawn at
//! [`Stage::Translucent`], alpha blended over the frame's depth (tested, not written), with the frame's light and fog; their
//! triangles are made again each frame the camera or the ropes moved. A
//! rope has no motion vector, so a fast one smears a little under the anti-aliasing.
//!
//! # Maintaining
//!
//! **Speed.** A rope that hangs still rests ([`sim::REST_MOVE`]): its frames run no substeps for it and the world's revision,
//! and so the tube mesh and the strips, stay put while every rope rests. Whatever changes a rope (every [`RopeWorld`] call on
//! it, a pin whose anchor moves, a new floor) wakes it, and a reeling, driven or fading rope never rests:
//! `a_hanging_rope_comes_to_rest_and_an_impulse_wakes_it`. The bench: `cargo bench -p torque-plugin-rope --bench sim` (64 ropes
//! of 32 segments at rest and swinging).

mod data;
pub mod driver;
mod image;
mod mesh;
mod natives;
pub mod net;
mod pass;
pub mod sim;
pub mod strip;

use std::rc::Rc;
use std::sync::Arc;

use torque_app::render::{EngineRender as _, Stage};
use torque_engine::assets::{Assets, Spec};
use torque_engine::net_classes::ClientGhostLookup;
use torque_engine::{ClientStage, ClientTick, EngineBuilder, Plugin, PluginId};
use torque_ts_text::atof;
use torque_ts_vm::sim::ObjId;
use torque_ts_vm::vm::Vm;

pub use data::{MAX_LENGTH, MAX_SEGMENTS_PER_ROPE, RopeData, RopeLook, RopeParams, RopeStyle};
pub use driver::{Drive, DriveEnd, MAX_DRIVER_NAME_BYTES, RopeDriver, RopeDrivers};
pub use image::{
    ImageData, MAX_IMAGE_BYTES, MAX_IMAGE_SIDE, SharedImage, encode_png, scroll_offset,
};
pub use mesh::{SIDES, TubeMesh, TubeVertex, VERTEX_BYTES};
pub use natives::RopeShared;
pub use net::{NetRopes, RopeGhost, RopeNet};
pub use pass::{RopePass, SharedWorld};
pub use sim::{
    Anchor, Appearance, Curve, MAX_ROPES, MAX_TEXTURES, MAX_TOTAL_SEGMENTS, Mount, Rope, RopeEnd,
    RopeError, RopeId, RopeWorld, Tangent, TextureEntry, TextureTable,
};
use torque_render_scene::hd::DynamicObject;

/// The seconds one client tick advances the ropes by (the server's tick, 32 ms).
pub const TICK_SECONDS: f32 = 0.032;

/// The most seconds one pass of the client's loop advances the ropes by (a stalled client does not run them on in one jump).
const MAX_TICK_SECONDS: f32 = 0.25;

/// Where an object a rope hangs from is: the game's answer for [`Anchor::Object`].
pub trait AnchorResolver: 'static {
    /// The place of `node` on the object `object` in the client's script engine `vm`; None if the object is gone.
    fn position(&self, vm: &Vm, object: ObjId, node: &str) -> Option<[f32; 3]>;

    /// The number of the ghost the client draws for `object`, if it has one: the end that hangs from the object then follows
    /// where the frame draws that ghost ([`FrameAnchors`]), each frame, instead of the script's reading once a tick. A game whose
    /// script objects are not its ghosts (or has no ghosts) leaves this at None.
    fn ghost(&self, _vm: &Vm, _object: ObjId) -> Option<u32> {
        None
    }
}

/// Where a ghost is drawn in a frame: the game's answer for the ends that follow a ghost ([`AnchorResolver::ghost`],
/// [`RopeWorld::pin_ghost`]). It is asked each frame the ropes are drawn, from the renderer's thread, with the shape as the frame
/// draws it (its transform as interpolated for this frame, its animated meshes' placements).
pub trait FrameAnchors: Send + Sync + 'static {
    /// The place of `node` on `shape`; None to keep the end where it was.
    fn place(&self, shape: &DynamicObject, node: &str) -> Option<[f32; 3]>;

    /// The place of `node` on `shape` and the way it points, for a rope whose curve leaves along the mount (`ropeSetCurve` with
    /// `"mount"`: a beam from a muzzle along the aim). The default is [`FrameAnchors::place`] with no direction.
    fn mount(&self, shape: &DynamicObject, node: &str) -> Option<Mount> {
        self.place(shape, node).map(|at| Mount { at, dir: None })
    }

    /// [`FrameAnchors::mount`] of the point `offset` from `node` in the shape's own frame (a rope end hung from a ghost at an
    /// offset, [`RopeWorld::pin_ghost_at`]): the offset turns with the shape's transform of this frame, so the point rotates with
    /// the shape and is drawn at the same interpolated pose the shape is. The default adds the turned offset to the mount; its
    /// direction is the mount's.
    fn mount_at(&self, shape: &DynamicObject, node: &str, offset: [f32; 3]) -> Option<Mount> {
        let mut mount = self.mount(shape, node)?;
        if offset != [0.0; 3] {
            let turned = shape.transform.mul_v(offset);
            for (at, d) in mount.at.iter_mut().zip(turned) {
                *at += d;
            }
        }
        Some(mount)
    }

    /// The place of `node` on ghost `ghost` when the frame draws no shape for it (the controlled player's own figure is not drawn
    /// in first person, though its weapon's muzzle is): the default is None, and the end stays where it was.
    fn ghost_mount(&self, _ghost: u32, _node: &str) -> Option<Mount> {
        None
    }
}

/// The default [`FrameAnchors`]: an empty `node` is the shape's origin; a number is the middle of that mesh of the shape as it is
/// animated now (a limb's middle: the meshes are the shape's `nodes`, in the order the renderer has them); a name, and a number
/// the shape has no mesh for, are the shape's origin too. A game that knows a shape's mounts (a hand, a hitch) supplies its own.
#[derive(Clone, Copy, Debug, Default)]
pub struct DrawnShapes;

impl FrameAnchors for DrawnShapes {
    fn place(&self, shape: &DynamicObject, node: &str) -> Option<[f32; 3]> {
        if let Some(place) = node
            .trim()
            .parse::<usize>()
            .ok()
            .and_then(|n| shape.nodes.get(n))
        {
            let [x, y, z, _] = place.sphere;
            return Some(place.now.mul_p([x, y, z]));
        }
        Some(shape.transform.mul_p([0.0, 0.0, 0.0]))
    }
}

/// The default [`AnchorResolver`]: the object's `position` field, else the first three numbers of its `transform`; the node is
/// ignored. A script object that moves itself (`%o.position = "1 2 3"`) carries a rope.
#[derive(Clone, Copy, Debug, Default)]
pub struct FieldAnchors;

impl AnchorResolver for FieldAnchors {
    fn position(&self, vm: &Vm, object: ObjId, _node: &str) -> Option<[f32; 3]> {
        vm.objects().get(object)?;
        ["position", "transform"].into_iter().find_map(|field| {
            let text = vm.objects().field(object, field.as_bytes(), b"")?;
            let mut words = text
                .split(|c| c.is_ascii_whitespace())
                .filter(|w| !w.is_empty());
            let mut next = || words.next().map(|w| atof(w) as f32);
            Some([next()?, next()?, next()?])
        })
    }
}

/// The muzzle of an image a ghost holds as the game drew it this frame ([`MuzzleTable`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DrawnMuzzle {
    /// The ghost holding the image.
    pub ghost: u32,
    /// The image's mount slot.
    pub slot: usize,
    /// The muzzle's place.
    pub at: [f32; 3],
    /// The way the barrel points.
    pub dir: [f32; 3],
    /// Whether this is the first-person image's muzzle (drawn before the eye, for the player who controls the view) and not the
    /// third-person mount's.
    pub first_person: bool,
}

/// The muzzles the game drew this frame, shared with the render passes: the game sets it each frame after it placed its held
/// images (for torque-app's `GhostScene` that is `scene.muzzles()`), [`MuzzleAnchors`] reads it when the ropes are drawn.
#[derive(Clone, Default)]
pub struct MuzzleTable {
    entries: Arc<std::sync::Mutex<(Vec<DrawnMuzzle>, bool)>>,
}

impl MuzzleTable {
    /// An empty table.
    pub fn new() -> MuzzleTable {
        MuzzleTable::default()
    }

    /// Replaces the muzzles with this frame's, and says whether the view is first person (the controlled player's first-person
    /// muzzle is the one to use, else its third-person one).
    pub fn set(&self, muzzles: impl IntoIterator<Item = DrawnMuzzle>, first_person_view: bool) {
        let mut guard = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.0.clear();
        guard.0.extend(muzzles);
        guard.1 = first_person_view;
    }

    /// The muzzle to use for image `slot` of `ghost`: the first-person one when the view is first person and there is one, else the
    /// third-person one, else (a game that only gives one) whichever there is; None when the image has no muzzle.
    pub fn find(&self, ghost: u32, slot: usize) -> Option<DrawnMuzzle> {
        let guard = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mine = || {
            guard
                .0
                .iter()
                .filter(move |m| m.ghost == ghost && m.slot == slot)
        };
        mine()
            .find(|m| m.first_person == guard.1)
            .or_else(|| mine().find(|m| !m.first_person))
            .or_else(|| mine().next())
            .copied()
    }
}

/// [`FrameAnchors`] that know the muzzles of held images: the node `"muzzle"` (or `"muzzle1"` for the image in slot 1) is the
/// muzzle of the ghost's image as the game drew it this frame ([`MuzzleTable`]), with the barrel's direction, so a rope's end
/// leaves the weapon's muzzle along its aim. For the player whose view is first person it is the first-person weapon's muzzle (with
/// the client's predicted aim), for everyone else and in third person the weapon's mount. An image whose shape has no muzzle node
/// has no entry: the end then goes to the shape's origin pointing along its forward (+y), as [`DrawnShapes`] does. Other nodes are
/// [`DrawnShapes`]'.
#[derive(Clone)]
pub struct MuzzleAnchors {
    table: MuzzleTable,
}

impl MuzzleAnchors {
    /// Anchors over `table`.
    pub fn new(table: MuzzleTable) -> MuzzleAnchors {
        MuzzleAnchors { table }
    }
}

impl FrameAnchors for MuzzleAnchors {
    fn place(&self, shape: &DynamicObject, node: &str) -> Option<[f32; 3]> {
        self.mount(shape, node).map(|m| m.at)
    }

    fn ghost_mount(&self, ghost: u32, node: &str) -> Option<Mount> {
        let slot = node.trim().strip_prefix("muzzle")?;
        let m = self
            .table
            .find(ghost, slot.trim().parse::<usize>().unwrap_or(0))?;
        Some(Mount {
            at: m.at,
            dir: Some(m.dir),
        })
    }

    fn mount(&self, shape: &DynamicObject, node: &str) -> Option<Mount> {
        let Some(slot) = node.trim().strip_prefix("muzzle") else {
            return DrawnShapes.mount(shape, node);
        };
        let slot = slot.trim().parse::<usize>().unwrap_or(0);
        Some(match self.table.find(shape.ghost, slot) {
            Some(m) => Mount {
                at: m.at,
                dir: Some(m.dir),
            },
            None => Mount {
                at: shape.transform.mul_p([0.0; 3]),
                dir: Some(shape.transform.mul_v([0.0, 1.0, 0.0])),
            },
        })
    }
}

/// Adds ropes to the engine's clients ([crate docs](crate)). Not in `DefaultPlugins`.
#[derive(Clone, PluginId)]
#[plugin(id = "torque.rope")]
pub struct RopePlugin {
    anchors: Rc<dyn AnchorResolver>,
    frame_anchors: Arc<dyn FrameAnchors>,
    drivers: Vec<(String, Rc<dyn RopeDriver>)>,
}

impl Default for RopePlugin {
    /// The plugin with [`FieldAnchors`] reading the objects the ropes hang from.
    fn default() -> RopePlugin {
        RopePlugin::with_anchors(FieldAnchors)
    }
}

impl RopePlugin {
    /// The plugin with `anchors` reading where the objects the ropes hang from are (a game's ghosts, say).
    pub fn with_anchors(anchors: impl AnchorResolver) -> RopePlugin {
        RopePlugin {
            anchors: Rc::new(anchors),
            frame_anchors: Arc::new(DrawnShapes),
            drivers: Vec::new(),
        }
    }

    /// The plugin whose ropes follow the muzzles in `table` ([`MuzzleAnchors`]).
    pub fn with_muzzles(self, table: MuzzleTable) -> RopePlugin {
        self.with_frame_anchors(MuzzleAnchors::new(table))
    }

    /// The plugin with the rope driver `driver` registered under `name` ([`RopeDrivers::register`]; a name that is not valid, or that
    /// two drivers want, is left out and the engine warns when it is built). Ropes bound to the driver follow what it says
    /// ([`driver`]).
    pub fn with_driver(mut self, name: &str, driver: impl RopeDriver) -> RopePlugin {
        self.drivers.push((name.to_owned(), Rc::new(driver)));
        self
    }

    /// The plugin reading where the frame draws a ghost its ropes follow with `frame_anchors` (a game's mounts, say) instead of
    /// [`DrawnShapes`].
    pub fn with_frame_anchors(mut self, frame_anchors: impl FrameAnchors) -> RopePlugin {
        self.frame_anchors = Arc::new(frame_anchors);
        self
    }
}

// ANCHOR: build
impl Plugin for RopePlugin {
    // ANCHOR: net_version
    fn net_version(&self) -> Option<u32> {
        Some(net::NET_VERSION)
    }
    // ANCHOR_END: net_version

    fn build(&self, engine: &mut EngineBuilder) {
        let ropes = RopeShared::new();
        let book = RopeNet::new();
        engine.insert_resource(ropes.clone());
        engine.insert_resource(book.clone());
        let drivers = RopeDrivers::new();
        for (name, driver) in &self.drivers {
            if !drivers.register_shared(name, Rc::clone(driver)) {
                engine.warn(format!(
                    "the rope driver name {name:?} is not valid (1 to {MAX_DRIVER_NAME_BYTES} bytes, no spaces) or is taken twice; it is left out"
                ));
            }
        }
        engine.insert_resource(drivers.clone());
        // The ropes a server sends are textured through the client's asset cache, which is there before the first one arrives.
        engine.client(ClientStage::Natives, |vm, engine| {
            let _ = engine.assets(vm);
            Ok(())
        });
        net::register(engine, &ropes, &book, &drivers);
        // The images a rope is textured with: `.png` and `.jpg` files through the engine's asset loaders (their caps, their trust, their
        // hot reload).
        engine.assets().loader(
            &["png", "jpg"],
            Spec::untrusted(MAX_IMAGE_BYTES),
            |bytes, cx| ImageData::decode(cx.path(), bytes).map(|i| SharedImage(Arc::new(i))),
        );
        engine
            .client_script()
            .class::<RopeData>()
            .natives(natives::declared());
        let anchors = Rc::clone(&self.anchors);
        let world = ropes.clone();
        // The seconds between two ticks are the script engine's own clock's: a host runs this hook once for each pass of its main
        // loop, which is not once for each 32 ms.
        let last = std::cell::Cell::new(None::<u32>);
        engine.push(ClientTick::new(move |vm| {
            let now = vm.sim_time();
            let seconds = last
                .replace(Some(now))
                .map_or(0.0, |was| now.wrapping_sub(was) as f32 / 1000.0)
                .min(MAX_TICK_SECONDS);
            let mut world = world.lock();
            world.resolve(|object, node| anchors.position(vm, object, node));
            world.resolve_ghosts(|object| anchors.ghost(vm, object));
            let lookup = ClientGhostLookup::of(vm);
            net::client::tick(&mut world, &mut book.lock(), lookup.as_deref(), seconds);
            drivers.run(vm, &mut world);
            // A texture file that changed on disk: the cache loaded it again, and the ropes that use it take the new image.
            if let Some(assets) = Assets::of(vm) {
                for path in assets.changed() {
                    if let Some(image) = assets.get::<SharedImage>(&path) {
                        for file in world.texture_files() {
                            if torque_engine::assets::normalize(&file).is_ok_and(|n| n == path) {
                                world.reload_texture(&file, &image.0);
                            }
                        }
                    }
                }
            }
            world.tick(seconds);
        }));
        let (shadows, opaque, translucent) = (ropes.0.clone(), ropes.0.clone(), ropes.0);
        let (a, b, c) = (
            Arc::clone(&self.frame_anchors),
            Arc::clone(&self.frame_anchors),
            Arc::clone(&self.frame_anchors),
        );
        engine
            .render()
            .pass(Stage::Shadows, move || {
                RopePass::with_anchors(shadows.clone(), Arc::clone(&a))
            })
            .pass(Stage::Opaque, move || {
                RopePass::with_anchors(opaque.clone(), Arc::clone(&b))
            })
            .pass(Stage::Translucent, move || {
                RopePass::with_anchors(translucent.clone(), Arc::clone(&c))
            });
    }
}
// ANCHOR_END: build

#[cfg(test)]
mod tests {
    #[test]
    fn the_tick_is_the_servers() {
        assert_eq!(
            (super::TICK_SECONDS * 1000.0).round() as u32,
            torque_server::server::TICK_MS
        );
    }
}
