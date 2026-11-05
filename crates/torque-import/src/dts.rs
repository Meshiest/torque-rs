//! The model layer as a Torque shape ([`to_dts`]) and a shape as the model layer ([`from_dts`]).
//!
//! **Into a shape.** Nodes become shape nodes with their rest turn and move; a shape node has no rest scale, so what scale
//! (and, under a non-uniform scale, shear) a node's rest transform has beyond its turn and move is carried into its meshes'
//! vertices and its children's moves: exact at rest. An animation that turns a node under a non-uniform scale, or scales
//! one, is approximated (the shape scales along the node's own axes). Meshes become objects (a mesh's
//! name and node, its meshes in each detail); a skinned mesh becomes a skin object whose bones' initial transforms put its
//! vertices where the source does at rest. A mesh over 65,535 vertices or indices (a shape mesh's limits) is cut into objects
//! of its own (`name`, `name_2`, ...). Animations become sequences with the same keys; scale keys become aligned scale keys,
//! relative to the node's rest scale.
//!
//! **Out of a shape.** Standard and skin meshes, triangle lists, strips and fans; the sequences' rotation, translation, uniform
//! and aligned scale keys and ground frames. Decal and sorted meshes, arbitrary scales, visibility, IFL frames and triggers are
//! left out with a note.

use std::collections::HashMap;

use torque_modelcore::math::{self, Mat4, Quat, Vec3};
use torque_modelcore::naming::{self, Role};
use torque_modelcore::{
    Alpha, Animation, Detail, Image, Material, Mesh, Model, Node, Primitive, Skin, Track,
};
use torque_shape_builder::{
    MeshData, SequenceSpec, ShapeBuilder, SkinDef, Track as ShapeTrack, material_flags,
};
use torque_shapes::shape::{MeshExtra, MeshKind, Quat16, RawMaterial, RawMaterials};

use crate::{ImportError, cap};

/// A shape material's flag: drawn blended by its alpha (`TSMaterialList::Translucent`).
pub const TRANSLUCENT: u32 = 0x4;
const NO_MAP: u32 = u32::MAX;
const PRIM_TYPE: u32 = 0xC000_0000;
const PRIM_STRIP: u32 = 0x4000_0000;
const PRIM_FAN: u32 = 0x8000_0000;
const PRIM_NO_MATERIAL: u32 = 0x1000_0000;
const PRIM_MATERIAL: u32 = 0x0FFF_FFFF;
const SEQ_UNIFORM_SCALE: u32 = 1;
const SEQ_ALIGNED_SCALE: u32 = 2;
const SEQ_ARBITRARY_SCALE: u32 = 4;
const SEQ_BLEND: u32 = 8;
const SEQ_CYCLIC: u32 = 16;
/// The most vertices, and indices, of one shape mesh.
const MESH_LIMIT: usize = 65_535;

/// A rotation as a shape stores it: Torque's quaternion-to-matrix routine writes the transpose, so the stored quaternion is
/// the conjugate, in shorts over 32767.
pub fn quat16(q: Quat) -> Quat16 {
    let s = |c: f32| (c * 32767.0).round().clamp(-32767.0, 32767.0) as i16;
    [s(-q[0]), s(-q[1]), s(-q[2]), s(q[3])]
}

/// A stored rotation back as a quaternion of the usual convention.
pub fn quat_of16(q: Quat16) -> Quat {
    [
        -f32::from(q[0]) / 32767.0,
        -f32::from(q[1]) / 32767.0,
        -f32::from(q[2]) / 32767.0,
        f32::from(q[3]) / 32767.0,
    ]
}

/// The flags a shape's material takes from a material that does not state them.
fn flags_of(m: &Material) -> u32 {
    let mut f = material_flags::NEVER_ENV_MAP;
    if m.wrap[0] {
        f |= material_flags::S_WRAP;
    }
    if m.wrap[1] {
        f |= material_flags::T_WRAP;
    }
    if m.alpha == Alpha::Blend {
        f |= TRANSLUCENT;
    }
    if m.unlit {
        f |= material_flags::SELF_ILLUMINATING;
    }
    f
}

/// A diagonal matrix.
fn diagonal(s: Vec3) -> Mat4 {
    let mut m = math::IDENTITY;
    m[0] = s[0];
    m[5] = s[1];
    m[10] = s[2];
    m
}

/// The factor of a matrix that is that many times the identity (a uniform scale), if it is one.
fn uniform(m: &Mat4) -> Option<f32> {
    let k = m[0];
    let off = [1, 2, 4, 6, 8, 9].iter().all(|&i| m[i] == 0.0);
    (off && m[5] == k && m[10] == k).then_some(k)
}

/// What each node's rest transform has beyond its (stored) turn and move, as a linear map of its own space: None for none.
/// A shape node is its parent's frame moved and turned; the source's node is that frame times this map
/// (`W = F * L`, with `L = R^T * L_parent * R * S`).
fn residuals(model: &Model) -> Vec<Option<Mat4>> {
    let mut out: Vec<Option<Option<Mat4>>> = vec![None; model.nodes.len()];
    for i in 0..model.nodes.len() {
        let mut chain = vec![i];
        let mut at = model.nodes[i].parent;
        while let Some(p) = at {
            if out[p].is_some() || chain.len() > model.nodes.len() {
                break;
            }
            chain.push(p);
            at = model.nodes[p].parent;
        }
        let mut parent = at.and_then(|p| out[p]).flatten();
        for &c in chain.iter().rev() {
            if let Some(done) = out[c] {
                parent = done;
                continue;
            }
            let n = &model.nodes[c];
            let l = match parent {
                None if n.scale == [1.0; 3] => None,
                None => Some(diagonal(n.scale)),
                Some(p) => match uniform(&p) {
                    Some(k) => Some(diagonal(n.scale.map(|s| s * k))),
                    None => {
                        let r = math::rotation(quat_of16(quat16(n.rotation)));
                        let rt = math::inverse(&r);
                        Some(math::mul(
                            &math::mul(&math::mul(&rt, &p), &r),
                            &diagonal(n.scale),
                        ))
                    }
                },
            };
            out[c] = Some(l);
            parent = l;
        }
    }
    out.into_iter().map(Option::flatten).collect()
}

/// `v` through the residual map `l` (unchanged for none).
fn through(l: Option<&Mat4>, v: Vec3) -> Vec3 {
    l.map_or(v, |l| math::vector(l, v))
}

/// An object's meshes: each its detail, its data and its bones (a skin mesh).
type ObjectMeshes = Vec<(usize, MeshData, Option<SkinDef>)>;

/// One shape mesh's worth of a mesh: the source vertices it keeps, and its triangles as (material, local indices) runs.
struct Chunk {
    verts: Vec<u32>,
    runs: Vec<(Option<usize>, Vec<u16>)>,
}

/// A mesh cut into shape meshes; one chunk keeping every vertex in order when it fits.
fn chunks(m: &Mesh) -> Vec<Chunk> {
    let total: usize = m.primitives.iter().map(|p| p.indices.len()).sum();
    if m.positions.len() <= MESH_LIMIT && total <= MESH_LIMIT {
        return vec![Chunk {
            verts: (0..m.positions.len() as u32).collect(),
            runs: m
                .primitives
                .iter()
                .map(|p| (p.material, p.indices.iter().map(|&i| i as u16).collect()))
                .collect(),
        }];
    }
    let mut out = Vec::new();
    let mut cur = Chunk {
        verts: Vec::new(),
        runs: Vec::new(),
    };
    let mut local: HashMap<u32, u16> = HashMap::new();
    let mut indices = 0usize;
    for p in &m.primitives {
        for t in p.indices.chunks_exact(3) {
            let new = t.iter().filter(|v| !local.contains_key(v)).count();
            if cur.verts.len() + new > MESH_LIMIT || indices + 3 > MESH_LIMIT {
                out.push(std::mem::replace(
                    &mut cur,
                    Chunk {
                        verts: Vec::new(),
                        runs: Vec::new(),
                    },
                ));
                local.clear();
                indices = 0;
            }
            if cur.runs.last().is_none_or(|r| r.0 != p.material) {
                cur.runs.push((p.material, Vec::new()));
            }
            for &v in t {
                let at = *local.entry(v).or_insert_with(|| {
                    cur.verts.push(v);
                    (cur.verts.len() - 1) as u16
                });
                if let Some(run) = cur.runs.last_mut() {
                    run.1.push(at);
                }
            }
            indices += 3;
        }
    }
    if !cur.runs.is_empty() {
        out.push(cur);
    }
    out
}

/// The model as a shape (module docs).
pub fn to_dts(model: &Model) -> Result<torque_shapes::Model, ImportError> {
    model.check()?;
    cap("nodes", model.nodes.len(), torque_shapes::MAX_SHAPE_NODES)?;
    cap(
        "materials",
        model.materials.len(),
        torque_shapes::MAX_MATERIALS,
    )?;
    cap(
        "animations",
        model.animations.len(),
        torque_shapes::MAX_SEQUENCES,
    )?;
    let mut notes: Vec<String> = Vec::new();
    let mut details = model.details.clone();
    // Meshes with no detail go to one by their role: collision, line of sight or the first drawn detail.
    let mut detail_of = Vec::with_capacity(model.meshes.len());
    for m in &model.meshes {
        let d = match m.detail {
            Some(d) => d,
            None => {
                let (name, size) = match naming::role(&m.name) {
                    Role::Collision => ("Collision-1".to_string(), -1.0),
                    Role::Los => ("LOS-9".to_string(), -1.0),
                    _ => details
                        .iter()
                        .find(|d| d.size >= 0.0)
                        .map_or(("detail2".to_string(), 2.0), |d| (d.name.clone(), d.size)),
                };
                details
                    .iter()
                    .position(|d| d.name == name)
                    .unwrap_or_else(|| {
                        details.push(Detail { name, size });
                        details.len() - 1
                    })
            }
        };
        detail_of.push(d);
    }
    cap("details", details.len(), torque_shapes::MAX_SHAPE_DETAILS)?;

    let residual = residuals(model);
    let parent_map = |n: &Node| n.parent.and_then(|p| residual[p].as_ref());
    let mut b = ShapeBuilder::new();
    let mut shape_local: Vec<Mat4> = Vec::with_capacity(model.nodes.len());
    for n in &model.nodes {
        let t = through(parent_map(n), n.translation);
        let q = quat16(n.rotation);
        b.node_turned(&n.name, n.parent, t, q);
        shape_local.push(math::trs(t, quat_of16(q), [1.0; 3]));
    }
    for d in &details {
        b.detail(&d.name, d.size);
    }
    for m in &model.materials {
        b.raw_material(RawMaterial {
            name: m.name.as_bytes().to_vec(),
            flags: m.flags.unwrap_or_else(|| flags_of(m)),
            reflectance_map: NO_MAP,
            bump_map: NO_MAP,
            detail_map: NO_MAP,
            detail_scale: 1.0,
            reflection_amount: 1.0,
        });
    }

    // Skins: each bone's initial transform takes a vertex from the source's bind space to where the shape's rest pose
    // (no scale, quantized turns) must see it for the source's rest pose to come out.
    let source_world = model.rest_world();
    let shape_world = model.world_of(&shape_local);

    // Objects: meshes of one name on one node (or one skin's) share an object, in the order they first appear.
    let mut objects: Vec<(String, Option<usize>, ObjectMeshes)> = Vec::new();
    let mut object_of: Vec<Option<usize>> = vec![None; model.meshes.len()];
    for (mi, m) in model.meshes.iter().enumerate() {
        let node = if m.skin.is_some() { None } else { Some(m.node) };
        let l = if m.skin.is_some() {
            None
        } else {
            residual[m.node].as_ref()
        };
        // Normals go through the inverse transpose; a mirror turns the triangles round.
        let normal_map = l.map(|l| {
            let inv = math::inverse(l);
            std::array::from_fn::<f32, 16, _>(|i| inv[(i % 4) * 4 + i / 4])
        });
        let mirrored = l.is_some_and(|l| {
            l[0] * (l[5] * l[10] - l[6] * l[9]) - l[1] * (l[4] * l[10] - l[6] * l[8])
                + l[2] * (l[4] * l[9] - l[5] * l[8])
                < 0.0
        });
        let parts = chunks(m);
        let split = parts.len() > 1;
        if split {
            notes.push(format!("{}: cut into {} shape meshes", m.name, parts.len()));
        }
        for (ci, c) in parts.into_iter().enumerate() {
            let mut data = MeshData::new(c.runs.first().and_then(|r| r.0).map(|x| x as u32));
            data.positions = c
                .verts
                .iter()
                .map(|&v| through(l, m.positions[v as usize]))
                .collect();
            data.normals = c
                .verts
                .iter()
                .map(|&v| {
                    let n = m
                        .normals
                        .get(v as usize)
                        .copied()
                        .unwrap_or([0.0, 0.0, 1.0]);
                    let Some(nm) = &normal_map else {
                        return n;
                    };
                    let k = math::vector(nm, n);
                    let len = (k[0] * k[0] + k[1] * k[1] + k[2] * k[2]).sqrt();
                    if len > 1e-20 { k.map(|x| x / len) } else { n }
                })
                .collect();
            data.tverts = c
                .verts
                .iter()
                .map(|&v| m.uv0.get(v as usize).copied().unwrap_or([0.0; 2]))
                .collect();
            for (_, run) in &c.runs {
                for t in run.chunks_exact(3) {
                    if mirrored {
                        data.indices.extend([t[0], t[2], t[1]]);
                    } else {
                        data.indices.extend_from_slice(t);
                    }
                }
            }
            if c.runs.len() > 1 {
                data.groups = c
                    .runs
                    .iter()
                    .map(|r| (r.1.len(), r.0.map(|x| x as u32)))
                    .collect();
            }
            let skin = m.skin.as_ref().map(|sk| {
                let bones = sk
                    .joints
                    .iter()
                    .zip(&sk.inverse_binds)
                    .map(|(&j, ib)| {
                        let init = math::mul(
                            &math::mul(&math::inverse(&shape_world[j]), &source_world[j]),
                            ib,
                        );
                        (j, init)
                    })
                    .collect();
                let mut weights = Vec::new();
                for (local, &v) in c.verts.iter().enumerate() {
                    let (js, ws) = (m.joints[v as usize], m.weights[v as usize]);
                    for k in 0..4 {
                        if ws[k] > 0.0 {
                            weights.push((local as u32, u32::from(js[k]), ws[k]));
                        }
                    }
                }
                SkinDef { bones, weights }
            });
            let name = if ci == 0 {
                m.name.clone()
            } else {
                format!("{}_{}", m.name, ci + 1)
            };
            let entry = (detail_of[mi], data, skin);
            // Another level of an earlier mesh joins its object, unless either was cut (or the detail is taken).
            let found = m
                .lod_of
                .filter(|_| !split)
                .and_then(|o| object_of[o])
                .filter(|&o| !objects[o].2.iter().any(|e| e.0 == entry.0));
            match found {
                Some(o) => objects[o].2.push(entry),
                None => {
                    if ci == 0 && !split {
                        object_of[mi] = Some(objects.len());
                    }
                    objects.push((name, node, vec![entry]));
                }
            }
        }
    }
    cap("objects", objects.len(), torque_shapes::MAX_SHAPE_OBJECTS)?;
    // A shape keeps a mesh slot for every object in every detail.
    cap(
        "mesh slots",
        objects.len().saturating_mul(details.len()),
        torque_shapes::MAX_SHAPE_MESHES,
    )?;
    for (name, node, meshes) in objects {
        b.object_meshes(&name, node, meshes);
    }

    for a in &model.animations {
        let mut spec = SequenceSpec {
            name: a.name.clone(),
            duration: a.duration,
            cyclic: a.cyclic,
            blend: a.blend,
            priority: a.priority,
            keyframes: a.keys,
            ground: a.ground.clone(),
            ..SequenceSpec::default()
        };
        for t in &a.tracks {
            let n = &model.nodes[t.node];
            if !t.rotations.is_empty() {
                spec.rotations.push(ShapeTrack {
                    node: t.node,
                    keys: t.rotations.iter().map(|&q| quat16(q)).collect(),
                });
            }
            if !t.translations.is_empty() {
                let pm = parent_map(n);
                spec.translations.push(ShapeTrack {
                    node: t.node,
                    keys: t.translations.iter().map(|&v| through(pm, v)).collect(),
                });
            }
            if !t.scales.is_empty() {
                let rel = |v: Vec3| {
                    std::array::from_fn(|i| {
                        if n.scale[i] != 0.0 {
                            v[i] / n.scale[i]
                        } else {
                            v[i]
                        }
                    })
                };
                spec.scales.push(ShapeTrack {
                    node: t.node,
                    keys: t.scales.iter().map(|&v| rel(v)).collect(),
                });
            }
        }
        b.add_sequence(spec);
    }
    let _ = notes;
    Ok(b.build())
}

fn name_of(shape: &torque_shapes::Shape, index: i32) -> String {
    usize::try_from(index)
        .ok()
        .and_then(|i| shape.names.get(i))
        .map(|n| String::from_utf8_lossy(n).into_owned())
        .unwrap_or_default()
}

/// The triangles of a shape primitive (triangle list, strip or fan), fronts as the shape draws them.
fn primitive_triangles(kind: u32, idx: &[u16]) -> Vec<u32> {
    let i = |k: usize| u32::from(idx[k]);
    match kind {
        PRIM_STRIP => (2..idx.len())
            .map(|k| {
                if k.is_multiple_of(2) {
                    [i(k - 2), i(k - 1), i(k)]
                } else {
                    [i(k - 1), i(k - 2), i(k)]
                }
            })
            .filter(|t| t[0] != t[1] && t[1] != t[2] && t[0] != t[2])
            .flatten()
            .collect(),
        PRIM_FAN => (2..idx.len())
            .flat_map(|k| [i(0), i(k - 1), i(k)])
            .collect(),
        _ => idx[..idx.len() / 3 * 3]
            .iter()
            .map(|&v| u32::from(v))
            .collect(),
    }
}

/// A shape as the model layer (module docs). The model's notes list what was left out.
pub fn from_dts(dts: &torque_shapes::Model) -> Result<Model, ImportError> {
    let s = &dts.shape;
    let bad = |what: &str| ImportError::Invalid(format!("shape {what}"));
    let mut model = Model {
        source: torque_modelcore::Source {
            format: "DTS".into(),
            up: 'z',
            scale: 1.0,
        },
        ..Model::default()
    };
    for (i, n) in s.nodes.iter().enumerate() {
        let parent = usize::try_from(n.parent)
            .ok()
            .filter(|&p| p < s.nodes.len());
        model.nodes.push(Node {
            name: name_of(s, n.name),
            parent,
            translation: *s
                .default_translations
                .get(i)
                .ok_or_else(|| bad("node translations"))?,
            rotation: quat_of16(
                *s.default_rotations
                    .get(i)
                    .ok_or_else(|| bad("node rotations"))?,
            ),
            scale: [1.0; 3],
        });
    }
    for d in &s.details {
        model.details.push(Detail {
            name: name_of(s, d.name),
            size: d.size,
        });
    }
    let listed: Vec<(String, u32)> = match &dts.file.materials {
        RawMaterials::Complete(list) => list
            .iter()
            .map(|m| (String::from_utf8_lossy(&m.name).into_owned(), m.flags))
            .collect(),
        _ => s
            .materials
            .iter()
            .map(|m| {
                (
                    m.name
                        .as_ref()
                        .map(|n| String::from_utf8_lossy(n).into_owned())
                        .unwrap_or_default(),
                    m.flags,
                )
            })
            .collect(),
    };
    for (name, flags) in listed {
        model.images.push(Image {
            name: name.clone(),
            uri: Some(format!("{name}.png")),
            ..Image::default()
        });
        model.materials.push(Material {
            name,
            base_texture: Some(model.images.len() - 1),
            alpha: if flags & TRANSLUCENT != 0 {
                Alpha::Blend
            } else {
                Alpha::Opaque
            },
            wrap: [
                flags & material_flags::S_WRAP != 0,
                flags & material_flags::T_WRAP != 0,
            ],
            unlit: flags & material_flags::SELF_ILLUMINATING != 0,
            metallic: 0.0,
            flags: Some(flags),
            ..Material::default()
        });
    }
    for o in &s.objects {
        let name = name_of(s, o.name);
        let first = model.meshes.len();
        for d in 0..usize::try_from(o.num_meshes).unwrap_or(0) {
            let index = usize::try_from(o.start_mesh_index)
                .unwrap_or(usize::MAX)
                .saturating_add(d);
            let (Some(Some(mesh)), Some(Some(render))) =
                (s.meshes.get(index), dts.render.get(index))
            else {
                continue;
            };
            let detail = s
                .details
                .iter()
                .position(|x| x.object_detail == d as i32)
                .unwrap_or(d);
            if !matches!(mesh.kind, MeshKind::Standard | MeshKind::Skin) {
                model
                    .notes
                    .push(format!("{name}: a {:?} mesh is left out", mesh.kind));
                continue;
            }
            let mut out = Mesh {
                name: name.clone(),
                node: usize::try_from(o.node)
                    .ok()
                    .filter(|&n| n < s.nodes.len())
                    .unwrap_or(0),
                detail: Some(detail),
                lod_of: (model.meshes.len() > first).then_some(first),
                ..Mesh::default()
            };
            match &render.extra {
                MeshExtra::Skin(sk) => {
                    let n = sk.num_initial_verts.min(sk.initial_verts.len());
                    out.positions = sk.initial_verts[..n].to_vec();
                    out.normals = (0..n)
                        .map(|v| {
                            sk.initial_normals
                                .get(v)
                                .copied()
                                .unwrap_or([0.0, 0.0, 1.0])
                        })
                        .collect();
                    let bones = sk
                        .num_nodes
                        .min(sk.node_index.len())
                        .min(sk.num_transforms)
                        .min(sk.initial_transforms.len());
                    let joints: Vec<usize> = sk.node_index[..bones]
                        .iter()
                        .map(|&j| {
                            usize::try_from(j)
                                .ok()
                                .filter(|&j| j < s.nodes.len())
                                .ok_or_else(|| bad("skin bone"))
                        })
                        .collect::<Result<_, _>>()?;
                    let mut slots: Vec<Vec<(u16, f32)>> = vec![Vec::new(); n];
                    for w in 0..sk
                        .num_weights
                        .min(sk.vertex_index.len())
                        .min(sk.bone_index.len())
                        .min(sk.weight.len())
                    {
                        let (v, b) = (sk.vertex_index[w], sk.bone_index[w]);
                        let (Ok(v), Ok(b)) = (usize::try_from(v), u16::try_from(b)) else {
                            continue;
                        };
                        if v < n && usize::from(b) < bones {
                            slots[v].push((b, sk.weight[w]));
                        }
                    }
                    for list in &mut slots {
                        list.sort_by(|a, b| b.1.total_cmp(&a.1));
                        list.truncate(4);
                        let sum: f32 = list.iter().map(|x| x.1).sum();
                        let mut j = [0u16; 4];
                        let mut w = [0f32; 4];
                        for (k, &(b, x)) in list.iter().enumerate() {
                            j[k] = b;
                            w[k] = if sum > 0.0 { x / sum } else { 0.0 };
                        }
                        if sum <= 0.0 {
                            w[0] = 1.0;
                        }
                        out.joints.push(j);
                        out.weights.push(w);
                    }
                    out.skin = Some(Skin {
                        joints,
                        inverse_binds: sk.initial_transforms[..bones].to_vec(),
                    });
                }
                _ => {
                    let v = mesh.verts();
                    out.positions = v.to_vec();
                    out.normals = (0..v.len())
                        .map(|i| render.normals.get(i).copied().unwrap_or([0.0, 0.0, 1.0]))
                        .collect();
                }
            }
            let n = out.positions.len();
            out.uv0 = (0..n)
                .map(|i| render.tverts.get(i).copied().unwrap_or([0.0; 2]))
                .collect();
            if mesh.num_frames > 1 {
                model.notes.push(format!(
                    "{name}: only the first of {} frames is kept",
                    mesh.num_frames
                ));
            }
            for p in &mesh.primitives {
                let start = usize::from(p.start);
                let Some(idx) = mesh.indices.get(start..start + usize::from(p.num_elements)) else {
                    return Err(bad("primitive"));
                };
                let tris = primitive_triangles(p.material & PRIM_TYPE, idx);
                if tris.iter().any(|&v| v as usize >= n) {
                    return Err(bad("index"));
                }
                let material = (p.material & PRIM_NO_MATERIAL == 0)
                    .then_some((p.material & PRIM_MATERIAL) as usize)
                    .filter(|&m| m < model.materials.len());
                out.primitives.push(Primitive {
                    material,
                    indices: tris,
                });
            }
            model.meshes.push(out);
        }
    }
    let nodes = s.nodes.len();
    for q in &s.sequences {
        let keys = usize::try_from(q.num_keyframes).unwrap_or(0).max(1);
        cap("keys of a sequence", keys, crate::caps::MAX_KEYS)?;
        let mut a = Animation {
            name: name_of(s, q.name),
            duration: q.duration,
            keys,
            cyclic: q.flags & SEQ_CYCLIC != 0,
            blend: q.flags & SEQ_BLEND != 0,
            priority: q.priority,
            ..Animation::default()
        };
        let track = |a: &mut Animation, node: usize| -> usize {
            match a.tracks.iter().position(|t| t.node == node) {
                Some(t) => t,
                None => {
                    a.tracks.push(Track {
                        node,
                        ..Track::default()
                    });
                    a.tracks.len() - 1
                }
            }
        };
        let base = |b: i32, j: usize| usize::try_from(b).unwrap_or(0) + j * keys;
        for (j, n) in (0..nodes)
            .filter(|&n| q.rotation_matters.test(n))
            .enumerate()
        {
            let at = base(q.base_rotation, j);
            let keys16 = s
                .node_rotations
                .get(at..at + keys)
                .ok_or_else(|| bad("rotation keys"))?;
            let t = track(&mut a, n);
            a.tracks[t].rotations = keys16.iter().map(|&k| quat_of16(k)).collect();
        }
        for (j, n) in (0..nodes)
            .filter(|&n| q.translation_matters.test(n))
            .enumerate()
        {
            let at = base(q.base_translation, j);
            let v = s
                .node_translations
                .get(at..at + keys)
                .ok_or_else(|| bad("translation keys"))?;
            let t = track(&mut a, n);
            a.tracks[t].translations = v.to_vec();
        }
        for (j, n) in (0..nodes).filter(|&n| q.scale_matters.test(n)).enumerate() {
            let at = base(q.base_scale, j);
            let v: Vec<Vec3> = if q.flags & SEQ_UNIFORM_SCALE != 0 {
                let u = s
                    .node_uniform_scales
                    .get(at..at + keys)
                    .ok_or_else(|| bad("scale keys"))?;
                u.iter().map(|&x| [x; 3]).collect()
            } else if q.flags & SEQ_ALIGNED_SCALE != 0 {
                s.node_aligned_scales
                    .get(at..at + keys)
                    .ok_or_else(|| bad("scale keys"))?
                    .to_vec()
            } else {
                if q.flags & SEQ_ARBITRARY_SCALE != 0 {
                    model
                        .notes
                        .push(format!("{}: arbitrary scale keys are left out", a.name));
                }
                break;
            };
            let t = track(&mut a, n);
            a.tracks[t].scales = v;
        }
        a.tracks.sort_by_key(|t| t.node);
        let first = usize::try_from(q.first_ground_frame).unwrap_or(0);
        let count = usize::try_from(q.num_ground_frames).unwrap_or(0);
        a.ground = s
            .ground_translations
            .get(first..first + count)
            .ok_or_else(|| bad("ground frames"))?
            .to_vec();
        if q.num_triggers > 0 {
            model
                .notes
                .push(format!("{}: triggers are left out", a.name));
        }
        model.animations.push(a);
    }
    model.check()?;
    Ok(model)
}
