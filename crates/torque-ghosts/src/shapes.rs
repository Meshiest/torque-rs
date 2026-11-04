//! TGE's small ShapeBase classes over [`TgeShapeBase`]: `Camera`, `StaticShape` (and `ScopeAlwaysShape`, which sends
//! StaticShape's record) and `Item`.
//!
//! ```text
//! Camera:      ShapeBase part; flag(controlled by this connection && !initial): the record ends;
//!              flag(Move, bit 25): F32 x3 position, F32 rotation x, F32 rotation z
//! StaticShape: ShapeBase part; flag(Position, bit 25): affine transform (193), F32 x3 scale; flag(powered)
//! Item:        ShapeBase part; flag(initial): flag rotate, flag static, flag collidable, flag(scale != 1,1,1): F32 x3 scale;
//!              flag(ThrowSrc, bit 26, && a collision object && it is ghosted): int(GB) index;
//!              flag(Rotation, bit 28, && !rotate): flag(axis z < 0), F32 angle;
//!              flag(Position, bit 27): F32 x3 position, flag(at rest) [else F32 x3 velocity], flag(!NoWarp)
//! ```
//! Camera's packer returns 0 (`Camera::packUpdate` drops the part's return mask); StaticShape and Item return the part's.
//!
//! **View fields:** the ShapeBase part's; Camera: `controlled_here` (this connection controls it), `position` 0..3,
//! `rotation` 0..3; StaticShape: `transform`, `scale` 0..3, `powered`; Item: `rotate`, `static`, `collidable`, `scale` 0..3, the
//! link `collision_object`, `axis_z` and `angle` (the object's rotation as an angle about z), `position` 0..3, `at_rest`,
//! `velocity` 0..3.
//!
//! Credit: these follow TGE 1.4's `game/camera.cc`, `game/staticShape.cc` and `game/item.cc` (Copyright (C) GarageGames.com,
//! Inc.), whose MIT-licensed successors are Torque3D's `T3D/camera.cpp` (Copyright (c) 2012 GarageGames, LLC),
//! `T3D/staticShape.cpp` (Copyright (c) 2012 GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.) and `T3D/item.cpp`
//! (Copyright (c) 2012 GarageGames, LLC). The layouts are TGE's (Torque3D's StaticShape sends a matrix and its Item no
//! collidable flag); the code is this crate's own.

use crate::codec::{Codec, F32, Layout, R, Xf, mask_bits, opt};
use crate::parts::mask::INITIAL;
use crate::shapebase::{TgeShapeBase, mask::NO_WARP, pack_shapebase};
use crate::{Link, ObjectView, PackCtx, boolean, fld, raw, raws, xform};

/// Camera's `MoveMask`.
pub const CAMERA_MOVE_MASK: u32 = 1 << 25;
/// StaticShape's `PositionMask`.
pub const STATIC_POSITION_MASK: u32 = 1 << 25;

/// Item's mask bits.
pub mod item_mask {
    pub const HIDDEN: u32 = 1 << 25;
    pub const THROW_SRC: u32 = 1 << 26;
    pub const POSITION: u32 = 1 << 27;
    pub const ROTATION: u32 = 1 << 28;
}

/// `Camera`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Camera {
    pub shape: TgeShapeBase,
    /// Controlled by this connection and not an initial update: nothing follows.
    pub controlled: bool,
    /// Position x, y, z, rotation x, z.
    pub pose: Option<[F32; 5]>,
}

impl Layout for Camera {
    fn layout<C: Codec>(&mut self, c: &mut C) -> R {
        self.shape.layout(c)?;
        c.flag("camera.controlled", &mut self.controlled)?;
        if self.controlled {
            self.pose = None;
            return Ok(());
        }
        opt(c, "camera.move?", &mut self.pose, |c, p| {
            c.floats("camera.pose", p)
        })
    }
}

/// Packs a Camera (the return mask is 0).
pub fn pack_camera(view: &dyn ObjectView, mask: u32, ctx: &PackCtx) -> Camera {
    let (shape, _) = pack_shapebase(view, mask, ctx);
    let controlled = boolean(view, "controlled_here", 0) && mask & INITIAL == 0;
    let pose = (!controlled && mask & CAMERA_MOVE_MASK != 0).then(|| {
        [
            raw(view, "position", 0),
            raw(view, "position", 1),
            raw(view, "position", 2),
            raw(view, "rotation", 0),
            raw(view, "rotation", 2),
        ]
    });
    Camera {
        shape,
        controlled,
        pose,
    }
}

/// `StaticShape` (and `ScopeAlwaysShape`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StaticShape {
    pub shape: TgeShapeBase,
    pub transform: Option<(Xf, [F32; 3])>,
    pub powered: bool,
}

impl Layout for StaticShape {
    fn layout<C: Codec>(&mut self, c: &mut C) -> R {
        self.shape.layout(c)?;
        opt(c, "static.position?", &mut self.transform, |c, t| {
            c.xform("static.transform", &mut t.0)?;
            c.floats("static.scale", &mut t.1)
        })?;
        c.flag("static.powered", &mut self.powered)
    }
}

pub fn pack_static_shape(view: &dyn ObjectView, mask: u32, ctx: &PackCtx) -> (StaticShape, u32) {
    let (shape, ret) = pack_shapebase(view, mask, ctx);
    let transform = (mask & STATIC_POSITION_MASK != 0)
        .then(|| (xform(view, "transform"), raws::<3>(view, "scale", 0)));
    (
        StaticShape {
            shape,
            transform,
            powered: boolean(view, "powered", 0),
        },
        ret,
    )
}

/// Item's initial block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ItemInitial {
    pub rotate: bool,
    pub is_static: bool,
    pub collidable: bool,
    /// `flag(scale != (1, 1, 1))` and the scale.
    pub scale: Option<[F32; 3]>,
}

/// Item's position block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ItemPosition {
    pub position: [F32; 3],
    /// `flag(at rest)`: `None` at rest, else the velocity.
    pub velocity: Option<[F32; 3]>,
    /// `flag(!(mask & NoWarp))`.
    pub warp: bool,
}

/// `Item`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Item {
    pub shape: TgeShapeBase,
    pub initial: Option<ItemInitial>,
    /// The ghost index of the object the item was thrown by.
    pub collision_object: Option<u32>,
    /// `flag(axis z < 0)` and the angle.
    pub rotation: Option<(bool, F32)>,
    pub position: Option<ItemPosition>,
}

impl Layout for Item {
    fn layout<C: Codec>(&mut self, c: &mut C) -> R {
        self.shape.layout(c)?;
        opt(c, "item.initial?", &mut self.initial, |c, i| {
            c.flag("item.rotate", &mut i.rotate)?;
            c.flag("item.static", &mut i.is_static)?;
            c.flag("item.collidable", &mut i.collidable)?;
            opt(c, "item.scale?", &mut i.scale, |c, s| {
                c.floats("item.scale", s)
            })
        })?;
        let bits = c.ghost_bits();
        opt(c, "item.collision?", &mut self.collision_object, |c, g| {
            c.int("item.collision", g, bits)?;
            *g = mask_bits(*g, bits);
            Ok(())
        })?;
        opt(c, "item.rotation?", &mut self.rotation, |c, r| {
            c.flag("item.negativeZ", &mut r.0)?;
            c.raw32("item.angle", &mut r.1.0)
        })?;
        opt(c, "item.position?", &mut self.position, |c, p| {
            c.floats("item.position", &mut p.position)?;
            let mut at_rest = p.velocity.is_none();
            c.flag("item.atRest", &mut at_rest)?;
            if at_rest {
                p.velocity = None;
            } else {
                let v = p.velocity.get_or_insert_with(Default::default);
                c.floats("item.velocity", v)?;
            }
            c.flag("item.warp", &mut p.warp)
        })
    }
}

pub fn pack_item(view: &dyn ObjectView, mask: u32, ctx: &PackCtx) -> (Item, u32) {
    let (shape, ret) = pack_shapebase(view, mask, ctx);
    let initial = (mask & INITIAL != 0).then(|| {
        let scale = raws::<3>(view, "scale", 0);
        ItemInitial {
            rotate: boolean(view, "rotate", 0),
            is_static: boolean(view, "static", 0),
            collidable: boolean(view, "collidable", 0),
            scale: (scale.map(F32::get) != [1.0; 3]).then_some(scale),
        }
    });
    let collision_object = match view.link("collision_object") {
        Link::Ghost(g) if mask & item_mask::THROW_SRC != 0 => Some(mask_bits(g, ctx.ghost_bits())),
        _ => None,
    };
    let rotation = (mask & item_mask::ROTATION != 0 && !boolean(view, "rotate", 0))
        .then(|| (fld(view, "axis_z", 0) < 0.0, raw(view, "angle", 0)));
    let position = (mask & item_mask::POSITION != 0).then(|| ItemPosition {
        position: raws::<3>(view, "position", 0),
        velocity: (!boolean(view, "at_rest", 0)).then(|| raws::<3>(view, "velocity", 0)),
        warp: mask & NO_WARP == 0,
    });
    (
        Item {
            shape,
            initial,
            collision_object,
            rotation,
            position,
        },
        ret,
    )
}
