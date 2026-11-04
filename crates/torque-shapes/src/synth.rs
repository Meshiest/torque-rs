//! Synthetic shapes for the tests and the fuzz: models made from nothing by a seeded generator, the writer
//! ([`crate::write`]) turns them into `.dts` and `.dsq` bytes, and [`mutate`] damages the bytes. Nothing here is derived from a
//! shape file (the fuzz never mutates one), so a seed is a complete, committable test case.
//!
//! The generator makes every kind of structure the reader has to handle: nodes, objects, decals, `.ifl` materials, sub-shapes,
//! every keyframe array, standard, skin, sorted and null meshes, meshes that share a parent's arrays, sequences with their
//! membership sets, and material lists (complete and ended early). Its models are valid (every count and index in range, a
//! parent mesh earlier than its children, sizes within the caps) and, when asked, consistent (every bound the semantic check
//! recomputes is the recomputed value).

use std::sync::Arc;

use crate::bounds;
use crate::dsq::{Dsq, DsqSequence};
use crate::shape::{
    Decal, Detail, FileExtras, IflMaterial, IntSet, Material, Mesh, MeshExtra, MeshKind,
    MeshRender, Model, Node, Object, ObjectState, Primitive, RawMaterial, RawMaterials, SeqLens,
    Sequence, Shape, SkinData, SortedData, SubShape, Trigger, check_sequence,
};

/// A seeded generator (splitmix64): the same seed gives the same case on every machine.
#[derive(Debug, Clone)]
pub struct Rng(pub u64);

impl Rng {
    pub fn bits(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// A number in `0..n` (0 when `n` is 0).
    pub fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.bits() % n as u64) as usize
        }
    }

    /// True once in `n` times.
    pub fn one_in(&mut self, n: usize) -> bool {
        self.below(n) == 0
    }

    fn float(&mut self) -> f32 {
        // A finite float with a little spread: whole and fractional parts of both signs.
        (self.bits() % 20_001) as f32 / 1000.0 - 10.0
    }

    fn f3(&mut self) -> [f32; 3] {
        [self.float(), self.float(), self.float()]
    }

    fn quat(&mut self) -> [i16; 4] {
        [
            self.bits() as i16,
            self.bits() as i16,
            self.bits() as i16,
            self.bits() as i16,
        ]
    }

    fn word(&mut self) -> i32 {
        self.bits() as i32
    }
}

fn rc<T>(v: Vec<T>) -> Arc<[T]> {
    Arc::from(v)
}

fn set(rng: &mut Rng, bits: usize) -> IntSet {
    let mut s = IntSet::default();
    if rng.one_in(3) {
        s.first = rng.below(3) as u32;
    }
    for _ in 0..rng.below(4) {
        s.set(rng.below(bits));
    }
    s
}

/// A random sequence (the name index is left for the caller).
fn sequence(rng: &mut Rng, name: i32) -> Sequence {
    Sequence {
        name,
        flags: rng.below(128) as u32,
        num_keyframes: rng.below(10) as i32,
        duration: rng.float().abs(),
        priority: rng.below(8) as i32,
        first_ground_frame: rng.below(4) as i32,
        num_ground_frames: rng.below(3) as i32,
        base_rotation: rng.below(4) as i32,
        base_translation: rng.below(4) as i32,
        base_scale: rng.below(4) as i32,
        base_object_state: rng.below(4) as i32,
        base_decal_state: rng.below(4) as i32,
        first_trigger: rng.below(3) as i32,
        num_triggers: rng.below(3) as i32,
        tool_begin: rng.float(),
        rotation_matters: set(rng, 8),
        translation_matters: set(rng, 8),
        scale_matters: set(rng, 8),
        decal_matters: set(rng, 8),
        ifl_matters: set(rng, 8),
        vis_matters: set(rng, 8),
        frame_matters: set(rng, 8),
        mat_frame_matters: set(rng, 8),
    }
}

/// A random sequence whose keyframe indices lie inside arrays of the lengths `lens` (`check_sequence`): a few random tries, then the
/// one that moves nothing.
fn valid_sequence(rng: &mut Rng, name: i32, lens: &SeqLens, states: bool) -> Sequence {
    for _ in 0..4 {
        let mut s = sequence(rng, name);
        s.num_keyframes = rng.below(3) as i32;
        s.num_ground_frames = rng.below(2) as i32;
        s.num_triggers = rng.below(2) as i32;
        s.base_rotation = rng.below(2) as i32;
        s.base_translation = rng.below(2) as i32;
        s.base_scale = rng.below(2) as i32;
        s.first_ground_frame = rng.below(2) as i32;
        s.first_trigger = rng.below(2) as i32;
        s.base_object_state = rng.below(2) as i32;
        s.base_decal_state = rng.below(2) as i32;
        if check_sequence(&s, lens, states).is_ok() {
            return s;
        }
    }
    let mut s = sequence(rng, name);
    s.num_keyframes = 0;
    s.num_ground_frames = 0;
    s.num_triggers = 0;
    s.base_rotation = 0;
    s.base_translation = 0;
    s.base_scale = 0;
    s.first_ground_frame = 0;
    s.first_trigger = 0;
    s.base_object_state = 0;
    s.base_decal_state = 0;
    s
}

/// One mesh and its render data, with `parent` an earlier mesh (or -1).
fn mesh(
    rng: &mut Rng,
    consistent: bool,
    parent: Option<(&Mesh, &MeshRender)>,
    kind: MeshKind,
) -> (Mesh, MeshRender) {
    // How many vertices and texture vertices the mesh may claim.
    let (cap_v, cap_t) = match parent {
        Some((pm, pr)) => (
            match &pr.extra {
                MeshExtra::Skin(s) => s.num_initial_verts,
                _ => pm.num_verts,
            },
            pm.num_tverts,
        ),
        None => (24, 24),
    };
    let num_verts = rng.below(cap_v + 1);
    let num_tverts = rng.below(cap_t + 1);
    let (positions, tverts, normals, encoded);
    if let Some((pm, pr)) = parent {
        let (pos, nor) = match &pr.extra {
            MeshExtra::Skin(s) => (Arc::clone(&s.initial_verts), Arc::clone(&s.initial_normals)),
            _ => (Arc::clone(&pm.positions), Arc::clone(&pr.normals)),
        };
        positions = pos;
        tverts = Arc::clone(&pr.tverts);
        normals = nor;
        encoded = Arc::clone(&pr.encoded_normals);
    } else {
        positions = rc((0..num_verts).map(|_| rng.f3()).collect());
        tverts = rc((0..num_tverts)
            .map(|_| [rng.float(), rng.float()])
            .collect());
        normals = rc((0..num_verts).map(|_| rng.f3()).collect());
        encoded = rc((0..num_verts).map(|_| rng.bits() as u8).collect());
    }
    // Indices inside the vertices, primitives inside the indices (`Shape::validate`).
    let indices: Vec<u16> = if num_verts == 0 {
        Vec::new()
    } else {
        (0..rng.below(30))
            .map(|_| rng.below(num_verts) as u16)
            .collect()
    };
    let primitives: Vec<Primitive> = (0..rng.below(5))
        .map(|_| {
            let start = rng.below(indices.len() + 1);
            Primitive {
                start: start as u16,
                num_elements: rng.below(indices.len() - start + 1) as u16,
                material: rng.bits() as u32,
            }
        })
        .collect();
    let merge_indices: Vec<u16> = (0..rng.below(4)).map(|_| rng.bits() as u16).collect();

    let mut extra = MeshExtra::None;
    let mut held = num_verts;
    match kind {
        MeshKind::Skin => {
            let (sz, tr, wt, nd);
            let parent_skin = parent.and_then(|(_, r)| match &r.extra {
                MeshExtra::Skin(s) => Some(s),
                _ => None,
            });
            let data = if let Some(p) = parent_skin {
                sz = rng.below(p.num_initial_verts + 1);
                tr = rng.below(p.num_transforms + 1);
                wt = rng.below(p.num_weights + 1);
                nd = rng.below(p.num_nodes + 1);
                SkinData {
                    num_initial_verts: sz,
                    num_transforms: tr,
                    num_weights: wt,
                    num_nodes: nd,
                    initial_verts: Arc::clone(&p.initial_verts),
                    initial_normals: Arc::clone(&p.initial_normals),
                    initial_encoded_normals: Arc::clone(&p.initial_encoded_normals),
                    initial_transforms: Arc::clone(&p.initial_transforms),
                    vertex_index: Arc::clone(&p.vertex_index),
                    bone_index: Arc::clone(&p.bone_index),
                    weight: Arc::clone(&p.weight),
                    node_index: Arc::clone(&p.node_index),
                }
            } else {
                sz = rng.below(10);
                tr = rng.below(4);
                wt = rng.below(10);
                nd = rng.below(4);
                SkinData {
                    num_initial_verts: sz,
                    num_transforms: tr,
                    num_weights: wt,
                    num_nodes: nd,
                    initial_verts: rc((0..sz).map(|_| rng.f3()).collect()),
                    initial_normals: rc((0..sz).map(|_| rng.f3()).collect()),
                    initial_encoded_normals: rc((0..sz).map(|_| rng.bits() as u8).collect()),
                    initial_transforms: rc((0..tr)
                        .map(|_| std::array::from_fn(|_| rng.float()))
                        .collect()),
                    vertex_index: rc((0..wt).map(|_| rng.below(sz + 1) as i32).collect()),
                    bone_index: rc((0..wt).map(|_| rng.below(nd + 1) as i32).collect()),
                    weight: rc((0..wt).map(|_| rng.float().abs() / 10.0).collect()),
                    node_index: rc((0..nd).map(|_| rng.below(5) as i32).collect()),
                }
            };
            held = held.max(data.num_initial_verts);
            extra = MeshExtra::Skin(data);
        }
        MeshKind::Sorted => {
            extra = MeshExtra::Sorted(SortedData {
                clusters: (0..rng.below(3))
                    .map(|_| std::array::from_fn(|_| rng.bits() as u32))
                    .collect(),
                start_cluster: (0..rng.below(3)).map(|_| rng.bits() as u32).collect(),
                first_vert: (0..rng.below(3)).map(|_| rng.bits() as u32).collect(),
                num_verts: (0..rng.below(3)).map(|_| rng.bits() as u32).collect(),
                first_tvert: (0..rng.below(3)).map(|_| rng.bits() as u32).collect(),
                always_write_depth: rng.below(2) as u32,
            });
        }
        _ => {}
    }
    // Frames: one of each, or several when the vertices divide.
    let (frames, mat_frames, per_frame) = if held >= 2 && num_tverts >= 2 && rng.one_in(3) {
        (2, 1, (held / 2).min(num_tverts))
    } else {
        (1, 1, held.min(num_tverts))
    };
    let mut m = Mesh {
        kind,
        parent: -1,
        num_frames: frames,
        num_mat_frames: mat_frames,
        verts_per_frame: per_frame as i32,
        flags: rng.bits() as u32 & 0x7,
        bounds: [0.0; 6],
        center: [0.0; 3],
        radius: 0,
        num_verts,
        num_tverts,
        positions,
        primitives,
        indices,
    };
    let r = MeshRender {
        tverts,
        normals,
        encoded_normals: encoded,
        merge_indices,
        extra,
    };
    if consistent {
        let verts = match &r.extra {
            MeshExtra::Skin(s) => s
                .initial_verts
                .get(..s.num_initial_verts)
                .unwrap_or(&s.initial_verts),
            _ => m.verts(),
        };
        let c = bounds::compute(verts);
        m.bounds = c.bounds;
        m.center = c.center;
        m.radius = c.radius as i32;
    } else {
        m.bounds = [
            rng.float(),
            rng.float(),
            rng.float(),
            rng.float(),
            rng.float(),
            rng.float(),
        ];
        m.center = rng.f3();
        m.radius = rng.below(5) as i32;
    }
    (m, r)
}

/// A random valid model. `consistent` makes the bounds the recomputed ones (so `bounds::check` accepts it); `size` scales the
/// number of nodes, objects and meshes (1 is a handful).
pub fn model(rng: &mut Rng, consistent: bool, size: usize) -> Model {
    let size = size.max(1);
    let mut s = Shape::empty();
    s.exporter = rng.bits() as u16;
    let names = 3 + rng.below(8 * size);
    s.names = (0..names)
        .map(|i| match i {
            0 => b"Collision-1".to_vec(),
            1 => b"LOS-9".to_vec(),
            _ => format!("n{}{}", i, rng.below(100)).into_bytes(),
        })
        .collect();
    let nodes = rng.below(4 * size + 1);
    for i in 0..nodes {
        s.nodes.push(Node {
            name: rng.below(names) as i32,
            parent: if i == 0 || rng.one_in(4) {
                -1
            } else {
                rng.below(i) as i32
            },
            first_object: rng.word() % 8,
            first_child: rng.word() % 8,
            next_sibling: rng.word() % 8,
        });
        s.default_rotations.push(rng.quat());
        s.default_translations.push(rng.f3());
    }
    s.node_rotations = (0..rng.below(4)).map(|_| rng.quat()).collect();
    s.node_translations = (0..rng.below(4)).map(|_| rng.f3()).collect();
    s.node_uniform_scales = (0..rng.below(3)).map(|_| rng.float()).collect();
    s.node_aligned_scales = (0..rng.below(3)).map(|_| rng.f3()).collect();
    let arbitrary = rng.below(3);
    s.node_arbitrary_scale_rots = (0..arbitrary).map(|_| rng.quat()).collect();
    s.node_arbitrary_scale_factors = (0..arbitrary).map(|_| rng.f3()).collect();
    let ground = rng.below(3);
    s.ground_translations = (0..ground).map(|_| rng.f3()).collect();
    s.ground_rotations = (0..ground).map(|_| rng.quat()).collect();
    s.object_states = (0..rng.below(3))
        .map(|_| ObjectState {
            vis: rng.float().abs(),
            frame: rng.word() % 5,
            mat_frame: rng.word() % 5,
        })
        .collect();
    s.decal_states = (0..rng.below(3)).map(|_| rng.word() % 5).collect();
    s.triggers = (0..rng.below(3))
        .map(|_| Trigger {
            state: rng.bits() as u32,
            pos: rng.float().abs(),
        })
        .collect();
    s.decals = (0..rng.below(2))
        .map(|_| Decal {
            name: rng.below(names) as i32,
            num_meshes: rng.below(3) as i32,
            start_mesh_index: rng.below(3) as i32,
            object: rng.below(3) as i32,
            next_sibling: -1,
        })
        .collect();
    s.ifl_materials = (0..rng.below(2))
        .map(|_| IflMaterial {
            name: rng.below(names) as i32,
            material_slot: rng.below(3) as i32,
            first_frame: rng.below(3) as i32,
            first_frame_off_time: rng.below(3) as i32,
            num_frames: rng.below(4) as i32,
        })
        .collect();

    // Meshes: some own their arrays, some share an earlier mesh's, some are absent.
    let mesh_count = rng.below(6 * size + 1);
    let mut meshes: Vec<Option<Mesh>> = Vec::new();
    let mut render: Vec<Option<MeshRender>> = Vec::new();
    for i in 0..mesh_count {
        if rng.one_in(4) {
            meshes.push(None);
            render.push(None);
            continue;
        }
        let kind = [
            MeshKind::Standard,
            MeshKind::Standard,
            MeshKind::Skin,
            MeshKind::Sorted,
        ][rng.below(4)];
        let parent = if i > 0 && rng.one_in(4) {
            let p = rng.below(i);
            match (&meshes[p], &render[p]) {
                (Some(m), Some(r)) => Some((p, m, r)),
                _ => None,
            }
        } else {
            None
        };
        // A skin child needs a skin parent; the other kinds share with any.
        let kind = match parent {
            Some((_, _, r)) if kind == MeshKind::Skin && !matches!(r.extra, MeshExtra::Skin(_)) => {
                MeshKind::Standard
            }
            _ => kind,
        };
        let (mut m, r) = mesh(rng, consistent, parent.map(|(_, m, r)| (m, r)), kind);
        if let Some((p, _, _)) = parent {
            m.parent = p as i32;
        }
        meshes.push(Some(m));
        render.push(Some(r));
    }
    s.meshes = meshes;

    let objects = rng.below(3 * size + 1);
    for _ in 0..objects {
        let num_object_meshes = rng.below(3).min(mesh_count);
        s.objects.push(Object {
            name: rng.below(names) as i32,
            num_meshes: num_object_meshes as i32,
            start_mesh_index: rng.below(mesh_count - num_object_meshes + 1) as i32,
            node: if nodes == 0 {
                -1
            } else {
                rng.below(nodes) as i32
            },
            next_sibling: rng.word() % 4,
            first_decal: -1,
        });
    }
    s.sub_shapes = vec![SubShape {
        first_node: 0,
        first_object: 0,
        first_decal: 0,
        num_nodes: nodes as i32,
        num_objects: objects as i32,
        num_decals: s.decals.len() as i32,
    }];
    for i in 0..rng.below(4) + 1 {
        s.details.push(Detail {
            name: rng.below(names) as i32,
            sub_shape: 0,
            object_detail: i as i32,
            size: (100 >> i.min(6)) as f32,
            average_error: rng.float().abs(),
            max_error: rng.float().abs(),
            poly_count: rng.below(100) as i32,
        });
    }
    s.smallest_visible_size = rng.below(100) as i32;
    s.smallest_visible_dl = rng.below(3) as i32;
    if consistent {
        s.bounds = [-3.0, -2.0, 0.0, 3.0, 2.0, 6.0];
        let (c, r, t) = bounds::shape_derived(&s.bounds);
        s.center = c;
        s.radius = r;
        s.tube_radius = t;
    } else {
        s.bounds = [
            rng.float(),
            rng.float(),
            rng.float(),
            rng.float(),
            rng.float(),
            rng.float(),
        ];
        s.center = rng.f3();
        s.radius = rng.float().abs();
        s.tube_radius = rng.float().abs();
    }
    for i in 0..rng.below(5) {
        let lens = s.seq_lens();
        s.sequences
            .push(valid_sequence(rng, (i % names) as i32, &lens, true));
    }

    // Materials: complete, or ended early by an empty name.
    let count = rng.below(5);
    let names_raw: Vec<Vec<u8>> = (0..count)
        .map(|i| match rng.below(3) {
            0 => format!("tex{i}").into_bytes(),
            1 => format!("dir/sub\\tex{i}").into_bytes(),
            _ => format!("m{}.ifl", rng.below(9)).into_bytes(),
        })
        .collect();
    let file_materials = if count > 0 && rng.one_in(5) {
        let cut = rng.below(count);
        RawMaterials::Early {
            declared: count as u32,
            names: names_raw[..cut].to_vec(),
            terminator: vec![0],
        }
    } else {
        RawMaterials::Complete(
            names_raw
                .into_iter()
                .map(|name| RawMaterial {
                    name,
                    flags: [0, 1, 0x0800_0000, 0x1000_0000, 0x7][rng.below(5)],
                    reflectance_map: rng.below(3) as u32,
                    bump_map: rng.below(3) as u32,
                    detail_map: rng.below(3) as u32,
                    detail_scale: rng.float().abs(),
                    reflection_amount: rng.float().abs(),
                })
                .collect(),
        )
    };
    s.materials = runtime_materials(&file_materials);
    Model {
        shape: s,
        render,
        file: FileExtras {
            version_middle: 0,
            materials: file_materials,
            left: [Vec::new(), Vec::new(), Vec::new()],
            trailing: if rng.one_in(8) {
                vec![rng.bits() as u8]
            } else {
                Vec::new()
            },
        },
    }
}

/// The runtime materials a raw list gives (`TSMaterialList::read`).
fn runtime_materials(raw: &RawMaterials) -> Vec<Material> {
    let name = |raw: &[u8]| -> Vec<u8> {
        let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
        let c = &raw[..end];
        let start = c
            .iter()
            .rposition(|&b| b == b'/' || b == b'\\')
            .map_or(0, |i| i + 1);
        c[start..].to_vec()
    };
    match raw {
        RawMaterials::Complete(list) => list
            .iter()
            .map(|m| Material {
                name: (m.flags & crate::matlist::IFL_MATERIAL == 0).then(|| name(&m.name)),
                flags: m.flags,
            })
            .collect(),
        RawMaterials::Early { names, .. } => names
            .iter()
            .map(|n| Material {
                name: Some(name(n)),
                flags: 0,
            })
            .collect(),
    }
}

/// A random sequence file for a shape whose nodes are the ones the generator names (`root`, `arm`): every node name is one the
/// shape has, so the import can succeed.
pub fn dsq(rng: &mut Rng, node_names: &[&[u8]], sequences: usize) -> Dsq {
    let rots = rng.below(4);
    let ground = rng.below(3);
    let arbitrary = rng.below(3);
    let (translations, uniform, aligned, triggers) =
        (rng.below(4), rng.below(3), rng.below(3), rng.below(3));
    let lens = SeqLens {
        rotations: rots,
        translations,
        uniform,
        aligned,
        arbitrary,
        ground,
        triggers,
        object_states: 0,
        decal_states: 0,
    };
    Dsq {
        version: 24,
        node_names: node_names.iter().map(|n| n.to_vec()).collect(),
        spare: [rng.below(3) as i32, rng.below(3) as i32],
        node_rotations: (0..rots).map(|_| rng.quat()).collect(),
        node_translations: (0..translations).map(|_| rng.f3()).collect(),
        uniform_scales: (0..uniform).map(|_| rng.float()).collect(),
        aligned_scales: (0..aligned).map(|_| rng.f3()).collect(),
        arbitrary_scale_rots: (0..arbitrary).map(|_| rng.quat()).collect(),
        arbitrary_scale_factors: (0..arbitrary).map(|_| rng.f3()).collect(),
        ground_translations: (0..ground).map(|_| rng.f3()).collect(),
        ground_rotations: (0..ground).map(|_| rng.quat()).collect(),
        object_states: rng.below(3) as u32,
        sequences: (0..sequences)
            .map(|i| DsqSequence {
                name: if rng.one_in(9) {
                    Vec::new()
                } else {
                    format!("seq{}", i % 7).into_bytes()
                },
                sequence: valid_sequence(rng, -1, &lens, false),
            })
            .collect(),
        triggers: (0..triggers)
            .map(|_| Trigger {
                state: rng.bits() as u32,
                pos: rng.float().abs(),
            })
            .collect(),
        trailing: if rng.one_in(10) { vec![7] } else { Vec::new() },
    }
}

/// A base shape with the nodes `root` and `arm` for the imports of the sequence-growth case.
pub fn base_shape() -> Shape {
    let mut shape = Shape::empty();
    shape.names = vec![b"root".to_vec(), b"arm".to_vec()];
    let node = |name, parent| Node {
        name,
        parent,
        first_object: -1,
        first_child: -1,
        next_sibling: -1,
    };
    shape.nodes = vec![node(0, -1), node(1, 0)];
    shape.default_rotations = vec![[0; 4]; 2];
    shape.default_translations = vec![[0.0; 3]; 2];
    shape
}

/// The values a mutation writes over a word: the edges of the ranges a count or a size can take.
const EDGES: [u32; 14] = [
    0,
    1,
    2,
    3,
    4,
    0xff,
    0x100,
    0xffff,
    0x1_0000,
    0x00ff_ffff,
    0x7fff_ffff,
    0x8000_0000,
    0xffff_ffff,
    0x4000_0000,
];

/// Damages `bytes` in one of the ways a hostile file can be wrong: bit flips, truncation, an inflated count or size (a word set to an edge
/// value, in the header or anywhere), an index out of range (a word or a short nudged past what it indexes), a 2^31 size, a
/// duplicated or removed piece. One to three damages are applied.
pub fn mutate(rng: &mut Rng, bytes: &[u8]) -> Vec<u8> {
    let mut out = bytes.to_vec();
    for _ in 0..1 + rng.below(3) {
        if out.is_empty() {
            break;
        }
        match rng.below(9) {
            0 => {
                for _ in 0..1 + rng.below(8) {
                    let at = rng.below(out.len());
                    out[at] ^= 1 << rng.below(8);
                }
            }
            1 => {
                let cut = rng.below(out.len());
                out.truncate(cut);
            }
            2 => {
                // A word anywhere set to an edge value.
                let at = rng.below(out.len().saturating_sub(3).max(1)) & !3;
                if at + 4 <= out.len() {
                    out[at..at + 4].copy_from_slice(&EDGES[rng.below(EDGES.len())].to_le_bytes());
                }
            }
            3 => {
                // A header size or a count of the first block set to an edge value (2^31 included).
                let at = (rng.below(8) + 1).min(out.len() / 4) * 4;
                if at + 4 <= out.len() {
                    out[at..at + 4].copy_from_slice(&EDGES[rng.below(EDGES.len())].to_le_bytes());
                }
            }
            4 => {
                // An index nudged one past or far out: add to a word.
                let at = rng.below(out.len().saturating_sub(3).max(1)) & !3;
                if at + 4 <= out.len() {
                    let w = u32::from_le_bytes([out[at], out[at + 1], out[at + 2], out[at + 3]]);
                    let d = [1u32, 2, 0x100, 0x1_0000, 0xffff_ffff][rng.below(5)];
                    out[at..at + 4].copy_from_slice(&w.wrapping_add(d).to_le_bytes());
                }
            }
            5 => {
                // A short set to an edge.
                let at = rng.below(out.len().saturating_sub(1).max(1)) & !1;
                if at + 2 <= out.len() {
                    out[at..at + 2]
                        .copy_from_slice(&(EDGES[rng.below(EDGES.len())] as u16).to_le_bytes());
                }
            }
            6 => {
                // A piece removed.
                let at = rng.below(out.len());
                let len = (1 + rng.below(16)).min(out.len() - at);
                out.drain(at..at + len);
            }
            7 => {
                // A piece duplicated.
                let at = rng.below(out.len());
                let len = (1 + rng.below(32)).min(out.len() - at);
                let piece = out[at..at + len].to_vec();
                for (i, b) in piece.into_iter().enumerate() {
                    out.insert(at + i, b);
                }
            }
            _ => {
                // The version word.
                out[0] = [0, 17, 18, 19, 23, 24, 25, 255][rng.below(8)];
            }
        }
    }
    out
}

/// The first named case, **mesh cloning**: one parent mesh of `parent_verts` vertices and `children` meshes
/// that name it as their parent and claim all its vertices. Each child costs a few bytes and no vertex data, so the file stays
/// small while the claimed vertices grow without limit; the per-shape totals must refuse it.
pub fn cloning_shape(parent_verts: usize, children: usize) -> Vec<u8> {
    let mut s = Shape::empty();
    s.names = vec![b"root".to_vec()];
    let positions: Arc<[[f32; 3]]> = Arc::from(vec![[1.0, 2.0, 3.0]; parent_verts]);
    let parent = Mesh {
        kind: MeshKind::Standard,
        parent: -1,
        num_frames: 1,
        num_mat_frames: 1,
        verts_per_frame: 0,
        flags: 0,
        bounds: [0.0; 6],
        center: [0.0; 3],
        radius: 0,
        num_verts: parent_verts,
        num_tverts: 0,
        positions,
        primitives: Vec::new(),
        indices: Vec::new(),
    };
    let render = MeshRender {
        normals: Arc::from(vec![[0.0f32, 0.0, 1.0]; parent_verts]),
        encoded_normals: Arc::from(vec![0u8; parent_verts]),
        ..MeshRender::default()
    };
    let mut meshes = vec![Some(parent.clone())];
    let mut renders = vec![Some(render.clone())];
    for _ in 0..children {
        let mut child = parent.clone();
        child.parent = 0;
        meshes.push(Some(child));
        renders.push(Some(render.clone()));
    }
    s.meshes = meshes;
    crate::write::write_dts(&Model {
        shape: s,
        render: renders,
        file: FileExtras::default(),
    })
}

/// The second named case, **frame products**: one mesh whose frame counts and vertices per frame are the given words (their
/// product overflows 32 and 64 bits for the edge values); the checked products must refuse it.
pub fn frame_product_shape(frames: i32, mat_frames: i32, per_frame: i32) -> Vec<u8> {
    let mut s = Shape::empty();
    s.names = vec![b"root".to_vec()];
    let mut rng = Rng(7);
    let (mut m, r) = mesh(&mut rng, false, None, MeshKind::Standard);
    m.num_frames = frames;
    m.num_mat_frames = mat_frames;
    m.verts_per_frame = per_frame;
    s.meshes = vec![Some(m)];
    crate::write::write_dts(&Model {
        shape: s,
        render: vec![Some(r)],
        file: FileExtras::default(),
    })
}

/// A `.dsq` of `sequences` sequences for [`base_shape`] (the third named case, **sequence growth**: the same file imported again
/// and again into a kept shape, as many `TSShapeConstructor` datablocks naming one `baseShape` do).
pub fn growth_dsq(sequences: usize) -> Dsq {
    let mut rng = Rng(11);
    dsq(&mut rng, &[b"root", b"arm"], sequences)
}

/// A small valid shape for the preload tests: a `root` node and the `extra_nodes` below it, one object whose
/// meshes are two-vertex boxes (the corners `min` and `max` of each entry of `details`, the detail's name first), the given shape
/// bounds (not derived from the meshes: the preload check compares the two), and one empty sequence for each name of
/// `sequences`. Returns the model; [`crate::write::write_dts`] makes the file.
pub fn hull_shape(
    shape_bounds: [f32; 6],
    details: &[(&str, [f32; 6])],
    extra_nodes: &[&str],
    sequences: &[&str],
) -> Model {
    let mut s = Shape::empty();
    let mut names: Vec<Vec<u8>> = details.iter().map(|(n, _)| n.as_bytes().to_vec()).collect();
    let root = names.len() as i32;
    names.push(b"root".to_vec());
    for n in extra_nodes {
        names.push(n.as_bytes().to_vec());
    }
    let object_name = names.len() as i32;
    names.push(b"obj".to_vec());
    let first_sequence_name = names.len() as i32;
    for n in sequences {
        names.push(n.as_bytes().to_vec());
    }
    s.names = names;
    let node_count = 1 + extra_nodes.len();
    s.nodes = (0..node_count)
        .map(|i| Node {
            name: root + i as i32,
            parent: if i == 0 { -1 } else { 0 },
            first_object: if i == 0 { 0 } else { -1 },
            first_child: if i == 0 && node_count > 1 { 1 } else { -1 },
            next_sibling: if i > 0 && i + 1 < node_count {
                i as i32 + 1
            } else {
                -1
            },
        })
        .collect();
    s.default_rotations = vec![[0, 0, 0, 32767]; node_count];
    s.default_translations = vec![[0.0; 3]; node_count];
    s.sub_shapes = vec![SubShape {
        first_node: 0,
        first_object: 0,
        first_decal: 0,
        num_nodes: node_count as i32,
        num_objects: 1,
        num_decals: 0,
    }];
    s.objects = vec![Object {
        name: object_name,
        num_meshes: details.len() as i32,
        start_mesh_index: 0,
        node: 0,
        next_sibling: -1,
        first_decal: -1,
    }];
    s.details = (0..details.len())
        .map(|i| Detail {
            name: i as i32,
            sub_shape: 0,
            object_detail: i as i32,
            size: 1.0,
            average_error: -1.0,
            max_error: -1.0,
            poly_count: 0,
        })
        .collect();
    let mut render = Vec::new();
    for (_, b) in details {
        let corners = [[b[0], b[1], b[2]], [b[3], b[4], b[5]]];
        s.meshes.push(Some(Mesh {
            kind: MeshKind::Standard,
            parent: -1,
            num_frames: 1,
            num_mat_frames: 1,
            verts_per_frame: 0,
            flags: 0,
            bounds: *b,
            center: [0.0; 3],
            radius: 0,
            num_verts: 2,
            num_tverts: 0,
            positions: rc(corners.to_vec()),
            primitives: Vec::new(),
            indices: Vec::new(),
        }));
        render.push(Some(MeshRender {
            normals: rc(vec![[0.0, 0.0, 1.0]; 2]),
            encoded_normals: rc(vec![0u8; 2]),
            ..MeshRender::default()
        }));
    }
    let zero = SeqLens {
        rotations: 0,
        translations: 0,
        uniform: 0,
        aligned: 0,
        arbitrary: 0,
        ground: 0,
        triggers: 0,
        object_states: 0,
        decal_states: 0,
    };
    let mut rng = Rng(5);
    s.sequences = (0..sequences.len())
        .map(|i| valid_sequence(&mut rng, first_sequence_name + i as i32, &zero, true))
        .collect();
    s.bounds = shape_bounds;
    s.center = [
        (shape_bounds[0] + shape_bounds[3]) / 2.0,
        (shape_bounds[1] + shape_bounds[4]) / 2.0,
        (shape_bounds[2] + shape_bounds[5]) / 2.0,
    ];
    Model {
        shape: s,
        render,
        file: FileExtras::default(),
    }
}

/// The eight corners of the box `b` (`x` is bit 0 of the corner's number, `y` bit 1, `z` bit 2) and the twelve triangles of its six
/// faces, each wound so that the cross product `(p3 - p1) x (p2 - p1)` (`TSMesh::buildConvexHull`) points out
/// of the box, as the strips and lists an exporter writes. `strips` makes each face a four-vertex strip (`v0 v1 v2 v3` is the
/// triangles `v0 v1 v2` and `v2 v1 v3`) and the whole mesh six primitives; else one primitive of 36 indices.
fn box_mesh(b: [f32; 6], strips: bool) -> (Vec<[f32; 3]>, Vec<u16>, Vec<Primitive>) {
    let corner = |i: usize| {
        [
            if i & 1 == 0 { b[0] } else { b[3] },
            if i & 2 == 0 { b[1] } else { b[4] },
            if i & 4 == 0 { b[2] } else { b[5] },
        ]
    };
    let verts: Vec<[f32; 3]> = (0..8).map(corner).collect();
    let mut indices = Vec::new();
    let mut prims = Vec::new();
    for axis in 0..3usize {
        for side in 0..2usize {
            let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
            let at = |du: usize, dv: usize| (side << axis) | (du << u) | (dv << v);
            let quad = [at(0, 0), at(1, 0), at(0, 1), at(1, 1)];
            // Whether the first triangle's cross product points outward, decided by the numbers.
            let out = if side == 0 { -1.0f32 } else { 1.0 };
            let p = |i: usize| verts[i];
            let cross = |a: usize, b: usize, c: usize| {
                let (e3, e2) = (
                    [p(c)[0] - p(a)[0], p(c)[1] - p(a)[1], p(c)[2] - p(a)[2]],
                    [p(b)[0] - p(a)[0], p(b)[1] - p(a)[1], p(b)[2] - p(a)[2]],
                );
                [
                    e3[1] * e2[2] - e3[2] * e2[1],
                    e3[2] * e2[0] - e3[0] * e2[2],
                    e3[0] * e2[1] - e3[1] * e2[0],
                ]
            };
            let c = cross(quad[0], quad[1], quad[2]);
            let q = if c[axis] * out >= 0.0 {
                quad
            } else {
                [quad[0], quad[2], quad[1], quad[3]]
            };
            // (v0 v1 v2) then (v2 v1 v3) keeps the winding of a strip.
            if strips {
                prims.push(Primitive {
                    start: indices.len() as u16,
                    num_elements: 4,
                    material: 0x4000_0000,
                });
                indices.extend(q.map(|i| i as u16));
            } else {
                if prims.is_empty() {
                    prims.push(Primitive {
                        start: 0,
                        num_elements: 36,
                        material: 0,
                    });
                }
                indices.extend([q[0], q[1], q[2], q[2], q[1], q[3]].map(|i| i as u16));
            }
        }
    }
    (verts, indices, prims)
}

/// A shape whose details are real solids, for the collision tests: `root` (the given rotation and translation) and one object
/// whose meshes are the boxes of `details` (each a name and the box's corners `[min, max]`, with 8 vertices and 12 triangles). The
/// shape's bounds are the given ones. `strips` writes the faces as strips, else as one list of triangles.
pub fn solid_shape(
    shape_bounds: [f32; 6],
    details: &[(&str, [f32; 6])],
    node_rotation: [i16; 4],
    node_translation: [f32; 3],
    strips: bool,
) -> Model {
    let mut model = hull_shape(shape_bounds, details, &[], &[]);
    model.shape.default_rotations = vec![node_rotation];
    model.shape.default_translations = vec![node_translation];
    for ((_, b), mesh) in details.iter().zip(model.shape.meshes.iter_mut()) {
        let (verts, indices, prims) = box_mesh(*b, strips);
        if let Some(m) = mesh {
            m.verts_per_frame = 8;
            m.num_verts = 8;
            m.num_tverts = 8;
            m.positions = rc(verts);
            m.indices = indices;
            m.primitives = prims;
        }
    }
    for r in model.render.iter_mut().flatten() {
        r.tverts = rc(vec![[0.0, 0.0]; 8]);
        r.normals = rc(vec![[0.0, 0.0, 1.0]; 8]);
        r.encoded_normals = rc(vec![0u8; 8]);
    }
    model
}

/// [`solid_shape`] as `.dts` bytes.
pub fn solid_dts(
    shape_bounds: [f32; 6],
    details: &[(&str, [f32; 6])],
    node_rotation: [i16; 4],
    node_translation: [f32; 3],
    strips: bool,
) -> Vec<u8> {
    crate::write::write_dts(&solid_shape(
        shape_bounds,
        details,
        node_rotation,
        node_translation,
        strips,
    ))
}

/// [`hull_shape`] as `.dts` bytes.
pub fn hull_dts(
    shape_bounds: [f32; 6],
    details: &[(&str, [f32; 6])],
    extra_nodes: &[&str],
    sequences: &[&str],
) -> Vec<u8> {
    crate::write::write_dts(&hull_shape(shape_bounds, details, extra_nodes, sequences))
}

/// A `.dts` that makes the walks over a shape's objects and primitives long without making a triangle (the named case **shared
/// meshes**): the eight collision details and their eight LOS details, `objects` objects that all point at the same sixteen
/// meshes, and `primitives` primitives of no elements in each mesh. At 4,096 objects and 120,000 primitives it is 15.4 MB, under
/// [`crate::MAX_DTS_BYTES`], and every product the caps and the parse's charges see is within the caps: the triangle count is 0.
pub fn shared_mesh_dts(objects: usize, primitives: usize) -> Vec<u8> {
    let unit = [0.0, 0.0, 0.0, 1.0, 1.0, 1.0];
    let names: Vec<String> = (1..=8)
        .map(|i| format!("Collision-{i}"))
        .chain((9..=16).map(|i| format!("LOS-{i}")))
        .collect();
    let details: Vec<(&str, [f32; 6])> = names.iter().map(|n| (n.as_str(), unit)).collect();
    let mut model = hull_shape([-10.0, -10.0, -10.0, 10.0, 10.0, 10.0], &details, &[], &[]);
    let object = model.shape.objects[0];
    model.shape.objects = vec![object; objects];
    model.shape.sub_shapes[0].num_objects = objects as i32;
    for mesh in model.shape.meshes.iter_mut().flatten() {
        mesh.primitives = vec![
            Primitive {
                start: 0,
                num_elements: 0,
                material: 0,
            };
            primitives
        ];
    }
    crate::write::write_dts(&model)
}

#[cfg(test)]
#[path = "synth_tests.rs"]
mod tests;
