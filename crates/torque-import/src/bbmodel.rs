//! Blockbench (`.bbmodel`): cubes, meshes, groups and embedded textures into the model layer.
//!
//! **What is read:** every element whose `export` is not false: cubes (`from`, `to`, `inflate`, a turn by `rotation` about
//! `origin`, and their faces: `north`, `east`, `south`, `west`, `up`, `down`, each with its pixel `uv` rectangle, its
//! `rotation` of 0, 90, 180 or 270 degrees and its `texture`), and meshes (`vertices` about `origin`, turned by `rotation`,
//! and faces of three or more of them, each corner with its pixel `uv`). The outliner's groups are nodes (each turned by its
//! `rotation` about its `origin`, inside its parent's turn); an element is a mesh on its group's node (or a root node), named
//! as the element is. Textures carried as `data:` PNGs are the images, one material each (named after the texture's file name
//! without its extension); a face of no texture takes the `default` material. Animations are not read (a note says so).
//!
//! **Frames.** Blockbench is y up with 16 units a block: a point `(x, y, z)` is Torque's `(x, -z, y) / 16` times
//! [`Options::scale`] (so a block is a meter, as Blockbench's own glTF export makes it). Turns are Euler angles in degrees,
//! applied x, then y, then z. A face's corners run clockwise seen from outside, as a shape's triangles do; a mesh face's
//! corners (stored counter-clockwise, a quad's in any order) are sorted round their middle. Pixel coordinates are divided by
//! the texture's `uv_width` and `uv_height` (else the project's `resolution`, else 16).
//!
//! **Hostile input.** The file is JSON: its size is capped ([`Options::max_json_bytes`]) and its values counted before it is
//! parsed; elements, a mesh's vertices and faces, a face's corners, groups, the outliner's depth, textures and triangles are
//! capped; every number must be finite and coordinates within [`MAX_COORD`]; textures are base64 `data:` PNGs or JPEGs whose
//! decoded size and header size are capped (never decoded here). What a bound refuses is an [`ImportError`], never a panic.

use std::collections::{HashMap, HashSet};

use serde_json::{Map, Value};
use torque_modelcore::math::{self, Quat, Vec3};
use torque_modelcore::{Detail, Image, Material, Mesh, Model, Node, Primitive, Source};

use crate::gltf::container::{base64, image_size, json_values};
use crate::{Charge, ImportError, Options, cap, caps, pay};

/// The most elements of a file.
pub const MAX_ELEMENTS: usize = 16_384;
/// The most vertices, and faces, of one mesh element.
pub const MAX_MESH_VERTICES: usize = 65_536;
pub const MAX_MESH_FACES: usize = 65_536;
/// The most corners of one mesh face.
pub const MAX_FACE_CORNERS: usize = 64;
/// The most groups, and the deepest group.
pub const MAX_GROUPS: usize = 16_384;
pub const MAX_DEPTH: usize = caps::MAX_NODE_DEPTH;
/// The farthest coordinate, in Blockbench units.
pub const MAX_COORD: f64 = 1.0e6;
/// The largest texture carried, decoded, and the widest or tallest.
pub const MAX_TEXTURE_BYTES: usize = 16 << 20;
pub const MAX_TEXTURE_SIDE: u32 = 4_096;

/// Blockbench units a meter (16 a block, a block a meter).
pub const UNITS_PER_METER: f32 = 16.0;

fn malformed<T>(what: impl Into<String>) -> Result<T, ImportError> {
    Err(ImportError::Malformed(what.into()))
}

/// A name cut to [`caps::MAX_NAME_BYTES`].
fn short(name: &str) -> String {
    short_to(name, caps::MAX_NAME_BYTES).to_string()
}

/// `name` cut to at most `max` bytes, at a character's end.
fn short_to(name: &str, max: usize) -> &str {
    let mut end = name.len().min(max);
    while !name.is_char_boundary(end) {
        end -= 1;
    }
    &name[..end]
}

fn num(v: &Value, what: &str) -> Result<f64, ImportError> {
    match v.as_f64() {
        Some(x) if x.is_finite() && x.abs() <= MAX_COORD => Ok(x),
        _ => malformed(format!("{what}: not a finite number within {MAX_COORD}")),
    }
}

/// An array of `N` numbers at `key`, or `default` when the key is absent or null.
fn nums<const N: usize>(
    o: &Map<String, Value>,
    key: &str,
    default: [f64; N],
) -> Result<[f64; N], ImportError> {
    let Some(v) = o.get(key).filter(|v| !v.is_null()) else {
        return Ok(default);
    };
    let Some(a) = v.as_array().filter(|a| a.len() >= N) else {
        return malformed(format!("{key}: not {N} numbers"));
    };
    let mut out = [0.0; N];
    for k in 0..N {
        out[k] = num(&a[k], key)?;
    }
    Ok(out)
}

type V3 = [f64; 3];

/// A turn by Euler angles in degrees (x first, then y, then z) as a 3 x 3 matrix, rows.
fn euler(deg: V3) -> [[f64; 3]; 3] {
    let [x, y, z] = deg.map(f64::to_radians);
    let (sx, cx) = x.sin_cos();
    let (sy, cy) = y.sin_cos();
    let (sz, cz) = z.sin_cos();
    let rx = [[1.0, 0.0, 0.0], [0.0, cx, -sx], [0.0, sx, cx]];
    let ry = [[cy, 0.0, sy], [0.0, 1.0, 0.0], [-sy, 0.0, cy]];
    let rz = [[cz, -sz, 0.0], [sz, cz, 0.0], [0.0, 0.0, 1.0]];
    mul3(&rz, &mul3(&ry, &rx))
}

fn mul3(a: &[[f64; 3]; 3], b: &[[f64; 3]; 3]) -> [[f64; 3]; 3] {
    let mut out = [[0.0; 3]; 3];
    for (r, row) in out.iter_mut().enumerate() {
        for (c, v) in row.iter_mut().enumerate() {
            *v = (0..3).map(|k| a[r][k] * b[k][c]).sum();
        }
    }
    out
}

fn apply(m: &[[f64; 3]; 3], v: V3) -> V3 {
    [0, 1, 2].map(|r| m[r][0] * v[0] + m[r][1] * v[1] + m[r][2] * v[2])
}

/// `p` turned by `deg` about `origin`.
fn turn(p: V3, origin: V3, deg: V3) -> V3 {
    if deg == [0.0; 3] {
        return p;
    }
    let r = apply(&euler(deg), [0, 1, 2].map(|k| p[k] - origin[k]));
    [0, 1, 2].map(|k| r[k] + origin[k])
}

/// The turn of Euler angles as a quaternion `[x, y, z, w]`.
fn quat(deg: V3) -> Quat {
    let q = math::quat_of(&{
        let m = euler(deg);
        let mut out = math::IDENTITY;
        for r in 0..3 {
            for c in 0..3 {
                out[r * 4 + c] = m[r][c] as f32;
            }
        }
        out
    });
    math::normalize_quat(q)
}

/// A cube face's corners clockwise seen from outside: top left, top right, bottom right, bottom left (as its `uv` rectangle
/// runs), the cube from `a` to `b`.
fn cube_face(face: &str, a: V3, b: V3) -> Option<[V3; 4]> {
    let ([x0, y0, z0], [x1, y1, z1]) = (a, b);
    Some(match face {
        "north" => [[x1, y1, z0], [x0, y1, z0], [x0, y0, z0], [x1, y0, z0]],
        "south" => [[x0, y1, z1], [x1, y1, z1], [x1, y0, z1], [x0, y0, z1]],
        "east" => [[x1, y1, z1], [x1, y1, z0], [x1, y0, z0], [x1, y0, z1]],
        "west" => [[x0, y1, z0], [x0, y1, z1], [x0, y0, z1], [x0, y0, z0]],
        "up" => [[x0, y1, z0], [x1, y1, z0], [x1, y1, z1], [x0, y1, z1]],
        "down" => [[x0, y0, z1], [x1, y0, z1], [x1, y0, z0], [x0, y0, z0]],
        _ => return None,
    })
}

/// A texture's pixel size for coordinates.
#[derive(Clone, Copy)]
struct UvSize([f64; 2]);

struct Reader<'a> {
    options: &'a Options,
    resolution: [f64; 2],
    /// Per texture: its material and its coordinate size.
    textures: Vec<(usize, UvSize)>,
    uuids: HashMap<String, usize>,
    default_material: Option<usize>,
    model: Model,
    triangles: usize,
}

impl Reader<'_> {
    fn point(&self, p: V3, origin: V3) -> Vec3 {
        let s = f64::from(self.options.scale) / f64::from(UNITS_PER_METER);
        let q = [0, 1, 2].map(|k| (p[k] - origin[k]) * s);
        let t = if self.options.up == 'z' {
            q
        } else {
            [q[0], -q[2], q[1]]
        };
        t.map(|v| v as f32)
    }

    fn dir_quat(&self, q: Quat) -> Quat {
        if self.options.up == 'z' {
            q
        } else {
            [q[0], -q[2], q[1], q[3]]
        }
    }

    /// The material and the coordinate size of a face's `texture`.
    fn texture(&mut self, face: &Map<String, Value>) -> Result<(usize, UvSize), ImportError> {
        let found = match face.get("texture") {
            Some(Value::Number(n)) => n
                .as_u64()
                .and_then(|i| self.textures.get(i as usize))
                .copied(),
            Some(Value::String(s)) => self
                .uuids
                .get(s)
                .and_then(|&i| self.textures.get(i))
                .copied(),
            _ => None,
        };
        if let Some(t) = found {
            return Ok(t);
        }
        if matches!(
            face.get("texture"),
            Some(Value::Number(_) | Value::String(_))
        ) {
            return malformed("a face names a texture the file does not have");
        }
        let m = match self.default_material {
            Some(m) => m,
            None => {
                cap(
                    "materials",
                    self.model.materials.len() + 1,
                    caps::MAX_MATERIALS,
                )?;
                self.model.materials.push(Material {
                    name: "default".into(),
                    ..Material::default()
                });
                self.default_material = Some(self.model.materials.len() - 1);
                self.model.materials.len() - 1
            }
        };
        Ok((m, UvSize(self.resolution)))
    }

    /// Adds a polygon (clockwise seen from outside, Blockbench's space) of `material` to `mesh` as a fan, its corners' pixel
    /// coordinates divided by `size`.
    fn polygon(
        &mut self,
        mesh: &mut Mesh,
        by_material: &mut HashMap<usize, usize>,
        (material, size): (usize, UvSize),
        corners: &[(V3, [f64; 2])],
        origin: V3,
        charge: &mut Charge,
    ) -> Result<(), ImportError> {
        if corners.len() < 3 {
            return Ok(());
        }
        self.triangles += corners.len() - 2;
        cap("triangles", self.triangles, caps::MAX_INDICES / 3)?;
        cap(
            "vertices",
            mesh.positions.len() + corners.len(),
            caps::MAX_VERTICES,
        )?;
        pay(charge, corners.len() as u64)?;
        let base = mesh.positions.len() as u32;
        for (p, uv) in corners {
            mesh.positions.push(self.point(*p, origin));
            mesh.uv0
                .push([(uv[0] / size.0[0]) as f32, (uv[1] / size.0[1]) as f32]);
        }
        let prim = *by_material.entry(material).or_insert_with(|| {
            mesh.primitives.push(Primitive {
                material: Some(material),
                indices: Vec::new(),
            });
            mesh.primitives.len() - 1
        });
        let list = &mut mesh.primitives[prim].indices;
        for k in 1..corners.len() as u32 - 1 {
            list.extend_from_slice(&[base, base + k, base + k + 1]);
        }
        Ok(())
    }

    fn cube(
        &mut self,
        e: &Map<String, Value>,
        mesh: &mut Mesh,
        origin_of_node: V3,
        charge: &mut Charge,
    ) -> Result<(), ImportError> {
        let inflate = e.get("inflate").map_or(Ok(0.0), |v| {
            if v.is_null() {
                Ok(0.0)
            } else {
                num(v, "inflate")
            }
        })?;
        let from = nums::<3>(e, "from", [0.0; 3])?.map(|v| v - inflate);
        let to = nums::<3>(e, "to", [0.0; 3])?.map(|v| v + inflate);
        let (a, b) = (
            [0, 1, 2].map(|k| from[k].min(to[k])),
            [0, 1, 2].map(|k| from[k].max(to[k])),
        );
        let origin = nums::<3>(e, "origin", [0.0; 3])?;
        let rotation = nums::<3>(e, "rotation", [0.0; 3])?;
        let Some(faces) = e.get("faces").and_then(Value::as_object) else {
            return Ok(());
        };
        let mut by_material = HashMap::new();
        for name in ["north", "east", "south", "west", "up", "down"] {
            let Some(face) = faces.get(name).and_then(Value::as_object) else {
                continue;
            };
            let Some(corners) = cube_face(name, a, b) else {
                continue;
            };
            let (material, size) = self.texture(face)?;
            let [u1, v1, u2, v2] = nums::<4>(face, "uv", [0.0, 0.0, size.0[0], size.0[1]])?;
            let mut uvs = [[u1, v1], [u2, v1], [u2, v2], [u1, v2]];
            let turns = match face.get("rotation") {
                None | Some(Value::Null) => 0,
                Some(v) => {
                    let r = num(v, "a face's rotation")?;
                    if r.rem_euclid(90.0) != 0.0 {
                        return malformed("a face's rotation is not a multiple of 90");
                    }
                    (r / 90.0).rem_euclid(4.0) as usize
                }
            };
            uvs.rotate_right(turns);
            let list: Vec<(V3, [f64; 2])> = corners
                .iter()
                .zip(uvs)
                .map(|(&p, uv)| (turn(p, origin, rotation), uv))
                .collect();
            self.polygon(
                mesh,
                &mut by_material,
                (material, size),
                &list,
                origin_of_node,
                charge,
            )?;
        }
        Ok(())
    }

    fn free_mesh(
        &mut self,
        e: &Map<String, Value>,
        mesh: &mut Mesh,
        origin_of_node: V3,
        charge: &mut Charge,
    ) -> Result<(), ImportError> {
        let origin = nums::<3>(e, "origin", [0.0; 3])?;
        let rotation = nums::<3>(e, "rotation", [0.0; 3])?;
        let empty = Map::new();
        let vertices = e
            .get("vertices")
            .and_then(Value::as_object)
            .unwrap_or(&empty);
        let faces = e.get("faces").and_then(Value::as_object).unwrap_or(&empty);
        cap("vertices of a mesh", vertices.len(), MAX_MESH_VERTICES)?;
        cap("faces of a mesh", faces.len(), MAX_MESH_FACES)?;
        pay(charge, (vertices.len() + faces.len()) as u64)?;
        let mut placed: HashMap<&str, V3> = HashMap::with_capacity(vertices.len());
        for (k, v) in vertices {
            let Some(a) = v.as_array().filter(|a| a.len() >= 3) else {
                return malformed("a mesh vertex is not three numbers");
            };
            let p = [
                num(&a[0], "a vertex")?,
                num(&a[1], "a vertex")?,
                num(&a[2], "a vertex")?,
            ];
            let at = [0, 1, 2].map(|i| p[i] + origin[i]);
            placed.insert(k.as_str(), turn(at, origin, rotation));
        }
        let mut by_material = HashMap::new();
        for face in faces.values() {
            let Some(face) = face.as_object() else {
                return malformed("a mesh face is not an object");
            };
            let Some(keys) = face.get("vertices").and_then(Value::as_array) else {
                return malformed("a mesh face without vertices");
            };
            cap("corners of a face", keys.len(), MAX_FACE_CORNERS)?;
            if keys.len() < 3 {
                continue;
            }
            let (material, size) = self.texture(face)?;
            let uv = face.get("uv").and_then(Value::as_object);
            let mut corners = Vec::with_capacity(keys.len());
            for k in keys {
                let Some(k) = k.as_str() else {
                    return malformed("a face corner is not a vertex key");
                };
                let Some(&p) = placed.get(k) else {
                    return malformed("a face corner names no vertex");
                };
                let t = match uv.and_then(|u| u.get(k)) {
                    Some(Value::Array(a)) if a.len() >= 2 => {
                        [num(&a[0], "a uv")?, num(&a[1], "a uv")?]
                    }
                    _ => [0.0, 0.0],
                };
                corners.push((p, t));
            }
            sort_round(&mut corners);
            // Stored counter-clockwise from the front: clockwise is the other way round.
            corners.reverse();
            self.polygon(
                mesh,
                &mut by_material,
                (material, size),
                &corners,
                origin_of_node,
                charge,
            )?;
        }
        Ok(())
    }
}

/// A face's corners in order round their middle, turning as its first three corners do (Blockbench keeps a quad's corners in
/// any order).
fn sort_round(c: &mut [(V3, [f64; 2])]) {
    if c.len() < 4 {
        return;
    }
    let mid = [0, 1, 2].map(|k| c.iter().map(|x| x.0[k]).sum::<f64>() / c.len() as f64);
    let sub = |a: V3, b: V3| [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    let cross = |a: V3, b: V3| {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    };
    let dot = |a: V3, b: V3| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let n = cross(sub(c[1].0, c[0].0), sub(c[2].0, c[0].0));
    let u = sub(c[0].0, mid);
    let w = cross(n, u);
    if dot(u, u) == 0.0 || dot(w, w) == 0.0 {
        return;
    }
    c.sort_by(|a, b| {
        let angle = |p: V3| {
            let d = sub(p, mid);
            dot(d, w).atan2(dot(d, u))
        };
        angle(a.0).total_cmp(&angle(b.0))
    });
}

/// Reads a `.bbmodel` (module docs).
pub fn read(bytes: &[u8], options: &Options, mut charge: Charge) -> Result<Model, ImportError> {
    let charge = &mut charge;
    if bytes.len() > options.max_json_bytes {
        return Err(ImportError::TooLarge {
            what: "the .bbmodel",
            bytes: bytes.len() as u64,
            cap: options.max_json_bytes as u64,
        });
    }
    if !(options.scale.is_finite() && options.scale > 0.0) {
        return Err(ImportError::Invalid(
            "a scale that is not a positive number".into(),
        ));
    }
    pay(charge, bytes.len() as u64)?;
    let values = json_values(bytes, caps::MAX_JSON_VALUES);
    cap("values in the JSON", values, caps::MAX_JSON_VALUES)?;
    pay(charge, values as u64)?;
    let root: Value = serde_json::from_slice(bytes).map_err(|e| {
        let mut s = e.to_string();
        s.truncate(200);
        ImportError::Malformed(s)
    })?;
    let Some(root) = root.as_object() else {
        return malformed("not a JSON object");
    };
    let res = root.get("resolution").and_then(Value::as_object);
    let side = |k: &str| {
        res.and_then(|r| r.get(k))
            .and_then(Value::as_f64)
            .filter(|v| v.is_finite() && *v >= 1.0 && *v <= 65_536.0)
            .unwrap_or(16.0)
    };
    let mut r = Reader {
        options,
        resolution: [side("width"), side("height")],
        textures: Vec::new(),
        uuids: HashMap::new(),
        default_material: None,
        model: Model {
            source: Source {
                format: "Blockbench".into(),
                up: if options.up == 'z' { 'z' } else { 'y' },
                scale: options.scale / UNITS_PER_METER,
            },
            ..Model::default()
        },
        triangles: 0,
    };
    // Textures: one image and one material each.
    let empty = Vec::new();
    let textures = root
        .get("textures")
        .and_then(Value::as_array)
        .unwrap_or(&empty);
    cap("textures", textures.len(), caps::MAX_IMAGES)?;
    let mut image_bytes = 0usize;
    // Names taken (lowercase: a file system may not tell case apart), and each name's next number to try.
    let mut taken: HashSet<String> = HashSet::new();
    let mut next: HashMap<String, usize> = HashMap::new();
    for (i, t) in textures.iter().enumerate() {
        let Some(t) = t.as_object() else {
            return malformed(format!("texture {i} is not an object"));
        };
        let raw = t.get("name").and_then(Value::as_str).unwrap_or("texture");
        let file = raw.rsplit(['/', '\\']).next().unwrap_or(raw);
        let base: String = file
            .rsplit_once('.')
            .map_or(file, |(a, _)| a)
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let base = short(if base.is_empty() || base.starts_with('.') {
            "texture"
        } else {
            &base
        });
        // Two textures of one name: the later ones numbered, past any name already taken.
        let mut name = base.clone();
        while taken.contains(&name.to_ascii_lowercase()) {
            let n = next.entry(base.to_ascii_lowercase()).or_insert(2);
            let suffix = format!("_{n}");
            *n += 1;
            name = format!(
                "{}{suffix}",
                short_to(&base, caps::MAX_NAME_BYTES.saturating_sub(suffix.len()))
            );
        }
        taken.insert(name.to_ascii_lowercase());
        let mut image = Image {
            name: name.clone(),
            ..Image::default()
        };
        if let Some(src) = t.get("source").and_then(Value::as_str) {
            let Some(rest) = src.strip_prefix("data:") else {
                return Err(ImportError::Unsupported(format!(
                    "texture {i}: a source that is not a data: URI"
                )));
            };
            let Some((head, data)) = rest.split_once(',') else {
                return malformed(format!("texture {i}: a data: URI without a comma"));
            };
            if !head.ends_with(";base64") {
                return Err(ImportError::Unsupported(format!(
                    "texture {i}: a data: URI that is not base64"
                )));
            }
            let decoded = base64(data.as_bytes(), MAX_TEXTURE_BYTES)?;
            pay(charge, decoded.len() as u64)?;
            image_bytes += decoded.len();
            if image_bytes > caps::MAX_IMAGE_BYTES_TOTAL {
                return Err(ImportError::TooLarge {
                    what: "the textures together",
                    bytes: image_bytes as u64,
                    cap: caps::MAX_IMAGE_BYTES_TOTAL as u64,
                });
            }
            let Some((w, h)) = image_size(&decoded) else {
                return Err(ImportError::Unsupported(format!(
                    "texture {i} is not a PNG or a JPEG"
                )));
            };
            if w == 0 || h == 0 || w > MAX_TEXTURE_SIDE || h > MAX_TEXTURE_SIDE {
                return Err(ImportError::TooLarge {
                    what: "a texture's side",
                    bytes: u64::from(w.max(h)),
                    cap: u64::from(MAX_TEXTURE_SIDE),
                });
            }
            image.mime = if decoded.starts_with(&[0xff, 0xd8]) {
                "image/jpeg".into()
            } else {
                "image/png".into()
            };
            image.size = Some((w, h));
            image.data = Some(decoded);
        } else {
            r.model.notes.push(format!(
                "texture {name}: not carried in the file, named only"
            ));
            image.uri = Some(format!("{name}.png"));
        }
        let uv = |k: &str, fallback: f64| {
            t.get(k)
                .and_then(Value::as_f64)
                .filter(|v| v.is_finite() && *v >= 1.0 && *v <= 65_536.0)
                .unwrap_or(fallback)
        };
        let size = UvSize([
            uv("uv_width", r.resolution[0]),
            uv("uv_height", r.resolution[1]),
        ]);
        r.model.images.push(image);
        r.model.materials.push(Material {
            name,
            base_texture: Some(r.model.images.len() - 1),
            ..Material::default()
        });
        r.textures.push((r.model.materials.len() - 1, size));
        for key in ["uuid", "id"] {
            if let Some(id) = t.get(key).and_then(Value::as_str) {
                r.uuids.insert(short(id), i);
            }
        }
    }
    // Elements by uuid.
    let elements = root
        .get("elements")
        .and_then(Value::as_array)
        .unwrap_or(&empty);
    cap("elements", elements.len(), MAX_ELEMENTS)?;
    let mut by_uuid: HashMap<&str, usize> = HashMap::new();
    for (i, e) in elements.iter().enumerate() {
        if let Some(u) = e.get("uuid").and_then(Value::as_str) {
            by_uuid.entry(u).or_insert(i);
        }
    }
    // The outliner: groups as nodes, depth first; each element on the first group that lists it.
    let mut placed_on: Vec<Option<(usize, V3)>> = vec![None; elements.len()];
    r.model.nodes.push(Node::new("root", None, [0.0; 3]));
    let outliner = root
        .get("outliner")
        .and_then(Value::as_array)
        .unwrap_or(&empty);
    // (entry, parent node, parent's origin, depth)
    let mut stack: Vec<(&Value, usize, V3, usize)> =
        outliner.iter().rev().map(|v| (v, 0, [0.0; 3], 1)).collect();
    let mut groups = 0usize;
    while let Some((entry, parent, parent_origin, depth)) = stack.pop() {
        pay(charge, 1)?;
        match entry {
            Value::String(u) => {
                if let Some(&i) = by_uuid.get(u.as_str())
                    && placed_on[i].is_none()
                {
                    placed_on[i] = Some((parent, parent_origin));
                }
            }
            Value::Object(g) => {
                groups += 1;
                cap("groups", groups, MAX_GROUPS)?;
                cap("group depth", depth, MAX_DEPTH)?;
                if g.get("export").and_then(Value::as_bool) == Some(false) {
                    continue;
                }
                let origin = nums::<3>(g, "origin", [0.0; 3])?;
                let rotation = nums::<3>(g, "rotation", [0.0; 3])?;
                let name = g.get("name").and_then(Value::as_str).unwrap_or("group");
                cap("nodes", r.model.nodes.len() + 1, caps::MAX_NODES)?;
                let mut node =
                    Node::new(&short(name), Some(parent), r.point(origin, parent_origin));
                node.rotation = r.dir_quat(quat(rotation));
                r.model.nodes.push(node);
                let at = r.model.nodes.len() - 1;
                if let Some(children) = g.get("children").and_then(Value::as_array) {
                    stack.extend(children.iter().rev().map(|c| (c, at, origin, depth + 1)));
                }
            }
            _ => {}
        }
    }
    for (i, e) in elements.iter().enumerate() {
        let Some(e) = e.as_object() else {
            return malformed(format!("element {i} is not an object"));
        };
        if e.get("export").and_then(Value::as_bool) == Some(false) {
            continue;
        }
        let (node, origin) = placed_on[i].unwrap_or((0, [0.0; 3]));
        let name = e.get("name").and_then(Value::as_str).unwrap_or("element");
        let mut mesh = Mesh {
            name: short(name),
            node,
            ..Mesh::default()
        };
        match e.get("type").and_then(Value::as_str).unwrap_or("cube") {
            "cube" => r.cube(e, &mut mesh, origin, charge)?,
            "mesh" => r.free_mesh(e, &mut mesh, origin, charge)?,
            other => {
                r.model
                    .notes
                    .push(format!("{name}: a {} element, not read", short(other)));
                continue;
            }
        }
        if !mesh.primitives.is_empty() {
            mesh.detail = Some(0);
            r.model.meshes.push(mesh);
        }
    }
    if root
        .get("animations")
        .and_then(Value::as_array)
        .is_some_and(|a| !a.is_empty())
    {
        r.model.notes.push("animations are not read".into());
    }
    let mut model = r.model;
    model.details.push(Detail {
        name: "detail2".into(),
        size: 2.0,
    });
    if options.box_hull {
        crate::add_box_hull(&mut model);
    }
    model.check()?;
    Ok(model)
}

#[cfg(test)]
#[path = "bbmodel_tests.rs"]
mod tests;
