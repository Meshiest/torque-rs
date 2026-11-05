//! The glTF writer: a model as a `.glb`, with this crate's `torque` extras (module docs of [`super`]).

use serde_json::{Map, Value, json};
use torque_modelcore::math::{self, Mat4, Quat, Vec3};
use torque_modelcore::{Alpha, Model};

use super::EXTRAS_KEY;
use crate::ImportError;

/// Torque's frame into glTF's (y up): the inverse of the reader's turn.
fn dir(v: Vec3) -> Vec3 {
    [v[0], v[2], -v[1]]
}

fn quat(q: Quat) -> Quat {
    let v = dir([q[0], q[1], q[2]]);
    [v[0], v[1], v[2], q[3]]
}

fn scale(s: Vec3) -> Vec3 {
    [s[0], s[2], s[1]]
}

/// A matrix of Torque's frame in glTF's, in glTF's column order.
fn matrix(m: &Mat4) -> [f32; 16] {
    let c: Mat4 = [
        1., 0., 0., 0., 0., 0., -1., 0., 0., 1., 0., 0., 0., 0., 0., 1.,
    ];
    let ct: Mat4 = [
        1., 0., 0., 0., 0., 0., 1., 0., 0., -1., 0., 0., 0., 0., 0., 1.,
    ];
    let g = math::mul(&math::mul(&ct, m), &c);
    std::array::from_fn(|i| g[(i % 4) * 4 + i / 4])
}

fn extras(fields: Value) -> Value {
    json!({ EXTRAS_KEY: fields })
}

#[derive(Default)]
struct Out {
    bin: Vec<u8>,
    views: Vec<Value>,
    accessors: Vec<Value>,
}

impl Out {
    fn view(&mut self, bytes: &[u8]) -> usize {
        while !self.bin.len().is_multiple_of(4) {
            self.bin.push(0);
        }
        self.views
            .push(json!({ "buffer": 0, "byteOffset": self.bin.len(), "byteLength": bytes.len() }));
        self.bin.extend_from_slice(bytes);
        self.views.len() - 1
    }

    fn accessor(
        &mut self,
        bytes: &[u8],
        component: u32,
        count: usize,
        kind: &str,
        bounds: Option<(Value, Value)>,
    ) -> usize {
        let view = self.view(bytes);
        let mut a =
            json!({ "bufferView": view, "componentType": component, "count": count, "type": kind });
        if let Some((min, max)) = bounds {
            a["min"] = min;
            a["max"] = max;
        }
        self.accessors.push(a);
        self.accessors.len() - 1
    }

    fn floats<const N: usize>(&mut self, data: &[[f32; N]], kind: &str, bounds: bool) -> usize {
        let bytes: Vec<u8> = data
            .iter()
            .flatten()
            .flat_map(|f| f.to_le_bytes())
            .collect();
        let b = bounds.then(|| {
            let (mut lo, mut hi) = ([f32::MAX; N], [f32::MIN; N]);
            for v in data {
                for k in 0..N {
                    lo[k] = lo[k].min(v[k]);
                    hi[k] = hi[k].max(v[k]);
                }
            }
            (json!(lo.to_vec()), json!(hi.to_vec()))
        });
        self.accessor(&bytes, 5126, data.len(), kind, b)
    }
}

/// The model as a `.glb` (module docs of [`super`]): every node, mesh (each in a holder node under its node, or at the root for a
/// skin), skin, material, image and animation, and the `torque` extras that make the read back exact.
pub fn write_glb(model: &Model) -> Result<Vec<u8>, ImportError> {
    model.check()?;
    let mut out = Out::default();
    let node_count = model.nodes.len();
    let mut nodes: Vec<Value> = model
        .nodes
        .iter()
        .map(|n| {
            json!({
                "name": n.name,
                "translation": dir(n.translation).to_vec(),
                "rotation": quat(n.rotation).to_vec(),
                "scale": scale(n.scale).to_vec(),
            })
        })
        .collect();
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); node_count];
    let mut roots = Vec::new();
    for (i, n) in model.nodes.iter().enumerate() {
        match n.parent {
            Some(p) => children[p].push(i),
            None => roots.push(i),
        }
    }

    // Images, then one texture a (image, wrap) pair, then materials.
    let mut images = Vec::new();
    for img in &model.images {
        let mut v = json!({ "name": img.name });
        match (&img.data, &img.uri) {
            (Some(data), _) => {
                v["bufferView"] = json!(out.view(data));
                v["mimeType"] = json!(if img.mime.is_empty() {
                    "image/png"
                } else {
                    img.mime.as_str()
                });
            }
            (None, Some(uri)) => v["uri"] = json!(uri),
            (None, None) => v["uri"] = json!(format!("{}.png", img.name)),
        }
        images.push(v);
    }
    let mut samplers: Vec<[bool; 2]> = Vec::new();
    let mut textures: Vec<(usize, usize)> = Vec::new();
    let mut texture = |image: usize, wrap: [bool; 2]| {
        let s = samplers.iter().position(|w| *w == wrap).unwrap_or_else(|| {
            samplers.push(wrap);
            samplers.len() - 1
        });
        let t = textures
            .iter()
            .position(|t| *t == (image, s))
            .unwrap_or_else(|| {
                textures.push((image, s));
                textures.len() - 1
            });
        json!({ "index": t })
    };
    let materials: Vec<Value> = model
        .materials
        .iter()
        .map(|m| {
            let mut pbr = json!({
                "baseColorFactor": m.base_color.to_vec(),
                "metallicFactor": m.metallic,
                "roughnessFactor": m.roughness,
            });
            if let Some(t) = m.base_texture {
                pbr["baseColorTexture"] = texture(t, m.wrap);
            }
            if let Some(t) = m.metal_rough_texture {
                pbr["metallicRoughnessTexture"] = texture(t, m.wrap);
            }
            let mut v = json!({
                "name": m.name,
                "pbrMetallicRoughness": pbr,
                "doubleSided": m.two_sided,
                "emissiveFactor": m.emissive.to_vec(),
            });
            match m.alpha {
                Alpha::Opaque => {}
                Alpha::Mask(c) => {
                    v["alphaMode"] = json!("MASK");
                    v["alphaCutoff"] = json!(c);
                }
                Alpha::Blend => v["alphaMode"] = json!("BLEND"),
            }
            if let Some(t) = m.normal_texture {
                v["normalTexture"] = texture(t, m.wrap);
            }
            if let Some(t) = m.emissive_texture {
                v["emissiveTexture"] = texture(t, m.wrap);
            }
            let mut t = Map::new();
            t.insert("name".into(), json!(m.name));
            if let Some(f) = m.flags {
                t.insert("flags".into(), json!(f));
            }
            v["extras"] = extras(Value::Object(t));
            v
        })
        .collect();

    // Meshes, each in a holder node.
    let mut meshes = Vec::new();
    let mut skins = Vec::new();
    for (i, m) in model.meshes.iter().enumerate() {
        let mut attributes = Map::new();
        attributes.insert(
            "POSITION".into(),
            json!(out.floats(
                &m.positions.iter().map(|&p| dir(p)).collect::<Vec<_>>(),
                "VEC3",
                true
            )),
        );
        if !m.normals.is_empty() {
            attributes.insert(
                "NORMAL".into(),
                json!(out.floats(
                    &m.normals.iter().map(|&p| dir(p)).collect::<Vec<_>>(),
                    "VEC3",
                    false
                )),
            );
        }
        if !m.uv0.is_empty() {
            attributes.insert(
                "TEXCOORD_0".into(),
                json!(out.floats(&m.uv0, "VEC2", false)),
            );
        }
        if !m.uv1.is_empty() {
            attributes.insert(
                "TEXCOORD_1".into(),
                json!(out.floats(&m.uv1, "VEC2", false)),
            );
        }
        if !m.colors.is_empty() {
            attributes.insert(
                "COLOR_0".into(),
                json!(out.floats(&m.colors, "VEC4", false)),
            );
        }
        if !m.joints.is_empty() {
            let bytes: Vec<u8> = m
                .joints
                .iter()
                .flatten()
                .flat_map(|j| j.to_le_bytes())
                .collect();
            attributes.insert(
                "JOINTS_0".into(),
                json!(out.accessor(&bytes, 5123, m.joints.len(), "VEC4", None)),
            );
        }
        if !m.weights.is_empty() {
            attributes.insert(
                "WEIGHTS_0".into(),
                json!(out.floats(&m.weights, "VEC4", false)),
            );
        }
        let primitives: Vec<Value> = m
            .primitives
            .iter()
            .map(|p| {
                // Back to glTF's counter-clockwise fronts.
                let bytes: Vec<u8> = p
                    .indices
                    .chunks_exact(3)
                    .flat_map(|t| [t[0], t[2], t[1]])
                    .flat_map(u32::to_le_bytes)
                    .collect();
                let mut v = json!({
                    "attributes": attributes.clone(),
                    "indices": out.accessor(&bytes, 5125, p.indices.len(), "SCALAR", None),
                });
                if let Some(mat) = p.material {
                    v["material"] = json!(mat);
                }
                v
            })
            .collect();
        meshes.push(json!({ "name": m.name, "primitives": primitives }));
        let mut holder = json!({
            "name": m.name,
            "mesh": i,
            "extras": extras(json!({ "holder": true, "node": m.node, "detail": m.detail, "name": m.name, "lod_of": m.lod_of })),
        });
        match &m.skin {
            Some(s) => {
                let ibm: Vec<[f32; 16]> = s.inverse_binds.iter().map(matrix).collect();
                skins.push(json!({ "joints": s.joints, "inverseBindMatrices": out.floats(&ibm, "MAT4", false) }));
                holder["skin"] = json!(skins.len() - 1);
                roots.push(nodes.len());
            }
            None => children[m.node].push(nodes.len()),
        }
        nodes.push(holder);
    }
    for (i, c) in children.iter().enumerate() {
        if !c.is_empty() {
            nodes[i]["children"] = json!(c);
        }
    }

    // Animations: one value a key, the first again at the end of a loop.
    let mut animations = Vec::new();
    for a in &model.animations {
        let wrap = usize::from(a.cyclic);
        let times: Vec<[f32; 1]> = (0..a.keys + wrap)
            .map(|k| {
                [if k == a.keys {
                    a.duration
                } else {
                    a.key_time(k)
                }]
            })
            .collect();
        let input = out.floats(&times, "SCALAR", true);
        let key = |k: usize| if k == a.keys { 0 } else { k };
        let mut samplers = Vec::new();
        let mut channels = Vec::new();
        for t in &a.tracks {
            let mut add = |path: &str, output: usize| {
                samplers
                    .push(json!({ "input": input, "output": output, "interpolation": "LINEAR" }));
                channels.push(json!({ "sampler": samplers.len() - 1, "target": { "node": t.node, "path": path } }));
            };
            if !t.translations.is_empty() {
                let v: Vec<Vec3> = (0..times.len())
                    .map(|k| dir(t.translations[key(k)]))
                    .collect();
                add("translation", out.floats(&v, "VEC3", false));
            }
            if !t.rotations.is_empty() {
                let v: Vec<Quat> = (0..times.len())
                    .map(|k| quat(t.rotations[key(k)]))
                    .collect();
                add("rotation", out.floats(&v, "VEC4", false));
            }
            if !t.scales.is_empty() {
                let v: Vec<Vec3> = (0..times.len()).map(|k| scale(t.scales[key(k)])).collect();
                add("scale", out.floats(&v, "VEC3", false));
            }
        }
        let ground: Vec<Value> = a.ground.iter().map(|g| json!(g.to_vec())).collect();
        animations.push(json!({
            "name": a.name,
            "samplers": samplers,
            "channels": channels,
            "extras": extras(json!({
                "keys": a.keys,
                "cyclic": a.cyclic,
                "blend": a.blend,
                "priority": a.priority,
                "duration": a.duration,
                "ground": ground,
            })),
        }));
    }

    if !out.bin.is_empty() {
        while !out.bin.len().is_multiple_of(4) {
            out.bin.push(0);
        }
    }
    let details: Vec<Value> = model
        .details
        .iter()
        .map(|d| json!([d.name, d.size]))
        .collect();
    let mut doc = json!({
        "asset": { "version": "2.0", "generator": "torque-import" },
        "scene": 0,
        "scenes": [{ "nodes": roots }],
        "nodes": nodes,
        "extras": extras(json!({ "details": details })),
    });
    if !out.bin.is_empty() {
        doc["buffers"] = json!([{ "byteLength": out.bin.len() }]);
    }
    let mut put = |key: &str, list: Vec<Value>| {
        if !list.is_empty() {
            doc[key] = Value::Array(list);
        }
    };
    put("meshes", meshes);
    put("skins", skins);
    put("materials", materials);
    put("images", images);
    put(
        "samplers",
        samplers
            .iter()
            .map(|w| {
                let mode = |r: bool| if r { 10497 } else { 33071 };
                json!({ "wrapS": mode(w[0]), "wrapT": mode(w[1]) })
            })
            .collect(),
    );
    put(
        "textures",
        textures
            .iter()
            .map(|&(source, sampler)| json!({ "source": source, "sampler": sampler }))
            .collect(),
    );
    put("animations", animations);
    put("bufferViews", std::mem::take(&mut out.views));
    put("accessors", std::mem::take(&mut out.accessors));

    let mut text = serde_json::to_vec(&doc).map_err(|e| ImportError::Invalid(e.to_string()))?;
    while !text.len().is_multiple_of(4) {
        text.push(b' ');
    }
    let total = 12
        + 8
        + text.len()
        + if out.bin.is_empty() {
            0
        } else {
            8 + out.bin.len()
        };
    let mut glb = Vec::with_capacity(total);
    glb.extend_from_slice(b"glTF");
    glb.extend_from_slice(&2u32.to_le_bytes());
    glb.extend_from_slice(&(total as u32).to_le_bytes());
    glb.extend_from_slice(&(text.len() as u32).to_le_bytes());
    glb.extend_from_slice(b"JSON");
    glb.extend_from_slice(&text);
    if !out.bin.is_empty() {
        glb.extend_from_slice(&(out.bin.len() as u32).to_le_bytes());
        glb.extend_from_slice(b"BIN\0");
        glb.extend_from_slice(&out.bin);
    }
    Ok(glb)
}
