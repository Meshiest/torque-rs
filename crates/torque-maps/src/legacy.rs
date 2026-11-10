//! The legacy importer: a `.ter`, `.dif` and `.dml` made into the format-neutral
//! map layer ([`torque_mapcore`]). The consumers (hosting, collision, drawing, the relight) call [`IMPORTER`] through
//! [`torque_mapcore::Importer`] and read only what comes back; this module is the only place that knows the legacy
//! files' layouts beyond the readers.
//!
//! - **Heightfield** ([`Legacy::heightfield`]): a `.ter`'s 256 x 256 heights in metres and its materials as layers
//!   (each one's alpha map, or in a version-1 file 255 where the base slot is its), cut [`torque_mapcore::collision::Split::Alternating`].
//! - **Surfaces** ([`Legacy::surfaces`]): the resource's first detail level. Each drawn surface is a triangle strip
//!   of its windings (`Interior`'s draw, as Torque3D's `interior.cpp` walks it), its UVs from its texgen planes (`u = x . p + x[3]`), its normal
//!   its plane's (negated when the plane index carries the flip bit). Surfaces of the editor's special materials (TGE's
//!   [`SPECIAL`], or the importer's own [`Legacy::special`]) are left out. A material whose surfaces carry this engine's
//!   [`dif::surface_flags::TRANSLUCENT`] or [`dif::surface_flags::NEAREST`] has them in the surfaces' `material_flags`. Each surface's lightmap rectangle comes from its lightmap planes and its place in the file's lightmap,
//!   bounded by [`torque_mapcore::budget::MAX_SOURCE_SIDE`] and [`torque_mapcore::budget::MAX_RECT_SIDE`].
//! - **Convexes** ([`Legacy::convexes`]): the first detail level's hulls: points `hullIndices` from `hullStart`, planes
//!   `hullPlaneIndices` from `planeStart` to the next hull's start (a plane index's high bit flips it), the ranges as the file
//!   names them.
//! - **Placement** ([`terrain_placement`]): a `TerrainBlock` record's square size, repeat flag and `emptySquares` runs, at
//!   height 0 whatever the mission wrote (`TerrainBlock::onAdd` sets the position's z to 0).
//! - **Files a model names** ([`Importer::named_files`]): its materials but the special ones, each once.
//!
//! The [`IMPORTER`] reads as TGE does; a [`Legacy`] with other settings reads the `.dif` layout with the border word
//! ([`dif::parse_with_border_pad`]) or takes another set of special materials.

use std::collections::{HashMap, HashSet};

use torque_mapcore::collision::{
    ConvexSet, Heightfield, HeightfieldPlacement, HullRange, Layer, Plane, PlaneRef, Split,
};
use torque_mapcore::geometry::{LightRect, LightmapTri, NO_RECT, Surfaces};
use torque_mapcore::material::{EncodedImage, Encoding, MaterialFlags};
use torque_mapcore::{FileKind, ImportError, Importer};

use crate::dif::{self, Interior, PLANE_FLIPPED};
use crate::{MAX_DIF_BYTES, MAX_DML_BYTES, MAX_TER_BYTES, dml, ter};

/// The material names `Interior::prepForRendering` takes out of the list before it loads the textures (compared with their
/// case): the editor's materials for surfaces that are not drawn (map2dif makes most of them null surfaces already).
pub const SPECIAL: [&str; 4] = ["NULL", "ORIGIN", "TRIGGER", "FORCEFIELD"];

/// Whether `material` is one of TGE's [`SPECIAL`] names, case and all.
pub fn is_special(material: &str) -> bool {
    SPECIAL.contains(&material)
}

/// The legacy importer and its settings.
#[derive(Debug, Clone, Copy)]
pub struct Legacy {
    /// Read `.dif`s with [`dif::parse_with_border_pad`] instead of [`dif::parse`].
    pub border_pad: bool,
    /// The special materials: their surfaces are not drawn and their files are not named.
    pub special: fn(&str) -> bool,
}

impl Legacy {
    /// TGE's reading.
    pub const TGE: Legacy = Legacy {
        border_pad: false,
        special: is_special,
    };

    fn dif(&self, bytes: &[u8]) -> Result<dif::Dif, crate::MapError> {
        if self.border_pad {
            dif::parse_with_border_pad(bytes)
        } else {
            dif::parse(bytes)
        }
    }
}

impl Default for Legacy {
    fn default() -> Legacy {
        Legacy::TGE
    }
}

/// The legacy importer as TGE reads.
pub static IMPORTER: Legacy = Legacy::TGE;

fn error(e: crate::MapError) -> ImportError {
    ImportError(e.to_string())
}

impl Importer for Legacy {
    fn name(&self) -> &'static str {
        "legacy"
    }

    fn max_bytes(&self, kind: FileKind) -> usize {
        match kind {
            FileKind::Heightfield => MAX_TER_BYTES,
            FileKind::Model => MAX_DIF_BYTES,
            FileKind::SkyList => MAX_DML_BYTES,
        }
    }

    fn heightfield(&self, bytes: &[u8]) -> Result<Heightfield, ImportError> {
        ter::parse(bytes).map(|t| heightfield(&t)).map_err(error)
    }

    fn surfaces(&self, bytes: &[u8]) -> Result<Surfaces, ImportError> {
        self.dif(bytes)
            .map(|d| surfaces_with(&d, self.special))
            .map_err(error)
    }

    fn convexes(&self, bytes: &[u8]) -> Result<ConvexSet, ImportError> {
        let d = self.dif(bytes).map_err(error)?;
        d.details
            .first()
            .map(convexes)
            .ok_or_else(|| ImportError("the file has no detail level".into()))
    }

    fn sky_list(&self, bytes: &[u8]) -> Result<Vec<String>, ImportError> {
        dml::parse(bytes).map_err(error)
    }

    fn named_files(&self, kind: FileKind, bytes: &[u8]) -> Result<Vec<String>, ImportError> {
        match kind {
            // A terrain's names as the file spells them, an empty slot left out.
            FileKind::Heightfield => ter::parse(bytes).map_err(error).map(|t| {
                t.materials
                    .into_iter()
                    .filter(|m| !m.name.is_empty())
                    .map(|m| m.name)
                    .collect()
            }),
            // Every interior's materials (detail levels and sub-objects), each once, but the special ones.
            FileKind::Model => {
                let d = self.dif(bytes).map_err(error)?;
                // A set of the folded names beside the list keeps the pass linear in the names a file can hold (millions).
                let mut seen: HashSet<String> = HashSet::new();
                let mut out: Vec<String> = Vec::new();
                for interior in d.details.iter().chain(d.sub_objects.iter()) {
                    for m in &interior.materials {
                        if m.is_empty() || (self.special)(m) {
                            continue;
                        }
                        if seen.insert(m.to_ascii_lowercase()) {
                            out.push(m.clone());
                        }
                    }
                }
                Ok(out)
            }
            FileKind::SkyList => dml::parse(bytes).map_err(error),
        }
    }
}

/// A `.ter` as a heightfield (module docs).
pub fn heightfield(t: &ter::Terrain) -> Heightfield {
    let heights = (0..ter::SAMPLES)
        .map(|i| t.height(i % ter::SIZE, i / ter::SIZE))
        .collect();
    let layers = t
        .materials
        .iter()
        .enumerate()
        .map(|(k, m)| Layer {
            material: m.name.clone(),
            weights: (0..ter::SAMPLES).map(|i| t.alpha(k, i)).collect(),
        })
        .collect();
    Heightfield {
        heights,
        layers,
        split: Split::Alternating,
    }
}

/// Where a `TerrainBlock` stands: sample (0, 0) at [`terrain_origin`] of its square size, at height 0 (`TerrainBlock::onAdd`
/// sets the position's z to 0 whatever the mission wrote), repeated as many times as the host asks, the squares its
/// `emptySquares` runs name left out.
pub fn terrain_placement(
    square_size: i32,
    repeat: bool,
    empty_runs: &[u32],
) -> HeightfieldPlacement {
    HeightfieldPlacement::new(
        terrain_origin(square_size),
        0.0,
        square_size,
        repeat,
        ter::empty_squares(empty_runs),
    )
}

/// Where a block's sample (0, 0) lies in x and y for its square size (held to 1..[`torque_mapcore::budget::MAX_SQUARE_SIZE`]):
/// [`ter::block_origin`].
pub fn terrain_origin(square_size: i32) -> [f32; 2] {
    ter::block_origin(square_size.clamp(1, torque_mapcore::budget::MAX_SQUARE_SIZE) as f32)
}

/// A `.dif`'s first detail level's hulls (module docs; no index checked: the consumer checks them).
pub fn convexes(i: &Interior) -> ConvexSet {
    let h = &i.hulls;
    let hulls = h
        .hulls
        .iter()
        .enumerate()
        .map(|(k, hull)| HullRange {
            point_start: hull.hull_start,
            point_count: u32::from(hull.hull_count),
            plane_start: hull.plane_start,
            plane_end: h
                .hulls
                .get(k + 1)
                .map_or(h.hull_plane_indices.len() as u32, |n| n.plane_start),
            contents: torque_mapcore::collision::Contents::SOLID,
        })
        .collect();
    ConvexSet {
        points: i.points.clone(),
        planes: i
            .planes
            .iter()
            .map(|p| Plane {
                normal: p.normal,
                d: p.d,
            })
            .collect(),
        point_refs: h.hull_indices.clone(),
        plane_refs: h
            .hull_plane_indices
            .iter()
            .map(|&raw| PlaneRef {
                index: u32::from(raw & !PLANE_FLIPPED),
                flipped: raw & PLANE_FLIPPED != 0,
            })
            .collect(),
        hulls,
    }
}

/// The most winding points one detail level's drawn surfaces take together, as a multiple of its windings. A well-formed file's
/// surfaces use each winding once; a file whose surfaces name the same long run over and over would otherwise make the
/// output (triangles, lightmap polygons) grow with surfaces x windings. The surfaces past the budget are left out.
pub const SURFACE_POINTS_PER_WINDING: usize = 4;

/// A `.dif`'s drawn surfaces, TGE's [`SPECIAL`] materials left out (module docs).
pub fn surfaces(d: &dif::Dif) -> Surfaces {
    surfaces_with(d, is_special)
}

/// [`surfaces`] with the materials `special` names left out.
pub fn surfaces_with(d: &dif::Dif, special: fn(&str) -> bool) -> Surfaces {
    let mut out = Surfaces::default();
    let Some(i) = d.details.first() else {
        return out;
    };
    out.bounds = (i.bounds.min, i.bounds.max);
    out.lightmaps = i
        .lightmaps
        .iter()
        .map(|l| EncodedImage {
            encoding: Encoding::Png,
            width: l.png.width,
            height: l.png.height,
            bytes: l.png.bytes.clone(),
        })
        .collect();
    let mut rect_of: HashMap<(u32, [u32; 2], [u32; 2]), u32> = HashMap::new();
    let mut budget = i.windings.len().saturating_mul(SURFACE_POINTS_PER_WINDING);
    for (si, s) in i.surfaces.iter().enumerate() {
        let Some(material) = i.materials.get(s.texture as usize) else {
            continue;
        };
        if special(material) {
            continue;
        }
        let Some(plane) = i.planes.get((s.plane & !PLANE_FLIPPED) as usize) else {
            continue;
        };
        let n = if s.plane & PLANE_FLIPPED != 0 {
            plane.normal.map(|c| -c)
        } else {
            plane.normal
        };
        let Some(tg) = i.texgens.get(s.texgen as usize) else {
            continue;
        };
        let start = s.winding_start as usize;
        let Some(strip) = i.windings.get(start..start + s.winding_count as usize) else {
            continue;
        };
        let Some(left) = budget.checked_sub(strip.len()) else {
            break;
        };
        budget = left;
        let pts: Vec<[f32; 3]> = strip
            .iter()
            .filter_map(|&k| i.points.get(k as usize).copied())
            .collect();
        if pts.len() != strip.len() {
            continue;
        }
        let uv = |p: [f32; 3]| {
            [
                tg.x[0] * p[0] + tg.x[1] * p[1] + tg.x[2] * p[2] + tg.x[3],
                tg.y[0] * p[0] + tg.y[1] * p[1] + tg.y[2] * p[2] + tg.y[3],
            ]
        };
        let rect = surface_rect(i, si, s, n, plane.d, &pts, &mut out.rects, &mut rect_of);
        let texel = |p: [f32; 3]| -> [f32; 2] {
            match out.rects.get(rect as usize) {
                Some(r) => [0, 1].map(|k| p[r.axes[k]] * r.scale[k] + r.add[k]),
                None => [0.0; 2],
            }
        };
        let mut marks = MaterialFlags::default();
        if s.flags & dif::surface_flags::TRANSLUCENT != 0 {
            marks = marks.with(MaterialFlags::TRANSLUCENT);
        }
        if s.flags & dif::surface_flags::NEAREST != 0 {
            marks = marks.with(MaterialFlags::NEAREST);
        }
        if marks != MaterialFlags::default() {
            let kept = out.material_flags.entry(material.clone()).or_default();
            *kept = kept.with(marks);
        }
        let mut lms = Vec::new();
        let list = out.by_material.entry(material.clone()).or_default();
        for k in 0..pts.len().saturating_sub(2) {
            let (a, b, c) = if k % 2 == 0 {
                (pts[k], pts[k + 1], pts[k + 2])
            } else {
                (pts[k + 1], pts[k], pts[k + 2])
            };
            for p in [a, b, c] {
                list.push((p, n, uv(p)));
            }
            lms.push(LightmapTri {
                rect,
                texel: [texel(a), texel(b), texel(c)],
            });
        }
        out.lightmap_tris
            .entry(material.clone())
            .or_default()
            .extend(lms);
    }
    out
}

/// Surface `si`'s lightmap rectangle (made, or shared with an earlier surface), [`NO_RECT`] when its lightmap, its rectangle
/// or its lightmap planes do not hold.
#[allow(clippy::too_many_arguments)]
fn surface_rect(
    i: &Interior,
    si: usize,
    s: &dif::Surface,
    normal: [f32; 3],
    plane_d: f32,
    pts: &[[f32; 3]],
    rects: &mut Vec<LightRect>,
    rect_of: &mut HashMap<(u32, [u32; 2], [u32; 2]), u32>,
) -> u32 {
    let Some(&lightmap) = i.normal_lightmaps.get(si) else {
        return NO_RECT;
    };
    let Some(png) = i.lightmaps.get(lightmap as usize).map(|l| &l.png) else {
        return NO_RECT;
    };
    let (w, h) = (png.width, png.height);
    let side = torque_mapcore::budget::MAX_SOURCE_SIDE;
    let [ox, oy] = s.map_offset;
    let [sw, sh] = s.map_size;
    let max = torque_mapcore::budget::MAX_RECT_SIDE;
    if w == 0 || h == 0 || w > side || h > side || sw == 0 || sh == 0 || sw > max || sh > max {
        return NO_RECT;
    }
    if u64::from(ox) + u64::from(sw) > u64::from(w) || u64::from(oy) + u64::from(sh) > u64::from(h)
    {
        return NO_RECT;
    }
    let axis = |g: &[f32; 4]| -> Option<usize> {
        let nz: Vec<usize> = (0..3)
            .filter(|&k| g[k] != 0.0 && g[k].is_finite())
            .collect();
        (nz.len() == 1).then(|| nz[0])
    };
    let tg = &s.lightmap_texgen;
    let (Some(a0), Some(a1)) = (axis(&tg.x), axis(&tg.y)) else {
        return NO_RECT;
    };
    if a0 == a1 || !tg.x[3].is_finite() || !tg.y[3].is_finite() || normal[3 - a0 - a1].abs() < 1e-4
    {
        return NO_RECT;
    }
    // The surface's own plane, its flip applied (`normal . p + d = 0`).
    let d = if s.plane & PLANE_FLIPPED != 0 {
        -plane_d
    } else {
        plane_d
    };
    let key = (lightmap, s.map_offset, s.map_size);
    if let Some(&r) = rect_of.get(&key) {
        if let Some(rect) = rects.get_mut(r as usize) {
            rect.polys.push(pts.to_vec());
        }
        return r;
    }
    let r = rects.len() as u32;
    rects.push(LightRect {
        lightmap,
        offset: s.map_offset,
        size: s.map_size,
        axes: [a0, a1],
        scale: [tg.x[a0] * w as f32, tg.y[a1] * h as f32],
        add: [tg.x[3] * w as f32, tg.y[3] * h as f32],
        normal,
        d,
        polys: std::iter::once(pts.to_vec()).collect(),
    });
    rect_of.insert(key, r);
    r
}

#[cfg(test)]
#[path = "legacy_tests.rs"]
mod tests;
