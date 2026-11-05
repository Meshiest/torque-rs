//! A frame's moving shapes: each ShapeBase ghost of the classes the world draws as shapes ([`ShapeWorld::shape_classes`]),
//! placed, posed and built with its mounted images and a WheeledVehicle's tyres ([`draws_culled`]), in three phases so their
//! posing runs on the pool ([`crate::par`]):
//!
//! - **place**, in the ghosts' order on the calling thread: each ghost selected, placed (a mounted one at its mount, which may
//!   pose the mount), loaded, culled and its detail picked, its images' shapes loaded; then each job's live pose and image clocks
//!   taken out of the host's keeping ([`Hooks`]; after every placement: a rider's placement reads its mount's live pose);
//! - **pose**, on the pool: each job's live pose advanced (or its pose at rest made, [`ShapeWorld::pose_at_rest`]), its images'
//!   clocks advanced and their poses, transforms and details made, and every build's cache key. It reads the world and the job
//!   alone: what it gives does not depend on how the jobs are split;
//! - **build**, in the ghosts' order on the calling thread: the notes in the order one thread would make them, the builds (the
//!   kept ones looked up, the rest planned; the textures asked for in one order), the tyres, and the live poses and clocks put
//!   back.
//!
//! A shape with a translucent material in its drawn detail is drawn with the translucent objects, back to front
//! (`ShapeBase::prepRenderImage`: the whole shape, sorted by its distance); its mounted images after it, each a shape of its own.
//!
//! Credit: follows TGE 1.4's `ShapeBase::prepRenderImage` and `renderObject` (`game/shapeBase.cc`), `Projectile::prepRenderImage`
//! and `renderObject` (`game/projectile.cc`), `MathUtils::createOrientFromDir` (`math/mathUtils.cc`) and
//! `Player::renderMountedImage` (`game/player.cc`), each Copyright (C) GarageGames.com, Inc.; as Torque3D carries them under the
//! MIT licence: `T3D/shapeBase.cpp`, `T3D/projectile.cpp` and `T3D/player.cpp` (Copyright (c) 2012 GarageGames, LLC; Copyright (C)
//! 2015 Faust Logic, Inc.), `math/mathUtils.cpp` (Copyright (c) 2012 GarageGames, LLC). The code here is this crate's own.

use std::collections::BTreeMap;
use std::sync::Arc;

use ghost_world::images::MountedImage;
use ghost_world::set::ClientSet;
use ghost_world::threads::ThreadState;
use ghost_world::world::{Ghost, Pose};
use shapes::instance::ShapeInstance;
use shapes::shape::{Model, Shape};

use crate::images::{HeldMotion, ImageClock, ImageDb, Owner, advance_clock};
use crate::math::Mat;

use crate::mounts::{MAX_MOUNT_DEPTH, mounted_transform, pose_matrix, render_pose};
use crate::shapes::{
    DetailPolicy, Loaded, MIN_VISIBILITY, Models, ShapeParts, detail_for, highest_detail,
    mat3_scale, shape_draws_keyed, shape_key_bytes, take_inline_fill,
};
use crate::textures::TextureSet;
use crate::wheels::{Tires, tire_draws};
use crate::world::{ClassOf, ShapeKind, ShapeWorld, Value};
use crate::{Assets, Draw, PointLight};

/// The canvas width the projected pixel sizes are measured against (a fixed reference, not the frame's size).
const REFERENCE_CANVAS_WIDTH: f32 = 800.0;

/// What a frame's moving shapes ask of their host beyond the world: the point lights a shape is lit by, and the live poses
/// and image clocks a live client keeps from one frame to the next (its own animation of a ghost, such as a Player's). The
/// defaults keep nothing: every ghost is posed at rest ([`ShapeWorld::pose_at_rest`]) and every image at its machine's times.
pub trait Hooks<W: ShapeWorld + ?Sized>: Sync {
    /// A ghost's live pose as the host keeps it.
    type Live: Send;

    /// The GL point lights a shape whose world sphere is at `centre` with `radius` is lit by, best first (none by default).
    fn lights(&self, centre: [f32; 3], radius: f32) -> Vec<PointLight> {
        let _ = (centre, radius);
        Vec::new()
    }

    /// Takes ghost `index`'s live pose out of the host's keeping (`None` before its first live frame).
    fn take_live(&self, index: u32) -> Option<Self::Live> {
        let _ = index;
        None
    }

    /// Takes the clock of the image in `ghost`'s slot `slot` out of the host's keeping.
    fn take_clock(&self, ghost: u32, slot: usize) -> Option<ImageClock> {
        let _ = (ghost, slot);
        None
    }

    /// Advances the live pose `kept` of the ghost `ask` names by the frame's time and poses it (a pool thread may run it): the
    /// pose to keep and the posed instance (`None`: posed at rest).
    fn advance_live(
        &self,
        world: &W,
        ask: LiveAsk<'_, W>,
        kept: Option<Self::Live>,
        notes: &mut Vec<String>,
    ) -> (Option<Self::Live>, Option<ShapeInstance>) {
        let _ = (world, ask, notes);
        (kept, None)
    }

    /// The host's own motion of the image in ghost `ghost`'s slot `slot` this frame ([`HeldMotion`]: none by default).
    fn held_motion(&self, ghost: u32, slot: usize) -> HeldMotion {
        let _ = (ghost, slot);
        HeldMotion::default()
    }

    /// Puts ghost `index`'s live pose and its images' clocks back in the host's keeping.
    fn put_back(
        &self,
        index: u32,
        live: Option<Self::Live>,
        clocks: Vec<((u32, usize), ImageClock)>,
    ) {
        let _ = (index, live, clocks);
    }
}

/// No hooks: no point lights, nothing kept between frames.
impl<W: ShapeWorld + ?Sized> Hooks<W> for () {
    type Live = ();
}

/// The ghost [`Hooks::advance_live`] poses: its index, its ghost, its shape and datablock, its head pitch and the detail drawn.
pub struct LiveAsk<'a, W: ShapeWorld + ?Sized> {
    pub index: u32,
    pub ghost: &'a Ghost<W::Set>,
    pub shape: &'a Shape,
    pub db: u32,
    pub pitch: f32,
    pub dl: usize,
}

/// Which shapes a frame may leave out: whether a shape whose bounding sphere (images and tyres included) is at `centre` with
/// `radius` may show in it.
pub trait Cull {
    fn keeps(&self, centre: [f32; 3], radius: f32) -> bool;
}

/// Where a Projectile ghost is drawn and its shape (`Projectile::prepRenderImage` and `renderObject`): not at all while hidden or
/// faded out (`fade <= 1/255`) or with no `projectileShapeName`; else at the render transform the client's step gives it
/// (`ClientWorld::projectile_render`), facing the camera instead with `faceViewer` (`createOrientFromDir` of the camera minus the
/// render point), scaled by the datablock's `scale` times the object's (times a uniform scale the render transform carries). The shape is drawn whole and opaque as any shape; a world
/// that steps no projectile draws none.
fn projectile_placement<W: ShapeWorld + ?Sized>(
    world: &W,
    index: u32,
    ghost: &Ghost<W::Set>,
    eye: [f32; 3],
) -> Option<(Mat, [f32; 3], String)> {
    let records = world.records();
    let look = records.projectile_render.get(&index)?;
    if look.fade <= 1.0 / 255.0 {
        return None;
    }
    let db = ghost.datablock()?;
    let file = records.datablock_text(db, "projectileShapeName")?;
    if file.is_empty() {
        return None;
    }
    let path = String::from_utf8_lossy(file).replace('\\', "/");
    let f = |name: &str, i: u32| world.float(db, name, i).filter(|x| x.is_finite());
    let face_viewer = matches!(world.field(db, "faceViewer", 0), Some(Value::Bool(true)));
    let own = ghost.scale().unwrap_or([1.0; 3]);
    let mut scale = [0, 1, 2].map(|k| f("scale", k as u32).unwrap_or(1.0) * own[k]);
    let mut m = Mat { m: look.transform };
    // A host may grow the projectile (a uniform scale in the transform's columns, as one leaving a muzzle): taken out into the
    // scale.
    let grow = m.column(1).iter().map(|c| c * c).sum::<f32>().sqrt();
    if grow.is_finite() && grow > 1.0e-6 && (grow - 1.0).abs() > 1.0e-3 {
        m = crate::images::scale_columns(&m, [1.0 / grow; 3]);
        scale = scale.map(|s| s * grow);
    }
    if face_viewer {
        let p = look.position();
        let d = [eye[0] - p[0], eye[1] - p[1], eye[2] - p[2]];
        let l = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        let dir = if l > 0.0 {
            d.map(|c| c / l)
        } else {
            [0.0, 0.0, 1.0]
        };
        m = Mat {
            m: orient_from_dir(dir),
        };
        m.set_column3(p);
    }
    Some((m, scale, path))
}

/// `MathUtils::createOrientFromDir`: the columns `i = j x z` (`(0, -1, 0)` when zero, normalised), `j`, `k = i x j`.
fn orient_from_dir(j: [f32; 3]) -> [f32; 16] {
    let mut i = [j[1], -j[0], 0.0];
    let len = |v: [f32; 3]| {
        if v.iter().any(|c| 1.0e-4 < c * c) {
            (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
        } else {
            0.0
        }
    };
    if len(i) == 0.0 {
        i = [0.0, -1.0, 0.0];
    }
    let l = len(i);
    if 1.0e-4 < l {
        i = i.map(|c| c / l);
    }
    let k = [
        i[1] * j[2] - i[2] * j[1],
        i[2] * j[0] - i[0] * j[2],
        i[0] * j[1] - i[1] * j[0],
    ];
    [
        i[0], j[0], k[0], 0.0, i[1], j[1], k[1], 0.0, i[2], j[2], k[2], 0.0, 0.0, 0.0, 0.0, 1.0,
    ]
}

/// The moving shapes [`draws_culled`] built and left out since the process started (a frame breakdown's counts).
static BUILT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static CULLED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// The shapes a culled build ([`draws_culled`] with a cull) built and left out so far.
pub fn cull_counts() -> (u64, u64) {
    use std::sync::atomic::Ordering::Relaxed;
    (BUILT.load(Relaxed), CULLED.load(Relaxed))
}

/// What the shapes add to a frame.
#[derive(Debug, Default)]
pub struct ShapeDraws {
    pub opaque: Vec<Draw>,
    /// Shapes with translucency: (eye distance, the shape's draws), to be drawn back to front.
    pub translucent: Vec<(f32, Vec<Draw>)>,
    /// The ghost each draw of [`ShapeDraws::opaque`] and each entry of [`ShapeDraws::translucent`] belongs to (the enhanced renderer
    /// keeps a shape's transforms by its ghost).
    pub opaque_owners: Vec<u32>,
    pub translucent_owners: Vec<u32>,
    /// The render transform (no scale) each drawn ghost was drawn with, by ghost (a mounted one's from its mount).
    pub transforms: BTreeMap<u32, Mat>,
    /// The ghosts left out because nothing in the frame could show them ([`draws_culled`]).
    pub culled: usize,
    /// With [`ShapeSelect::OnlyControl`]: the nanoseconds the build spent placing, posing, building the bodies and the images,
    /// and filling vertices inline, in that order.
    pub laps: Option<[u64; 5]>,
    /// Inside [`crate::shapecache::with_cache`]: each ghost's calls in order (their kept draws are not in `opaque` and
    /// `translucent`: [`crate::shapecache::with_draws`]).
    pub calls: BTreeMap<u32, Vec<crate::shapecache::Call>>,
}
/// Which shapes a frame's build takes (a live window draws the client's control object every frame, the rest of the
/// world's shapes with the scene it rebuilds now and then).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShapeSelect {
    /// Every shape (the whole frame).
    All,
    /// Every shape but the control object's (its body and images).
    NotControl,
    /// The control object's shape and images only.
    OnlyControl,
    /// The shapes a live window's control sync takes ([`is_moving`]), in a world that does not animate (a replay: the
    /// checks of the meshes placed on the GPU).
    Moving,
}

impl ShapeSelect {
    fn takes<W: ShapeWorld + ?Sized>(self, world: &W, index: u32) -> bool {
        match self {
            ShapeSelect::All => true,
            ShapeSelect::NotControl => !is_dynamic(world, index),
            ShapeSelect::OnlyControl => is_dynamic(world, index),
            ShapeSelect::Moving => is_moving(world, index),
        }
    }
}

/// Whether a live window draws ghost `index` every frame: the control object, and while the world animates
/// (`ClientWorld::animate`) every shape that moves or animates (Players, vehicles, items, projectiles): their transforms come with
/// the ghost updates and their threads advance by the frame's time. A static shape stays with the scene rebuilt now and then,
/// unless it plays a script thread.
pub fn is_dynamic<W: ShapeWorld + ?Sized>(world: &W, index: u32) -> bool {
    dynamic_in(world, index, world.records().animate)
}

/// Whether a live window draws its moving shapes every frame apart from the rest of the scene ([`is_dynamic`]): with a control
/// object the client predicts (`ClientWorld::control_render`), or any control object while the world animates (a Camera).
pub fn live_split<W: ShapeWorld + ?Sized>(world: &W) -> bool {
    let r = world.records();
    r.control.object.is_some() && (r.control_render.is_some() || r.animate)
}

/// [`is_dynamic`] as a live window would have it (the world animating) whatever `ClientWorld::animate` says.
pub fn is_moving<W: ShapeWorld + ?Sized>(world: &W, index: u32) -> bool {
    dynamic_in(world, index, true)
}

fn dynamic_in<W: ShapeWorld + ?Sized>(world: &W, index: u32, animate: bool) -> bool {
    let r = world.records();
    // The vehicle the client drives (and runs) moves with the control object; whatever rides a moving object moves with
    // it (the driver, a turret and its gunner).
    if r.control.object == Some(index) || r.vehicle_render.contains_key(&index) {
        return true;
    }
    let mut at = index;
    for _ in 0..MAX_MOUNT_DEPTH {
        let Some((m, _)) = r.mount_of(at) else {
            break;
        };
        if r.control.object == Some(m) || r.vehicle_render.contains_key(&m) {
            return true;
        }
        at = m;
    }
    animate
        && r.ghosts
            .get(&index)
            .is_some_and(|g| match world.kind_of(g) {
                ShapeKind::Player
                | ShapeKind::WheeledVehicle
                | ShapeKind::Vehicle
                | ShapeKind::Item
                | ShapeKind::Projectile => true,
                // A static shape playing a script thread (a clock's hands, a blinking sign).
                ShapeKind::Static => r.threads.get(&index).is_some_and(|slots| {
                    slots.iter().flatten().any(|t| t.state == ThreadState::Play)
                }),
                ShapeKind::None => false,
            })
}

/// The draws of the shapes `select` takes at the detail levels `policy` picks, leaving out the shapes `cull` says nothing in the
/// frame can show (never the control object): they are not posed or built.
#[allow(clippy::too_many_arguments)]
pub fn draws_culled<W: ShapeWorld + ?Sized, H: Hooks<W>>(
    world: &W,
    hooks: &H,
    assets: &dyn Assets,
    textures: &mut TextureSet<'_>,
    notes: &mut Vec<String>,
    policy: DetailPolicy,
    select: ShapeSelect,
    cull: Option<&dyn Cull>,
) -> ShapeDraws {
    let records = world.records();
    let eye = records.eye().map(|e| e.position).unwrap_or([0.0; 3]);
    // The camera's transform (`Camera::from_eye`'s): a Player's render eye in its first-person view (the images).
    let camera = records.eye().map_or(Mat::IDENTITY, |e| {
        let mut m = Mat::mul(&Mat::euler_z(e.yaw), &Mat::euler_x(e.pitch));
        m.set_column3(e.position);
        m
    });
    // Pixels a meter covers at a distance of one meter (the reference canvas and the world's fov).
    let fov = records.fov_deg().to_radians();
    let pixels_per_meter = REFERENCE_CANVAS_WIDTH / (2.0 * (fov / 2.0).tan());
    let mut models = Models::default();
    let mut out = ShapeDraws::default();
    let keeping = crate::shapecache::on();
    let nodes = keeping && NODES.with(std::cell::Cell::get);
    let mut laps = Laps::new(select == ShapeSelect::OnlyControl);
    // Placed, in the ghosts' order.
    let mut entries: Vec<Entry> = Vec::new();
    let mut jobs: Vec<Job<'_, W::Set, H::Live>> = Vec::new();
    for class in world.shape_classes() {
        for (index, ghost) in records.of_class(class) {
            if !select.takes(world, index) {
                continue;
            }
            let mut here = Vec::new();
            let job = place(
                world,
                assets,
                &mut models,
                &mut here,
                &mut out,
                cull,
                (class, index, ghost),
                (eye, pixels_per_meter, policy),
            );
            entries.push(Entry {
                index,
                notes: here,
                placed: job.is_some(),
            });
            jobs.extend(job);
        }
    }
    take_live(world, hooks, &mut jobs);
    laps.lap(Lap::Place);
    // Posed on the pool.
    let frame = Frame {
        world,
        hooks,
        eye,
        camera,
        pixels_per_meter,
        policy,
        keeping,
        nodes,
        shadow_body: shadow_body(),
    };
    let mut posed = crate::par::map_owned(jobs, weight, |j| pose(&frame, j)).into_iter();
    laps.lap(Lap::Pose);
    // Built, in the ghosts' order.
    let mut build = Build {
        world,
        assets,
        textures,
        notes,
        eye,
        pixels_per_meter,
        policy,
        nodes,
    };
    // The ghost whose builds are being stamped and the call it started at.
    let mut open: Option<(u32, usize)> = None;
    for e in entries {
        if let Some((g, from)) = open.take() {
            out.calls.insert(g, crate::shapecache::calls_from(from));
        }
        if keeping {
            open = Some((e.index, crate::shapecache::start_ghost(e.index)));
        }
        build.notes.extend(e.notes);
        if e.placed {
            let Some(p) = posed.next() else {
                break;
            };
            build_posed(&mut build, hooks, &mut models, &mut out, &mut laps, p);
        }
    }
    if let Some((g, from)) = open.take() {
        out.calls.insert(g, crate::shapecache::calls_from(from));
    }
    laps.lap(Lap::Place);
    out.laps = laps.done();
    out
}

/// The place phase of ghost `index`: its placement, shape, cull and detail, and its images' shapes (`None`: not drawn, its notes
/// given).
#[allow(clippy::too_many_arguments)]
fn place<'w, W: ShapeWorld + ?Sized, L>(
    world: &'w W,
    assets: &dyn Assets,
    models: &mut Models,
    notes: &mut Vec<String>,
    out: &mut ShapeDraws,
    cull: Option<&dyn Cull>,
    (class, index, ghost): (ClassOf<W>, u32, &'w Ghost<W::Set>),
    (eye, pixels_per_meter, policy): ([f32; 3], f32, DetailPolicy),
) -> Option<Job<'w, W::Set, L>> {
    let records = world.records();
    // The control object where the client predicts it (`ClientWorld::control_render`), a vehicle the client runs where it runs
    // it, a mounted ghost at its mount; every other ghost where it was sent.
    let pose = render_pose(world, index);
    let mounted = mounted_transform(world, assets, models, notes, index, 0);
    // The client's own object in its first-person view is not drawn (its ghost stays where the server last ghosted it); its
    // images are.
    let first_person = records.hide_control && records.control.object == Some(index);
    let db = ghost.datablock()?;
    let (path, placed) = if world.kind(class) == ShapeKind::Projectile {
        let (m, scale, path) = projectile_placement(world, index, ghost, eye)?;
        (path, Some((m, scale)))
    } else {
        let file = records.datablock_text(db, "shapeFile")?;
        let path = String::from_utf8_lossy(file).replace('\\', "/");
        let placed = match mounted {
            Some(m) => Some((m, ghost.scale().unwrap_or([1.0; 3]))),
            None => pose.as_ref().and_then(|p| pose_matrix(p, ghost.scale())),
        };
        (path, placed)
    };
    let (xform, scale) = placed?;
    out.transforms.insert(index, xform);
    let Some(loaded) = models.get(&|p| records.constructor_sequences(p), assets, &path, notes)
    else {
        notes.push(format!("shape {path}: not loaded"));
        return None;
    };
    let model = &loaded.model;
    if let Some(cull) = cull
        && records.control.object != Some(index)
        && !shape_may_show(
            cull, world, assets, models, notes, index, model, &xform, scale,
        )
    {
        out.culled += 1;
        CULLED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        return None;
    }
    if cull.is_some() {
        BUILT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
    let centre = xform.mul_p(model.shape.center);
    let d = [centre[0] - eye[0], centre[1] - eye[1], centre[2] - eye[2]];
    let eye_distance = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
    let max_scale = scale.iter().copied().fold(0.0, f32::max).max(1e-6);
    let dl = detail_for(model, eye_distance / max_scale, pixels_per_meter, policy)?;
    let images = image_jobs(world, assets, models, notes, class, index, first_person);
    Some(Job {
        class,
        index,
        ghost,
        db,
        path,
        loaded,
        xform,
        scale,
        first_person,
        pose,
        centre,
        eye_distance,
        max_scale,
        dl,
        live_out: false,
        live: None,
        images,
    })
}

/// Runs `f` with the kept builds of its [`draws_culled`] calls in their meshes' own space (`on`: the enhanced renderer's node
/// transforms on the GPU, [`crate::shapes::MeshPlace`]) or in the world.
pub fn with_nodes<T>(on: bool, f: impl FnOnce() -> T) -> T {
    let was = NODES.with(|n| n.replace(on));
    let out = f();
    NODES.with(|n| n.set(was));
    out
}

/// The parts of a moving shapes' build [`Laps`] times ([`ShapeDraws::laps`]).
#[derive(Clone, Copy)]
enum Lap {
    Place,
    Pose,
    Body,
    Images,
}

/// Runs `f` with the client's own object in its first-person view built as a shape that only casts shadows (`on`: the enhanced
/// renderer; its draws' labels marked, [`is_shadow_body`]) instead of left out (the fixed-function frame).
pub fn with_shadow_body<T>(on: bool, f: impl FnOnce() -> T) -> T {
    let was = SHADOW_BODY.with(|n| n.replace(on));
    let out = f();
    SHADOW_BODY.with(|n| n.set(was));
    out
}

/// What a shadow-only body's label starts with (its draws' labels: `shape <label> object <n>`).
const SHADOW_BODY_LABEL: &str = "shadow-only ";

/// Whether a draw is a first-person body's built to cast shadows only ([`with_shadow_body`]).
pub fn is_shadow_body(label: &str) -> bool {
    label
        .strip_prefix("shape ")
        .is_some_and(|l| l.starts_with(SHADOW_BODY_LABEL))
}

/// What a mounted image's draws' labels start with (`shape <label> object <n>`, the label `image <slot> of <class> <ghost> <path>`).
pub const IMAGE_LABEL: &str = "image ";

/// What the label of an image drawn before the eye starts with when a copy of it casts its shadow where it is held
/// ([`is_view_model`]).
const VIEW_MODEL_LABEL: &str = "view-model ";

/// Whether a draw is a mounted image's (a held weapon, a turret): a shape of its own, not its owner's body.
pub fn is_image(label: &str) -> bool {
    label.strip_prefix("shape ").is_some_and(|l| {
        l.strip_prefix(VIEW_MODEL_LABEL)
            .unwrap_or(l)
            .starts_with(IMAGE_LABEL)
    })
}

/// Whether a draw is a first-person image's drawn before the eye that casts no shadow from there (in the enhanced renderer a
/// shadow-only copy of it, [`is_shadow_body`], casts where the first-person body holds it).
pub fn is_view_model(label: &str) -> bool {
    label
        .strip_prefix("shape ")
        .is_some_and(|l| l.starts_with(VIEW_MODEL_LABEL))
}

/// A view-model image's label ([`is_view_model`]).
pub fn view_model_label(label: &str) -> String {
    format!("{VIEW_MODEL_LABEL}{label}")
}

/// Whether the calling thread is inside [`with_shadow_body`]`(true, ..)`.
pub fn shadow_body() -> bool {
    SHADOW_BODY.with(std::cell::Cell::get)
}

/// A shadow-only body's label.
pub fn shadow_body_label(label: &str) -> String {
    format!("{SHADOW_BODY_LABEL}{label}")
}

thread_local! {
    /// Inside [`with_nodes`]`(true, ..)`: a kept build stays in its meshes' own space ([`crate::shapes::MeshPlace`]).
    static NODES: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// Inside [`with_shadow_body`]`(true, ..)`: a first-person body is built to cast shadows only.
    static SHADOW_BODY: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// A moving shapes' build timed by part: each lap's time since the last goes to its part, the vertices filled in it to the fill.
struct Laps {
    on: bool,
    at: web_time::Instant,
    spent: [u64; 5],
}

impl Laps {
    fn new(on: bool) -> Laps {
        take_inline_fill();
        Laps {
            on,
            at: web_time::Instant::now(),
            spent: [0; 5],
        }
    }

    fn lap(&mut self, part: Lap) {
        if !self.on {
            return;
        }
        let now = web_time::Instant::now();
        let ns = (now - self.at).as_nanos() as u64;
        let fill = take_inline_fill().min(ns);
        self.spent[part as usize] += ns - fill;
        self.spent[4] += fill;
        self.at = now;
    }

    fn done(self) -> Option<[u64; 5]> {
        self.on.then_some(self.spent)
    }
}

/// Whether ghost `index`'s shape `model` placed by `xform` at `scale`, with its mounted images (and a vehicle's tyres), may show in
/// the frame `cull` stands for: its sphere grown by twice each image's radius (an image hangs from a node inside the body's
/// bounds), and a wheeled vehicle's by twice its tyres'.
#[allow(clippy::too_many_arguments)]
fn shape_may_show<W: ShapeWorld + ?Sized>(
    cull: &dyn Cull,
    world: &W,
    assets: &dyn Assets,
    models: &mut Models,
    notes: &mut Vec<String>,
    index: u32,
    model: &Model,
    xform: &Mat,
    scale: [f32; 3],
) -> bool {
    let records = world.records();
    let centre = xform.mul_p(model.shape.center);
    let radius = model.shape.radius * scale.iter().fold(0.0f32, |a, b| a.max(b.abs()));
    let max_scale = scale.iter().fold(1.0f32, |a, b| a.max(b.abs()));
    let mut extra = 0.0f32;
    for (_, image) in records.mounted_images(index) {
        let Some(db) = ImageDb::of(world, image.datablock) else {
            continue;
        };
        if let Some(l) = models.get(
            &|p| records.constructor_sequences(p),
            assets,
            &db.shape_file,
            notes,
        ) {
            extra = extra.max(2.0 * l.model.shape.radius * max_scale);
        }
    }
    if records
        .ghosts
        .get(&index)
        .is_some_and(|g| g.record.is_some() && world.kind_of(g) == ShapeKind::WheeledVehicle)
    {
        // Tyres sit on the hubs, inside the body's bounds; a tyre's own shape is at most a few units across.
        extra = extra.max(4.0 * max_scale);
    }
    cull.keeps(centre, radius + extra)
}

/// Where a ghost's nodes and its images' nodes are at the pose a frame draws them in, for what hangs from them (an image state's
/// particle emitter: `updateImageAnimation` asks `getRenderImageTransform(slot, node)`, the image's render transform times the
/// node's in the image's pose; a light at a mount point). The shapes it loads are kept by path.
#[derive(Default)]
pub struct ImageFrames {
    models: Models,
}

impl ImageFrames {
    /// The ghost posed as the frame poses it (its live pose, mounts and vehicles): its shape, the posed instance, its render
    /// transform and scale.
    fn owner_posed<W: ShapeWorld + ?Sized, H: Hooks<W>>(
        &mut self,
        world: &W,
        hooks: &H,
        assets: &dyn Assets,
        ghost: u32,
        notes: &mut Vec<String>,
    ) -> Option<(Arc<Loaded>, ShapeInstance, Mat, [f32; 3])> {
        let records = world.records();
        let g = records.ghosts.get(&ghost)?;
        let owner_db = g.datablock()?;
        let owner_path = String::from_utf8_lossy(records.datablock_text(owner_db, "shapeFile")?)
            .replace('\\', "/");
        let pose = render_pose(world, ghost);
        let (xform, scale) =
            match mounted_transform(world, assets, &mut self.models, notes, ghost, 0) {
                Some(m) => (m, g.scale().unwrap_or([1.0; 3])),
                None => pose.as_ref().and_then(|p| pose_matrix(p, g.scale()))?,
            };
        let owner = self.models.get(
            &|p| records.constructor_sequences(p),
            assets,
            &owner_path,
            notes,
        )?;
        let dl = owner.model.shape.smallest_visible_dl.max(0) as usize;
        let dl = dl.min(owner.model.shape.details.len().saturating_sub(1));
        let shape = &owner.model.shape;
        let posed = match (g.record.as_ref(), pose) {
            (Some(_), Some(Pose::Euler { pitch, .. })) => {
                // The live pose, as the frame draws it.
                let live = if records.animate {
                    let ask = LiveAsk {
                        index: ghost,
                        ghost: g,
                        shape,
                        db: owner_db,
                        pitch,
                        dl,
                    };
                    let (back, posed) =
                        hooks.advance_live(world, ask, hooks.take_live(ghost), notes);
                    if back.is_some() {
                        hooks.put_back(ghost, back, Vec::new());
                    }
                    posed
                } else {
                    None
                };
                match live {
                    Some(instance) => instance,
                    None => {
                        let mut charge = |_: u64| true;
                        let mut instance = ShapeInstance::new(shape, &mut charge).ok()?;
                        world.pose_at_rest(ghost, pose, shape, &mut instance, dl, notes);
                        instance
                    }
                }
            }
            _ => {
                let mut charge = |_: u64| true;
                ShapeInstance::new(shape, &mut charge).ok()?
            }
        };
        Some((owner, posed, xform, scale))
    }

    /// The world frames of ghost `ghost`'s nodes named `names` at the pose the frame draws it in: the render transform times the
    /// node's transform, the node's translation scaled by the object's scale. A node the shape lacks is `None`.
    pub fn owner_nodes<W: ShapeWorld + ?Sized, H: Hooks<W>>(
        &mut self,
        world: &W,
        hooks: &H,
        assets: &dyn Assets,
        ghost: u32,
        names: &[&[u8]],
    ) -> Option<Vec<Option<Mat>>> {
        let mut notes = Vec::new();
        let (owner, instance, xform, scale) =
            self.owner_posed(world, hooks, assets, ghost, &mut notes)?;
        let shape = &owner.model.shape;
        let node = |name: &[u8]| {
            let n = crate::images::node_named(shape, name)?;
            let m = instance.node_transform(n)?;
            Some(Mat::mul(
                &xform,
                &crate::images::scaled_translation(&Mat { m: *m }, scale),
            ))
        };
        Some(names.iter().map(|n| node(n)).collect())
    }

    /// The world point of ghost `ghost`'s `mount<N>` node as `getRenderMountTransform(N)` gives it: the shape's `mount<N>` node
    /// at the pose the frame draws, its translation scaled by the mount scale ([`ShapeWorld::mount_scale`]), through the render
    /// transform; the render transform's point when the shape has no such node.
    pub fn mount_point<W: ShapeWorld + ?Sized, H: Hooks<W>>(
        &mut self,
        world: &W,
        hooks: &H,
        assets: &dyn Assets,
        ghost: u32,
        mount: u32,
    ) -> Option<[f32; 3]> {
        let mut notes = Vec::new();
        let g = world.records().ghosts.get(&ghost)?;
        let (owner, instance, xform, scale) =
            self.owner_posed(world, hooks, assets, ghost, &mut notes)?;
        let scale = world.mount_scale(g, scale);
        let shape = &owner.model.shape;
        let node = (mount < 32)
            .then(|| crate::images::node_named(shape, format!("mount{mount}").as_bytes()))
            .flatten()
            .and_then(|n| instance.node_transform(n));
        let m = match node {
            Some(n) => Mat::mul(
                &xform,
                &crate::images::scaled_translation(&Mat { m: *n }, scale),
            ),
            None => xform,
        };
        Some([m.m[3], m.m[7], m.m[11]])
    }

    /// The world transform of node `node` of the image in `ghost`'s slot `slot` (a state's `stateEmitterNode`, as
    /// [`ShapeWorld::image_emitter_node`] maps it: TGE's -1 is the image shape's `muzzlePoint`), and whether it emits
    /// ([`ShapeWorld::image_emits`] in the owner's first-person view or not: TGE's always does).
    #[allow(clippy::too_many_arguments)]
    pub fn node_frame<W: ShapeWorld + ?Sized, H: Hooks<W>>(
        &mut self,
        world: &W,
        hooks: &H,
        assets: &dyn Assets,
        ghost: u32,
        slot: usize,
        node: i32,
    ) -> Option<(Mat, bool)> {
        let records = world.records();
        let g = records.ghosts.get(&ghost)?;
        let class = g.class?;
        let (_, image) = records
            .mounted_images(ghost)
            .into_iter()
            .find(|(s, _)| *s == slot)?;
        let db = ImageDb::of(world, image.datablock)?;
        let mut notes = Vec::new();
        let first_person = records.hide_control && records.control.object == Some(ghost);
        let camera = records.eye().map_or(Mat::IDENTITY, |e| {
            let mut m = Mat::mul(&Mat::euler_z(e.yaw), &Mat::euler_x(e.pitch));
            m.set_column3(e.position);
            m
        });
        let (owner, instance, xform, scale) =
            self.owner_posed(world, hooks, assets, ghost, &mut notes)?;
        let image_shape = self.models.get(
            &|p| records.constructor_sequences(p),
            assets,
            &db.shape_file,
            &mut notes,
        )?;
        let owner_view = Owner {
            kind: world.kind(class),
            scaled_eye_offset: world.scaled_eye_offset(),
            scaled_bare_mount: world.scaled_bare_mount(),
            motion: hooks.held_motion(ghost, slot),
            shape: &owner.model.shape,
            instance: &instance,
            xform,
            scale: world.mount_scale(g, scale),
            first_person,
            camera,
        };
        let full = owner_view.image_transform(&db, &image_shape.model.shape);
        let shape = &image_shape.model.shape;
        let muzzle = crate::images::node_named(shape, b"muzzlePoint").map_or(-1, |n| n as i32);
        let node = world.image_emitter_node(shape, node, muzzle);
        // The slot's clock, advanced as the frame advances it.
        let clock = if records.animate {
            let next = advance_clock(records, &image, hooks.take_clock(ghost, slot).as_ref());
            hooks.put_back(ghost, None, vec![((ghost, slot), next)]);
            next
        } else {
            advance_clock(records, &image, None)
        };
        let image_instance = crate::images::pose_at(shape, &image, &clock.now)?;
        let m = match usize::try_from(node)
            .ok()
            .and_then(|n| image_instance.node_transform(n))
        {
            Some(n) => Mat::mul(&full, &Mat { m: *n }),
            None => full,
        };
        Some((m, world.image_emits(image.datablock, first_person)))
    }
}

/// A node posed counts as this many vertices of [`crate::par`]'s work (a pose of a shape of some eighty nodes is about 3,300:
/// a lone one stays on the calling thread).
const POSE_WORK: usize = 40;

/// What the pose phase reads besides its job (shared by the pool's threads).
struct Frame<'a, W: ShapeWorld + ?Sized, H> {
    world: &'a W,
    hooks: &'a H,
    eye: [f32; 3],
    camera: Mat,
    pixels_per_meter: f32,
    policy: DetailPolicy,
    /// The kept builds are on ([`crate::shapecache::on`]): the pose phase makes their keys.
    keeping: bool,
    /// The kept builds stay in their meshes' own space (the enhanced renderer's node transforms on the GPU:
    /// [`crate::shapes::MeshPlace`]).
    nodes: bool,
    /// The first-person body is cast as a shadow only ([`with_shadow_body`]; the pool's threads do not see the calling
    /// thread's setting).
    shadow_body: bool,
}

/// One ghost the place phase took, in the ghosts' order: its notes so far and whether it has a job (else placing it stopped).
struct Entry {
    index: u32,
    notes: Vec<String>,
    placed: bool,
}

/// A ghost to pose and build.
struct Job<'w, S: ClientSet, L> {
    class: S::Class,
    index: u32,
    ghost: &'w Ghost<S>,
    db: u32,
    path: String,
    loaded: Arc<Loaded>,
    xform: Mat,
    scale: [f32; 3],
    first_person: bool,
    pose: Option<Pose>,
    centre: [f32; 3],
    eye_distance: f32,
    max_scale: f32,
    dl: usize,
    /// Its live pose was taken out of the host's keeping ([`take_live`]), and the pose taken (none: its first live frame).
    live_out: bool,
    live: Option<L>,
    images: Vec<ImageJob>,
}

/// One mounted image of a job, as far as the place phase took it.
enum ImageJob {
    /// Its shape did not load: the note.
    NotLoaded(String),
    Pose(Box<ImagePose>),
}

struct ImagePose {
    slot: usize,
    image: MountedImage,
    db: ImageDb,
    path: String,
    loaded: Arc<Loaded>,
    /// Its clock was taken out of the host's keeping, and the clock taken (none: its first live frame).
    clock_out: bool,
    clock: Option<ImageClock>,
}

/// A job posed.
struct Posed<'w, S: ClientSet, L> {
    job: Job<'w, S, L>,
    /// The live pose to put back.
    live: Option<L>,
    /// The notes of its posing, in order.
    notes: Vec<String>,
    /// The body's pose and build (none: no pose, its note given).
    body: Option<Body>,
    images: Vec<ImagePosed>,
}

struct Body {
    instance: ShapeInstance,
    ifl_frames: BTreeMap<usize, String>,
    point: Vec<PointLight>,
    label: String,
    key: Option<Vec<u8>>,
    /// The objects its threads hide (their visibility at or under [`MIN_VISIBILITY`]), when a thread animates visibility.
    vis_hidden: Option<Vec<bool>>,
}

/// One image posed: its notes, its clock to put back, and its build (none: not drawn).
struct ImagePosed {
    notes: Vec<String>,
    clock: Option<((u32, usize), ImageClock)>,
    draw: Option<ImageDraw>,
}

struct ImageDraw {
    loaded: Arc<Loaded>,
    instance: ShapeInstance,
    full: Mat,
    rotation: Mat,
    distance: f32,
    sub_shape: i32,
    object_detail: i32,
    ifl_frames: BTreeMap<usize, String>,
    color_all: Option<[f32; 4]>,
    label: String,
    key: Option<Vec<u8>>,
    /// The copy cast as a shadow only where the first-person body holds it (the image drawn before the eye casts none).
    held: Option<HeldShadow>,
}

/// A first-person image's shadow-only copy where its owner holds it ([`ImageDraw::held`]).
struct HeldShadow {
    full: Mat,
    rotation: Mat,
    distance: f32,
    sub_shape: i32,
    object_detail: i32,
    label: String,
    key: Option<Vec<u8>>,
}

/// The images of ghost `index` the frame poses: none for the client's own Player's in its first-person view when the world
/// hides them (`ClientWorld::hide_own_items`, `$pref::Player::renderMyItems` 0); each with a datablock, its shape loaded and a
/// detail.
fn image_jobs<W: ShapeWorld + ?Sized>(
    world: &W,
    assets: &dyn Assets,
    models: &mut Models,
    notes: &mut Vec<String>,
    class: ClassOf<W>,
    index: u32,
    first_person: bool,
) -> Vec<ImageJob> {
    let records = world.records();
    let player = world.kind(class) == ShapeKind::Player;
    if player && first_person && records.hide_own_items {
        return Vec::new();
    }
    let mut out = Vec::new();
    for (slot, image) in records.mounted_images(index) {
        let Some(db) = ImageDb::of(world, image.datablock) else {
            continue;
        };
        let path = db.shape_file.clone();
        let Some(loaded) = models.get(&|p| records.constructor_sequences(p), assets, &path, notes)
        else {
            out.push(ImageJob::NotLoaded(format!("image {path}: not loaded")));
            continue;
        };
        // `prepRenderImage`: no render image for a shape with no detail.
        if loaded.model.shape.details.is_empty() {
            continue;
        }
        out.push(ImageJob::Pose(Box::new(ImagePose {
            slot,
            image,
            db,
            path,
            loaded,
            clock_out: false,
            clock: None,
        })));
    }
    out
}

/// Takes each job's live pose and image clocks out of the host's keeping (only while the world animates: a world that does not
/// keeps none).
fn take_live<W: ShapeWorld + ?Sized, H: Hooks<W>>(
    world: &W,
    hooks: &H,
    jobs: &mut [Job<'_, W::Set, H::Live>],
) {
    if !world.records().animate {
        return;
    }
    for j in jobs.iter_mut() {
        if j.ghost.record.is_some() && matches!(j.pose, Some(Pose::Euler { .. })) {
            j.live_out = true;
            j.live = hooks.take_live(j.index);
        }
        for i in &mut j.images {
            if let ImageJob::Pose(p) = i {
                p.clock_out = true;
                p.clock = hooks.take_clock(j.index, p.slot);
            }
        }
    }
}

/// A job's share of the pool's work: its nodes and its images' nodes.
fn weight<S: ClientSet, L>(job: &Job<'_, S, L>) -> usize {
    let images: usize = job
        .images
        .iter()
        .map(|i| match i {
            ImageJob::Pose(p) => p.loaded.model.shape.nodes.len(),
            ImageJob::NotLoaded(_) => 0,
        })
        .sum();
    (job.loaded.model.shape.nodes.len() + images) * POSE_WORK
}

/// The pose phase of `job`.
fn pose<'w, W: ShapeWorld + ?Sized, H: Hooks<W>>(
    f: &Frame<'_, W, H>,
    mut job: Job<'w, W::Set, H::Live>,
) -> Posed<'w, W::Set, H::Live> {
    let world = f.world;
    let mut notes = Vec::new();
    let model = &job.loaded.model;
    let shape = &model.shape;
    let index = job.index;
    let dl = job.dl;
    let mut live_back = None;
    // The live client's pose, advanced by the frame's time: its own instance posed at the detail drawn.
    let live = match job.pose {
        Some(Pose::Euler { pitch, .. }) if job.live_out => {
            let ask = LiveAsk {
                index,
                ghost: job.ghost,
                shape,
                db: job.db,
                pitch,
                dl,
            };
            let (kept, posed) = f
                .hooks
                .advance_live(world, ask, job.live.take(), &mut notes);
            live_back = kept;
            posed
        }
        _ => None,
    };
    let instance = match live {
        Some(instance) => Some(instance),
        None => pose_record(f, &job, &mut notes),
    };
    let Some(instance) = instance else {
        return Posed {
            job,
            live: live_back,
            notes,
            body: None,
            images: Vec::new(),
        };
    };
    let ifl = world.ifl_frames(Some(job.ghost), &job.loaded.ifls);
    let vis_hidden = vis_hidden(shape, &instance);
    let point = f.hooks.lights(job.centre, shape.radius * job.max_scale);
    let label = format!("{} {index} {}", W::Set::class_name(job.class), job.path);
    let detail = shape.details[dl];
    let nodes = job.loaded.node_spheres(f.nodes).is_some();
    // (The first-person body cast as a shadow only is kept as any body, under its own label: its ghost's object is then the
    // last frame's again, its meshes placed on the GPU with their last frame's places, so the held image's swing and the
    // body's limbs have their motion under a still camera.)
    let key = (f.keeping && (!job.first_person || f.shadow_body)).then(|| {
        shape_key_bytes(
            model,
            &instance,
            &ShapeParts {
                sub_shape: detail.sub_shape,
                object_detail: detail.object_detail,
                hidden: world.hidden_nodes(job.ghost).unwrap_or(&[]),
                colors: world.mesh_colors(job.ghost),
                ifl_frames: &ifl,
                hidden_objects: vis_hidden.as_deref(),
                skin: None,
                color_all: None,
            },
            &mat3_scale(&job.xform, job.scale),
            &job.xform,
            job.scale == [1.0; 3],
            &point,
            nodes,
        )
    });
    // The mounted images (`prepRenderImage` adds them after the body's own render image).
    let owner = Owner {
        kind: world.kind(job.class),
        scaled_eye_offset: world.scaled_eye_offset(),
        scaled_bare_mount: world.scaled_bare_mount(),
        motion: HeldMotion::default(),
        shape,
        instance: &instance,
        xform: job.xform,
        scale: world.mount_scale(job.ghost, job.scale),
        first_person: job.first_person,
        camera: f.camera,
    };
    let unscaled = job.scale == [1.0; 3];
    let images = job
        .images
        .iter_mut()
        .map(|i| pose_image(f, &owner, (job.class, index), unscaled, &point, i))
        .collect();
    Posed {
        live: live_back,
        notes,
        body: Some(Body {
            instance,
            ifl_frames: ifl,
            point,
            label,
            key,
            vis_hidden,
        }),
        images,
        job,
    }
}

/// The objects a shape's threads hide (TGE's `animateVisibility`: a static shape's script thread, a clock's digits), when a
/// thread animates visibility. A part-visible object (a fade) draws whole.
fn vis_hidden(shape: &Shape, instance: &ShapeInstance) -> Option<Vec<bool>> {
    instance.animates_visibility(shape).then(|| {
        instance
            .object_visibility(shape)
            .into_iter()
            .map(|v| v <= MIN_VISIBILITY)
            .collect()
    })
}

/// A body posed at rest (no live pose: [`ShapeWorld::pose_at_rest`]).
fn pose_record<W: ShapeWorld + ?Sized, H, L>(
    f: &Frame<'_, W, H>,
    job: &Job<'_, W::Set, L>,
    notes: &mut Vec<String>,
) -> Option<ShapeInstance> {
    let shape = &job.loaded.model.shape;
    let mut charge = |_: u64| true;
    let Ok(mut instance) = ShapeInstance::new(shape, &mut charge) else {
        notes.push(format!("shape {}: no pose", job.path));
        return None;
    };
    f.world
        .pose_at_rest(job.index, job.pose, shape, &mut instance, job.dl, notes);
    Some(instance)
}

/// The pose phase of one image (before its build).
fn pose_image<W: ShapeWorld + ?Sized, H: Hooks<W>>(
    f: &Frame<'_, W, H>,
    owner: &Owner<'_>,
    (class, index): (ClassOf<W>, u32),
    unscaled: bool,
    point: &[PointLight],
    job: &mut ImageJob,
) -> ImagePosed {
    let mut out = ImagePosed {
        notes: Vec::new(),
        clock: None,
        draw: None,
    };
    let p = match job {
        ImageJob::NotLoaded(note) => {
            out.notes.push(std::mem::take(note));
            return out;
        }
        ImageJob::Pose(p) => p,
    };
    let model = &p.loaded.model;
    let path = &p.path;
    let owner = &Owner {
        motion: f.hooks.held_motion(index, p.slot),
        ..*owner
    };
    let clock = advance_clock(f.world.records(), &p.image, p.clock.as_ref());
    if p.clock_out {
        out.clock = Some(((index, p.slot), clock));
    }
    let Some(instance) = crate::images::pose_at(&model.shape, &p.image, &clock.now) else {
        out.notes.push(format!("image {path}: no pose"));
        return out;
    };
    let full = owner.image_transform(&p.db, &model.shape);
    let Some((rotation, distance, detail)) =
        image_place(f, owner, model, &full, owner.first_person)
    else {
        return out;
    };
    if detail.sub_shape < 0 {
        out.notes
            .push(format!("image {path}: a billboard detail is not drawn"));
        return out;
    }
    let player = owner.kind == ShapeKind::Player;
    let ifl = f.world.ifl_frames(None, &p.loaded.ifls);
    let color_all = if player {
        f.world.player_image_color(p.image.datablock)
    } else {
        None
    };
    let label = format!(
        "{IMAGE_LABEL}{} of {} {index} {path}",
        p.slot,
        W::Set::class_name(class)
    );
    let nodes = p.loaded.node_spheres(f.nodes).is_some();
    let key_at = |full: &Mat, rotation: &Mat, detail: &shapes::shape::Detail| {
        f.keeping.then(|| {
            shape_key_bytes(
                model,
                &instance,
                &ShapeParts {
                    sub_shape: detail.sub_shape,
                    object_detail: detail.object_detail,
                    hidden: &[],
                    colors: None,
                    ifl_frames: &ifl,
                    hidden_objects: None,
                    skin: None,
                    color_all,
                },
                full,
                rotation,
                unscaled,
                point,
                nodes,
            )
        })
    };
    // In the enhanced renderer an image the first person draws before the eye (`eyeOffset`) casts no shadow from there; a copy where
    // the first-person body (cast as a shadow only) holds it casts in its place.
    let held = (f.shadow_body && owner.before_eye(&p.db))
        .then(|| {
            let full = owner.held_transform(&p.db, &model.shape);
            let (rotation, distance, detail) = image_place(f, owner, model, &full, false)?;
            (detail.sub_shape >= 0).then(|| HeldShadow {
                key: key_at(&full, &rotation, &detail),
                full,
                rotation,
                distance,
                sub_shape: detail.sub_shape,
                object_detail: detail.object_detail,
                label: shadow_body_label(&label),
            })
        })
        .flatten();
    let key = key_at(&full, &rotation, &detail);
    let label = if held.is_some() {
        view_model_label(&label)
    } else {
        label
    };
    out.draw = Some(ImageDraw {
        loaded: Arc::clone(&p.loaded),
        instance,
        full,
        rotation,
        distance,
        sub_shape: detail.sub_shape,
        object_detail: detail.object_detail,
        ifl_frames: ifl,
        color_all,
        label,
        key,
        held,
    });
    out
}

/// An image placed at `full`: the rotation its normals turn with, its distance from the eye and the detail it is drawn at
/// (`first_person`: its owner's view the detail is picked for).
fn image_place<W: ShapeWorld + ?Sized, H>(
    f: &Frame<'_, W, H>,
    owner: &Owner<'_>,
    model: &Model,
    full: &Mat,
    first_person: bool,
) -> Option<(Mat, f32, shapes::shape::Detail)> {
    // The normals turn with the transform's rotation (the scale taken back out of its columns).
    let rotation = {
        let s = owner.scale;
        let inv = [0, 1, 2].map(|k| if s[k] != 0.0 { 1.0 / s[k] } else { 1.0 });
        crate::images::scale_columns(full, inv)
    };
    let centre = full.mul_p(model.shape.center);
    let d = [0, 1, 2].map(|k| centre[k] - f.eye[k]);
    let distance = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
    let player_detail = owner.player_detail(f.world, model.shape.details.len(), first_person);
    let dl = match (f.policy, player_detail) {
        (DetailPolicy::Highest, _) => highest_detail(model),
        (_, Some(dl)) => Some(dl),
        (_, None) => {
            let max_scale = owner.scale.iter().copied().fold(0.0, f32::max).max(1e-6);
            detail_for(model, distance / max_scale, f.pixels_per_meter, f.policy)
        }
    };
    let detail = dl.and_then(|dl| model.shape.details.get(dl)).copied()?;
    Some((rotation, distance, detail))
}

/// What the build phase uses besides the posed job.
struct Build<'a, 't, W: ShapeWorld + ?Sized> {
    world: &'a W,
    assets: &'a dyn Assets,
    textures: &'a mut TextureSet<'t>,
    notes: &'a mut Vec<String>,
    eye: [f32; 3],
    pixels_per_meter: f32,
    policy: DetailPolicy,
    /// As [`Frame::nodes`].
    nodes: bool,
}

/// The build phase of a posed job: its notes, its body's build and tyres, its images' builds, each pushed as one thread pushes
/// them; then its live pose and clocks put back in the host's keeping.
fn build_posed<W: ShapeWorld + ?Sized, H: Hooks<W>>(
    b: &mut Build<'_, '_, W>,
    hooks: &H,
    models: &mut Models,
    out: &mut ShapeDraws,
    laps: &mut Laps,
    posed: Posed<'_, W::Set, H::Live>,
) {
    let Posed {
        job,
        live,
        notes,
        body,
        images,
    } = posed;
    b.notes.extend(notes);
    let mut clocks = Vec::new();
    match body {
        Some(body) => {
            build_body(b, models, out, &job, &body);
            laps.lap(Lap::Body);
            for i in images {
                b.notes.extend(i.notes);
                clocks.extend(i.clock);
                if let Some(d) = i.draw {
                    build_image(b, out, job.index, &body.point, job.scale == [1.0; 3], d);
                }
            }
            laps.lap(Lap::Images);
        }
        // No pose: the images were not posed, their clocks go back as they were.
        None => {
            for i in job.images {
                if let ImageJob::Pose(p) = i
                    && let Some(c) = p.clock
                {
                    clocks.push(((job.index, p.slot), c));
                }
            }
        }
    }
    if live.is_some() || !clocks.is_empty() {
        hooks.put_back(job.index, live, clocks);
    }
}

/// The body's build and its tyres, pushed.
fn build_body<W: ShapeWorld + ?Sized, L>(
    b: &mut Build<'_, '_, W>,
    models: &mut Models,
    out: &mut ShapeDraws,
    job: &Job<'_, W::Set, L>,
    body: &Body,
) {
    let model = &job.loaded.model;
    let detail = model.shape.details[job.dl];
    // (The body's and its tyres' calls go in the body's list.)
    let pushed = crate::shapecache::mark();
    // The client's own object in its first-person view: not drawn, or with the enhanced renderer drawn into the shadow maps only.
    let shadow_body = job.first_person && shadow_body();
    let shadow_label;
    let label = if shadow_body {
        shadow_label = shadow_body_label(&body.label);
        &shadow_label
    } else {
        &body.label
    };
    let (mut draws, translucent) = if job.first_person && !shadow_body {
        (Vec::new(), false)
    } else {
        shape_draws_keyed(
            model,
            &body.instance,
            ShapeParts {
                sub_shape: detail.sub_shape,
                object_detail: detail.object_detail,
                hidden: b.world.hidden_nodes(job.ghost).unwrap_or(&[]),
                colors: b.world.mesh_colors(job.ghost),
                ifl_frames: &body.ifl_frames,
                hidden_objects: body.vis_hidden.as_deref(),
                skin: None,
                color_all: None,
            },
            &mat3_scale(&job.xform, job.scale),
            &job.xform,
            job.scale == [1.0; 3],
            &body.point,
            b.textures,
            label,
            body.key.clone(),
            job.loaded.node_spheres(b.nodes),
        )
    };
    if !job.first_person
        && let Some(wheels) = b.world.wheels(job.ghost)
    {
        let wheels = Tires {
            world: b.world,
            assets: b.assets,
            wheels,
            look: b.world.records().vehicle_look.get(&job.index),
            vehicle: model,
            instance: &body.instance,
            xform: &job.xform,
            scale: job.scale,
            eye: b.eye,
            pixels_per_meter: b.pixels_per_meter,
            point: &body.point,
            policy: b.policy,
            nodes: b.nodes,
        };
        draws.extend(tire_draws(&wheels, models, b.textures, b.notes));
    }
    crate::shapecache::pushed(pushed, translucent);
    push(out, job.index, job.eye_distance, draws, translucent);
}

/// An image's build, pushed.
fn build_image<W: ShapeWorld + ?Sized>(
    b: &mut Build<'_, '_, W>,
    out: &mut ShapeDraws,
    index: u32,
    point: &[PointLight],
    unscaled: bool,
    d: ImageDraw,
) {
    let pushed = crate::shapecache::mark();
    let (draws, translucent) = shape_draws_keyed(
        &d.loaded.model,
        &d.instance,
        ShapeParts {
            sub_shape: d.sub_shape,
            object_detail: d.object_detail,
            hidden: &[],
            colors: None,
            ifl_frames: &d.ifl_frames,
            hidden_objects: None,
            skin: None,
            color_all: d.color_all,
        },
        &d.full,
        &d.rotation,
        unscaled,
        point,
        b.textures,
        &d.label,
        d.key,
        d.loaded.node_spheres(b.nodes),
    );
    crate::shapecache::pushed(pushed, translucent);
    push(out, index, d.distance, draws, translucent);
    if let Some(h) = d.held {
        let pushed = crate::shapecache::mark();
        let (draws, translucent) = shape_draws_keyed(
            &d.loaded.model,
            &d.instance,
            ShapeParts {
                sub_shape: h.sub_shape,
                object_detail: h.object_detail,
                hidden: &[],
                colors: None,
                ifl_frames: &d.ifl_frames,
                hidden_objects: None,
                skin: None,
                color_all: d.color_all,
            },
            &h.full,
            &h.rotation,
            unscaled,
            point,
            b.textures,
            &h.label,
            h.key,
            d.loaded.node_spheres(b.nodes),
        );
        crate::shapecache::pushed(pushed, translucent);
        push(out, index, h.distance, draws, translucent);
    }
}

/// A build's draws into the translucent shapes (one entry, at `distance`) or the opaque draws.
fn push(out: &mut ShapeDraws, index: u32, distance: f32, draws: Vec<Draw>, translucent: bool) {
    if translucent {
        out.translucent.push((distance, draws));
        out.translucent_owners.push(index);
    } else {
        out.opaque_owners
            .extend(std::iter::repeat_n(index, draws.len()));
        out.opaque.extend(draws);
    }
}

#[cfg(test)]
#[path = "moving_tests.rs"]
mod tests;
