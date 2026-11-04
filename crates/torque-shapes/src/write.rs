//! The writers: [`write_dts`] gives a [`Model`] back as the bytes of a version-24 `.dts`, [`write_dsq`] a [`Dsq`] as a `.dsq`.
//!
//! They exist for the tests and the fuzz, not for running a shape: the round trip (parse, write, compare with the original bytes)
//! is the structural proof that the reader skips no field, and the synthetic shapes of `crate::synth` are made through the
//! writer so that the fuzz mutates files no exporter wrote. The layout is `TSShape::write` / `assembleShape` read the other
//! way: three buffers (words, shorts, bytes) each with its own cursor, a guard (a word, a short and a byte from one counter) after
//! each group, then the sequences and the material list.
//!
//! The writer derives every count from the length of the array it belongs to, so a model whose arrays disagree with the counts it
//! carries (a [`Mesh::num_verts`] longer than its positions) is written self-consistent: arrays are cut or zero-padded to the
//! count. A model that came from [`crate::dts::parse`] needs neither.

use crate::dsq::Dsq;
use crate::matlist;
use crate::shape::{
    Mesh, MeshExtra, MeshKind, MeshRender, Model, Sequence, SkinData, SortedData, SubShape,
};

/// The three buffers being written and the guard counter.
struct Out {
    w32: Vec<u8>,
    w16: Vec<u8>,
    w8: Vec<u8>,
    guard: u32,
}

impl Out {
    fn u32(&mut self, v: u32) {
        self.w32.extend_from_slice(&v.to_le_bytes());
    }

    fn i32(&mut self, v: i32) {
        self.u32(v as u32);
    }

    fn f32(&mut self, v: f32) {
        self.u32(v.to_bits());
    }

    fn count(&mut self, n: usize) {
        self.u32(n as u32);
    }

    fn short(&mut self, v: u16) {
        self.w16.extend_from_slice(&v.to_le_bytes());
    }

    fn guard(&mut self) {
        let g = self.guard;
        self.u32(g);
        self.short(g as u16);
        self.w8.push(g as u8);
        self.guard += 1;
    }

    fn quats(&mut self, quats: &[[i16; 4]]) {
        for q in quats {
            for &c in q {
                self.short(c as u16);
            }
        }
    }

    fn f32x3(&mut self, v: &[[f32; 3]]) {
        for p in v {
            for &c in p {
                self.f32(c);
            }
        }
    }

    /// `n` triples: the first `n` of `v`, then zeros if there are fewer.
    fn f32x3_n(&mut self, v: &[[f32; 3]], n: usize) {
        for i in 0..n {
            for c in v.get(i).copied().unwrap_or([0.0; 3]) {
                self.f32(c);
            }
        }
    }
}

/// `n` items of `v` (cut or padded with `fill`).
fn exactly<T: Copy>(v: &[T], n: usize, fill: T) -> impl Iterator<Item = T> + '_ {
    (0..n).map(move |i| v.get(i).copied().unwrap_or(fill))
}

fn write_set(out: &mut Vec<u8>, set: &crate::shape::IntSet) {
    out.extend_from_slice(&set.first.to_le_bytes());
    out.extend_from_slice(&(set.words.len() as u32).to_le_bytes());
    for w in &set.words {
        out.extend_from_slice(&w.to_le_bytes());
    }
}

/// A sequence as `TSShape::Sequence::write` makes it: 14 words and the eight sets, with the name index first when `with_name`.
pub(crate) fn write_sequence(out: &mut Vec<u8>, s: &Sequence, with_name: bool) {
    let mut word = |v: u32| out.extend_from_slice(&v.to_le_bytes());
    if with_name {
        word(s.name as u32);
    }
    word(s.flags);
    word(s.num_keyframes as u32);
    word(s.duration.to_bits());
    word(s.priority as u32);
    word(s.first_ground_frame as u32);
    word(s.num_ground_frames as u32);
    word(s.base_rotation as u32);
    word(s.base_translation as u32);
    word(s.base_scale as u32);
    word(s.base_object_state as u32);
    word(s.base_decal_state as u32);
    word(s.first_trigger as u32);
    word(s.num_triggers as u32);
    word(s.tool_begin.to_bits());
    for set in [
        &s.rotation_matters,
        &s.translation_matters,
        &s.scale_matters,
        &s.decal_matters,
        &s.ifl_matters,
        &s.vis_matters,
        &s.frame_matters,
        &s.mat_frame_matters,
    ] {
        write_set(out, set);
    }
}

fn write_skin(o: &mut Out, owns: bool, s: &SkinData) {
    o.count(s.num_initial_verts);
    if owns {
        o.f32x3_n(&s.initial_verts, s.num_initial_verts);
        o.f32x3_n(&s.initial_normals, s.num_initial_verts);
        let enc: Vec<u8> = exactly(&s.initial_encoded_normals, s.num_initial_verts, 0).collect();
        o.w8.extend_from_slice(&enc);
    }
    o.count(s.num_transforms);
    if owns {
        for m in exactly(&s.initial_transforms, s.num_transforms, [0.0; 16]) {
            for c in m {
                o.f32(c);
            }
        }
    }
    o.count(s.num_weights);
    if owns {
        for v in exactly(&s.vertex_index, s.num_weights, 0) {
            o.i32(v);
        }
        for v in exactly(&s.bone_index, s.num_weights, 0) {
            o.i32(v);
        }
        for v in exactly(&s.weight, s.num_weights, 0.0) {
            o.f32(v);
        }
    }
    o.count(s.num_nodes);
    if owns {
        for v in exactly(&s.node_index, s.num_nodes, 0) {
            o.i32(v);
        }
    }
    o.guard();
}

fn write_sorted(o: &mut Out, s: &SortedData) {
    o.count(s.clusters.len());
    for c in &s.clusters {
        for &w in c {
            o.u32(w);
        }
    }
    for list in [
        &s.start_cluster,
        &s.first_vert,
        &s.num_verts,
        &s.first_tvert,
    ] {
        o.count(list.len());
        for &w in list {
            o.u32(w);
        }
    }
    o.u32(s.always_write_depth);
    o.guard();
}

fn write_mesh(o: &mut Out, m: &Mesh, r: &MeshRender) {
    o.u32(m.kind.word());
    o.guard();
    o.i32(m.num_frames);
    o.i32(m.num_mat_frames);
    o.i32(m.parent);
    for &c in &m.bounds {
        o.f32(c);
    }
    for &c in &m.center {
        o.f32(c);
    }
    o.i32(m.radius);
    let owns = m.parent < 0;
    o.count(m.num_verts);
    if owns {
        o.f32x3_n(&m.positions, m.num_verts);
    }
    o.count(m.num_tverts);
    if owns {
        for t in exactly(&r.tverts, m.num_tverts, [0.0; 2]) {
            o.f32(t[0]);
            o.f32(t[1]);
        }
        o.f32x3_n(&r.normals, m.num_verts);
        let enc: Vec<u8> = exactly(&r.encoded_normals, m.num_verts, 0).collect();
        o.w8.extend_from_slice(&enc);
    }
    o.count(m.primitives.len());
    for p in &m.primitives {
        o.short(p.start);
        o.short(p.num_elements);
    }
    for p in &m.primitives {
        o.u32(p.material);
    }
    o.count(m.indices.len());
    for &i in &m.indices {
        o.short(i);
    }
    o.count(r.merge_indices.len());
    for &i in &r.merge_indices {
        o.short(i);
    }
    o.i32(m.verts_per_frame);
    o.u32(m.flags);
    o.guard();
    match (&m.kind, &r.extra) {
        (MeshKind::Skin, MeshExtra::Skin(s)) => write_skin(o, owns, s),
        (MeshKind::Sorted, MeshExtra::Sorted(s)) => write_sorted(o, s),
        // A kind without its arrays (a model built by hand): write them empty.
        (MeshKind::Skin, _) => {
            for _ in 0..4 {
                o.count(0);
            }
            o.guard();
        }
        (MeshKind::Sorted, _) => write_sorted(o, &SortedData::default()),
        _ => {}
    }
}

/// The bytes of a version-24 `.dts` for `model`.
pub fn write_dts(model: &Model) -> Vec<u8> {
    let s = &model.shape;
    let mut o = Out {
        w32: Vec::new(),
        w16: Vec::new(),
        w8: Vec::new(),
        guard: 0,
    };
    for n in [
        s.nodes.len(),
        s.objects.len(),
        s.decals.len(),
        s.sub_shapes.len(),
        s.ifl_materials.len(),
        s.node_rotations.len(),
        s.node_translations.len(),
        s.node_uniform_scales.len(),
        s.node_aligned_scales.len(),
        s.node_arbitrary_scale_factors.len(),
        s.ground_translations.len(),
        s.object_states.len(),
        s.decal_states.len(),
        s.triggers.len(),
        s.details.len(),
        s.meshes.len(),
        s.names.len(),
    ] {
        o.count(n);
    }
    o.i32(s.smallest_visible_size);
    o.i32(s.smallest_visible_dl);
    o.guard();

    o.f32(s.radius);
    o.f32(s.tube_radius);
    for &c in &s.center {
        o.f32(c);
    }
    for &c in &s.bounds {
        o.f32(c);
    }
    o.guard();

    for n in &s.nodes {
        for v in [
            n.name,
            n.parent,
            n.first_object,
            n.first_child,
            n.next_sibling,
        ] {
            o.i32(v);
        }
    }
    o.guard();
    for n in &s.objects {
        for v in [
            n.name,
            n.num_meshes,
            n.start_mesh_index,
            n.node,
            n.next_sibling,
            n.first_decal,
        ] {
            o.i32(v);
        }
    }
    o.guard();
    for n in &s.decals {
        for v in [
            n.name,
            n.num_meshes,
            n.start_mesh_index,
            n.object,
            n.next_sibling,
        ] {
            o.i32(v);
        }
    }
    o.guard();
    for n in &s.ifl_materials {
        for v in [
            n.name,
            n.material_slot,
            n.first_frame,
            n.first_frame_off_time,
            n.num_frames,
        ] {
            o.i32(v);
        }
    }
    o.guard();
    let firsts: [fn(&SubShape) -> i32; 3] =
        [|t| t.first_node, |t| t.first_object, |t| t.first_decal];
    for pick in firsts {
        for t in &s.sub_shapes {
            o.i32(pick(t));
        }
    }
    o.guard();
    let nums: [fn(&SubShape) -> i32; 3] = [|t| t.num_nodes, |t| t.num_objects, |t| t.num_decals];
    for pick in nums {
        for t in &s.sub_shapes {
            o.i32(pick(t));
        }
    }
    o.guard();
    o.quats(&s.default_rotations);
    o.f32x3(&s.default_translations);
    o.f32x3(&s.node_translations);
    o.quats(&s.node_rotations);
    o.guard();
    for &v in &s.node_uniform_scales {
        o.f32(v);
    }
    o.f32x3(&s.node_aligned_scales);
    o.f32x3(&s.node_arbitrary_scale_factors);
    o.quats(&s.node_arbitrary_scale_rots);
    o.guard();
    o.f32x3(&s.ground_translations);
    o.quats(&s.ground_rotations);
    o.guard();
    for st in &s.object_states {
        o.f32(st.vis);
        o.i32(st.frame);
        o.i32(st.mat_frame);
    }
    o.guard();
    for &v in &s.decal_states {
        o.i32(v);
    }
    o.guard();
    for t in &s.triggers {
        o.u32(t.state);
        o.f32(t.pos);
    }
    o.guard();
    for d in &s.details {
        o.i32(d.name);
        o.i32(d.sub_shape);
        o.i32(d.object_detail);
        o.f32(d.size);
        o.f32(d.average_error);
        o.f32(d.max_error);
        o.i32(d.poly_count);
    }
    o.guard();
    let empty = MeshRender::default();
    for (i, slot) in s.meshes.iter().enumerate() {
        match slot {
            None => o.u32(MeshKind::Null.word()),
            Some(m) => write_mesh(
                &mut o,
                m,
                model
                    .render
                    .get(i)
                    .and_then(Option::as_ref)
                    .unwrap_or(&empty),
            ),
        }
    }
    o.guard();
    for name in &s.names {
        o.w8.extend_from_slice(name);
        o.w8.push(0);
    }
    o.guard();

    // What the file held past what was read (the padding of the sections), then the padding this writer needs.
    for (buffer, left) in [&mut o.w32, &mut o.w16, &mut o.w8]
        .into_iter()
        .zip(&model.file.left)
    {
        buffer.extend_from_slice(left);
        while !buffer.len().is_multiple_of(4) {
            buffer.push(0);
        }
    }
    let start16 = (o.w32.len() / 4) as u32;
    let start8 = start16 + (o.w16.len() / 4) as u32;
    let size = start8 + (o.w8.len() / 4) as u32;
    let mut out = Vec::new();
    let word = u32::from(s.version)
        | (u32::from(model.file.version_middle) << 8)
        | (u32::from(s.exporter) << 16);
    out.extend_from_slice(&word.to_le_bytes());
    out.extend_from_slice(&size.to_le_bytes());
    out.extend_from_slice(&start16.to_le_bytes());
    out.extend_from_slice(&start8.to_le_bytes());
    out.extend_from_slice(&o.w32);
    out.extend_from_slice(&o.w16);
    out.extend_from_slice(&o.w8);
    out.extend_from_slice(&(s.sequences.len() as u32).to_le_bytes());
    for q in &s.sequences {
        write_sequence(&mut out, q, true);
    }
    matlist::write(&model.file.materials, &mut out);
    out.extend_from_slice(&model.file.trailing);
    out
}

/// The bytes of a `.dsq` for `dsq` (`importSequences`' layout, version 24).
pub fn write_dsq(dsq: &Dsq) -> Vec<u8> {
    let mut out = Vec::new();
    let word = |out: &mut Vec<u8>, v: u32| out.extend_from_slice(&v.to_le_bytes());
    let name = |out: &mut Vec<u8>, n: &[u8]| {
        out.extend_from_slice(&(n.len() as u32).to_le_bytes());
        out.extend_from_slice(n);
    };
    word(&mut out, dsq.version);
    word(&mut out, dsq.node_names.len() as u32);
    for n in &dsq.node_names {
        name(&mut out, n);
    }
    word(&mut out, dsq.spare[0] as u32);
    word(&mut out, dsq.spare[1] as u32);
    let quats = |out: &mut Vec<u8>, q: &[[i16; 4]]| {
        for q in q {
            for &c in q {
                out.extend_from_slice(&c.to_le_bytes());
            }
        }
    };
    let f3 = |out: &mut Vec<u8>, v: &[[f32; 3]]| {
        for p in v {
            for &c in p {
                out.extend_from_slice(&c.to_le_bytes());
            }
        }
    };
    word(&mut out, dsq.node_rotations.len() as u32);
    quats(&mut out, &dsq.node_rotations);
    word(&mut out, dsq.node_translations.len() as u32);
    f3(&mut out, &dsq.node_translations);
    word(&mut out, dsq.uniform_scales.len() as u32);
    for &v in &dsq.uniform_scales {
        word(&mut out, v.to_bits());
    }
    word(&mut out, dsq.aligned_scales.len() as u32);
    f3(&mut out, &dsq.aligned_scales);
    word(&mut out, dsq.arbitrary_scale_factors.len() as u32);
    quats(&mut out, &dsq.arbitrary_scale_rots);
    f3(&mut out, &dsq.arbitrary_scale_factors);
    word(&mut out, dsq.ground_translations.len() as u32);
    f3(&mut out, &dsq.ground_translations);
    quats(&mut out, &dsq.ground_rotations);
    word(&mut out, dsq.object_states);
    word(&mut out, dsq.sequences.len() as u32);
    for q in &dsq.sequences {
        name(&mut out, &q.name);
        write_sequence(&mut out, &q.sequence, false);
    }
    word(&mut out, dsq.triggers.len() as u32);
    for t in &dsq.triggers {
        word(&mut out, t.state);
        word(&mut out, t.pos.to_bits());
    }
    out.extend_from_slice(&dsq.trailing);
    out
}
