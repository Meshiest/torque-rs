//! Generated mesh fixtures for the mesh verification: models made in code, written as glTF (`.gltf` with its buffer in a
//! `data:` URI) or as OBJ, and read back through `torque-import`'s readers, the way a file from an editor would come in.
//!
//! Five kinds by the seed: closed solids (boxes, turned and overlapping, a square ring, a star prism, a box with a `NULL`
//! face), open meshes (a terrain patch, single walls, a box without a top), authored hulls (`col_*`, `hull_*`, `Col-N`) under
//! drawn meshes, convex rocks with a two-sided sheet and mission volumes, and a mix (boxes stacked face to face, a terrain, a
//! star). Texture coordinates: one random map a face (so a face's triangles join into one polygon) or one a triangle. Even
//! seeds are glTF, odd ones OBJ; [`BIG`] is a terrain of 200,344 triangles (glTF), cut into pieces.

use crate::brush::{Brush, TexGen};
use crate::geom::{self, Plane, V3};
use crate::mesh::MeshInput;
use crate::verify::Rng;

/// The seed of the large fixture.
pub const BIG: u64 = 19;

/// A file's format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Gltf,
    Obj,
}

/// A generated file.
#[derive(Debug, Clone)]
pub struct Fixture {
    pub name: String,
    pub format: Format,
    pub bytes: Vec<u8>,
    /// The mission volumes it names.
    pub volumes: usize,
    pub triangles: usize,
}

/// A triangle in a mesh's own space, clockwise seen from the front: corners, texture coordinates, material.
type T = ([V3; 3], [[f64; 2]; 3], usize);

struct GMesh {
    name: String,
    tris: Vec<T>,
    /// Its node's translation and turn about z (degrees), and the node above it (an index of an earlier mesh's node).
    at: V3,
    yaw: f64,
    parent: Option<usize>,
}

#[derive(Default)]
struct Gen {
    meshes: Vec<GMesh>,
    /// Name and two-sidedness.
    materials: Vec<(String, bool)>,
}

impl Gen {
    fn mesh(&mut self, name: &str, tris: Vec<T>, at: V3, yaw: f64) -> usize {
        self.meshes.push(GMesh {
            name: name.to_string(),
            tris,
            at,
            yaw,
            parent: None,
        });
        self.meshes.len() - 1
    }

    /// A mesh's triangles in the model's space.
    fn placed(&self, k: usize) -> Vec<T> {
        let mut tris = self.meshes[k].tris.clone();
        let mut at = Some(k);
        while let Some(m) = at {
            let g = &self.meshes[m];
            let (s, c) = g.yaw.to_radians().sin_cos();
            let f = |p: V3| {
                V3::new(
                    p[0] * c - p[1] * s + g.at[0],
                    p[0] * s + p[1] * c + g.at[1],
                    p[2] + g.at[2],
                )
            };
            for t in &mut tris {
                t.0 = t.0.map(f);
            }
            at = g.parent;
        }
        tris
    }
}

/// A texture map for a face: random axes in its plane, scale and offset.
fn face_map(r: &mut Rng, n: V3) -> impl Fn(V3) -> [f64; 2] + use<> {
    let a = geom::unit(n.cross(r.unit())).unwrap_or(V3::new(1.0, 0.0, 0.0));
    let b = n.cross(a);
    let (sa, sb) = (r.range(0.2, 1.5), r.range(0.2, 1.5));
    let (oa, ob) = (r.range(-1.0, 1.0), r.range(-1.0, 1.0));
    move |p: V3| [p.dot(a) * sa + oa, p.dot(b) * sb + ob]
}

/// A convex polygon (clockwise from the front) as a fan of triangles: one map for it, or random coordinates a corner when
/// `scatter`.
fn fan(r: &mut Rng, poly: &[V3], material: usize, scatter: bool) -> Vec<T> {
    let n = geom::unit(-geom::area_vector(poly)).unwrap_or(V3::Z);
    let map = face_map(r, n);
    let mut out = Vec::new();
    for k in 1..poly.len() - 1 {
        let p = [poly[0], poly[k], poly[k + 1]];
        let uv = if scatter {
            [0; 3].map(|_| [r.range(-2.0, 2.0), r.range(-2.0, 2.0)])
        } else {
            p.map(&map)
        };
        out.push((p, uv, material));
    }
    out
}

fn brush_tris(r: &mut Rng, b: &Brush, material: impl Fn(usize) -> usize, scatter: bool) -> Vec<T> {
    let polys = b.polygons().unwrap_or_default();
    let mut out = Vec::new();
    for (k, p) in polys.iter().enumerate() {
        if p.len() >= 3 {
            out.extend(fan(r, p, material(k), scatter));
        }
    }
    out
}

fn cuboid(lo: V3, hi: V3) -> Brush {
    Brush::cuboid(lo, hi, "", |n| TexGen::world(n, 1.0))
}

/// A convex rock: a ball of random planes round `c`.
fn rock(r: &mut Rng, c: V3, radius: f64) -> Brush {
    loop {
        let planes: Vec<Plane> = (0..10 + r.below(10))
            .filter_map(|_| {
                let n = r.unit();
                Plane::new(n, c + n * radius)
            })
            .collect();
        let b = Brush::from_planes(&planes, "", |n| TexGen::world(n, 1.0));
        if b.polygons().is_ok() {
            return b;
        }
    }
}

/// A prism over a polygon star-shaped round its first point's... round `center`: caps fanned from the center, sides as quads.
fn prism(
    r: &mut Rng,
    outline: &[[f64; 2]],
    center: [f64; 2],
    z: (f64, f64),
    material: usize,
) -> Vec<T> {
    let mut out = Vec::new();
    let n = outline.len();
    let top = face_map(r, V3::new(0.0, 0.0, 1.0));
    let bottom = face_map(r, V3::new(0.0, 0.0, -1.0));
    // The outline runs counter-clockwise seen from above.
    for i in 0..n {
        let (a, b) = (outline[i], outline[(i + 1) % n]);
        let c = V3::new(center[0], center[1], z.1);
        let pa = V3::new(a[0], a[1], z.1);
        let pb = V3::new(b[0], b[1], z.1);
        out.push(([c, pb, pa], [c, pb, pa].map(&top), material));
        let c = V3::new(center[0], center[1], z.0);
        let pa = V3::new(a[0], a[1], z.0);
        let pb = V3::new(b[0], b[1], z.0);
        out.push(([c, pa, pb], [c, pa, pb].map(&bottom), material));
        let side = [
            V3::new(a[0], a[1], z.0),
            V3::new(a[0], a[1], z.1),
            V3::new(b[0], b[1], z.1),
            V3::new(b[0], b[1], z.0),
        ];
        out.extend(fan(r, &side, material, false));
    }
    out
}

fn star(points: usize, inner: f64, outer: f64, c: [f64; 2]) -> Vec<[f64; 2]> {
    (0..points * 2)
        .map(|k| {
            let a = k as f64 * std::f64::consts::PI / points as f64;
            let rad = if k % 2 == 0 { outer } else { inner };
            [c[0] + rad * a.cos(), c[1] + rad * a.sin()]
        })
        .collect()
}

/// A square ring round `c`: outer half side `o`, hole half side `i`, from `z0` to `z1`.
fn ring(r: &mut Rng, c: V3, o: f64, i: f64, h: f64, material: usize) -> Vec<T> {
    let outer = [[-o, -o], [o, -o], [o, o], [-o, o]];
    let hole = [[-i, -i], [i, -i], [i, i], [-i, i]];
    let p = |q: [f64; 2], z: f64| V3::new(c[0] + q[0], c[1] + q[1], c[2] + z);
    let mut out = Vec::new();
    for k in 0..4 {
        let (a, b) = (outer[k], outer[(k + 1) % 4]);
        let (x, y) = (hole[k], hole[(k + 1) % 4]);
        for quad in [
            [p(a, h), p(x, h), p(y, h), p(b, h)],
            [p(a, 0.0), p(b, 0.0), p(y, 0.0), p(x, 0.0)],
            [p(a, 0.0), p(a, h), p(b, h), p(b, 0.0)],
            [p(x, 0.0), p(y, 0.0), p(y, h), p(x, h)],
        ] {
            out.extend(fan(r, &quad, material, false));
        }
    }
    out
}

/// A terrain patch of `n` x `m` squares of side `step` from `origin`, heights random up to `rise`, one world map.
fn terrain(
    r: &mut Rng,
    origin: V3,
    n: usize,
    m: usize,
    step: f64,
    rise: f64,
    material: usize,
) -> Vec<T> {
    let mut h = vec![0.0; (n + 1) * (m + 1)];
    for v in &mut h {
        *v = r.range(0.0, rise);
    }
    let p = |i: usize, j: usize| {
        V3::new(
            origin[0] + i as f64 * step,
            origin[1] + j as f64 * step,
            origin[2] + h[j * (n + 1) + i],
        )
    };
    let uv = |q: V3| [q[0] * 0.25, -q[1] * 0.25];
    let mut out = Vec::with_capacity(n * m * 2);
    for j in 0..m {
        for i in 0..n {
            // Clockwise seen from above.
            let (a, b, c, d) = (p(i, j), p(i, j + 1), p(i + 1, j + 1), p(i + 1, j));
            out.push(([a, b, c], [a, b, c].map(uv), material));
            out.push(([a, c, d], [a, c, d].map(uv), material));
        }
    }
    out
}

fn quad(r: &mut Rng, corners: [V3; 4], material: usize) -> Vec<T> {
    fan(r, &corners, material, false)
}

fn generate(seed: u64) -> (Gen, usize) {
    let mut r = Rng::new(seed * 7 + 3);
    let mut g = Gen::default();
    for (name, two) in [
        ("stone", false),
        ("wood", false),
        ("metal", false),
        ("NULL", false),
        ("leaf", true),
    ] {
        g.materials.push((name.to_string(), two));
    }
    let mut volumes = 0;
    if seed == BIG {
        let tris = terrain(&mut r, V3::new(-95.0, -95.0, 0.0), 317, 316, 0.6, 3.0, 0);
        g.mesh("ground", tris, V3::splat(0.0), 0.0);
        return (g, 0);
    }
    match seed % 5 {
        0 => {
            for k in 0..3 + r.below(3) {
                let lo = [r.range(-12.0, 8.0), r.range(-12.0, 8.0), r.range(0.0, 3.0)];
                let hi = [0, 1, 2].map(|i| lo[i] + r.range(1.0, 5.0));
                let b = cuboid(V3::from(lo), V3::from(hi));
                let m = r.below(3);
                let scatter = k == 1;
                let tris = brush_tris(&mut r, &b, |_| m, scatter);
                let yaw = if k % 2 == 0 { r.range(0.0, 90.0) } else { 0.0 };
                g.mesh(&format!("box{k}"), tris, V3::splat(0.0), yaw);
            }
            let tris = ring(&mut r, V3::new(0.0, 0.0, 0.0), 3.0, 1.2, 1.5, 1);
            g.mesh(
                "ring",
                tris,
                V3::new(r.range(-5.0, 5.0), 14.0, 0.0),
                r.range(0.0, 45.0),
            );
            let tris = prism(
                &mut r,
                &star(5, 1.0, 2.5, [0.0, 0.0]),
                [0.0, 0.0],
                (0.0, 2.0),
                2,
            );
            let parent = g.mesh("star_base", Vec::new(), V3::new(-14.0, 0.0, 0.0), 30.0);
            let child = g.mesh("star", tris, V3::new(0.0, 2.0, 0.5), 15.0);
            g.meshes[child].parent = Some(parent);
            let b = cuboid(V3::new(10.0, -14.0, 0.0), V3::new(13.0, -11.0, 2.0));
            let tris = brush_tris(&mut r, &b, |f| if f == 5 { 3 } else { 0 }, false);
            g.mesh("crate", tris, V3::splat(0.0), 0.0);
        }
        1 => {
            let tris = terrain(&mut r, V3::new(-15.0, -15.0, 0.0), 24, 20, 1.25, 1.5, 0);
            g.mesh("ground", tris, V3::splat(0.0), 0.0);
            for k in 0..3 {
                let x = -10.0 + k as f64 * 8.0;
                let y = r.range(-8.0, 8.0);
                let tris = quad(
                    &mut r,
                    [
                        V3::new(x, y, 2.0),
                        V3::new(x, y, 5.0),
                        V3::new(x + 4.0, y + 1.0, 5.0),
                        V3::new(x + 4.0, y + 1.0, 2.0),
                    ],
                    1,
                );
                g.mesh(&format!("wall{k}"), tris, V3::splat(0.0), 0.0);
            }
            let b = cuboid(V3::new(5.0, 5.0, 2.0), V3::new(9.0, 8.0, 4.0));
            // No top: an open box.
            let tris = brush_tris(&mut r, &b, |_| 2, false);
            let up = tris
                .into_iter()
                .filter(|t| t.0.iter().any(|p| p[2] < 3.9))
                .collect();
            g.mesh("tub", up, V3::splat(0.0), 20.0);
        }
        2 => {
            let b = cuboid(V3::new(-6.0, -6.0, 0.0), V3::new(6.0, 6.0, 1.0));
            let mut tris = brush_tris(&mut r, &b, |_| 0, true);
            // A finer top: the slab's top cut into a grid of quads with scattered coordinates.
            tris.retain(|t| t.0.iter().any(|p| p[2] < 0.99));
            for i in 0..6 {
                for j in 0..6 {
                    let (x, y) = (-6.0 + i as f64 * 2.0, -6.0 + j as f64 * 2.0);
                    tris.extend(fan(
                        &mut r,
                        &[
                            V3::new(x, y, 1.0),
                            V3::new(x, y + 2.0, 1.0),
                            V3::new(x + 2.0, y + 2.0, 1.0),
                            V3::new(x + 2.0, y, 1.0),
                        ],
                        1,
                        (i + j) % 3 == 0,
                    ));
                }
            }
            g.mesh("platform", tris, V3::splat(0.0), 10.0);
            let b = cuboid(V3::new(-6.0, -6.0, 0.0), V3::new(6.0, 6.0, 1.0));
            let hull = brush_tris(&mut r, &b, |_| 0, false);
            g.mesh("col_platform", hull, V3::splat(0.0), 10.0);
            let rk = rock(&mut r, V3::new(9.0, 3.0, 2.0), 1.8);
            let drawn = brush_tris(&mut r, &rk, |_| 2, true);
            g.mesh("boulder", drawn, V3::splat(0.0), 0.0);
            let hull = brush_tris(&mut r, &rk, |_| 0, false);
            g.mesh("hull_boulder", hull, V3::splat(0.0), 0.0);
            let b = cuboid(V3::new(-2.0, 9.0, 0.0), V3::new(2.0, 10.0, 3.0));
            let wall = brush_tris(&mut r, &b, |_| 1, false);
            g.mesh("wall", wall.clone(), V3::splat(0.0), 0.0);
            g.mesh("Col-1", wall, V3::splat(0.0), 0.0);
        }
        3 => {
            for k in 0..3 {
                let c = [
                    r.range(-10.0, 10.0),
                    r.range(-10.0, 10.0),
                    r.range(1.0, 4.0),
                ];
                let radius = r.range(0.8, 2.5);
                let rk = rock(&mut r, V3::from(c), radius);
                let tris = brush_tris(&mut r, &rk, |f| f % 2, k == 0);
                g.mesh(&format!("rock{k}"), tris, V3::splat(0.0), 0.0);
            }
            let tris = quad(
                &mut r,
                [
                    V3::new(-3.0, 12.0, 0.0),
                    V3::new(-3.0, 12.0, 3.0),
                    V3::new(3.0, 12.5, 3.0),
                    V3::new(3.0, 12.5, 0.0),
                ],
                4,
            );
            g.mesh("hedge", tris, V3::splat(0.0), 0.0);
            for (name, lo, hi) in [
                ("water_pool", [-5.0, -15.0, -1.0], [5.0, -10.0, 0.5]),
                ("trigger_door", [10.0, 10.0, 0.0], [12.0, 11.0, 3.0]),
                ("physical_zone_lift", [-12.0, 8.0, 0.0], [-10.0, 10.0, 6.0]),
            ] {
                let b = cuboid(V3::from(lo), V3::from(hi));
                let tris = brush_tris(&mut r, &b, |_| 0, false);
                g.mesh(name, tris, V3::splat(0.0), 0.0);
                volumes += 1;
            }
        }
        _ => {
            let base = cuboid(V3::new(-8.0, -8.0, 0.0), V3::new(0.0, 0.0, 2.0));
            let tris = brush_tris(&mut r, &base, |_| 0, false);
            g.mesh("base", tris, V3::splat(0.0), 0.0);
            // Face to face on the base's top.
            let top = cuboid(V3::new(-6.0, -6.0, 2.0), V3::new(-2.0, -2.0, 4.0));
            let tris = brush_tris(&mut r, &top, |_| 1, false);
            g.mesh("top", tris, V3::splat(0.0), 0.0);
            let tris = terrain(&mut r, V3::new(2.0, 2.0, 0.0), 10, 12, 1.0, 0.8, 2);
            g.mesh("field", tris, V3::splat(0.0), 0.0);
            let tris = prism(
                &mut r,
                &star(6, 1.2, 2.0, [0.0, 0.0]),
                [0.0, 0.0],
                (0.0, 1.5),
                1,
            );
            g.mesh("star", tris, V3::new(8.0, -8.0, 0.0), 12.0);
        }
    }
    (g, volumes)
}

/// Torque's frame into a y-up file's.
fn y_up(p: V3) -> [f32; 3] {
    [p[0] as f32, p[2] as f32, -p[1] as f32]
}

fn write_gltf(g: &Gen) -> Vec<u8> {
    let mut bin: Vec<u8> = Vec::new();
    let mut views = Vec::new();
    let mut accessors = Vec::new();
    let mut add = |bin: &mut Vec<u8>,
                   data: Vec<u8>,
                   count: usize,
                   kind: &str,
                   comp: u32,
                   minmax: String| {
        while !bin.len().is_multiple_of(4) {
            bin.push(0);
        }
        views.push(format!(
            "{{\"buffer\":0,\"byteOffset\":{},\"byteLength\":{}}}",
            bin.len(),
            data.len()
        ));
        bin.extend_from_slice(&data);
        accessors.push(format!(
            "{{\"bufferView\":{},\"componentType\":{comp},\"count\":{count},\"type\":\"{kind}\"{minmax}}}",
            views.len() - 1
        ));
        accessors.len() - 1
    };
    let mut meshes = Vec::new();
    let mut nodes = Vec::new();
    for m in &g.meshes {
        let mut prims = Vec::new();
        for mat in 0..g.materials.len() {
            let tris: Vec<&T> = m.tris.iter().filter(|t| t.2 == mat).collect();
            if tris.is_empty() {
                continue;
            }
            let (mut pos, mut uv, mut idx) = (Vec::new(), Vec::new(), Vec::new());
            let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
            for (k, t) in tris.iter().enumerate() {
                // Clockwise in Torque's frame is counter-clockwise in glTF's: corners 0, 2, 1.
                for c in [0, 2, 1] {
                    let p = y_up(t.0[c]);
                    for a in 0..3 {
                        lo[a] = lo[a].min(p[a]);
                        hi[a] = hi[a].max(p[a]);
                    }
                    p.iter()
                        .for_each(|v| pos.extend_from_slice(&v.to_le_bytes()));
                    t.1[c]
                        .iter()
                        .for_each(|v| uv.extend_from_slice(&(*v as f32).to_le_bytes()));
                }
                for c in 0..3u32 {
                    idx.extend_from_slice(&(k as u32 * 3 + c).to_le_bytes());
                }
            }
            let n = tris.len() * 3;
            let mm = format!(
                ",\"min\":[{},{},{}],\"max\":[{},{},{}]",
                lo[0], lo[1], lo[2], hi[0], hi[1], hi[2]
            );
            let a_pos = add(&mut bin, pos, n, "VEC3", 5126, mm);
            let a_uv = add(&mut bin, uv, n, "VEC2", 5126, String::new());
            let a_idx = add(&mut bin, idx, n, "SCALAR", 5125, String::new());
            prims.push(format!(
                "{{\"attributes\":{{\"POSITION\":{a_pos},\"TEXCOORD_0\":{a_uv}}},\"indices\":{a_idx},\"material\":{mat}}}"
            ));
        }
        let mesh = if prims.is_empty() {
            String::new()
        } else {
            meshes.push(format!("{{\"primitives\":[{}]}}", prims.join(",")));
            format!(",\"mesh\":{}", meshes.len() - 1)
        };
        let t = y_up(m.at);
        let (s, c) = (m.yaw.to_radians() * 0.5).sin_cos();
        // A turn about Torque's z is one about glTF's y.
        nodes.push((
            format!(
                "{{\"name\":\"{}\",\"translation\":[{},{},{}],\"rotation\":[0,{},0,{}]{mesh}",
                m.name, t[0], t[1], t[2], s as f32, c as f32
            ),
            m.parent,
        ));
    }
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); nodes.len()];
    let mut roots = Vec::new();
    for (k, (_, p)) in nodes.iter().enumerate() {
        match p {
            Some(p) => children[*p].push(k),
            None => roots.push(k),
        }
    }
    let node_json: Vec<String> = nodes
        .iter()
        .zip(&children)
        .map(|((n, _), c)| {
            if c.is_empty() {
                format!("{n}}}")
            } else {
                let list: Vec<String> = c.iter().map(|x| x.to_string()).collect();
                format!("{n},\"children\":[{}]}}", list.join(","))
            }
        })
        .collect();
    let materials: Vec<String> = g
        .materials
        .iter()
        .map(|(n, two)| format!("{{\"name\":\"{n}\",\"doubleSided\":{two}}}"))
        .collect();
    let roots: Vec<String> = roots.iter().map(|x| x.to_string()).collect();
    format!(
        "{{\"asset\":{{\"version\":\"2.0\"}},\"scene\":0,\"scenes\":[{{\"nodes\":[{}]}}],\"nodes\":[{}],\"meshes\":[{}],\
         \"materials\":[{}],\"accessors\":[{}],\"bufferViews\":[{}],\"buffers\":[{{\"byteLength\":{},\
         \"uri\":\"data:application/octet-stream;base64,{}\"}}]}}",
        roots.join(","),
        node_json.join(","),
        meshes.join(","),
        materials.join(","),
        accessors.join(","),
        views.join(","),
        bin.len(),
        torque_bytes::base64::encode(&bin)
    )
    .into_bytes()
}

fn write_obj(g: &Gen) -> Vec<u8> {
    use std::fmt::Write;
    let mut s = String::new();
    let mut at = 0usize;
    for k in 0..g.meshes.len() {
        let tris = g.placed(k);
        if tris.is_empty() {
            continue;
        }
        let _ = writeln!(s, "o {}", g.meshes[k].name);
        let mut current = usize::MAX;
        for t in tris {
            if t.2 != current {
                current = t.2;
                let _ = writeln!(s, "usemtl {}", g.materials[current].0);
            }
            for c in [0, 2, 1] {
                let p = y_up(t.0[c]);
                let _ = writeln!(s, "v {} {} {}", p[0], p[1], p[2]);
                let _ = writeln!(s, "vt {} {}", t.1[c][0] as f32, 1.0 - t.1[c][1] as f32);
            }
            let _ = writeln!(s, "f {0}/{0} {1}/{1} {2}/{2}", at + 1, at + 2, at + 3);
            at += 3;
        }
    }
    s.into_bytes()
}

/// Fixture `seed` (module docs).
pub fn fixture(seed: u64) -> Fixture {
    let (g, volumes) = generate(seed);
    let format = if seed.is_multiple_of(2) {
        Format::Gltf
    } else {
        Format::Obj
    };
    let kind = if seed == BIG {
        "big terrain"
    } else {
        ["closed", "open", "authored", "rocks and volumes", "mixed"][(seed % 5) as usize]
    };
    let triangles = g.meshes.iter().map(|m| m.tris.len()).sum();
    Fixture {
        name: format!(
            "{kind} ({})",
            if format == Format::Gltf {
                "glTF"
            } else {
                "OBJ"
            }
        ),
        bytes: match format {
            Format::Gltf => write_gltf(&g),
            Format::Obj => write_obj(&g),
        },
        format,
        volumes,
        triangles,
    }
}

/// A fixture read back through `torque-import`'s reader of its format.
pub fn read(f: &Fixture) -> Result<MeshInput, String> {
    let options = torque_import::Options {
        box_hull: false,
        ..torque_import::Options::default()
    };
    let mut free = |_: u64| true;
    let mut none = |_: &str, _: usize| None;
    let model = match f.format {
        Format::Gltf => torque_import::gltf::read(&f.bytes, &mut none, &options, &mut free),
        Format::Obj => torque_import::obj::read(&f.bytes, &mut none, &options, &mut free),
    }
    .map_err(|e| e.to_string())?;
    let input = MeshInput::from_model(&model).map_err(|e| e.to_string())?;
    let read: usize = input
        .drawn
        .iter()
        .chain(&input.authored)
        .map(|m| m.tris.len())
        .sum();
    // Volumes are read as objects, not triangles; two-sided sheets twice.
    if read > f.triangles * 2 {
        return Err(format!("{read} triangles read of {}", f.triangles));
    }
    Ok(input)
}
