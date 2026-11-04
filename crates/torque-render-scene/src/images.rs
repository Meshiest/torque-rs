//! Mounted images: the shapes a ShapeBase ghost holds (a Player's weapon, a vehicle's turret), placed and posed as TGE's client
//! draws them (the slots and their machine are `torque_ghost_world::images`).
//!
//! **Where** (`ShapeBase::getRenderImageTransform`): with `useEyeOffset` (an `eyeOffset` that is not the identity) and the owner in
//! its first-person view, the owner's render eye transform times `eyeOffset`; a Player's render eye is the transform its
//! first-person camera is drawn from, so the frame's camera is taken for it; another class's is its render transform times its
//! `eye` node. Else `getRenderMountTransform(mountPoint)` (the owner's render transform times its `mount<N>` node, the node's
//! translation and the columns scaled by the owner's scale; no node: the render transform) times the
//! image's mount transform (`ShapeBaseImageData::preload`: `offset` times the inverse of the product, from the image shape's
//! `mountPoint` node up to the root, of each node's default matrix, multiplied as `total = total * node`). A game may scale the
//! eye offset by the owner's scale too ([`ShapeWorld::scaled_eye_offset`]), and the render transform of a mount point with no
//! node ([`ShapeWorld::scaled_bare_mount`]).
//!
//! **Which detail**: the image's own, chosen as a body's is; a game may pin a Player's ([`ShapeWorld::player_image_detail`]).
//! **The pose**: the shape's default pose with its state animation thread (the last state's `stateSequence`), its `ambient` and
//! `spin` threads at the times the client's machine ran them (`torque_ghost_world::images`), or at the frame's times
//! ([`advance_clock`]): `updateImageAnimation` advances the slot's threads by each frame's time, while the machine that sets
//! them runs on the ticks.
//!
//! Credit: the rules are TGE 1.4's `ShapeBase::getRenderMountTransform`, `getRenderImageTransform` and
//! `ShapeBaseImageData::preload` (`game/shapeImage.cc`, Copyright (C) GarageGames.com, Inc.), as Torque3D carries them under the
//! MIT licence (`T3D/shapeImage.cpp`, Copyright (c) 2012 GarageGames, LLC); the code here is this crate's own.

use ghost_world::images::MountedImage;
use ghost_world::set::ClientSet;
use ghost_world::world::ClientWorld;
use shapes::instance::{ShapeInstance, find_sequence};
use shapes::shape::Shape;

use crate::math::Mat;
use crate::world::{ShapeKind, ShapeWorld, Value};

/// The times an image's three threads are drawn at, in seconds: the animation thread's since its state set it, the `ambient`
/// thread's and the `spin` thread's (already at the spin's speeds) since the mount.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ThreadTimes {
    pub anim: f32,
    pub ambient: f32,
    pub spin: f32,
}

impl ThreadTimes {
    /// The times the client's machine has run the slot to (its ticks).
    pub fn of(image: &MountedImage) -> ThreadTimes {
        ThreadTimes {
            anim: image.anim.map_or(0.0, |a| a.time),
            ambient: image.ambient_time,
            spin: image.spin_time,
        }
    }
}

/// An image slot's threads as the live client plays them ([`advance_clock`]): this frame's times and the previous frame's (the pose
/// one frame back, for whatever needs the motion between frames: [`pose_at`] with `prev`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ImageClock {
    pub now: ThreadTimes,
    pub prev: ThreadTimes,
    datablock: u32,
    entered: Option<u32>,
    /// The machine's times when last looked at.
    seen: ThreadTimes,
    last_ms: u32,
}

/// Engine cap: the most a frame advances the threads (a stall or a clock that jumped).
const MAX_FRAME_STEP_S: f32 = 0.25;

/// The times a slot's image is drawn at this frame, from the slot's clock `kept` (none: its first frame): the clock to keep, and
/// its times. A world that does not animate (a session played back) draws the machine's times. A live client plays the threads by its
/// frame clock (`ClientWorld::anim_ms`), as `updateImageAnimation` does each frame: each frame adds its time to the three (the
/// spin's at its present speed); a new mount (another datablock, or the machine's `ambient` time gone back) starts from the
/// machine's times, and a state that set the animation thread again (`ImageAnim::entered`) starts that thread from the machine's
/// time since. Asking twice in one frame gives the same answer. It touches no state of its own (a pool thread may run it).
pub fn advance_clock<S: ClientSet>(
    world: &ClientWorld<S>,
    image: &MountedImage,
    kept: Option<&ImageClock>,
) -> ImageClock {
    let seen = ThreadTimes::of(image);
    let entered = image.anim.map(|a| a.entered);
    let now_ms = world.anim_ms;
    let fixed = ImageClock {
        now: seen,
        prev: seen,
        datablock: image.datablock,
        entered,
        seen,
        last_ms: now_ms,
    };
    if !world.animate {
        return fixed;
    }
    match kept {
        Some(k) if k.datablock == image.datablock && seen.ambient >= k.seen.ambient => {
            let mut k = *k;
            if k.last_ms != now_ms {
                let dt = (now_ms.saturating_sub(k.last_ms) as f32 / 1000.0).min(MAX_FRAME_STEP_S);
                k.prev = k.now;
                k.now.anim += dt;
                k.now.ambient += dt;
                k.now.spin += dt * image.spin_scale;
                k.last_ms = now_ms;
            }
            if entered != k.entered || seen.anim < k.seen.anim {
                k.now.anim = seen.anim;
                k.entered = entered;
            }
            k.seen = seen;
            for t in [&mut k.now.anim, &mut k.now.ambient, &mut k.now.spin] {
                if !t.is_finite() {
                    *t = 0.0;
                }
            }
            k
        }
        _ => fixed,
    }
}

/// The image's datablock as the renderer reads it.
#[derive(Debug, Clone, PartialEq)]
pub struct ImageDb {
    pub shape_file: String,
    pub mount_point: u32,
    /// `offset` and `eyeOffset` (`None`: the identity).
    pub offset: Mat,
    pub eye_offset: Option<Mat>,
}

/// A transform as `readAffineTransform` reads it: the position, the quaternion's x, y, z and the sign of w, w = `sqrt(1 - |xyz|^2)`.
pub fn xform_matrix(words: &[u32; 7]) -> Mat {
    let f = |i: usize| f32::from_bits(words[i]);
    let (x, y, z) = (f(3), f(4), f(5));
    let mut w = (1.0 - (x * x + y * y + z * z)).max(0.0).sqrt();
    if words[6] != 0 {
        w = -w;
    }
    let mut m = Mat::from_quat([x, y, z, w]);
    m.set_column3([f(0), f(1), f(2)]);
    m
}

impl ImageDb {
    /// The datablock `id` when it is a ShapeBaseImageData with a shape file.
    pub fn of<W: ShapeWorld + ?Sized>(world: &W, id: u32) -> Option<ImageDb> {
        if !world.is_image(id) {
            return None;
        }
        let get = |name: &str, i: u32| world.field(id, name, i);
        let shape_file = match get("shapeFile", 0)? {
            Value::Str(s) if !s.is_empty() => String::from_utf8_lossy(s).replace('\\', "/"),
            _ => return None,
        };
        let mount_point = match get("mountPoint", 0) {
            Some(Value::Int(v)) => u32::try_from(*v).unwrap_or(u32::MAX),
            _ => 0,
        };
        let xform = |name: &str| match get(name, 0) {
            Some(Value::Xform(Some(w))) => Some(xform_matrix(w)),
            _ => None,
        };
        Some(ImageDb {
            shape_file,
            mount_point,
            offset: xform("offset").unwrap_or(Mat::IDENTITY),
            eye_offset: xform("eyeOffset").filter(|m| *m != Mat::IDENTITY),
        })
    }

    /// The mount transform `ShapeBaseImageData::preload` computes.
    pub fn mount_transform(&self, shape: &Shape) -> Mat {
        let Some(mp) = node_named(shape, b"mountPoint") else {
            return self.offset;
        };
        let mut total = Mat::IDENTITY.m;
        let mut node = mp;
        // The parent walk is bounded by the node count (a hostile shape's parents may cycle).
        for _ in 0..=shape.nodes.len() {
            let (Some(q), Some(t)) = (
                shape.default_rotations.get(node),
                shape.default_translations.get(node),
            ) else {
                break;
            };
            total = shapes::math::mat_mul(&total, &shapes::math::node_matrix(*q, *t));
            match shape.nodes.get(node).map(|n| n.parent) {
                Some(p) if p >= 0 => node = p as usize,
                _ => break,
            }
        }
        let inv = shapes::math::mat_inverse(&total);
        Mat {
            m: shapes::math::mat_mul(&self.offset.m, &inv),
        }
    }
}

/// `TSShape::findNode`: the node whose name is `name`, ignoring case.
pub fn node_named(shape: &Shape, name: &[u8]) -> Option<usize> {
    let n = shape.find_name(name)?;
    shape.nodes.iter().position(|node| node.name == n)
}

/// `MatrixF::scale(Point3F)`: each of the first three columns times the scale's component.
pub fn scale_columns(m: &Mat, s: [f32; 3]) -> Mat {
    let mut r = *m;
    for row in 0..4 {
        for (col, k) in s.iter().enumerate() {
            r.m[row * 4 + col] *= k;
        }
    }
    r
}

/// The translation of `m` times the scale's components (`Point3F::convolve`).
pub fn scaled_translation(m: &Mat, s: [f32; 3]) -> Mat {
    let mut r = *m;
    r.m[3] *= s[0];
    r.m[7] *= s[1];
    r.m[11] *= s[2];
    r
}

/// What the image transform reads of its owner.
pub struct Owner<'a> {
    pub kind: ShapeKind,
    pub shape: &'a Shape,
    pub instance: &'a ShapeInstance,
    /// The render transform (no scale) and the scale `getRenderMountTransform` uses ([`ShapeWorld::mount_scale`]).
    pub xform: Mat,
    pub scale: [f32; 3],
    /// The owner is the client's control object in its first-person view.
    pub first_person: bool,
    /// The frame's camera transform (a Player's render eye in first person).
    pub camera: Mat,
    /// The eye offset is scaled by the owner's scale ([`ShapeWorld::scaled_eye_offset`]).
    pub scaled_eye_offset: bool,
    /// A mount point with no node scales the render transform's columns ([`ShapeWorld::scaled_bare_mount`]).
    pub scaled_bare_mount: bool,
}

impl Owner<'_> {
    fn is_player(&self) -> bool {
        self.kind.is_player()
    }

    /// `getRenderMountTransform(point)`.
    fn mount_transform(&self, point: u32) -> Mat {
        let node = (point < 32)
            .then(|| node_named(self.shape, format!("mount{point}").as_bytes()))
            .flatten()
            .and_then(|n| self.instance.node_transform(n));
        match node {
            Some(n) => {
                let n = scale_columns(&scaled_translation(&Mat { m: *n }, self.scale), self.scale);
                Mat::mul(&self.xform, &n)
            }
            None if self.scaled_bare_mount => scale_columns(&self.xform, self.scale),
            None => self.xform,
        }
    }

    /// The render eye transform (`getRenderEyeTransform`).
    fn eye_transform(&self) -> Mat {
        if self.is_player() {
            return self.camera;
        }
        match node_named(self.shape, b"eye").and_then(|n| self.instance.node_transform(n)) {
            Some(n) => Mat::mul(&self.xform, &Mat { m: *n }),
            None => self.xform,
        }
    }

    /// `getRenderImageTransform(slot)` for an image of `db` whose shape is `shape`.
    pub fn image_transform(&self, db: &ImageDb, shape: &Shape) -> Mat {
        if let Some(eye_offset) = &db.eye_offset
            && self.first_person
        {
            if !self.scaled_eye_offset {
                return Mat::mul(&self.eye_transform(), eye_offset);
            }
            let l = scaled_translation(eye_offset, self.scale);
            return scale_columns(&Mat::mul(&self.eye_transform(), &l), self.scale);
        }
        self.held_transform(db, shape)
    }

    /// Whether the image of `db` is drawn before the eye (its `eyeOffset` in the first person), not where its owner holds it.
    pub fn before_eye(&self, db: &ImageDb) -> bool {
        db.eye_offset.is_some() && self.first_person
    }

    /// Where the owner holds the image of `db` (its mount point's transform and the image's own mount): the third person's
    /// place of an image the first person draws before the eye.
    pub fn held_transform(&self, db: &ImageDb, shape: &Shape) -> Mat {
        Mat::mul(
            &self.mount_transform(db.mount_point),
            &db.mount_transform(shape),
        )
    }

    /// The detail the image of `details` details is drawn at when the owner is a Player and the world pins it
    /// ([`ShapeWorld::player_image_detail`]): `None` for another class or no pin. `first_person`: the owner's first-person view (the
    /// third person's detail for a held image's shadow).
    pub fn player_detail<W: ShapeWorld + ?Sized>(
        &self,
        world: &W,
        details: usize,
        first_person: bool,
    ) -> Option<usize> {
        if !self.is_player() {
            return None;
        }
        world.player_image_detail(details, first_person)
    }
}

/// The image shape's pose: its `ambient` and `spin` threads (`setImage`: each at position 0, speed 1) run for the times the
/// client's machine gives, then the nodes animated at the shape's smallest visible detail.
///
/// The threads are at `times`: the machine's ([`ThreadTimes::of`]) or the live client's frame times ([`advance_clock`]).
pub fn pose_at(shape: &Shape, image: &MountedImage, times: &ThreadTimes) -> Option<ShapeInstance> {
    let mut charge = |_: u64| true;
    let mut instance = ShapeInstance::new(shape, &mut charge).ok()?;
    let mut threads = false;
    // The animation thread (`isAnimated`, made first at `setImage`): the state's sequence (its index modulo the shape's count, as
    // `preload` keeps it) from its start, at its speed, for the time it has run (`torque_ghost_world::images::ImageAnim`).
    if let Some(anim) = image.anim
        && !shape.sequences.is_empty()
        && anim.sequence >= 0
        && let Some(t) = instance.create_thread(shape)
    {
        let seq = (anim.sequence as usize) % shape.sequences.len();
        instance.set_sequence(shape, t, seq as i32, anim.start());
        let duration = shape.sequences[seq].duration;
        if let Some(th) = instance.thread_mut(t) {
            th.time_scale = anim.time_scale(duration);
        }
        if times.anim.is_finite() && times.anim > 0.0 {
            instance.advance_time(shape, t, times.anim);
        }
        threads = true;
    }
    for (name, time) in [(&b"ambient"[..], times.ambient), (b"spin", times.spin)] {
        let Some(seq) = find_sequence(shape, name) else {
            continue;
        };
        let Some(t) = instance.create_thread(shape) else {
            continue;
        };
        instance.set_sequence(shape, t, seq as i32, 0.0);
        if time.is_finite() && time > 0.0 {
            instance.advance_time(shape, t, time);
        }
        threads = true;
    }
    if threads && !shape.details.is_empty() {
        let dl = usize::try_from(shape.smallest_visible_dl)
            .unwrap_or(0)
            .min(shape.details.len() - 1);
        let _ = instance.animate(shape, dl, None, &mut charge);
    }
    Some(instance)
}

#[cfg(test)]
#[path = "images_tests.rs"]
mod tests;
