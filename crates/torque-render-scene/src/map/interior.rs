//! A map's interiors (`InteriorInstance` over a `.dif`; the engine's own reading of TGE's look).
//!
//! **Geometry.** The importer's drawn surfaces ([`mapcore::geometry::Surfaces`]; the legacy importer's are the resource's first
//! detail level, `maps::legacy`): triangles by material with their normals and UVs. Each instance's points are scaled, rotated
//! and moved by its transform, normals by the inverse scale; every triangle is wound clockwise seen from its front, the
//! fixed-function renderer's front.
//!
//! **Merging.** Every instance's triangles of one texture go into one mesh over the whole map (a map's hundred interiors draw in
//! about as many draws as it has materials). A material's texture is looked up beside the `.dif` (the resource manager's search up
//! the folders included, [`crate::textures::candidates`]); one with alpha is drawn blended. A material the file marks is drawn
//! see-through ([`mapcore::material::MaterialFlags::TRANSLUCENT`]: blended, never lightmapped) or sampled with nearest-neighbor
//! filtering ([`NEAREST_SAMPLER`]), in meshes apart from the same texture unmarked.
//!
//! **Lighting.** With the sun's relight ([`super::light`]) an opaque material's surfaces are drawn unlit and multiplied by
//! their instance's lightmap rectangles in a pass of their own (one draw an atlas page, after the materials): each surface's
//! lightmap texel coordinates come from its lightmap planes, moved from the `.dif`'s lightmap to the rectangle's place in the
//! atlas. Surfaces without a rectangle, translucent materials and every surface until the relight is done are drawn with GL
//! lighting from the sun and white vertices.
//!
//! Credit: follows TGE 1.4's `interior/interiorInstance.cc` and `interior/interiorRender.cc` (Copyright (C) GarageGames.com,
//! Inc.); the code here is this crate's own.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};

use std::sync::Arc;

use super::light::Lit;
use super::{Fnv, Mesh, Object, TexRef, find_texture, folder_of};
use super::{MapClass, MapProfile};
use glam::Vec3;

use crate::math::{Mat, unit_or_same};
use crate::textures::TextureSet;
use crate::{Assets, Blend, Cull, TexEnv, Vertex};
use mapcore::material::MaterialFlags;

/// The most distinct `.dif` files kept parsed on a thread.
pub const MAX_KEPT_FILES: usize = 256;

/// A lightmap rectangle, as the layer has it.
pub(crate) use mapcore::geometry::LightRect as Rect;

/// An interior's triangles in its own space ([`mapcore::geometry::Surfaces`], the importer's; its `content` is the file's bytes'
/// hash, the light's key).
pub(crate) type Local = mapcore::geometry::Surfaces;

type Kept = HashMap<String, (u64, Option<Arc<Local>>)>;

thread_local! {
    /// Each `.dif` read (by its folded path): the files' generation it was read at and its triangles.
    static FILES: RefCell<Kept> = RefCell::new(HashMap::new());
}

/// The triangles of the `.dif` at `path` (read once a files' generation).
pub(crate) fn local(
    path: &str,
    importer: &dyn mapcore::Importer,
    assets: &dyn Assets,
) -> Option<Arc<Local>> {
    let folded = path.to_ascii_lowercase();
    let generation = crate::textures::files_generation();
    if let Some((g, l)) = FILES.with(|f| f.borrow().get(&folded).cloned())
        && g == generation
    {
        return l;
    }
    let made = assets.read(path).and_then(|b| {
        let mut l = importer.surfaces(&b).ok()?;
        let mut h = Fnv::new("dif");
        h.bytes(&b);
        l.content = h.0;
        Some(Arc::new(l))
    });
    FILES.with(|f| {
        let mut f = f.borrow_mut();
        if f.len() >= MAX_KEPT_FILES {
            f.clear();
        }
        f.insert(folded, (generation, made.clone()));
    });
    made
}

/// An instance's placement: the matrix (rotation and translation) and the scale.
fn placement(o: &Object) -> (Mat, [f32; 3]) {
    let s = o.scale();
    let s = s.map(|c| if c.is_finite() && c != 0.0 { c } else { 1.0 });
    (o.matrix(), s)
}

/// An interior instance whose `.dif` was read.
#[derive(Clone)]
pub(crate) struct Instance {
    pub folder: String,
    pub local: Arc<Local>,
    pub matrix: Mat,
    pub scale: [f32; 3],
}

/// Every interior instance of `objects` whose file reads (a note for each that does not).
pub(crate) fn instances(
    objects: &[Object],
    profile: &MapProfile,
    assets: &dyn Assets,
    notes: &mut Vec<String>,
) -> Vec<Instance> {
    let mut out = Vec::new();
    for o in objects
        .iter()
        .filter(|o| o.class == MapClass::InteriorInstance)
    {
        let file = o.text("interiorFile");
        let Some(local) = local(&file, profile.importer, assets) else {
            notes.push(format!("map interior {file}: not read"));
            continue;
        };
        let (matrix, scale) = placement(o);
        out.push(Instance {
            folder: folder_of(&file).to_string(),
            local,
            matrix,
            scale,
        });
    }
    out
}

/// A material of an instance as it is drawn: the merged mesh's group, its texture, whether it is opaque (no alpha in its
/// texture: it casts shadows and takes a lightmap).
#[derive(Debug, Clone)]
pub(crate) struct Material {
    pub group: String,
    pub texture: TexRef,
    pub opaque: bool,
    /// Sampled with nearest-neighbor filtering ([`mapcore::material::MaterialFlags::NEAREST`]).
    pub nearest: bool,
    /// Drawn see-through whatever its texture ([`mapcore::material::MaterialFlags::TRANSLUCENT`]).
    pub see_through: bool,
}

/// Each instance's materials (in `by_material`'s order), their textures looked up once a folder and name.
pub(crate) fn materials(
    instances: &[Instance],
    profile: &MapProfile,
    textures: &mut TextureSet<'_>,
) -> Vec<Vec<Material>> {
    let mut found: HashMap<(String, String, u32), Material> = HashMap::new();
    instances
        .iter()
        .map(|inst| {
            inst.local
                .by_material
                .keys()
                .map(|material| {
                    let marks = inst
                        .local
                        .material_flags
                        .get(material)
                        .copied()
                        .unwrap_or_default();
                    let nearest = marks.has(MaterialFlags::NEAREST);
                    let see_through = marks.has(MaterialFlags::TRANSLUCENT);
                    found
                        .entry((inst.folder.clone(), material.clone(), marks.0))
                        .or_insert_with(|| {
                            let folder = &inst.folder;
                            match find_texture(
                                textures,
                                &format!("{folder}/{material}"),
                                &[folder, profile.interior_fallback],
                            ) {
                                Some((t, n)) => Material {
                                    group: marked(n.to_ascii_lowercase(), marks),
                                    texture: TexRef::Name(n),
                                    opaque: !textures.has_alpha(t) && !see_through,
                                    nearest,
                                    see_through,
                                },
                                None => {
                                    let n = format!("{folder}/{material}");
                                    Material {
                                        group: marked(
                                            format!("missing {}", n.to_ascii_lowercase()),
                                            marks,
                                        ),
                                        texture: TexRef::Missing(n),
                                        opaque: !see_through,
                                        nearest,
                                        see_through,
                                    }
                                }
                            }
                        })
                        .clone()
                })
                .collect()
        })
        .collect()
}

/// A group's name with the marks that change its draw (a marked material is never merged with the same texture unmarked).
fn marked(group: String, marks: MaterialFlags) -> String {
    if marks == MaterialFlags::default() {
        group
    } else {
        format!("{group} marked {}", marks.0)
    }
}

/// The light's view of the instances.
pub(crate) fn light_inputs(
    instances: &[Instance],
    materials: &[Vec<Material>],
) -> Vec<super::light::InstanceIn> {
    instances
        .iter()
        .zip(materials)
        .map(|(i, m)| super::light::InstanceIn {
            local: Arc::clone(&i.local),
            matrix: i.matrix,
            scale: i.scale,
            opaque: m.iter().map(|m| m.opaque).collect(),
        })
        .collect()
}

/// The sampler of a material marked nearest: sharp texels near and far, the mipmap levels still blended between
/// (`GL_NEAREST_MIPMAP_LINEAR`), so a far wall does not shimmer.
pub const NEAREST_SAMPLER: crate::Sampler = crate::Sampler {
    repeat: true,
    mag: crate::Filter::Nearest,
    min: crate::Filter::Nearest,
    mip: crate::Filter::Linear,
    mipmaps: true,
};

/// The sampler of a lightmap page: clamped (each rectangle has its gutter), linear, level 0 only.
pub const LIGHTMAP_SAMPLER: crate::Sampler = crate::Sampler {
    repeat: false,
    mag: crate::Filter::Linear,
    min: crate::Filter::Linear,
    mip: crate::Filter::Linear,
    mipmaps: false,
};

/// The merged meshes of every instance: a mesh a texture (lit ones apart), then with `lit` a lightmap pass an atlas page.
pub(crate) fn meshes(
    instances: &[Instance],
    materials: &[Vec<Material>],
    key: u64,
    lit: Option<&Lit>,
    notes: &mut Vec<String>,
) -> Vec<Mesh> {
    meshes_capped(
        instances,
        materials,
        key,
        lit,
        notes,
        super::MAX_MERGED_VERTICES,
    )
}

/// [`meshes`] holding at most `cap` merged vertices.
pub(crate) fn meshes_capped(
    instances: &[Instance],
    materials: &[Vec<Material>],
    key: u64,
    lit: Option<&Lit>,
    notes: &mut Vec<String>,
    cap: usize,
) -> Vec<Mesh> {
    let mut merged = 0usize;
    // By group: texture, whether lightmapped, the vertices and the sum of their positions (the mesh's centre), and its
    // material (whether see-through, whether sampled nearest).
    type Group = (TexRef, bool, Vec<Vertex>, [f64; 3], (bool, bool));
    let mut groups: BTreeMap<String, Group> = BTreeMap::new();
    let pages = lit.map_or(0, |l| l.pages.len());
    let mut passes: Vec<(Vec<Vertex>, [f64; 3])> = vec![(Vec::new(), [0.0; 3]); pages];
    'all: for (ii, (inst, mats)) in instances.iter().zip(materials).enumerate() {
        let (m, s) = (inst.matrix, inst.scale);
        let mirrored = s[0] * s[1] * s[2] < 0.0;
        let places = lit.and_then(|l| l.plan.places.get(ii));
        for (k, (tris, lms)) in inst
            .local
            .by_material
            .values()
            .zip(inst.local.lightmap_tris.values())
            .enumerate()
        {
            let Some(mat) = mats.get(k) else {
                continue;
            };
            for (t, lm) in tris.as_chunks::<3>().0.iter().zip(lms) {
                if merged >= cap {
                    notes.push(format!(
                        "map interiors: past {cap} vertices, {} of {} instances not all drawn",
                        instances.len() - ii,
                        instances.len()
                    ));
                    break 'all;
                }
                merged += 3;
                let place = places
                    .filter(|_| mat.opaque)
                    .and_then(|p| p.get(lm.rect as usize).copied().flatten());
                let mut v = [0, 1, 2].map(|k| {
                    let (p, n, uv) = t[k];
                    let s = Vec3::from(s);
                    let w = m.mul_p((Vec3::from(p) * s).to_array());
                    let wn = Vec3::from(m.mul_v((Vec3::from(n) / s).to_array()));
                    Vertex {
                        position: w,
                        normal: unit_or_same(wn).to_array(),
                        color: [255; 4],
                        uv,
                    }
                });
                let mut lm_uv = [0, 1, 2].map(|k| lm.texel[k]);
                let [a, b, c] = v.map(|x| Vec3::from(x.position));
                let facing = (b - a).cross(c - a).dot(v[0].normal.into());
                // Clockwise seen from the front.
                if facing > 0.0 || (mirrored && facing == 0.0) {
                    v.swap(1, 2);
                    lm_uv.swap(1, 2);
                }
                let lightmapped = place.is_some();
                let label = if lightmapped {
                    format!("{} lit", mat.group)
                } else {
                    mat.group.clone()
                };
                let entry = groups.entry(label).or_insert_with(|| {
                    (
                        mat.texture.clone(),
                        lightmapped,
                        Vec::new(),
                        [0.0; 3],
                        (mat.see_through, mat.nearest),
                    )
                });
                for x in v {
                    for c in 0..3 {
                        entry.3[c] += f64::from(x.position[c]);
                    }
                    entry.2.push(x);
                }
                if let (Some(p), Some(l)) = (place, lit) {
                    let r = &inst.local.rects[lm.rect as usize];
                    let [pw, ph] = l.plan.pages[p.page as usize];
                    let pass = &mut passes[p.page as usize];
                    for (x, t) in v.iter().zip(lm_uv) {
                        let u = (p.x as f32 + 1.0 + t[0] - r.offset[0] as f32) / pw as f32;
                        let w = (p.y as f32 + 1.0 + t[1] - r.offset[1] as f32) / ph as f32;
                        for c in 0..3 {
                            pass.1[c] += f64::from(x.position[c]);
                        }
                        pass.0.push(Vertex {
                            color: [255; 4],
                            uv: [u, w],
                            ..*x
                        });
                    }
                }
            }
        }
    }
    let light_key = lit.map_or(0, |l| l.key);
    let mut out: Vec<Mesh> = groups
        .into_iter()
        .enumerate()
        .map(
            |(n, (label, (texture, lightmapped, vertices, sum, (see_through, nearest))))| {
                let count = vertices.len().max(1) as f64;
                let mut h = Fnv::new("map interior mesh");
                h.u64(key);
                h.u64(light_key);
                h.u64(n as u64);
                Mesh {
                    label: format!("interiors {label}"),
                    texture,
                    vertices: Arc::new(vertices),
                    lighting: !lightmapped,
                    tex_env: TexEnv::Modulate,
                    blend: if see_through {
                        Blend::Alpha
                    } else {
                        Blend::Opaque
                    },
                    cull: Cull::BackCw,
                    depth_write: !see_through,
                    sampler: if nearest {
                        NEAREST_SAMPLER
                    } else {
                        super::SAMPLER
                    },
                    alpha_from_texture: true,
                    overlay: false,
                    centre: sum.map(|c| (c / count) as f32),
                    key: h.0,
                }
            },
        )
        .collect();
    if let Some(l) = lit {
        for (n, (vertices, sum)) in passes.into_iter().enumerate() {
            let count = vertices.len().max(1) as f64;
            let mut h = Fnv::new("map interior lightmap");
            h.u64(key);
            h.u64(l.key);
            h.u64(n as u64);
            out.push(Mesh {
                label: format!("interiors lightmap {n}"),
                texture: TexRef::Generated(Arc::clone(&l.pages[n])),
                vertices: Arc::new(vertices),
                lighting: false,
                tex_env: TexEnv::Replace,
                blend: Blend::Multiply,
                cull: Cull::BackCw,
                depth_write: false,
                sampler: LIGHTMAP_SAMPLER,
                alpha_from_texture: false,
                overlay: true,
                centre: sum.map(|c| (c / count) as f32),
                key: h.0,
            });
        }
    }
    out
}

/// Each interior instance's world bounds.
pub(crate) fn bounds(
    objects: &[Object],
    profile: &MapProfile,
    assets: &dyn Assets,
) -> Vec<([f32; 3], [f32; 3])> {
    let mut out = Vec::new();
    for o in objects
        .iter()
        .filter(|o| o.class == MapClass::InteriorInstance)
    {
        let Some(l) = local(&o.text("interiorFile"), profile.importer, assets) else {
            continue;
        };
        let (m, s) = placement(o);
        let (lo, hi) = l.bounds;
        let mut min = [f32::INFINITY; 3];
        let mut max = [f32::NEG_INFINITY; 3];
        for k in 0..8 {
            let p = [
                if k & 1 == 0 { lo[0] } else { hi[0] },
                if k & 2 == 0 { lo[1] } else { hi[1] },
                if k & 4 == 0 { lo[2] } else { hi[2] },
            ];
            let w = m.mul_p([p[0] * s[0], p[1] * s[1], p[2] * s[2]]);
            for c in 0..3 {
                min[c] = min[c].min(w[c]);
                max[c] = max[c].max(w[c]);
            }
        }
        if min.iter().chain(&max).all(|c| c.is_finite()) {
            out.push((min, max));
        }
    }
    out
}
