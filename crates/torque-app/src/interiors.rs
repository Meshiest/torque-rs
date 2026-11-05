//! The map's interiors as the view draws them: each `InteriorInstance` ghost's `.dif` (its first detail level's drawn surfaces,
//! the legacy importer's triangles, [`torque_maps::legacy::IMPORTER`]) at the place its record gives ([`placed`]: a hostile
//! transform or scale is not drawn), as a static mesh object of the view ([`WorldView::mesh_object`]), one draw a material.
//! A material's texture is found beside the file as the resource manager finds it ([`find_texture`]); the host's look may change each material's draw first (a tint, a filter).
//! A material the file marks ([`MaterialFlags`]) is sampled with nearest-neighbor filtering ([`interior_sampler`]) or drawn
//! see-through: alpha blended, writing no depth, each of its panes ([`panes`]) an object of its own so the translucent pass
//! sorts them back to front. Any other material is drawn as before. Every triangle is
//! wound clockwise seen from its front (its plane's normal). The interior's faces in the world are kept for the surfaces a
//! projectile's hole may lie on ([`Interiors::faces_near`]). A file is read once ([`MAX_FILES`] kept, at most
//! [`MAX_READS_PER_FRAME`] new ones a frame; the importer caps each file's size and work); an interior's object is kept until
//! its record moves it or [`Interiors::clear`].
//!
//! Credit: follows TGE 1.4's `interior/interiorInstance.cc` and `interior/interiorRender.cc` (Copyright (C) GarageGames.com,
//! Inc.) as Torque3D 2.0, its MIT-licensed release, keeps them in `interior/interiorInstance.cpp` and
//! `interior/interiorRender.cpp` (Copyright (c) 2012 GarageGames, LLC); the code here is this crate's own.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use torque_ghost_world::tge::TgeClientSet;
use torque_ghost_world::world::{ClientWorld, Pose};
use torque_ghosts::TgeRecord;
use torque_mapcore::Importer as _;
use torque_mapcore::geometry::Surfaces;
use torque_mapcore::material::MaterialFlags;
use torque_render_scene::decals::Triangle;
use torque_render_scene::hd::DynamicObject;
use torque_render_scene::map::{find_texture, folder_of};
use torque_render_scene::math::Mat;
use torque_render_scene::mounts::pose_matrix;
use torque_render_scene::textures::TextureSet;
use torque_render_scene::{Assets, Blend, Cull, Draw, Filter, Sampler, TexEnv, Vertex};

use crate::view::WorldView;

/// The smallest scale part an interior is placed with (as `InteriorShape::new` takes it).
pub const MIN_SCALE: f32 = 1e-4;
/// The most interior files kept read.
pub const MAX_FILES: usize = 256;
/// The most interior files read in one frame (the others wait for the next).
pub const MAX_READS_PER_FRAME: usize = 2;

/// Where a ghost's pose places it: its matrix and scale (a matrix pose, as an InteriorInstance's record sends, as it is); `None`
/// for a matrix or a scale that is not finite or a scale part under [`MIN_SCALE`] (a hostile record).
pub fn placed(pose: &Pose) -> Option<(Mat, [f32; 3])> {
    let (m, scale) = match *pose {
        Pose::Matrix { m, scale } => (Mat { m }, scale),
        ref other => pose_matrix(other, None)?,
    };
    let ok = m.m.iter().all(|v| v.is_finite())
        && scale.iter().all(|s| s.is_finite() && s.abs() >= MIN_SCALE);
    ok.then_some((m, scale))
}

/// The id of interior ghost `index`'s object among the frame's objects.
pub fn object_id(index: u32) -> u32 {
    0x5000_0000 | (index & 0x0fff_ffff)
}

/// The most panes of one interior drawn as objects of their own (the rest join the last).
pub const MAX_PANES: usize = 0x0fff;

/// The id of pane `pane` (from 0) of interior ghost `index` (a ghost index fits 16 bits).
pub fn pane_id(index: u32, pane: usize) -> u32 {
    0x5000_0000 | (((pane.min(MAX_PANES - 1) as u32 + 1) & 0x0fff) << 16) | (index & 0xffff)
}

/// The sampler of an interior's texture: linear (TGE's), or with `nearest` sharp texels near and far, the mipmap levels
/// still blended between (`GL_NEAREST_MIPMAP_LINEAR`), so a far wall does not shimmer.
pub fn interior_sampler(nearest: bool) -> Sampler {
    let near = if nearest {
        Filter::Nearest
    } else {
        Filter::Linear
    };
    Sampler {
        repeat: true,
        mag: near,
        min: near,
        mip: Filter::Linear,
        mipmaps: true,
    }
}

/// `vertices` (three a triangle) split into panes: triangles that share a corner are one pane. At most [`MAX_PANES`].
pub fn panes(vertices: &[Vertex]) -> Vec<Vec<Vertex>> {
    let tris = vertices.len() / 3;
    let mut parent: Vec<usize> = (0..tris).collect();
    fn root(parent: &mut [usize], mut i: usize) -> usize {
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }
    let key = |p: [f32; 3]| p.map(|c| (c * 1024.0).round() as i64);
    let mut first: HashMap<[i64; 3], usize> = HashMap::new();
    for t in 0..tris {
        for c in 0..3 {
            let k = key(vertices[3 * t + c].position);
            let other = *first.entry(k).or_insert(t);
            let (a, b) = (root(&mut parent, t), root(&mut parent, other));
            if a != b {
                parent[a] = b;
            }
        }
    }
    let mut index: HashMap<usize, usize> = HashMap::new();
    let mut out: Vec<Vec<Vertex>> = Vec::new();
    for t in 0..tris {
        let r = root(&mut parent, t);
        let next = index.len().min(MAX_PANES - 1);
        let k = *index.entry(r).or_insert(next);
        if k == out.len() {
            out.push(Vec::new());
        }
        out[k].extend_from_slice(&vertices[3 * t..3 * t + 3]);
    }
    out
}

/// An interior's objects (its opaque draws, then each pane of its translucent ones), kept with the pose they were made at, and
/// its faces in the world (counter-clockwise seen from the front, as the decals take them).
struct Kept {
    pose: Pose,
    objects: Vec<DynamicObject>,
    faces: Arc<Vec<Triangle>>,
}

/// The interiors the view draws (module docs).
#[derive(Default)]
pub struct Interiors {
    /// The files read, by path (`None` for one that did not read).
    files: HashMap<String, Option<Arc<Surfaces>>>,
    kept: BTreeMap<u32, Kept>,
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

impl Interiors {
    /// Forgets every interior's objects (made again at the next frame: a new connection, or a new look); the files stay read.
    pub fn clear(&mut self) {
        self.kept.clear();
    }

    /// The frame's interior objects: one an interior's material, `look` handed each draw (in the world) before it joins its
    /// object.
    pub fn objects(
        &mut self,
        view: &mut WorldView,
        assets: &dyn Assets,
        world: &ClientWorld<TgeClientSet>,
        look: &mut dyn FnMut(&str, &mut Draw),
    ) -> Vec<DynamicObject> {
        let mut seen = Vec::new();
        let mut reads = 0;
        for (&index, ghost) in &world.ghosts {
            let (Some(TgeRecord::InteriorInstance(r)), Some(pose)) = (&ghost.record, ghost.pose())
            else {
                continue;
            };
            let Some(init) = &r.init else { continue };
            seen.push(index);
            if self.kept.get(&index).is_some_and(|k| k.pose == pose) {
                continue;
            }
            let path = String::from_utf8_lossy(&init.file).replace('\\', "/");
            let surfaces = match self.files.get(&path) {
                Some(kept) => kept.clone(),
                None => {
                    if self.files.len() >= MAX_FILES || reads >= MAX_READS_PER_FRAME {
                        continue;
                    }
                    reads += 1;
                    let read = assets.read(&path).and_then(|bytes| {
                        torque_maps::legacy::IMPORTER
                            .surfaces(&bytes)
                            .ok()
                            .map(Arc::new)
                    });
                    self.files.insert(path.clone(), read.clone());
                    read
                }
            };
            let Some(surfaces) = surfaces else { continue };
            if let Some(k) = build(view, assets, &surfaces, &path, pose, index, look) {
                self.kept.insert(index, k);
            }
        }
        self.kept.retain(|i, _| seen.contains(i));
        self.kept
            .values()
            .flat_map(|k| k.objects.iter().cloned())
            .collect()
    }

    /// Every interior's faces, each list shared (for a host's own surface lookups).
    pub fn faces(&self) -> Vec<Arc<Vec<Triangle>>> {
        self.kept.values().map(|k| Arc::clone(&k.faces)).collect()
    }

    /// The faces whose boxes come within `reach` of `at`.
    pub fn faces_near(&self, at: [f32; 3], reach: f32) -> Vec<Triangle> {
        faces_near(&self.faces(), at, reach)
    }
}

/// The faces of `lists` whose boxes come within `reach` of `at`.
pub fn faces_near(lists: &[Arc<Vec<Triangle>>], at: [f32; 3], reach: f32) -> Vec<Triangle> {
    lists
        .iter()
        .flat_map(|l| l.iter())
        .filter(|t| {
            (0..3).all(|c| {
                let lo = t[0][c].min(t[1][c]).min(t[2][c]);
                let hi = t[0][c].max(t[1][c]).max(t[2][c]);
                at[c] >= lo - reach && at[c] <= hi + reach
            })
        })
        .copied()
        .collect()
}

/// An interior's objects and faces at `pose`.
fn build(
    view: &mut WorldView,
    assets: &dyn Assets,
    surfaces: &Surfaces,
    path: &str,
    pose: Pose,
    index: u32,
    look: &mut dyn FnMut(&str, &mut Draw),
) -> Option<Kept> {
    let (m, scale) = placed(&pose)?;
    let mut set = TextureSet::new(assets);
    let mut draws = Vec::new();
    let mut see_through = Vec::new();
    let mut faces = Vec::new();
    for (material, corners) in &surfaces.by_material {
        let marks = surfaces
            .material_flags
            .get(material)
            .copied()
            .unwrap_or_default();
        let mut vertices = Vec::with_capacity(corners.len());
        for t in corners.chunks_exact(3) {
            let mut v = [0, 1, 2].map(|c| {
                let (p, n, uv) = t[c];
                let normal = m.mul_v([n[0] / scale[0], n[1] / scale[1], n[2] / scale[2]]);
                let len = dot(normal, normal).sqrt().max(1e-12);
                Vertex {
                    position: m.mul_p([p[0] * scale[0], p[1] * scale[1], p[2] * scale[2]]),
                    normal: normal.map(|x| x / len),
                    color: [255; 4],
                    uv,
                }
            });
            let face = cross(
                sub(v[1].position, v[0].position),
                sub(v[2].position, v[0].position),
            );
            if dot(face, v[0].normal) > 0.0 {
                v.swap(1, 2);
            }
            faces.push([v[0].position, v[2].position, v[1].position]);
            vertices.extend(v);
        }
        let sampler = interior_sampler(marks.has(MaterialFlags::NEAREST));
        let texture =
            find_texture(&mut set, material, &[folder_of(path)]).map(|(i, _)| (i, sampler));
        let translucent = marks.has(MaterialFlags::TRANSLUCENT);
        let mut draw = Draw {
            label: material.clone(),
            vertices,
            lighting: true,
            tex_env: TexEnv::Modulate,
            texture,
            blend: if translucent {
                Blend::Alpha
            } else {
                Blend::Opaque
            },
            cull: Cull::BackCw,
            depth_test: true,
            depth_write: !translucent,
            fog: true,
            lights: Vec::new(),
            vertex_key: None,
        };
        look(material, &mut draw);
        if translucent {
            see_through.push(draw);
        } else {
            draws.push(draw);
        }
    }
    // Each pane its own object, so the translucent pass sorts the panes back to front.
    let mut groups = vec![(draws, object_id(index))];
    let mut pane_draws: Vec<Vec<Draw>> = Vec::new();
    for draw in see_through {
        for (k, vertices) in panes(&draw.vertices).into_iter().enumerate() {
            if k == pane_draws.len() {
                pane_draws.push(Vec::new());
            }
            pane_draws[k].push(Draw {
                vertices,
                ..draw.clone()
            });
        }
    }
    groups.extend(
        pane_draws
            .into_iter()
            .enumerate()
            .map(|(k, d)| (d, pane_id(index, k))),
    );
    Some(Kept {
        pose,
        objects: view.mesh_objects(set, &groups),
        faces: Arc::new(faces),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tri(a: [f32; 3], b: [f32; 3], c: [f32; 3]) -> [Vertex; 3] {
        [a, b, c].map(|position| Vertex {
            position,
            normal: [0.0, 0.0, 1.0],
            color: [255; 4],
            uv: [0.0; 2],
        })
    }

    #[test]
    fn triangles_that_share_a_corner_are_one_pane() {
        let mut v = Vec::new();
        // A quad of two triangles, and a triangle apart.
        v.extend(tri([0.0; 3], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0]));
        v.extend(tri([5.0, 0.0, 0.0], [6.0, 0.0, 0.0], [6.0, 1.0, 0.0]));
        v.extend(tri([0.0; 3], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0]));
        let p = panes(&v);
        assert_eq!(p.iter().map(Vec::len).collect::<Vec<_>>(), [6, 3]);
    }

    #[test]
    fn panes_have_their_own_ids_and_nearest_keeps_its_mipmaps() {
        assert_ne!(pane_id(7, 0), object_id(7));
        assert_ne!(pane_id(7, 0), pane_id(7, 1));
        assert_ne!(pane_id(7, 0), pane_id(8, 0));
        let s = interior_sampler(true);
        assert_eq!(
            (s.mag, s.min, s.mip),
            (Filter::Nearest, Filter::Nearest, Filter::Linear)
        );
        assert!(s.mipmaps);
        assert_eq!(interior_sampler(false).mag, Filter::Linear);
    }
}
