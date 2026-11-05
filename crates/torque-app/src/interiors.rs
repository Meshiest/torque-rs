//! The map's interiors as the view draws them: each `InteriorInstance` ghost's `.dif` (its first detail level's drawn surfaces,
//! the legacy importer's triangles, [`torque_maps::legacy::IMPORTER`]) at the place its record gives ([`placed`]: a hostile
//! transform or scale is not drawn), as a static mesh of the view ([`WorldView::set_static_mesh`]: uploaded once, kept on the
//! GPU, culled there by its parts' boxes, its shadows in the cached cascades), one batch a material. A frame costs nothing for
//! an interior that stays as it was. A material's texture is found beside the file as the resource manager finds it
//! ([`find_texture`]); the host's look may change each material's draw first (a tint, a filter). Every triangle is wound
//! clockwise seen from its front (its plane's normal). The interior's faces in the world are kept for the surfaces a
//! projectile's hole may lie on ([`Interiors::faces_near`]). A file is read once ([`MAX_FILES`] kept, at most
//! [`MAX_READS_PER_FRAME`] new ones a frame; the importer caps each file's size and work); an interior's mesh is kept until its
//! record moves it, its ghost names another file or its ghost goes; [`Interiors::clear`] makes them all again.
//!
//! Credit: follows TGE 1.4's `interior/interiorInstance.cc` and `interior/interiorRender.cc` (Copyright (C) GarageGames.com,
//! Inc.) as Torque3D 2.0, its MIT-licensed release, keeps them in `interior/interiorInstance.cpp` and
//! `interior/interiorRender.cpp` (Copyright (c) 2012 GarageGames, LLC); the code here is this crate's own.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use torque_ghost_world::tge::TgeClientSet;
use torque_ghost_world::world::{ClientWorld, Pose};
use torque_ghosts::TgeRecord;
use torque_mapcore::Importer as _;
use torque_mapcore::geometry::Surfaces;
use torque_render_scene::decals::Triangle;
use torque_render_scene::map::{find_texture, folder_of};
use torque_render_scene::math::Mat;
use torque_render_scene::mounts::pose_matrix;
use torque_render_scene::textures::TextureSet;
use torque_render_scene::{Assets, Blend, Cull, Draw, Filter, Sampler, TexEnv, Vertex};

use crate::view::{WorldView, static_mesh_key};

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

/// The key of interior ghost `index`'s mesh among the view's static meshes.
pub fn mesh_key(index: u32) -> [i32; 3] {
    static_mesh_key(index)
}

/// An interior whose mesh is in the view, kept with the file and the pose it was made of, and its faces in the world
/// (counter-clockwise seen from the front, as the decals take them).
struct Kept {
    path: String,
    pose: Pose,
    faces: Arc<Vec<Triangle>>,
}

/// The interiors the view draws (module docs).
#[derive(Default)]
pub struct Interiors {
    /// The files read, by path (`None` for one that did not read).
    files: HashMap<String, Option<Arc<Surfaces>>>,
    kept: BTreeMap<u32, Kept>,
    /// The ghosts whose meshes are in the view (those no longer kept are taken out at the next update).
    placed: BTreeSet<u32>,
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
    /// Forgets every interior's mesh (made again at the next update: a new connection, or a new look); the files stay read.
    pub fn clear(&mut self) {
        self.kept.clear();
    }

    /// Brings the view's interior meshes up to the world's interiors: each new or moved one made (`look` handed each draw, in
    /// the world, first), each gone one taken out. An interior that stays as it was costs nothing.
    pub fn update(
        &mut self,
        view: &mut WorldView,
        assets: &dyn Assets,
        world: &ClientWorld<TgeClientSet>,
        look: &mut dyn FnMut(&str, &mut Draw),
    ) {
        let mut seen = Vec::new();
        let mut reads = 0;
        for (&index, ghost) in &world.ghosts {
            let (Some(TgeRecord::InteriorInstance(r)), Some(pose)) = (&ghost.record, ghost.pose())
            else {
                continue;
            };
            let Some(init) = &r.init else { continue };
            seen.push(index);
            let path = String::from_utf8_lossy(&init.file).replace('\\', "/");
            // A ghost index a new map's interior took (the server reset its ghosting) is made again from its own file.
            if still_kept(&mut self.kept, index, |k| k.pose == pose && k.path == path) {
                continue;
            }
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
                self.placed.insert(index);
            }
        }
        self.kept.retain(|i, _| seen.contains(i));
        let gone: Vec<u32> = self
            .placed
            .iter()
            .copied()
            .filter(|i| !self.kept.contains_key(i))
            .collect();
        for index in gone {
            self.placed.remove(&index);
            view.remove_static_mesh(mesh_key(index));
        }
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

/// An interior's mesh at `pose` put in the view, and its faces.
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
    let mut faces = Vec::new();
    for (material, corners) in &surfaces.by_material {
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
        let texture = find_texture(&mut set, material, &[folder_of(path)]).map(|(i, _)| {
            (
                i,
                Sampler {
                    repeat: true,
                    mag: Filter::Linear,
                    min: Filter::Linear,
                    mip: Filter::Linear,
                    mipmaps: true,
                },
            )
        });
        let mut draw = Draw {
            label: material.clone(),
            vertices,
            lighting: true,
            tex_env: TexEnv::Modulate,
            texture,
            blend: Blend::Opaque,
            cull: Cull::BackCw,
            depth_test: true,
            depth_write: true,
            fog: true,
            lights: Vec::new(),
            vertex_key: None,
        };
        look(material, &mut draw);
        draws.push(draw);
    }
    view.set_static_mesh(mesh_key(index), set, &draws);
    Some(Kept {
        path: path.to_string(),
        pose,
        faces: Arc::new(faces),
    })
}

/// Whether the object kept at `index` is still the one to draw (`same`: made of the file and at the pose the ghost now
/// gives). One that is not is dropped at once, so an interior whose new file cannot be made (not read yet, unreadable, over
/// the caps) draws nothing rather than the one it replaced.
fn still_kept<K>(kept: &mut BTreeMap<u32, K>, index: u32, same: impl Fn(&K) -> bool) -> bool {
    match kept.get(&index) {
        Some(k) if same(k) => true,
        Some(_) => {
            kept.remove(&index);
            false
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_kept_interior_stays_only_while_its_file_and_pose_do() {
        let mut kept = BTreeMap::from([(3, ("kit/arena.dif", 0)), (4, ("kit/keep.dif", 0))]);
        let current = |file: &'static str, pose: u32| move |k: &(&str, u32)| *k == (file, pose);
        assert!(still_kept(&mut kept, 3, current("kit/arena.dif", 0)));
        // Another map's interior at the same index: the old one is gone even before the new one is made.
        assert!(!still_kept(&mut kept, 3, current("kit/yard.dif", 0)));
        assert!(!kept.contains_key(&3));
        // Moved: made again.
        assert!(!still_kept(&mut kept, 4, current("kit/keep.dif", 1)));
        assert!(!kept.contains_key(&4));
        assert!(!still_kept(&mut kept, 5, current("kit/keep.dif", 0)));
    }
}
