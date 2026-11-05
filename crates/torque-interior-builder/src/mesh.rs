//! Meshes into interiors: a model of the model layer (`torque-modelcore`, from any reader of `torque-import`: glTF, OBJ,
//! Blockbench) built into one or more `.dif`s.
//!
//! 1. **The model's meshes** ([`MeshInput::from_model`]), at rest in the model's space, by what their names say
//!    (`torque_modelcore::naming::role`): drawn meshes (those of the model's first drawn detail), authored hulls (`col_*`,
//!    `hull_*`, `Col-N`, `Collision-N`), mission volumes (`water*`, `trigger*`, `physical_zone*`: each a
//!    [`MissionObject`] over its box). Line-of-sight meshes and other details are left out with a note.
//! 2. **Drawn polygons:** each mesh's triangles grouped into convex polygons of one plane, one material and one texture map
//!    (a surface has one texgen, linear in position: a triangle always has one, so any UV layout is exact). Normals are the
//!    planes'.
//! 3. **Hulls** ([`HullMode`]): the authored hulls (each the convex hull of its mesh's points; a mesh that is not convex is an
//!    error naming it); else each connected part of a drawn mesh: a closed one (every edge between two triangles, one each
//!    way, the volume positive) cut into the solid leaves of a BSP of its faces, each leaf a hull, exact; an open one as
//!    slabs, each polygon pushed back by [`MeshOptions::slab`] into a hull.
//! 4. **Pieces:** a set over a piece's limits (or over a cap of the version-0 interior) is cut in two at the middle of its
//!    longest side, again until every piece builds; each piece is a `.dif` at the same place (polygons and hulls go to the
//!    piece their middles fall in).
//!
//! Inputs are untrusted: every count is capped before the work it sizes, the BSP work is capped, coordinates are finite and
//! within [`MAX_COORD`].

use std::collections::HashMap;

use torque_modelcore::Model;
use torque_modelcore::naming::{Role, Volume, role};

use crate::brush::{Brush, Face, TexGen};
use crate::build::{BuildError, Built, Options, Polygon, build_polygons};
use crate::convex::{self, HullFail};
use crate::geom::{self, EPS, Plane, V3};
use crate::mission::{MissionObject, Shape};

/// The most triangles read from a model.
pub const MAX_TRIANGLES: usize = 1 << 21;
/// The farthest a vertex may be from the origin on any axis.
pub const MAX_COORD: f64 = 1.0e5;
/// The most hulls in all.
pub const MAX_HULLS: usize = 1 << 20;
/// The most polygon-against-plane tests of all the solid-leaf trees together.
pub const MAX_LEAF_WORK: u64 = 400_000_000;
/// A piece's limits before it is built: hulls and drawn polygons.
pub const PIECE_HULLS: usize = 1_000;
pub const PIECE_POLYGONS: usize = 8_000;
/// The most pieces.
pub const MAX_PIECES: usize = 4_096;
/// The most pieces that may fail to build (over a cap, then cut again): each failure is a whole build's work.
pub const MAX_FAILED_PIECES: usize = 64;
/// How far a point may be off a polygon's plane, and its texture coordinates off its map, to join it.
const PLANE_TOL: f64 = 2e-5;
const UV_TOL: f64 = 1e-5;
/// The most points of a drawn polygon (a surface's fan mask is 32 bits).
const MAX_POLYGON: usize = 32;

/// How a mesh's collision is made.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HullMode {
    /// Authored hulls when the model has any, else solid leaves for closed parts and slabs for open ones.
    #[default]
    Auto,
    /// The authored hulls only (an error when there are none).
    Authored,
    /// Slabs for every part, closed or not.
    Slabs,
}

/// How meshes are built.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MeshOptions {
    pub hulls: HullMode,
    /// How thick a slab is.
    pub slab: f64,
    pub build: Options,
    /// A piece's limits before it is built.
    pub piece_hulls: usize,
    pub piece_polygons: usize,
}

impl Default for MeshOptions {
    fn default() -> MeshOptions {
        MeshOptions {
            hulls: HullMode::Auto,
            slab: 0.1,
            build: Options::default(),
            piece_hulls: PIECE_HULLS,
            piece_polygons: PIECE_POLYGONS,
        }
    }
}

/// Why meshes are not built.
#[derive(Debug, Clone, PartialEq)]
pub enum MeshError {
    /// The model fails its check.
    Invalid(String),
    TooMany {
        what: &'static str,
        count: usize,
        cap: usize,
    },
    /// A vertex that is not finite or is farther than [`MAX_COORD`]: the mesh named.
    Coordinates(String),
    /// An authored hull that is not convex, flat or too large: the mesh named.
    Hull(String, String),
    /// Authored hulls asked for and none in the model, or no hull at all.
    NoHulls,
    /// A piece that cannot be built and cannot be cut further.
    Build(BuildError),
}

impl std::fmt::Display for MeshError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MeshError::Invalid(s) => write!(f, "invalid model: {s}"),
            MeshError::TooMany { what, count, cap } => {
                write!(f, "{count} {what}, over the cap of {cap}")
            }
            MeshError::Coordinates(m) => {
                write!(
                    f,
                    "mesh {m:?}: a vertex not finite or farther than {MAX_COORD}"
                )
            }
            MeshError::Hull(m, why) => write!(f, "hull {m:?}: {why}"),
            MeshError::NoHulls => write!(f, "no collision hulls"),
            MeshError::Build(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for MeshError {}

fn too_many(what: &'static str, count: usize, cap: usize) -> Result<(), MeshError> {
    if count > cap {
        Err(MeshError::TooMany { what, count, cap })
    } else {
        Ok(())
    }
}

/// A triangle of a mesh, clockwise seen from the front, with its texture coordinates and material.
#[derive(Debug, Clone, PartialEq)]
pub struct Tri {
    pub p: [V3; 3],
    pub uv: [[f64; 2]; 3],
    /// An index into [`MeshInput::materials`].
    pub material: usize,
}

impl Tri {
    /// The plane facing the front; None for a triangle of no area.
    pub fn plane(&self) -> Option<Plane> {
        let [a, b, c] = self.p;
        let n = geom::cross(geom::sub(b, a), geom::sub(c, a));
        if geom::length(n) < 1e-12 {
            return None;
        }
        Plane::new(geom::scale(n, -1.0), a)
    }
}

/// A named mesh's triangles.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct NamedMesh {
    pub name: String,
    pub tris: Vec<Tri>,
}

/// What a model gives an interior (module docs, step 1).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MeshInput {
    pub drawn: Vec<NamedMesh>,
    pub authored: Vec<NamedMesh>,
    /// Material names (a surface's texture name).
    pub materials: Vec<String>,
    pub objects: Vec<MissionObject>,
    pub notes: Vec<String>,
}

impl MeshInput {
    /// The model's meshes at rest (module docs, step 1). Two-sided materials draw both sides.
    pub fn from_model(model: &Model) -> Result<MeshInput, MeshError> {
        model.check().map_err(|e| MeshError::Invalid(e.0))?;
        let total: usize = model.meshes.iter().map(|m| m.triangles()).sum();
        too_many("triangles", total, MAX_TRIANGLES)?;
        let world = model.rest_world();
        // The drawn detail: the largest drawn size among the meshes' details (meshes of no detail are drawn).
        let size_of = |d: Option<usize>| d.and_then(|d| model.details.get(d)).map(|d| d.size);
        let drawn_size = model
            .meshes
            .iter()
            .filter_map(|m| size_of(m.detail))
            .filter(|s| *s >= 0.0)
            .fold(None, |a: Option<f32>, s| Some(a.map_or(s, |a| a.max(s))));
        let mut out = MeshInput::default();
        let mut material_ids: HashMap<String, usize> = HashMap::new();
        for (mi, mesh) in model.meshes.iter().enumerate() {
            let node_name = model.nodes.get(mesh.node).map_or("", |n| n.name.as_str());
            let name = if mesh.name.is_empty() {
                node_name
            } else {
                mesh.name.as_str()
            };
            let mut r = role(name);
            if r == Role::Visible && !node_name.is_empty() {
                r = role(node_name);
            }
            let positions: Vec<V3> = model
                .placed_positions(mi, &world)
                .iter()
                .map(|p| p.map(f64::from))
                .collect();
            if positions
                .iter()
                .flatten()
                .any(|v| !v.is_finite() || v.abs() > MAX_COORD)
            {
                return Err(MeshError::Coordinates(name.to_string()));
            }
            match r {
                Role::Los => {
                    out.notes
                        .push(format!("{name}: a line-of-sight mesh, left out"));
                    continue;
                }
                Role::Volume(v) => {
                    if positions.is_empty() {
                        continue;
                    }
                    let (lo, hi) = geom::bounds(positions.iter().copied());
                    out.objects.push(MissionObject {
                        class: match v {
                            Volume::Water => "WaterBlock",
                            Volume::Trigger => "Trigger",
                            Volume::PhysicalZone => "PhysicalZone",
                        }
                        .to_string(),
                        name: Some(name.to_string()),
                        shape: Shape::Volume(lo, hi),
                        yaw: 0.0,
                        fields: Vec::new(),
                    });
                    continue;
                }
                Role::Visible => {
                    if mesh.lod_of.is_some()
                        || size_of(mesh.detail).is_some_and(|s| Some(s) != drawn_size)
                    {
                        out.notes
                            .push(format!("{name}: another detail level, left out"));
                        continue;
                    }
                }
                Role::Collision => {}
            }
            let mut named = NamedMesh {
                name: name.to_string(),
                tris: Vec::new(),
            };
            for prim in &mesh.primitives {
                let material = prim.material.and_then(|m| model.materials.get(m));
                let mname = material.map_or("default", |m| m.name.as_str());
                let mname = if mname.is_empty() { "default" } else { mname };
                let next = material_ids.len();
                let id = *material_ids.entry(mname.to_string()).or_insert(next);
                if id == out.materials.len() {
                    out.materials.push(mname.to_string());
                }
                let two_sided = material.is_some_and(|m| m.two_sided) && r == Role::Visible;
                for t in prim.indices.chunks_exact(3) {
                    let v = [t[0], t[1], t[2]].map(|i| i as usize);
                    let uv = v.map(|i| mesh.uv0.get(i).map_or([0.0; 2], |u| u.map(f64::from)));
                    let tri = Tri {
                        p: v.map(|i| positions[i]),
                        uv,
                        material: id,
                    };
                    if two_sided {
                        named.tris.push(Tri {
                            p: [tri.p[0], tri.p[2], tri.p[1]],
                            uv: [tri.uv[0], tri.uv[2], tri.uv[1]],
                            material: id,
                        });
                    }
                    named.tris.push(tri);
                }
            }
            if r == Role::Collision {
                out.authored.push(named);
            } else {
                out.drawn.push(named);
            }
        }
        Ok(out)
    }
}

/// The texture planes that give a triangle's texture coordinates at its corners (each gradient in the triangle's plane); None
/// for a triangle of no area.
pub fn texgen_of(p: [V3; 3], uv: [[f64; 2]; 3]) -> Option<TexGen> {
    let e1 = geom::sub(p[1], p[0]);
    let e2 = geom::sub(p[2], p[0]);
    let (a11, a12, a22) = (geom::dot(e1, e1), geom::dot(e1, e2), geom::dot(e2, e2));
    let det = a11 * a22 - a12 * a12;
    if det <= 1e-20 * a11 * a22 || det <= 1e-24 {
        return None;
    }
    let row = |c: usize| {
        let (d1, d2) = (uv[1][c] - uv[0][c], uv[2][c] - uv[0][c]);
        let s = (d1 * a22 - d2 * a12) / det;
        let t = (d2 * a11 - d1 * a12) / det;
        let g = geom::add(geom::scale(e1, s), geom::scale(e2, t));
        [g[0], g[1], g[2], uv[0][c] - geom::dot(g, p[0])]
    };
    let tg = TexGen {
        u: row(0),
        v: row(1),
    };
    (tg.u.iter().chain(&tg.v).all(|x| x.is_finite())).then_some(tg)
}

/// Vertices by their exact position, numbered.
#[derive(Default)]
struct Welded {
    ids: HashMap<[u64; 3], u32>,
    points: Vec<V3>,
}

impl Welded {
    fn id(&mut self, p: V3) -> u32 {
        let key = p.map(|v| (v + 0.0).to_bits());
        *self.ids.entry(key).or_insert_with(|| {
            self.points.push(p);
            self.points.len() as u32 - 1
        })
    }
}

/// Whether the corner `b` (between `a` and `c`) of a polygon clockwise seen from the front of `n` is convex (or straight).
fn convex_corner(a: V3, b: V3, c: V3, n: V3) -> bool {
    let (u, w) = (geom::sub(b, a), geom::sub(c, b));
    let s = geom::length(u) * geom::length(w);
    s > 0.0 && geom::dot(geom::cross(u, w), n) <= 1e-9 * s
}

/// Triangles grouped into convex polygons: each grows from a seed triangle across its edges by the triangles `joins` lets in
/// (`joins(seed, other)`), while the polygon stays convex and at most [`MAX_POLYGON`] points. Returns each polygon's points
/// (welded vertex numbers) and seed; triangles of no area are left out.
fn group(
    w: &mut Welded,
    tris: &[Tri],
    joins: &dyn Fn(usize, usize) -> bool,
) -> Vec<(Vec<u32>, usize)> {
    let ids: Vec<[u32; 3]> = tris.iter().map(|t| t.p.map(|p| w.id(p))).collect();
    let mut by_edge: HashMap<(u32, u32), Vec<usize>> = HashMap::new();
    for (k, v) in ids.iter().enumerate() {
        if v[0] == v[1] || v[1] == v[2] || v[0] == v[2] || tris[k].plane().is_none() {
            continue;
        }
        for e in 0..3 {
            by_edge.entry((v[e], v[(e + 1) % 3])).or_default().push(k);
        }
    }
    let mut used = vec![false; tris.len()];
    let mut out = Vec::new();
    for seed in 0..tris.len() {
        let Some(plane) = tris[seed].plane() else {
            continue;
        };
        let v = ids[seed];
        if used[seed] || v[0] == v[1] || v[1] == v[2] || v[0] == v[2] {
            continue;
        }
        used[seed] = true;
        let mut poly: Vec<u32> = v.to_vec();
        let mut grew = true;
        while grew && poly.len() < MAX_POLYGON {
            grew = false;
            let n = poly.len();
            for i in 0..n {
                let (a, b) = (poly[i], poly[(i + 1) % n]);
                let Some(cands) = by_edge.get(&(b, a)) else {
                    continue;
                };
                let found = cands.iter().copied().find_map(|k| {
                    if used[k] || !joins(seed, k) {
                        return None;
                    }
                    let c = ids[k].into_iter().find(|&x| x != a && x != b)?;
                    if poly.contains(&c) {
                        return None;
                    }
                    let pt = |x: u32| w.points[x as usize];
                    let prev = poly[(i + n - 1) % n];
                    let next = poly[(i + 2) % n];
                    (convex_corner(pt(prev), pt(a), pt(c), plane.n)
                        && convex_corner(pt(a), pt(c), pt(b), plane.n)
                        && convex_corner(pt(c), pt(b), pt(next), plane.n))
                    .then_some((k, c))
                });
                if let Some((k, c)) = found {
                    used[k] = true;
                    poly.insert(i + 1, c);
                    grew = true;
                    break;
                }
            }
        }
        out.push((poly, seed));
    }
    out
}

/// Whether triangle `t` lies on `plane` facing its way.
fn on_plane(t: &Tri, plane: &Plane) -> bool {
    t.plane()
        .is_some_and(|q| geom::dot(q.n, plane.n) > 1.0 - 1e-9)
        && t.p.iter().all(|&p| plane.dist(p).abs() < PLANE_TOL)
}

/// A mesh's drawn polygons (module docs, step 2).
fn drawn_polygons(mesh: &NamedMesh, materials: &[String], out: &mut Vec<Polygon>) -> usize {
    let tris = &mesh.tris;
    let maps: Vec<Option<TexGen>> = tris.iter().map(|t| texgen_of(t.p, t.uv)).collect();
    let joins = |s: usize, k: usize| {
        let (Some(plane), Some(tg)) = (tris[s].plane(), maps[s]) else {
            return false;
        };
        tris[k].material == tris[s].material
            && on_plane(&tris[k], &plane)
            && (0..3).all(|c| {
                let want = tg.at(tris[k].p[c]);
                (0..2)
                    .all(|x| (want[x] - tris[k].uv[c][x]).abs() <= UV_TOL * want[x].abs().max(1.0))
            })
    };
    let mut w = Welded::default();
    let groups = group(&mut w, tris, &joins);
    let mut skipped = 0;
    for (poly, seed) in groups {
        let (Some(plane), Some(texgen)) = (tris[seed].plane(), maps[seed]) else {
            skipped += 1;
            continue;
        };
        out.push(Polygon {
            points: poly.iter().map(|&i| w.points[i as usize]).collect(),
            plane,
            material: materials[tris[seed].material].clone(),
            texgen,
        });
    }
    skipped
}

/// A hull and what it was made from.
#[derive(Debug, Clone, PartialEq)]
pub struct Hull {
    pub brush: Brush,
    pub kind: HullKind,
}

/// What a hull was made from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HullKind {
    /// Authored mesh `n` ([`MeshInput::authored`]).
    Authored(usize),
    /// A solid leaf of closed part `n` ([`Plan::solids`]).
    Leaf(usize),
    /// A slab of an open part.
    Slab,
}

/// A closed part of a drawn mesh: its triangles (the solid's surface).
#[derive(Debug, Clone, PartialEq)]
pub struct Solid {
    pub mesh: usize,
    pub tris: Vec<Tri>,
}

/// What is built: the drawn polygons, the hulls and the closed parts they came from.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Plan {
    pub polygons: Vec<Polygon>,
    pub hulls: Vec<Hull>,
    pub solids: Vec<Solid>,
    pub notes: Vec<String>,
}

fn null_face(plane: Plane) -> Face {
    Face {
        plane,
        material: "NULL".to_string(),
        texgen: TexGen::world(plane.n, 1.0),
    }
}

/// The brush of `planes` with the faces that bound nothing left out; None when they bound no solid.
fn pruned(planes: Vec<Plane>) -> Option<Brush> {
    let polys = crate::brush::polygons_of(planes.iter().copied()).ok()?;
    let faces: Vec<Face> = planes
        .into_iter()
        .zip(&polys)
        .filter(|(_, p)| !p.is_empty())
        .map(|(q, _)| null_face(q))
        .collect();
    let b = Brush { faces };
    b.polygons().ok()?;
    Some(b)
}

/// The connected parts of a mesh's triangles (by shared corners), each a list of triangle numbers.
fn parts(tris: &[Tri]) -> Vec<Vec<usize>> {
    let mut w = Welded::default();
    let ids: Vec<[u32; 3]> = tris.iter().map(|t| t.p.map(|p| w.id(p))).collect();
    let mut up: Vec<u32> = (0..w.points.len() as u32).collect();
    fn root(up: &mut [u32], mut x: u32) -> u32 {
        while up[x as usize] != x {
            up[x as usize] = up[up[x as usize] as usize];
            x = up[x as usize];
        }
        x
    }
    for v in &ids {
        for e in 1..3 {
            let (a, b) = (root(&mut up, v[0]), root(&mut up, v[e]));
            up[a as usize] = b;
        }
    }
    let mut by_root: HashMap<u32, usize> = HashMap::new();
    let mut out: Vec<Vec<usize>> = Vec::new();
    for (k, v) in ids.iter().enumerate() {
        let r = root(&mut up, v[0]);
        let at = *by_root.entry(r).or_insert_with(|| {
            out.push(Vec::new());
            out.len() - 1
        });
        out[at].push(k);
    }
    out
}

/// Whether triangles make a closed solid: every edge between two of them, one each way, and the volume they hold positive.
fn closed(tris: &[&Tri]) -> bool {
    let mut w = Welded::default();
    let mut edges: HashMap<(u32, u32), u32> = HashMap::new();
    let mut volume = 0.0;
    let mut any = false;
    for t in tris {
        let v = t.p.map(|p| w.id(p));
        if v[0] == v[1] || v[1] == v[2] || v[0] == v[2] {
            continue;
        }
        any = true;
        for e in 0..3 {
            *edges.entry((v[e], v[(e + 1) % 3])).or_default() += 1;
        }
        // Clockwise from outside: the usual signed volume comes out negative.
        volume -= geom::dot(t.p[0], geom::cross(t.p[1], t.p[2])) / 6.0;
    }
    let (lo, hi) = geom::bounds(tris.iter().flat_map(|t| t.p));
    let size = (0..3).map(|k| hi[k] - lo[k]).fold(0.0, f64::max);
    any && volume > (EPS * size * size).max(1e-12)
        && edges
            .iter()
            .all(|(&(a, b), &n)| n == 1 && edges.get(&(b, a)) == Some(&1))
}

/// The solid leaves of a closed part (module docs, step 3): a BSP of its faces, each splitter one of its faces' planes; a cell
/// with no face left is solid behind the splitter that made it and empty in front of it. Each leaf's planes are the
/// splitters on the way down (and the part's box, a unit out), the ones that bound nothing left out.
fn solid_leaves(tris: &[&Tri], work: &mut u64, out: &mut Vec<Brush>) -> Result<(), MeshError> {
    let mut w = Welded::default();
    let tri_list: Vec<Tri> = tris.iter().map(|t| (*t).clone()).collect();
    let groups = group(&mut w, &tri_list, &|s, k| {
        tri_list[s]
            .plane()
            .is_some_and(|p| on_plane(&tri_list[k], &p))
    });
    let polys: Vec<(Plane, Vec<V3>)> = groups
        .into_iter()
        .filter_map(|(poly, seed)| {
            Some((
                tri_list[seed].plane()?,
                poly.iter().map(|&i| w.points[i as usize]).collect(),
            ))
        })
        .collect();
    let (lo, hi) = geom::bounds(tris.iter().flat_map(|t| t.p));
    let mut boxed = Vec::new();
    for k in 0..3 {
        let mut n = [0.0; 3];
        n[k] = 1.0;
        boxed.extend(Plane::new(n, geom::add(hi, [1.0; 3])));
        n[k] = -1.0;
        boxed.extend(Plane::new(n, geom::sub(lo, [1.0; 3])));
    }
    // Each task: a cell's faces and the planes on the way down to it.
    type Cell = (Vec<(Plane, Vec<V3>)>, Vec<Plane>);
    let mut tasks: Vec<Cell> = vec![(polys, Vec::new())];
    while let Some((polys, path)) = tasks.pop() {
        // The splitter: of the first planes of the cell's faces, the one cutting fewest, then the most even.
        let mut candidates: Vec<Plane> = Vec::new();
        for (p, _) in &polys {
            if candidates.len() >= 32 {
                break;
            }
            if !candidates.iter().any(|c| c.coplanar(p)) {
                candidates.push(*p);
            }
        }
        *work = work.saturating_add((candidates.len() * polys.len()) as u64 + polys.len() as u64);
        if *work > MAX_LEAF_WORK {
            return Err(MeshError::TooMany {
                what: "solid-leaf tests",
                count: *work as usize,
                cap: MAX_LEAF_WORK as usize,
            });
        }
        let mut best = (u64::MAX, candidates[0]);
        for c in &candidates {
            let (mut cut, mut front, mut back) = (0u64, 0i64, 0i64);
            for (_, poly) in &polys {
                match geom::side(poly, c) {
                    geom::Side::Both => {
                        cut += 1;
                        front += 1;
                        back += 1;
                    }
                    geom::Side::Front => front += 1,
                    geom::Side::Back => back += 1,
                    geom::Side::On => {}
                }
            }
            let score = cut * 8 + (front - back).unsigned_abs();
            if score < best.0 {
                best = (score, *c);
            }
        }
        let s = best.1;
        let (mut front, mut back) = (Vec::new(), Vec::new());
        for (p, poly) in polys {
            match geom::side(&poly, &s) {
                geom::Side::On => {}
                geom::Side::Front => front.push((p, poly)),
                geom::Side::Back => back.push((p, poly)),
                geom::Side::Both => {
                    let (f, b) = geom::split(&poly, &s);
                    if !f.is_empty() {
                        front.push((p, f));
                    }
                    if !b.is_empty() {
                        back.push((p, b));
                    }
                }
            }
        }
        let mut back_path = path.clone();
        back_path.push(s);
        if back.is_empty() {
            too_many("hulls", out.len() + 1, MAX_HULLS)?;
            let planes: Vec<Plane> = back_path.iter().chain(&boxed).copied().collect();
            // A leaf's faces are each of its planes clipped by the others.
            *work = work.saturating_add((planes.len() * planes.len()) as u64);
            if *work > MAX_LEAF_WORK {
                return Err(MeshError::TooMany {
                    what: "solid-leaf tests",
                    count: *work as usize,
                    cap: MAX_LEAF_WORK as usize,
                });
            }
            if let Some(b) = pruned(planes) {
                out.push(b);
            }
        } else {
            tasks.push((back, back_path));
        }
        if !front.is_empty() {
            let mut front_path = path;
            front_path.push(s.flipped());
            tasks.push((front, front_path));
        }
    }
    Ok(())
}

/// A slab under a polygon: its face, its back `thickness` behind it, and a side through each edge: the plane halfway between
/// the polygon's and its neighbor's across that edge (`sides[i]`, the neighbor's normal, for edge `i`), so two neighbors'
/// slabs meet on one plane with no gap and no overlap; square to the polygon at an open edge or a fold sharper than 120
/// degrees.
fn slab(points: &[V3], plane: Plane, sides: &[Option<V3>], thickness: f64) -> Option<Brush> {
    let mut planes = vec![plane];
    let back = geom::sub(points[0], geom::scale(plane.n, thickness));
    planes.push(Plane::new(geom::scale(plane.n, -1.0), back)?);
    let n = points.len();
    for i in 0..n {
        let (a, b) = (points[i], points[(i + 1) % n]);
        if geom::length(geom::sub(b, a)) < EPS {
            continue;
        }
        let across = sides
            .get(i)
            .copied()
            .flatten()
            .filter(|m| geom::dot(*m, plane.n) > -0.5)
            .and_then(|m| geom::normalize(geom::add(plane.n, m)))
            .unwrap_or(plane.n);
        planes.push(Plane::new(geom::cross(across, geom::sub(b, a)), a)?);
    }
    pruned(planes)
}

impl Plan {
    /// The polygons and hulls of `input` (module docs, steps 2 and 3).
    pub fn new(input: &MeshInput, options: &MeshOptions) -> Result<Plan, MeshError> {
        if !(options.slab.is_finite() && options.slab > 0.0 && options.slab <= 1_000.0) {
            return Err(MeshError::Invalid(format!(
                "slab thickness {} is not between 0 and 1000",
                options.slab
            )));
        }
        let mut plan = Plan::default();
        let mut skipped = 0;
        for m in &input.drawn {
            skipped += drawn_polygons(m, &input.materials, &mut plan.polygons);
        }
        if skipped > 0 {
            plan.notes
                .push(format!("{skipped} triangles of no area left out"));
        }
        let authored = options.hulls != HullMode::Slabs && !input.authored.is_empty();
        if options.hulls == HullMode::Authored && !authored {
            return Err(MeshError::NoHulls);
        }
        if authored {
            for (k, m) in input.authored.iter().enumerate() {
                let pts: Vec<V3> = m.tris.iter().flat_map(|t| t.p).collect();
                let planes = convex::hull_planes(&pts).map_err(|e| {
                    MeshError::Hull(
                        m.name.clone(),
                        match e {
                            HullFail::Flat => "flat".into(),
                            HullFail::NotFinite => "a point not finite".into(),
                            HullFail::TooMany(n) => {
                                format!("{n} points, over {}", convex::MAX_POINTS)
                            }
                        },
                    )
                })?;
                // Convex: every triangle of the mesh on a face of its hull.
                if let Some(t) = m.tris.iter().find(|t| {
                    t.plane().is_some()
                        && !planes
                            .iter()
                            .any(|q| t.p.iter().all(|&p| q.dist(p).abs() < EPS * 10.0))
                }) {
                    return Err(MeshError::Hull(
                        m.name.clone(),
                        format!(
                            "not convex (the triangle at {:?} is inside its hull)",
                            t.p[0]
                        ),
                    ));
                }
                let brush = pruned(planes)
                    .ok_or_else(|| MeshError::Hull(m.name.clone(), "bounds no solid".into()))?;
                if brush.faces.len() > 255 {
                    return Err(MeshError::Hull(
                        m.name.clone(),
                        format!("{} faces, over 255", brush.faces.len()),
                    ));
                }
                plan.hulls.push(Hull {
                    brush,
                    kind: HullKind::Authored(k),
                });
            }
            return Ok(plan);
        }
        let mut work = 0u64;
        for (mi, m) in input.drawn.iter().enumerate() {
            for part in parts(&m.tris) {
                let tris: Vec<&Tri> = part.iter().map(|&k| &m.tris[k]).collect();
                if options.hulls == HullMode::Auto && closed(&tris) {
                    let mut leaves = Vec::new();
                    solid_leaves(&tris, &mut work, &mut leaves)?;
                    let n = plan.solids.len();
                    plan.hulls.extend(leaves.into_iter().map(|brush| Hull {
                        brush,
                        kind: HullKind::Leaf(n),
                    }));
                    plan.solids.push(Solid {
                        mesh: mi,
                        tris: tris.into_iter().cloned().collect(),
                    });
                } else {
                    let list: Vec<Tri> = tris.into_iter().cloned().collect();
                    let mut w = Welded::default();
                    let groups = group(&mut w, &list, &|s, k| {
                        list[s].plane().is_some_and(|p| on_plane(&list[k], &p))
                    });
                    // Each polygon's neighbor across each edge (by its corners' numbers, the other way round).
                    let planes: Vec<Option<Plane>> =
                        groups.iter().map(|(_, seed)| list[*seed].plane()).collect();
                    let mut by_edge: HashMap<(u32, u32), usize> = HashMap::new();
                    for (k, (poly, _)) in groups.iter().enumerate() {
                        for i in 0..poly.len() {
                            by_edge.insert((poly[i], poly[(i + 1) % poly.len()]), k);
                        }
                    }
                    for (k, (poly, _)) in groups.iter().enumerate() {
                        let Some(plane) = planes[k] else {
                            continue;
                        };
                        let pts: Vec<V3> = poly.iter().map(|&i| w.points[i as usize]).collect();
                        let sides: Vec<Option<V3>> = (0..poly.len())
                            .map(|i| {
                                let j = *by_edge.get(&(poly[(i + 1) % poly.len()], poly[i]))?;
                                planes[j].map(|q| q.n)
                            })
                            .collect();
                        if let Some(brush) = slab(&pts, plane, &sides, options.slab) {
                            plan.hulls.push(Hull {
                                brush,
                                kind: HullKind::Slab,
                            });
                        }
                    }
                }
                too_many("hulls", plan.hulls.len(), MAX_HULLS)?;
            }
        }
        if plan.hulls.is_empty() {
            return Err(MeshError::NoHulls);
        }
        Ok(plan)
    }
}

/// One `.dif` of a plan: the built interior, and the plan's polygons and hulls in it (in the order the build took them).
#[derive(Debug, Clone, PartialEq)]
pub struct Piece {
    pub built: Built,
    pub polygons: Vec<usize>,
    pub hulls: Vec<usize>,
}

/// A plan built into pieces.
#[derive(Debug, Clone, PartialEq)]
pub struct MeshBuilt {
    pub plan: Plan,
    pub pieces: Vec<Piece>,
}

fn middle(points: impl Iterator<Item = V3>) -> V3 {
    let (lo, hi) = geom::bounds(points);
    [0, 1, 2].map(|k| (lo[k] + hi[k]) * 0.5)
}

/// Builds `input` (module docs): the plan, then its pieces.
pub fn build_mesh(input: &MeshInput, options: &MeshOptions) -> Result<MeshBuilt, MeshError> {
    let plan = Plan::new(input, options)?;
    let polygon_at: Vec<V3> = plan
        .polygons
        .iter()
        .map(|p| middle(p.points.iter().copied()))
        .collect();
    let hull_at: Vec<V3> = plan
        .hulls
        .iter()
        .map(|h| {
            let b = h.brush.bounds().unwrap_or(([0.0; 3], [0.0; 3]));
            middle([b.0, b.1].into_iter())
        })
        .collect();
    let mut tasks: Vec<(Vec<usize>, Vec<usize>)> = vec![(
        (0..plan.polygons.len()).collect(),
        (0..plan.hulls.len()).collect(),
    )];
    let mut pieces = Vec::new();
    let mut failed = 0usize;
    while let Some((polys, hulls)) = tasks.pop() {
        too_many("pieces", pieces.len() + tasks.len() + 1, MAX_PIECES)?;
        let over =
            hulls.len() > options.piece_hulls.max(1) || polys.len() > options.piece_polygons.max(1);
        let attempt = if over {
            None
        } else {
            let ps: Vec<Polygon> = polys.iter().map(|&k| plan.polygons[k].clone()).collect();
            let hs: Vec<Brush> = hulls.iter().map(|&k| plan.hulls[k].brush.clone()).collect();
            match build_polygons(&ps, &hs, &options.build) {
                Ok(built) => {
                    pieces.push(Piece {
                        built,
                        polygons: polys,
                        hulls,
                    });
                    continue;
                }
                Err(e @ BuildError::TooMany { .. }) => {
                    failed += 1;
                    if failed > MAX_FAILED_PIECES {
                        return Err(MeshError::Build(e));
                    }
                    Some(e)
                }
                Err(BuildError::NoBrushes) => continue,
                Err(e) => return Err(MeshError::Build(e)),
            }
        };
        // Cut at the middle of the longest side of the middles' box.
        let all = polys
            .iter()
            .map(|&k| polygon_at[k])
            .chain(hulls.iter().map(|&k| hull_at[k]));
        let (lo, hi) = geom::bounds(all);
        let axis = (0..3)
            .max_by(|&a, &b| (hi[a] - lo[a]).total_cmp(&(hi[b] - lo[b])))
            .unwrap_or(0);
        let mut keys: Vec<f64> = polys
            .iter()
            .map(|&k| polygon_at[k][axis])
            .chain(hulls.iter().map(|&k| hull_at[k][axis]))
            .collect();
        keys.sort_by(f64::total_cmp);
        let cut = keys[keys.len() / 2];
        let side = |at: V3| at[axis] < cut;
        let (pa, pb): (Vec<usize>, Vec<usize>) = polys.iter().partition(|&&k| side(polygon_at[k]));
        let (ha, hb): (Vec<usize>, Vec<usize>) = hulls.iter().partition(|&&k| side(hull_at[k]));
        if (pa.is_empty() && ha.is_empty()) || (pb.is_empty() && hb.is_empty()) {
            return Err(match attempt {
                Some(e) => MeshError::Build(e),
                None => MeshError::TooMany {
                    what: "hulls or polygons in one place",
                    count: hulls.len().max(polys.len()),
                    cap: options.piece_hulls.min(options.piece_polygons),
                },
            });
        }
        tasks.push((pa, ha));
        tasks.push((pb, hb));
    }
    pieces.sort_by_key(|p| (p.hulls.first().copied(), p.polygons.first().copied()));
    Ok(MeshBuilt { plan, pieces })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quad(a: V3, b: V3, c: V3, d: V3, material: usize) -> [Tri; 2] {
        let uv = |p: V3| [p[0] + p[2], p[1] - p[2]];
        [
            Tri {
                p: [a, b, c],
                uv: [uv(a), uv(b), uv(c)],
                material,
            },
            Tri {
                p: [a, c, d],
                uv: [uv(a), uv(c), uv(d)],
                material,
            },
        ]
    }

    /// A box's twelve triangles, clockwise seen from outside.
    pub fn box_tris(lo: V3, hi: V3) -> Vec<Tri> {
        let c = |i: usize| {
            [
                if i & 1 == 0 { lo[0] } else { hi[0] },
                if i & 2 == 0 { lo[1] } else { hi[1] },
                if i & 4 == 0 { lo[2] } else { hi[2] },
            ]
        };
        // Each face's corners clockwise seen from outside.
        let faces = [
            [0, 1, 3, 2],
            [4, 6, 7, 5],
            [0, 4, 5, 1],
            [2, 3, 7, 6],
            [0, 2, 6, 4],
            [1, 5, 7, 3],
        ];
        faces
            .iter()
            .flat_map(|f| quad(c(f[0]), c(f[1]), c(f[2]), c(f[3]), 0))
            .collect()
    }

    #[test]
    fn a_box_is_closed_and_one_solid_leaf() {
        let tris = box_tris([0.0; 3], [2.0, 1.0, 3.0]);
        let refs: Vec<&Tri> = tris.iter().collect();
        assert!(closed(&refs));
        let mut leaves = Vec::new();
        solid_leaves(&refs, &mut 0, &mut leaves).unwrap();
        assert_eq!(leaves.len(), 1);
        assert_eq!(leaves[0].faces.len(), 6);
        assert!(leaves[0].contains([1.0, 0.5, 1.5], 0.4));
        // Each face's two triangles are one polygon.
        let input = MeshInput {
            drawn: vec![NamedMesh {
                name: "box".into(),
                tris,
            }],
            materials: vec!["m".into()],
            ..MeshInput::default()
        };
        let plan = Plan::new(&input, &MeshOptions::default()).unwrap();
        assert_eq!(plan.polygons.len(), 6);
        assert!(plan.polygons.iter().all(|p| p.points.len() == 4));
    }

    #[test]
    fn a_ring_is_cut_into_leaves_that_fill_it_only() {
        // A square ring: four boxes' outer surface would be the test, here as a closed mesh made of quads.
        let mut tris = Vec::new();
        let (o, i, h) = (3.0, 1.0, 1.0);
        let ring = [[-o, -o], [o, -o], [o, o], [-o, o]];
        let hole = [[-i, -i], [i, -i], [i, i], [-i, i]];
        for k in 0..4 {
            let (a, b) = (ring[k], ring[(k + 1) % 4]);
            let (c, d) = (hole[k], hole[(k + 1) % 4]);
            // Top (clockwise seen from above) and bottom.
            tris.extend(quad(
                [a[0], a[1], h],
                [c[0], c[1], h],
                [d[0], d[1], h],
                [b[0], b[1], h],
                0,
            ));
            tris.extend(quad(
                [a[0], a[1], 0.0],
                [b[0], b[1], 0.0],
                [d[0], d[1], 0.0],
                [c[0], c[1], 0.0],
                0,
            ));
            // Outer wall and inner wall.
            tris.extend(quad(
                [a[0], a[1], 0.0],
                [a[0], a[1], h],
                [b[0], b[1], h],
                [b[0], b[1], 0.0],
                0,
            ));
            tris.extend(quad(
                [c[0], c[1], 0.0],
                [d[0], d[1], 0.0],
                [d[0], d[1], h],
                [c[0], c[1], h],
                0,
            ));
        }
        let refs: Vec<&Tri> = tris.iter().collect();
        assert!(closed(&refs));
        let mut leaves = Vec::new();
        solid_leaves(&refs, &mut 0, &mut leaves).unwrap();
        assert!(leaves.len() >= 4);
        for x in [-2.5, -1.5, 0.0, 1.5, 2.5] {
            for y in [-2.5, -1.5, 0.0, 1.5, 2.5] {
                let p: V3 = [x + 0.013, y + 0.017, 0.5];
                let want = !(p[0].abs() < i && p[1].abs() < i);
                let got = leaves.iter().any(|b| b.contains(p, 0.0));
                assert_eq!(got, want, "{p:?}");
            }
        }
    }

    #[test]
    fn an_open_floor_is_slabs() {
        let floor = quad(
            [0.0, 0.0, 0.0],
            [0.0, 4.0, 0.0],
            [4.0, 4.0, 0.0],
            [4.0, 0.0, 0.0],
            0,
        );
        let input = MeshInput {
            drawn: vec![NamedMesh {
                name: "floor".into(),
                tris: floor.to_vec(),
            }],
            materials: vec!["m".into()],
            ..MeshInput::default()
        };
        let built = build_mesh(&input, &MeshOptions::default()).unwrap();
        assert_eq!(built.plan.hulls.len(), 1);
        assert_eq!(built.plan.hulls[0].kind, HullKind::Slab);
        let b = &built.plan.hulls[0].brush;
        assert!(b.contains([2.0, 2.0, -0.05], 0.0) && !b.contains([2.0, 2.0, 0.05], 0.0));
        assert_eq!(built.pieces.len(), 1);
    }

    #[test]
    fn texture_planes_give_each_corner_its_coordinates() {
        let p = [[0.0, 0.0, 0.0], [1.0, 2.0, 0.5], [3.0, -1.0, 2.0]];
        let uv = [[0.1, 0.2], [0.7, -0.3], [1.5, 2.0]];
        let tg = texgen_of(p, uv).unwrap();
        for k in 0..3 {
            let got = tg.at(p[k]);
            assert!((got[0] - uv[k][0]).abs() < 1e-12 && (got[1] - uv[k][1]).abs() < 1e-12);
        }
        assert!(texgen_of([[0.0; 3], [1.0; 3], [2.0; 3]], uv).is_none());
    }

    #[test]
    fn random_triangle_soups_build_or_fail_without_panics() {
        let mut seed = 0x51ed_270b_2c4a_3f19u64;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for case in 0..120 {
            // Corners on a coarse grid: coplanar, repeated, flat and tiny triangles come often.
            let mut corner = || [0, 1, 2].map(|_| (next() % 5) as f64 * 0.5 - 1.0);
            let mut tri = |m: usize| Tri {
                p: [corner(), corner(), corner()],
                uv: [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]],
                material: m,
            };
            let mut input = MeshInput {
                materials: vec!["m".into(), "NULL".into()],
                ..MeshInput::default()
            };
            let n = 1 + case % 24;
            input.drawn.push(NamedMesh {
                name: "soup".into(),
                tris: (0..n).map(|k| tri(k % 2)).collect(),
            });
            let mut closed_box = box_tris([0.0; 3], [1.0, 1.0, 1.0]);
            closed_box.truncate(12 - case % 3);
            input.drawn.push(NamedMesh {
                name: "box".into(),
                tris: closed_box,
            });
            if case % 4 == 0 {
                input.authored.push(NamedMesh {
                    name: "col_soup".into(),
                    tris: (0..n).map(|_| tri(0)).collect(),
                });
            }
            let _ = build_mesh(&input, &MeshOptions::default());
        }
    }
}
