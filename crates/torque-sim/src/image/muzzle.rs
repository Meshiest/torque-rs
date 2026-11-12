//! The muzzle transform, point and vector of a Player's image: `ShapeBase::getImageTransform`, `getMuzzleTransform`,
//! `getMuzzlePoint`, `getMuzzleVector` and `getCorrectedAim`, over the host's [`Skeleton`] and ray cast.
//!
//! **Notes:**
//! - `getMuzzleVector` makes the muzzle transform, then the muzzle point, and asks for the corrected aim only when the image's
//!   `correctMuzzleVector` is set and the controlling client is a first-person, non-AI one; else (or when the correction
//!   declines) the vector is the transform's y axis.
//! - the corrected aim is the ray caster's ([`RayCaster::corrected_aim`]); the default is [`corrected_aim`].
//! - `Player::getMuzzleTransform` copies the translation words 3, 7 and 11 of the base muzzle transform into the rebuilt rotation.
//! - the console's vector is normalised only when the length exceeds 1e-4.
//!
//! The muzzle point is the translation of the muzzle transform (`ShapeBase::getMuzzlePoint`; a Player's through its own
//! `getMuzzleTransform`), unless the ray caster moves it ([`RayCaster::muzzle_point`]). The aim ray's mask is the ray caster's
//! ([`RayCaster::type_masks`]).
//!
//! Credit: where Torque3D has the same rules, this module follows its MIT-licensed `T3D/shapeImage.cpp` (Copyright (c) 2012
//! GarageGames, LLC) and `T3D/player.cpp` (Copyright (c) 2012 GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.); the
//! code here is this crate's own.

use torque_collide::math::vec3::{add, length_squared, scale, sub};
use torque_collide::math::{F3, Matrix, matrix_product, set_euler};

use super::ImageData;
use super::host::{AimClient, RayCaster};
use crate::player::{
    ActionNumbers, ImageMuzzle, PlayerState, Skeleton, eye_transform, mount_transform,
};

/// What the muzzle chain reads of the owner.
pub struct MuzzleCtx<'a> {
    pub s: &'a PlayerState,
    pub skel: &'a mut dyn Skeleton,
    pub rays: &'a mut dyn RayCaster,
    /// The owner is viewed in the first person.
    pub first_person: bool,
    /// The controlling client, `None` when there is none.
    pub client: Option<AimClient>,
    /// The Player's action numbers (its datablock's).
    pub actions: ActionNumbers,
}

/// `MatrixF::scale` as the muzzle chain uses it: the first three columns of every row times the object's scale.
fn scale_columns(m: &Matrix, scale: F3) -> Matrix {
    let mut o = *m;
    for r in 0..4 {
        for c in 0..3 {
            o[4 * r + c] = m[4 * r + c] * scale[c];
        }
    }
    o
}

fn translation(m: &Matrix) -> F3 {
    [m[3], m[7], m[11]]
}

/// `ShapeBase::getImageTransform(slot)`: the object's matrix with no image; the first-person eye case; else the mount
/// transform of the image's mount point with its columns scaled, times the image's `offset`.
pub fn image_transform(ctx: &mut MuzzleCtx, img: Option<&ImageData>) -> Matrix {
    let info = img.map(|i| ImageMuzzle {
        mount_point: i.mount_point,
        muzzle: i.mount_transform,
        first_person: (i.first_person && ctx.first_person).then_some(i.eye_offset),
    });
    crate::player::muzzle_transform(ctx.s, ctx.skel, info.as_ref())
}

/// `ShapeBase::getImageTransform(slot, node)` for the image's muzzle node: `getImageTransform(slot)` times the node's
/// transform in the image shape with its columns scaled; node -1 (or a shape without the node) leaves the image transform.
pub fn image_node_transform(ctx: &mut MuzzleCtx, img: &ImageData) -> Matrix {
    let t = image_transform(ctx, Some(img));
    match (img.muzzle_node, &img.muzzle_node_transform) {
        (n, Some(node)) if n != -1 => matrix_product(&t, &scale_columns(node, ctx.s.scale)),
        _ => t,
    }
}

/// `ShapeBase::getMuzzleTransform(slot)`: with an image its muzzle-node transform, else the mount transform of mount point
/// `slot`.
pub fn shape_muzzle_transform(ctx: &mut MuzzleCtx, img: Option<&ImageData>, slot: usize) -> Matrix {
    match img {
        Some(i) => image_node_transform(ctx, i),
        None => mount_transform(ctx.s, ctx.skel, slot as u32),
    }
}

/// `Player::getMuzzleTransform(slot)`: the base transform; with an action animation of the table's fixed ones the rotation is
/// rebuilt from the object's matrix and the head pitch (`objToWorld * Euler(pitch, 0, 0)`), keeping the base translation.
pub fn player_muzzle_transform(
    ctx: &mut MuzzleCtx,
    img: Option<&ImageData>,
    slot: usize,
) -> Matrix {
    let m = shape_muzzle_transform(ctx, img, slot);
    if ctx.s.action_anim < ctx.actions.table {
        let r = set_euler([ctx.s.head[0], 0.0, 0.0]);
        let mut o = matrix_product(&ctx.s.obj_to_world, &r);
        o[3] = m[3];
        o[7] = m[7];
        o[11] = m[11];
        o
    } else {
        m
    }
}

/// `Player::getMuzzlePoint(slot)`: the translation of [`player_muzzle_transform`], through [`RayCaster::muzzle_point`] (the
/// slot's mount transform is made first).
pub fn player_muzzle_point(ctx: &mut MuzzleCtx, img: Option<&ImageData>, slot: usize) -> F3 {
    let mount = translation(&mount_transform(ctx.s, ctx.skel, slot as u32));
    let muzzle = translation(&player_muzzle_transform(ctx, img, slot));
    ctx.rays.muzzle_point(ctx.s, mount, muzzle, true)
}

/// `ShapeBase::getMuzzlePoint(slot)`: the translation of [`shape_muzzle_transform`], through [`RayCaster::muzzle_point`].
pub fn shape_muzzle_point(ctx: &mut MuzzleCtx, img: Option<&ImageData>, slot: usize) -> F3 {
    let mount = translation(&mount_transform(ctx.s, ctx.skel, slot as u32));
    let muzzle = translation(&shape_muzzle_transform(ctx, img, slot));
    ctx.rays.muzzle_point(ctx.s, mount, muzzle, false)
}

/// How far ahead of the eye the corrected aim looks, and the distance a nearer aim point is pushed out to.
pub const AIM_REACH: f32 = 500.0;
pub const AIM_PULL_IN: f32 = 6.0;
/// Points closer together than the root of this (0.2) count as one for the aim.
const AIM_MIN_SQUARED: f32 = 0.04;

/// The corrected aim: the unit vector from `muzzle` toward what lies straight ahead of the eye, or `None` when the muzzle is
/// within 0.2 of that point.
///
/// The rule follows Torque3D's MIT-licensed `ShapeBase::getCorrectedAim` (`shapeImage.cpp`), Copyright (c) 2012 GarageGames,
/// LLC; the code here is this crate's own. As in TGE 1.4 the ray starts at the eye (not the camera) and meets every type: it
/// runs [`AIM_REACH`] along the eye's forward axis with this object left out; its end stands in when it meets nothing; an
/// aim point nearer the eye than [`AIM_PULL_IN`] (but not within 0.2 of it) is pushed out to that distance along the ray.
pub fn corrected_aim<R: RayCaster + ?Sized>(rays: &mut R, eye: &Matrix, muzzle: F3) -> Option<F3> {
    let start = translation(eye);
    let forward = [eye[1], eye[5], eye[9]];
    let end = add(start, scale(forward, AIM_REACH));
    let mask = rays.type_masks().muzzle_aim;
    let met = rays
        .cast_ray(start, end, mask, false)
        .map_or(end, |h| h.point);
    (crate::library::get().aim_vector)(start, met, muzzle)
}

/// The corrected aim after its ray, from the ray's `start`, the point it `met` and the `muzzle`: the aim point pulled out to
/// [`AIM_PULL_IN`] when nearer, then the unit vector from the muzzle to it. [`PLAIN`](crate::library::PLAIN)'s `aim_vector`.
pub fn plain_aim_vector(start: F3, met: F3, muzzle: F3) -> Option<F3> {
    let reach = sub(met, start);
    let reach_sq = length_squared(reach);
    let aim_at = if reach_sq < AIM_PULL_IN * AIM_PULL_IN && reach_sq > AIM_MIN_SQUARED {
        add(start, scale(reach, AIM_PULL_IN / reach_sq.sqrt()))
    } else {
        met
    };
    let to = sub(aim_at, muzzle);
    let to_sq = length_squared(to);
    (to_sq > AIM_MIN_SQUARED).then(|| scale(to, 1.0 / to_sq.sqrt()))
}

/// `getMuzzleVector(slot)`: the muzzle transform, then the muzzle point (in that order), then the corrected aim when the
/// image asks for it and the controlling client is in the first person and not an AI; else the transform's y axis (elements 1,
/// 5, 9). `player` selects the Player overrides of the transform and the point.
pub fn muzzle_vector(
    ctx: &mut MuzzleCtx,
    img: Option<&ImageData>,
    slot: usize,
    player: bool,
) -> F3 {
    let m = if player {
        player_muzzle_transform(ctx, img, slot)
    } else {
        shape_muzzle_transform(ctx, img, slot)
    };
    let p = if player {
        player_muzzle_point(ctx, img, slot)
    } else {
        shape_muzzle_point(ctx, img, slot)
    };
    if let Some(i) = img
        && i.correct_muzzle_vector
        && let Some(AimClient {
            freelook_value,
            ai: false,
        }) = ctx.client
        && freelook_value == 0.0
    {
        let eye = eye_transform(ctx.s, ctx.skel);
        if let Some(v) = ctx.rays.corrected_aim(&eye, translation(&m), p) {
            return v;
        }
    }
    [m[1], m[5], m[9]]
}

/// The length the console's `getMuzzleVector` takes: 0 when no component squared exceeds 1e-4 (`Point3F::isZero`; a NaN
/// component counts as exceeding), else the length.
pub fn fast_len(v: F3) -> f32 {
    let over = |x: f32| !(x * x <= 0.0001);
    if over(v[0]) || over(v[1]) || over(v[2]) {
        length_squared(v).sqrt()
    } else {
        0.0
    }
}

/// The console method's normalisation (through the [`library`](crate::library) table's `muzzle_normalize`).
pub fn console_normalize(v: F3) -> F3 {
    (crate::library::get().muzzle_normalize)(v)
}

/// The console method's normalisation: only when the length is above 1e-4 (a NaN length is not), each component times
/// `1 / len`. [`PLAIN`](crate::library::PLAIN)'s `muzzle_normalize`.
pub fn plain_console_normalize(v: F3) -> F3 {
    let len = fast_len(v);
    if !(len > 0.0001) {
        return v;
    }
    let inv = 1.0 / len;
    std::array::from_fn(|i| v[i] * inv)
}
