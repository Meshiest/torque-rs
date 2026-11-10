//! The map's interiors as the view draws them: each `InteriorInstance` ghost's `.dif` (its first detail level's drawn surfaces,
//! the legacy importer's triangles, [`torque_maps::legacy::IMPORTER`]) at the place its record gives ([`placed`]: a hostile
//! transform or scale is not drawn), as a static mesh of the view ([`WorldView::set_static_mesh`]: uploaded once, kept on the
//! GPU, culled there by its parts' boxes, its shadows in the cached cascades), one batch a material. A frame costs nothing for
//! an interior that stays as it was. A material's texture is found beside the file as the resource manager finds it
//! ([`find_texture`]); the host's look may change each material's draw first (a tint, a filter). A material the file marks
//! ([`MaterialFlags`]) is sampled with nearest-neighbor filtering ([`interior_sampler`], the static mesh's batch keeps its
//! sampler) or drawn see-through: alpha blended, writing no depth, and kept out of the static mesh, each of its panes
//! ([`panes`]) a moving object of its own so the translucent pass sorts them back to front ([`Interiors::update`] returns
//! them each frame, made with the interior's mesh and forgotten with it). Every triangle is wound clockwise seen from its
//! front (its plane's normal). The interior's faces in the world are kept for the surfaces a
//! projectile's hole may lie on ([`Interiors::faces_near`]). A file is read once ([`MAX_FILES`] kept, at most
//! [`MAX_READS_PER_FRAME`] new ones a frame; the importer caps each file's size and work); an interior's mesh is kept until its
//! record moves it, its ghost names another file or its ghost goes; [`Interiors::clear`] makes them all again.
//!
//! Credit: follows TGE 1.4's `interior/interiorInstance.cc` and `interior/interiorRender.cc` (Copyright (C) GarageGames.com,
//! Inc.) as Torque3D 2.0, its MIT-licensed release, keeps them in `interior/interiorInstance.cpp` and
//! `interior/interiorRender.cpp` (Copyright (c) 2012 GarageGames, LLC); the code here is this crate's own.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use glam::Vec3;

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
use torque_render_scene::shapecache::next_stamp;
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

/// The most panes drawn as objects of their own over every interior kept: an interior is built with what the others left
/// ([`Interiors::update`]), its panes past that joining its last one, and one with none left still has one.
pub const MAX_PANES: usize = 0x0fff;

/// The id of the pane object numbered `serial` (the interiors hand them out in turn): a space of its own, apart from the
/// ghosts' objects' (their ghost indices).
pub fn pane_id(serial: u32) -> u32 {
    0x6000_0000 | (serial & 0x0fff_ffff)
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

/// An interior's translucent draws as objects' draws, one object a pane ([`panes`]), so the translucent pass sorts the panes
/// back to front: at most `budget` objects (over every material; at least one; the rest join the last). Each draw's vertices
/// are moved into its panes, never copied whole for each.
pub fn pane_objects(see_through: Vec<Draw>, budget: usize) -> Vec<Vec<Draw>> {
    let budget = budget.clamp(1, MAX_PANES);
    let mut objects: Vec<Vec<Draw>> = Vec::new();
    for mut draw in see_through {
        let vertices = std::mem::take(&mut draw.vertices);
        for pane in panes(&vertices) {
            if objects.len() < budget {
                objects.push(Vec::new());
            }
            let last = objects.len() - 1;
            let into = &mut objects[last];
            match into.iter_mut().find(|d| d.label == draw.label) {
                Some(d) => d.vertices.extend(pane),
                None => into.push(Draw {
                    vertices: pane,
                    ..draw.clone()
                }),
            }
        }
    }
    objects
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

/// An interior whose mesh is in the view, kept with the file and the pose it was made of, its pane objects (its see-through
/// materials, drawn each frame) and its faces in the world (counter-clockwise seen from the front, as the decals take them).
struct Kept {
    /// The file's name as the ghost gave it.
    file: Vec<u8>,
    pose: Pose,
    panes: Vec<DynamicObject>,
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
    /// Each interior's pane objects (by ghost index), their sum, and the next pane object's serial ([`pane_id`]).
    pane_counts: BTreeMap<u32, usize>,
    panes_used: usize,
    serial: u32,
}

impl Interiors {
    /// Forgets every interior's mesh (made again at the next update: a new connection, or a new look); the files stay read.
    pub fn clear(&mut self) {
        self.kept.clear();
        self.pane_counts.clear();
        self.panes_used = 0;
    }

    /// Brings the view's interior meshes up to the world's interiors: each new or moved one made (`look` handed each draw, in
    /// the world, first), each gone one taken out. An interior that stays as it was costs nothing. Returns the frame's pane
    /// objects (the see-through materials of the interiors kept, the only interior objects drawn a frame).
    pub fn update(
        &mut self,
        view: &mut WorldView,
        assets: &dyn Assets,
        world: &ClientWorld<TgeClientSet>,
        look: &mut dyn FnMut(&str, &mut Draw),
    ) -> Vec<DynamicObject> {
        let mut seen = BTreeSet::new();
        let mut reads = 0;
        for (&index, ghost) in &world.ghosts {
            let (Some(TgeRecord::InteriorInstance(r)), Some(pose)) = (&ghost.record, ghost.pose())
            else {
                continue;
            };
            let Some(init) = &r.init else { continue };
            seen.insert(index);
            // A ghost index a new map's interior took (the server reset its ghosting) is made again from its own file.
            if still_kept(&mut self.kept, index, |k| {
                k.pose == pose && k.file == init.file
            }) {
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
            let named = (init.file.as_slice(), path.as_str());
            if let Some(k) = build(view, assets, &surfaces, named, pose, index, look, self) {
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
        let gone: Vec<u32> = self
            .pane_counts
            .keys()
            .filter(|i| !self.kept.contains_key(i))
            .copied()
            .collect();
        for index in gone {
            self.forget_panes(index);
        }
        // Each pane the last frame drew of the same build gives its draws back (moved, not copied).
        let objects = view.objects_mut();
        let last: HashMap<u32, usize> = objects
            .iter()
            .enumerate()
            .filter(|(_, o)| o.stamp.is_some())
            .map(|(i, o)| (o.ghost, i))
            .collect();
        let mut out = Vec::with_capacity(self.panes_used);
        for p in self.kept.values().flat_map(|k| &k.panes) {
            let drawn = last
                .get(&p.ghost)
                .map(|&i| &mut objects[i])
                .filter(|o| o.stamp == p.stamp && !o.draws.is_empty());
            out.push(match drawn {
                Some(o) => DynamicObject {
                    draws: std::mem::take(&mut o.draws),
                    previous: None,
                    nodes: Vec::new(),
                    ..*p
                },
                None => p.clone(),
            });
        }
        out
    }

    /// Interior ghost `index`'s translucent draws as pane objects with their ids, within what the other interiors kept left of
    /// [`MAX_PANES`] (its own earlier panes given back first).
    pub fn pane_groups(&mut self, index: u32, see_through: Vec<Draw>) -> Vec<(Vec<Draw>, u32)> {
        self.forget_panes(index);
        if see_through.is_empty() {
            return Vec::new();
        }
        let objects = pane_objects(see_through, MAX_PANES.saturating_sub(self.panes_used));
        self.panes_used += objects.len();
        self.pane_counts.insert(index, objects.len());
        objects
            .into_iter()
            .map(|d| {
                let id = pane_id(self.serial);
                self.serial = self.serial.wrapping_add(1) & 0x0fff_ffff;
                (d, id)
            })
            .collect()
    }

    fn forget_panes(&mut self, index: u32) {
        if let Some(n) = self.pane_counts.remove(&index) {
            self.panes_used -= n;
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

/// An interior's mesh at `pose` put in the view (its opaque materials), its pane objects and its faces.
#[allow(clippy::too_many_arguments)]
fn build(
    view: &mut WorldView,
    assets: &dyn Assets,
    surfaces: &Surfaces,
    (file, path): (&[u8], &str),
    pose: Pose,
    index: u32,
    look: &mut dyn FnMut(&str, &mut Draw),
    interiors: &mut Interiors,
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
        for t in corners.as_chunks::<3>().0 {
            let mut v = [0, 1, 2].map(|c| {
                let (p, n, uv) = t[c];
                let scale = Vec3::from(scale);
                let normal = Vec3::from(m.mul_v((Vec3::from(n) / scale).to_array()));
                Vertex {
                    position: m.mul_p((Vec3::from(p) * scale).to_array()),
                    normal: (normal / normal.length().max(1e-12)).to_array(),
                    color: [255; 4],
                    uv,
                }
            });
            let [a, b, c] = v.map(|x| Vec3::from(x.position));
            if (b - a).cross(c - a).dot(v[0].normal.into()) > 0.0 {
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
    let groups = interiors.pane_groups(index, see_through);
    let mut panes = view.set_static_mesh_with_objects(mesh_key(index), set, &draws, &groups);
    // Each pane its own stamp: the renderer keeps its vertices where it wrote them, and its motion is its own (it does not move).
    for p in &mut panes {
        p.stamp = Some(next_stamp());
        p.prev_transform = p.transform;
        p.previous = None;
    }
    Some(Kept {
        file: file.to_vec(),
        pose,
        panes,
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

    /// `n` triangles apart, one a pane.
    fn draw(label: &str, n: usize) -> Draw {
        let mut vertices = Vec::with_capacity(3 * n);
        for k in 0..n {
            let x = k as f32 * 3.0;
            vertices.extend(tri([x, 0.0, 0.0], [x + 1.0, 0.0, 0.0], [x, 1.0, 0.0]));
        }
        Draw {
            label: label.to_string(),
            vertices,
            lighting: true,
            tex_env: TexEnv::Modulate,
            texture: None,
            blend: Blend::Alpha,
            cull: Cull::BackCw,
            depth_test: true,
            depth_write: false,
            fog: true,
            lights: Vec::new(),
            vertex_key: None,
        }
    }

    #[test]
    fn many_panes_over_several_materials_make_at_most_the_cap_of_objects_and_keep_every_triangle() {
        let n = MAX_PANES + 500;
        let objects = pane_objects(vec![draw("a", n), draw("b", n)], MAX_PANES);
        assert_eq!(objects.len(), MAX_PANES);
        let vertices: usize = objects.iter().flatten().map(|d| d.vertices.len()).sum();
        assert_eq!(vertices, 2 * 3 * n);
        // The last object holds the rest, one draw a material.
        assert_eq!(objects.last().unwrap().len(), 2);
        // No budget left: still one object.
        assert_eq!(pane_objects(vec![draw("a", 10)], 0).len(), 1);
    }

    #[test]
    fn many_instances_share_one_pane_budget_and_give_theirs_back() {
        let mut interiors = Interiors::default();
        let instances = 200u32;
        let mut ids = std::collections::HashSet::new();
        let mut objects = 0;
        for index in 0..instances {
            for (_, id) in interiors.pane_groups(index, vec![draw("glass", 1000)]) {
                assert!(ids.insert(id), "id {id:#x} twice");
                assert!((0..instances).all(|i| id != i));
                objects += 1;
            }
        }
        // The budget, then one each for the instances it ran out on.
        assert!(
            objects <= MAX_PANES + instances as usize,
            "{objects} objects"
        );
        assert_eq!(interiors.panes_used, objects);
        // An instance built again gives its panes back first; one that leaves gives them back too.
        let again = interiors.pane_groups(0, vec![draw("glass", 1000)]).len();
        assert_eq!(again, MAX_PANES - (objects - 1000));
        assert_eq!(interiors.panes_used, MAX_PANES);
        interiors.forget_panes(0);
        assert_eq!(interiors.panes_used, MAX_PANES - again);
    }

    #[test]
    fn panes_have_their_own_ids_and_nearest_keeps_its_mipmaps() {
        assert_ne!(pane_id(7), 7);
        assert_ne!(pane_id(7), pane_id(8));
        assert_ne!(pane_id(0x1_0007), 0x1_0007);
        let s = interior_sampler(true);
        assert_eq!(
            (s.mag, s.min, s.mip),
            (Filter::Nearest, Filter::Nearest, Filter::Linear)
        );
        assert!(s.mipmaps);
        assert_eq!(interior_sampler(false).mag, Filter::Linear);
    }
}
