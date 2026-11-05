//! The buggy: a small open dune buggy, a tube frame with a roll cage and little body, two low seats side by side, big wheels
//! under rounded fenders on a wide stance; and its wheel, a shape of its own (a WheeledVehicle draws its tires from the tire
//! datablock's shape at each `hub<i>` node).
//!
//! | file | nodes | material |
//! |---|---|---|
//! | `buggy.dts` | `buggy`, `hub0`..`hub3`, `mount0`, `mount1`, `mount2`, `eye`, `cam`, `lamps_lit` | `buggy` |
//! | `buggy_wheel.dts` | `wheel` | `buggy` |
//!
//! **The buggy** faces +y with its origin on the ground line between its wheels: a floor pan between two lower rails, a nose
//! of tubes with a sloped panel, two round lamps and a bumper bar, a roll cage (front pillars, roof, main hoop behind the seats
//! and stays back to the engine), side bars at the hips, the engine and its exhaust in the open at the back, lamps on the roof's
//! front bar. The fenders are arches over the wheels on struts from the frame. The hubs are the wheel centers with the
//! springs unloaded: `hub0` front left, `hub1` front right, `hub2` rear left, `hub3` rear right. `mount0` is the driver's seat
//! (left), `mount1` the passenger's (right): a seated figure's origin, its hips [`SEAT_HIP`] above it on the cushion; `mount2` a
//! headlight's place between the roof's lamps ([`HEADLIGHT`]); `lamps_lit` carries the roof lamps' lit faces, which
//! [`LIGHTS_ON`] brings forward of the dark ones (drawn without lighting, so they glow). The steering wheel is a ring with three spokes on a short column, placed where the seated figure's hands grip ([`grips`]: the
//! figure's `sit` pose). `eye` is at the driver's eyes and `cam` above the middle of the cage. Its `Collision-1` and `LOS-9`
//! details are one box round the frame (the wheels are not in it: they hold it up on their springs).
//!
//! **The wheel** is a twenty-sided tire [`TIRE_RADIUS`] in radius and [`TIRE_WIDTH`] wide with two staggered rows of tread
//! blocks and a silver rim proud of both faces, its axle along y (a WheeledVehicle turns a tire's y axis out along the hub's
//! side).
//!
//! The texture (`buggy.png`, [`crate::textures::buggy`]) is a grid of swatches: the frame's paint, the body panels, the trim,
//! the seats, the lamps, the taillights, bare metal, the floor, the tire, the rim and the tread.

use std::f32::consts::TAU;

use torque_shapes::instance::{ShapeInstance, find_sequence};
use torque_shapes::math::mul_p;
use torque_shapes::shape::Model;

use crate::builder::{MeshData, SequenceDef, ShapeBuilder, Track, UvRect, Vec3};
use crate::builder::{material_flags, turn};
use crate::shapes::{GRIP, OPAQUE};

/// The hubs: across (x, each side), along (y, front and rear) and up (z).
pub const HUB_X: f32 = 1.08;
pub const HUB_FRONT: f32 = 1.3;
pub const HUB_REAR: f32 = -1.2;
pub const HUB_Z: f32 = 0.62;
/// The wheel.
pub const TIRE_RADIUS: f32 = 0.5;
pub const TIRE_WIDTH: f32 = 0.42;
/// The lamps on the roof's front bar: across (x), their faces' place along (y) and their height (z), their radius and depth.
pub const ROOF_LAMP_X: [f32; 4] = [-0.33, -0.11, 0.11, 0.33];
pub const ROOF_LAMP_Y: f32 = 0.39;
pub const ROOF_LAMP_Z: f32 = 1.87;
pub const ROOF_LAMP_RADIUS: f32 = 0.06;
pub const ROOF_LAMP_DEPTH: f32 = 0.08;
/// Where a headlight rides the buggy (`mount2`, its +y ahead): just ahead of the roof's lamps, between them; and how far it
/// dips below level (degrees), so its light falls on the ground ahead of the nose.
pub const HEADLIGHT: Vec3 = [0.0, ROOF_LAMP_Y + 0.08, ROOF_LAMP_Z];
pub const HEADLIGHT_DIP: f32 = 15.0;
/// The sequence that lights the roof's lamps (their lit faces, drawn without lighting, come forward of the dark ones at its
/// end); a game plays it while the headlight is on (held at its end) and stops it (back to its start) to put them out.
pub const LIGHTS_ON: &str = "lights_on";
/// How far behind the lamps' faces the lit faces wait inside the lamps while the lights are off.
const LIT_HIDDEN: f32 = 0.04;
/// How far ahead of the lamps' faces the lit faces are while the lights are on.
const LIT_AHEAD: f32 = 0.006;
const X_AXIS: Vec3 = [1.0, 0.0, 0.0];

/// The seats: the driver's (left) and the passenger's (right) seated figure's origin, and how high above it the hips sit.
pub const SEATS: [Vec3; 2] = [[-0.32, -0.3, -0.29], [0.32, -0.3, -0.29]];
pub const SEAT_HIP: f32 = 0.95;
/// The frame's half width at its rails.
pub const FRAME_X: f32 = 0.62;
/// The body's collision box.
pub const BODY_MIN: Vec3 = [-0.7, -1.95, 0.18];
pub const BODY_MAX: Vec3 = [0.7, 2.06, 1.9];
/// The tubes' radius.
pub const TUBE: f32 = 0.04;
/// The steering column's angle below level (degrees) and its length.
pub const COLUMN_DROP: f32 = 24.0;
pub const COLUMN_LENGTH: f32 = 0.46;
/// The buggy's steering lock (radians: its datablock's `maxSteeringAngle`) and how far the steering wheel turns at full lock
/// (degrees).
pub const MAX_STEERING: f32 = 0.55;
pub const WHEEL_LOCK: f32 = 120.0;
/// The keys of the `steering` sequence.
pub const STEERING_KEYS: usize = 17;

/// The swatches of `buggy.png`: a 4 x 4 grid, each cell inset a little so the sampling never reaches the next.
fn cell(swatch: (u32, u32)) -> UvRect {
    let e = 0.02;
    let (x, y) = (swatch.0 as f32 / 4.0, swatch.1 as f32 / 4.0);
    [x + e, y + e, x + 0.25 - e, y + 0.25 - e]
}

const FRAME: (u32, u32) = (0, 0);
const PANEL: (u32, u32) = (1, 0);
const TRIM: (u32, u32) = (2, 0);
const SEAT_CLOTH: (u32, u32) = (3, 0);
const LAMP: (u32, u32) = (0, 1);
const TAILLIGHT: (u32, u32) = (1, 1);
const METAL: (u32, u32) = (2, 1);
const FLOOR: (u32, u32) = (3, 1);
const TIRE: (u32, u32) = (0, 2);
const RIM: (u32, u32) = (1, 2);
const TREAD: (u32, u32) = (2, 2);

fn sub(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn add(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn scale(a: Vec3, k: f32) -> Vec3 {
    a.map(|v| v * k)
}

fn cross(a: Vec3, b: Vec3) -> Vec3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn unit(a: Vec3) -> Vec3 {
    let l = (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt().max(1e-6);
    a.map(|v| v / l)
}

/// The vertices of `mesh` from `from` on moved into a frame: a position `p` becomes `origin + p.x * axes[0] + p.y * axes[1] +
/// p.z * axes[2]`, a normal turns with it (`axes` right-handed and unit, so the winding is kept).
fn orient(mesh: &mut MeshData, from: usize, origin: Vec3, axes: [Vec3; 3]) {
    let turn =
        |v: Vec3| [0, 1, 2].map(|k| v[0] * axes[0][k] + v[1] * axes[1][k] + v[2] * axes[2][k]);
    for p in &mut mesh.positions[from..] {
        *p = add(origin, turn(*p));
    }
    for n in &mut mesh.normals[from..] {
        *n = turn(*n);
    }
}

/// A right-handed frame whose first axis is along `along` (the others any way round it).
fn frame_along(along: Vec3) -> [Vec3; 3] {
    let x = unit(along);
    let helper = if x[2].abs() < 0.9 {
        [0.0, 0.0, 1.0]
    } else {
        [0.0, 1.0, 0.0]
    };
    let z = unit(cross(x, helper));
    let y = cross(z, x);
    [x, y, z]
}

/// A round profile of `sides` points, radius `r`, in the (y, z) plane.
fn round(sides: usize, r: f32) -> Vec<[f32; 2]> {
    (0..sides)
        .map(|k| {
            let a = TAU * (k as f32 + 0.5) / sides as f32;
            [r * a.cos(), r * a.sin()]
        })
        .collect()
}

/// A tube of radius `r` from `a` to `b` (eight-sided).
fn tube(mesh: &mut MeshData, a: Vec3, b: Vec3, r: f32, swatch: (u32, u32)) {
    let d = sub(b, a);
    let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
    let from = mesh.positions.len();
    mesh.add_prism(&round(8, r), 0.0, len, cell(swatch), cell(swatch));
    orient(mesh, from, a, frame_along(d));
}

/// [`tube`] and its mirror across x = 0.
fn tube_pair(mesh: &mut MeshData, a: Vec3, b: Vec3, swatch: (u32, u32)) {
    tube(mesh, a, b, TUBE, swatch);
    let m = |p: Vec3| [-p[0], p[1], p[2]];
    tube(mesh, m(b), m(a), TUBE, swatch);
}

/// A box of half sizes `half` about `center`, along `axes` (right-handed, unit).
fn oriented_box(
    mesh: &mut MeshData,
    center: Vec3,
    axes: [Vec3; 3],
    half: Vec3,
    swatch: (u32, u32),
) {
    let from = mesh.positions.len();
    let uv = cell(swatch);
    mesh.add_box(half.map(|h| -h), half, |_| uv);
    orient(mesh, from, center, axes);
}

/// A box `min`..`max` along the shape's axes.
fn block(mesh: &mut MeshData, min: Vec3, max: Vec3, swatch: (u32, u32)) {
    let uv = cell(swatch);
    mesh.add_box(min, max, |_| uv);
}

/// [`block`] and its mirror across x = 0 (`min`, `max` on the left side, x negative).
fn block_pair(mesh: &mut MeshData, min: Vec3, max: Vec3, swatch: (u32, u32)) {
    block(mesh, min, max, swatch);
    block(
        mesh,
        [-max[0], min[1], min[2]],
        [-min[0], max[1], max[2]],
        swatch,
    );
}

/// A round lamp of radius `r` and depth `depth` at `at`, its face along `facing`.
fn lamp(mesh: &mut MeshData, at: Vec3, facing: Vec3, r: f32, depth: f32, swatch: (u32, u32)) {
    let from = mesh.positions.len();
    mesh.add_prism(&round(12, r), -depth, 0.0, cell(swatch), cell(TRIM));
    orient(mesh, from, at, frame_along(facing));
}

/// An arch over a wheel whose middle is `center`: `segments` slabs from `inner` to `outer` radius, `half_width` each side of
/// `center` in x, from angle `a0` to `a1` (degrees, 0 toward +y, 90 straight up).
#[allow(clippy::too_many_arguments)]
fn arch(
    mesh: &mut MeshData,
    center: Vec3,
    inner: f32,
    outer: f32,
    half_width: f32,
    a0: f32,
    a1: f32,
    segments: usize,
    swatch: (u32, u32),
) {
    let at = |r: f32, deg: f32| {
        let (s, c) = deg.to_radians().sin_cos();
        [center[1] + r * c, center[2] + r * s]
    };
    for k in 0..segments {
        let t0 = a0 + (a1 - a0) * k as f32 / segments as f32;
        let t1 = a0 + (a1 - a0) * (k + 1) as f32 / segments as f32;
        let quad = [at(inner, t0), at(outer, t0), at(outer, t1), at(inner, t1)];
        mesh.add_prism(
            &quad,
            center[0] - half_width,
            center[0] + half_width,
            cell(swatch),
            cell(swatch),
        );
    }
}

/// The points `local` of the figure's nodes `names` with its sequence `sequence` posed, in the figure's own space.
fn posed_figure<const N: usize>(sequence: &str, names: [&str; N], local: Vec3) -> [Vec3; N] {
    let model = crate::shapes::figure();
    let shape = &model.shape;
    let mut free = |_| true;
    let Ok(mut inst) = ShapeInstance::new(shape, &mut free) else {
        return [[0.0; 3]; N];
    };
    if let (Some(t), Some(seq)) = (
        inst.create_thread(shape),
        find_sequence(shape, sequence.as_bytes()).and_then(|s| i32::try_from(s).ok()),
    ) {
        inst.set_sequence(shape, t, seq, 0.0);
    }
    let _ = inst.animate(shape, 0, None, &mut free);
    names.map(|name| {
        let node = shape
            .find_name(name.as_bytes())
            .and_then(|n| shape.nodes.iter().position(|x| x.name == n));
        node.and_then(|i| inst.node_transforms().get(i))
            .map_or([0.0; 3], |m| mul_p(m, local))
    })
}

/// Where the seated figure's hands grip, left then right, in the figure's own space: its `sit` pose posed, the point
/// [`GRIP`] down each forearm (where the closed hand is).
pub fn grips() -> [Vec3; 2] {
    posed_figure("sit", ["elbow_l", "elbow_r"], [0.0, 0.0, -GRIP])
}

/// The seated figure's `eye` node in the buggy's space, in seat `seat` (0 the driver's `sit`, 1 the passenger's `ride`).
pub fn seated_eye(seat: usize) -> Vec3 {
    let pose = if seat == 0 { "sit" } else { "ride" };
    let [eye] = posed_figure(pose, ["eye"], [0.0; 3]);
    add(SEATS[seat.min(SEATS.len() - 1)], eye)
}

/// The steering wheel's middle and radius in the buggy's space: halfway between the driver's grips, as wide as they are apart.
pub fn steering_wheel() -> (Vec3, f32) {
    let [l, r] = grips();
    let mid = scale(add(l, r), 0.5);
    let radius = 0.5 * (r[0] - l[0]).abs();
    (add(SEATS[0], mid), radius)
}

/// The steering wheel (a ring of `sides` tubes with three spokes and a hub) and its short column down to the dash.
fn wheel_and_column(mesh: &mut MeshData, trim: &mut MeshData) {
    let (center, radius) = steering_wheel();
    let (s, c) = COLUMN_DROP.to_radians().sin_cos();
    // The column runs forward and down from the wheel's middle; the wheel faces back up it.
    let along = [0.0, c, -s];
    let x = [1.0, 0.0, 0.0];
    let up = cross(x, along);
    let rim = |deg: f32| {
        let (sa, ca) = deg.to_radians().sin_cos();
        add(center, add(scale(x, radius * ca), scale(up, radius * sa)))
    };
    let sides = 20;
    for k in 0..sides {
        let (d0, d1) = (
            360.0 * k as f32 / sides as f32,
            360.0 * (k + 1) as f32 / sides as f32,
        );
        tube(mesh, rim(d0), rim(d1), 0.022, TRIM);
    }
    for deg in [0.0, 180.0, 270.0] {
        tube(mesh, center, rim(deg), 0.014, METAL);
    }
    oriented_box(
        mesh,
        center,
        [x, cross(up, x), up],
        [0.045, 0.03, 0.045],
        TRIM,
    );
    tube(
        trim,
        center,
        add(center, scale(along, COLUMN_LENGTH)),
        0.025,
        METAL,
    );
}

/// The steering wheel's turn about its column (degrees, positive clockwise as the driver sees it) at `steering` (radians,
/// positive to the right) as the `steering` sequence plays it: TGE's thread sits at `0.5 - t / 2`, `t` the steering times its
/// own size over the lock, so the wheel turns [`WHEEL_LOCK`] at full lock.
pub fn wheel_turn(steering: f32) -> f32 {
    let t = steering * steering.abs() / MAX_STEERING;
    t / MAX_STEERING * WHEEL_LOCK
}

/// The `steering` sequence: its place 0 is full right, 0.5 straight ahead, 1 full left (see [`wheel_turn`]); the wheel turns
/// about the column, by `(1 - 2 * place) / lock` of [`WHEEL_LOCK`] (a full place's turn is past the lock: TGE's thread never
/// gets there).
fn steering_sequence(wheel: usize) -> SequenceDef {
    let (s, c) = COLUMN_DROP.to_radians().sin_cos();
    let along = [0.0, c, -s];
    let keys = (0..STEERING_KEYS)
        .map(|k| {
            let place = k as f32 / (STEERING_KEYS - 1) as f32;
            turn(along, (1.0 - 2.0 * place) / MAX_STEERING * WHEEL_LOCK)
        })
        .collect();
    SequenceDef {
        name: "steering",
        duration: 1.0,
        cyclic: false,
        priority: 0,
        keyframes: STEERING_KEYS,
        rotations: vec![Track { node: wheel, keys }],
        translations: Vec::new(),
        ground: Vec::new(),
    }
}

/// The buggy's body (module docs).
pub fn buggy() -> Model {
    let mut b = ShapeBuilder::new();
    let root = b.node("buggy", None, [0.0; 3]);
    for (i, (x, y)) in [
        (-HUB_X, HUB_FRONT),
        (HUB_X, HUB_FRONT),
        (-HUB_X, HUB_REAR),
        (HUB_X, HUB_REAR),
    ]
    .into_iter()
    .enumerate()
    {
        b.node(&format!("hub{i}"), Some(root), [x, y, HUB_Z]);
    }
    for (i, seat) in SEATS.iter().enumerate() {
        b.node(&format!("mount{i}"), Some(root), *seat);
    }
    // The headlight's place: between the roof's lamps, just ahead of them, facing ahead and down.
    b.node_turned(
        "mount2",
        Some(root),
        HEADLIGHT,
        turn(X_AXIS, -HEADLIGHT_DIP),
    );
    // The steering wheel's own node at its middle, turned about the column by `steering`.
    let (wheel_center, _) = steering_wheel();
    let wheel_node = b.node("steering_wheel", Some(root), wheel_center);
    b.node("eye", Some(root), [SEATS[0][0], -0.25, 1.4]);
    b.node("cam", Some(root), [0.0, -0.3, 1.5]);
    let drawn = b.detail("detail1", 1.0);
    let collision = b.detail("Collision-1", -1.0);
    let sight = b.detail("LOS-9", -1.0);
    let m = b.material("buggy", OPAQUE);

    let w = FRAME_X;
    let mut frame = MeshData::new(Some(m));
    // The lower rails, the nose and its bars.
    tube_pair(&mut frame, [-w, -1.25, 0.24], [-w, 1.15, 0.24], FRAME);
    tube_pair(&mut frame, [-w, 1.15, 0.24], [-0.32, 1.9, 0.4], FRAME);
    tube_pair(&mut frame, [-0.56, 0.62, 0.86], [-0.32, 1.9, 0.62], FRAME);
    tube_pair(&mut frame, [-0.32, 1.9, 0.4], [-0.32, 1.9, 0.62], FRAME);
    tube(
        &mut frame,
        [-0.32, 1.9, 0.62],
        [0.32, 1.9, 0.62],
        TUBE,
        FRAME,
    );
    tube(&mut frame, [-0.32, 1.9, 0.4], [0.32, 1.9, 0.4], TUBE, FRAME);
    tube_pair(&mut frame, [-0.32, 1.9, 0.4], [-0.5, 2.0, 0.38], FRAME);
    tube(&mut frame, [-0.5, 2.0, 0.38], [0.5, 2.0, 0.38], 0.05, METAL);
    // The cage: the front pillars over the dash, the roof, the main hoop behind the seats with its brace, the stays back to the
    // engine.
    tube_pair(&mut frame, [-w, 0.75, 0.24], [-0.56, 0.62, 0.86], FRAME);
    tube_pair(&mut frame, [-0.56, 0.62, 0.86], [-0.5, 0.32, 1.78], FRAME);
    tube(
        &mut frame,
        [-0.56, 0.62, 0.86],
        [0.56, 0.62, 0.86],
        TUBE,
        FRAME,
    );
    tube(
        &mut frame,
        [-0.5, 0.32, 1.78],
        [0.5, 0.32, 1.78],
        TUBE,
        FRAME,
    );
    tube_pair(&mut frame, [-0.5, 0.32, 1.78], [-0.52, -0.85, 1.8], FRAME);
    tube(
        &mut frame,
        [-0.52, -0.85, 1.8],
        [0.52, -0.85, 1.8],
        TUBE,
        FRAME,
    );
    tube_pair(&mut frame, [-0.52, -0.85, 1.8], [-w, -0.85, 0.24], FRAME);
    tube(
        &mut frame,
        [-0.52, -0.85, 1.8],
        [w, -0.85, 0.24],
        TUBE,
        FRAME,
    );
    tube_pair(&mut frame, [-0.52, -0.85, 1.8], [-0.45, -1.75, 0.85], FRAME);
    tube_pair(&mut frame, [-w, -1.25, 0.24], [-0.45, -1.85, 0.42], FRAME);
    tube_pair(
        &mut frame,
        [-0.45, -1.85, 0.42],
        [-0.45, -1.75, 0.85],
        FRAME,
    );
    tube(
        &mut frame,
        [-0.45, -1.75, 0.85],
        [0.45, -1.75, 0.85],
        TUBE,
        FRAME,
    );
    tube(
        &mut frame,
        [-0.45, -1.85, 0.42],
        [0.45, -1.85, 0.42],
        TUBE,
        FRAME,
    );
    // The side bars at the hips.
    tube_pair(&mut frame, [-w, -0.85, 0.6], [-0.6, 0.66, 0.66], FRAME);
    // The fenders' struts.
    for (y, to) in [
        (HUB_FRONT, [-0.43, HUB_FRONT, 0.73]),
        (HUB_REAR, [-0.49, HUB_REAR, 1.34]),
    ] {
        tube_pair(&mut frame, [-(HUB_X - 0.24), y, 1.08], to, FRAME);
    }
    b.object("frame", root, drawn, frame);

    // The fenders: rounded arches over the wheels.
    let mut fenders = MeshData::new(Some(m));
    for (x, y) in [
        (-HUB_X, HUB_FRONT),
        (HUB_X, HUB_FRONT),
        (-HUB_X, HUB_REAR),
        (HUB_X, HUB_REAR),
    ] {
        arch(
            &mut fenders,
            [x, y, 0.5],
            0.6,
            0.645,
            0.25,
            12.0,
            168.0,
            6,
            PANEL,
        );
    }
    b.object("fenders", root, drawn, fenders);

    // The body: the floor pan, the nose panel, the dash, the engine and its exhaust.
    let mut body = MeshData::new(Some(m));
    block(&mut body, [-w, -1.25, 0.18], [w, 0.75, 0.24], FLOOR);
    body.add_prism(
        &[[0.66, 0.9], [1.9, 0.67], [1.9, 0.62], [0.66, 0.85]],
        -0.36,
        0.36,
        cell(PANEL),
        cell(PANEL),
    );
    body.add_prism(
        &[[0.58, 0.74], [0.68, 0.74], [0.68, 0.9], [0.58, 0.9]],
        -0.56,
        0.56,
        cell(TRIM),
        cell(PANEL),
    );
    b.object("body", root, drawn, body);
    let mut engine = MeshData::new(Some(m));
    block(&mut engine, [-0.32, -1.7, 0.26], [0.32, -1.02, 0.66], METAL);
    for k in 0..5 {
        let y = -1.62 + 0.13 * k as f32;
        block(&mut engine, [-0.36, y, 0.4], [0.36, y + 0.05, 0.62], METAL);
    }
    let from = engine.positions.len();
    engine.add_prism(&round(12, 0.12), -0.08, 0.08, cell(TRIM), cell(TRIM));
    orient(
        &mut engine,
        from,
        [0.0, -1.2, 0.8],
        [[0.0, 0.0, 1.0], [0.0, 1.0, 0.0], [-1.0, 0.0, 0.0]],
    );
    tube(
        &mut engine,
        [-0.2, -1.5, 0.5],
        [-0.22, -1.98, 0.6],
        0.05,
        METAL,
    );
    tube(
        &mut engine,
        [0.2, -1.5, 0.5],
        [0.22, -1.98, 0.6],
        0.05,
        METAL,
    );
    b.object("engine", root, drawn, engine);

    // The lamps: two round ones in the nose, four on the roof's front bar, the taillights on the rear bar.
    let mut lamps = MeshData::new(Some(m));
    for x in [-0.25, 0.25] {
        lamp(
            &mut lamps,
            [x, 1.93, 0.66],
            [0.0, 1.0, 0.0],
            0.075,
            0.06,
            LAMP,
        );
    }
    for x in ROOF_LAMP_X {
        lamp(
            &mut lamps,
            [x, ROOF_LAMP_Y, ROOF_LAMP_Z],
            [0.0, 1.0, 0.0],
            ROOF_LAMP_RADIUS,
            ROOF_LAMP_DEPTH,
            LAMP,
        );
    }
    block_pair(
        &mut lamps,
        [-0.42, -1.8, 0.8],
        [-0.3, -1.78, 0.9],
        TAILLIGHT,
    );
    b.object("lamps", root, drawn, lamps);

    // The roof lamps' lit faces: drawn without lighting, on a node that waits inside the lamps and comes just forward of
    // their faces while `lights_on` plays.
    let lit_material = b.material("buggy", OPAQUE | material_flags::SELF_ILLUMINATING);
    let hidden = [0.0, ROOF_LAMP_Y - LIT_HIDDEN, ROOF_LAMP_Z];
    let shown = [0.0, ROOF_LAMP_Y + LIT_AHEAD, ROOF_LAMP_Z];
    let lit_node = b.node("lamps_lit", Some(root), hidden);
    let mut lit = MeshData::new(Some(lit_material));
    for x in ROOF_LAMP_X {
        lamp(
            &mut lit,
            [x, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            ROOF_LAMP_RADIUS * 0.85,
            0.004,
            LAMP,
        );
    }
    b.object("lamps_lit", lit_node, drawn, lit);
    b.sequence(SequenceDef {
        name: LIGHTS_ON,
        duration: 0.1,
        cyclic: false,
        priority: 0,
        keyframes: 2,
        rotations: Vec::new(),
        translations: vec![Track {
            node: lit_node,
            keys: vec![hidden, shown],
        }],
        ground: Vec::new(),
    });

    // The seats: a cushion and a backrest leaned back with the figure.
    let mut seats = MeshData::new(Some(m));
    let lean = crate::shapes::SIT_LEAN.to_radians();
    let up = [0.0, -lean.sin(), lean.cos()];
    let back = [0.0, lean.cos(), lean.sin()];
    for seat in SEATS {
        let (sx, sy) = (seat[0], seat[1]);
        let top = seat[2] + SEAT_HIP - 0.1;
        block(
            &mut seats,
            [sx - 0.22, sy - 0.32, 0.24],
            [sx + 0.22, sy + 0.18, top],
            SEAT_CLOTH,
        );
        oriented_box(
            &mut seats,
            [sx, sy - 0.27, top + 0.4],
            [[1.0, 0.0, 0.0], back, up],
            [0.21, 0.05, 0.42],
            SEAT_CLOTH,
        );
    }
    b.object("seats", root, drawn, seats);

    let mut wheel = MeshData::new(Some(m));
    let mut column = MeshData::new(Some(m));
    wheel_and_column(&mut wheel, &mut column);
    for p in &mut wheel.positions {
        *p = sub(*p, wheel_center);
    }
    b.object("steering_wheel", wheel_node, drawn, wheel);
    b.object("column", root, drawn, column);
    b.sequence(steering_sequence(wheel_node));

    b.object(
        "Col-1",
        root,
        collision,
        MeshData::hull_box(BODY_MIN, BODY_MAX),
    );
    b.object("LOS-9", root, sight, MeshData::hull_box(BODY_MIN, BODY_MAX));
    b.build()
}

/// The buggy's wheel (module docs).
pub fn wheel() -> Model {
    let mut b = ShapeBuilder::new();
    let root = b.node("wheel", None, [0.0; 3]);
    b.detail("detail1", 1.0);
    let m = b.material("buggy", OPAQUE);
    // Built with the axle along x, then turned so it lies along y.
    let to_y = [[0.0, 1.0, 0.0], [-1.0, 0.0, 0.0], [0.0, 0.0, 1.0]];
    let half = 0.5 * TIRE_WIDTH;
    let core = TIRE_RADIUS - 0.07;
    let mut tire = MeshData::new(Some(m));
    tire.add_prism(&round(20, core), -half, half, cell(TIRE), cell(TIRE));
    // Two rows of tread blocks, the second turned half a block from the first.
    let blocks = 20;
    for row in 0..2 {
        let (x0, x1) = if row == 0 {
            (0.02, half - 0.01)
        } else {
            (-half + 0.01, -0.02)
        };
        for k in 0..blocks {
            let a = TAU * (k as f32 + 0.5 * row as f32) / blocks as f32;
            let radial = [0.0, a.cos(), a.sin()];
            let tangent = [0.0, -a.sin(), a.cos()];
            let r = 0.5 * (core - 0.01 + TIRE_RADIUS);
            oriented_box(
                &mut tire,
                [0.5 * (x0 + x1), radial[1] * r, radial[2] * r],
                [[1.0, 0.0, 0.0], radial, tangent],
                [0.5 * (x1 - x0), 0.5 * (TIRE_RADIUS - core + 0.01), 0.055],
                TREAD,
            );
        }
    }
    orient(&mut tire, 0, [0.0; 3], to_y);
    b.object("tire", root, 0, tire);
    let mut rim = MeshData::new(Some(m));
    rim.add_prism(
        &round(16, 0.27),
        -half - 0.012,
        half + 0.012,
        cell(RIM),
        cell(RIM),
    );
    rim.add_prism(
        &round(8, 0.1),
        -half - 0.03,
        half + 0.03,
        cell(METAL),
        cell(METAL),
    );
    orient(&mut rim, 0, [0.0; 3], to_y);
    b.object("rim", root, 0, rim);
    b.build()
}
