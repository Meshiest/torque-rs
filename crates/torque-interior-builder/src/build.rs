//! Brushes into one interior (detail level 0, one zone) of a version-0 `.dif` in TGE's form.
//!
//! 1. Each brush's face polygons ([`Brush::polygons`]), then the visible parts ([`crate::csg`]).
//! 2. A drawn part whose lightmap rectangle would pass [`lightmap::MAX_RECT`] texels is cut on the lightmap axes; then every
//!    part gets the corners of its neighbors that lie on its edges (no T-junctions: the windings meet point for point) and one
//!    of more than 32 points is cut into fans of 32 (a surface's fan mask is a 32-bit word).
//! 3. The tables: planes (each once, the opposite of a kept plane by the flip bit), points (each once), texture planes and
//!    materials. A part of an editor material ([`is_null_material`]) is a null surface: collided with, never drawn.
//! 4. Each drawn part is a surface: its windings the triangle strip of its polygon (`Interior`'s draw order), its fan mask its
//!    corners, its lightmap rectangle packed into [`lightmap::PAGE`]-square black pages, outside-visible in zone 0.
//! 5. Each brush is a convex hull: its corners, its planes, an emit string a corner, its surfaces and their polygon list
//!    ([`crate::hull`]); the hulls are binned on the 16 x 16 grid over the interior's x and y.
//! 6. The solid BSP ([`crate::bsp`]): each solid leaf lists its brush's surfaces; every empty leaf is zone 0.
//!
//! Credit: the parts and their meaning follow Torque3D 2.0's MIT-licensed `Engine/source/interior/` (`interior.h`,
//! `interior.cpp`, `interiorCollision.cpp`, `interiorIO.cpp`), each `Copyright (c) 2012 GarageGames, LLC`; the code here is
//! this crate's own.

use std::collections::HashMap;

use torque_maps::dif::{
    self, Aabb, BspChild, BspNode, ConvexHull, Dif, Hulls, Interior, Lightmap, NullSurface,
    PLANE_FLIPPED, SolidLeaf, Surface, Zone, surface_flags,
};
use torque_maps::png::{Png, encode_rgb};

use crate::brush::{Brush, BrushError, TexGen};
use crate::bsp::{self, Child};
use crate::csg;
use crate::geom::{self, EPS, Plane, V3};
use crate::hull::{self, HullError, HullFace, HullSurface};
use crate::lightmap::{self, MAX_RECT, PAGE};

/// The editor's materials: their faces are null surfaces (TGE's `NULL`, `ORIGIN`, `TRIGGER` and `FORCEFIELD`, any case).
pub fn is_null_material(name: &str) -> bool {
    torque_maps::legacy::SPECIAL
        .iter()
        .any(|s| s.eq_ignore_ascii_case(name))
}

/// How an interior is built.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Options {
    /// One lightmap texel every this many units (a power of two).
    pub lightmap_texel: f64,
    /// The interior's `minPixels`.
    pub min_pixels: u32,
}

impl Default for Options {
    fn default() -> Options {
        Options {
            lightmap_texel: 0.5,
            min_pixels: 250,
        }
    }
}

/// Why an interior cannot be built.
#[derive(Debug, Clone, PartialEq)]
pub enum BuildError {
    NoBrushes,
    /// Brush (or hull) `index` bounds no solid, or polygon `index` has a value that is not finite.
    Brush {
        index: usize,
        error: BrushError,
    },
    /// More of something than a version-0 interior can number.
    TooMany {
        what: &'static str,
        count: usize,
        cap: usize,
    },
    /// A hull that its byte strings cannot hold.
    Hull {
        brush: usize,
        error: HullError,
    },
    /// The lightmap texel is not a power of two between 1/1024 and 1024.
    LightmapTexel(f64),
    /// The writer refused a value.
    Write(String),
}

impl std::fmt::Display for BuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BuildError::NoBrushes => write!(f, "no brushes"),
            BuildError::Brush { index, error } => write!(f, "brush {index}: {error:?}"),
            BuildError::TooMany { what, count, cap } => {
                write!(
                    f,
                    "{count} {what}, over the {cap} a version-0 interior holds"
                )
            }
            BuildError::Hull { brush, error } => write!(f, "brush {brush}'s hull: {error:?}"),
            BuildError::LightmapTexel(t) => {
                write!(
                    f,
                    "lightmap texel {t} is not a power of two from 1/1024 to 1024"
                )
            }
            BuildError::Write(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for BuildError {}

/// Where a surface came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// Face `face` of brush `brush` ([`build`]).
    Face { brush: usize, face: usize },
    /// Polygon `n` of the polygons drawn ([`build_polygons`]).
    Polygon(usize),
    /// Face `face` of hull `hull` ([`build_polygons`]: a null surface the hull's polygon list reads).
    HullFace { hull: usize, face: usize },
}

/// A convex polygon to draw, clockwise seen from the front of `plane` (the mesh input of [`build_polygons`]).
#[derive(Debug, Clone, PartialEq)]
pub struct Polygon {
    pub points: Vec<V3>,
    pub plane: Plane,
    pub material: String,
    pub texgen: TexGen,
}

/// What was built.
#[derive(Debug, Clone, PartialEq)]
pub struct Built {
    pub dif: Dif,
    /// Each drawn surface's source and its polygon (clockwise seen from the front), in surface order.
    pub surfaces: Vec<(Source, Vec<V3>)>,
    /// Each null surface's source, in order.
    pub null_surfaces: Vec<(Source, Vec<V3>)>,
}

impl Built {
    /// The `.dif`'s bytes.
    pub fn bytes(&self) -> Result<Vec<u8>, BuildError> {
        dif::write(&self.dif).map_err(|e| BuildError::Write(e.to_string()))
    }
}

/// The most nodes and solid leaves of a version-0 BSP (14-bit child numbers).
pub const MAX_BSP: usize = 0x3fff;
/// The most planes (15-bit plane numbers).
pub const MAX_PLANES: usize = 0x7fff;
/// The most surfaces (zone surface lists are 16-bit) and hulls (bin entries are 16-bit).
pub const MAX_SURFACES: usize = 0xffff;
pub const MAX_HULLS: usize = 0xffff;
/// The most points looked at for T-junctions in all.
pub const MAX_TJUNCTION_WORK: u64 = 400_000_000;
/// The most brushes built into one interior (hidden-face removal compares brushes pairwise).
pub const MAX_BRUSHES: usize = 8_192;
/// The most points a winding takes (a surface's fan mask is 32 bits).
pub const MAX_WINDING: usize = 32;

fn cap(what: &'static str, count: usize, cap: usize) -> Result<(), BuildError> {
    if count > cap {
        Err(BuildError::TooMany { what, count, cap })
    } else {
        Ok(())
    }
}

/// One point a vertex within [`EPS`] of another: the first's.
#[derive(Default)]
struct Weld {
    points: Vec<V3>,
    /// The points by their cell of side [`WELD_CELL`].
    cells: HashMap<[i64; 3], Vec<usize>>,
    /// The points by x, for the range searches.
    by_x: Vec<(f64, usize)>,
}

/// The side of a weld cell: points within [`EPS`] are in the same or a neighboring cell.
const WELD_CELL: f64 = EPS * 4.0;

impl Weld {
    fn cell(p: V3) -> [i64; 3] {
        p.map(|v| (v / WELD_CELL).floor() as i64)
    }

    fn add(&mut self, p: V3) -> V3 {
        let c = Weld::cell(p);
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    let k = [c[0] + dx, c[1] + dy, c[2] + dz];
                    for &i in self.cells.get(&k).map_or(&[][..], Vec::as_slice) {
                        if geom::length(geom::sub(self.points[i], p)) < EPS {
                            return self.points[i];
                        }
                    }
                }
            }
        }
        self.cells.entry(c).or_default().push(self.points.len());
        self.by_x.push((p[0], self.points.len()));
        self.points.push(p);
        p
    }

    fn sort(&mut self) {
        self.by_x.sort_by(|a, b| a.0.total_cmp(&b.0));
    }

    /// The points (other than `a` and `b`) on the segment `a`-`b`, nearest `a` first.
    /// Each point looked at is a unit of `work`.
    fn on_segment(&self, a: V3, b: V3, work: &mut u64) -> Vec<V3> {
        let lo = a[0].min(b[0]) - EPS;
        let hi = a[0].max(b[0]) + EPS;
        let start = self.by_x.partition_point(|e| e.0 < lo);
        let d = geom::sub(b, a);
        let len2 = geom::dot(d, d);
        let mut out: Vec<(f64, V3)> = Vec::new();
        for &(x, i) in &self.by_x[start..] {
            if x > hi {
                break;
            }
            *work += 1;
            let p = self.points[i];
            let t = geom::dot(geom::sub(p, a), d) / len2;
            if t <= 0.0 || t >= 1.0 {
                continue;
            }
            let foot = geom::add(a, geom::scale(d, t));
            let off = geom::length(geom::sub(p, foot));
            let ends = geom::length(geom::sub(p, a)).min(geom::length(geom::sub(p, b)));
            if off < EPS && ends > EPS {
                out.push((t, p));
            }
        }
        out.sort_by(|x, y| x.0.total_cmp(&y.0));
        out.into_iter().map(|(_, p)| p).collect()
    }
}

/// A part to become a surface: where it came from, the hull it lies on, its face and its polygon.
struct Part {
    source: Source,
    hull: Option<usize>,
    face: usize,
    poly: Vec<V3>,
    null: bool,
}

/// A face as the surfaces take it: its plane, material and texture planes.
struct FaceSpec<'a> {
    plane: Plane,
    material: &'a str,
    texgen: TexGen,
}

/// Cuts `poly` until its lightmap rectangle fits [`MAX_RECT`].
fn fit_lightmap(
    poly: Vec<V3>,
    n: V3,
    texel: f64,
    out: &mut Vec<Vec<V3>>,
    left: &mut usize,
) -> Result<(), BuildError> {
    let axes = lightmap::axes(n);
    let r = lightmap::rect(&poly, axes, texel);
    // The pieces this part needs at least, counted before any is cut: a part too large for what is left fails at once.
    let need = [0, 1].map(|k| u64::from(r.size[k]).div_ceil(u64::from(MAX_RECT)));
    if need[0].saturating_mul(need[1]) > *left as u64 {
        return Err(BuildError::TooMany {
            what: "surfaces",
            count: MAX_SURFACES + 1,
            cap: MAX_SURFACES,
        });
    }
    let k = if r.size[0] > MAX_RECT {
        Some(axes.0)
    } else if r.size[1] > MAX_RECT {
        Some(axes.1)
    } else {
        None
    };
    let Some(k) = k else {
        *left -= 1;
        out.push(poly);
        return Ok(());
    };
    let lo = poly.iter().map(|p| p[k]).fold(f64::INFINITY, f64::min);
    let hi = poly.iter().map(|p| p[k]).fold(f64::NEG_INFINITY, f64::max);
    let mid = ((lo + hi) * 0.5 / texel).round() * texel;
    let mut normal = [0.0; 3];
    normal[k] = 1.0;
    let Some(cut) = Plane::new(normal, [mid, mid, mid]) else {
        *left -= 1;
        out.push(poly);
        return Ok(());
    };
    let (front, back) = geom::split(&poly, &cut);
    if front.is_empty() || back.is_empty() {
        *left -= 1;
        out.push(poly);
        return Ok(());
    }
    fit_lightmap(front, n, texel, out, left)?;
    fit_lightmap(back, n, texel, out, left)
}

/// Whether `b` is a corner between `a` and `c` (not on the line through them).
fn is_corner(a: V3, b: V3, c: V3) -> bool {
    let d = geom::sub(c, a);
    let l = geom::length(d);
    if l < EPS {
        return true;
    }
    geom::length(geom::cross(geom::sub(b, a), d)) / l > EPS
}

/// The strip order of a convex polygon (`collisionFanFromSurface` turns it back): `q0 q1 q(n-1) q2 q(n-2) ...`.
pub fn strip_order(n: usize) -> Vec<usize> {
    let mut out = Vec::with_capacity(n);
    if n == 0 {
        return out;
    }
    out.push(0);
    if n > 1 {
        out.push(1);
    }
    let (mut lo, mut hi) = (2usize, n - 1);
    while out.len() < n {
        out.push(hi);
        hi -= 1;
        if out.len() < n {
            out.push(lo);
            lo += 1;
        }
    }
    out
}

struct Tables {
    planes: Vec<dif::Plane>,
    plane_keys: HashMap<([i64; 3], i64), u16>,
    points: Vec<[f32; 3]>,
    point_keys: HashMap<[u32; 3], u32>,
    texgens: Vec<dif::TexGen>,
    texgen_keys: HashMap<[u32; 8], u32>,
    materials: Vec<String>,
}

impl Tables {
    fn new() -> Tables {
        Tables {
            planes: Vec::new(),
            plane_keys: HashMap::new(),
            points: Vec::new(),
            point_keys: HashMap::new(),
            texgens: Vec::new(),
            texgen_keys: HashMap::new(),
            materials: Vec::new(),
        }
    }

    /// The plane's index, [`PLANE_FLIPPED`] set when the table holds its opposite.
    fn plane(&mut self, p: &Plane) -> Result<u16, BuildError> {
        let key = |p: &Plane| {
            (
                p.n.map(|v| (v * 1e6).round() as i64),
                (p.d * 1e4).round() as i64,
            )
        };
        if let Some(&k) = self.plane_keys.get(&key(p)) {
            return Ok(k);
        }
        if let Some(&k) = self.plane_keys.get(&key(&p.flipped())) {
            return Ok(k | PLANE_FLIPPED);
        }
        cap("planes", self.planes.len() + 1, MAX_PLANES)?;
        let k = self.planes.len() as u16;
        self.planes.push(dif::Plane {
            normal: p.n.map(|v| v as f32),
            d: p.d as f32,
        });
        self.plane_keys.insert(key(p), k);
        Ok(k)
    }

    fn point(&mut self, p: V3) -> u32 {
        let f = p.map(|v| v as f32);
        let key = f.map(f32::to_bits);
        *self.point_keys.entry(key).or_insert_with(|| {
            self.points.push(f);
            self.points.len() as u32 - 1
        })
    }

    fn texgen(&mut self, t: &TexGen) -> u32 {
        let g = dif::TexGen {
            x: t.u.map(|v| v as f32),
            y: t.v.map(|v| v as f32),
        };
        let mut key = [0u32; 8];
        for k in 0..4 {
            key[k] = g.x[k].to_bits();
            key[k + 4] = g.y[k].to_bits();
        }
        *self.texgen_keys.entry(key).or_insert_with(|| {
            self.texgens.push(g);
            self.texgens.len() as u32 - 1
        })
    }

    fn material(&mut self, m: &str) -> Result<u16, BuildError> {
        if let Some(i) = self.materials.iter().position(|x| x == m) {
            return Ok(i as u16);
        }
        cap("materials", self.materials.len() + 1, 0xffff)?;
        self.materials.push(m.to_string());
        Ok(self.materials.len() as u16 - 1)
    }

    fn plane_normal(&self, index: u16) -> [f32; 3] {
        let p = self.planes[(index & !PLANE_FLIPPED) as usize];
        if index & PLANE_FLIPPED != 0 {
            p.normal.map(|v| -v)
        } else {
            p.normal
        }
    }
}

fn aabb(points: impl IntoIterator<Item = [f32; 3]>) -> Aabb {
    let mut b = Aabb {
        min: [f32::INFINITY; 3],
        max: [f32::NEG_INFINITY; 3],
    };
    for p in points {
        b.min = [0, 1, 2].map(|k| b.min[k].min(p[k]));
        b.max = [0, 1, 2].map(|k| b.max[k].max(p[k]));
    }
    b
}

fn check_texel(texel: f64) -> Result<(), BuildError> {
    let log = texel.log2();
    if texel.is_finite() && log.fract() == 0.0 && (-10.0..=10.0).contains(&log) {
        Ok(())
    } else {
        Err(BuildError::LightmapTexel(texel))
    }
}

/// Builds `brushes` (each a solid in the interior's space) into a `.dif` (module docs).
pub fn build(brushes: &[Brush], options: &Options) -> Result<Built, BuildError> {
    check_texel(options.lightmap_texel)?;
    if brushes.is_empty() {
        return Err(BuildError::NoBrushes);
    }
    cap("brushes", brushes.len(), MAX_BRUSHES)?;
    let mut polys = Vec::with_capacity(brushes.len());
    let mut boxes = Vec::with_capacity(brushes.len());
    for (index, b) in brushes.iter().enumerate() {
        let p = b
            .polygons()
            .map_err(|error| BuildError::Brush { index, error })?;
        boxes.push(geom::bounds(p.iter().flatten().copied()));
        polys.push(p);
    }
    let fragments = csg::visible(brushes, &polys, &boxes, MAX_SURFACES).map_err(|e| match e {
        csg::TooMuch::Parts(count) => BuildError::TooMany {
            what: "surfaces",
            count,
            cap: MAX_SURFACES,
        },
        csg::TooMuch::Work(w) => BuildError::TooMany {
            what: "hidden-face tests",
            count: w as usize,
            cap: csg::MAX_WORK as usize,
        },
    })?;
    // One face spec a brush face, numbered brush by brush.
    let mut specs = Vec::new();
    let mut first_face = Vec::with_capacity(brushes.len());
    for b in brushes {
        first_face.push(specs.len());
        specs.extend(b.faces.iter().map(|f| FaceSpec {
            plane: f.plane,
            material: &f.material,
            texgen: f.texgen,
        }));
    }
    let parts = fragments
        .into_iter()
        .map(|f| Part {
            source: Source::Face {
                brush: f.brush,
                face: f.face,
            },
            hull: Some(f.brush),
            face: first_face[f.brush] + f.face,
            null: is_null_material(&brushes[f.brush].faces[f.face].material),
            poly: f.poly,
        })
        .collect();
    assemble(brushes, polys, parts, &specs, options)
}

/// Builds drawn `polygons` and collision `hulls` (each a convex solid) into a `.dif`: the mesh input. The polygons are drawn as
/// they are (no hidden faces removed); a polygon of an editor material ([`is_null_material`]) is left out. Each hull face is a
/// null surface, so a hull's polygon list is its faces.
pub fn build_polygons(
    polygons: &[Polygon],
    hulls: &[Brush],
    options: &Options,
) -> Result<Built, BuildError> {
    check_texel(options.lightmap_texel)?;
    if hulls.is_empty() && polygons.is_empty() {
        return Err(BuildError::NoBrushes);
    }
    cap("hulls", hulls.len(), MAX_BRUSHES)?;
    cap("polygons", polygons.len(), MAX_SURFACES)?;
    let mut polys = Vec::with_capacity(hulls.len());
    for (index, b) in hulls.iter().enumerate() {
        polys.push(
            b.polygons()
                .map_err(|error| BuildError::Brush { index, error })?,
        );
    }
    let mut specs = Vec::new();
    let mut parts = Vec::new();
    for (k, p) in polygons.iter().enumerate() {
        if is_null_material(&p.material) || p.points.len() < 3 {
            continue;
        }
        if p.points.iter().flatten().any(|v| !v.is_finite()) {
            return Err(BuildError::Brush {
                index: k,
                error: BrushError::NotFinite,
            });
        }
        parts.push(Part {
            source: Source::Polygon(k),
            hull: None,
            face: specs.len(),
            poly: p.points.clone(),
            null: false,
        });
        specs.push(FaceSpec {
            plane: p.plane,
            material: &p.material,
            texgen: p.texgen,
        });
    }
    for (h, (b, faces)) in hulls.iter().zip(&polys).enumerate() {
        for (fi, poly) in faces.iter().enumerate() {
            if poly.is_empty() {
                continue;
            }
            parts.push(Part {
                source: Source::HullFace { hull: h, face: fi },
                hull: Some(h),
                face: specs.len(),
                poly: poly.clone(),
                null: true,
            });
            specs.push(FaceSpec {
                plane: b.faces[fi].plane,
                material: "NULL",
                texgen: b.faces[fi].texgen,
            });
        }
    }
    assemble(hulls, polys, parts, &specs, options)
}

/// Steps 2 to 6 (module docs) over the hulls (`polys[h]`: hull `h`'s face polygons) and the parts to become surfaces.
fn assemble(
    brushes: &[Brush],
    polys: Vec<Vec<Vec<V3>>>,
    fragments: Vec<Part>,
    specs: &[FaceSpec],
    options: &Options,
) -> Result<Built, BuildError> {
    let texel = options.lightmap_texel;
    // 2: the parts cut to fit the lightmaps, welded, with their neighbors' corners on their edges.
    let mut parts: Vec<Part> = Vec::new();
    // Every drawn piece is a surface: the lightmap cuts stop once the surfaces would pass the cap.
    let mut left = MAX_SURFACES;
    for f in fragments {
        let mut cut = Vec::new();
        if f.null {
            cut.push(f.poly);
        } else {
            fit_lightmap(f.poly, specs[f.face].plane.n, texel, &mut cut, &mut left)?;
        }
        parts.extend(cut.into_iter().map(|poly| Part { poly, ..f }));
    }
    let mut weld = Weld::default();
    for p in &mut parts {
        for v in &mut p.poly {
            *v = weld.add(*v);
        }
    }
    weld.sort();
    let mut work = 0u64;
    let mut pieces: Vec<Part> = Vec::new();
    for p in parts {
        let n = p.poly.len();
        let mut poly = Vec::with_capacity(n);
        for i in 0..n {
            let (a, b) = (p.poly[i], p.poly[(i + 1) % n]);
            poly.push(a);
            poly.extend(weld.on_segment(a, b, &mut work));
            if work > MAX_TJUNCTION_WORK {
                return Err(BuildError::TooMany {
                    what: "T-junction tests",
                    count: work as usize,
                    cap: MAX_TJUNCTION_WORK as usize,
                });
            }
        }
        geom::dedup(&mut poly);
        if poly.len() < 3 {
            continue;
        }
        // Fans of at most 32 points: the first 32, then the first point, the 32nd and the rest.
        let mut rest = poly;
        while rest.len() > MAX_WINDING {
            let head: Vec<V3> = rest[..MAX_WINDING].to_vec();
            let mut tail = vec![rest[0]];
            tail.extend_from_slice(&rest[MAX_WINDING - 1..]);
            pieces.push(Part { poly: head, ..p });
            rest = tail;
        }
        pieces.push(Part { poly: rest, ..p });
    }

    // 3-4: the tables and the surfaces.
    let mut t = Tables::new();
    let mut windings: Vec<u32> = Vec::new();
    let mut surfaces: Vec<Surface> = Vec::new();
    let mut surface_src = Vec::new();
    let mut rects = Vec::new();
    let mut null_surfaces: Vec<NullSurface> = Vec::new();
    let mut null_src = Vec::new();
    // Per brush: its surface references (with the null bit for null surfaces) and their polygon-list form.
    let mut brush_surfaces: Vec<Vec<(u32, HullSurface)>> = vec![Vec::new(); brushes.len()];
    for p in &pieces {
        let face = &specs[p.face];
        let plane = t.plane(&face.plane)?;
        let ids: Vec<u32> = p.poly.iter().map(|&v| t.point(v)).collect();
        let n = ids.len();
        if p.null {
            let at = windings.len() as u32;
            windings.extend_from_slice(&ids);
            null_surfaces.push(NullSurface {
                winding_start: at,
                plane,
                flags: 0,
                winding_count: n as u32,
            });
            null_src.push((p.source, p.poly.clone()));
            let r = (null_surfaces.len() as u32 - 1) | dif::NULL_SURFACE;
            if let Some(h) = p.hull {
                brush_surfaces[h].push((r, HullSurface { plane, points: ids }));
            }
            continue;
        }
        let mut fan_mask = 0u32;
        let mut fan = Vec::new();
        for (i, &id) in ids.iter().enumerate() {
            if is_corner(p.poly[(i + n - 1) % n], p.poly[i], p.poly[(i + 1) % n]) {
                fan_mask |= 1 << i;
                fan.push(id);
            }
        }
        let at = windings.len() as u32;
        windings.extend(strip_order(n).into_iter().map(|k| ids[k]));
        let axes = lightmap::axes(face.plane.n);
        rects.push((lightmap::rect(&p.poly, axes, texel), axes));
        surfaces.push(Surface {
            winding_start: at,
            winding_count: n as u32,
            plane,
            texture: t.material(face.material)?,
            texgen: t.texgen(&face.texgen),
            flags: surface_flags::OUTSIDE_VISIBLE,
            fan_mask,
            lightmap_texgen: dif::TexGen {
                x: [0.0; 4],
                y: [0.0; 4],
            },
            light_count: 0,
            light_state_start: 0,
            map_offset: [0, 0],
            map_size: [0, 0],
        });
        surface_src.push((p.source, p.poly.clone()));
        if let Some(h) = p.hull {
            brush_surfaces[h].push((
                surfaces.len() as u32 - 1,
                HullSurface { plane, points: fan },
            ));
        }
    }
    cap("surfaces", surfaces.len(), MAX_SURFACES)?;

    // The lightmaps: the rectangles packed, each surface's planes moved onto its place.
    let packing = lightmap::pack(&rects.iter().map(|(r, _)| r.size).collect::<Vec<_>>());
    let scale = 1.0 / (texel * f64::from(PAGE));
    let mut normal_lightmaps = Vec::with_capacity(surfaces.len());
    for (s, ((r, (a, b)), &(page, off))) in
        surfaces.iter_mut().zip(rects.iter().zip(&packing.places))
    {
        let mut x = [0.0f32; 4];
        let mut y = [0.0f32; 4];
        x[*a] = scale as f32;
        y[*b] = scale as f32;
        x[3] = ((i64::from(off[0]) - r.origin[0]) as f64 / f64::from(PAGE)) as f32;
        y[3] = ((i64::from(off[1]) - r.origin[1]) as f64 / f64::from(PAGE)) as f32;
        s.lightmap_texgen = dif::TexGen { x, y };
        s.map_offset = off;
        s.map_size = r.size;
        normal_lightmaps.push(page);
    }
    cap("lightmap pages", packing.pages as usize, 255)?;
    let black = encode_rgb(PAGE, PAGE, &[]);
    let lightmaps: Vec<Lightmap> = (0..packing.pages)
        .map(|_| Lightmap {
            png: Png {
                width: PAGE,
                height: PAGE,
                bytes: black.clone(),
            },
            direction: None,
            keep: true,
        })
        .collect();

    // 5: the hulls.
    let mut hulls = Hulls::default();
    let mut hull_boxes = Vec::with_capacity(brushes.len());
    for (bi, b) in brushes.iter().enumerate() {
        let hull_err = |error| BuildError::Hull { brush: bi, error };
        let mut corners: Vec<V3> = Vec::new();
        let mut faces: Vec<HullFace> = Vec::new();
        let plane_start = hulls.hull_plane_indices.len() as u32;
        for (fi, poly) in polys[bi].iter().enumerate() {
            if poly.is_empty() {
                continue;
            }
            let plane = t.plane(&b.faces[fi].plane)?;
            let k = hulls.hull_plane_indices.len() as u32 - plane_start;
            hulls.hull_plane_indices.push(plane);
            let mut pts = Vec::with_capacity(poly.len());
            for &v in poly {
                let v = weld.add(v);
                let at = match corners
                    .iter()
                    .position(|&c| geom::length(geom::sub(c, v)) < EPS)
                {
                    Some(i) => i,
                    None => {
                        corners.push(v);
                        corners.len() - 1
                    }
                };
                pts.push(
                    u8::try_from(at)
                        .map_err(|_| hull_err(HullError::TooMany("hull points", at + 1)))?,
                );
            }
            faces.push(HullFace {
                plane: u8::try_from(k)
                    .map_err(|_| hull_err(HullError::TooMany("hull planes", k as usize + 1)))?,
                points: pts,
            });
        }
        let ids: Vec<u32> = corners.iter().map(|&c| t.point(c)).collect();
        let hull_start = hulls.hull_indices.len() as u32;
        hulls.hull_indices.extend_from_slice(&ids);
        for v in 0..corners.len() {
            let s = hull::emit_string(v as u8, &faces).map_err(hull_err)?;
            let at = find_or_append(&mut hulls.emit_strings, &s);
            hulls.hull_emit_string_indices.push(at);
        }
        let surface_start = hulls.hull_surface_indices.len() as u32;
        let own = &brush_surfaces[bi];
        hulls
            .hull_surface_indices
            .extend(own.iter().map(|(r, _)| *r));
        let list = hull::poly_list(
            &own.iter().map(|(_, s)| s.clone()).collect::<Vec<_>>(),
            |p| t.plane_normal(p),
        )
        .map_err(hull_err)?;
        let bounds = aabb(ids.iter().map(|&i| t.points[i as usize]));
        hull_boxes.push(bounds);
        hulls.hulls.push(ConvexHull {
            hull_start,
            hull_count: corners.len() as u16,
            bounds,
            surface_start,
            surface_count: own.len() as u16,
            plane_start,
            poly_list_plane_start: hulls.poly_list_planes.len() as u32,
            poly_list_point_start: hulls.poly_list_points.len() as u32,
            poly_list_string_start: hulls.poly_list_strings.len() as u32,
            static_mesh: false,
        });
        hulls.poly_list_planes.extend_from_slice(&list.planes);
        hulls.poly_list_points.extend_from_slice(&list.points);
        hulls.poly_list_strings.extend_from_slice(&list.string);
    }

    let bounds = aabb(t.points.iter().copied());
    let (coord_bins, coord_bin_indices) = bins(&bounds, &hull_boxes);

    // 6: the BSP.
    let tree = bsp::build(brushes, MAX_BSP).map_err(|e| match e {
        bsp::TooMuch::Nodes(n) => BuildError::TooMany {
            what: "BSP nodes",
            count: n + 1,
            cap: MAX_BSP,
        },
        bsp::TooMuch::Work(w) => BuildError::TooMany {
            what: "BSP splitter tests",
            count: w as usize,
            cap: bsp::MAX_WORK as usize,
        },
    })?;
    cap("BSP solid leaves", tree.solid.len(), MAX_BSP)?;
    let child = |c: Child| match c {
        Child::Node(i) => BspChild::Node(i),
        Child::Solid(i) => BspChild::Solid(i),
        Child::Empty => BspChild::Empty(0),
    };
    let mut bsp_nodes = Vec::with_capacity(tree.nodes.len());
    for n in &tree.nodes {
        bsp_nodes.push(BspNode {
            plane: t.plane(&n.plane)?,
            front: child(n.front),
            back: child(n.back),
        });
    }
    // A tree of one leaf (no hull at all) as one node with that leaf on both sides: a walk always starts at node 0.
    if bsp_nodes.is_empty() {
        let leaf = tree.root.map_or(BspChild::Empty(0), child);
        let up = Plane::new([0.0, 0.0, 1.0], [0.0; 3]).ok_or(BuildError::NoBrushes)?;
        bsp_nodes.push(BspNode {
            plane: t.plane(&up)?,
            front: leaf,
            back: leaf,
        });
    }
    let mut solid_leaf_surfaces = Vec::new();
    let mut bsp_solid_leaves = Vec::with_capacity(tree.solid.len());
    for &b in &tree.solid {
        let own = &brush_surfaces[b];
        bsp_solid_leaves.push(SolidLeaf {
            surface_start: solid_leaf_surfaces.len() as u32,
            surface_count: own.len().min(0xffff) as u16,
        });
        solid_leaf_surfaces.extend(own.iter().take(0xffff).map(|(r, _)| *r));
    }

    let n_surfaces = surfaces.len();
    let center = [0, 1, 2].map(|k| (bounds.min[k] + bounds.max[k]) * 0.5);
    let radius = (0..3)
        .map(|k| (bounds.max[k] - center[k]).powi(2))
        .sum::<f32>()
        .sqrt();
    let interior = Interior {
        version: 0,
        plain_surfaces: true,
        detail_level: 0,
        min_pixels: options.min_pixels,
        bounds,
        sphere: [center[0], center[1], center[2], radius],
        has_alarm_state: false,
        light_state_entries: 0,
        point_visibility: vec![1; t.points.len()],
        planes: t.planes,
        points: t.points,
        texgens: t.texgens,
        bsp_nodes,
        bsp_solid_leaves,
        materials: t.materials,
        windings,
        winding_indices: Vec::new(),
        edges: Vec::new(),
        zones: vec![Zone {
            portal_start: 0,
            portal_count: 0,
            surface_start: 0,
            surface_count: n_surfaces as u16,
            static_mesh_start: 0,
            static_mesh_count: 0,
            flags: 0,
        }],
        zone_surfaces: (0..n_surfaces as u32).map(|s| s as u16).collect(),
        zone_static_meshes: Vec::new(),
        zone_portals: Vec::new(),
        portals: Vec::new(),
        surfaces,
        alarm_lightmaps: normal_lightmaps.clone(),
        normal_lightmaps,
        null_surfaces,
        lightmaps,
        solid_leaf_surfaces,
        animated_lights: Vec::new(),
        light_states: Vec::new(),
        state_data: Vec::new(),
        state_data_flags: 0,
        state_data_buffer: Vec::new(),
        name_buffer: Vec::new(),
        mirrors: Vec::new(),
        hulls,
        coord_bins,
        coord_bin_indices,
        coord_bin_mode: 0,
        base_ambient: [0, 0, 0, 255],
        alarm_ambient: [0, 0, 0, 255],
        static_meshes: Vec::new(),
        normals: Vec::new(),
        tex_matrices: Vec::new(),
        tex_mat_indices: Vec::new(),
        lightmap_border: None,
        vehicle: None,
    };
    Ok(Built {
        dif: Dif {
            preview: None,
            details: vec![interior],
            sub_objects: Vec::new(),
            triggers: Vec::new(),
            path_followers: Vec::new(),
            force_fields: Vec::new(),
            ai_nodes: Vec::new(),
            game_entities: Vec::new(),
            border_pad: false,
            trailing: 0,
        },
        surfaces: surface_src,
        null_surfaces: null_src,
    })
}

/// The offset of `s` in `buf`, appended when it is not there yet.
fn find_or_append(buf: &mut Vec<u8>, s: &[u8]) -> u32 {
    if let Some(at) = buf.windows(s.len()).position(|w| w == s) {
        return at as u32;
    }
    buf.extend_from_slice(s);
    (buf.len() - s.len()) as u32
}

/// The 16 x 16 coordinate bins over `bounds`' x and y (`mCoordBins[x * 16 + y]`), each the hulls whose boxes reach it.
fn bins(bounds: &Aabb, hulls: &[Aabb]) -> (Vec<[u32; 2]>, Vec<u16>) {
    const N: usize = 16;
    let size = [0, 1].map(|k| (bounds.max[k] - bounds.min[k]) / N as f32);
    let mut out = Vec::with_capacity(N * N);
    let mut indices = Vec::new();
    for i in 0..N {
        for j in 0..N {
            let lo = [
                bounds.min[0] + size[0] * i as f32,
                bounds.min[1] + size[1] * j as f32,
            ];
            let hi = [lo[0] + size[0], lo[1] + size[1]];
            let start = indices.len() as u32;
            for (h, b) in hulls.iter().enumerate() {
                if b.min[0] <= hi[0] && b.max[0] >= lo[0] && b.min[1] <= hi[1] && b.max[1] >= lo[1]
                {
                    indices.push(h as u16);
                }
            }
            out.push([start, indices.len() as u32 - start]);
        }
    }
    (out, indices)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid(n: V3) -> TexGen {
        TexGen::world(n, 1.0)
    }

    #[test]
    fn an_enormous_brush_fails_before_its_lightmap_cuts() {
        // 200,000 units a side at half a unit a texel: about 2.5 million rectangles a face, refused before one is cut.
        let b = Brush::cuboid([-1.0e5; 3], [1.0e5; 3], "m", grid);
        let started = web_time::Instant::now();
        let e = build(&[b], &Options::default()).unwrap_err();
        assert!(
            matches!(
                e,
                BuildError::TooMany {
                    what: "surfaces",
                    ..
                }
            ),
            "{e}"
        );
        assert!(started.elapsed().as_secs() < 5);
        // One just inside the cap builds.
        let b = Brush::cuboid([0.0; 3], [100.0, 100.0, 1.0], "m", grid);
        assert!(build(&[b], &Options::default()).is_ok());
    }

    #[test]
    fn too_many_brushes_fail_before_any_work() {
        let brushes: Vec<Brush> = (0..=MAX_BRUSHES)
            .map(|i| {
                let x = i as f64 * 2.0;
                Brush::cuboid([x, 0.0, 0.0], [x + 1.0, 1.0, 1.0], "m", grid)
            })
            .collect();
        let e = build(&brushes, &Options::default()).unwrap_err();
        assert_eq!(
            e,
            BuildError::TooMany {
                what: "brushes",
                count: MAX_BRUSHES + 1,
                cap: MAX_BRUSHES
            }
        );
    }

    #[test]
    fn the_strip_order_turns_back_into_the_polygon() {
        for n in 3..=32 {
            let strip = strip_order(n);
            // `collisionFanFromSurface`'s order: 0, the odd strip places up, then the even ones down.
            let mut fan = vec![0];
            fan.extend((1..n).step_by(2));
            let mut i = (n - 1) & !1;
            while i > 0 {
                fan.push(i);
                i -= 2;
            }
            let back: Vec<usize> = fan.iter().map(|&k| strip[k]).collect();
            assert_eq!(back, (0..n).collect::<Vec<_>>(), "{n}");
        }
    }
}
