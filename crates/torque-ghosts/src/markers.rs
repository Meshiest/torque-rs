//! The MissionMarker family's bodies, generic over the [`ShapeBasePart`] they follow: `MissionMarker` (transform and scale),
//! `WayPoint` and `SpawnSphere`. Their mask bits are the part's next free bits ([`ShapeBasePart::NEXT_FREE_MASK`] and up).
//!
//! ```text
//! MissionMarker:  ShapeBase part; flag(mask & M): affine transform (193), F32 x3 scale
//! WayPoint:       the above, then flag(mask & M << 1): string255 name;  flag(mask & M << 2): raw 32 bits (team);
//!                 flag(mask & M << 3): flag (hidden)
//! SpawnSphere:    the above, then flag(mask & M << 1): F32 x4 (radius, sphere, indoor and outdoor weights)
//! ```
//!
//! **View fields:** the part's, `transform` (a `Value::Xform`), `scale` 0..3; WayPoint: `name`, `team`, `hidden`; SpawnSphere:
//! `sphere` 0..4.
//!
//! Credit: these follow TGE 1.4's `game/missionMarker.cc`, carried into the MIT-licensed Torque3D as `T3D/missionMarker.cpp`
//! (Copyright (c) 2012 GarageGames, LLC); the code here is this crate's own.

use crate::codec::{Codec, F32, Layout, R, Xf, opt};
use crate::parts::ShapeBasePart;
use crate::{ObjectView, PackCtx, boolean, raws, text, word, xform};

/// The transform block's mask bit after part `P`.
pub fn transform_mask<P: ShapeBasePart>() -> u32 {
    P::NEXT_FREE_MASK
}

/// The class's own block (WayPoint's name, SpawnSphere's sphere).
pub fn extra_mask<P: ShapeBasePart>() -> u32 {
    P::NEXT_FREE_MASK << 1
}

/// WayPoint's team.
pub fn team_mask<P: ShapeBasePart>() -> u32 {
    P::NEXT_FREE_MASK << 2
}

/// WayPoint's hidden flag.
pub fn flag_mask<P: ShapeBasePart>() -> u32 {
    P::NEXT_FREE_MASK << 3
}

/// The ShapeBase part and the transform block (MissionMarker's body).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MissionMarker<P> {
    pub shape: P,
    pub transform: Option<(Xf, [F32; 3])>,
}

impl<P: ShapeBasePart> MissionMarker<P> {
    /// The part and the transform block: the layout every body here starts with.
    pub fn layout_part<C: Codec>(&mut self, c: &mut C) -> R {
        ShapeBasePart::layout(&mut self.shape, c)?;
        opt(c, "marker.transform?", &mut self.transform, |c, t| {
            c.xform("marker.transform", &mut t.0)?;
            c.floats("marker.scale", &mut t.1)
        })
    }
}

impl<P: ShapeBasePart> Layout for MissionMarker<P> {
    fn layout<C: Codec>(&mut self, c: &mut C) -> R {
        self.layout_part(c)
    }
}

/// Packs the part and the transform block.
pub fn pack_part<P: ShapeBasePart>(
    view: &dyn ObjectView,
    mask: u32,
    ctx: &PackCtx,
) -> (MissionMarker<P>, u32) {
    let (shape, ret) = P::pack(view, mask, ctx);
    let transform = (mask & transform_mask::<P>() != 0)
        .then(|| (xform(view, "transform"), raws::<3>(view, "scale", 0)));
    (MissionMarker { shape, transform }, ret)
}

/// MissionMarker: the part and the transform block (the body of WayPoint and SpawnSphere too).
pub fn pack_mission_marker<P: ShapeBasePart>(
    view: &dyn ObjectView,
    mask: u32,
    ctx: &PackCtx,
) -> (MissionMarker<P>, u32) {
    pack_part(view, mask, ctx)
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WayPoint<P> {
    pub marker: MissionMarker<P>,
    pub name: Option<Vec<u8>>,
    pub team: Option<u32>,
    pub hidden: Option<bool>,
}

impl<P: ShapeBasePart> Layout for WayPoint<P> {
    fn layout<C: Codec>(&mut self, c: &mut C) -> R {
        self.marker.layout_part(c)?;
        opt(c, "waypoint.name?", &mut self.name, |c, s| {
            c.string("waypoint.name", s)
        })?;
        opt(c, "waypoint.team?", &mut self.team, |c, t| {
            c.raw32("waypoint.team", t)
        })?;
        opt(c, "waypoint.hidden?", &mut self.hidden, |c, b| {
            c.flag("waypoint.hidden", b)
        })
    }
}

pub fn pack_waypoint<P: ShapeBasePart>(
    view: &dyn ObjectView,
    mask: u32,
    ctx: &PackCtx,
) -> (WayPoint<P>, u32) {
    let (marker, ret) = pack_part(view, mask, ctx);
    (
        WayPoint {
            marker,
            name: (mask & extra_mask::<P>() != 0).then(|| text(view, "name", 0)),
            team: (mask & team_mask::<P>() != 0).then(|| word(view, "team", 0)),
            hidden: (mask & flag_mask::<P>() != 0).then(|| boolean(view, "hidden", 0)),
        },
        ret,
    )
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SpawnSphere<P> {
    pub marker: MissionMarker<P>,
    pub sphere: Option<[F32; 4]>,
}

impl<P: ShapeBasePart> Layout for SpawnSphere<P> {
    fn layout<C: Codec>(&mut self, c: &mut C) -> R {
        self.marker.layout_part(c)?;
        opt(c, "spawn.sphere?", &mut self.sphere, |c, s| {
            c.floats("spawn.sphere", s)
        })
    }
}

pub fn pack_spawn_sphere<P: ShapeBasePart>(
    view: &dyn ObjectView,
    mask: u32,
    ctx: &PackCtx,
) -> (SpawnSphere<P>, u32) {
    let (marker, ret) = pack_part(view, mask, ctx);
    (
        SpawnSphere {
            marker,
            sphere: (mask & extra_mask::<P>() != 0).then(|| raws::<4>(view, "sphere", 0)),
        },
        ret,
    )
}
