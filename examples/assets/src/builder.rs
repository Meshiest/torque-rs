//! A small builder of shape models: named nodes in a tree, objects of triangle meshes (each in one detail level), details,
//! materials and keyframed sequences, made into a [`Model`] that [`torque_shapes::write::write_dts`] writes as a version-24
//! `.dts`.
//!
//! **What it derives**, so that the model is what an exporter writes: the name table (each name once), each node's first
//! object, first child and next sibling and each object's next sibling (`TSShape::init`'s links), a default state for each
//! object, each detail's triangle count and the smallest visible detail, every mesh's box, centre and radius
//! ([`bounds::compute`]), the shape's box (detail 0 placed by the default pose, [`bounds::detail_bounds`]) and the figures
//! derived from it ([`bounds::shape_derived`]), and each sequence's keyframe ranges and membership sets.
//!
//! **Conventions.** Torque's axes: x right, y forward, z up. A triangle's front is clockwise seen from outside (the shape
//! renderer culls the other side); [`MeshData::add_box`] winds every face that way. Rotations are stored as Torque stores
//! them ([`turn`]).
//!
//! Credit: the derived links, detail figures and default states follow `TSShape::init`, and [`turn`] the quaternion layout of
//! `QuatF::setMatrix`, as Torque3D carries them under the MIT licence (`Engine/source/ts/tsShape.cpp` and
//! `Engine/source/math/mQuat.cpp`, Copyright (c) 2012 GarageGames, LLC). The code here is this crate's own.

use std::sync::Arc;

use torque_shapes::bounds;
use torque_shapes::shape::{
    Detail, FileExtras, Material, Mesh, MeshKind, MeshRender, Model, Node, Object, ObjectState,
    Primitive, Quat16, RawMaterial, RawMaterials, Sequence, Shape, SubShape,
};

pub type Vec3 = [f32; 3];

/// A texture rectangle `[u0, v0, u1, v1]` (v grows down the image).
pub type UvRect = [f32; 4];

/// The whole texture.
pub const FULL_UV: UvRect = [0.0, 0.0, 1.0, 1.0];

/// A node's rest rotation: none.
pub const NO_TURN: Quat16 = [0, 0, 0, 32767];

/// The material flags of a shape's material list (`TSMaterialList`).
pub mod material_flags {
    /// The texture repeats across u.
    pub const S_WRAP: u32 = 0x1;
    /// The texture repeats across v.
    pub const T_WRAP: u32 = 0x2;
    /// Drawn without lighting, at full brightness.
    pub const SELF_ILLUMINATING: u32 = 0x20;
    /// Never environment-mapped.
    pub const NEVER_ENV_MAP: u32 = 0x40;
}

/// `TSDrawPrimitive`'s bits: an indexed triangle list, and a primitive with no material.
const PRIM_INDEXED: u32 = 0x2000_0000;
const PRIM_NO_MATERIAL: u32 = 0x1000_0000;
/// `TSShape::Sequence`'s flag for a sequence that loops.
const SEQ_CYCLIC: u32 = 16;
/// The "no map" index of the material list's reflectance, bump and detail maps.
const NO_MAP: u32 = u32::MAX;

/// A turn of `degrees` about `axis`, right-handed (a positive turn about x takes y toward z), as a node rotation.
///
/// Torque's quaternion-to-matrix routine (`QuatF::setMatrix`) writes the transpose of the usual matrix, so the stored
/// quaternion is the conjugate of the usual one: its vector part is negated.
pub fn turn(axis: Vec3, degrees: f32) -> Quat16 {
    let len = (axis[0] * axis[0] + axis[1] * axis[1] + axis[2] * axis[2]).sqrt();
    if len == 0.0 || degrees == 0.0 {
        return NO_TURN;
    }
    let half = degrees.to_radians() * 0.5;
    let (s, c) = (half.sin() / len, half.cos());
    let q = [-axis[0] * s, -axis[1] * s, -axis[2] * s, c];
    // `Quat16::set`: each component times 32767, truncated.
    q.map(|v| (v * 32767.0) as i16)
}

/// One face of a box, by its outward normal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Face {
    XNeg,
    XPos,
    YNeg,
    YPos,
    ZNeg,
    ZPos,
}

impl Face {
    pub const ALL: [Face; 6] = [
        Face::XNeg,
        Face::XPos,
        Face::YNeg,
        Face::YPos,
        Face::ZNeg,
        Face::ZPos,
    ];

    /// The outward normal, and the face's u and v directions (each a unit axis): `u x v` is the normal, so the corners
    /// (-u -v), (+u -v), (+u +v), (-u +v) go counter-clockwise seen from outside. On the side faces v is up.
    fn frame(self) -> (Vec3, Vec3, Vec3) {
        match self {
            Face::XPos => ([1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]),
            Face::XNeg => ([-1.0, 0.0, 0.0], [0.0, -1.0, 0.0], [0.0, 0.0, 1.0]),
            Face::YPos => ([0.0, 1.0, 0.0], [-1.0, 0.0, 0.0], [0.0, 0.0, 1.0]),
            Face::YNeg => ([0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]),
            Face::ZPos => ([0.0, 0.0, 1.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
            Face::ZNeg => ([0.0, 0.0, -1.0], [1.0, 0.0, 0.0], [0.0, -1.0, 0.0]),
        }
    }
}

/// The corner of the box `min`..`max` in the direction `d` (each component's sign picks the side; 0 never occurs here).
fn corner(min: Vec3, max: Vec3, d: Vec3) -> Vec3 {
    [0, 1, 2].map(|k| if d[k] > 0.0 { max[k] } else { min[k] })
}

/// A mesh of one material: triangles (three indices each) over vertices with a normal and a texture coordinate each.
#[derive(Debug, Clone, PartialEq)]
pub struct MeshData {
    pub positions: Vec<Vec3>,
    pub normals: Vec<Vec3>,
    pub tverts: Vec<[f32; 2]>,
    pub indices: Vec<u16>,
    /// The material list's index, or None for a collision mesh (no material).
    pub material: Option<u32>,
}

impl MeshData {
    /// An empty mesh drawn with material `material`.
    pub fn new(material: Option<u32>) -> MeshData {
        MeshData {
            positions: Vec::new(),
            normals: Vec::new(),
            tverts: Vec::new(),
            indices: Vec::new(),
            material,
        }
    }

    /// The triangles.
    pub fn triangles(&self) -> usize {
        self.indices.len() / 3
    }

    /// Adds the box `min`..`max` with flat faces (four vertices a face, its own normal), each face's texture the rectangle
    /// `uv(face)` (the top of a side face at the top of the rectangle).
    pub fn add_box(&mut self, min: Vec3, max: Vec3, uv: impl Fn(Face) -> UvRect) {
        for face in Face::ALL {
            let (n, u, v) = face.frame();
            let r = uv(face);
            let base = self.positions.len() as u16;
            for (su, sv) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
                let d = [0, 1, 2].map(|k| n[k] + su * u[k] + sv * v[k]);
                self.positions.push(corner(min, max, d));
                self.normals.push(n);
                let tu = if su < 0.0 { r[0] } else { r[2] };
                let tv = if sv > 0.0 { r[1] } else { r[3] };
                self.tverts.push([tu, tv]);
            }
            // The corners go counter-clockwise seen from outside; each triangle is written the other way round.
            self.indices
                .extend([0, 2, 1, 0, 3, 2].map(|i: u16| base + i));
        }
    }

    /// Adds a prism: the convex polygon `profile` (points `[y, z]`, in order around it either way) drawn between `x0` and `x1`
    /// (`x0 < x1`), with flat faces. Its two ends take the texture rectangle `ends` laid over the profile's box (u along y, v
    /// down z); each side takes `sides`, u along the edge and v across the prism.
    pub fn add_prism(
        &mut self,
        profile: &[[f32; 2]],
        x0: f32,
        x1: f32,
        ends: UvRect,
        sides: UvRect,
    ) {
        let n = profile.len();
        if n < 3 {
            return;
        }
        let c = [0, 1].map(|k| profile.iter().map(|p| p[k]).sum::<f32>() / n as f32);
        let (lo, hi) = (
            [0, 1].map(|k| profile.iter().map(|p| p[k]).fold(f32::INFINITY, f32::min)),
            [0, 1].map(|k| {
                profile
                    .iter()
                    .map(|p| p[k])
                    .fold(f32::NEG_INFINITY, f32::max)
            }),
        );
        let span = |v: f32, k: usize| {
            if hi[k] > lo[k] {
                (v - lo[k]) / (hi[k] - lo[k])
            } else {
                0.0
            }
        };
        let end_uv = |p: [f32; 2]| {
            [
                ends[0] + (ends[2] - ends[0]) * span(p[0], 0),
                ends[1] + (ends[3] - ends[1]) * (1.0 - span(p[1], 1)),
            ]
        };
        // The two ends.
        for (x, normal) in [(x1, [1.0, 0.0, 0.0]), (x0, [-1.0, 0.0, 0.0])] {
            let face: Vec<(Vec3, [f32; 2])> = profile
                .iter()
                .map(|&p| ([x, p[0], p[1]], end_uv(p)))
                .collect();
            self.add_face(&face, normal);
        }
        // The sides, one an edge.
        for i in 0..n {
            let (a, b) = (profile[i], profile[(i + 1) % n]);
            let edge = [b[0] - a[0], b[1] - a[1]];
            let mut normal = [0.0, edge[1], -edge[0]];
            let mid = [0.5 * (a[0] + b[0]) - c[0], 0.5 * (a[1] + b[1]) - c[1]];
            if normal[1] * mid[0] + normal[2] * mid[1] < 0.0 {
                normal = [0.0, -normal[1], -normal[2]];
            }
            let l = (normal[1] * normal[1] + normal[2] * normal[2]).sqrt();
            if l == 0.0 {
                continue;
            }
            let normal = normal.map(|v| v / l);
            let face = [
                ([x0, a[0], a[1]], [sides[0], sides[3]]),
                ([x0, b[0], b[1]], [sides[2], sides[3]]),
                ([x1, b[0], b[1]], [sides[2], sides[1]]),
                ([x1, a[0], a[1]], [sides[0], sides[1]]),
            ];
            self.add_face(&face, normal);
        }
    }

    /// Adds one flat convex face (its corners in order either way round) facing `normal`, as a fan of triangles wound clockwise
    /// seen from outside.
    fn add_face(&mut self, corners: &[(Vec3, [f32; 2])], normal: Vec3) {
        let base = self.positions.len() as u16;
        for &(p, uv) in corners {
            self.positions.push(p);
            self.normals.push(normal);
            self.tverts.push(uv);
        }
        let (a, b, c) = (corners[0].0, corners[1].0, corners[2].0);
        let (e1, e2) = (
            [0, 1, 2].map(|k| b[k] - a[k]),
            [0, 1, 2].map(|k| c[k] - a[k]),
        );
        let cross = [
            e1[1] * e2[2] - e1[2] * e2[1],
            e1[2] * e2[0] - e1[0] * e2[2],
            e1[0] * e2[1] - e1[1] * e2[0],
        ];
        let ccw = cross[0] * normal[0] + cross[1] * normal[1] + cross[2] * normal[2] > 0.0;
        for i in 1..corners.len() as u16 - 1 {
            if ccw {
                self.indices.extend([base, base + i + 1, base + i]);
            } else {
                self.indices.extend([base, base + i, base + i + 1]);
            }
        }
    }

    /// The box `min`..`max` as a collision hull: its eight corners and twelve triangles, no material.
    pub fn hull_box(min: Vec3, max: Vec3) -> MeshData {
        let mut m = MeshData::new(None);
        // Corner `i` has x from bit 0, y from bit 1, z from bit 2.
        for i in 0..8u16 {
            let d = [0, 1, 2].map(|k| if i & (1 << k) != 0 { 1.0 } else { -1.0 });
            m.positions.push(corner(min, max, d));
            let l = 3.0f32.sqrt();
            m.normals.push(d.map(|c| c / l));
            m.tverts.push([0.0, 0.0]);
        }
        for face in Face::ALL {
            let (n, u, v) = face.frame();
            let at = |su: f32, sv: f32| -> u16 {
                let d = [0, 1, 2].map(|k| n[k] + su * u[k] + sv * v[k]);
                (0..3).map(|k| u16::from(d[k] > 0.0) << k).sum()
            };
            let q = [at(-1.0, -1.0), at(1.0, -1.0), at(1.0, 1.0), at(-1.0, 1.0)];
            m.indices.extend([q[0], q[2], q[1], q[0], q[3], q[2]]);
        }
        m
    }

    /// The mesh and its render arrays as the model holds them.
    fn into_parts(self) -> (Mesh, MeshRender) {
        let c = bounds::compute(&self.positions);
        let n = self.positions.len();
        let material = match self.material {
            Some(m) => PRIM_INDEXED | m,
            None => PRIM_INDEXED | PRIM_NO_MATERIAL,
        };
        let mesh = Mesh {
            kind: MeshKind::Standard,
            parent: -1,
            num_frames: 1,
            num_mat_frames: 1,
            verts_per_frame: n as i32,
            flags: 0,
            bounds: c.bounds,
            center: c.center,
            // Torque's writer stores the radius as the integer `(S32)mRadius`.
            radius: c.radius as i32,
            num_verts: n,
            num_tverts: self.tverts.len(),
            positions: Arc::from(self.positions),
            primitives: vec![Primitive {
                start: 0,
                num_elements: self.indices.len() as u16,
                material,
            }],
            indices: self.indices,
        };
        let render = MeshRender {
            tverts: Arc::from(self.tverts),
            normals: Arc::from(self.normals),
            encoded_normals: Arc::from(vec![0u8; n]),
            merge_indices: Vec::new(),
            extra: Default::default(),
        };
        (mesh, render)
    }
}

/// The keyframes of one node in a sequence.
#[derive(Debug, Clone, PartialEq)]
pub struct Track<T> {
    pub node: usize,
    pub keys: Vec<T>,
}

/// A sequence: `keyframes` evenly spaced keys over `duration` seconds for each track (a missing key holds the track's last
/// one), and the shape's travel at the end of each ground frame (none: the sequence does not move the shape).
#[derive(Debug, Clone, PartialEq)]
pub struct SequenceDef {
    pub name: &'static str,
    pub duration: f32,
    pub cyclic: bool,
    pub priority: i32,
    pub keyframes: usize,
    pub rotations: Vec<Track<Quat16>>,
    pub translations: Vec<Track<Vec3>>,
    pub ground: Vec<Vec3>,
}

#[derive(Debug, Clone)]
struct NodeDef {
    name: i32,
    parent: Option<usize>,
    translation: Vec3,
    rotation: Quat16,
}

#[derive(Debug, Clone)]
struct ObjectDef {
    name: i32,
    node: usize,
    detail: usize,
    mesh: MeshData,
}

/// A shape being built (module docs).
#[derive(Debug, Clone, Default)]
pub struct ShapeBuilder {
    names: Vec<Vec<u8>>,
    nodes: Vec<NodeDef>,
    objects: Vec<ObjectDef>,
    details: Vec<(i32, f32)>,
    materials: Vec<RawMaterial>,
    sequences: Vec<SequenceDef>,
}

impl ShapeBuilder {
    pub fn new() -> ShapeBuilder {
        ShapeBuilder::default()
    }

    /// The name table's index of `name`, added once.
    fn name(&mut self, name: &str) -> i32 {
        let bytes = name.as_bytes();
        let at = match self.names.iter().position(|n| n == bytes) {
            Some(i) => i,
            None => {
                self.names.push(bytes.to_vec());
                self.names.len() - 1
            }
        };
        at as i32
    }

    /// A node at `translation` from its parent (or the shape's origin), not turned at rest; returns its index.
    pub fn node(&mut self, name: &str, parent: Option<usize>, translation: Vec3) -> usize {
        self.node_turned(name, parent, translation, NO_TURN)
    }

    /// A node at `translation` from its parent, turned by `rotation` at rest ([`turn`]); returns its index.
    pub fn node_turned(
        &mut self,
        name: &str,
        parent: Option<usize>,
        translation: Vec3,
        rotation: Quat16,
    ) -> usize {
        let name = self.name(name);
        self.nodes.push(NodeDef {
            name,
            parent,
            translation,
            rotation,
        });
        self.nodes.len() - 1
    }

    /// A detail level; `size` is the pixel size it is drawn down to (negative: never drawn, a collision or line-of-sight
    /// detail). Returns its index.
    pub fn detail(&mut self, name: &str, size: f32) -> usize {
        let name = self.name(name);
        self.details.push((name, size));
        self.details.len() - 1
    }

    /// A material of the list: the texture's name without its extension (`grid` is drawn with `grid.png` beside the shape).
    pub fn material(&mut self, name: &str, flags: u32) -> u32 {
        self.materials.push(RawMaterial {
            name: name.as_bytes().to_vec(),
            flags,
            reflectance_map: NO_MAP,
            bump_map: NO_MAP,
            detail_map: NO_MAP,
            detail_scale: 1.0,
            reflection_amount: 1.0,
        });
        (self.materials.len() - 1) as u32
    }

    /// An object on `node` whose mesh in detail `detail` is `mesh` (it has none in the other details).
    pub fn object(&mut self, name: &str, node: usize, detail: usize, mesh: MeshData) {
        let name = self.name(name);
        self.objects.push(ObjectDef {
            name,
            node,
            detail,
            mesh,
        });
    }

    pub fn sequence(&mut self, def: SequenceDef) {
        self.name(def.name);
        self.sequences.push(def);
    }

    /// The model (module docs).
    pub fn build(mut self) -> Model {
        let mut s = Shape::empty();
        let parent_of = |n: &NodeDef| n.parent.map_or(-1, |p| p as i32);
        for (i, n) in self.nodes.iter().enumerate() {
            let first_object = self.objects.iter().position(|o| o.node == i);
            let first_child = self.nodes.iter().position(|c| c.parent == Some(i));
            // `TSShape::init` links only the children of a parent; the roots stay unlinked.
            let next_sibling = n.parent.and_then(|p| {
                self.nodes
                    .iter()
                    .enumerate()
                    .skip(i + 1)
                    .find(|(_, c)| c.parent == Some(p))
                    .map(|(j, _)| j)
            });
            s.nodes.push(Node {
                name: n.name,
                parent: parent_of(n),
                first_object: first_object.map_or(-1, |j| j as i32),
                first_child: first_child.map_or(-1, |j| j as i32),
                next_sibling: next_sibling.map_or(-1, |j| j as i32),
            });
            s.default_rotations.push(n.rotation);
            s.default_translations.push(n.translation);
        }

        let detail_count = self.details.len();
        let mut polys = vec![0i32; detail_count];
        let mut render = Vec::new();
        let objects = std::mem::take(&mut self.objects);
        for (i, o) in objects.iter().enumerate() {
            let next_sibling = objects
                .iter()
                .enumerate()
                .skip(i + 1)
                .find(|(_, p)| p.node == o.node)
                .map_or(-1, |(j, _)| j as i32);
            s.objects.push(Object {
                name: o.name,
                num_meshes: detail_count as i32,
                start_mesh_index: (i * detail_count) as i32,
                node: o.node as i32,
                next_sibling,
                first_decal: -1,
            });
            s.object_states.push(ObjectState {
                vis: 1.0,
                frame: 0,
                mat_frame: 0,
            });
        }
        for o in objects {
            let mut mesh = Some(o.mesh);
            for (d, count) in polys.iter_mut().enumerate() {
                if d == o.detail
                    && let Some(m) = mesh.take()
                {
                    *count += m.triangles() as i32;
                    let (m, r) = m.into_parts();
                    s.meshes.push(Some(m));
                    render.push(Some(r));
                } else {
                    s.meshes.push(None);
                    render.push(None);
                }
            }
        }
        s.sub_shapes.push(SubShape {
            first_node: 0,
            first_object: 0,
            first_decal: 0,
            num_nodes: s.nodes.len() as i32,
            num_objects: s.objects.len() as i32,
            num_decals: 0,
        });

        s.smallest_visible_dl = -1;
        let mut smallest = f32::MAX;
        for (i, &(name, size)) in self.details.iter().enumerate() {
            s.details.push(Detail {
                name,
                sub_shape: 0,
                object_detail: i as i32,
                size,
                average_error: -1.0,
                max_error: -1.0,
                poly_count: polys[i],
            });
            if size >= 0.0 && size < smallest {
                smallest = size;
                s.smallest_visible_dl = i as i32;
                s.smallest_visible_size = size as i32;
            }
        }

        for def in std::mem::take(&mut self.sequences) {
            let name = self.name(def.name);
            let seq = sequence(&mut s, name, def);
            s.sequences.push(seq);
        }

        s.names = self.names;
        s.materials = self
            .materials
            .iter()
            .map(|m| Material {
                name: Some(m.name.clone()),
                flags: m.flags,
            })
            .collect();
        let mut free = |_: u64| true;
        s.bounds = bounds::detail_bounds(&s, 0, &mut free)
            .ok()
            .flatten()
            .unwrap_or([0.0; 6]);
        let (center, radius, tube) = bounds::shape_derived(&s.bounds);
        s.center = center;
        s.radius = radius;
        s.tube_radius = tube;

        Model {
            shape: s,
            render,
            file: FileExtras {
                materials: RawMaterials::Complete(self.materials),
                ..FileExtras::default()
            },
        }
    }
}

/// The keys of `tracks` laid out as `TSShape` keeps them: the tracks in node order, each `keyframes` long (a short track holds
/// its last key), appended to `out`; returns the base index and the membership set.
fn lay_out<T: Copy>(
    out: &mut Vec<T>,
    mut tracks: Vec<Track<T>>,
    keyframes: usize,
    rest: T,
) -> (i32, torque_shapes::shape::IntSet) {
    let base = out.len() as i32;
    let mut set = torque_shapes::shape::IntSet::default();
    tracks.sort_by_key(|t| t.node);
    for t in tracks {
        set.set(t.node);
        let last = t.keys.last().copied().unwrap_or(rest);
        out.extend((0..keyframes).map(|k| t.keys.get(k).copied().unwrap_or(last)));
    }
    (base, set)
}

fn sequence(s: &mut Shape, name: i32, def: SequenceDef) -> Sequence {
    let k = def.keyframes;
    let (base_rotation, rotation_matters) =
        lay_out(&mut s.node_rotations, def.rotations, k, NO_TURN);
    let (base_translation, translation_matters) =
        lay_out(&mut s.node_translations, def.translations, k, [0.0; 3]);
    let first_ground_frame = s.ground_translations.len() as i32;
    s.ground_translations.extend(&def.ground);
    s.ground_rotations
        .extend(std::iter::repeat_n(NO_TURN, def.ground.len()));
    Sequence {
        name,
        flags: if def.cyclic { SEQ_CYCLIC } else { 0 },
        num_keyframes: k as i32,
        duration: def.duration,
        priority: def.priority,
        first_ground_frame,
        num_ground_frames: def.ground.len() as i32,
        base_rotation,
        base_translation,
        base_scale: 0,
        // The objects' default states come first; no sequence here animates an object's state.
        base_object_state: s.objects.len() as i32,
        base_decal_state: 0,
        first_trigger: 0,
        num_triggers: 0,
        tool_begin: 0.0,
        rotation_matters,
        translation_matters,
        scale_matters: Default::default(),
        decal_matters: Default::default(),
        ifl_matters: Default::default(),
        vis_matters: Default::default(),
        frame_matters: Default::default(),
        mat_frame_matters: Default::default(),
    }
}
