//! Where a ghost is drawn: the transform its pose makes ([`pose_matrix`]), the pose a frame draws it at ([`render_pose`]: the
//! client's own where it runs the object), and the render transform of a ghost riding another ([`mounted_transform`]).
//!
//! Credit: follows TGE 1.4's `Player::setRenderPosition` (`game/player.cc`) and `ShapeBase::getRenderMountTransform`
//! (`game/shapeImage.cc`), Copyright (C) GarageGames.com, Inc., as Torque3D carries them under the MIT licence (`T3D/player.cpp`,
//! Copyright (c) 2012 GarageGames, LLC and Copyright (C) 2015 Faust Logic, Inc.; `T3D/shapeImage.cpp`, Copyright (c) 2012
//! GarageGames, LLC); the code here is this crate's own.

use ghost_world::world::Pose;
use shapes::instance::ShapeInstance;

use crate::Assets;
use crate::images::{node_named, scale_columns, scaled_translation};
use crate::math::Mat;
use crate::shapes::Models;
use crate::world::{ShapeKind, ShapeWorld};

/// The ghost's world transform and scale.
pub fn pose_matrix(pose: &Pose, scale: Option<[f32; 3]>) -> Option<(Mat, [f32; 3])> {
    match pose {
        Pose::Affine { xf, scale } => {
            let [x, y, z] = xf.quat.map(|v| v.get());
            let mut w = (1.0 - (x * x + y * y + z * z)).max(0.0).sqrt();
            if xf.negative_w {
                w = -w;
            }
            let mut m = Mat::from_quat([x, y, z, w]);
            m.set_column3(xf.position.map(|v| v.get()));
            Some((m, scale.unwrap_or([1.0; 3])))
        }
        Pose::Quat { position, rotation } => {
            let mut m = Mat::from_quat(*rotation);
            m.set_column3(*position);
            Some((m, scale.unwrap_or([1.0; 3])))
        }
        Pose::Item { position, rotation } => {
            let angle = rotation.map_or(0.0, |r| r.1);
            let mut m = Mat::from_axis_angle([0.0, 0.0, 1.0], angle);
            m.set_column3(*position);
            Some((m, scale.unwrap_or([1.0; 3])))
        }
        Pose::Euler { position, yaw, .. } => {
            let mut m = Mat::euler_z(*yaw);
            m.set_column3(*position);
            Some((m, scale.unwrap_or([1.0; 3])))
        }
        _ => None,
    }
}

/// The most mounts followed from one object to the one it rides last (a driver on a turret on a tank is two): a hostile server's
/// longer chain or cycle stops here and the object is drawn where its ghost was sent.
pub const MAX_MOUNT_DEPTH: u32 = 4;

/// Where ghost `index` is drawn when it rides nothing: a vehicle the client runs itself where it runs it
/// (`ClientWorld::vehicle_render`), a ghost the client runs itself where it draws it (`ClientWorld::ghost_render`: a corpse a
/// death camera orbits), the control object where the client predicts it (`ClientWorld::control_render`), else where its ghost
/// was sent.
pub fn render_pose<W: ShapeWorld + ?Sized>(w: &W, index: u32) -> Option<Pose> {
    let world = w.records();
    let g = world.ghosts.get(&index)?;
    if let Some(p) = world.vehicle_render.get(&index) {
        return Some(*p);
    }
    if let Some(p) = world.ghost_render.get(&index) {
        return Some(*p);
    }
    match &world.control_render {
        Some(p) if world.control.object == Some(index) => Some(*p),
        _ => g.pose(),
    }
}

/// The render transform (no scale of its own) of a ghost mounted on another, as the client sets it every frame: a Player's
/// `setRenderPosition` takes its mount's `getRenderMountTransform(node)` (the mount's render transform times its `mount<N>` node,
/// the node's translation and the columns scaled by the mount's scale, [`ShapeWorld::mount_scale`]) and turns it by its own yaw
/// (`zrot`); another class rides the node as it is. The mount is placed the same way when it rides something itself (a gunner
/// on a tank's turret), at most [`MAX_MOUNT_DEPTH`] deep, and posed as a frame poses it at rest
/// ([`ShapeWorld::pose_at_rest`]). `None`: not mounted (or a mount the client cannot place), drawn by [`render_pose`].
pub fn mounted_transform<W: ShapeWorld + ?Sized>(
    w: &W,
    assets: &dyn Assets,
    models: &mut Models,
    notes: &mut Vec<String>,
    index: u32,
    depth: u32,
) -> Option<Mat> {
    if depth >= MAX_MOUNT_DEPTH {
        return None;
    }
    let world = w.records();
    let (m, node) = world.mount_of(index)?;
    if m == index {
        return None;
    }
    let mg = world.ghosts.get(&m)?;
    let mount_pose = render_pose(w, m);
    let mxf = match mounted_transform(w, assets, models, notes, m, depth + 1) {
        Some(x) => x,
        None => pose_matrix(mount_pose.as_ref()?, mg.scale())?.0,
    };
    let mscale = w.mount_scale(mg, mg.scale().unwrap_or([1.0; 3]));
    let path = String::from_utf8_lossy(world.datablock_text(mg.datablock()?, "shapeFile")?)
        .replace('\\', "/");
    let loaded = models.get(&|p| world.constructor_sequences(p), assets, &path, notes)?;
    let shape = &loaded.model.shape;
    let mut charge = |_: u64| true;
    let mut instance = ShapeInstance::new(shape, &mut charge).ok()?;
    // The mount posed as a frame poses it (a Player mount, the turret, by its head pitch; another by its script threads).
    w.pose_at_rest(m, mount_pose, shape, &mut instance, 0, notes);
    let n = (node < 32)
        .then(|| node_named(shape, format!("mount{node}").as_bytes()))
        .flatten()
        .and_then(|n| instance.node_transform(n));
    let nmat = match n {
        Some(n) => Mat::mul(
            &mxf,
            &scale_columns(&scaled_translation(&Mat { m: *n }, mscale), mscale),
        ),
        None if w.scaled_bare_mount() => scale_columns(&mxf, mscale),
        None => mxf,
    };
    let g = world.ghosts.get(&index)?;
    if w.kind_of(g) == ShapeKind::Player {
        let yaw = match render_pose(w, index) {
            Some(Pose::Euler { yaw, .. }) => yaw,
            _ => 0.0,
        };
        return Some(Mat::mul(&nmat, &Mat::euler_z(yaw)));
    }
    Some(nmat)
}

#[cfg(test)]
#[path = "mounts_tests.rs"]
mod tests;
