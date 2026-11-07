//! What the HUD classes read of the connection (`gui::hud`'s [`HudFeed`], made every tick from the connection's world and
//! prediction: [`feed_of`] over the record world and what the game's view of it answers, [`HudWorld`]).
//!
//! - **The control object** (`GameConnection`'s): the predicted Player (its energy over its datablock's `maxEnergy`,
//!   `getEnergyValue`), the vehicle it drives, or the Camera of a spectator; a camera position of 0 is the first-person camera (the
//!   prediction's `camera_pos`).
//! - **The camera** is the client's eye (`ClientWorld::eye`: the predicted Player's eye or the Camera's), or the game's own
//!   camera when it draws the world from one ([`ClientState::names_eye`](crate::state::ClientState::names_eye)), its
//!   transform `EulerF(pitch, 0, yaw)` as the renderer builds it, the fov the connection's unless that camera carries one.
//!   The names' line-of-sight rays and the in-front test start from it, as the projection does.
//! - **The named shapes** are the shapes the game's view names ([`HudWorld::named_candidates`]), each with the name block the
//!   game's view answers ([`HudWorld::shape_name`]: its name, colour and distance). [`NameList`] keeps them in the order they were
//!   first named across ticks (one whose name goes, or whose ghost is deleted, is taken out and the last moved into its place);
//!   the shapes first named in the same tick are taken in ghost order. The label's point is a Player's eye (its shape's eye node,
//!   [`crate::eye`], as the first-person view; [`PLAYER_EYE_Z`] over its position without one) or the ghost's position. The ray
//!   from the camera to the point against the static world (`castRay` with the game's names mask,
//!   [`crate::predict::game::CameraMasks::names`]) is cast only for a shape inside the distance and in front of the camera,
//!   within one budget a tick ([`NAME_RAY_WORK`]).
//!
//! Credit: follows TGE's `GuiShapeNameHud` label point and line-of-sight ray as Torque3D carries it (MIT:
//! `T3D/fps/guiShapeNameHud.cpp`, Copyright (c) 2012 GarageGames, LLC).

use std::cell::RefCell;
use std::rc::Rc;

use ghost_world::Ghost;
use ghost_world::world::Pose;
use gui::hud::{HudCamera, HudControl, HudFeed, MAX_NAMED_SHAPES, NamedShape};

use crate::conn::ConnGame;
use crate::predict::PLAYER_EYE_Z;
use crate::render::PoseView;
use crate::state::ClientState;

/// The name rays' work one tick may do (`ClientCollision` units).
pub const NAME_RAY_WORK: u64 = 4_000_000;

/// The named-shape list in arrival order: ghost indices.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NameList {
    pub order: Vec<u32>,
}

impl NameList {
    /// The list after a tick: ghosts that lost their name (or are gone) leave by a swap-remove; ghosts with a new name
    /// join at the end.
    pub fn update(&mut self, named: &[u32]) {
        let mut i = 0;
        while i < self.order.len() {
            if named.contains(&self.order[i]) {
                i += 1;
            } else {
                self.order.swap_remove(i);
            }
        }
        for g in named {
            if !self.order.contains(g) && self.order.len() < MAX_NAMED_SHAPES {
                self.order.push(*g);
            }
        }
    }
}

/// A shape's name block: the name, its colour and the distance it shows to.
#[derive(Debug, Clone, PartialEq)]
pub struct ShapeName {
    pub name: Vec<u8>,
    pub color: [f32; 3],
    pub distance: f32,
}

/// What the HUD reads of the client's world past the record world and the poses ([`PoseView`]): the game's view of it answers.
pub trait HudWorld: PoseView {
    /// The scene's visible and fog distances.
    fn view_distances(&self) -> (f32, f32);

    /// The ghosts that may carry a name, in index order.
    fn named_candidates(&self) -> Vec<(u32, &Ghost<Self::Set>)>;

    /// A ghost's name block, `None` without a name.
    fn shape_name(&self, ghost: &Ghost<Self::Set>) -> Option<ShapeName>;

    /// Whether a ghost's label stands at its eye (a Player's) rather than at its position.
    fn label_at_eye(&self, ghost: &Ghost<Self::Set>) -> bool;
}

/// Where a ghost's label sits before `verticalOffset`.
fn label_point<W: HudWorld>(world: &W, g: &Ghost<W::Set>) -> Option<[f32; 3]> {
    let base = match g.pose() {
        Some(
            Pose::Euler { position, .. }
            | Pose::Quat { position, .. }
            | Pose::Item { position, .. },
        ) => position,
        Some(Pose::Affine { xf, .. }) => xf.position.map(|v| v.get()),
        Some(Pose::Matrix { m, .. }) => [m[3], m[7], m[11]],
        _ => g.position?,
    };
    let base = g.position.unwrap_or(base);
    Some(if world.label_at_eye(g) {
        // TGE 1.4's GuiShapeNameHud takes a Player's render eye (`getRenderEyeTransform`): its shape's eye node.
        let (pitch, yaw) = match g.pose() {
            Some(Pose::Euler { pitch, yaw, .. }) => (pitch, yaw),
            _ => (0.0, 0.0),
        };
        let offset = g.datablock().and_then(|db| {
            crate::eye::eye_offset(
                world,
                db,
                g.record.as_ref(),
                pitch,
                g.record.as_ref().is_some_and(|r| world.record_crouched(r)),
            )
        });
        match offset {
            Some(o) => crate::eye::eye_point(base, yaw, g.scale().unwrap_or([1.0; 3]), o),
            None => [base[0], base[1], base[2] + PLAYER_EYE_Z],
        }
    } else {
        base
    })
}

/// The camera transform's rotation (`EulerF(pitch, 0, yaw)`: `Rz * Rx`, TGE row-major).
fn rotation(pitch: f32, yaw: f32) -> [[f32; 3]; 3] {
    let (sp, cp) = if pitch == 0.0 {
        (0.0, 1.0)
    } else {
        pitch.sin_cos()
    };
    let (sy, cy) = if yaw == 0.0 {
        (0.0, 1.0)
    } else {
        yaw.sin_cos()
    };
    let rz = [[cy, sy, 0.0], [-sy, cy, 0.0], [0.0, 0.0, 1.0]];
    let rx = [[1.0, 0.0, 0.0], [0.0, cp, sp], [0.0, -sp, cp]];
    let mut m = [[0.0f32; 3]; 3];
    for (i, row) in m.iter_mut().enumerate() {
        for (j, v) in row.iter_mut().enumerate() {
            *v = (0..3).map(|k| rz[i][k] * rx[k][j]).sum();
        }
    }
    m
}

/// The feed of this tick (the client's [`NameList`] keeps the arrival order across ticks).
pub fn feed<G: ConnGame>(client: &Rc<RefCell<ClientState<G>>>) -> HudFeed {
    let mut guard = client.borrow_mut();
    let st = &mut *guard;
    let names = &mut st.names;
    let view = st.names_eye;
    let Some(conn) = st.conn.conn.as_mut().filter(|c| c.accepted) else {
        names.order.clear();
        return HudFeed::default();
    };
    let conn = &mut **conn;
    let world = &conn.world;
    let first_person = conn.predict.camera_pos == 0.0;
    let control_ghost = world.control.object;
    // A predicted Camera (the death camera, a spectator) is the control object while it runs: no Player's HUD.
    let predicted = if conn.predict.camera.is_some() {
        None
    } else {
        conn.predict.predicted.as_ref()
    };
    let control = if let Some(pr) = predicted {
        let max = pr.data.max_energy;
        let energy = if max > 0.0 {
            pr.player.s.energy / max
        } else {
            0.0
        };
        Some(HudControl {
            player: pr.vehicle.is_none(),
            vehicle: pr.vehicle.is_some(),
            first_person,
            energy,
            damage: 0.0,
        })
    } else if control_ghost.is_some() || world.eye().is_some() {
        Some(HudControl {
            player: false,
            vehicle: false,
            first_person,
            energy: 0.0,
            damage: 0.0,
        })
    } else {
        None
    };
    // The name rays against the static world: the prediction's collision, made for the first ray of the tick.
    let mask = conn.predict.game().camera_masks().names;
    let mut ctx = Some(crate::predict::Ctx {
        near: [0.0; 3],
        static_world: &mut conn.predict.static_world,
        shapes: &mut conn.predict.shapes,
        driven: None,
        ghosts: Some(&conn.predict.ghost_vehicles),
        disabled: None,
        player_at: None,
    });
    let mut collision: Option<crate::predict::ClientCollision> = None;
    let mut cast = |from: [f32; 3], to: [f32; 3]| {
        let c = collision.get_or_insert_with(|| {
            let ctx = ctx.take().map(|mut x| {
                x.near = from;
                x
            });
            crate::predict::ClientCollision::new(world, u32::MAX, NAME_RAY_WORK, ctx)
        });
        c.cast_ray(from, to, mask).is_some()
    };
    feed_of(world, names, control, view, &mut cast)
}

/// The feed of a tick from the client's world: `control` is the control object's part, `view` the game's own camera (none:
/// the world's eye), `cast` the ray from the camera to a label against the static world (true: something stands in the way),
/// `names` the list as the last tick left it.
pub fn feed_of<W: HudWorld>(
    world: &W,
    names: &mut NameList,
    control: Option<HudControl>,
    view: Option<ghost_world::world::Eye>,
    cast: &mut dyn FnMut([f32; 3], [f32; 3]) -> bool,
) -> HudFeed {
    let (visible_distance, fog_distance) = world.view_distances();
    let mut out = HudFeed {
        visible_distance,
        fog_distance,
        control,
        ..HudFeed::default()
    };
    let control_ghost = world.control.object;
    let Some(eye) = world.eye() else {
        names.order.clear();
        return out;
    };
    let eye = view.unwrap_or(eye);
    let camera = HudCamera {
        position: eye.position,
        rotation: rotation(eye.pitch, eye.yaw),
        fov: view
            .and_then(|v| v.fov)
            .map_or_else(|| world.fov_deg(), f32::from),
    };
    out.camera = Some(camera);
    // The named ghosts this tick, the control object left out.
    // Only the ghosts that can carry a name are looked at (the game's view answers which).
    let named: Vec<u32> = world
        .named_candidates()
        .into_iter()
        .filter(|(i, g)| Some(*i) != control_ghost && world.shape_name(g).is_some())
        .map(|(i, _)| i)
        .collect();
    names.update(&named);
    let dir = [
        camera.rotation[0][1],
        camera.rotation[1][1],
        camera.rotation[2][1],
    ];
    for index in &names.order {
        let Some(g) = world.ghosts.get(index) else {
            continue;
        };
        let (
            Some(ShapeName {
                name,
                color,
                distance,
            }),
            Some(point),
        ) = (world.shape_name(g), label_point(world, g))
        else {
            continue;
        };
        let d = [0, 1, 2].map(|i| point[i] - eye.position[i]);
        let dist = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        let in_front = dir[0] * d[0] + dir[1] * d[1] + dir[2] * d[2] > 0.0;
        let max = distance.min(out.visible_distance);
        let hidden = dist <= max && in_front && cast(eye.position, point);
        out.names.push(NamedShape {
            name,
            position: point,
            color,
            distance,
            hidden,
        });
    }
    out
}

#[cfg(test)]
#[path = "hud_tests.rs"]
mod tests;
