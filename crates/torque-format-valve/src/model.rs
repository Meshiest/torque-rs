//! A studio model's three files made into a format-neutral [`torque_modelcore::Model`]: its bones as nodes, its meshes skinned
//! to them, its materials, and its sequences sampled as animations.
//!
//! **Frame.** Source's model space is x forward, y left, z up, in inches; Torque's is x right, y forward, z up, in meters. The
//! model's root node does the turn (a quarter turn about z) and the scale ([`INCH`]), so the bones, the vertices and the
//! animations keep Source's numbers below it. Triangles keep their order (clockwise is the front in both).
//!
//! **Animations.** A sequence plays its grid's first animation; its frames become keys of every bone the animation's model
//! shares by name with this one (so the animations of an included model, `$includemodel`, apply too). A looping sequence's
//! last frame repeats its first and is left out of the keys. Delta sequences (layers added over another) are marked `blend`.

use std::collections::HashMap;

use torque_modelcore::math::{self, Mat4};
use torque_modelcore::{
    Alpha, Animation, Image, Material, Mesh, Model, Node, Primitive, Skin, Source, Track,
};

use crate::mdl::{Mdl, anim_flags};
use crate::read::{Error, Result, invalid};
use crate::vtx::Vtx;
use crate::vvd::Vvd;

/// Meters per Source unit (an inch).
pub const INCH: f32 = 0.0254;
/// The most material paths one model makes (a path for each of its materials in each of its texture folders).
pub const MAX_MATERIAL_PATHS: usize = 1 << 16;
/// The most keys sampled from one sequence.
pub const MAX_KEYS: usize = 4096;
/// The most bone samples (keys times bones) of a model's animations.
pub const MAX_SAMPLES: usize = 1 << 24;

/// A model and where its materials may be found.
#[derive(Debug, Clone, PartialEq)]
pub struct StudioModel {
    pub model: Model,
    /// Each material's `.vmt` paths to try, in order ([`Mdl::material_paths`]).
    pub material_paths: Vec<Vec<String>>,
    /// Each bone's node.
    pub bone_nodes: Vec<usize>,
}

/// What to assemble: the skin family and each body part's model (missing parts take their first).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Choice {
    pub skin: usize,
    pub body: Vec<usize>,
}

/// A 3 x 4 row-major matrix as a 4 x 4.
fn mat34(m: &[f32; 12]) -> Mat4 {
    [
        m[0], m[1], m[2], m[3], m[4], m[5], m[6], m[7], m[8], m[9], m[10], m[11], 0.0, 0.0, 0.0,
        1.0,
    ]
}

/// The model of a header, vertex and mesh file (module docs). The files must share the header's checksum.
pub fn assemble(mdl: &Mdl, vvd: &Vvd, vtx: &Vtx, choice: &Choice) -> Result<StudioModel> {
    if vvd.checksum != mdl.checksum || vtx.checksum != mdl.checksum {
        return invalid(
            "the model's .vvd or .vtx is not the one made with its .mdl (checksums differ)",
        );
    }
    let mut nodes = vec![Node {
        name: "source".into(),
        parent: None,
        translation: [0.0; 3],
        rotation: [
            0.0,
            0.0,
            std::f32::consts::FRAC_1_SQRT_2,
            std::f32::consts::FRAC_1_SQRT_2,
        ],
        scale: [INCH; 3],
    }];
    let mut bone_nodes = Vec::with_capacity(mdl.bones.len());
    for b in &mdl.bones {
        bone_nodes.push(nodes.len());
        nodes.push(Node {
            name: b.name.clone(),
            parent: Some(b.parent.map_or(0, |p| p + 1)),
            translation: b.pos,
            rotation: math::normalize_quat(b.quat),
            scale: [1.0; 3],
        });
    }
    let skin = Skin {
        joints: bone_nodes.clone(),
        inverse_binds: mdl.bones.iter().map(|b| mat34(&b.pose_to_bone)).collect(),
    };
    let skinned = !mdl.bones.is_empty();
    let family = mdl.skins.get(choice.skin).or(mdl.skins.first());
    let mut materials: Vec<Material> = Vec::new();
    let mut images: Vec<Image> = Vec::new();
    let mut material_paths = Vec::new();
    let mut by_texture: HashMap<usize, usize> = HashMap::new();
    let mut paths_made = 0usize;
    let mut meshes = Vec::new();
    for (p, part) in mdl.body_parts.iter().enumerate() {
        let pick = choice.body.get(p).copied().unwrap_or(0);
        let Some(sub) = part.models.get(pick) else {
            continue;
        };
        let Some(vtx_meshes) = vtx.body_parts.get(p).and_then(|m| m.get(pick)) else {
            continue;
        };
        for (k, m) in sub.meshes.iter().enumerate() {
            let Some(tris) = vtx_meshes.get(k) else {
                continue;
            };
            if tris.is_empty() {
                continue;
            }
            let slot = usize::try_from(m.material).unwrap_or(0);
            let texture = family
                .and_then(|f| f.get(slot))
                .map_or(slot, |&t| usize::from(t));
            if !by_texture.contains_key(&texture) {
                paths_made += mdl.texture_dirs.len().max(1);
                if paths_made > MAX_MATERIAL_PATHS {
                    return Err(Error::TooLarge {
                        what: "the model's material paths",
                        asked: paths_made as u64,
                        cap: MAX_MATERIAL_PATHS as u64,
                    });
                }
            }
            let material = *by_texture.entry(texture).or_insert_with(|| {
                let name = mdl
                    .textures
                    .get(texture)
                    .cloned()
                    .unwrap_or_else(|| format!("texture{texture}"));
                let paths = mdl.material_paths(texture);
                images.push(Image {
                    name: name.rsplit('/').next().unwrap_or(&name).to_string(),
                    uri: paths.first().cloned(),
                    ..Image::default()
                });
                materials.push(Material {
                    name: name.clone(),
                    base_texture: Some(images.len() - 1),
                    metallic: 0.0,
                    ..Material::default()
                });
                material_paths.push(paths);
                materials.len() - 1
            });
            let base = sub
                .vertex_start
                .checked_add(usize::try_from(m.vertex_offset).unwrap_or(usize::MAX))
                .ok_or(Error::Invalid("a mesh's vertices past the list".into()))?;
            let mut local: HashMap<u32, u32> = HashMap::new();
            let mut mesh = Mesh {
                name: format!("{}_{k}", sub.name),
                node: 0,
                skin: skinned.then(|| skin.clone()),
                ..Mesh::default()
            };
            let mut indices = Vec::with_capacity(tris.len());
            for &t in tris {
                let i = match local.get(&t) {
                    Some(&i) => i,
                    None => {
                        let v = base
                            .checked_add(t as usize)
                            .and_then(|at| vvd.vertices.get(at))
                            .ok_or(Error::Invalid("a triangle's vertex past the .vvd's".into()))?;
                        let i = mesh.positions.len() as u32;
                        mesh.positions.push(v.position);
                        mesh.normals.push(v.normal);
                        mesh.uv0.push(v.uv);
                        if skinned {
                            let mut joints = [0u16; 4];
                            let mut weights = [0f32; 4];
                            for s in 0..usize::from(v.bone_count.min(3)) {
                                let bone = usize::from(v.bones[s]);
                                if bone < mdl.bones.len() {
                                    joints[s] = bone as u16;
                                    weights[s] = v.weights[s];
                                }
                            }
                            if weights.iter().sum::<f32>() <= 0.0 {
                                weights[0] = 1.0;
                            }
                            mesh.joints.push(joints);
                            mesh.weights.push(weights);
                        }
                        local.insert(t, i);
                        i
                    }
                };
                indices.push(i);
            }
            mesh.primitives.push(Primitive {
                material: Some(material),
                indices,
            });
            meshes.push(mesh);
        }
    }
    let model = Model {
        nodes,
        meshes,
        details: Vec::new(),
        materials,
        images,
        animations: Vec::new(),
        source: Source {
            format: "mdl".into(),
            up: 'z',
            scale: INCH,
        },
        notes: Vec::new(),
    };
    model.check().map_err(|e| Error::Invalid(e.to_string()))?;
    Ok(StudioModel {
        model,
        material_paths,
        bone_nodes,
    })
}

impl StudioModel {
    /// Adds the sequences of `from` (this model's own header, or an included model's) that `keep` takes, as animations; `ani`
    /// is `from`'s `.ani` file when it has one. A sequence whose animation does not decode is skipped (its name in the model's
    /// notes).
    pub fn add_sequences(
        &mut self,
        mdl: &Mdl,
        from: &Mdl,
        ani: Option<&[u8]>,
        keep: &mut dyn FnMut(&crate::mdl::Sequence) -> bool,
    ) -> Result<()> {
        // Bones matched by name through a table (not a search of every bone for every bone).
        let by_name: HashMap<String, usize> = mdl
            .bones
            .iter()
            .enumerate()
            .rev()
            .map(|(i, b)| (b.name.to_ascii_lowercase(), i))
            .collect();
        let map: Vec<Option<usize>> = from
            .bones
            .iter()
            .map(|b| {
                by_name
                    .get(&b.name.to_ascii_lowercase())
                    .map(|&i| self.bone_nodes[i])
            })
            .collect();
        let mut names: std::collections::HashSet<String> = self
            .model
            .animations
            .iter()
            .map(|a| a.name.to_ascii_lowercase())
            .collect();
        let mut samples: usize = self
            .model
            .animations
            .iter()
            .map(|a| a.keys * a.tracks.len())
            .sum();
        for seq in &from.sequences {
            if !keep(seq) || names.contains(&seq.label.to_ascii_lowercase()) {
                continue;
            }
            let Some(anim) = seq.anims.first().and_then(|&a| usize::try_from(a).ok()) else {
                continue;
            };
            let Some(desc) = from.anims.get(anim) else {
                continue;
            };
            let frames = (desc.frames as usize).clamp(1, MAX_KEYS);
            let looping =
                seq.flags & anim_flags::LOOPING != 0 || desc.flags & anim_flags::LOOPING != 0;
            let keys = if looping && frames > 1 {
                frames - 1
            } else {
                frames
            };
            // Paid for every bone the animation's model decodes, whether this model has it or not.
            samples += keys * from.bones.len().max(1);
            if samples > MAX_SAMPLES {
                return Err(Error::TooLarge {
                    what: "the model's animation samples",
                    asked: samples as u64,
                    cap: MAX_SAMPLES as u64,
                });
            }
            let mut tracks: Vec<Track> = map
                .iter()
                .flatten()
                .map(|&node| Track {
                    node,
                    rotations: Vec::with_capacity(keys),
                    translations: Vec::with_capacity(keys),
                    scales: Vec::new(),
                })
                .collect();
            let Ok(poses) = from.frames(anim, ani, keys as u32) else {
                self.model
                    .notes
                    .push(format!("sequence {} did not decode", seq.label));
                continue;
            };
            let keys = poses.len();
            for pose in &poses {
                let mut t = 0;
                for (b, node) in map.iter().enumerate() {
                    if node.is_none() {
                        continue;
                    }
                    let (p, q) = pose[b];
                    tracks[t].rotations.push(math::normalize_quat(q));
                    tracks[t].translations.push(p);
                    t += 1;
                }
            }
            let fps = if desc.fps > 0.0 && desc.fps.is_finite() {
                desc.fps
            } else {
                30.0
            };
            let spans = if looping {
                keys
            } else {
                keys.saturating_sub(1)
            };
            names.insert(seq.label.to_ascii_lowercase());
            self.model.animations.push(Animation {
                name: seq.label.clone(),
                duration: (spans.max(1) as f32 / fps).max(1.0 / fps),
                keys,
                cyclic: looping,
                blend: desc.flags & anim_flags::DELTA != 0 || seq.flags & anim_flags::DELTA != 0,
                priority: 0,
                tracks,
                ground: Vec::new(),
            });
        }
        Ok(())
    }
}

/// Every node's transform in the model's world (the root's turn and scale included), from per-node local transforms.
pub fn world_transforms(model: &Model, local: &[Mat4]) -> Vec<Mat4> {
    let mut out = vec![math::IDENTITY; model.nodes.len()];
    for (i, n) in model.nodes.iter().enumerate() {
        let l = local.get(i).copied().unwrap_or_else(|| n.local());
        out[i] = match n.parent {
            Some(p) if p < i => math::mul(&out[p], &l),
            _ => l,
        };
    }
    out
}

/// The nodes' local transforms at time `t` of `anim` (None: at rest), keys interpolated.
pub fn pose_at(model: &Model, anim: Option<&Animation>, t: f32) -> Vec<Mat4> {
    let mut local: Vec<Mat4> = model.nodes.iter().map(Node::local).collect();
    let Some(a) = anim else { return local };
    if a.keys == 0 {
        return local;
    }
    let spans = if a.cyclic { a.keys } else { a.keys - 1 };
    let pos = if a.duration > 0.0 && spans > 0 {
        let u = if a.cyclic {
            (t / a.duration).rem_euclid(1.0)
        } else {
            (t / a.duration).clamp(0.0, 1.0)
        };
        u * spans as f32
    } else {
        0.0
    };
    let k0 = (pos.floor() as usize).min(a.keys - 1);
    let k1 = if a.cyclic {
        (k0 + 1) % a.keys
    } else {
        (k0 + 1).min(a.keys - 1)
    };
    let f = pos - pos.floor();
    for tr in &a.tracks {
        let Some(n) = model.nodes.get(tr.node) else {
            continue;
        };
        let r = match (tr.rotations.get(k0), tr.rotations.get(k1)) {
            (Some(&a), Some(&b)) => math::slerp(a, b, f),
            _ => n.rotation,
        };
        let p = match (tr.translations.get(k0), tr.translations.get(k1)) {
            (Some(a), Some(b)) => [0, 1, 2].map(|k| a[k] + (b[k] - a[k]) * f),
            _ => n.translation,
        };
        local[tr.node] = math::trs(p, r, n.scale);
    }
    local
}

/// Every alpha a material reads as (`$alphatest` cut at one half, `$translucent` blended).
pub fn alpha_of(blend: crate::vmt::Blend) -> Alpha {
    match blend {
        crate::vmt::Blend::AlphaTest => Alpha::Mask(0.5),
        crate::vmt::Blend::Translucent | crate::vmt::Blend::Additive => Alpha::Blend,
        crate::vmt::Blend::Opaque => Alpha::Opaque,
    }
}
