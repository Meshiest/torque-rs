//! Where a ghost is drawn: the transform its pose makes ([`pose_matrix`]), the pose a frame draws it at ([`render_pose`]: the
//! client's own where it runs the object), and the render transform of a ghost riding another ([`mounted_transform`]).
//!
//! Credit: follows TGE 1.4's `Player::setRenderPosition` (`game/player.cc`) and `ShapeBase::getRenderMountTransform`
//! (`game/shapeImage.cc`), Copyright (C) GarageGames.com, Inc., as Torque3D carries them under the MIT licence (`T3D/player.cpp`,
//! Copyright (c) 2012 GarageGames, LLC and Copyright (C) 2015 Faust Logic, Inc.; `T3D/shapeImage.cpp`, Copyright (c) 2012
//! GarageGames, LLC); the code here is this crate's own.

use std::collections::HashMap;
use std::sync::Arc;

use ghost_world::world::Pose;
use shapes::instance::ShapeInstance;
use shapes::shape::Shape;

use crate::Assets;
use crate::images::{node_named, scale_columns, scaled_translation};
use crate::math::Mat;
use crate::shapes::{Loaded, Models};
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
        // A ghost the client shows between its server's updates (a plugin's): where its warp has it.
        _ => match g.pose() {
            Some(pose) if g.class.is_some_and(|c| w.interpolated_class(c)) => {
                Some(world.warped_pose(index, pose))
            }
            pose => pose,
        },
    }
}

/// The longest chain of scene-hierarchy links and mounts followed from one ghost: the hierarchy's own hard depth with the
/// mounts' on top. A hostile server's longer chain stops here and the ghost is drawn where its ghost was sent.
pub const MAX_CARRY_DEPTH: u32 = hierarchy::HARD_DEPTH as u32 + MAX_MOUNT_DEPTH;

/// Where one frame draws the ghosts something carries (scene-hierarchy children and mounted ghosts), each worked out once:
/// a ghost's render transform and its shape posed at rest, which the ghosts hung on its nodes share. One per frame, since
/// what it holds is that frame's poses; [`mounted_transform`] makes one for its single call.
#[derive(Default)]
pub struct Carried {
    placed: HashMap<u32, Slot>,
    posed: HashMap<u32, Option<(Arc<Loaded>, ShapeInstance)>>,
    worked: usize,
}

impl Carried {
    /// How many ghosts' placements were worked out: each ghost once a frame (a hostile chain cut by the caps may work one
    /// out again).
    #[must_use]
    pub fn worked(&self) -> usize {
        self.worked
    }
}

/// The render transform (no scale of its own) of a ghost the scene hierarchy hangs on another, or mounted on another.
///
/// A child of the scene hierarchy (`ClientWorld::links`) is drawn at its parent's render transform (the parent itself
/// placed this way when something carries it) times the parent's node, as a frame poses the parent at rest, times the
/// child's local pose blended to the frame's clock, by the mount rule (the parent's scale scales the node's translation
/// and the local one, never the child's rotation); a child whose parent's ghost went is drawn where it was left. A ghost
/// with a link follows its link alone: `None` when its parent cannot be placed, so it is drawn at its own pose.
///
/// A ghost with no link and mounted on another, as the client sets it every frame: a Player's
/// `setRenderPosition` takes its mount's `getRenderMountTransform(node)` (the mount's render transform times its `mount<N>` node,
/// the node's translation and the columns scaled by the mount's scale, [`ShapeWorld::mount_scale`]) and turns it by its own yaw
/// (`zrot`); another class rides the node as it is. The mount is placed the same way when it rides something itself (a gunner
/// on a tank's turret), at most [`MAX_MOUNT_DEPTH`] deep, and posed as a frame poses it at rest
/// ([`ShapeWorld::pose_at_rest`]). `None`: not mounted (or a mount the client cannot place), drawn by [`render_pose`].
///
/// A frame that places many ghosts shares one [`Carried`] through [`carried_transform`], so each carrier is placed once.
pub fn mounted_transform<W: ShapeWorld + ?Sized>(
    w: &W,
    assets: &dyn Assets,
    models: &mut Models,
    notes: &mut Vec<String>,
    index: u32,
    depth: u32,
) -> Option<Mat> {
    let mut carried = Carried::default();
    carried_transform(w, assets, models, notes, &mut carried, index, depth)
}

/// [`mounted_transform`] with the frame's [`Carried`]: the placements of every carrier on the way are kept for the
/// frame's later ghosts, so placing all of a frame's ghosts costs each ghost once, whatever the depth of its chain.
pub fn carried_transform<W: ShapeWorld + ?Sized>(
    w: &W,
    assets: &dyn Assets,
    models: &mut Models,
    notes: &mut Vec<String>,
    carried: &mut Carried,
    index: u32,
    depth: u32,
) -> Option<Mat> {
    let chain = Chain {
        mounts: depth,
        links: 0,
    };
    Placer {
        w,
        assets,
        models,
        notes,
        carried,
    }
    .place(index, chain)
    .at
}

/// A ghost's entry in [`Carried`].
#[derive(Debug, Clone, Copy)]
enum Slot {
    /// On the way: a chain that reaches it again is a loop.
    Placing,
    /// Placed for the frame (`None`: carried by nothing the client can place).
    Placed(Option<Mat>),
}

/// How far down a chain of mounts and scene-hierarchy links a placement is.
#[derive(Debug, Clone, Copy)]
struct Chain {
    mounts: u32,
    links: u32,
}

/// A placement, and whether it holds for the ghost whatever chain asked for it (not cut short by a cap on the way).
#[derive(Debug, Clone, Copy)]
struct Placed {
    at: Option<Mat>,
    whole: bool,
}

impl Placed {
    const CUT: Placed = Placed {
        at: None,
        whole: false,
    };

    fn whole(at: Option<Mat>) -> Placed {
        Placed { at, whole: true }
    }
}

/// What a frame's placements read and write.
struct Placer<'a, W: ShapeWorld + ?Sized> {
    w: &'a W,
    assets: &'a dyn Assets,
    models: &'a mut Models,
    notes: &'a mut Vec<String>,
    carried: &'a mut Carried,
}

impl<W: ShapeWorld + ?Sized> Placer<'_, W> {
    /// Ghost `index`'s carried render transform, `chain` down a chain: kept, or worked out once (one path: its link when it
    /// has one, else its mount).
    fn place(&mut self, index: u32, chain: Chain) -> Placed {
        match self.carried.placed.get(&index) {
            Some(Slot::Placed(at)) => return Placed::whole(*at),
            // A ghost on the way to itself (a hostile loop through links and mounts) finds no carrier there, and nothing
            // placed against that is kept: it depends on where the loop was entered.
            Some(Slot::Placing) => return Placed::CUT,
            None => {}
        }
        if chain.mounts + chain.links >= MAX_CARRY_DEPTH {
            return Placed::CUT;
        }
        self.carried.placed.insert(index, Slot::Placing);
        self.carried.worked += 1;
        let links = &self.w.records().links;
        let placed = if links.held(index).is_some() || links.parent(index).is_some() {
            self.linked(index, chain)
        } else {
            self.mounted(index, chain)
        };
        if placed.whole {
            self.carried.placed.insert(index, Slot::Placed(placed.at));
        } else {
            self.carried.placed.remove(&index);
        }
        placed
    }

    /// The render transform of carrier `index`, `chain` down a chain: carried, or at its own pose.
    fn carrier(&mut self, index: u32, chain: Chain) -> Option<(Mat, bool)> {
        let placed = self.place(index, chain);
        let at = match placed.at {
            Some(at) => at,
            None => {
                let g = self.w.records().ghosts.get(&index)?;
                pose_matrix(render_pose(self.w, index).as_ref()?, g.scale())?.0
            }
        };
        Some((at, placed.whole))
    }

    /// Node `node` picks of ghost `index`'s shape posed at rest (`Some(None)`: no such node), the shape built once a frame;
    /// `None` when the ghost has no shape the client can pose.
    fn posed_node(
        &mut self,
        index: u32,
        node: impl FnOnce(&Shape) -> Option<usize>,
    ) -> Option<Option<[f32; 16]>> {
        let (w, assets, models, notes) = (self.w, self.assets, &mut *self.models, &mut *self.notes);
        let posed = self.carried.posed.entry(index).or_insert_with(|| {
            let world = w.records();
            let g = world.ghosts.get(&index)?;
            let path = String::from_utf8_lossy(world.datablock_text(g.datablock()?, "shapeFile")?)
                .replace('\\', "/");
            let loaded = models.get(&|p| world.constructor_sequences(p), assets, &path, notes)?;
            let mut charge = |_: u64| true;
            let mut instance = ShapeInstance::new(&loaded.model.shape, &mut charge).ok()?;
            // Posed as a frame poses it (a Player mount, the turret, by its head pitch; another by its script threads).
            w.pose_at_rest(
                index,
                render_pose(w, index),
                &loaded.model.shape,
                &mut instance,
                0,
                notes,
            );
            Some((loaded, instance))
        });
        let (loaded, instance) = posed.as_ref()?;
        Some(
            node(&loaded.model.shape)
                .and_then(|n| instance.node_transform(n))
                .copied(),
        )
    }

    /// A child of the scene hierarchy ([`mounted_transform`]'s first case).
    fn linked(&mut self, index: u32, chain: Chain) -> Placed {
        let links = &self.w.records().links;
        if let Some((m, _)) = links.held(index) {
            return Placed::whole(Some(Mat { m }));
        }
        let (Some(parent), Some(local)) = (
            links.parent(index),
            links.local(index, self.w.records().anim_ms, self.w.records().animate),
        ) else {
            return Placed::whole(None);
        };
        let p = parent.object.0;
        let up = Chain {
            links: chain.links + 1,
            ..chain
        };
        let Some((at, whole)) = self.carrier(p, up) else {
            return Placed::whole(None);
        };
        let node = match parent.node {
            hierarchy::NodeRef::Origin => None,
            // A node the client cannot find counts as the parent's origin, as on the server.
            hierarchy::NodeRef::Node(n) => self.posed_node(p, |_| Some(usize::from(n))).flatten(),
        };
        let scale = self
            .w
            .records()
            .ghosts
            .get(&p)
            .and_then(|g| g.scale())
            .unwrap_or([1.0; 3]);
        let at = hierarchy::Pose { world: at.m, scale };
        Placed {
            at: Some(Mat {
                m: hierarchy::compose(&at, node.as_ref(), &local),
            }),
            whole,
        }
    }

    /// A mounted ghost ([`mounted_transform`]'s second case).
    fn mounted(&mut self, index: u32, chain: Chain) -> Placed {
        if chain.mounts >= MAX_MOUNT_DEPTH {
            return Placed::CUT;
        }
        let w = self.w;
        let world = w.records();
        let Some((m, node)) = world.mount_of(index).filter(|(m, _)| *m != index) else {
            return Placed::whole(None);
        };
        let Some(mg) = world.ghosts.get(&m) else {
            return Placed::whole(None);
        };
        let up = Chain {
            mounts: chain.mounts + 1,
            ..chain
        };
        let Some((mxf, whole)) = self.carrier(m, up) else {
            return Placed::whole(None);
        };
        let mscale = w.mount_scale(mg, mg.scale().unwrap_or([1.0; 3]));
        let mount_node = |shape: &Shape| {
            (node < 32)
                .then(|| node_named(shape, format!("mount{node}").as_bytes()))
                .flatten()
        };
        let Some(n) = self.posed_node(m, mount_node) else {
            return Placed::whole(None);
        };
        let nmat = match n {
            Some(n) => Mat::mul(
                &mxf,
                &scale_columns(&scaled_translation(&Mat { m: n }, mscale), mscale),
            ),
            None if w.scaled_bare_mount() => scale_columns(&mxf, mscale),
            None => mxf,
        };
        let Some(g) = world.ghosts.get(&index) else {
            return Placed::whole(None);
        };
        let at = if w.kind_of(g) == ShapeKind::Player {
            let yaw = match render_pose(w, index) {
                Some(Pose::Euler { yaw, .. }) => yaw,
                _ => 0.0,
            };
            Mat::mul(&nmat, &Mat::euler_z(yaw))
        } else {
            nmat
        };
        Placed {
            at: Some(at),
            whole,
        }
    }
}

#[cfg(test)]
#[path = "mounts_tests.rs"]
mod tests;
