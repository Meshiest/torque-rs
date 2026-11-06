//! glTF 2.0: the reader ([`read`]) into the model layer and the writer ([`write_glb`]) out of it.
//!
//! **What is read:** the node tree (TRS or matrix), every node's mesh (triangles, strips and fans; positions, normals, two UV
//! sets, the first color set, the first joint and weight sets), skins, materials (base color, alpha mode, double-sided, normal,
//! metal/rough and emissive textures), images (by URI, or their bytes when carried) and animations (translation, rotation and
//! scale channels; step, linear and cubic-spline curves sampled at [`Options::rate`]). Not read, each with a note: morph
//! targets, points and lines, cameras, lights.
//!
//! **Details and collision.** A mesh's detail comes from its node's name as Torque's shape loader takes it
//! (`torque_modelcore::naming::mesh_detail`: `box2`, `Col-1`, `LOSCol-9`; no number: `detail2`). A model with no collision mesh
//! gets a box hull ([`Options::box_hull`]).
//!
//! **Files this crate wrote** carry a `torque` object in the glTF's `extras` (the document's, each material's, each animation's
//! and each mesh holder node's): the details, the material names and flags, the sequences' keys and flags and ground frames
//! (in Torque's frame), and which node a mesh hangs on. The reader takes them as they are, so a shape survives the trip
//! unchanged; a file without them is read by the conventions above.

mod accessor;
mod anim;
pub mod container;
mod write;

#[cfg(test)]
mod tests;

pub use write::write_glb;

use gltf::json;
use gltf::json::validation::Checked;
use serde_json::Value;
use torque_modelcore::math::{self, Mat4, Quat, Vec3};
use torque_modelcore::naming::mesh_detail;
use torque_modelcore::{
    Alpha, Animation, Detail, Image, Material, Mesh, Model, Node, Primitive, Skin, Source, Track,
};

use crate::{Charge, ImportError, Options, cap, caps, pay};
use accessor::raw;
use anim::{Curve, Interp};
use container::{UriSource, image_size, resolve_uri};

/// The key of this crate's object in a glTF's `extras`.
pub const EXTRAS_KEY: &str = "torque";

/// Gives the first bytes of a side file, at most the second argument's count, by its relative path (`/`-separated, no `..`),
/// or None. The reader asks for a buffer's declared length (already under [`caps::MAX_BUFFER_BYTES`] with every other
/// buffer's) and refuses a longer answer, so a resolver must read no more than it is asked for ([`side_files`] does not).
pub type Resolver<'a> = &'a mut dyn FnMut(&str, usize) -> Option<Vec<u8>>;

/// A resolver reading side files under the folder `dir`: at most the asked-for bytes of each (a file longer than that is read
/// only that far), regular files only, and none that resolves (through a link) outside `dir`.
pub fn side_files(dir: &std::path::Path) -> impl FnMut(&str, usize) -> Option<Vec<u8>> + use<> {
    let dir = dir.to_path_buf();
    move |rel: &str, max: usize| {
        use std::io::Read;
        let root = dir.canonicalize().ok()?;
        let path = rel
            .split('/')
            .fold(root.clone(), |p, part| p.join(part))
            .canonicalize()
            .ok()?;
        if !path.starts_with(&root) {
            return None;
        }
        let file = std::fs::File::open(&path).ok()?;
        let meta = file.metadata().ok()?;
        if !meta.is_file() {
            return None;
        }
        let len = usize::try_from(meta.len()).unwrap_or(usize::MAX);
        let mut out = Vec::with_capacity(len.min(max));
        file.take(max as u64).read_to_end(&mut out).ok()?;
        Some(out)
    }
}

fn malformed<T>(what: impl Into<String>) -> Result<T, ImportError> {
    Err(ImportError::Malformed(what.into()))
}

/// The `torque` object of an `extras`, if it is one.
fn torque_extras(extras: &json::Extras) -> Option<serde_json::Map<String, Value>> {
    let raw = extras.as_ref()?;
    match serde_json::from_str::<Value>(raw.get()).ok()? {
        Value::Object(mut o) => match o.remove(EXTRAS_KEY)? {
            Value::Object(t) => Some(t),
            _ => None,
        },
        _ => None,
    }
}

fn get_f32(o: &serde_json::Map<String, Value>, key: &str) -> Option<f32> {
    let v = o.get(key)?.as_f64()? as f32;
    v.is_finite().then_some(v)
}

fn get_usize(o: &serde_json::Map<String, Value>, key: &str) -> Option<usize> {
    usize::try_from(o.get(key)?.as_u64()?).ok()
}

fn get_vec3s(o: &serde_json::Map<String, Value>, key: &str, max: usize) -> Option<Vec<Vec3>> {
    let list = o.get(key)?.as_array()?;
    if list.len() > max {
        return None;
    }
    list.iter()
        .map(|v| {
            let a = v.as_array()?;
            let c = |i: usize| {
                a.get(i)?
                    .as_f64()
                    .map(|x| x as f32)
                    .filter(|x| x.is_finite())
            };
            Some([c(0)?, c(1)?, c(2)?])
        })
        .collect()
}

/// A name cut to [`caps::MAX_NAME_BYTES`].
fn short_name(name: &str) -> String {
    let mut end = name.len().min(caps::MAX_NAME_BYTES);
    while !name.is_char_boundary(end) {
        end -= 1;
    }
    name[..end].to_string()
}

/// The turn from a file's frame into Torque's.
#[derive(Clone, Copy)]
struct Frame {
    y_up: bool,
    scale: f32,
}

impl Frame {
    fn dir(self, v: Vec3) -> Vec3 {
        if self.y_up { [v[0], -v[2], v[1]] } else { v }
    }
    fn point(self, v: Vec3) -> Vec3 {
        self.dir(v).map(|c| c * self.scale)
    }
    fn quat(self, q: Quat) -> Quat {
        let v = self.dir([q[0], q[1], q[2]]);
        [v[0], v[1], v[2], q[3]]
    }
    fn scale(self, s: Vec3) -> Vec3 {
        if self.y_up { [s[0], s[2], s[1]] } else { s }
    }
    /// A matrix of the file's frame in Torque's: the axes turned on both sides, the translation scaled.
    fn matrix(self, m: &Mat4) -> Mat4 {
        let mut out = *m;
        if self.y_up {
            let c: Mat4 = [
                1., 0., 0., 0., 0., 0., -1., 0., 0., 1., 0., 0., 0., 0., 0., 1.,
            ];
            let ct: Mat4 = [
                1., 0., 0., 0., 0., 0., 1., 0., 0., -1., 0., 0., 0., 0., 0., 1.,
            ];
            out = math::mul(&math::mul(&c, m), &ct);
        }
        for r in 0..3 {
            out[r * 4 + 3] *= self.scale;
        }
        out
    }
}

/// Reads a `.glb`, or a `.gltf` whose side files `resolve` gives, into the model layer in Torque's frame (module docs). Work
/// is paid through `charge` before it is done.
pub fn read(
    bytes: &[u8],
    resolve: Resolver,
    options: &Options,
    mut charge: Charge,
) -> Result<Model, ImportError> {
    let charge = &mut charge;
    pay(charge, bytes.len() as u64)?;
    let (json_bytes, bin) = container::split(bytes)?;
    if json_bytes.len() > options.max_json_bytes {
        return Err(ImportError::TooLarge {
            what: "the JSON",
            bytes: json_bytes.len() as u64,
            cap: options.max_json_bytes as u64,
        });
    }
    // The parse builds every object, array and string at once, at many times its text's size: their count is held first.
    let values = container::json_values(json_bytes, caps::MAX_JSON_VALUES);
    cap("values in the JSON", values, caps::MAX_JSON_VALUES)?;
    pay(charge, values as u64)?;
    let to_error = |e: &dyn std::fmt::Display| {
        let mut s = e.to_string();
        s.truncate(200);
        ImportError::Malformed(s)
    };
    let parsed: json::Root = json::deserialize::from_slice(json_bytes).map_err(|e| to_error(&e))?;
    // `gltf-json`'s validation reads each primitive's position accessor before it checks the index: checked here first.
    let positions = &Checked::Valid(json::mesh::Semantic::Positions);
    let bad_position = parsed.meshes.iter().flat_map(|m| &m.primitives).any(|p| {
        p.attributes
            .get(positions)
            .is_some_and(|a| a.value() >= parsed.accessors.len())
    });
    if bad_position {
        return malformed("a primitive's positions name no accessor");
    }
    let document = gltf::Document::from_json(parsed).map_err(|e| match e {
        // Only the first few of a file's validation errors are told.
        gltf::Error::Validation(list) => {
            let first: Vec<String> = list
                .iter()
                .take(3)
                .map(|(path, e)| format!("{path}: {e}"))
                .collect();
            ImportError::Malformed(format!(
                "{} validation errors: {}",
                list.len(),
                first.join("; ")
            ))
        }
        other => to_error(&other),
    })?;
    let root = document.as_json();
    cap("nodes", root.nodes.len(), caps::MAX_NODES)?;
    cap("meshes", root.meshes.len(), caps::MAX_MESHES)?;
    cap("materials", root.materials.len(), caps::MAX_MATERIALS)?;
    cap("images", root.images.len(), caps::MAX_IMAGES)?;
    cap("animations", root.animations.len(), caps::MAX_ANIMATIONS)?;
    if !(options.scale.is_finite()
        && options.scale > 0.0
        && options.rate.is_finite()
        && options.rate > 0.0)
    {
        return Err(ImportError::Invalid(
            "a scale or rate that is not a positive number".into(),
        ));
    }
    let buffers = load_buffers(root, bin, resolve, charge)?;
    let frame = Frame {
        y_up: options.up != 'z',
        scale: options.scale,
    };
    let mut r = Reader {
        root,
        buffers: &buffers,
        frame,
        options,
        model: Model {
            source: Source {
                format: "glTF".into(),
                up: if frame.y_up { 'y' } else { 'z' },
                scale: options.scale,
            },
            ..Model::default()
        },
        exact: torque_extras(&root.extras),
        node_map: Vec::new(),
        holders: Vec::new(),
        read: Counts::default(),
    };
    r.nodes()?;
    r.materials(charge)?;
    r.meshes(charge)?;
    r.animations(charge)?;
    let mut model = r.model;
    if r.exact.is_none() && options.box_hull {
        add_box_hull(&mut model);
    }
    model.check()?;
    Ok(model)
}

/// Each buffer's bytes: the `.glb`'s binary chunk, a `data:` URI decoded or a side file, cut to the declared length.
fn load_buffers(
    root: &json::Root,
    bin: Option<&[u8]>,
    resolve: Resolver,
    charge: &mut Charge,
) -> Result<Vec<Vec<u8>>, ImportError> {
    cap("buffers", root.buffers.len(), 1_024)?;
    let total = root
        .buffers
        .iter()
        .try_fold(0u64, |sum, b| sum.checked_add(b.byte_length.0));
    let total = total.unwrap_or(u64::MAX);
    if total > caps::MAX_BUFFER_BYTES as u64 {
        return Err(ImportError::TooLarge {
            what: "the buffers",
            bytes: total,
            cap: caps::MAX_BUFFER_BYTES as u64,
        });
    }
    pay(charge, total)?;
    let mut out = Vec::with_capacity(root.buffers.len());
    // The bytes actually held, against the same cap as the declared lengths (a resolver that answers with more is refused).
    let mut held = 0usize;
    for (i, b) in root.buffers.iter().enumerate() {
        let want = b.byte_length.0 as usize;
        let mut bytes = match (&b.uri, i, bin) {
            (None, 0, Some(bin)) => bin.get(..want).unwrap_or(bin).to_vec(),
            (None, _, _) => {
                return malformed(format!(
                    "buffer {i} has no URI and is not the .glb's binary chunk"
                ));
            }
            (Some(uri), _, _) => match resolve_uri(uri, want)? {
                UriSource::Data(data, _) => data,
                UriSource::File(path) => {
                    let bytes = resolve(&path, want).ok_or(ImportError::Missing(path))?;
                    if bytes.len() > want {
                        return Err(ImportError::TooLarge {
                            what: "a side file's answer",
                            bytes: bytes.len() as u64,
                            cap: want as u64,
                        });
                    }
                    bytes
                }
            },
        };
        held = held.saturating_add(bytes.len());
        cap("bytes of buffers held", held, caps::MAX_BUFFER_BYTES)?;
        if bytes.len() < want {
            return malformed(format!("buffer {i} is shorter than its byteLength"));
        }
        bytes.truncate(want);
        out.push(bytes);
    }
    Ok(out)
}

struct Reader<'a> {
    root: &'a json::Root,
    buffers: &'a [Vec<u8>],
    frame: Frame,
    options: &'a Options,
    model: Model,
    /// The document's `torque` extras: a file this crate wrote.
    exact: Option<serde_json::Map<String, Value>>,
    /// Each glTF node's node of the model (None for a mesh holder).
    node_map: Vec<Option<usize>>,
    /// Each glTF node that holds a mesh for another (a file this crate wrote): the node it hangs on and the detail.
    holders: Vec<Option<Holder>>,
    /// What has been read so far, each held under its cap before the next read.
    read: Counts,
}

#[derive(Default)]
struct Counts {
    vertices: usize,
    indices: usize,
    joints: usize,
    keys: usize,
    image_bytes: usize,
}

impl Counts {
    fn add(slot: &mut usize, what: &'static str, n: usize, max: usize) -> Result<(), ImportError> {
        *slot = slot.saturating_add(n);
        cap(what, *slot, max)
    }
}

/// A glTF node that holds a mesh for a node of the model (a file this crate wrote).
struct Holder {
    node: usize,
    detail: Option<usize>,
    name: String,
    lod_of: Option<usize>,
}

impl Reader<'_> {
    fn nodes(&mut self) -> Result<(), ImportError> {
        let nodes = &self.root.nodes;
        let mut parent: Vec<Option<usize>> = vec![None; nodes.len()];
        for (i, n) in nodes.iter().enumerate() {
            for c in n.children.iter().flatten() {
                let c = c.value();
                if c >= nodes.len() || c == i || parent[c].is_some() {
                    return malformed(format!("node {c} has two parents or is its own"));
                }
                parent[c] = Some(i);
            }
        }
        for i in 0..nodes.len() {
            let mut at = parent[i];
            let mut depth = 0;
            while let Some(p) = at {
                depth += 1;
                if depth > caps::MAX_NODE_DEPTH {
                    return malformed(format!(
                        "node {i} is deeper than {} or in a cycle",
                        caps::MAX_NODE_DEPTH
                    ));
                }
                at = parent[p];
            }
        }
        let exact = self.exact.is_some();
        self.holders = nodes
            .iter()
            .map(|n| {
                let t = torque_extras(&n.extras).filter(|_| exact)?;
                let holds = t.get("holder").and_then(Value::as_bool)?;
                let node = get_usize(&t, "node")?;
                let detail = get_usize(&t, "detail");
                let name = t.get("name").and_then(Value::as_str).unwrap_or("mesh");
                let lod_of = get_usize(&t, "lod_of");
                holds.then(|| Holder {
                    node,
                    detail,
                    name: short_name(name),
                    lod_of,
                })
            })
            .collect();
        let mut map = vec![None; nodes.len()];
        let mut next = 0;
        for (i, slot) in map.iter_mut().enumerate() {
            if self.holders[i].is_none() {
                *slot = Some(next);
                next += 1;
            }
        }
        for (i, n) in nodes.iter().enumerate() {
            let Some(_) = map[i] else {
                if n.children.as_ref().is_some_and(|c| !c.is_empty()) {
                    return malformed(format!("mesh holder node {i} has children"));
                }
                continue;
            };
            let (t, q, s) = match n.matrix {
                Some(m) => {
                    let rows: Mat4 = std::array::from_fn(|k| m[(k % 4) * 4 + k / 4]);
                    math::decompose(&rows)
                }
                None => (
                    n.translation.unwrap_or([0.0; 3]),
                    n.rotation.map_or(math::NO_TURN, |q| q.0),
                    n.scale.unwrap_or([1.0; 3]),
                ),
            };
            if !t.iter().chain(&q).chain(&s).all(|c| c.is_finite()) {
                return malformed(format!("node {i}'s transform"));
            }
            let name = n
                .name
                .as_deref()
                .map_or_else(|| format!("node{i}"), short_name);
            self.model.nodes.push(Node {
                name,
                parent: parent[i].and_then(|p| map[p]),
                translation: self.frame.point(t),
                // A file this crate wrote keeps its stored turns as they are; any other's are made unit length.
                rotation: if exact {
                    self.frame.quat(q)
                } else {
                    math::normalize_quat(self.frame.quat(q))
                },
                scale: self.frame.scale(s),
            });
        }
        // A holder's node must be a node of the model.
        for h in self.holders.iter_mut().flatten() {
            match map.get(h.node).copied().flatten() {
                Some(n) => h.node = n,
                None => return malformed("a mesh holder names a node that is not one"),
            }
        }
        self.node_map = map;
        if let Some(t) = &self.exact {
            let list = t
                .get("details")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            cap("details", list.len(), torque_shapes::MAX_SHAPE_DETAILS)?;
            for d in list {
                let pair = d.as_array().filter(|a| a.len() == 2);
                let name = pair.and_then(|a| a[0].as_str());
                let size = pair
                    .and_then(|a| a[1].as_f64())
                    .map(|s| s as f32)
                    .filter(|s| s.is_finite());
                let (Some(name), Some(size)) = (name, size) else {
                    return malformed("a detail of the torque extras");
                };
                self.model.details.push(Detail {
                    name: short_name(name),
                    size,
                });
            }
        }
        Ok(())
    }

    fn image(&mut self, index: usize, charge: &mut Charge) -> Result<usize, ImportError> {
        let Some(img) = self.root.images.get(index) else {
            return malformed(format!("image {index}"));
        };
        let stem = |path: &str| {
            let file = path.rsplit('/').next().unwrap_or(path);
            short_name(file.rsplit_once('.').map_or(file, |(s, _)| s))
        };
        let mut image = Image {
            name: img
                .name
                .as_deref()
                .map_or_else(|| format!("image{index}"), short_name),
            mime: img
                .mime_type
                .as_ref()
                .map(|m| m.0.clone())
                .unwrap_or_default(),
            ..Image::default()
        };
        if let Some(uri) = &img.uri {
            match resolve_uri(uri, caps::MAX_IMAGE_BYTES)? {
                UriSource::Data(data, mime) => {
                    image.data = Some(data);
                    image.mime = mime;
                }
                UriSource::File(path) => {
                    image.name = stem(&path);
                    image.uri = Some(path);
                }
            }
        } else if let Some(view) = img.buffer_view {
            let Some(v) = self.root.buffer_views.get(view.value()) else {
                return malformed(format!("image {index}'s view"));
            };
            let (offset, length) = (v.byte_offset.map_or(0, |o| o.0), v.byte_length.0);
            if length > caps::MAX_IMAGE_BYTES as u64 {
                return Err(ImportError::TooLarge {
                    what: "an image",
                    bytes: length,
                    cap: caps::MAX_IMAGE_BYTES as u64,
                });
            }
            let bytes = self.buffers.get(v.buffer.value()).and_then(|b| {
                let start = usize::try_from(offset).ok()?;
                b.get(start..start.checked_add(length as usize)?)
            });
            let Some(bytes) = bytes else {
                return malformed(format!("image {index} past its buffer"));
            };
            pay(charge, length)?;
            image.data = Some(bytes.to_vec());
        }
        if let Some(data) = &image.data {
            Counts::add(
                &mut self.read.image_bytes,
                "bytes of images",
                data.len(),
                caps::MAX_IMAGE_BYTES_TOTAL,
            )?;
            let Some((w, h)) = image_size(data) else {
                return Err(ImportError::Unsupported(format!(
                    "image {index} is not a PNG or a JPEG"
                )));
            };
            if w > caps::MAX_IMAGE_SIDE || h > caps::MAX_IMAGE_SIDE {
                return Err(ImportError::Cap {
                    what: "pixels a side of an image",
                    count: u64::from(w.max(h)),
                    cap: u64::from(caps::MAX_IMAGE_SIDE),
                });
            }
            image.size = Some((w, h));
        }
        self.model.images.push(image);
        Ok(self.model.images.len() - 1)
    }

    /// The model's image of texture `index`, read once.
    fn texture(
        &mut self,
        index: usize,
        seen: &mut [Option<usize>],
        charge: &mut Charge,
    ) -> Result<usize, ImportError> {
        let Some(t) = self.root.textures.get(index) else {
            return malformed(format!("texture {index}"));
        };
        let source = t.source.value();
        if let Some(Some(done)) = seen.get(source) {
            return Ok(*done);
        }
        let at = self.image(source, charge)?;
        if let Some(slot) = seen.get_mut(source) {
            *slot = Some(at);
        }
        Ok(at)
    }

    fn materials(&mut self, charge: &mut Charge) -> Result<(), ImportError> {
        let mut seen = vec![None; self.root.images.len()];
        for (i, m) in self.root.materials.iter().enumerate() {
            let pbr = &m.pbr_metallic_roughness;
            let mut out = Material {
                base_color: pbr.base_color_factor.0,
                alpha: match m.alpha_mode {
                    Checked::Valid(json::material::AlphaMode::Blend) => Alpha::Blend,
                    Checked::Valid(json::material::AlphaMode::Mask) => {
                        Alpha::Mask(m.alpha_cutoff.map_or(0.5, |c| c.0))
                    }
                    _ => Alpha::Opaque,
                },
                two_sided: m.double_sided,
                emissive: m.emissive_factor.0,
                metallic: pbr.metallic_factor.0,
                roughness: pbr.roughness_factor.0,
                ..Material::default()
            };
            if let Some(info) = &pbr.base_color_texture {
                let t = info.index.value();
                out.base_texture = Some(self.texture(t, &mut seen, charge)?);
                let sampler = self.root.textures.get(t).and_then(|t| t.sampler);
                if let Some(s) = sampler.and_then(|s| self.root.samplers.get(s.value())) {
                    let repeats = |w: &Checked<json::texture::WrappingMode>| {
                        !matches!(w, Checked::Valid(json::texture::WrappingMode::ClampToEdge))
                    };
                    out.wrap = [repeats(&s.wrap_s), repeats(&s.wrap_t)];
                    out.nearest = matches!(
                        s.mag_filter,
                        Some(Checked::Valid(json::texture::MagFilter::Nearest))
                    );
                }
            }
            if let Some(n) = &m.normal_texture {
                out.normal_texture = Some(self.texture(n.index.value(), &mut seen, charge)?);
            }
            if let Some(t) = &pbr.metallic_roughness_texture {
                out.metal_rough_texture = Some(self.texture(t.index.value(), &mut seen, charge)?);
            }
            if let Some(t) = &m.emissive_texture {
                out.emissive_texture = Some(self.texture(t.index.value(), &mut seen, charge)?);
            }
            out.name = match out.base_texture.and_then(|t| self.model.images.get(t)) {
                Some(img) => img.name.clone(),
                None => m
                    .name
                    .as_deref()
                    .map_or_else(|| format!("material{i}"), short_name),
            };
            if let Some(t) = torque_extras(&m.extras).filter(|_| self.exact.is_some()) {
                if let Some(name) = t.get("name").and_then(Value::as_str) {
                    out.name = short_name(name);
                }
                out.flags = t
                    .get("flags")
                    .and_then(Value::as_u64)
                    .and_then(|f| u32::try_from(f).ok());
            }
            self.model.materials.push(out);
        }
        Ok(())
    }

    fn meshes(&mut self, charge: &mut Charge) -> Result<(), ImportError> {
        let mut detail_keys: Vec<(String, i32)> = Vec::new();
        let mut mesh_keys: Vec<usize> = Vec::new();
        let mut primitives = 0usize;
        for (i, n) in self.root.nodes.iter().enumerate() {
            let Some(mesh_index) = n.mesh else {
                continue;
            };
            let Some(gm) = self.root.meshes.get(mesh_index.value()) else {
                return malformed(format!("node {i}'s mesh"));
            };
            primitives += gm.primitives.len();
            cap("primitives", primitives, caps::MAX_PRIMITIVES)?;
            cap("mesh nodes", self.model.meshes.len() + 1, caps::MAX_MESHES)?;
            let (node, detail, name, lod_of) = match (&self.holders[i], self.node_map[i]) {
                (Some(h), _) => (h.node, h.detail, h.name.clone(), h.lod_of),
                (None, Some(node)) => (node, None, self.model.nodes[node].name.clone(), None),
                (None, None) => return malformed(format!("node {i}")),
            };
            let mut mesh = Mesh {
                name,
                node,
                detail,
                lod_of,
                ..Mesh::default()
            };
            if gm.weights.is_some() || gm.primitives.iter().any(|p| p.targets.is_some()) {
                self.model
                    .notes
                    .push(format!("{}: morph targets are not read", mesh.name));
            }
            self.primitives(&mut mesh, gm, charge)?;
            if let Some(skin) = n.skin {
                self.skin(&mut mesh, skin.value(), charge)?;
            }
            if mesh.detail.is_none() && self.exact.is_some() {
                // A mesh of a file this crate wrote that is not in a holder: its detail by name, after the listed ones.
                let d = mesh_detail(&mesh.name);
                mesh.name = d.object;
                let details = &mut self.model.details;
                let at = details
                    .iter()
                    .position(|x| x.name == d.detail)
                    .unwrap_or_else(|| {
                        details.push(Detail {
                            name: d.detail,
                            size: d.size as f32,
                        });
                        details.len() - 1
                    });
                mesh.detail = Some(at);
            } else if mesh.detail.is_none() {
                let d = mesh_detail(&mesh.name);
                mesh.name = d.object;
                let key = match detail_keys
                    .iter()
                    .position(|k| k.0 == d.detail && k.1 == d.size)
                {
                    Some(k) => k,
                    None => {
                        detail_keys.push((d.detail, d.size));
                        detail_keys.len() - 1
                    }
                };
                mesh_keys.push(self.model.meshes.len());
                mesh.detail = Some(key);
            }
            self.model.meshes.push(mesh);
        }
        if self.exact.is_none() {
            // Details by size, largest first (as `TSShape::addDetail` keeps them); equal sizes in order of appearance.
            let mut order: Vec<usize> = (0..detail_keys.len()).collect();
            order.sort_by(|&a, &b| detail_keys[b].1.cmp(&detail_keys[a].1));
            let mut rank = vec![0; detail_keys.len()];
            for (r, &k) in order.iter().enumerate() {
                rank[k] = r;
                self.model.details.push(Detail {
                    name: detail_keys[k].0.clone(),
                    size: detail_keys[k].1 as f32,
                });
            }
            for m in mesh_keys {
                let mesh = &mut self.model.meshes[m];
                mesh.detail = mesh.detail.map(|k| rank[k]);
            }
            // Meshes of one name on one node in other details are one object (`TSShapeLoader::generateObjects`).
            let meshes = &mut self.model.meshes;
            for i in 0..meshes.len() {
                let (head, rest) = meshes.split_at_mut(i);
                let m = &mut rest[0];
                let level = (0..i).find(|&j| {
                    let o = &head[j];
                    o.lod_of.is_none()
                        && o.name == m.name
                        && o.node == m.node
                        && o.skin.is_some() == m.skin.is_some()
                        && o.detail != m.detail
                        && !head
                            .iter()
                            .any(|x| x.lod_of == Some(j) && x.detail == m.detail)
                });
                m.lod_of = level;
            }
        }
        cap(
            "details",
            self.model.details.len(),
            torque_shapes::MAX_SHAPE_DETAILS,
        )?;
        Ok(())
    }

    /// Every primitive of `gm` appended to `mesh`: its vertices after the ones there, its triangles turned clockwise.
    fn primitives(
        &mut self,
        mesh: &mut Mesh,
        gm: &json::Mesh,
        charge: &mut Charge,
    ) -> Result<(), ImportError> {
        use json::mesh::{Mode, Semantic};
        let frame = self.frame;
        let materials = self.model.materials.len();
        // Primitives that name the same vertex accessors share their vertices: (accessors, first vertex, count).
        let mut shared: Vec<(Vec<Option<usize>>, usize, usize)> = Vec::new();
        for (pi, p) in gm.primitives.iter().enumerate() {
            let mode = match p.mode {
                Checked::Valid(m) => m,
                Checked::Invalid => return malformed("a primitive's mode"),
            };
            if !matches!(
                mode,
                Mode::Triangles | Mode::TriangleStrip | Mode::TriangleFan
            ) {
                self.model.notes.push(format!(
                    "{}: primitive {pi} is points or lines, not read",
                    mesh.name
                ));
                continue;
            }
            let attr = |s: Semantic| p.attributes.get(&Checked::Valid(s)).map(|a| a.value());
            let semantics = [
                Semantic::Positions,
                Semantic::Normals,
                Semantic::TexCoords(0),
                Semantic::TexCoords(1),
                Semantic::Colors(0),
                Semantic::Joints(0),
                Semantic::Weights(0),
            ];
            let key: Vec<Option<usize>> = semantics.iter().map(|s| attr(s.clone())).collect();
            let Some(pos) = key[0] else {
                return malformed(format!("{}: primitive {pi} has no positions", mesh.name));
            };
            let known = shared.iter().find(|s| s.0 == key).map(|s| (s.1, s.2));
            let n = match known {
                Some((_, n)) => n,
                None => self
                    .root
                    .accessors
                    .get(pos)
                    .map_or(0, |a| usize::try_from(a.count.0).unwrap_or(usize::MAX)),
            };
            let list = match p.indices {
                Some(a) => {
                    let r = raw(self.root, self.buffers, a.value(), charge)?;
                    Counts::add(
                        &mut self.read.indices,
                        "indices",
                        r.count,
                        caps::MAX_INDICES,
                    )?;
                    r.indices()?
                }
                None => {
                    Counts::add(&mut self.read.indices, "indices", n, caps::MAX_INDICES)?;
                    (0..n.min(caps::MAX_ACCESSOR_ELEMENTS) as u32).collect()
                }
            };
            if list.iter().any(|&v| v as usize >= n) {
                return malformed(format!(
                    "{}: primitive {pi} has an index past its vertices",
                    mesh.name
                ));
            }
            let tris = triangles(&list, mode);
            let base = match known {
                Some((base, _)) => base,
                None => {
                    let base = mesh.positions.len();
                    let positions = raw(self.root, self.buffers, pos, charge)?;
                    Counts::add(
                        &mut self.read.vertices,
                        "vertices",
                        positions.count,
                        caps::MAX_VERTICES,
                    )?;
                    let positions = positions.floats::<3>("positions")?;
                    let read =
                        |s: Semantic,
                         charge: &mut Charge|
                         -> Result<Option<accessor::Raw<'_>>, ImportError> {
                            let Some(a) = attr(s) else {
                                return Ok(None);
                            };
                            let r = raw(self.root, self.buffers, a, charge)?;
                            if r.count != n {
                                return malformed(format!(
                                    "an attribute of {} vertices beside {n} positions",
                                    r.count
                                ));
                            }
                            Ok(Some(r))
                        };
                    let normals = match read(Semantic::Normals, charge)? {
                        Some(r) => Some(r.floats::<3>("normals")?),
                        None => None,
                    };
                    let uv = |set: u32,
                              charge: &mut Charge|
                     -> Result<Option<Vec<[f32; 2]>>, ImportError> {
                        match read(Semantic::TexCoords(set), charge)? {
                            Some(r) => Ok(Some(r.floats::<2>("texture coordinates")?)),
                            None => Ok(None),
                        }
                    };
                    let uv0 = uv(0, charge)?;
                    let uv1 = uv(1, charge)?;
                    let colors = match read(Semantic::Colors(0), charge)? {
                        Some(r) if r.dims == 3 => Some(
                            r.floats::<3>("colors")?
                                .into_iter()
                                .map(|c| [c[0], c[1], c[2], 1.0])
                                .collect(),
                        ),
                        Some(r) => Some(r.floats::<4>("colors")?),
                        None => None,
                    };
                    let joints = match read(Semantic::Joints(0), charge)? {
                        Some(r) => Some(r.joints()?),
                        None => None,
                    };
                    let weights = match read(Semantic::Weights(0), charge)? {
                        Some(r) => Some(r.floats::<4>("weights")?),
                        None => None,
                    };
                    // Arrays any primitive has are filled for every vertex of the mesh.
                    fill(&mut mesh.uv0, base, uv0, n, [0.0; 2]);
                    fill(&mut mesh.uv1, base, uv1, n, [0.0; 2]);
                    fill(&mut mesh.colors, base, colors, n, [1.0; 4]);
                    fill(&mut mesh.joints, base, joints, n, [0; 4]);
                    fill(&mut mesh.weights, base, weights, n, [1.0, 0.0, 0.0, 0.0]);
                    let normals = normals.unwrap_or_else(|| smooth_normals(&positions, &tris));
                    mesh.positions
                        .extend(positions.iter().map(|&v| frame.point(v)));
                    mesh.normals.extend(normals.iter().map(|&v| frame.dir(v)));
                    shared.push((key, base, n));
                    base
                }
            };
            let material = match p.material {
                Some(m) if m.value() < materials => Some(m.value()),
                Some(_) => return malformed("a primitive's material"),
                None => None,
            };
            mesh.primitives.push(Primitive {
                material,
                indices: tris
                    .chunks_exact(3)
                    .flat_map(|t| [t[0], t[2], t[1]].map(|v| v + base as u32))
                    .collect(),
            });
        }
        Ok(())
    }

    fn skin(
        &mut self,
        mesh: &mut Mesh,
        index: usize,
        charge: &mut Charge,
    ) -> Result<(), ImportError> {
        let Some(s) = self.root.skins.get(index) else {
            return malformed(format!("skin {index}"));
        };
        cap("joints of a skin", s.joints.len(), caps::MAX_SKIN_JOINTS)?;
        Counts::add(
            &mut self.read.joints,
            "joints of all skins",
            s.joints.len(),
            caps::MAX_SKIN_JOINTS_TOTAL,
        )?;
        let mut joints = Vec::with_capacity(s.joints.len());
        for j in &s.joints {
            match self.node_map.get(j.value()).copied().flatten() {
                Some(n) => joints.push(n),
                None => return malformed(format!("skin {index}'s joint")),
            }
        }
        let inverse_binds = match s.inverse_bind_matrices {
            Some(a) => {
                let m = raw(self.root, self.buffers, a.value(), charge)?.matrices()?;
                if m.len() != joints.len() {
                    return malformed(format!(
                        "skin {index} has {} matrices for {} joints",
                        m.len(),
                        joints.len()
                    ));
                }
                m.iter().map(|m| self.frame.matrix(m)).collect()
            }
            None => vec![math::IDENTITY; joints.len()],
        };
        let n = mesh.positions.len();
        if mesh.joints.len() != n || mesh.weights.len() != n {
            mesh.joints = vec![[0; 4]; n];
            mesh.weights = vec![[1.0, 0.0, 0.0, 0.0]; n];
        }
        if mesh
            .joints
            .iter()
            .flatten()
            .any(|&j| usize::from(j) >= joints.len())
        {
            return malformed(format!(
                "{}: a joint slot past skin {index}'s joints",
                mesh.name
            ));
        }
        for w in &mut mesh.weights {
            let sum: f32 = w.iter().map(|x| x.max(0.0)).sum();
            *w = if sum > 1e-12 && sum.is_finite() {
                w.map(|x| x.max(0.0) / sum)
            } else {
                [1.0, 0.0, 0.0, 0.0]
            };
        }
        mesh.skin = Some(Skin {
            joints,
            inverse_binds,
        });
        Ok(())
    }

    fn animations(&mut self, charge: &mut Charge) -> Result<(), ImportError> {
        use json::animation::{Interpolation, Property};
        let mut samples = 0usize;
        for (ai, a) in self.root.animations.iter().enumerate() {
            cap(
                "channels of an animation",
                a.channels.len(),
                caps::MAX_CHANNELS,
            )?;
            let name = a
                .name
                .as_deref()
                .map_or_else(|| format!("anim{ai}"), short_name);
            let extras = torque_extras(&a.extras).filter(|_| self.exact.is_some());
            let mut curves: Vec<(usize, Property, Curve)> = Vec::new();
            for ch in &a.channels {
                let Checked::Valid(path) = ch.target.path else {
                    return malformed("a channel's path");
                };
                if path == Property::MorphTargetWeights {
                    continue;
                }
                let Some(node) = self.node_map.get(ch.target.node.value()).copied().flatten()
                else {
                    continue;
                };
                let Some(s) = a.samplers.get(ch.sampler.value()) else {
                    return malformed("a channel's sampler");
                };
                let interp = match s.interpolation {
                    Checked::Valid(Interpolation::Step) => Interp::Step,
                    Checked::Valid(Interpolation::CubicSpline) => Interp::Cubic,
                    _ => Interp::Linear,
                };
                let input = raw(self.root, self.buffers, s.input.value(), charge)?;
                Counts::add(
                    &mut self.read.keys,
                    "key times and values",
                    input.count,
                    caps::MAX_KEY_ELEMENTS,
                )?;
                let times: Vec<f32> = input
                    .floats::<1>("key times")?
                    .into_iter()
                    .map(|t| t[0])
                    .collect();
                if times.iter().any(|t| !t.is_finite()) || times.windows(2).any(|w| w[1] < w[0]) {
                    return malformed(format!("{name}: key times out of order"));
                }
                let out = raw(self.root, self.buffers, s.output.value(), charge)?;
                Counts::add(
                    &mut self.read.keys,
                    "key times and values",
                    out.count,
                    caps::MAX_KEY_ELEMENTS,
                )?;
                let per = if interp == Interp::Cubic { 3 } else { 1 };
                if out.count != times.len() * per {
                    return malformed(format!(
                        "{name}: {} values for {} keys",
                        out.count,
                        times.len()
                    ));
                }
                let frame = self.frame;
                let values: Vec<[f32; 4]> = match path {
                    Property::Rotation => out.floats::<4>("rotations")?,
                    _ => out
                        .floats::<3>("moves")?
                        .into_iter()
                        .map(|v| [v[0], v[1], v[2], 0.0])
                        .collect(),
                };
                let values = values
                    .into_iter()
                    .map(|v| match path {
                        Property::Rotation => {
                            let d = frame.dir([v[0], v[1], v[2]]);
                            [d[0], d[1], d[2], v[3]]
                        }
                        Property::Translation => {
                            let p = frame.point([v[0], v[1], v[2]]);
                            [p[0], p[1], p[2], 0.0]
                        }
                        _ => {
                            let s = frame.scale([v[0], v[1], v[2]]);
                            [s[0], s[1], s[2], 0.0]
                        }
                    })
                    .collect();
                curves.push((
                    node,
                    path,
                    Curve {
                        times,
                        values,
                        interp,
                        rotation: path == Property::Rotation,
                    },
                ));
            }
            let end = curves
                .iter()
                .filter_map(|c| c.2.times.last().copied())
                .fold(0.0f32, f32::max);
            let duration = extras
                .as_ref()
                .and_then(|e| get_f32(e, "duration"))
                .unwrap_or(end);
            if !(0.0..=caps::MAX_DURATION).contains(&duration) {
                return malformed(format!("{name}: a duration of {duration} seconds"));
            }
            let (mut keys, mut cyclic) = match &extras {
                Some(e) => (
                    get_usize(e, "keys").unwrap_or(1),
                    e.get("cyclic").and_then(Value::as_bool).unwrap_or(false),
                ),
                None => ((duration * self.options.rate + 0.5) as usize + 1, false),
            };
            cap("keys of an animation", keys, caps::MAX_KEYS)?;
            keys = keys.max(1);
            samples = samples.saturating_add(keys.saturating_mul(curves.len()));
            cap("sampled values", samples, caps::MAX_SAMPLES)?;
            pay(charge, (keys * curves.len()) as u64)?;
            if extras.is_none() && keys > 2 && duration > 0.0 {
                // A loop: every curve ends where it starts. Its last key is its first again, so it is dropped.
                let same = |a: [f32; 4], b: [f32; 4], rot: bool| {
                    let d = |s: f32| (0..4).all(|i| (a[i] - s * b[i]).abs() <= 1e-4);
                    d(1.0) || (rot && d(-1.0))
                };
                if curves
                    .iter()
                    .all(|c| same(c.2.sample(0.0), c.2.sample(duration), c.2.rotation))
                {
                    cyclic = true;
                    keys -= 1;
                }
            }
            let mut anim = Animation {
                name,
                duration,
                keys,
                cyclic,
                ..Animation::default()
            };
            if let Some(e) = &extras {
                anim.blend = e.get("blend").and_then(Value::as_bool).unwrap_or(false);
                anim.priority = e
                    .get("priority")
                    .and_then(Value::as_i64)
                    .and_then(|p| i32::try_from(p).ok())
                    .unwrap_or(0);
                anim.ground = get_vec3s(e, "ground", caps::MAX_KEYS).unwrap_or_default();
            }
            let key_times: Vec<f32> = (0..keys).map(|k| anim.key_time(k)).collect();
            for (node, path, curve) in &curves {
                // A file this crate wrote has one value a key (and the first again at the end of a loop): taken as it is.
                let direct = extras.is_some()
                    && curve.interp != Interp::Cubic
                    && curve.times.len() == keys + usize::from(cyclic);
                let value = |k: usize| {
                    if direct {
                        curve.key(k)
                    } else {
                        curve.sample(key_times[k])
                    }
                };
                let track = match anim.tracks.iter().position(|t| t.node == *node) {
                    Some(t) => t,
                    None => {
                        anim.tracks.push(Track {
                            node: *node,
                            ..Track::default()
                        });
                        anim.tracks.len() - 1
                    }
                };
                let t = &mut anim.tracks[track];
                match path {
                    Property::Rotation => {
                        t.rotations = (0..keys)
                            .map(|k| {
                                let q = value(k);
                                if direct { q } else { math::normalize_quat(q) }
                            })
                            .collect();
                    }
                    Property::Translation => {
                        t.translations = (0..keys)
                            .map(|k| {
                                let v = value(k);
                                [v[0], v[1], v[2]]
                            })
                            .collect()
                    }
                    _ => {
                        t.scales = (0..keys)
                            .map(|k| {
                                let v = value(k);
                                [v[0], v[1], v[2]]
                            })
                            .collect()
                    }
                }
            }
            anim.tracks.sort_by_key(|t| t.node);
            self.model.animations.push(anim);
        }
        Ok(())
    }
}

/// `values` (or `blank` for each of `n` vertices when there are none) appended to `out`, which first gets `blank` for the
/// `base` vertices before when it is empty and this primitive has values.
fn fill<T: Copy>(out: &mut Vec<T>, base: usize, values: Option<Vec<T>>, n: usize, blank: T) {
    match values {
        Some(v) => {
            if out.len() < base {
                out.resize(base, blank);
            }
            out.extend(v);
        }
        None if !out.is_empty() => out.extend(std::iter::repeat_n(blank, n)),
        None => {}
    }
}

/// The triangles (three indices each, counter-clockwise as glTF has them) of an index list drawn as `mode`.
fn triangles(list: &[u32], mode: json::mesh::Mode) -> Vec<u32> {
    use json::mesh::Mode;
    match mode {
        Mode::TriangleStrip => (2..list.len())
            .flat_map(|i| {
                if i.is_multiple_of(2) {
                    [list[i - 2], list[i - 1], list[i]]
                } else {
                    [list[i - 1], list[i - 2], list[i]]
                }
            })
            .collect(),
        Mode::TriangleFan => (2..list.len())
            .flat_map(|i| [list[0], list[i - 1], list[i]])
            .collect(),
        _ => list[..list.len() / 3 * 3].to_vec(),
    }
}

/// Each vertex's normal: the sum of its triangles' area-weighted normals (counter-clockwise fronts), at unit length.
fn smooth_normals(positions: &[Vec3], tris: &[u32]) -> Vec<Vec3> {
    let mut sums = vec![glam::Vec3::ZERO; positions.len()];
    for t in tris.chunks_exact(3) {
        let [a, b, c] = [t[0], t[1], t[2]]
            .map(|i| glam::Vec3::from(positions.get(i as usize).copied().unwrap_or_default()));
        let n = (b - a).cross(c - a);
        for &i in t {
            if let Some(s) = sums.get_mut(i as usize) {
                *s += n;
            }
        }
    }
    sums.into_iter()
        .map(|s| {
            let l = s.length();
            if l > 1e-20 {
                (s / l).to_array()
            } else {
                [0.0, 0.0, 1.0]
            }
        })
        .collect()
}

pub use crate::add_box_hull;
