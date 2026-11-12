//! The stages' shapes, made here: any triangles as an object the renderer draws (lit, or a glow), boxes, grass blades, a
//! tree (a trunk, branches and a crown of tufts) and a glowing ribbon along a polyline (the magic circle's figure).

use std::ops::Range;

use glam::Vec3;
use torque_render_scene::hd::{DynamicObject, HdDraw, material};
use torque_render_scene::math::Mat;
use torque_render_scene::{Blend, Cull, Draw, Filter, Sampler, TexEnv, Vertex};

use crate::effects::Lcg;

pub fn sampler(repeat: bool) -> Sampler {
    Sampler {
        repeat,
        mag: Filter::Linear,
        min: Filter::Linear,
        mip: Filter::Linear,
        mipmaps: true,
    }
}

pub fn vertex(p: Vec3, n: Vec3, color: [u8; 4], uv: [f32; 2]) -> Vertex {
    Vertex {
        position: p.to_array(),
        normal: n.to_array(),
        color,
        uv,
    }
}

/// A shape of `vertices` (triangles) as an object: lit and opaque, or a glow (added over the frame, unlit, in the HDR range the
/// bloom picks up, casting no shadow), `texture` (a scene table index) modulating its colors.
pub fn mesh(
    ghost: u32,
    label: &str,
    vertices: Vec<Vertex>,
    glow: bool,
    texture: Option<usize>,
) -> DynamicObject {
    let (lo, hi) = vertices.iter().fold(
        (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)),
        |(lo, hi), v| {
            let p = Vec3::from_array(v.position);
            (lo.min(p), hi.max(p))
        },
    );
    let draw = Draw {
        label: label.into(),
        vertices,
        lighting: !glow,
        tex_env: if texture.is_some() {
            TexEnv::Modulate
        } else {
            TexEnv::None
        },
        texture: texture.map(|_| (0, sampler(false))),
        blend: if glow { Blend::Additive } else { Blend::Opaque },
        cull: Cull::None,
        depth_test: true,
        depth_write: !glow,
        fog: true,
        lights: Vec::new(),
        vertex_key: None,
    };
    let mut hd = HdDraw::empty();
    let textures: Vec<usize> = texture.into_iter().collect();
    hd.assign_fixed(
        &draw,
        material::KIND_SHAPE,
        &textures,
        if glow {
            material::GLOW | material::VIEW_ONLY
        } else {
            0
        },
    );
    DynamicObject {
        ghost,
        transform: Mat::IDENTITY,
        prev_transform: Mat::IDENTITY,
        translucent: glow,
        centre: ((lo + hi) * 0.5).to_array(),
        radius: (hi - lo).length() * 0.5 + 0.01,
        draws: vec![hd],
        previous: None,
        stamp: None,
        nodes: Vec::new(),
    }
}

/// A glowing decal of `vertices` (triangles on a surface): added over the frame, unlit, in the HDR range the bloom picks up,
/// never faded into the surface it lies on, `texture` (an index into the frame's texture set) modulating its colors.
pub fn decal(label: &str, vertices: Vec<Vertex>, texture: Option<usize>) -> Draw {
    Draw {
        label: label.into(),
        vertices,
        lighting: false,
        tex_env: if texture.is_some() {
            TexEnv::Modulate
        } else {
            TexEnv::None
        },
        texture: texture.map(|i| (i, sampler(false))),
        blend: Blend::Additive,
        cull: Cull::None,
        depth_test: true,
        depth_write: false,
        fog: true,
        lights: Vec::new(),
        vertex_key: None,
    }
}

/// A log: a box from `a` to `b`, `half` thick each way across, in `color`.
pub fn log(a: Vec3, b: Vec3, half: f32, color: [u8; 3], out: &mut Vec<Vertex>) {
    let along = (b - a).normalize_or(Vec3::Z);
    let side = along
        .cross(if along.z.abs() < 0.9 {
            Vec3::Z
        } else {
            Vec3::X
        })
        .normalize_or(Vec3::X);
    let up = side.cross(along);
    let rgba = [color[0], color[1], color[2], 255];
    let center = (a + b) * 0.5;
    let axes = [along, side, up];
    let halves = [a.distance(b) * 0.5, half, half];
    for axis in 0..3 {
        for sign in [-1.0f32, 1.0] {
            let n = axes[axis] * sign;
            let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
            let corner = |p: f32, q: f32| {
                vertex(
                    center
                        + n * halves[axis]
                        + axes[u] * (p * halves[u])
                        + axes[v] * (q * halves[v]),
                    n,
                    rgba,
                    [0.0; 2],
                )
            };
            let (p0, p1, p2, p3) = (
                corner(-1.0, -1.0),
                corner(1.0, -1.0),
                corner(1.0, 1.0),
                corner(-1.0, 1.0),
            );
            out.extend([p0, p1, p2, p0, p2, p3]);
        }
    }
}

/// A box's six faces.
pub fn cuboid(center: Vec3, half: Vec3, color: [u8; 3], out: &mut Vec<Vertex>) {
    let rgba = [color[0], color[1], color[2], 255];
    for axis in 0..3 {
        for sign in [-1.0f32, 1.0] {
            let mut n = Vec3::ZERO;
            n[axis] = sign;
            let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
            let corner = |a: f32, b: f32| {
                let mut p = center;
                p[axis] += sign * half[axis];
                p[u] += a * half[u];
                p[v] += b * half[v];
                vertex(p, n, rgba, [0.0; 2])
            };
            let (a, b, c, d) = (
                corner(-1.0, -1.0),
                corner(1.0, -1.0),
                corner(1.0, 1.0),
                corner(-1.0, 1.0),
            );
            out.extend([a, b, c, a, c, d]);
        }
    }
}

/// `count` grass blades over `x` by `y`: each a thin triangle leaning its own way, dark green, lighter at the tip.
pub fn blades(seed: u64, count: usize, x: Range<f32>, y: Range<f32>, out: &mut Vec<Vertex>) {
    let mut g = Lcg(seed);
    for _ in 0..count {
        let base = Vec3::new(g.range(x.start, x.end), g.range(y.start, y.end), 0.0);
        let height = g.range(0.16, 0.5);
        let facing = g.range(0.0, std::f32::consts::TAU);
        let lean = g.range(0.1, 0.4) * height;
        let lean_dir = g.range(0.0, std::f32::consts::TAU);
        let across = Vec3::new(facing.cos(), facing.sin(), 0.0) * 0.02;
        let tip = base + Vec3::new(lean_dir.cos() * lean, lean_dir.sin() * lean, height);
        let n = Vec3::new(-facing.sin(), facing.cos(), 0.6).normalize();
        let shade = g.range(0.0, 1.0);
        let foot = [
            (16.0 + 10.0 * shade) as u8,
            (30.0 + 16.0 * shade) as u8,
            (12.0 + 6.0 * shade) as u8,
            255,
        ];
        let top = [
            (30.0 + 16.0 * shade) as u8,
            (52.0 + 26.0 * shade) as u8,
            (18.0 + 8.0 * shade) as u8,
            255,
        ];
        out.extend([
            vertex(base - across, n, foot, [0.0; 2]),
            vertex(base + across, n, foot, [0.0; 2]),
            vertex(tip, n, top, [0.0; 2]),
        ]);
    }
}

/// A tree's canopy: an ellipsoid of `radii` about `center`, its surface pushed in and out so it is not a smooth egg.
pub fn canopy(center: Vec3, radii: Vec3, seed: u64, color: [u8; 3], out: &mut Vec<Vertex>) {
    const RINGS: usize = 9;
    const SEGS: usize = 14;
    let mut g = Lcg(seed);
    let rgba = [color[0], color[1], color[2], 255];
    let mut points = Vec::with_capacity((RINGS + 1) * SEGS);
    for i in 0..=RINGS {
        let lat = std::f32::consts::PI * (i as f32 / RINGS as f32 - 0.5);
        for j in 0..SEGS {
            let lon = std::f32::consts::TAU * j as f32 / SEGS as f32;
            let dir = Vec3::new(lat.cos() * lon.cos(), lat.cos() * lon.sin(), lat.sin());
            let bump = if i == 0 || i == RINGS {
                1.0
            } else {
                g.range(0.9, 1.1)
            };
            points.push((center + dir * radii * bump, dir));
        }
    }
    let at = |i: usize, j: usize| points[i * SEGS + j % SEGS];
    for i in 0..RINGS {
        for j in 0..SEGS {
            let (a, b, c, d) = (at(i, j), at(i, j + 1), at(i + 1, j + 1), at(i + 1, j));
            let v = |(p, n): (Vec3, Vec3)| vertex(p, n, rgba, [0.0; 2]);
            out.extend([v(a), v(b), v(c), v(a), v(c), v(d)]);
        }
    }
}

/// A crown's tufts: `count` small lumps (center, radii) about `center` within `radii`, more toward the surface than the heart
/// (a tree's leaves and blossoms hang on the outside), each a tuft of radius `tuft` of `radii`'s largest, varied.
pub fn crown_tufts(
    center: Vec3,
    radii: Vec3,
    count: usize,
    tuft: f32,
    g: &mut Lcg,
) -> Vec<(Vec3, Vec3)> {
    let largest = radii.max_element();
    (0..count)
        .map(|_| {
            let z = g.range(-1.0, 1.0);
            let a = g.range(0.0, std::f32::consts::TAU);
            let s = (1.0 - z * z).sqrt();
            let r = 0.45 + 0.55 * g.unit().sqrt();
            let at = center + Vec3::new(s * a.cos(), s * a.sin(), z) * radii * r;
            let t = largest * tuft * g.range(0.6, 1.5);
            (
                at,
                Vec3::new(t, t * g.range(0.85, 1.1), t * g.range(0.6, 0.85)),
            )
        })
        .collect()
}

/// A tree at `foot`: a trunk `height` tall that tapers and bends a little, main branches leaning up and out from its upper
/// half toward the crown, and the crown's `tufts` (from [`crown_tufts`]) as lumpy canopies in `color` (each its own shade,
/// lighter at the top of the crown than under it).
pub fn tree(
    foot: Vec3,
    height: f32,
    tufts: &[(Vec3, Vec3)],
    bark: [u8; 3],
    color: [u8; 3],
    seed: u64,
    out: &mut Vec<Vertex>,
) {
    let mut g = Lcg(seed);
    // The trunk: three lengths, each leaning its own small way, thinner as they go.
    let mut at = foot;
    let mut top = foot + Vec3::Z * height;
    for (k, (from, to, half)) in [(0.0, 0.42, 0.055), (0.42, 0.74, 0.042), (0.74, 1.0, 0.03)]
        .into_iter()
        .enumerate()
    {
        let lean = Vec3::new(g.range(-0.08, 0.08), g.range(-0.08, 0.08), 0.0) * height;
        let next = foot + Vec3::Z * (height * to) + lean * (k as f32 + 1.0) * 0.5;
        let _ = from;
        log(at, next, half * height, bark, out);
        at = next;
        top = next;
    }
    // The branches: from the trunk's upper half toward the tufts farthest out, each thinning to its end.
    let (lo, hi) = (0.55, 0.9);
    let mut ends: Vec<Vec3> = tufts.iter().map(|(c, _)| *c).collect();
    ends.sort_by(|a, b| {
        (b - foot)
            .truncate()
            .length()
            .total_cmp(&(a - foot).truncate().length())
    });
    for (k, end) in ends.iter().take(6).enumerate() {
        let start =
            foot + Vec3::Z * (height * g.range(lo, hi)) + (top - foot - Vec3::Z * height) * 0.6;
        let elbow = start + (*end - start) * 0.55 + Vec3::Z * g.range(0.1, 0.4);
        let half = 0.018 * height * if k == 0 { 1.2 } else { 1.0 };
        log(start, elbow, half, bark, out);
        log(elbow, *end, half * 0.6, bark, out);
    }
    // The crown.
    let crown_z = tufts
        .iter()
        .map(|(c, _)| c.z)
        .fold((f32::MAX, f32::MIN), |(lo, hi), z| (lo.min(z), hi.max(z)));
    for (k, (center, radii)) in tufts.iter().enumerate() {
        let up = if crown_z.1 > crown_z.0 {
            (center.z - crown_z.0) / (crown_z.1 - crown_z.0)
        } else {
            0.5
        };
        let shade = 0.78 + 0.3 * up + g.range(-0.07, 0.07);
        let c = color.map(|v| (f32::from(v) * shade).min(255.0) as u8);
        canopy(*center, *radii, seed.wrapping_mul(31) + k as u64, c, out);
    }
}

/// A ribbon of `width` along `points` in the plane of `normal` (the floor's: z) with `colors` (one a point), its texture
/// across it (v 0 at the left edge, 1 at the right) and along it in units of its width (u: the texture's footprint on the
/// screen is then the same both ways, so the mips filter a line texture, constant along u, by the width it shows at; u
/// per quad would pick a mip by the quad's length and average the profile away). Gentle turns are mitered into one strip; a sharp corner (a star's point, a rune's angle)
/// ends the strip and starts another, each squared off `cap` past the corner (so the strips meet); open ends are squared
/// off `cap` past their points too. A path that returns to its first point is closed without ends.
pub fn ribbon(
    points: &[Vec3],
    colors: &[[u8; 4]],
    width: f32,
    cap: f32,
    normal: Vec3,
    out: &mut Vec<Vertex>,
) {
    let n = points.len();
    if n < 2 {
        return;
    }
    let closed = n > 3 && points[0].distance(points[n - 1]) < 1e-4;
    let tangent = |i: usize| -> Vec3 {
        let prev = if i == 0 {
            if closed { points[n - 2] } else { points[0] }
        } else {
            points[i - 1]
        };
        let next = if i + 1 == n {
            if closed { points[1] } else { points[n - 1] }
        } else {
            points[i + 1]
        };
        (next - prev).normalize_or(Vec3::X)
    };
    let step = |i: usize| -> Vec3 { (points[i + 1] - points[i]).normalize_or(Vec3::X) };
    // Each point's strip-start flag: a sharp corner (or an open end) starts a new strip at it.
    let sharp = |i: usize| -> bool {
        if i == 0 || i + 1 == n {
            !closed || step(n - 2).dot(step(0)) < 0.5
        } else {
            step(i - 1).dot(step(i)) < 0.5
        }
    };
    let half = width * 0.5;
    // Each point's distance along the path, in widths: the texture's u.
    let mut along_u = Vec::with_capacity(n);
    along_u.push(0.0f32);
    for w in points.windows(2) {
        along_u.push(along_u[along_u.len() - 1] + w[0].distance(w[1]) / width);
    }
    // A strip's edge points: left, right, u, color.
    let mut strip: Vec<(Vec3, Vec3, f32, [u8; 4])> = Vec::new();
    let flush = |strip: &mut Vec<(Vec3, Vec3, f32, [u8; 4])>, out: &mut Vec<Vertex>| {
        for w in strip.windows(2) {
            let ((l0, r0, u0, c0), (l1, r1, u1, c1)) = (w[0], w[1]);
            let n = normal;
            out.extend([
                vertex(l0, n, c0, [u0, 0.0]),
                vertex(r0, n, c0, [u0, 1.0]),
                vertex(r1, n, c1, [u1, 1.0]),
                vertex(l0, n, c0, [u0, 0.0]),
                vertex(r1, n, c1, [u1, 1.0]),
                vertex(l1, n, c1, [u1, 0.0]),
            ]);
        }
        strip.clear();
    };
    for i in 0..n {
        let color = colors[i];
        let u = along_u[i];
        let push_side =
            |strip: &mut Vec<(Vec3, Vec3, f32, [u8; 4])>, p: Vec3, along: Vec3, u: f32| {
                let side = along.cross(normal).normalize_or(Vec3::Y) * half;
                strip.push((p - side, p + side, u, color));
            };
        if sharp(i) {
            // The strip arriving here ends squared off past the corner; the one leaving starts squared off before it.
            if i > 0 {
                let along = step(i - 1);
                push_side(&mut strip, points[i] + along * cap, along, u + cap / width);
                flush(&mut strip, out);
            }
            if i + 1 < n {
                let along = step(i);
                push_side(&mut strip, points[i] - along * cap, along, u - cap / width);
            }
            continue;
        }
        let t = tangent(i);
        let miter = if i > 0 && i + 1 < n {
            // Widen at the turn so the strip's edges meet (capped: a turn past 60 degrees is sharp above).
            (1.0 / step(i - 1).dot(t).max(0.5)).min(1.2)
        } else {
            1.0
        };
        let side = t.cross(normal).normalize_or(Vec3::Y) * (half * miter);
        strip.push((points[i] - side, points[i] + side, u, color));
    }
    flush(&mut strip, out);
}

/// A beam from `a` to `b` of `width`: one quad turned to face `eye`, the line texture across it.
pub fn beam(a: Vec3, b: Vec3, width: f32, color: [u8; 4], eye: Vec3, out: &mut Vec<Vertex>) {
    let along = (b - a).normalize_or(Vec3::Y);
    let side = along.cross(eye - (a + b) * 0.5).normalize_or(Vec3::Z) * (width * 0.5);
    let n = side.cross(along).normalize_or(Vec3::Z);
    out.extend([
        vertex(a - side, n, color, [0.0, 0.0]),
        vertex(a + side, n, color, [0.0, 1.0]),
        vertex(b + side, n, color, [1.0, 1.0]),
        vertex(a - side, n, color, [0.0, 0.0]),
        vertex(b + side, n, color, [1.0, 1.0]),
        vertex(b - side, n, color, [1.0, 0.0]),
    ]);
}
