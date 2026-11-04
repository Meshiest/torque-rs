//! A Player's eye as TGE's client places it: `Player::getEyeTransform` and its render twin take the translation of the shape's
//! `eye` node at the animated pose (scaled by the object's scale) and the rotation from the head (`EulerF(head.x, 0, 0)`, times
//! `head.z` about z), then the object's transform. The first-person view and GuiShapeNameHud's label (TGE 1.4's
//! `getRenderEyeTransform` for a Player) both stand there.
//!
//! The shape is the ghost's datablock's `shapeFile`, read from the install only ([`crate::conn::read_install`]: a plain relative path),
//! parsed and given its constructor's sequences once per shape (by the path lower-cased with `\` as `/`, as the install's
//! reader matches it, and the constructor's sequence names; at most [`MAX_SHAPES`] and [`MAX_SHAPE_CACHE_BYTES`] kept, the least
//! recently used dropped first), and posed as the renderer poses a Player ([`PoseView::pose`]: the game's renderer, over the record
//! world [`ghost_world::ClientWorld`]) for each eye asked. A shape without an `eye` node, or none that loads, gives `None`: the callers
//! keep [`crate::predict::PLAYER_EYE_Z`] over the position.
//!
//! **The posed eyes** are kept by shape, arm, action and pitch ([`MAX_EYES`], least recently used dropped first).
//! The pitch stays in the key: the eye is the `eye` node at the pose, and the pose's head and arm threads follow the pitch. It is
//! bounded: another Player's pitch comes as 6 bits (`writeSignedFloat(head.x / maxLookAngle, 6)` in the Player's update), so a
//! shape, arm and action have at most 64 eyes, and the client's own continuous pitch takes one entry a tick from the least recently
//! used. [`clear`] forgets both on each connection (a new server's datablocks name other shapes and constructors).
//!
//! Credit: follows TGE's Player eye as Torque3D carries it (MIT: `T3D/player.cpp`, Copyright (c) 2012 GarageGames, LLC; Copyright
//! (C) 2015 Faust Logic, Inc.).

use std::cell::RefCell;
use std::rc::Rc;

use images::lru::Lru;
use shapes::instance::ShapeInstance;
use shapes::shape::Shape;

use crate::render::{PoseView, RecordOf};

/// The most shapes kept (a hostile server can name many datablocks; each costs a parse once while it is kept).
pub const MAX_SHAPES: usize = 256;
/// The most bytes the kept shapes hold (each its `Shape::approx_bytes` and an entry's cost).
pub const MAX_SHAPE_CACHE_BYTES: usize = 128 << 20;
/// The largest shape file read for an eye.
pub const MAX_SHAPE_BYTES: usize = 8 << 20;

/// A shape file (or a sequence file a constructor names) as the eye reads it: from the install only, at most [`MAX_SHAPE_BYTES`].
pub fn read_shape(path: &str) -> Option<Vec<u8>> {
    crate::net::conn::read_install(path.as_bytes()).filter(|b| b.len() <= MAX_SHAPE_BYTES)
}

/// A shape's key: its path as the install's reader matches it (lower case, `\` as `/`).
fn shape_key(path: &str) -> String {
    path.replace('\\', "/").to_ascii_lowercase()
}

/// A shape loaded for its eye, and its `eye` node.
struct EyeShape {
    shape: Shape,
    eye: usize,
}

/// The most posed eyes kept (by shape, arm, action and pitch to a thousandth): every named Player is asked for each tick.
pub const MAX_EYES: usize = 4096;

/// A shape's key: its path ([`shape_key`]) and its constructor's sequence names.
type ShapeKey = (String, Option<Vec<Vec<u8>>>);
/// A posed eye's key: the shape's, the pitch in thousandths, the record's arm and action.
type EyeKey = (ShapeKey, i32, Option<u32>, Option<u32>, bool);

const SHAPE_ENTRY_COST: usize = 256;

thread_local! {
    static SHAPES: RefCell<Lru<ShapeKey, Option<Rc<EyeShape>>>> =
        RefCell::new(Lru::new(MAX_SHAPES, MAX_SHAPE_CACHE_BYTES));
    static EYES: RefCell<Lru<EyeKey, Option<[f32; 3]>>> = RefCell::new(Lru::new(MAX_EYES, usize::MAX));
    /// How many times a shape was parsed (the tests' count).
    static PARSES: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    /// The last live eye ([`live_eye_offset`]) and what it was posed from: asked again in the same frame with the same inputs (the
    /// pass's view, then the drawn frame's), it is not posed again.
    static LAST_LIVE: std::cell::Cell<Option<(LiveKey, Option<[f32; 3]>)>> = const { std::cell::Cell::new(None) };
    /// How many times a live eye was posed (the tests' count).
    static LIVE_POSES: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// A live eye's inputs: the ghost, its datablock, the frame's clock, the pitch's bits, the crouch flag and the record's arm.
type LiveKey = (u32, u32, u32, u32, bool, Option<u32>);

/// Forgets the kept shapes and eyes (each connection starts with none; a new server's datablocks and constructors differ).
pub fn clear() {
    SHAPES.with(|m| m.borrow_mut().clear());
    EYES.with(|m| m.borrow_mut().clear());
    LAST_LIVE.with(|l| l.set(None));
}

/// The live eyes posed so far on this thread.
pub fn live_poses() -> u64 {
    LIVE_POSES.with(|p| p.get())
}

/// The shapes parsed so far on this thread.
pub fn parses() -> u64 {
    PARSES.with(|p| p.get())
}

fn load<W: PoseView>(world: &W, key: &ShapeKey, path: &str) -> Option<Rc<EyeShape>> {
    if let Some(found) = SHAPES.with(|m| m.borrow_mut().get(key).cloned()) {
        return found;
    }
    let loaded = (|| {
        let bytes = read_shape(path)?;
        PARSES.with(|p| p.set(p.get() + 1));
        let mut charge = |_: u64| true;
        let mut model = shapes::dts::parse(&bytes, &mut charge).ok()?;
        let dir = path.rsplit_once('/').map_or("", |(d, _)| d).to_string();
        if let Some(seqs) = &key.1 {
            world.import_sequences(&mut model.shape, &dir, seqs);
        }
        let shape = model.shape;
        let eye = shape.nodes.iter().position(|n| {
            usize::try_from(n.name)
                .ok()
                .and_then(|i| shape.names.get(i))
                .is_some_and(|name| name.eq_ignore_ascii_case(b"eye"))
        })?;
        Some(Rc::new(EyeShape { shape, eye }))
    })();
    let cost =
        SHAPE_ENTRY_COST + key.0.len() + loaded.as_ref().map_or(0, |s| s.shape.approx_bytes());
    SHAPES.with(|m| m.borrow_mut().insert(key.clone(), loaded.clone(), cost));
    loaded
}

/// The eye node's translation in the object's space (before its scale) for a ghost of datablock `db` posed by `record` with head
/// pitch `pitch`, crouched or not (the crouch thread moves the eye; the client's own Player's flag is its prediction's):
/// `None` when the datablock names no shape with an `eye` node.
pub fn eye_offset<W: PoseView>(
    world: &W,
    db: u32,
    record: Option<&RecordOf<W>>,
    pitch: f32,
    crouched: bool,
) -> Option<[f32; 3]> {
    let file = world.datablock_text(db, "shapeFile")?;
    let path = String::from_utf8_lossy(file).replace('\\', "/");
    let inputs = record
        .and_then(|r| world.pose_inputs(r))
        .unwrap_or_default();
    let (arm, action) = (inputs.arm, inputs.action);
    let shape: ShapeKey = (shape_key(&path), world.constructor_sequences(&path));
    let key: EyeKey = (
        shape,
        (pitch * 1000.0).round() as i32,
        arm,
        action,
        crouched,
    );
    if let Some(found) = EYES.with(|m| m.borrow_mut().get(&key).copied()) {
        return found;
    }
    let eye = posed_eye(world, &key.0, &path, record, pitch, crouched);
    EYES.with(|m| m.borrow_mut().insert(key, eye, 1));
    eye
}

/// The eye node's translation in the object's space for ghost `index` as the live client poses it this frame
/// ([`PoseView::pose_live`], so the eye moves with the crouch thread and the action's transition as the body does; the eye is the
/// `eye` node of the pose its last animate left, `getEyeTransform`). Without the live clock (a world replayed from a record) or a live pose,
/// [`eye_offset`] with the ghost's crouch flag. Not kept: the pose changes from frame to frame.
pub fn live_eye_offset<W: PoseView>(
    world: &W,
    index: u32,
    db: u32,
    record: Option<&RecordOf<W>>,
    pitch: f32,
) -> Option<[f32; 3]> {
    let crouched = world.crouched(index);
    let arm = record
        .and_then(|r| world.pose_inputs(r))
        .and_then(|p| p.arm);
    // The live pose moves only when the frame's clock does ([`PoseView::pose_live`]), and the eye's pose reads only the
    // pitch, the crouch flag and the record's arm over it.
    let key: LiveKey = (index, db, world.anim_ms, pitch.to_bits(), crouched, arm);
    if world.animate
        && record.is_some()
        && let Some((k, eye)) = LAST_LIVE.with(|l| l.get())
        && k == key
    {
        return eye;
    }
    let live = world.animate.then_some(()).and(record).and_then(|r| {
        LIVE_POSES.with(|p| p.set(p.get() + 1));
        let file = world.datablock_text(db, "shapeFile")?;
        let path = String::from_utf8_lossy(file).replace('\\', "/");
        let key: ShapeKey = (shape_key(&path), world.constructor_sequences(&path));
        let shape = load(world, &key, &path)?;
        let instance = world.pose_live(index, db, &shape.shape, r, pitch)?;
        let n = instance.node_transform(shape.eye)?;
        Some([n[3], n[7], n[11]])
    });
    let eye = live.or_else(|| eye_offset(world, db, record, pitch, crouched));
    if world.animate && record.is_some() {
        LAST_LIVE.with(|l| l.set(Some((key, eye))));
    }
    eye
}

/// The render transform of ghost `index` riding a mount, as the frame draws it ([`PoseView::mount_render`]: the mount's `mount<N>`
/// node turned by the Player's own yaw), its shapes read from the install as the eye's are.
pub fn mount_render<W: PoseView>(world: &W, index: u32) -> Option<[f32; 16]> {
    world.mount_render(index)
}

fn posed_eye<W: PoseView>(
    world: &W,
    key: &ShapeKey,
    path: &str,
    record: Option<&RecordOf<W>>,
    pitch: f32,
    crouched: bool,
) -> Option<[f32; 3]> {
    let shape = load(world, key, path)?;
    let mut charge = |_: u64| true;
    let mut instance = ShapeInstance::new(&shape.shape, &mut charge).ok()?;
    match record.filter(|r| world.pose_inputs(r).is_some()) {
        Some(r) => world.pose(&shape.shape, &mut instance, r, pitch, crouched),
        None => {
            instance.animate(&shape.shape, 0, None, &mut charge).ok()?;
        }
    }
    let n = instance.node_transform(shape.eye)?;
    Some([n[3], n[7], n[11]])
}

/// The world point of the eye: the object's transform (`yaw` about z at `position`, scale `scale`) applied to the eye node's
/// translation scaled by `scale` (`getEyeTransform`'s three products).
pub fn eye_point(position: [f32; 3], yaw: f32, scale: [f32; 3], offset: [f32; 3]) -> [f32; 3] {
    let m = collide::math::set_euler([0.0, 0.0, yaw]);
    let local = [
        offset[0] * scale[0],
        offset[1] * scale[1],
        offset[2] * scale[2],
    ];
    let v = collide::math::mul_v(&m, local);
    [position[0] + v[0], position[1] + v[1], position[2] + v[2]]
}
