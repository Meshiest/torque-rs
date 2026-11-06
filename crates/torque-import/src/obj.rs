//! Wavefront OBJ (and its `.mtl`): static meshes into the model layer.
//!
//! **What is read:** positions (`v`), texture coordinates (`vt`), normals (`vn`) and faces (`f`, any number of corners, fanned
//! into triangles; indices from 1, or negative from the end); each `o` or `g` starts a mesh on its own node, named as it says;
//! `usemtl` picks the material of the faces after it (one primitive a material). `mtllib` files (through the resolver) give
//! each material's diffuse color (`Kd`), opacity (`d`, or `Tr` as its complement) and diffuse texture (`map_Kd`, its last
//! token a relative path). Lines, points, curves and the rest are left out (lines and points with a note).
//!
//! **Frames.** OBJ is y up and counter-clockwise in front by custom: a point `(x, y, z)` is Torque's `(x, -z, y)` (as glTF's;
//! [`Options::up`] `'z'` keeps it), fronts turn clockwise. A texture coordinate's v counts up from the bottom: it is turned
//! (`1 - v`) to the top-down v glTF and shapes use.
//!
//! **Hostile input.** The file and each `.mtl` are size-capped, every count is held to the caps before it grows, numbers must
//! be finite, an index out of range is an error, paths are relative with no `..`, and work is paid through the charge.

use std::collections::HashMap;

use torque_modelcore::naming::mesh_detail;
use torque_modelcore::{Detail, Image, Material, Mesh, Model, Node, Primitive, Source};

use crate::{Charge, ImportError, Options, cap, caps, pay};

/// The largest `.mtl` read.
pub const MAX_MTL_BYTES: usize = 1 << 20;
/// The most `.mtl` files one model names.
pub const MAX_MTL_FILES: usize = 16;
/// The most corners of one face.
pub const MAX_FACE_CORNERS: usize = 256;

/// Gives a side file's first bytes (at most the second argument's count) by its relative path, or None.
pub type Resolver<'a> = &'a mut dyn FnMut(&str, usize) -> Option<Vec<u8>>;

fn malformed<T>(line: usize, what: &str) -> Result<T, ImportError> {
    Err(ImportError::Malformed(format!("line {line}: {what}")))
}

/// A name cut to [`caps::MAX_NAME_BYTES`].
fn short(name: &str) -> String {
    let mut end = name.len().min(caps::MAX_NAME_BYTES);
    while !name.is_char_boundary(end) {
        end -= 1;
    }
    name[..end].to_string()
}

/// A relative path with `/` (no scheme, no root, no `..`), or None.
pub fn relative_path(s: &str) -> Option<String> {
    let s = s.trim().replace('\\', "/");
    if s.is_empty()
        || s.len() > caps::MAX_URI_BYTES
        || s.starts_with('/')
        || s.contains(':')
        || s.split('/').any(|p| p == ".." || p.is_empty())
    {
        return None;
    }
    Some(s)
}

/// A file name without its folder or extension.
fn stem(path: &str) -> String {
    let file = path.rsplit('/').next().unwrap_or(path);
    short(file.rsplit_once('.').map_or(file, |(a, _)| a))
}

fn floats<const N: usize>(line: usize, parts: &[&str]) -> Result<[f32; N], ImportError> {
    let mut out = [0.0f32; N];
    for (k, o) in out.iter_mut().enumerate() {
        let Some(t) = parts.get(k) else {
            return malformed(line, "too few numbers");
        };
        match t.parse::<f32>() {
            Ok(v) if v.is_finite() => *o = v,
            _ => return malformed(line, "a number that is not finite"),
        }
    }
    Ok(out)
}

/// One material of a `.mtl`.
#[derive(Default, Clone)]
struct Mtl {
    color: [f32; 4],
    texture: Option<String>,
}

fn read_mtl(text: &str, out: &mut HashMap<String, Mtl>) -> Result<(), ImportError> {
    let mut current: Option<String> = None;
    for (n, line) in text.lines().enumerate() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        let Some(&key) = parts.first() else { continue };
        let rest = &parts[1..];
        match key {
            "newmtl" => {
                cap("materials", out.len() + 1, caps::MAX_MATERIALS)?;
                let name = short(&rest.join(" "));
                out.insert(
                    name.clone(),
                    Mtl {
                        color: [1.0; 4],
                        texture: None,
                    },
                );
                current = Some(name);
            }
            "Kd" | "d" | "Tr" | "map_Kd" => {
                let Some(m) = current.as_ref().and_then(|c| out.get_mut(c)) else {
                    continue;
                };
                match key {
                    "Kd" => {
                        let c = floats::<3>(n + 1, rest)?;
                        m.color[..3].copy_from_slice(&c);
                    }
                    "d" => m.color[3] = floats::<1>(n + 1, rest)?[0].clamp(0.0, 1.0),
                    "Tr" => m.color[3] = 1.0 - floats::<1>(n + 1, rest)?[0].clamp(0.0, 1.0),
                    _ => m.texture = rest.last().and_then(|p| relative_path(p)),
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// The mesh being read: its vertices by (position, texture, normal) and its primitives by material.
struct Building {
    mesh: Mesh,
    corners: HashMap<(u32, u32, u32), u32>,
    by_material: HashMap<usize, usize>,
}

impl Building {
    fn new(name: &str) -> Building {
        Building {
            mesh: Mesh {
                name: short(name),
                ..Mesh::default()
            },
            corners: HashMap::new(),
            by_material: HashMap::new(),
        }
    }
}

/// Reads an OBJ (module docs); its `.mtl` files through `resolve`.
pub fn read(
    bytes: &[u8],
    resolve: Resolver,
    options: &Options,
    mut charge: Charge,
) -> Result<Model, ImportError> {
    let charge = &mut charge;
    if bytes.len() > caps::MAX_FILE_BYTES {
        return Err(ImportError::TooLarge {
            what: "the OBJ file",
            bytes: bytes.len() as u64,
            cap: caps::MAX_FILE_BYTES as u64,
        });
    }
    if !(options.scale.is_finite() && options.scale > 0.0) {
        return Err(ImportError::Invalid(
            "a scale that is not a positive number".into(),
        ));
    }
    pay(charge, bytes.len() as u64)?;
    let text = String::from_utf8_lossy(bytes);
    let y_up = options.up != 'z';
    let frame = |v: [f32; 3]| {
        let t = if y_up { [v[0], -v[2], v[1]] } else { v };
        t.map(|c| c * options.scale)
    };
    let mut positions: Vec<[f32; 3]> = Vec::new();
    let mut uvs: Vec<[f32; 2]> = Vec::new();
    let mut normals: Vec<[f32; 3]> = Vec::new();
    let mut mtls: HashMap<String, Mtl> = HashMap::new();
    let mut mtl_files = 0usize;
    let mut material_ids: HashMap<String, usize> = HashMap::new();
    let mut materials: Vec<(String, Mtl)> = Vec::new();
    let mut meshes: Vec<Mesh> = Vec::new();
    let mut cur = Building::new("mesh");
    let mut material: Option<usize> = None;
    let (mut vertices, mut indices, mut skipped) = (0usize, 0usize, 0usize);
    let finish = |b: Building, meshes: &mut Vec<Mesh>| -> Result<(), ImportError> {
        if !b.mesh.primitives.is_empty() {
            cap("meshes", meshes.len() + 1, caps::MAX_MESHES)?;
            meshes.push(b.mesh);
        }
        Ok(())
    };
    for (n, line) in text.lines().enumerate() {
        let n = n + 1;
        let line = line.split('#').next().unwrap_or("");
        let parts: Vec<&str> = line.split_whitespace().collect();
        let Some(&key) = parts.first() else { continue };
        let rest = &parts[1..];
        match key {
            "v" => {
                cap("positions", positions.len() + 1, caps::MAX_VERTICES)?;
                positions.push(floats::<3>(n, rest)?);
            }
            "vt" => {
                cap("texture coordinates", uvs.len() + 1, caps::MAX_VERTICES)?;
                let u = floats::<1>(n, rest)?[0];
                let v = if rest.len() > 1 {
                    floats::<2>(n, rest)?[1]
                } else {
                    0.0
                };
                uvs.push([u, 1.0 - v]);
            }
            "vn" => {
                cap("normals", normals.len() + 1, caps::MAX_VERTICES)?;
                normals.push(floats::<3>(n, rest)?);
            }
            "o" | "g" => {
                let name = rest.join(" ");
                let name = if name.is_empty() { "mesh" } else { &name };
                let done = std::mem::replace(&mut cur, Building::new(name));
                finish(done, &mut meshes)?;
            }
            "usemtl" => {
                let name = short(&rest.join(" "));
                let next = material_ids.len();
                let id = *material_ids.entry(name.clone()).or_insert(next);
                if id == materials.len() {
                    cap("materials", materials.len() + 1, caps::MAX_MATERIALS)?;
                    materials.push((name, Mtl::default()));
                }
                material = Some(id);
            }
            "mtllib" => {
                for p in rest {
                    mtl_files += 1;
                    cap("mtllib files", mtl_files, MAX_MTL_FILES)?;
                    let Some(path) = relative_path(p) else {
                        return Err(ImportError::Uri((*p).to_string()));
                    };
                    match resolve(&path, MAX_MTL_BYTES + 1) {
                        Some(b) if b.len() > MAX_MTL_BYTES => {
                            return Err(ImportError::TooLarge {
                                what: "a .mtl file",
                                bytes: b.len() as u64,
                                cap: MAX_MTL_BYTES as u64,
                            });
                        }
                        Some(b) => {
                            pay(charge, b.len() as u64)?;
                            read_mtl(&String::from_utf8_lossy(&b), &mut mtls)?;
                        }
                        None => {}
                    }
                }
            }
            "f" => {
                cap("corners of a face", rest.len(), MAX_FACE_CORNERS)?;
                if rest.len() < 3 {
                    return malformed(n, "a face of fewer than three corners");
                }
                pay(charge, rest.len() as u64)?;
                let mut ids = Vec::with_capacity(rest.len());
                for corner in rest {
                    let mut f = corner.split('/');
                    let index =
                        |s: Option<&str>, count: usize| -> Result<Option<u32>, ImportError> {
                            let Some(s) = s.filter(|s| !s.is_empty()) else {
                                return Ok(None);
                            };
                            let Ok(i) = s.parse::<i64>() else {
                                return malformed(n, "an index that is not a number");
                            };
                            let at = if i > 0 { i - 1 } else { count as i64 + i };
                            if i == 0 || at < 0 || at >= count as i64 {
                                return malformed(n, "an index out of range");
                            }
                            Ok(Some(at as u32))
                        };
                    let Some(v) = index(f.next(), positions.len())? else {
                        return malformed(n, "a corner without a position");
                    };
                    let t = index(f.next(), uvs.len())?;
                    let nn = index(f.next(), normals.len())?;
                    let key = (v, t.unwrap_or(u32::MAX), nn.unwrap_or(u32::MAX));
                    let mesh = &mut cur.mesh;
                    let id = match cur.corners.get(&key) {
                        Some(&id) => id,
                        None => {
                            vertices += 1;
                            cap("vertices", vertices, caps::MAX_VERTICES)?;
                            let id = mesh.positions.len() as u32;
                            mesh.positions.push(frame(positions[v as usize]));
                            mesh.uv0.push(t.map_or([0.0; 2], |t| uvs[t as usize]));
                            let nv = nn.map_or([0.0, 0.0, 1.0], |x| normals[x as usize]);
                            let nv = if y_up { [nv[0], -nv[2], nv[1]] } else { nv };
                            mesh.normals.push(nv);
                            cur.corners.insert(key, id);
                            id
                        }
                    };
                    ids.push(id);
                }
                let prim = *cur
                    .by_material
                    .entry(material.unwrap_or(usize::MAX))
                    .or_insert_with(|| {
                        cur.mesh.primitives.push(Primitive {
                            material,
                            indices: Vec::new(),
                        });
                        cur.mesh.primitives.len() - 1
                    });
                indices += (ids.len() - 2) * 3;
                cap("triangle indices", indices, caps::MAX_INDICES)?;
                let list = &mut cur.mesh.primitives[prim].indices;
                // Fanned from the first corner, turned clockwise.
                for k in 1..ids.len() - 1 {
                    list.extend_from_slice(&[ids[0], ids[k + 1], ids[k]]);
                }
            }
            "l" | "p" => skipped += 1,
            _ => {}
        }
    }
    finish(cur, &mut meshes)?;
    let mut model = Model {
        source: Source {
            format: "OBJ".into(),
            up: if y_up { 'y' } else { 'z' },
            scale: options.scale,
        },
        ..Model::default()
    };
    if skipped > 0 {
        model
            .notes
            .push(format!("{skipped} lines and points not read"));
    }
    for (name, _) in &materials {
        let m = mtls.get(name).cloned().unwrap_or(Mtl {
            color: [1.0; 4],
            texture: None,
        });
        let mut out = Material {
            name: name.clone(),
            base_color: m.color,
            ..Material::default()
        };
        if m.color[3] < 1.0 {
            out.alpha = torque_modelcore::Alpha::Blend;
        }
        if let Some(path) = &m.texture {
            cap("images", model.images.len() + 1, caps::MAX_IMAGES)?;
            out.name = stem(path);
            out.base_texture = Some(model.images.len());
            model.images.push(Image {
                name: stem(path),
                uri: Some(path.clone()),
                ..Image::default()
            });
        }
        model.materials.push(out);
    }
    // Every mesh on its own node; its detail by its name, as glTF's (largest first).
    let mut details: Vec<(String, i32)> = Vec::new();
    let mut detail_of = Vec::with_capacity(meshes.len());
    for mut mesh in meshes {
        cap("nodes", model.nodes.len() + 1, caps::MAX_NODES)?;
        model.nodes.push(Node::new(&mesh.name, None, [0.0; 3]));
        mesh.node = model.nodes.len() - 1;
        let d = mesh_detail(&mesh.name);
        mesh.name = d.object;
        let k = details
            .iter()
            .position(|x| x.0 == d.detail)
            .unwrap_or_else(|| {
                details.push((d.detail, d.size));
                details.len() - 1
            });
        detail_of.push(k);
        model.meshes.push(mesh);
    }
    let mut order: Vec<usize> = (0..details.len()).collect();
    order.sort_by(|&a, &b| details[b].1.cmp(&details[a].1));
    let mut rank = vec![0; details.len()];
    for (r, &k) in order.iter().enumerate() {
        rank[k] = r;
        model.details.push(Detail {
            name: details[k].0.clone(),
            size: details[k].1 as f32,
        });
    }
    for (m, k) in model.meshes.iter_mut().zip(detail_of) {
        m.detail = Some(rank[k]);
    }
    if options.box_hull {
        crate::add_box_hull(&mut model);
    }
    model.check()?;
    Ok(model)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn free(_: u64) -> bool {
        true
    }

    fn no_files(_: &str, _: usize) -> Option<Vec<u8>> {
        None
    }

    fn opts() -> Options {
        Options {
            box_hull: false,
            ..Options::default()
        }
    }

    #[test]
    fn a_quad_with_a_material_reads_as_two_clockwise_triangles() {
        let obj = b"mtllib a.mtl\no floor\nv 0 0 0\nv 1 0 0\nv 1 0 -1\nv 0 0 -1\nvt 0 0\nvt 1 0\nvt 1 1\nvt 0 1\n\
            usemtl stone\nf 1/1 2/2 3/3 4/4\n";
        let mut side = |p: &str, _: usize| {
            (p == "a.mtl").then(|| b"newmtl stone\nKd 0.5 0.5 0.5\nmap_Kd tex/stone.png\n".to_vec())
        };
        let m = read(obj, &mut side, &opts(), &mut free).unwrap();
        assert_eq!(m.meshes.len(), 1);
        let mesh = &m.meshes[0];
        assert_eq!(mesh.name, "floor");
        assert_eq!(mesh.primitives[0].indices, vec![0, 2, 1, 0, 3, 2]);
        // y up: (1, 0, -1) is Torque's (1, 1, 0).
        assert_eq!(mesh.positions[2], [1.0, 1.0, 0.0]);
        assert_eq!(mesh.uv0[2], [1.0, 0.0]);
        assert_eq!(m.materials[0].name, "stone");
        assert_eq!(m.images[0].uri.as_deref(), Some("tex/stone.png"));
        // Clockwise seen from above (+z): the area vector of (p0, p1, p2) points down.
        let p = |i: u32| mesh.positions[i as usize];
        let t = &mesh.primitives[0].indices;
        let (a, b, c) = (p(t[0]), p(t[1]), p(t[2]));
        let z = (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
        assert!(z < 0.0);
    }

    #[test]
    fn negative_indices_count_from_the_end_and_groups_make_meshes() {
        let obj = b"v 0 0 0\nv 1 0 0\nv 0 1 0\ng a\nf -3 -2 -1\ng b\nv 0 0 1\nf 1 2 4\n";
        let m = read(obj, &mut no_files, &opts(), &mut free).unwrap();
        assert_eq!(m.meshes.len(), 2);
        assert_eq!(m.nodes[1].name, "b");
    }

    #[test]
    fn hostile_files_are_errors() {
        let bad: [&[u8]; 7] = [
            b"v 0 0 0\nf 1 2 3\n",
            b"v nan 0 0\n",
            b"v 1e40 0 0\n",
            b"v 0 0 0\nv 1 0 0\nv 0 1 0\nf 0 1 2\n",
            b"v 0 0 0\nv 1 0 0\nf 1 2\n",
            b"mtllib ../secret.mtl\n",
            b"mtllib C:/x.mtl\n",
        ];
        for b in bad {
            assert!(
                read(b, &mut no_files, &opts(), &mut free).is_err(),
                "{}",
                String::from_utf8_lossy(b)
            );
        }
        let mut long = b"v 0 0 0\nv 1 0 0\nv 0 1 0\nf".to_vec();
        for _ in 0..MAX_FACE_CORNERS + 1 {
            long.extend_from_slice(b" 1");
        }
        assert!(read(&long, &mut no_files, &opts(), &mut free).is_err());
        let mut out_of_budget = |_: u64| false;
        assert_eq!(
            read(b"v 0 0 0\n", &mut no_files, &opts(), &mut out_of_budget),
            Err(ImportError::OutOfBudget)
        );
    }

    #[test]
    fn the_reader_never_panics() {
        let base: &[u8] =
            b"mtllib a.mtl\no x\nv 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 1\nvt 0 0\nvt 1 0\nvn 0 0 1\n\
            usemtl m\nf 1/1/1 2/2/1 3/1/1 4/2/1\ng y\nf -1 -2 -3\nl 1 2\n";
        let cases: usize = std::env::var("TORQUE_IMPORT_FUZZ_CASES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(3_000);
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let alphabet = b" \n/-0123456789.vtnfgomlusecrib#e";
        for _ in 0..cases {
            let mut b = base.to_vec();
            for _ in 0..1 + next() % 6 {
                let at = (next() % b.len() as u64) as usize;
                match next() % 3 {
                    0 => b[at] = alphabet[(next() % alphabet.len() as u64) as usize],
                    1 => {
                        b.remove(at);
                    }
                    _ => b.insert(at, alphabet[(next() % alphabet.len() as u64) as usize]),
                }
                if b.is_empty() {
                    b.push(b'v');
                }
            }
            let mut side = |_: &str, _: usize| Some(b"newmtl m\nmap_Kd t.png\nKd 1 1".to_vec());
            let _ = read(&b, &mut side, &opts(), &mut free);
        }
    }
}
