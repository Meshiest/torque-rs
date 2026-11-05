//! The format-neutral model layer: what a model is once a reader has read its file, in Torque's frame (x right, y forward, z
//! up, meters).
//!
//! A [`Model`] is a node tree with rest transforms; meshes (positions, normals, two UV sets, colors, up to four joints and
//! weights a vertex, primitives of one material each) placed on a node or bound to a skin; materials; detail levels; and
//! animations sampled at evenly spaced keys per node. Readers (`torque-import`: glTF, and more formats later) write it; the
//! shape writer (`torque-import`'s `dts` module) and the interior builder read it. Nothing here reads a file.
//!
//! **Conventions.** Quaternions are `[x, y, z, w]` in the usual convention ([`math`]); matrices are row-major with the
//! translation in the last column, as Torque's. A triangle's front is clockwise seen from outside, as a shape draws it.
//! [`naming`] says what a node's name means (detail markers, collision and line-of-sight meshes, hulls, mission volumes).
//!
//! **Checked, not trusted.** [`Model::check`] is what every consumer runs first: every index in range, every array the
//! length its count says, the node tree without a cycle. A reader's output passes it; a model built by hand may not.

pub mod math;
pub mod naming;

use math::{Mat4, Quat, Vec3};

/// A model (crate docs).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Model {
    pub nodes: Vec<Node>,
    pub meshes: Vec<Mesh>,
    pub details: Vec<Detail>,
    pub materials: Vec<Material>,
    pub images: Vec<Image>,
    pub animations: Vec<Animation>,
    pub source: Source,
    /// What the reader left out or changed, one line each (a morph target, a line primitive, a mesh split).
    pub notes: Vec<String>,
}

/// Where a model came from.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Source {
    /// The format (`glTF`, ...).
    pub format: String,
    /// The source's up axis before the reader turned it into Torque's (`'y'` or `'z'`).
    pub up: char,
    /// The scale the reader applied to reach meters.
    pub scale: f32,
}

/// A node of the tree.
#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub name: String,
    /// The parent node; None for a root.
    pub parent: Option<usize>,
    pub translation: Vec3,
    pub rotation: Quat,
    pub scale: Vec3,
}

impl Node {
    /// A node at rest at `translation`, not turned or scaled.
    pub fn new(name: &str, parent: Option<usize>, translation: Vec3) -> Node {
        Node {
            name: name.to_string(),
            parent,
            translation,
            rotation: math::NO_TURN,
            scale: [1.0; 3],
        }
    }

    /// The rest transform from the parent's space.
    pub fn local(&self) -> Mat4 {
        math::trs(self.translation, self.rotation, self.scale)
    }
}

/// A detail level: drawn down to `size` pixels, or never drawn (negative: a collision or line-of-sight level).
#[derive(Debug, Clone, PartialEq)]
pub struct Detail {
    pub name: String,
    pub size: f32,
}

/// One material's triangles of a mesh: three indices each, front clockwise seen from outside.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Primitive {
    /// The material, or None (a collision mesh).
    pub material: Option<usize>,
    pub indices: Vec<u32>,
}

/// The bones of a skinned mesh.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Skin {
    /// Each joint's node.
    pub joints: Vec<usize>,
    /// Each joint's inverse bind matrix: from the mesh's space to the joint's at rest.
    pub inverse_binds: Vec<Mat4>,
}

/// A mesh: vertex arrays (positions required; the others empty or one entry a vertex) and its primitives.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Mesh {
    pub name: String,
    /// The node it hangs on (for a skinned mesh, the node it was found on; its vertices are placed by the skin's joints).
    pub node: usize,
    /// Its detail level, or None when the reader left it to the writer.
    pub detail: Option<usize>,
    /// The mesh (an earlier one, itself not another's level) this one is another detail level of: the two are one object of
    /// a shape, drawn in their own details.
    pub lod_of: Option<usize>,
    pub positions: Vec<Vec3>,
    pub normals: Vec<Vec3>,
    pub uv0: Vec<[f32; 2]>,
    pub uv1: Vec<[f32; 2]>,
    pub colors: Vec<[f32; 4]>,
    /// Joint slots of the skin (indices into [`Skin::joints`]), four a vertex.
    pub joints: Vec<[u16; 4]>,
    pub weights: Vec<[f32; 4]>,
    pub skin: Option<Skin>,
    pub primitives: Vec<Primitive>,
}

impl Mesh {
    /// The triangles of every primitive.
    pub fn triangles(&self) -> usize {
        self.primitives.iter().map(|p| p.indices.len() / 3).sum()
    }
}

/// How a material's alpha is used.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub enum Alpha {
    #[default]
    Opaque,
    /// Cut out below the value.
    Mask(f32),
    Blend,
}

/// A material.
#[derive(Debug, Clone, PartialEq)]
pub struct Material {
    /// The name a shape's material list keeps: the base color texture's file name without its folder or extension when it
    /// has one, else the material's own name.
    pub name: String,
    pub base_color: [f32; 4],
    /// The base color texture ([`Model::images`]).
    pub base_texture: Option<usize>,
    pub alpha: Alpha,
    pub two_sided: bool,
    /// Whether the texture repeats across u and across v.
    pub wrap: [bool; 2],
    pub normal_texture: Option<usize>,
    pub metal_rough_texture: Option<usize>,
    pub emissive_texture: Option<usize>,
    pub emissive: Vec3,
    pub metallic: f32,
    pub roughness: f32,
    /// Lit by nothing (drawn at full brightness).
    pub unlit: bool,
    /// The shape material flags when the source states them (a file this crate's writer made).
    pub flags: Option<u32>,
}

impl Default for Material {
    fn default() -> Material {
        Material {
            name: String::new(),
            base_color: [1.0; 4],
            base_texture: None,
            alpha: Alpha::Opaque,
            two_sided: false,
            wrap: [true; 2],
            normal_texture: None,
            metal_rough_texture: None,
            emissive_texture: None,
            emissive: [0.0; 3],
            metallic: 1.0,
            roughness: 1.0,
            unlit: false,
            flags: None,
        }
    }
}

/// A texture image: a file beside the model (`uri`), or bytes the model carries (PNG or JPEG, never decoded here).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Image {
    /// The file name without its folder or extension (the texture's name in a shape's material list).
    pub name: String,
    /// The relative path the model names, if it is a file.
    pub uri: Option<String>,
    /// The bytes, if the model carries them.
    pub data: Option<Vec<u8>>,
    /// `image/png` or `image/jpeg` when known.
    pub mime: String,
    /// The size the header gives, when there are bytes.
    pub size: Option<(u32, u32)>,
}

/// One node's keys in an animation: each array empty (not animated) or [`Animation::keys`] long.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Track {
    pub node: usize,
    pub rotations: Vec<Quat>,
    pub translations: Vec<Vec3>,
    pub scales: Vec<Vec3>,
}

/// An animation sampled at evenly spaced keys: key `k` is at [`key_time`](Self::key_time)`(k)`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Animation {
    pub name: String,
    /// Seconds.
    pub duration: f32,
    pub keys: usize,
    /// Loops: the last key leads back to the first (key `k` at `k / keys` of the duration); otherwise the last key is at the end
    /// (`k / (keys - 1)`).
    pub cyclic: bool,
    /// Adds to whatever else plays (its keys are moves from the rest pose) rather than replacing it.
    pub blend: bool,
    pub priority: i32,
    pub tracks: Vec<Track>,
    /// The model's travel at each ground frame (none: the animation does not move the model).
    pub ground: Vec<Vec3>,
}

impl Animation {
    /// The time of key `k`.
    pub fn key_time(&self, k: usize) -> f32 {
        let spans = if self.cyclic {
            self.keys
        } else {
            self.keys.saturating_sub(1)
        };
        if spans == 0 {
            0.0
        } else {
            self.duration * k as f32 / spans as f32
        }
    }
}

/// Why a model fails [`Model::check`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invalid(pub String);

impl std::fmt::Display for Invalid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "invalid model: {}", self.0)
    }
}

impl std::error::Error for Invalid {}

fn bad<T>(what: String) -> Result<T, Invalid> {
    Err(Invalid(what))
}

impl Model {
    /// Every index in range, every array its count long, the node tree without a cycle, every number finite.
    pub fn check(&self) -> Result<(), Invalid> {
        let nodes = self.nodes.len();
        for (i, n) in self.nodes.iter().enumerate() {
            if n.parent.is_some_and(|p| p >= nodes) {
                return bad(format!("node {i}'s parent"));
            }
            let finite = n
                .translation
                .iter()
                .chain(&n.rotation)
                .chain(&n.scale)
                .all(|c| c.is_finite());
            if !finite {
                return bad(format!("node {i}'s transform"));
            }
        }
        // A walk up from every node ends within `nodes` steps unless there is a cycle.
        for i in 0..nodes {
            let mut at = self.nodes[i].parent;
            let mut steps = 0;
            while let Some(p) = at {
                steps += 1;
                if steps > nodes {
                    return bad(format!("node {i} is its own ancestor"));
                }
                at = self.nodes[p].parent;
            }
        }
        for (i, m) in self.meshes.iter().enumerate() {
            let n = m.positions.len();
            if m.node >= nodes || m.detail.is_some_and(|d| d >= self.details.len()) {
                return bad(format!("mesh {i}'s node or detail"));
            }
            if m.lod_of
                .is_some_and(|o| o >= i || self.meshes[o].lod_of.is_some())
            {
                return bad(format!("mesh {i}'s level"));
            }
            for (len, what) in [
                (m.normals.len(), "normals"),
                (m.uv0.len(), "uv0"),
                (m.uv1.len(), "uv1"),
                (m.colors.len(), "colors"),
                (m.joints.len(), "joints"),
                (m.weights.len(), "weights"),
            ] {
                if len != 0 && len != n {
                    return bad(format!("mesh {i}'s {what}"));
                }
            }
            if !m.positions.iter().flatten().all(|c| c.is_finite()) {
                return bad(format!("mesh {i}'s positions"));
            }
            for p in &m.primitives {
                if p.indices.len() % 3 != 0 || p.indices.iter().any(|&v| v as usize >= n) {
                    return bad(format!("mesh {i}'s indices"));
                }
                if p.material.is_some_and(|x| x >= self.materials.len()) {
                    return bad(format!("mesh {i}'s material"));
                }
            }
            if let Some(s) = &m.skin {
                if s.joints.len() != s.inverse_binds.len() || s.joints.iter().any(|&j| j >= nodes) {
                    return bad(format!("mesh {i}'s skin"));
                }
                if m.joints.len() != n || m.weights.len() != n {
                    return bad(format!("mesh {i}'s weights"));
                }
                if m.joints
                    .iter()
                    .flatten()
                    .any(|&j| usize::from(j) >= s.joints.len())
                {
                    return bad(format!("mesh {i}'s joint slots"));
                }
            }
        }
        for (i, m) in self.materials.iter().enumerate() {
            let texs = [
                m.base_texture,
                m.normal_texture,
                m.metal_rough_texture,
                m.emissive_texture,
            ];
            if texs.iter().flatten().any(|&t| t >= self.images.len()) {
                return bad(format!("material {i}'s textures"));
            }
        }
        for (i, a) in self.animations.iter().enumerate() {
            if !a.duration.is_finite() || a.duration < 0.0 || a.keys == 0 {
                return bad(format!("animation {i}'s duration or keys"));
            }
            for t in &a.tracks {
                let lens = [t.rotations.len(), t.translations.len(), t.scales.len()];
                if t.node >= nodes || lens.iter().any(|&l| l != 0 && l != a.keys) {
                    return bad(format!("animation {i}'s tracks"));
                }
            }
        }
        Ok(())
    }

    /// Every node's transform in the model's space for the local transforms `local` (one a node). Call after
    /// [`check`](Self::check).
    pub fn world_of(&self, local: &[Mat4]) -> Vec<Mat4> {
        let count = self.nodes.len();
        let mut world: Vec<Option<Mat4>> = vec![None; count];
        let parent = |i: usize| self.nodes.get(i).and_then(|n| n.parent);
        for i in 0..count {
            if world[i].is_some() {
                continue;
            }
            // The chain up to the first placed ancestor (a cycle ends the walk), then down again.
            let mut chain = vec![i];
            let mut at = parent(i);
            while let Some(p) = at {
                if world.get(p).is_none_or(|w| w.is_some()) || chain.len() > count {
                    break;
                }
                chain.push(p);
                at = parent(p);
            }
            let mut m = at
                .and_then(|p| world.get(p).copied().flatten())
                .unwrap_or(math::IDENTITY);
            for &c in chain.iter().rev() {
                m = math::mul(&m, local.get(c).unwrap_or(&math::IDENTITY));
                world[c] = Some(m);
            }
        }
        world
            .into_iter()
            .map(|m| m.unwrap_or(math::IDENTITY))
            .collect()
    }

    /// Every node's rest transform in the model's space.
    pub fn rest_world(&self) -> Vec<Mat4> {
        let local: Vec<Mat4> = self.nodes.iter().map(Node::local).collect();
        self.world_of(&local)
    }

    /// Every node's local transform at key `key` of animation `anim` (the rest transform where it has no keys).
    pub fn pose_local(&self, anim: &Animation, key: usize) -> Vec<Mat4> {
        let mut parts: Vec<(Vec3, Quat, Vec3)> = self
            .nodes
            .iter()
            .map(|n| (n.translation, n.rotation, n.scale))
            .collect();
        for t in &anim.tracks {
            let Some(p) = parts.get_mut(t.node) else {
                continue;
            };
            if let Some(&q) = t.rotations.get(key) {
                p.1 = q;
            }
            if let Some(&v) = t.translations.get(key) {
                p.0 = v;
            }
            if let Some(&s) = t.scales.get(key) {
                p.2 = s;
            }
        }
        parts
            .into_iter()
            .map(|(t, q, s)| math::trs(t, q, s))
            .collect()
    }

    /// The vertices of mesh `mesh` in the model's space at rest: placed by its node, or by its skin's joints. Call after
    /// [`check`](Self::check).
    pub fn rest_positions(&self, mesh: usize) -> Vec<Vec3> {
        let world = self.rest_world();
        self.placed_positions(mesh, &world)
    }

    /// The vertices of mesh `mesh` placed by the node transforms `world` (one a node, in the model's space).
    pub fn placed_positions(&self, mesh: usize, world: &[Mat4]) -> Vec<Vec3> {
        let Some(m) = self.meshes.get(mesh) else {
            return Vec::new();
        };
        match &m.skin {
            None => {
                let at = world.get(m.node).copied().unwrap_or(math::IDENTITY);
                m.positions.iter().map(|&p| math::point(&at, p)).collect()
            }
            Some(skin) => {
                let bones: Vec<Mat4> = skin
                    .joints
                    .iter()
                    .zip(&skin.inverse_binds)
                    .map(|(&j, ib)| math::mul(&world.get(j).copied().unwrap_or(math::IDENTITY), ib))
                    .collect();
                m.positions
                    .iter()
                    .enumerate()
                    .map(|(v, &p)| {
                        let mut out = [0.0f32; 3];
                        let (js, ws) = (m.joints.get(v), m.weights.get(v));
                        for k in 0..4 {
                            let (Some(j), Some(w)) = (js.map(|j| j[k]), ws.map(|w| w[k])) else {
                                continue;
                            };
                            let Some(b) = bones.get(usize::from(j)) else {
                                continue;
                            };
                            let q = math::point(b, p);
                            for c in 0..3 {
                                out[c] += q[c] * w;
                            }
                        }
                        out
                    })
                    .collect()
            }
        }
    }

    /// The node named `name` (case ignored).
    pub fn find_node(&self, name: &str) -> Option<usize> {
        self.nodes
            .iter()
            .position(|n| n.name.eq_ignore_ascii_case(name))
    }
}

#[cfg(test)]
mod tests;
