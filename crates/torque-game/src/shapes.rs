//! What loading a shape does (`TSShape::read`; TGE 1.4 `ts/tsShape.cc` for the reading order) and what the
//! datablocks that load one find in it (`ShapeBaseData::preload`, `ShapeBaseImageData::preload`,
//! `TSShapeConstructor::preload`).
//!
//! **The version.** The version of a `.dts` is the low byte of the file's first little-endian 32-bit word (the exporter's
//! version sits in the bits above): above 24 (`TSShape::smVersion`) the shape does not load and the engine
//! prints `Error: attempt to load a version <n> dts-shape, can currently only load version 24 and before.`
//! (`<n>` is the low byte, 25 for `0x00020019`, 255 for 255); below 19 it prints
//! `... Shape with old version.` and reads the old format. The resource manager keeps a shape it loaded, by its
//! path ignoring case, so the line prints once per shape (a second datablock on the same path, the same
//! path in upper case and a redeclaration print nothing, a copy of the shape under another path prints again; a
//! shape that failed is not kept: the error prints each time it is asked for).
//!
//! **Where it runs.** On a dedicated server the shape-carrying datablocks load their shape in `preload`
//! ([`SHAPE_FIELDS`]: the class and the field holding the path; the subclasses of `ShapeBaseData`, such as
//! `StaticShapeData`, `ItemData`, `PlayerData` and `MissionMarkerData`, carry its field): `shapeFile` of `ShapeBaseData`'s
//! subclasses and of `ShapeBaseImageData` and `WheeledVehicleTire`, `explosionShape`, `projectileShapeName` and `baseShape` of
//! `TSShapeConstructor`; not `DebrisData`'s `shapeFile` or `ShapeBaseData`'s `debrisShapeName` (the server does not load a
//! debris shape), a `.dts.txt` path, or a field a class does not have. The loader is the
//! `EngineClass::preload` of those classes (the interpreter's seam), so the line comes
//! out in the middle of the datablock's declaration.
//! A plugin's own datablock class loads a shape through the same loader ([`shapes_of`] gives it, once [`register`] ran), so its
//! shape is in the same kept set and paid for by the same budget (the props plugin takes collision hulls from a shape so).
//!
//! **What it reads.** The file through [`FileHost`](ts_vm::host::FileHost) only, and a version-24 file in full:
//! `shapes::dts::parse` (which ends with `Shape::validate`) and `shapes::preload::facts` give the runtime
//! [`Shape`] and the facts the preloads find in it, kept in a [`KeptShape`]. The old shape (version below 19) and
//! versions 19 to 23 are kept header-only: the version and the
//! size, no body. A file shorter than four bytes, a path that is not a `.dts`, one past [`MAX_SHAPE_PATH`] bytes, a file the
//! host does not have and one over the read bound print nothing and are not kept; a version-24 file the parser refuses (cut
//! short, over a cap, inconsistent) prints [`BAD_SHAPE_LINE`] and does not load; a synthetic old shape loads header-only.
//!
//! **What the preloads find.** `ShapeBaseData`'s family checks each of the eight collision slots' boxes
//! against the shape's bounds (`shapes::preload`; the two functions hold their own copy of the check): a box
//! outside them becomes the shape's and, when `$Pref::Debug::ShowShapeMessages` is true, `Warning: shape <path> collision
//! detail <slot> (Collision-<detail>) bounds exceed that of shape.` is printed (a warning line); a box inside but inverted (a
//! detail without a mesh) becomes the shape's and `Error: shape <path>-collision detail <slot> (Collision-<detail>) bounds box
//! invalid!` is printed always (an error line). An ordinary shape prints neither, so the engine printing one means a
//! wrong bound. The findings are computed once per shape and printed by each datablock that preloads it, as the check runs per
//! datablock. **The error buffer.** The preloads write their refusal into one static 256-byte array that is
//! never cleared. `ShapeBaseImageData` puts `Unable to load shape: <path>` in it when its shape does not load and `Bad` when
//! its path is empty (TGE writes `Bad Datablock from server` with the size `sizeof(errorBuffer)`, the size of a pointer for an
//! array parameter, so three characters stay), which refuses the datablock; `TSShapeConstructor` imports its `sequence<n>`
//! files into the kept shape ([`Shapes::constructor_preload`]) and leaves the
//! [`DatablockRules`](crate::dbvalues::rules::DatablockRules) text for a file not there or one that does not import, and when
//! its `baseShape` does not load (empty, missing, above version 24, unparsable) it returns false WITHOUT writing the buffer, so
//! its `preload failed` line shows whatever the last failed preload left (`PreloadError::Stale`; empty before any).
//! A shape that does not load is the rules' ([`DatablockRules::shape_not_loaded`](crate::dbvalues::rules::DatablockRules)):
//! TGE's refuse the datablock with `ShapeBaseData: Couldn't load shape "<path>"`.
//!
//! **Cost.** Every datablock's preload is charged its path's bytes; a shape not yet kept is charged one step
//! for the lookup and the bytes of the file (a `stat` first, so a run that cannot pay never makes the host
//! read or inflate it) besides the work the host reports, and then the parse charges the file's bytes again before it
//! allocates, each mesh and table as it reads it, and the facts charge their scans and every vertex of a collision box (all
//! through [`Vm::charge_work`], before the work). An import is charged its file, its parse, and per sequence, name and
//! keyframe. The kept set holds at most [`MAX_KEPT_SHAPES`] paths and [`MAX_KEPT_SHAPE_BYTES`] of estimated model
//! ([`Shape::approx_bytes`]); a shape past either is loaded, and prints, again each time.
//!
//! **`purgeResources()`** frees the kept shapes nothing holds ([`Shapes::purge`]): a datablock holds the shape its preload
//! loaded, an object the one its datablock gave it. `deleteDataBlocks(); purgeResources();` with the mission's markers still
//! there frees nothing (the marker shape does not print its line again); a server that purges after the mission is gone prints
//! the line again when the shape loads next.
//!
//! Credit: this module follows TGE as Torque3D, its MIT-licensed release, keeps it, where the two agree: `ts/tsShape.cpp`,
//! `ts/tsShapeConstruct.cpp`, `T3D/shapeImage.cpp` and `core/resourceManager.cpp` (Copyright (c) 2012 GarageGames, LLC);
//! `T3D/shapeBase.cpp` (Copyright (c) 2012 GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.). The code here is this
//! crate's own.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use shapes::collide::Collision;
use shapes::preload::{Facts, Finding, SLOTS, exceed_line, invalid_line};
use shapes::{Charge, MAX_DSQ_BYTES, MAX_NAME_BYTES, MAX_SHAPE_NAMES, Shape, ShapeError};
use ts_vm::classes::{EngineClass, PreloadError};
use ts_vm::fields::Value;
use ts_vm::fields::atob;
use ts_vm::host::{FileRead, FileStat};
use ts_vm::sim::ObjId;
use ts_vm::text::fold;
use ts_vm::vm::{Native, Vm, VmError, WORK_PER_STEP};

use crate::charge::{charged_host, pay};
use crate::dbvalues::rules::ShapeClass;
use crate::dbvalues::{DbValues, fields, image};
use torque_resource::files::MAX_READ_BYTES;

/// The engine's datablock classes whose server-side `preload` loads a shape, and the field naming it. A host class that
/// loads one gives its class its own behaviour, which calls [`Shapes::load_field`].
pub const SHAPE_FIELDS: [(&str, &str); 6] = [
    ("ShapeBaseData", "shapeFile"),
    ("ShapeBaseImageData", "shapeFile"),
    ("ExplosionData", "explosionShape"),
    ("ProjectileData", "projectileShapeName"),
    ("WheeledVehicleTire", "shapeFile"),
    ("TSShapeConstructor", "baseShape"),
];

/// The newest `.dts` version the loader reads (`TSShape::smVersion`, TGE 1.4).
pub const NEWEST_VERSION: u8 = shapes::dts::NEWEST_VERSION;

/// Versions below this print [`OLD_VERSION_LINE`] (the old-format reader runs below 19).
pub const OLD_BELOW_VERSION: u8 = shapes::dts::OLD_BELOW_VERSION;

/// What an old shape prints (`Con::printf`, a normal line).
pub const OLD_VERSION_LINE: &str = "... Shape with old version.";

/// What a version-24 file the parser refuses prints (the text for a stream that failed; an error line).
pub const BAD_SHAPE_LINE: &str = "Error: bad shape file.";

/// The most shapes the kept set remembers (Torque's resource manager keeps every loaded shape). A shape
/// past it is read again each time it is asked for.
pub const MAX_KEPT_SHAPES: usize = 16_384;

/// The estimated bytes of model the kept set holds: a shape that would pass it is
/// loaded again each time it is asked for, like one past [`MAX_KEPT_SHAPES`].
pub const MAX_KEPT_SHAPE_BYTES: usize = shapes::MAX_KEPT_SHAPE_BYTES;

/// The longest path the loader looks up (Torque builds paths in 1,024-byte buffers; this is the
/// bound the files use for the paths a script may write). A longer path is not a shape the loader finds.
pub const MAX_SHAPE_PATH: usize = 1_023;

/// The `sequence<n>` fields of `TSShapeConstructor` (`sequence0` to `sequence126`: `MaxSequences`, which the preload's loop
/// runs below).
pub const SEQUENCE_FIELDS: usize = 127;

/// The longest `sequence<n>` text the import reads: TGE copies it into a 256-byte buffer (`dSprintf` keeps 255 bytes).
pub const MAX_SEQUENCE_TEXT: usize = 255;

/// The pref that turns the `bounds exceed` warning on (a bool: `Con::getBoolVariable`, `dAtob`).
pub const SHOW_SHAPE_MESSAGES: &str = "$Pref::Debug::ShowShapeMessages";

/// What the resource manager keeps of a loaded shape.
///
/// The parsed [`Shape`] (the runtime model, `Shape::for_server`: no texture coordinates or normals) and what the preloads find
/// in it ([`Facts`]), or neither for a shape the engine reads header-only (versions below 24). A host's mesh queries and
/// manifest collector (the shape's material names) read it through
/// [`Shapes::get`]. A `TSShapeConstructor` replaces the entry with one that has the sequences imported.
#[derive(Debug, Clone, PartialEq)]
pub struct KeptShape {
    /// The low byte of the file's version word.
    pub version: u8,
    /// The file's size in bytes.
    pub bytes: usize,
    /// The parsed shape; `None` for the old shape and for versions 19 to 23, which the engine keeps header-only.
    pub shape: Option<Shape>,
    /// What the preloads find in `shape` (the eight slots, the collision boxes after the check, the nodes).
    pub facts: Option<Facts>,
    /// The estimated bytes this entry takes (what the kept set's total counts).
    pub model_bytes: usize,
    /// True once a `TSShapeConstructor` has imported its sequences into the shape (`getSequencesConstructed`): a second
    /// constructor naming the same `baseShape` imports nothing.
    pub sequences_constructed: bool,
    /// The collision queries of the eight slots ([`shapes::collide::Collision`]), made when the first ray or box reaches the
    /// shape (a mesh's hull is built then too), paid for by the query that makes them. `None` inside for a shape with no body
    /// or no slots to ask.
    collision: std::cell::OnceCell<Option<Rc<Collision>>>,
    /// The hull of each mesh a moving object's ray has reached (`TSMesh::buildConvexHull`, built the first time), and the
    /// accelerator of each collision detail a mover's convex has used: what the container's rays and `collide`'s convexes keep.
    pub hulls: HullCache,
}

/// The per-shape caches of [`KeptShape::hulls`]: made on first use, paid for by the query that makes them. They are derived
/// data, so two kept shapes compare equal whatever their caches hold.
#[derive(Debug, Clone, Default)]
pub struct HullCache {
    pub meshes: RefCell<std::collections::BTreeMap<usize, Rc<shapes::collide::Hull>>>,
    /// The hull of each mesh a hitbox ray has reached ([`shapes::collide::Hull::build_relative`]).
    pub parts: RefCell<std::collections::BTreeMap<usize, Rc<shapes::collide::Hull>>>,
    pub accels: RefCell<std::collections::BTreeMap<i32, Rc<collide::mesh::HullAccel>>>,
    /// The node transforms at rest (an instance made with no thread).
    pub rest: RefCell<Option<Rc<Vec<shapes::math::Matrix>>>>,
    /// The eight collision slots as `collide` takes them ([`crate::scene::container_world::hull_slots`]).
    pub slots: std::cell::OnceCell<Rc<[collide::convex::HullSlot]>>,
    /// The collision hulls made of each detail, by the poses of their parts
    /// ([`crate::scene::container_world::BuiltHulls`]).
    pub built: RefCell<std::collections::BTreeMap<i32, crate::scene::container_world::BuiltHulls>>,
}

impl PartialEq for HullCache {
    fn eq(&self, _: &HullCache) -> bool {
        true
    }
}

impl KeptShape {
    fn sized(
        vm: &Vm,
        version: u8,
        bytes: usize,
        shape: Option<Shape>,
        facts: Option<Facts>,
    ) -> Result<Rc<KeptShape>, VmError> {
        let mut kept = KeptShape {
            version,
            bytes,
            shape,
            facts,
            model_bytes: 0,
            sequences_constructed: false,
            collision: std::cell::OnceCell::new(),
            hulls: HullCache::default(),
        };
        kept.resize(vm)?;
        Ok(Rc::new(kept))
    }

    /// The prepared collision queries of the shape ([`Collision::build`]), built on the first call and paid for through
    /// `charge` (a call that cannot pay builds nothing and leaves the next to pay again). `Ok(None)` for a shape the engine
    /// holds no body for, or one whose slots are all empty.
    pub fn collision(&self, charge: Charge) -> Result<Option<Rc<Collision>>, ShapeError> {
        if let Some(done) = self.collision.get() {
            return Ok(done.clone());
        }
        let built = match (&self.shape, &self.facts) {
            (Some(shape), Some(facts)) => {
                Some(Rc::new(Collision::build(shape, &facts.slots, charge)?))
            }
            _ => None,
        };
        let _ = self.collision.set(built.clone());
        Ok(built)
    }

    /// Counts the entry's estimated bytes again (after a parse or an import grew the shape). The walk over the names, the
    /// sequences' sets and the meshes is paid for first.
    fn resize(&mut self, vm: &Vm) -> Result<(), VmError> {
        if let Some(shape) = &self.shape {
            let walk = 64 + shape.names.len() + 8 * shape.sequences.len() + shape.meshes.len();
            pay(vm, walk as u64)?;
        }
        self.model_bytes = std::mem::size_of::<KeptShape>()
            .saturating_add(self.shape.as_ref().map_or(0, Shape::approx_bytes));
        Ok(())
    }
}

/// What a load found.
#[derive(Debug, Clone)]
pub enum Loaded {
    /// An empty path: nothing to load (a datablock without a shape).
    Nothing,
    /// The load failed (its error line, if there is one, is printed): not a `.dts`, a path too long, a folder the
    /// resource manager refuses, a file that is not there, a version above 24, a version-24 file the parser refuses.
    Failed,
    /// The shape is there but the engine holds no data for it: a file over the read bound or shorter than a version word.
    Unread,
    /// The shape, kept or (past the kept set's bounds) loaded for this call only.
    Shape(Rc<KeptShape>),
}

impl Loaded {
    /// True unless the load failed (the shape is there, or there was nothing to load).
    pub fn is_present(&self) -> bool {
        !matches!(self, Loaded::Failed)
    }
}

/// The shape resource a `ShapeBaseData` loaded at its preload. Torque's resource is one `TSShape` that a `TSShapeConstructor`
/// later changes in place, so every datablock (and every object built on it) sees the imported sequences; the engine replaces
/// the kept entry instead, so the resource remembers the path and reads the entry that is current when an object asks
/// ([`ShapeResource::current`]).
#[derive(Clone)]
pub struct ShapeResource {
    shapes: Rc<Shapes>,
    path: Vec<u8>,
    loaded: Rc<KeptShape>,
}

impl ShapeResource {
    /// The kept shape now: the entry at the path when it has a body, else the one this resource loaded.
    pub fn current(&self) -> Rc<KeptShape> {
        self.shapes
            .get(&self.path)
            .filter(|k| k.shape.is_some())
            .unwrap_or_else(|| Rc::clone(&self.loaded))
    }
}

impl std::fmt::Debug for ShapeResource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ShapeResource")
            .field("path", &String::from_utf8_lossy(&self.path))
            .finish()
    }
}

impl PartialEq for ShapeResource {
    fn eq(&self, other: &ShapeResource) -> bool {
        self.path == other.path && Rc::ptr_eq(&self.loaded, &other.loaded)
    }
}

/// The shapes loaded so far, shared by the behaviours of the shape classes.
#[derive(Debug)]
pub struct Shapes {
    /// The kept shapes by folded path: the resource manager's table.
    kept: RefCell<HashMap<Vec<u8>, Rc<KeptShape>>>,
    /// The sum of the kept entries' `model_bytes`.
    kept_bytes: Cell<usize>,
    /// The `ExplosionData` datablocks whose shape did not load and that the rules kept (they are not listed among the resources
    /// a client is offered). Emptied by
    /// `deleteDataBlocks()`, which resets the ids.
    unlisted: RefCell<std::collections::HashSet<ObjId>>,
    /// The shapes (folded paths) whose engine-limit line was printed.
    limit_reported: RefCell<std::collections::HashSet<Vec<u8>>>,
    /// What `kept_bytes` may reach ([`MAX_KEPT_SHAPE_BYTES`]; a test lowers it).
    byte_limit: Cell<usize>,
    reads: Cell<u64>,
    /// The host's own state ([`Shapes::with_game`]).
    game: RefCell<Option<Box<dyn std::any::Any>>>,
    /// The values the datablocks compute at add and preload time (the packers' view reads them).
    values: DbValues,
}

impl Default for Shapes {
    fn default() -> Shapes {
        Shapes {
            kept: RefCell::default(),
            kept_bytes: Cell::new(0),
            unlisted: RefCell::default(),
            limit_reported: RefCell::default(),
            byte_limit: Cell::new(MAX_KEPT_SHAPE_BYTES),
            game: RefCell::default(),
            reads: Cell::new(0),
            values: DbValues::default(),
        }
    }
}

/// The version word of a shape file's first four bytes.
fn read_version(bytes: &[u8]) -> Option<u8> {
    bytes.first_chunk::<4>().map(|&[version, ..]| version)
}

/// A text static field of datablock `id` (empty when the object is gone or the field has no value).
fn static_text(vm: &Vm, id: ObjId, field: &[u8]) -> Vec<u8> {
    let objects = vm.objects();
    match objects.get(id) {
        Some(object) => object.static_value(&objects.classes, field),
        None => Vec::new(),
    }
}

/// Whether a text static field of datablock `id` was assigned (an empty string included; see `SimObject::static_text_is_set`).
fn static_is_set(vm: &Vm, id: ObjId, field: &[u8]) -> bool {
    let objects = vm.objects();
    objects
        .get(id)
        .is_some_and(|object| object.static_text_is_set(&objects.classes, field))
}

struct ImportEnd {
    /// The buffer text of the last missing file (the others are still imported and the preload fails at the end) or of the load
    /// that failed (it ends the import).
    error: Option<String>,
}

impl Shapes {
    /// Whether the manifest leaves datablock `id` out: an `ExplosionData` whose shape failed to load.
    pub fn is_unlisted(&self, id: ObjId) -> bool {
        self.unlisted.borrow().contains(&id)
    }

    /// Forgets the unlisted datablocks and the computed values (`deleteDataBlocks()` resets the ids).
    pub fn clear_unlisted(&self) {
        self.unlisted.borrow_mut().clear();
        self.values.clear();
    }

    /// The values the datablocks compute at add and preload time.
    pub fn values(&self) -> &DbValues {
        &self.values
    }

    /// How many shapes are kept.
    pub fn kept(&self) -> usize {
        self.kept.borrow().len()
    }

    /// The folded paths of the kept shapes (at most [`MAX_KEPT_SHAPES`]): a test checks that no file whose load failed is among
    /// them.
    pub fn kept_paths(&self) -> Vec<Vec<u8>> {
        self.kept.borrow().keys().cloned().collect()
    }

    /// Lowers the bytes of model the kept set may hold (default [`MAX_KEPT_SHAPE_BYTES`]): the tests' way to meet the bound
    /// without 256 MiB of shapes.
    pub fn set_byte_limit(&self, bytes: usize) {
        self.byte_limit.set(bytes);
    }

    /// The estimated bytes of model the kept shapes take (bounded by [`MAX_KEPT_SHAPE_BYTES`]).
    pub fn kept_bytes(&self) -> usize {
        self.kept_bytes.get()
    }

    /// The kept shape at `path` (found ignoring case, as the resource manager finds it), if the loader kept one: the lookup
    /// the mesh queries and the manifest collector will use.
    pub fn get(&self, path: &[u8]) -> Option<Rc<KeptShape>> {
        self.kept.borrow().get(&fold(path)).cloned()
    }

    /// How many shape files the loader has read in all (a shape kept is read once).
    pub fn reads(&self) -> u64 {
        self.reads.get()
    }

    /// Puts `kept` in the kept set when there is room (a path not kept yet, fewer than [`MAX_KEPT_SHAPES`], and the estimated
    /// bytes within [`MAX_KEPT_SHAPE_BYTES`]); returns it either way.
    fn keep(&self, key: Vec<u8>, kept: Rc<KeptShape>) -> Rc<KeptShape> {
        let mut map = self.kept.borrow_mut();
        let fits = map.len() < MAX_KEPT_SHAPES
            && self.kept_bytes.get().saturating_add(kept.model_bytes) <= self.byte_limit.get();
        if fits && !map.contains_key(&key) {
            self.kept_bytes
                .set(self.kept_bytes.get().saturating_add(kept.model_bytes));
            map.insert(key, Rc::clone(&kept));
        }
        kept
    }

    /// `ResManager::purge`: the kept shapes no datablock or object holds any more are forgotten (a datablock holds
    /// the shape its preload loaded and an object the shape its datablock gave it, `ObjectExt::shape_hold`, header-only ones
    /// too). With the mission's markers still there, `deleteDataBlocks(); purgeResources();` frees nothing of the marker shape.
    pub fn purge(&self) {
        let mut map = self.kept.borrow_mut();
        let mut freed = 0usize;
        map.retain(|_, kept| {
            let keep = Rc::strong_count(kept) > 1;
            if !keep {
                freed = freed.saturating_add(kept.model_bytes);
            }
            keep
        });
        self.kept_bytes
            .set(self.kept_bytes.get().saturating_sub(freed));
    }

    /// Takes `kept` out of the kept set when the set holds this very entry under `key`.
    fn unkeep(&self, key: &[u8], kept: &Rc<KeptShape>) {
        let mut map = self.kept.borrow_mut();
        if map.get(key).is_some_and(|k| Rc::ptr_eq(k, kept)) {
            map.remove(key);
            self.kept_bytes
                .set(self.kept_bytes.get().saturating_sub(kept.model_bytes));
        }
    }

    /// Loads the shape at `path` as a datablock's preload does (a host that loads a shape for another reason, a
    /// mission object's, calls it too): see [`load_shape`](Self::load_shape). True when the shape is there (loaded now, kept,
    /// or a file the engine does not read: over the read bound, shorter than a version word) or there is nothing to load (an
    /// empty path); false when the load failed: a path that is not a `.dts` (or a `.glb` / `.gltf`), one too long, a folder the resource manager
    /// refuses (it prints its error and a BackTrace, as every look-up of such a path does), a file that is not there, a version
    /// above 24 (after its error line), a version-24 file the parser refuses (after [`BAD_SHAPE_LINE`]). What a datablock says
    /// of a failed load is the caller's (`ShapeLoad`). Errors are the run's own (`StepLimit`).
    pub fn load(&self, vm: &mut Vm, path: &[u8]) -> Result<bool, VmError> {
        Ok(self.load_shape(vm, path)?.is_present())
    }

    /// Loads the shape at `path`: nothing for a path that is empty or kept; otherwise one `stat`, the payment, one `read`, the
    /// version word and, for version 24, the parse and the facts (charged before the work). The kept shapes are found by the
    /// path ignoring case (a datablock's `Filename` field already keeps one spelling, the first, so the datablocks never show
    /// the difference). No borrow of the kept set is held while the VM prints.
    pub fn load_shape(&self, vm: &mut Vm, path: &[u8]) -> Result<Loaded, VmError> {
        if path.is_empty() {
            return Ok(Loaded::Nothing);
        }
        // The path is scanned for its extension and folded: its bytes are the work.
        pay(vm, path.len() as u64)?;
        if path.len() > MAX_SHAPE_PATH {
            return Ok(Loaded::Failed);
        }
        let key = fold(path);
        if !key.ends_with(b".dts") && !torque_import::native::is_gltf_path(&key) {
            return Ok(Loaded::Failed);
        }
        let known = self.kept.borrow().get(&key).cloned();
        if let Some(kept) = known {
            return Ok(Loaded::Shape(kept));
        }
        let len = match charged_host(vm, |host| host.stat(path))? {
            FileStat::Found { len } => len,
            FileStat::Missing => return Ok(Loaded::Failed),
            FileStat::Refused(refusal) => {
                vm.report_refusal(path, refusal)?;
                return Ok(Loaded::Failed);
            }
        };
        if len > MAX_READ_BYTES {
            return Ok(Loaded::Unread);
        }
        // The file is read whole by the host: pay for it before it is asked.
        pay(vm, WORK_PER_STEP.saturating_add(len))?;
        let FileRead::Found(bytes) = charged_host(vm, |host| host.read(path))? else {
            return Ok(Loaded::Unread);
        };
        self.reads.set(self.reads.get().saturating_add(1));
        self.finish_load(vm, key, bytes)
    }

    /// What [`load_shape`](Self::load_shape) does with the bytes of a shape file it read: the version, the parse, the facts,
    /// the kept set. A function of its own so that its frame is not on the stack while the host inflates the file (the deepest
    /// point of a load from a zip).
    #[inline(never)]
    fn finish_load(&self, vm: &mut Vm, key: Vec<u8>, bytes: Vec<u8>) -> Result<Loaded, VmError> {
        let size = bytes.len();
        // A glTF is read as the `.dts` it makes (`torque_import::native`), paid for through the same budget.
        let bytes = match torque_import::native::shape_file(&key, bytes, &mut |units| {
            vm.charge_work(units)
        }) {
            Ok(bytes) => bytes,
            Err(torque_import::ImportError::OutOfBudget) => return Err(VmError::StepLimit),
            Err(_) => {
                vm.error(BAD_SHAPE_LINE.to_string());
                return Ok(Loaded::Failed);
            }
        };
        let Some(version) = read_version(&bytes) else {
            return Ok(Loaded::Unread);
        };
        if version > NEWEST_VERSION {
            vm.error(format!(
                "Error: attempt to load a version {version} dts-shape, can currently only load version {NEWEST_VERSION} and before."
            ));
            return Ok(Loaded::Failed);
        }
        if version < OLD_BELOW_VERSION {
            vm.print(OLD_VERSION_LINE.as_bytes());
        }
        let (shape, facts) = if version == NEWEST_VERSION {
            match Self::parse(vm, &bytes) {
                Ok(parsed) => parsed,
                Err(ShapeError::OutOfBudget) => return Err(VmError::StepLimit),
                Err(_) => {
                    vm.error(BAD_SHAPE_LINE.to_string());
                    return Ok(Loaded::Failed);
                }
            }
        } else {
            (None, None)
        };
        drop(bytes);
        let kept = KeptShape::sized(vm, version, size, shape, facts)?;
        Ok(Loaded::Shape(self.keep(key, kept)))
    }

    /// The parse of a version-24 file and the facts of the shape, each paid for through the VM's budget before its work.
    #[inline(never)]
    fn parse(vm: &Vm, bytes: &[u8]) -> Result<(Option<Shape>, Option<Facts>), ShapeError> {
        let model = shapes::dts::parse(bytes, &mut |units| vm.charge_work(units))?;
        let shape = Shape::for_server(model);
        let rule = crate::sim_classes::sim_classes(vm).shape_slots();
        let facts = shapes::preload::facts_with(&shape, rule, &mut |units| vm.charge_work(units))?;
        Ok((Some(shape), Some(facts)))
    }

    /// Loads the shape the static field `field` of datablock `id` names ([`load`](Self::load)): what the preload of a
    /// shape class does, for a class whose behaviour does more than that. Nothing for an object that is gone or a field it has
    /// no value for.
    pub fn load_field(&self, vm: &mut Vm, id: ObjId, field: &[u8]) -> Result<bool, VmError> {
        Ok(self.load_named(vm, id, field)?.1.is_present())
    }

    /// [`load_shape`](Self::load_shape) of a field's path, with the path too (for the message of a failure).
    pub fn load_named(
        &self,
        vm: &mut Vm,
        id: ObjId,
        field: &[u8],
    ) -> Result<(Vec<u8>, Loaded), VmError> {
        let path = static_text(vm, id, field);
        let loaded = self.load_shape(vm, &path)?;
        Ok((path, loaded))
    }

    /// Whether the kept set holds `kept` under `path`.
    pub fn kept_holds(&self, path: &[u8], kept: &Rc<KeptShape>) -> bool {
        self.kept
            .borrow()
            .get(&fold(path))
            .is_some_and(|k| Rc::ptr_eq(k, kept))
    }

    /// What the kept set's bytes may reach ([`MAX_KEPT_SHAPE_BYTES`] unless a test lowered it).
    pub fn byte_limit(&self) -> usize {
        self.byte_limit.get()
    }

    /// True the first time it is asked about `path`: whether its engine-limit line is still to be printed.
    pub fn first_limit_report(&self, path: &[u8]) -> bool {
        self.limit_reported.borrow_mut().insert(fold(path))
    }

    /// The host's own state kept with the shapes (one value of the host's type, made as its default on first use).
    pub fn with_game<T: Default + 'static, R>(&self, f: impl FnOnce(&mut T) -> R) -> R {
        let mut slot = self.game.borrow_mut();
        let state = slot.get_or_insert_with(|| Box::new(T::default()));
        match state.downcast_mut::<T>() {
            Some(state) => f(state),
            None => f(&mut T::default()),
        }
    }

    /// `TSShapeConstructor::preload`'s import (TGE 1.4 `tsShapeConstruct.cc`): into the kept shape `kept`, loaded from `base`,
    /// the `.dsq` files of `sequence0`, `sequence1`, ... skipping the ones never assigned (each cut at its first space or tab,
    /// the text after it renaming the file's last sequence), by [`Shape::import_sequences`] (atomic: a refused file leaves the
    /// shape as it was). Nothing for a shape already constructed, or kept header-only. A file that is not there leaves the
    /// rules' [`missing_sequence`](crate::dbvalues::rules::DatablockRules::missing_sequence) text in the error buffer, the
    /// import goes on and the preload fails at the end with the last text; a file that does not import leaves
    /// [`load_sequence_failed`](crate::dbvalues::rules::DatablockRules::load_sequence_failed)'s and ends it. The shape is taken
    /// out of the kept set while it changes (no borrow is held while the run pays) and put back: the entry then counts its new
    /// size, and a shape that grew past the kept set's bounds is not kept (the next constructor loads it again). Every file
    /// read, parse and import is charged before it is done.
    pub fn constructor_preload(
        &self,
        vm: &mut Vm,
        id: ObjId,
        base: &[u8],
        kept: Rc<KeptShape>,
    ) -> Result<(), PreloadError> {
        if kept.sequences_constructed || kept.shape.is_none() {
            return Ok(());
        }
        let key = fold(base);
        // A shape someone else still holds is copied; the copy (and the new size count) is paid for before anything changes.
        pay(vm, kept.model_bytes as u64).map_err(PreloadError::Vm)?;
        self.unkeep(&key, &kept);
        let mut work = Rc::try_unwrap(kept).unwrap_or_else(|shared| (*shared).clone());
        let ended = match work.shape.as_mut() {
            Some(shape) => Self::import_sequences(vm, id, shape),
            None => Ok(ImportEnd { error: None }),
        };
        if let Ok(end) = &ended {
            work.sequences_constructed = end.error.is_none();
        }
        let sized = work.resize(vm);
        self.keep(key, Rc::new(work));
        sized.map_err(PreloadError::Vm)?;
        match ended {
            Ok(ImportEnd { error: None }) => Ok(()),
            Ok(ImportEnd { error: Some(text) }) => Err(PreloadError::Failed(text)),
            Err(e) => Err(PreloadError::Vm(e)),
        }
    }

    /// The loop of [`constructor_preload`](Self::constructor_preload) over the `sequence<n>` fields of datablock `id`.
    #[inline(never)]
    fn import_sequences(vm: &mut Vm, id: ObjId, shape: &mut Shape) -> Result<ImportEnd, VmError> {
        let mut error = None;
        let stops_at_unset = crate::dbvalues::rules::rules(vm).constructor_stops_at_unset();
        for i in 0..SEQUENCE_FIELDS {
            let field = format!("sequence{i}");
            // An entry that is NULL, a field never assigned, is skipped (or ends the loop, by the rules); `sequence0 = "";`
            // stores an empty entry, which the loader tries to open as a file (and fails to).
            if !static_is_set(vm, id, field.as_bytes()) {
                if stops_at_unset {
                    break;
                }
                continue;
            }
            let value = static_text(vm, id, field.as_bytes());
            pay(vm, value.len() as u64 + WORK_PER_STEP)?;
            let (file, name) = split_sequence(&value);
            let shape_name = static_text(vm, id, b"baseShape");
            let rules = crate::dbvalues::rules::rules(vm);
            let bytes = match fetch_sequence_file(vm, file)? {
                Fetched::Bytes(bytes) => bytes,
                Fetched::Missing => {
                    error = Some(rules.missing_sequence(&value, &shape_name));
                    continue;
                }
                Fetched::TooBig => {
                    return Ok(ImportEnd {
                        error: Some(rules.load_sequence_failed(&value, &shape_name)),
                    });
                }
            };
            let sequences_before = shape.sequences.len();
            let imported = shapes::dsq::parse(&bytes, &mut |units| vm.charge_work(units))
                .and_then(|dsq| shape.import_sequences(&dsq, &mut |units| vm.charge_work(units)));
            drop(bytes);
            match imported {
                Ok(()) => {}
                Err(ShapeError::OutOfBudget) => return Err(VmError::StepLimit),
                Err(_) => {
                    return Ok(ImportEnd {
                        error: Some(rules.load_sequence_failed(&value, &shape_name)),
                    });
                }
            }
            // A space or tab in the text names the file's last sequence anew (the old name stays in the table). TGE renames the
            // shape's last sequence whatever the file brought; a file that added none is the engine's choice: it renames
            // nothing.
            if let Some(name) = name
                && shape.sequences.len() > sequences_before
            {
                pay(vm, name.len() as u64 + shape.names.len() as u64)?;
                let index = match shape.find_name(name) {
                    Some(index) => Some(index),
                    None if shape.names.len() < MAX_SHAPE_NAMES && name.len() <= MAX_NAME_BYTES => {
                        shape.names.push(name.to_vec());
                        i32::try_from(shape.names.len() - 1).ok()
                    }
                    None => None,
                };
                match (index, shape.sequences.last_mut()) {
                    (Some(index), Some(last)) => last.name = index,
                    _ => {
                        return Ok(ImportEnd {
                            error: Some(rules.load_sequence_failed(&value, &shape_name)),
                        });
                    }
                }
            }
        }
        Ok(ImportEnd { error })
    }
}

/// A `.dsq` file as the import found it.
enum Fetched {
    /// Not there, refused, or not readable.
    Missing,
    /// Longer than [`MAX_DSQ_BYTES`]: not read.
    TooBig,
    Bytes(Vec<u8>),
}

/// The file of a `sequence<n>` text: one `stat`, the payment for its bytes before the read, one `read`. A function of its own
/// so that the frame of the loop is not the one the host inflates a zip entry under.
#[inline(never)]
fn fetch_sequence_file(vm: &mut Vm, file: &[u8]) -> Result<Fetched, VmError> {
    let len = match charged_host(vm, |host| host.stat(file))? {
        FileStat::Found { len } => len,
        FileStat::Missing => return Ok(Fetched::Missing),
        FileStat::Refused(refusal) => {
            vm.report_refusal(file, refusal)?;
            return Ok(Fetched::Missing);
        }
    };
    if len > MAX_DSQ_BYTES as u64 {
        return Ok(Fetched::TooBig);
    }
    pay(vm, WORK_PER_STEP.saturating_add(len))?;
    match charged_host(vm, |host| host.read(file))? {
        FileRead::Found(bytes) => Ok(Fetched::Bytes(bytes)),
        _ => Ok(Fetched::Missing),
    }
}

/// The file name and the new sequence name of a `sequence<n>` text, as TGE cuts it: the copy is at most
/// [`MAX_SEQUENCE_TEXT`] bytes; the file ends at the first space or tab; the name starts after the later of the first space and
/// the first tab (when both are there), past the spaces and tabs that follow. `None` when the text has neither.
fn split_sequence(value: &[u8]) -> (&[u8], Option<&[u8]>) {
    let text = &value[..value.len().min(MAX_SEQUENCE_TEXT)];
    let space = text.iter().position(|&b| b == b' ');
    let tab = text.iter().position(|&b| b == b'\t');
    let (first, later) = match (space, tab) {
        (Some(s), Some(t)) => (s.min(t), s.max(t)),
        (Some(p), None) | (None, Some(p)) => (p, p),
        (None, None) => return (text, None),
    };
    let mut start = later + 1;
    while text.get(start).is_some_and(|&b| b == b' ' || b == b'\t') {
        start += 1;
    }
    (&text[..first], Some(&text[start.min(text.len())..]))
}

/// What the check of a shape's collision boxes prints for a datablock that names it as `name` (see the module doc): the slots
/// in order, the warning only when `$Pref::Debug::ShowShapeMessages` is true, the error always.
pub fn report_findings(vm: &mut Vm, name: &[u8], facts: &Facts) {
    // The entries the check made (at most `SLOTS`: a shape with fewer collision details has fewer).
    let entries = facts
        .findings
        .iter()
        .zip(&facts.slots.collision)
        .take(SLOTS);
    for (slot, (finding, &detail)) in entries.enumerate() {
        match finding {
            Some(Finding::Exceeds) => {
                if vm.global(SHOW_SHAPE_MESSAGES).is_some_and(|v| atob(&v)) {
                    vm.warn(exceed_line(name, slot, detail));
                }
            }
            Some(Finding::Invalid) => vm.error(invalid_line(name, slot, detail)),
            Some(Finding::Inside) | None => {}
        }
    }
}

/// What a datablock does when its shape did not load (the rules' [`ShapeClass`] answer, or the class's own).
#[derive(Clone, Copy)]
enum OnFailure {
    /// Nothing (the classes that print no message here).
    Silent,
    /// `ShapeBaseData`: the rules' answer ([`ShapeClass::ShapeBase`]), and when it loaded, the findings of the bounds check.
    ShapeBase,
    /// `ExplosionData`: the rules' answer ([`ShapeClass::Explosion`]); a datablock they keep has no shape.
    CouldntLoad,
    /// `ShapeBaseImageData`: the error buffer `Unable to load shape: <path>` (an empty path: `Bad`) refuses the datablock.
    Image,
    /// `WheeledVehicleTire`: a shape that does not load is the rules' ([`ShapeClass::Tire`]); one that does sets `radius` to
    /// half the height of its bounds.
    Tire,
    /// `TSShapeConstructor`: refused with the stale buffer when `baseShape` does not load; when it does, the sequences are
    /// imported.
    Constructor,
}

/// The behaviour of one shape class: its preload loads the shape its field names. `on_add` is another behaviour
/// of the same class (a class has one), run first.
struct ShapeLoad {
    field: &'static [u8],
    shapes: Rc<Shapes>,
    on_failure: OnFailure,
    on_add: Option<Rc<dyn EngineClass>>,
}

impl EngineClass for ShapeLoad {
    fn on_add(&self, vm: &mut Vm, id: ObjId) -> Result<bool, VmError> {
        match &self.on_add {
            Some(behaviour) => behaviour.on_add(vm, id),
            None => Ok(true),
        }
    }

    fn on_remove(&self, vm: &mut Vm, id: ObjId) -> Result<(), VmError> {
        match &self.on_add {
            Some(behaviour) => behaviour.on_remove(vm, id),
            None => Ok(()),
        }
    }

    fn preload(&self, vm: &mut Vm, id: ObjId) -> Result<(), PreloadError> {
        let (path, loaded) = self
            .shapes
            .load_named(vm, id, self.field)
            .map_err(PreloadError::Vm)?;
        self.after_load(vm, id, &path, loaded)
    }
}

/// Whether datablock `id` is a `PlayerData` or a class below it.
fn is_player_data(vm: &Vm, id: ObjId) -> bool {
    let objects = vm.objects();
    let (Some(player), Some(object)) = (objects.classes.find(b"PlayerData"), objects.get(id))
    else {
        return false;
    };
    objects.classes.is_a(object.class(), player)
}

impl ShapeLoad {
    /// What the class does once its shape is loaded or failed: a function of its own so that its frame (the formatted texts) is
    /// not on the stack while the host inflates the file.
    #[inline(never)]
    fn after_load(
        &self,
        vm: &mut Vm,
        id: ObjId,
        path: &[u8],
        loaded: Loaded,
    ) -> Result<(), PreloadError> {
        let shown = String::from_utf8_lossy(path).into_owned();
        // Every shape-carrying datablock holds the resource it loaded (a `Resource<TSShape>` member of each class), so
        // `purgeResources` frees no shape a datablock still has.
        if let Loaded::Shape(kept) = &loaded
            && let Some(e) = crate::scene::ext::get_mut(vm.objects_mut(), id)
        {
            e.shape_hold = Some(Rc::clone(kept));
        }
        match self.on_failure {
            OnFailure::Silent => {}
            OnFailure::ShapeBase => {
                // The datablock holds the resource it loaded, whatever the field says later: the objects built on it
                // (`ShapeBase::onNewDataBlock`) make their shape instance from this one.
                let resource = match &loaded {
                    Loaded::Shape(kept) => Some(ShapeResource {
                        shapes: Rc::clone(&self.shapes),
                        path: path.to_vec(),
                        loaded: Rc::clone(kept),
                    }),
                    _ => None,
                };
                if let Some(e) = crate::scene::ext::get_mut(vm.objects_mut(), id) {
                    e.shape_resource = resource;
                }
                if !loaded.is_present() {
                    crate::dbvalues::rules::rules(vm)
                        .shape_not_loaded(vm, ShapeClass::ShapeBase, &shown)
                        .map_err(PreloadError::Failed)?;
                } else if let Loaded::Shape(kept) = &loaded
                    && let Some(facts) = &kept.facts
                {
                    report_findings(vm, path, facts);
                }
                // `PlayerData::preload` returns false when the shape resource is null, a missing file and an
                // empty `shapeFile` alike, without writing the error buffer; the datablock is refused with whatever the buffer
                // held (`preload failed for <name>: .` before any other failure). `VehicleData::preload` has no such check.
                // Its jump energy and delay are clamped BEFORE that check (the delay line prints even for a datablock then
                // refused); the maximum jump speed and the pickup radius are inside the shape branch.
                if is_player_data(vm, id) {
                    crate::dbvalues::rules::rules(vm).player_preload_clamps(vm, id);
                    if matches!(loaded, Loaded::Failed | Loaded::Nothing) {
                        return Err(PreloadError::Stale);
                    }
                    crate::dbvalues::rules::rules(vm).player_shape_clamps(vm, id);
                }
            }
            OnFailure::CouldntLoad => {
                if loaded.is_present() {
                    self.shapes.unlisted.borrow_mut().remove(&id);
                } else {
                    crate::dbvalues::rules::rules(vm)
                        .shape_not_loaded(vm, ShapeClass::Explosion, &shown)
                        .map_err(PreloadError::Failed)?;
                    // Kept with no shape: the member is NULL: the packed shape name is empty and a script reads "".
                    fields::unset(vm, id, "explosionShape", 0);
                    self.shapes.unlisted.borrow_mut().insert(id);
                }
            }
            OnFailure::Image => {
                if path.is_empty() {
                    // `dSprintf(errorBuffer, sizeof(errorBuffer), ...)`: the size of a pointer, three characters stay.
                    return Err(PreloadError::Failed("Bad".to_string()));
                }
                if !loaded.is_present() {
                    return Err(PreloadError::Failed(format!(
                        "Unable to load shape: {shown}"
                    )));
                }
                let shape = match &loaded {
                    Loaded::Shape(kept) => kept.shape.as_ref(),
                    _ => None,
                };
                image::preload(&self.shapes.values, vm, id, shape);
            }
            OnFailure::Tire => match &loaded {
                Loaded::Failed => {
                    crate::dbvalues::rules::rules(vm)
                        .shape_not_loaded(vm, ShapeClass::Tire, &shown)
                        .map_err(PreloadError::Failed)?;
                }
                Loaded::Shape(kept) => {
                    if let Some(shape) = &kept.shape {
                        let radius = crate::dbvalues::rules::rules(vm).tire_radius(&shape.bounds);
                        fields::put(vm, id, "radius", 0, Value::Float(radius));
                    }
                }
                Loaded::Nothing | Loaded::Unread => {}
            },
            OnFailure::Constructor => match loaded {
                Loaded::Shape(kept) => self.shapes.constructor_preload(vm, id, path, kept)?,
                Loaded::Unread => {}
                // A resource that did not load returns false without writing the buffer: the line shows what
                // the last failed preload left in it.
                Loaded::Nothing | Loaded::Failed => return Err(PreloadError::Stale),
            },
        }
        Ok(())
    }
}

/// The shapes a script engine's loader keeps, as its extension.
struct Installed(Rc<Shapes>);

/// The loader [`register`] gave `vm`, for a plugin that loads a shape of its own (a datablock of its own class) through the
/// same kept set, paid for by the same budget.
pub fn shapes_of(vm: &Vm) -> Option<Rc<Shapes>> {
    vm.extension::<Installed>().map(|i| Rc::clone(&i.0))
}

/// Gives the shape classes their loader (after the host's class table is installed) and returns the shapes it keeps
/// ([`shapes_of`] finds them again). Registering again replaces the behaviours and starts a new set.
pub fn register(vm: &mut Vm) -> Rc<Shapes> {
    let shapes = Rc::new(Shapes::default());
    vm.set_extension(Rc::new(Installed(Rc::clone(&shapes))));
    crate::dbvalues::register(vm, &shapes.values);
    for (class, field) in SHAPE_FIELDS {
        let (on_failure, on_add): (OnFailure, Option<Rc<dyn EngineClass>>) = match class {
            "ShapeBaseData" => (OnFailure::ShapeBase, None),
            "ExplosionData" => (
                OnFailure::CouldntLoad,
                Some(crate::dbvalues::rules::rules(vm).explosion()),
            ),
            "ShapeBaseImageData" => (
                OnFailure::Image,
                Some(Rc::new(image::ImageDataAdd {
                    values: shapes.values.clone(),
                })),
            ),
            "ProjectileData" => (
                OnFailure::Silent,
                crate::dbvalues::rules::rules(vm).projectile(),
            ),
            "WheeledVehicleTire" => (OnFailure::Tire, None),
            "TSShapeConstructor" => (OnFailure::Constructor, None),
            _ => (OnFailure::Silent, None),
        };
        vm.set_class_behaviour(
            class,
            Rc::new(ShapeLoad {
                field: field.as_bytes(),
                shapes: Rc::clone(&shapes),
                on_failure,
                on_add,
            }),
        );
    }
    // `purgeResources` (TGE `ResManager::purge`): every resource no one holds any more is freed, so a shape loaded again after
    // it is read again (and an old one prints its line again: a server that purges after the mission and the datablocks are
    // gone).
    let held = Rc::clone(&shapes);
    crate::charge::register_charged(
        vm,
        "purgeResources",
        1,
        "Purge resources from the resource manager.",
        move |_, _| {
            held.purge();
            Native::Void
        },
    );
    shapes
}

#[cfg(test)]
impl KeptShape {
    /// A kept shape holding `shape` as parsed, with nothing found in it.
    pub(crate) fn holding(shape: Shape) -> KeptShape {
        KeptShape {
            version: 24,
            bytes: 0,
            shape: Some(shape),
            facts: None,
            model_bytes: 0,
            sequences_constructed: false,
            collision: std::cell::OnceCell::new(),
            hulls: HullCache::default(),
        }
    }
}

#[cfg(test)]
#[path = "shapes_tests.rs"]
mod tests;
