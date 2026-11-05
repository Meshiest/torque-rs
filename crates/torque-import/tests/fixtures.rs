//! Twenty glTF files made here (static, multi-material, strips and fans, matrices and scales, linear, step and cubic-spline
//! animation, loops, skins with up to four weights, collision names, a deep tree), each read, written as a `.dts`, read back by
//! the shape reader, and held against the glTF's own meaning, worked out here independently of the reader:
//! - the same node, mesh, vertex and sequence counts;
//! - every vertex the shape keeps within 1e-4 of the glTF's (in Torque's frame, with a node's rest scale carried in);
//! - at rest and at 10 times of each sequence, every node's origin and every vertex as the shape poses it (rigid or skinned)
//!   within 1e-3 of where the glTF puts it.

use serde_json::{Value, json};
use torque_import::{Options, dts, gltf};
use torque_shapes::instance::ShapeInstance;
use torque_shapes::math::{mat_mul, mul_p};
use torque_shapes::shape::{MeshExtra, Shape};

type V3 = [f64; 3];
type Q = [f64; 4];
type M = [f64; 16];

const I: M = [
    1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.,
];

fn mul(a: &M, b: &M) -> M {
    std::array::from_fn(|k| (0..4).map(|j| a[k / 4 * 4 + j] * b[j * 4 + k % 4]).sum())
}

fn apply(m: &M, p: V3) -> V3 {
    std::array::from_fn(|r| {
        m[r * 4] * p[0] + m[r * 4 + 1] * p[1] + m[r * 4 + 2] * p[2] + m[r * 4 + 3]
    })
}

fn trs(t: V3, q: Q, s: V3) -> M {
    let [x, y, z, w] = q;
    let r = [
        1. - 2. * (y * y + z * z),
        2. * (x * y - z * w),
        2. * (x * z + y * w),
        2. * (x * y + z * w),
        1. - 2. * (x * x + z * z),
        2. * (y * z - x * w),
        2. * (x * z - y * w),
        2. * (y * z + x * w),
        1. - 2. * (x * x + y * y),
    ];
    [
        r[0] * s[0],
        r[1] * s[1],
        r[2] * s[2],
        t[0],
        r[3] * s[0],
        r[4] * s[1],
        r[5] * s[2],
        t[1],
        r[6] * s[0],
        r[7] * s[1],
        r[8] * s[2],
        t[2],
        0.,
        0.,
        0.,
        1.,
    ]
}

/// The inverse of an affine matrix by its 3x3 cofactors.
fn inverse(m: &M) -> M {
    let a = |r: usize, c: usize| m[r * 4 + c];
    let c = |r: usize, k: usize| {
        let (r1, r2, k1, k2) = ((r + 1) % 3, (r + 2) % 3, (k + 1) % 3, (k + 2) % 3);
        a(r1, k1) * a(r2, k2) - a(r1, k2) * a(r2, k1)
    };
    let det: f64 = (0..3).map(|k| a(0, k) * c(0, k)).sum();
    let mut o = I;
    for r in 0..3 {
        for k in 0..3 {
            o[r * 4 + k] = c(k, r) / det;
        }
    }
    for r in 0..3 {
        o[r * 4 + 3] = -(0..3).map(|k| o[r * 4 + k] * m[k * 4 + 3]).sum::<f64>();
    }
    o
}

fn norm(q: Q) -> Q {
    let l = q.iter().map(|c| c * c).sum::<f64>().sqrt();
    q.map(|c| c / l)
}

fn slerp(a: Q, b: Q, t: f64) -> Q {
    let mut d: f64 = (0..4).map(|i| a[i] * b[i]).sum();
    let b = if d < 0. {
        d = -d;
        b.map(|c| -c)
    } else {
        b
    };
    if d > 0.9999 {
        return norm(std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t));
    }
    let th = d.acos();
    let (wa, wb) = (((1. - t) * th).sin() / th.sin(), (t * th).sin() / th.sin());
    std::array::from_fn(|i| a[i] * wa + b[i] * wb)
}

/// glTF's frame into Torque's.
fn torque(p: V3) -> V3 {
    [p[0], -p[2], p[1]]
}

/// A tiny deterministic generator.
struct Rng(u64);

impl Rng {
    fn f(&mut self) -> f64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((self.0 >> 33) as f64) / (1u64 << 31) as f64
    }
    fn range(&mut self, a: f64, b: f64) -> f64 {
        a + (b - a) * self.f()
    }
    fn v3(&mut self, a: f64) -> V3 {
        [self.range(-a, a), self.range(-a, a), self.range(-a, a)]
    }
    fn quat(&mut self) -> Q {
        norm([
            self.range(-1., 1.),
            self.range(-1., 1.),
            self.range(-1., 1.),
            self.range(0.2, 1.),
        ])
    }
}

#[derive(Clone)]
struct FNode {
    name: String,
    parent: Option<usize>,
    t: V3,
    r: Q,
    s: V3,
    matrix: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum Ix {
    None,
    U8,
    U16,
    U32,
}

struct FPrim {
    positions: Vec<V3>,
    normals: bool,
    list: Vec<u32>,
    mode: u32,
    ix: Ix,
    material: Option<usize>,
    joints: Vec<[u16; 4]>,
    weights: Vec<[f64; 4]>,
}

struct FMesh {
    node: usize,
    prims: Vec<FPrim>,
    skin: Option<(Vec<usize>, Vec<M>)>,
    u8_joints: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum Path {
    T,
    R,
    S,
}

struct FChan {
    node: usize,
    path: Path,
    interp: &'static str,
    times: Vec<f64>,
    /// One value a key (three for a cubic spline: in-tangent, value, out-tangent), four numbers each.
    values: Vec<Q>,
    i16_rotations: bool,
}

struct FAnim {
    name: String,
    chans: Vec<FChan>,
}

#[derive(Default)]
struct Fix {
    nodes: Vec<FNode>,
    meshes: Vec<FMesh>,
    anims: Vec<FAnim>,
    materials: usize,
}

impl FChan {
    fn key(&self, k: usize) -> Q {
        if self.interp == "CUBICSPLINE" {
            self.values[3 * k + 1]
        } else {
            self.values[k]
        }
    }

    fn sample(&self, t: f64) -> Q {
        let n = self.times.len();
        let after = self.times.partition_point(|&x| x <= t);
        if after == 0 {
            return self.key(0);
        }
        if after >= n {
            return self.key(n - 1);
        }
        let k = after - 1;
        let span = self.times[k + 1] - self.times[k];
        let s = (t - self.times[k]) / span;
        let (a, b) = (self.key(k), self.key(k + 1));
        let rot = self.path == Path::R;
        let v = match self.interp {
            "STEP" => a,
            "LINEAR" if rot => slerp(a, b, s),
            "LINEAR" => std::array::from_fn(|i| a[i] + (b[i] - a[i]) * s),
            _ => {
                let (bo, ai) = (self.values[3 * k + 2], self.values[3 * (k + 1)]);
                let (s2, s3) = (s * s, s * s * s);
                std::array::from_fn(|i| {
                    (2. * s3 - 3. * s2 + 1.) * a[i]
                        + (s3 - 2. * s2 + s) * span * bo[i]
                        + (-2. * s3 + 3. * s2) * b[i]
                        + (s3 - s2) * span * ai[i]
                })
            }
        };
        if rot { norm(v) } else { v }
    }
}

impl Fix {
    fn node(&mut self, name: &str, parent: Option<usize>, t: V3, r: Q, s: V3) -> usize {
        self.nodes.push(FNode {
            name: name.into(),
            parent,
            t,
            r,
            s,
            matrix: false,
        });
        self.nodes.len() - 1
    }

    /// Each node's transform in the glTF's space, animation `anim` at `t` (None: at rest).
    fn world(&self, anim: Option<(usize, f64)>) -> Vec<M> {
        let mut parts: Vec<(V3, Q, V3)> = self.nodes.iter().map(|n| (n.t, n.r, n.s)).collect();
        if let Some((a, t)) = anim {
            for c in &self.anims[a].chans {
                let v = c.sample(t);
                let p = &mut parts[c.node];
                match c.path {
                    Path::T => p.0 = [v[0], v[1], v[2]],
                    Path::R => p.1 = v,
                    Path::S => p.2 = [v[0], v[1], v[2]],
                }
            }
        }
        let mut w: Vec<M> = Vec::new();
        for (i, n) in self.nodes.iter().enumerate() {
            let local = trs(parts[i].0, parts[i].1, parts[i].2);
            w.push(match n.parent {
                Some(p) => mul(&w[p], &local),
                None => local,
            });
        }
        w
    }

    fn duration(&self, a: usize) -> f64 {
        self.anims[a]
            .chans
            .iter()
            .map(|c| *c.times.last().unwrap())
            .fold(0., f64::max)
    }

    /// Every vertex of mesh `m` in Torque's frame, posed by `world`.
    fn posed(&self, m: usize, world: &[M]) -> Vec<V3> {
        let mesh = &self.meshes[m];
        let mut out = Vec::new();
        for p in &mesh.prims {
            for (v, &pos) in p.positions.iter().enumerate() {
                let q = match &mesh.skin {
                    None => apply(&world[mesh.node], pos),
                    Some((joints, ibm)) => {
                        let mut acc = [0.0; 3];
                        let sum: f64 = p.weights[v].iter().sum();
                        for k in 0..4 {
                            let j = usize::from(p.joints[v][k]);
                            let w = p.weights[v][k] / sum;
                            let x = apply(&mul(&world[joints[j]], &ibm[j]), pos);
                            for c in 0..3 {
                                acc[c] += w * x[c];
                            }
                        }
                        acc
                    }
                };
                out.push(torque(q));
            }
        }
        out
    }

    /// The `.glb` of the fixture.
    fn glb(&self) -> Vec<u8> {
        let mut g = G::default();
        let mut nodes: Vec<Value> = self
            .nodes
            .iter()
            .map(|n| {
                let mut v = json!({ "name": n.name });
                if n.matrix {
                    let m = trs(n.t, n.r, n.s);
                    let cols: Vec<f64> = (0..16).map(|i| m[(i % 4) * 4 + i / 4]).collect();
                    v["matrix"] = json!(cols);
                } else {
                    v["translation"] = json!(n.t.to_vec());
                    v["rotation"] = json!(n.r.to_vec());
                    v["scale"] = json!(n.s.to_vec());
                }
                v
            })
            .collect();
        for (i, n) in self.nodes.iter().enumerate() {
            if let Some(p) = n.parent {
                let list = nodes[p]
                    .get("children")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                let mut list = list;
                list.push(json!(i));
                nodes[p]["children"] = json!(list);
            }
        }
        let mut meshes = Vec::new();
        let mut skins = Vec::new();
        for (mi, m) in self.meshes.iter().enumerate() {
            let prims: Vec<Value> = m
                .prims
                .iter()
                .map(|p| {
                    let mut attrs = json!({ "POSITION": g.floats(&p.positions.iter().map(|v| v.to_vec()).collect::<Vec<_>>(), "VEC3", true) });
                    if p.normals {
                        let normals: Vec<Vec<f64>> = p
                            .positions
                            .iter()
                            .map(|v| {
                                let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-9);
                                v.map(|c| c / l).to_vec()
                            })
                            .collect();
                        attrs["NORMAL"] = json!(g.floats(&normals, "VEC3", false));
                    }
                    let uvs: Vec<Vec<f64>> = p.positions.iter().map(|v| vec![v[0] * 0.5, v[1] * 0.5]).collect();
                    attrs["TEXCOORD_0"] = json!(g.floats(&uvs, "VEC2", false));
                    if !p.joints.is_empty() {
                        let (bytes, comp) = if m.u8_joints {
                            (p.joints.iter().flatten().map(|&j| j as u8).collect::<Vec<u8>>(), 5121)
                        } else {
                            (p.joints.iter().flatten().flat_map(|j| j.to_le_bytes()).collect(), 5123)
                        };
                        attrs["JOINTS_0"] = json!(g.raw(&bytes, comp, p.joints.len(), "VEC4", false));
                        let w: Vec<u8> = if m.u8_joints {
                            // Normalized shorts.
                            p.weights
                                .iter()
                                .flat_map(|w| {
                                    let sum: f64 = w.iter().sum();
                                    w.map(|x| ((x / sum) * 65535.0).round() as u16)
                                })
                                .flat_map(u16::to_le_bytes)
                                .collect()
                        } else {
                            p.weights.iter().flatten().flat_map(|&x| (x as f32).to_le_bytes()).collect()
                        };
                        let (comp, norm) = if m.u8_joints { (5123, true) } else { (5126, false) };
                        attrs["WEIGHTS_0"] = json!(g.raw(&w, comp, p.weights.len(), "VEC4", norm));
                    }
                    let mut v = json!({ "attributes": attrs, "mode": p.mode });
                    let ix = match p.ix {
                        Ix::None => None,
                        Ix::U8 => Some(g.raw(&p.list.iter().map(|&i| i as u8).collect::<Vec<_>>(), 5121, p.list.len(), "SCALAR", false)),
                        Ix::U16 => Some(g.raw(
                            &p.list.iter().flat_map(|&i| (i as u16).to_le_bytes()).collect::<Vec<_>>(),
                            5123,
                            p.list.len(),
                            "SCALAR",
                            false,
                        )),
                        Ix::U32 => Some(g.raw(
                            &p.list.iter().flat_map(|i| i.to_le_bytes()).collect::<Vec<_>>(),
                            5125,
                            p.list.len(),
                            "SCALAR",
                            false,
                        )),
                    };
                    if let Some(ix) = ix {
                        v["indices"] = json!(ix);
                    }
                    if let Some(mat) = p.material {
                        v["material"] = json!(mat);
                    }
                    v
                })
                .collect();
            meshes.push(json!({ "primitives": prims }));
            nodes[m.node]["mesh"] = json!(mi);
            if let Some((joints, ibm)) = &m.skin {
                let cols: Vec<Vec<f64>> = ibm
                    .iter()
                    .map(|m| (0..16).map(|i| m[(i % 4) * 4 + i / 4]).collect())
                    .collect();
                skins.push(json!({ "joints": joints, "inverseBindMatrices": g.floats(&cols, "MAT4", false) }));
                nodes[m.node]["skin"] = json!(skins.len() - 1);
            }
        }
        let mut anims = Vec::new();
        for a in &self.anims {
            let mut samplers = Vec::new();
            let mut channels = Vec::new();
            for c in &a.chans {
                let input = g.floats(
                    &c.times.iter().map(|&t| vec![t]).collect::<Vec<_>>(),
                    "SCALAR",
                    true,
                );
                let output = match c.path {
                    Path::R if c.i16_rotations => {
                        let b: Vec<u8> = c
                            .values
                            .iter()
                            .flatten()
                            .flat_map(|&x| ((x * 32767.0).round() as i16).to_le_bytes())
                            .collect();
                        g.raw(&b, 5122, c.values.len(), "VEC4", true)
                    }
                    Path::R => g.floats(
                        &c.values.iter().map(|v| v.to_vec()).collect::<Vec<_>>(),
                        "VEC4",
                        false,
                    ),
                    _ => g.floats(
                        &c.values.iter().map(|v| v[..3].to_vec()).collect::<Vec<_>>(),
                        "VEC3",
                        false,
                    ),
                };
                samplers
                    .push(json!({ "input": input, "output": output, "interpolation": c.interp }));
                let path = match c.path {
                    Path::T => "translation",
                    Path::R => "rotation",
                    Path::S => "scale",
                };
                channels.push(json!({ "sampler": samplers.len() - 1, "target": { "node": c.node, "path": path } }));
            }
            anims.push(json!({ "name": a.name, "samplers": samplers, "channels": channels }));
        }
        let roots: Vec<usize> = (0..self.nodes.len())
            .filter(|&i| self.nodes[i].parent.is_none())
            .collect();
        let materials: Vec<Value> = (0..self.materials)
            .map(|i| json!({ "name": format!("mat{i}") }))
            .collect();
        let mut doc = json!({
            "asset": { "version": "2.0" },
            "scene": 0,
            "scenes": [{ "nodes": roots }],
            "nodes": nodes,
            "meshes": meshes,
            "buffers": [{ "byteLength": g.bin.len() }],
            "bufferViews": g.views,
            "accessors": g.accessors,
        });
        if !skins.is_empty() {
            doc["skins"] = json!(skins);
        }
        if !anims.is_empty() {
            doc["animations"] = json!(anims);
        }
        if !materials.is_empty() {
            doc["materials"] = json!(materials);
        }
        let mut text = serde_json::to_vec(&doc).unwrap();
        while !text.len().is_multiple_of(4) {
            text.push(b' ');
        }
        let mut out = b"glTF".to_vec();
        out.extend(2u32.to_le_bytes());
        out.extend(((28 + text.len() + g.bin.len()) as u32).to_le_bytes());
        out.extend((text.len() as u32).to_le_bytes());
        out.extend(b"JSON");
        out.extend(text);
        out.extend((g.bin.len() as u32).to_le_bytes());
        out.extend(b"BIN\0");
        out.extend(g.bin);
        out
    }
}

/// The binary chunk and its views and accessors.
#[derive(Default)]
struct G {
    bin: Vec<u8>,
    views: Vec<Value>,
    accessors: Vec<Value>,
}

impl G {
    fn raw(
        &mut self,
        bytes: &[u8],
        comp: u32,
        count: usize,
        kind: &str,
        normalized: bool,
    ) -> usize {
        while !self.bin.len().is_multiple_of(4) {
            self.bin.push(0);
        }
        self.views
            .push(json!({ "buffer": 0, "byteOffset": self.bin.len(), "byteLength": bytes.len() }));
        self.bin.extend_from_slice(bytes);
        self.accessors.push(json!({
            "bufferView": self.views.len() - 1, "componentType": comp, "count": count, "type": kind, "normalized": normalized,
        }));
        self.accessors.len() - 1
    }

    fn floats(&mut self, data: &[Vec<f64>], kind: &str, bounds: bool) -> usize {
        let bytes: Vec<u8> = data
            .iter()
            .flatten()
            .flat_map(|&x| (x as f32).to_le_bytes())
            .collect();
        let a = self.raw(&bytes, 5126, data.len(), kind, false);
        if bounds {
            let n = data[0].len();
            let lo: Vec<f64> = (0..n)
                .map(|k| {
                    data.iter()
                        .map(|v| f64::from(v[k] as f32))
                        .fold(f64::MAX, f64::min)
                })
                .collect();
            let hi: Vec<f64> = (0..n)
                .map(|k| {
                    data.iter()
                        .map(|v| f64::from(v[k] as f32))
                        .fold(f64::MIN, f64::max)
                })
                .collect();
            self.accessors[a]["min"] = json!(lo);
            self.accessors[a]["max"] = json!(hi);
        }
        a
    }
}

/// A box of 8 corners, 12 triangles (counter-clockwise from outside, as glTF has them) around `c`, `h` a half side.
fn boxed(c: V3, h: f64, material: Option<usize>, ix: Ix) -> FPrim {
    let positions: Vec<V3> = (0..8)
        .map(|i| std::array::from_fn(|k| c[k] + if i & (1 << k) != 0 { h } else { -h }))
        .collect();
    let list = vec![
        0, 2, 1, 1, 2, 3, 4, 5, 6, 5, 7, 6, 0, 1, 4, 1, 5, 4, 2, 6, 3, 3, 6, 7, 0, 4, 2, 2, 4, 6,
        1, 3, 5, 3, 7, 5,
    ];
    FPrim {
        positions,
        normals: true,
        list,
        mode: 4,
        ix,
        material,
        joints: Vec::new(),
        weights: Vec::new(),
    }
}

fn rigid(node: usize, prims: Vec<FPrim>) -> FMesh {
    FMesh {
        node,
        prims,
        skin: None,
        u8_joints: false,
    }
}

/// A channel with keys at 0, `d` and random times between, values from `value`.
fn chan(
    rng: &mut Rng,
    node: usize,
    path: Path,
    interp: &'static str,
    d: f64,
    keys: usize,
    mut value: impl FnMut(&mut Rng) -> Q,
) -> FChan {
    let mut times: Vec<f64> = (0..keys)
        .map(|k| d * k as f64 / (keys - 1) as f64)
        .collect();
    for t in times.iter_mut().take(keys - 1).skip(1) {
        *t += rng.range(-0.3, 0.3) * d / (keys - 1) as f64;
    }
    let per = if interp == "CUBICSPLINE" { 3 } else { 1 };
    let values = (0..keys * per).map(|_| value(rng)).collect();
    FChan {
        node,
        path,
        interp,
        times,
        values,
        i16_rotations: false,
    }
}

fn skin_mesh(
    f: &Fix,
    node: usize,
    joints: Vec<usize>,
    rng: &mut Rng,
    influences: usize,
    u8_joints: bool,
) -> FMesh {
    let world = f.world(None);
    let ibm: Vec<M> = joints.iter().map(|&j| inverse(&world[j])).collect();
    let mut prim = boxed([0.0, 0.0, 0.0], 0.0, None, Ix::U16);
    // A column of points along the joints' chain.
    prim.positions = (0..24)
        .map(|i| [rng.range(-0.3, 0.3), i as f64 * 0.12, rng.range(-0.3, 0.3)])
        .collect();
    prim.list = (0..22).flat_map(|i| [i, i + 1, i + 2]).collect();
    prim.joints = (0..24)
        .map(|_| {
            std::array::from_fn(|_| (rng.f() * joints.len() as f64) as u16 % joints.len() as u16)
        })
        .collect();
    prim.weights = (0..24)
        .map(|_| {
            std::array::from_fn(|k| {
                if k < influences {
                    rng.range(0.1, 1.0)
                } else {
                    0.0
                }
            })
        })
        .collect();
    FMesh {
        node,
        prims: vec![prim],
        skin: Some((joints, ibm)),
        u8_joints,
    }
}

fn chain(f: &mut Fix, rng: &mut Rng, n: usize) -> Vec<usize> {
    let mut parent = None;
    (0..n)
        .map(|i| {
            let id = f.node(
                &format!("bone{}", ["a", "b", "c", "d", "e", "f", "g", "h"][i]),
                parent,
                [0.0, 0.4, 0.0],
                rng.quat(),
                [1.0; 3],
            );
            if i == 0 {
                f.nodes[id].t = rng.v3(1.0);
            }
            parent = Some(id);
            id
        })
        .collect()
}

fn fixture(i: usize) -> Fix {
    let mut rng = Rng(0x9E37_79B9 + i as u64 * 7919);
    let mut f = Fix::default();
    let no = [0.0, 0.0, 0.0, 1.0];
    match i {
        0 => {
            let n = f.node("cube", None, [0.0; 3], no, [1.0; 3]);
            f.materials = 1;
            f.meshes
                .push(rigid(n, vec![boxed([0.0; 3], 0.5, Some(0), Ix::U16)]));
        }
        1 => {
            let n = f.node("plain", None, [1.0, 2.0, 3.0], no, [1.0; 3]);
            let mut p = boxed([0.2; 3], 0.4, None, Ix::None);
            p.positions = p.list.iter().map(|&k| p.positions[k as usize]).collect();
            p.normals = false;
            f.meshes.push(rigid(n, vec![p]));
        }
        2 => {
            let a = f.node("base", None, rng.v3(2.0), rng.quat(), [1.0; 3]);
            let b = f.node("arm", Some(a), rng.v3(1.0), rng.quat(), [1.0; 3]);
            f.meshes
                .push(rigid(a, vec![boxed([0.0; 3], 0.3, None, Ix::U8)]));
            f.meshes
                .push(rigid(b, vec![boxed([0.1, 0.0, 0.0], 0.2, None, Ix::U32)]));
        }
        3 => {
            let a = f.node("frame", None, rng.v3(1.0), rng.quat(), [2.0; 3]);
            f.nodes[a].matrix = true;
            let b = f.node("inner", Some(a), rng.v3(0.5), rng.quat(), [1.0; 3]);
            f.nodes[b].matrix = true;
            f.meshes
                .push(rigid(b, vec![boxed([0.0; 3], 0.25, None, Ix::U16)]));
        }
        4 => {
            let n = f.node("multi", None, [0.0; 3], rng.quat(), [1.0; 3]);
            f.materials = 3;
            f.meshes.push(rigid(
                n,
                (0..3)
                    .map(|k| boxed([k as f64, 0.0, 0.0], 0.3, Some(k), Ix::U16))
                    .collect(),
            ));
        }
        5 => {
            let n = f.node("strips", None, [0.0; 3], no, [1.0; 3]);
            let mut strip = boxed([0.0; 3], 0.5, None, Ix::U16);
            strip.mode = 5;
            strip.list = vec![0, 1, 2, 3, 6, 7, 4, 5];
            let mut fan = boxed([2.0, 0.0, 0.0], 0.5, None, Ix::U16);
            fan.mode = 6;
            fan.list = vec![0, 1, 3, 2, 6, 4];
            f.meshes.push(rigid(n, vec![strip, fan]));
        }
        6..=12 | 18 | 19 => {
            let d = [1.0, 2.0, 1.0, 1.0, 1.0, 1.0, 1.0][i.min(12) - 6];
            let root = f.node("root", None, [0.0; 3], no, [1.0; 3]);
            let a = f.node("upper", Some(root), rng.v3(1.0), rng.quat(), [1.0; 3]);
            let b = f.node("lower", Some(a), [0.0, 0.6, 0.0], rng.quat(), [1.0; 3]);
            if i == 18 {
                f.nodes[a].s = [1.5, 0.75, 2.0];
            }
            f.meshes
                .push(rigid(a, vec![boxed([0.0; 3], 0.2, None, Ix::U16)]));
            f.meshes
                .push(rigid(b, vec![boxed([0.0, 0.3, 0.0], 0.15, None, Ix::U16)]));
            let mv = |r: &mut Rng| {
                let v = r.v3(1.0);
                [v[0], v[1], v[2], 0.0]
            };
            let mut anim = FAnim {
                name: format!("seq{i}"),
                chans: Vec::new(),
            };
            match i {
                6 | 18 => anim
                    .chans
                    .push(chan(&mut rng, b, Path::T, "LINEAR", d, 5, mv)),
                7 => anim
                    .chans
                    .push(chan(&mut rng, b, Path::R, "LINEAR", d, 6, Rng::quat)),
                8 => {
                    anim.chans
                        .push(chan(&mut rng, a, Path::R, "STEP", d, 4, Rng::quat));
                    anim.chans
                        .push(chan(&mut rng, b, Path::T, "STEP", d, 4, mv));
                }
                9 => {
                    let mut c = chan(&mut rng, b, Path::T, "CUBICSPLINE", d, 4, mv);
                    // Tangents small next to the values.
                    for (k, v) in c.values.iter_mut().enumerate() {
                        if k % 3 != 1 {
                            *v = v.map(|x| x * 0.2);
                        }
                    }
                    anim.chans.push(c);
                    let mut r = chan(&mut rng, a, Path::R, "CUBICSPLINE", d, 4, Rng::quat);
                    for (k, v) in r.values.iter_mut().enumerate() {
                        if k % 3 != 1 {
                            *v = [0.0; 4];
                        }
                    }
                    anim.chans.push(r);
                }
                10 => {
                    anim.chans
                        .push(chan(&mut rng, a, Path::S, "LINEAR", d, 4, |r| {
                            [r.range(0.5, 2.0); 4]
                        }));
                    anim.chans
                        .push(chan(&mut rng, b, Path::S, "LINEAR", d, 3, |r| {
                            [r.range(0.5, 2.0), r.range(0.5, 2.0), r.range(0.5, 2.0), 0.0]
                        }));
                }
                11 => {
                    anim.chans
                        .push(chan(&mut rng, a, Path::R, "LINEAR", d, 5, Rng::quat));
                    f.anims.push(anim);
                    anim = FAnim {
                        name: "second".into(),
                        chans: vec![chan(&mut rng, b, Path::T, "LINEAR", 3.0, 7, mv)],
                    };
                }
                12 => {
                    let mut c = chan(&mut rng, b, Path::R, "LINEAR", d, 5, Rng::quat);
                    let last = c.values.len() - 1;
                    c.values[last] = c.values[0];
                    anim.chans.push(c);
                }
                19 => {
                    // A deep tree under the arm, every node turning, the turns in normalized shorts.
                    let mut parent = b;
                    for k in 0..6 {
                        let n = f.node(
                            &format!("link{}", ["a", "b", "c", "d", "e", "f"][k]),
                            Some(parent),
                            [0.0, 0.3, 0.1],
                            rng.quat(),
                            [1.0; 3],
                        );
                        f.meshes
                            .push(rigid(n, vec![boxed([0.0; 3], 0.05, None, Ix::U16)]));
                        let mut c = chan(&mut rng, n, Path::R, "LINEAR", 2.0, 5, Rng::quat);
                        c.i16_rotations = true;
                        anim.chans.push(c);
                        parent = n;
                    }
                }
                _ => {}
            }
            f.anims.push(anim);
        }
        13..=16 => {
            let root = f.node("rig", None, [0.0; 3], no, [1.0; 3]);
            let bones = chain(&mut f, &mut rng, [2, 3, 3, 4][i - 13]);
            f.nodes[bones[0]].parent = Some(root);
            let holder = f.node("skinned", None, [0.0; 3], no, [1.0; 3]);
            let (influences, u8j) = [(2, false), (2, false), (4, true), (3, false)][i - 13];
            let mut m = skin_mesh(&f, holder, bones.clone(), &mut rng, influences, u8j);
            if i == 16 {
                f.materials = 2;
                m.prims[0].material = Some(0);
                let mut second = boxed([0.0; 3], 0.1, Some(1), Ix::U16);
                second.joints = vec![[1, 0, 0, 0]; 8];
                second.weights = vec![[1.0, 0.0, 0.0, 0.0]; 8];
                m.prims.push(second);
                let tip = f.node("tip", Some(bones[2]), [0.0, 0.2, 0.0], rng.quat(), [1.0; 3]);
                f.meshes
                    .push(rigid(tip, vec![boxed([0.0; 3], 0.05, None, Ix::U16)]));
            }
            f.meshes.push(m);
            if i > 13 {
                let d = [1.0, 2.0, 1.0][i - 14];
                let interp = if i == 16 { "CUBICSPLINE" } else { "LINEAR" };
                let chans = bones
                    .iter()
                    .map(|&b| {
                        let mut c = chan(&mut rng, b, Path::R, interp, d, 4, Rng::quat);
                        if interp == "CUBICSPLINE" {
                            for (k, v) in c.values.iter_mut().enumerate() {
                                if k % 3 != 1 {
                                    *v = [0.0; 4];
                                }
                            }
                        }
                        c
                    })
                    .collect();
                f.anims.push(FAnim {
                    name: "bend".into(),
                    chans,
                });
            }
        }
        17 => {
            let n = f.node("crate", None, rng.v3(1.0), rng.quat(), [1.0; 3]);
            let c = f.node("Col-1", Some(n), [0.0; 3], no, [1.0; 3]);
            let l = f.node("LOSCol-9", Some(n), [0.0; 3], no, [1.0; 3]);
            f.meshes
                .push(rigid(n, vec![boxed([0.0; 3], 0.5, None, Ix::U16)]));
            f.meshes
                .push(rigid(c, vec![boxed([0.0; 3], 0.5, None, Ix::U16)]));
            f.meshes
                .push(rigid(l, vec![boxed([0.0; 3], 0.5, None, Ix::U16)]));
        }
        _ => unreachable!(),
    }
    // The reader takes meshes in the order of their nodes.
    f.meshes.sort_by_key(|m| m.node);
    f
}

fn free(_: u64) -> bool {
    true
}

fn far(a: V3, b: [f32; 3]) -> f64 {
    (0..3)
        .map(|k| (a[k] - f64::from(b[k])).abs())
        .fold(0.0, f64::max)
}

/// Where the shape puts every vertex of object `o`'s one mesh, and the mesh's own vertices.
fn shape_vertices(
    shape: &Shape,
    render: &[Option<torque_shapes::shape::MeshRender>],
    o: usize,
    nodes: &[[f32; 16]],
) -> (Vec<[f32; 3]>, Vec<[f32; 3]>) {
    let obj = &shape.objects[o];
    let start = obj.start_mesh_index as usize;
    let k = (0..obj.num_meshes as usize)
        .find(|&d| shape.meshes[start + d].is_some())
        .unwrap();
    let (mesh, r) = (
        shape.meshes[start + k].as_ref().unwrap(),
        render[start + k].as_ref().unwrap(),
    );
    match &r.extra {
        MeshExtra::Skin(sk) => {
            let bones: Vec<[f32; 16]> = (0..sk.num_nodes)
                .map(|b| mat_mul(&nodes[sk.node_index[b] as usize], &sk.initial_transforms[b]))
                .collect();
            let mut out = vec![[0.0f32; 3]; sk.num_initial_verts];
            for w in 0..sk.num_weights {
                let (v, b) = (sk.vertex_index[w] as usize, sk.bone_index[w] as usize);
                let q = mul_p(&bones[b], sk.initial_verts[v]);
                for c in 0..3 {
                    out[v][c] += q[c] * sk.weight[w];
                }
            }
            (out, sk.initial_verts.to_vec())
        }
        _ => {
            let m = nodes[obj.node as usize];
            (
                mesh.verts().iter().map(|&p| mul_p(&m, p)).collect(),
                mesh.verts().to_vec(),
            )
        }
    }
}

#[test]
fn twenty_fixtures_read_back_with_equal_counts_positions_and_poses() {
    let mut report = Vec::new();
    for i in 0..20 {
        let f = fixture(i);
        let glb = f.glb();
        let model = gltf::read(&glb, &mut |_, _| None, &Options::default(), &mut free)
            .unwrap_or_else(|e| panic!("fixture {i}: {e}"));
        let bytes = torque_shapes::write::write_dts(&dts::to_dts(&model).unwrap());
        let back = torque_shapes::dts::parse(&bytes, &mut free)
            .unwrap_or_else(|e| panic!("fixture {i}: {e}"));
        let shape = &back.shape;

        // Counts: every node, every mesh (one object each, the box hull after them when there is no collision mesh), every
        // vertex, every animation.
        assert_eq!(shape.nodes.len(), f.nodes.len(), "fixture {i}: nodes");
        let hull = usize::from(i != 17);
        assert_eq!(
            shape.objects.len(),
            f.meshes.len() + hull,
            "fixture {i}: meshes"
        );
        assert_eq!(
            shape.sequences.len(),
            f.anims.len(),
            "fixture {i}: sequences"
        );
        // A loop (its curves end where they start) plays as a cyclic sequence.
        assert_eq!(
            i == 12,
            shape.sequences.iter().any(|s| s.flags & 16 != 0),
            "fixture {i}: cyclic"
        );
        let carried = |node: usize| {
            let mut s = [1.0f64; 3];
            let mut at = Some(node);
            while let Some(n) = at {
                let ns = f.nodes[n].s;
                // The glTF's scale turned into Torque's axes.
                s = [s[0] * ns[0], s[1] * ns[2], s[2] * ns[1]];
                at = f.nodes[n].parent;
            }
            s
        };
        let rest_shape = ShapeInstance::new(shape, &mut free).unwrap();
        let mut worst_pos = 0.0f64;
        for (o, m) in f.meshes.iter().enumerate() {
            let (_, local) = shape_vertices(shape, &back.render, o, rest_shape.node_transforms());
            let expected: Vec<V3> = m
                .prims
                .iter()
                .flat_map(|p| p.positions.iter().copied())
                .collect();
            assert_eq!(
                local.len(),
                expected.len(),
                "fixture {i}: vertices of mesh {o}"
            );
            // A mesh under a uniform scale keeps its vertices scaled; under a non-uniform one also sheared (the pose check
            // below holds those).
            let s = if m.skin.is_some() {
                [1.0; 3]
            } else {
                carried(m.node)
            };
            let sheared = f.nodes[m.node].parent.is_some_and(|p| {
                let c = carried(p);
                (c[0] - c[1]).abs() > 1e-9 || (c[1] - c[2]).abs() > 1e-9
            });
            if sheared {
                continue;
            }
            for (e, v) in expected.iter().zip(&local) {
                let t = torque(*e);
                worst_pos = worst_pos.max(far([t[0] * s[0], t[1] * s[1], t[2] * s[2]], *v));
            }
        }
        assert!(
            worst_pos <= 1e-4,
            "fixture {i}: a vertex {worst_pos} from the glTF's"
        );

        // Poses: at rest, then at 10 times of each sequence.
        let mut worst_pose = 0.0f64;
        let mut check = |world: &[M], nodes: &[[f32; 16]], what: &str| {
            for (n, w) in world.iter().enumerate() {
                let o = torque([w[3], w[7], w[11]]);
                let d = far(o, [nodes[n][3], nodes[n][7], nodes[n][11]]);
                assert!(d <= 1e-3, "fixture {i} {what}: node {n} origin {d} off");
                worst_pose = worst_pose.max(d);
            }
            for o in 0..f.meshes.len() {
                let (posed, _) = shape_vertices(shape, &back.render, o, nodes);
                for (e, v) in f.posed(o, world).iter().zip(&posed) {
                    let d = far(*e, *v);
                    assert!(
                        d <= 1e-3,
                        "fixture {i} {what}: a vertex of mesh {o} {d} off"
                    );
                    worst_pose = worst_pose.max(d);
                }
            }
        };
        check(&f.world(None), rest_shape.node_transforms(), "at rest");
        for a in 0..f.anims.len() {
            let d = f.duration(a);
            let seq = shape
                .sequences
                .iter()
                .position(|s| shape.names[s.name as usize] == f.anims[a].name.as_bytes())
                .unwrap();
            for j in 0..10 {
                let t = d * j as f64 / 10.0;
                let mut inst = ShapeInstance::new(shape, &mut free).unwrap();
                let th = inst.create_thread(shape).unwrap();
                inst.set_sequence(shape, th, seq as i32, (t / d) as f32);
                inst.animate(shape, 0, None, &mut free).unwrap();
                check(
                    &f.world(Some((a, t))),
                    inst.node_transforms(),
                    &format!("sequence {a} at {t:.3} s"),
                );
            }
        }
        report.push(format!("fixture {i}: {} nodes, {} meshes, {} sequences; positions within {worst_pos:.1e}, poses within {worst_pose:.1e}", f.nodes.len(), f.meshes.len(), f.anims.len()));
    }
    println!("{}", report.join("\n"));
}
