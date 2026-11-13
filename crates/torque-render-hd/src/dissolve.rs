//! Dissolving objects ([`HdScene::dissolves`](torque_render_scene::hd::HdScene::dissolves)): their opaque draws clipped by the
//! object's noise field in the depth prepass, the opaque pass and the shadow casters, with the edge's light added in HDR.
//!
//! - **Which draws**: an object of the scene's dissolves, not late, its draws that are opaque, depth tested and written and not a
//!   cutout's or a shadow-only body's (their motion slots marked in `HdWorld::clipped` each frame by `prepare_dissolves`). An object wholly
//!   gone is left out of every pass (`GpuObject::gone`: its translucent parts too); a secondary view (the mirror, a capture's
//!   face) draws it whole under half eaten and leaves it out over half (`GpuObject::eaten`). Everything else is drawn as
//!   before, by the same pipelines and modules: a frame with no dissolve makes nothing here.
//! - **The slots**: one 128-byte slot a motion slot ([`SLOT_BYTES`]; `dissolve_common.wgsl`'s `DissolveSlot`: the object's
//!   world-to-object matrix, the edge's light, the threshold, the band's end, the cell and sweep, its height span), written
//!   each frame for the clipped draws. A clipped draw is instance `motion` of one, so the vertex stages hand the slot on.
//! - **Groups**: the forward and prepass pipelines' group 1 is the texture group with the slots (binding 2, fragment only); the
//!   casters' group 3 is their extension group with the slots (binding 3, [`crate::shadows`]).
//!
//! # Maintaining
//!
//! - Module map: this file (the slots, the forward and prepass pipelines and groups, `prepare_dissolves`, `ensure_dissolve`,
//!   `draw_dissolve`, `draw_dissolve_depth`); `shadows.rs`'s `ShadowClip` (the casters'); `shaders/dissolve_common.wgsl` (the
//!   slot and the field), `dissolve.wgsl` (the forward module's entries), `dissolve_shadow.wgsl` (the casters');
//!   `torque_render_scene::hd::dissolve` (the field on the CPU, `HdDissolve`) and `torque_render_scene::dissolve` (the client's
//!   runner: the thresholds, the samples, the front's particles).
//! - Invariants: the modules every other draw uses are byte for byte as without dissolves (`Permutation::Dissolve` composes the
//!   forward text with two entries renamed, `only_the_dissolve_permutations_take_its_chunks_and_their_entries_are_plain_functions_there`);
//!   the prepass, the opaque pass and the casters discard by the same field of the same rest place (`tests/dissolve.rs`: the
//!   drawn share of the object's pixels tracks the share left, the prepass's coverage is the colour's, the shadow shrinks with
//!   it); the CPU field is the shader's (`the_cpu_field_is_the_shaders`).
//! - Extension points: a translucent object's own clip (the weighted blend's stages would take the slots as the opaque pass
//!   does); a per-draw edge texture (another binding in group 1).
//! - Checks: `cargo test -p torque-render-hd --test main dissolve::`, `cargo test -p torque-render-hd --lib shaders`,
//!   `cargo test -p torque-render-scene dissolve`.

use torque_render_scene::hd::dissolve::{HdDissolve, WHOLE};

use super::*;

/// One dissolve slot's bytes (`DissolveSlot`).
pub(crate) const SLOT_BYTES: u64 = 128;

/// The dissolving draws' group 1: the texture and sampler, and the slots (the fragment stage's).
pub(crate) fn dissolve_entries() -> Vec<wgpu::BindGroupLayoutEntry> {
    let mut e = texture_entries();
    e.push(crate::storage(2, true, wgpu::ShaderStages::FRAGMENT));
    e
}

/// Whether `device` has room for the dissolving draws' bindings (else they are drawn whole).
pub(super) fn dissolve_supported(device: &wgpu::Device) -> bool {
    let groups = [
        frame_entries(),
        dissolve_entries(),
        crate::sky::group_entries(),
        reflect::layout_entries(),
    ];
    let groups: Vec<&[wgpu::BindGroupLayoutEntry]> = groups.iter().map(Vec::as_slice).collect();
    let needs = crate::limits::PipelineNeeds {
        name: "dissolving draws",
        vertex: crate::limits::count(&groups, wgpu::ShaderStages::VERTEX),
        fragment: crate::limits::count(&groups, wgpu::ShaderStages::FRAGMENT),
        compute: crate::limits::StageCounts::default(),
    };
    crate::limits::check_needs(&[needs], &device.limits()).is_ok()
}

/// A slot's bytes for `d` over an object placed by `transform`, member by member as `DissolveSlot` lays them out: the
/// world-to-object matrix, `edge`, `band`, `span` and the padding (a value that is not finite is 0).
pub(crate) fn slot_bytes(
    d: &HdDissolve,
    transform: &torque_render_scene::math::Mat,
) -> [u8; SLOT_BYTES as usize] {
    let to_object = HdDissolve::to_object(transform);
    let [r, g, b] = d.edge_color;
    let edge = [r, g, b, 0.0];
    let band = [
        d.threshold,
        d.edge_to,
        1.0 / d.cell.max(torque_render_scene::hd::dissolve::MIN_CELL),
        d.sweep,
    ];
    let span = [d.z_range[0], d.z_range[1], 0.0, 0.0];
    let pad = [0.0f32; 4];
    let mut out = [0u8; SLOT_BYTES as usize];
    let words = to_object
        .iter()
        .chain(&edge)
        .chain(&band)
        .chain(&span)
        .chain(&pad);
    for (bytes, w) in out.as_chunks_mut::<4>().0.iter_mut().zip(words) {
        *bytes = if w.is_finite() { *w } else { 0.0 }.to_le_bytes();
    }
    out
}

/// The dissolving draws on the GPU: their slots, pipelines and groups.
pub(crate) struct GpuDissolve {
    layout: wgpu::BindGroupLayout,
    forward_layout: wgpu::PipelineLayout,
    prepass_layout: wgpu::PipelineLayout,
    modules: HashMap<bool, wgpu::ShaderModule>,
    pipelines: HashMap<PipelineKey, wgpu::RenderPipeline>,
    prepass: HashMap<Cull, wgpu::RenderPipeline>,
    pub(crate) slots: wgpu::Buffer,
    groups: HashMap<(TexRef, SamplerKey), wgpu::BindGroup>,
    bytes: Vec<u8>,
}

impl GpuDissolve {
    fn new(
        device: &wgpu::Device,
        frame: &wgpu::BindGroupLayout,
        sky: &wgpu::BindGroupLayout,
        refl: &wgpu::BindGroupLayout,
        motion: &wgpu::BindGroupLayout,
    ) -> GpuDissolve {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("hd dissolve"),
            entries: &dissolve_entries(),
        });
        let forward_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("hd dissolve"),
            bind_group_layouts: &[Some(frame), Some(&layout), Some(sky), Some(refl)],
            immediate_size: 0,
        });
        let prepass_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("hd dissolve prepass"),
            bind_group_layouts: &[Some(frame), Some(&layout), Some(motion)],
            immediate_size: 0,
        });
        GpuDissolve {
            layout,
            forward_layout,
            prepass_layout,
            modules: HashMap::new(),
            pipelines: HashMap::new(),
            prepass: HashMap::new(),
            slots: Self::slots_buffer(device, SLOT_BYTES * 16),
            groups: HashMap::new(),
            bytes: Vec::new(),
        }
    }

    fn slots_buffer(device: &wgpu::Device, size: u64) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("hd dissolve slots"),
            size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }
}

impl<E: HdExtension> HdWorld<E> {
    /// The frame's dissolving draws marked and their slots written (module docs): every loose object's draws unmarked first,
    /// then the scene's dissolves' objects' clipped. None (or a device without room for them): nothing is clipped.
    pub(super) fn prepare_dissolves(&mut self, scene: &E::Scene) {
        let mut any = false;
        for o in &mut self.loose.objects {
            o.gone = false;
            o.eaten = 0.0;
        }
        self.clipped.clear();
        self.dissolving = false;
        if scene.dissolves.is_empty() {
            return;
        }
        if !self.dissolve_supported {
            // Drawn whole; one wholly gone is still left out.
            for o in &mut self.loose.objects {
                if let Some(d) = scene.dissolves.iter().find(|d| d.ghost == o.ghost) {
                    o.gone = d.gone();
                    o.eaten = d.eaten;
                }
            }
            return;
        }
        let dissolve = self.dissolve.get_or_insert_with(|| {
            GpuDissolve::new(
                &self.device,
                &self.frame_layout,
                &self.sky.layout,
                &self.refl.layout,
                &self.post.motion_layout,
            )
        });
        dissolve.bytes.clear();
        for (i, o) in self.loose.objects.iter_mut().enumerate() {
            let Some(d) = scene.dissolves.iter().find(|d| d.ghost == o.ghost) else {
                continue;
            };
            o.eaten = d.eaten;
            if o.late {
                continue;
            }
            if d.gone() {
                o.gone = true;
                continue;
            }
            if d.threshold <= WHOLE {
                continue;
            }
            // The scene's object of the same place is the one uploaded (else the first of the ghost).
            let placed = scene
                .objects
                .get(i)
                .filter(|s| s.ghost == o.ghost)
                .or_else(|| scene.objects.iter().find(|s| s.ghost == o.ghost));
            let Some(placed) = placed else {
                continue;
            };
            let bytes = slot_bytes(d, &placed.transform);
            for r in &o.draws {
                let clipped = r.blend == Blend::Opaque
                    && r.depth_test
                    && r.depth_write
                    && !r.particle
                    && !r.cutout
                    && !r.shadow_only
                    && r.sprites.is_none();
                if !clipped {
                    continue;
                }
                let slot = r.motion as usize;
                if self.clipped.len() <= slot {
                    self.clipped.resize(slot + 1, false);
                }
                self.clipped[slot] = true;
                any = true;
                let at = r.motion as usize * SLOT_BYTES as usize;
                if dissolve.bytes.len() < at + SLOT_BYTES as usize {
                    dissolve.bytes.resize(at + SLOT_BYTES as usize, 0);
                }
                dissolve.bytes[at..at + SLOT_BYTES as usize].copy_from_slice(&bytes);
            }
        }
        if !any {
            self.clipped.clear();
            return;
        }
        let need = dissolve.bytes.len() as u64;
        if dissolve.slots.size() < need {
            dissolve.slots = GpuDissolve::slots_buffer(&self.device, need.next_power_of_two());
            dissolve.groups.clear();
        }
        self.queue.write_buffer(&dissolve.slots, 0, &dissolve.bytes);
        self.dissolving = true;
    }

    /// Whether `d` is clipped this frame.
    pub(super) fn clipped(&self, d: &DrawRef) -> bool {
        self.dissolving
            && !d.particle
            && d.motion > 0
            && self
                .clipped
                .get(d.motion as usize)
                .copied()
                .unwrap_or(false)
    }

    /// What `o`'s shadow depends on: its content, and its clipped draws' slots this frame (a kept overlay is drawn again as
    /// the front moves).
    pub(super) fn clip_content(&self, o: &GpuObject) -> u64 {
        use std::hash::{Hash, Hasher};
        let Some(dissolve) = self.dissolve.as_ref().filter(|_| self.dissolving) else {
            return o.content;
        };
        let mut h = std::hash::DefaultHasher::new();
        o.content.hash(&mut h);
        let mut any = false;
        for d in o.draws.iter().filter(|d| self.clipped(d)) {
            let at = d.motion as usize * SLOT_BYTES as usize;
            dissolve
                .bytes
                .get(at..at + SLOT_BYTES as usize)
                .hash(&mut h);
            any = true;
        }
        if any { h.finish() } else { o.content }
    }

    /// The pipelines (`key`'s, its prepass's) and the group of `(tex, s)` a clipped draw is drawn with.
    pub(super) fn ensure_dissolve(&mut self, key: PipelineKey, tex: TexRef, s: SamplerKey) {
        self.sampler(s);
        let view = match tex {
            TexRef::White => &self.white,
            TexRef::Empty => &self.empty,
            TexRef::Table(i) => self.textures.get(i).unwrap_or(&self.white),
        };
        let (device, cache) = (&self.device, self.pipeline_cache.as_ref());
        let Some(dissolve) = &mut self.dissolve else {
            return;
        };
        let GpuDissolve {
            modules,
            pipelines,
            prepass,
            forward_layout,
            prepass_layout,
            ..
        } = dissolve;
        // The modules the pipelines are made of: the key's clustering's, and the clustered one the prepass takes.
        for clustered in [key.clustered, true] {
            modules
                .entry(clustered)
                .or_insert_with(|| crate::module(device, Permutation::Dissolve { clustered }));
        }
        let made = &mut self.pipelines_made;
        if let std::collections::hash_map::Entry::Vacant(e) = pipelines.entry(key) {
            e.insert(forward_pipeline_of(
                device,
                cache,
                forward_layout,
                &modules[&key.clustered],
                key,
                "fs_dissolve",
                Vertices::Dissolve,
            ));
            *made += 1;
        }
        if let std::collections::hash_map::Entry::Vacant(e) = prepass.entry(key.cull) {
            let entries = ("vs_depth_dissolve", "fs_depth_dissolve");
            e.insert(prepass_pipeline(
                device,
                cache,
                prepass_layout,
                &modules[&true],
                key.cull,
                entries,
            ));
            *made += 1;
        }
        if !dissolve.groups.contains_key(&(tex, s)) {
            let group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("hd dissolve"),
                layout: &dissolve.layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.samplers[&s]),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: dissolve.slots.as_entire_binding(),
                    },
                ],
            });
            dissolve.groups.insert((tex, s), group);
        }
    }

    /// Draws the clipped draw `d` into the opaque pass with `key`'s pipeline and the group of `group`: the draws made.
    pub(super) fn draw_dissolve(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        key: PipelineKey,
        group: (TexRef, SamplerKey),
        d: &DrawRef,
    ) -> usize {
        let (Some(dissolve), Some(v), Some(i)) =
            (&self.dissolve, &self.loose.vertices, &self.loose.indices)
        else {
            return 0;
        };
        let (Some(pipeline), Some(g)) = (dissolve.pipelines.get(&key), dissolve.groups.get(&group))
        else {
            return 0;
        };
        if d.count == 0 {
            return 0;
        }
        pass.set_pipeline(pipeline);
        pass.set_bind_group(1, g, &[]);
        pass.set_vertex_buffer(0, v.slice(..));
        pass.set_index_buffer(i.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(
            d.first..d.first + d.count,
            d.base_vertex,
            d.motion..d.motion + 1,
        );
        crate::count_draws(1);
        1
    }

    /// Draws the clipped draw `d` into the prepass (culled by `cull`, the group of `group`), the whole table of motion slots
    /// bound (its instance is its slot whichever way the other draws bind theirs), the shapes' previous positions from
    /// `previous`.
    pub(super) fn draw_dissolve_depth(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        cull: Cull,
        group: (TexRef, SamplerKey),
        d: &DrawRef,
        previous: Option<&wgpu::Buffer>,
    ) {
        let (Some(dissolve), Some(v), Some(i)) =
            (&self.dissolve, &self.loose.vertices, &self.loose.indices)
        else {
            return;
        };
        let (Some(pipeline), Some(g)) = (dissolve.prepass.get(&cull), dissolve.groups.get(&group))
        else {
            return;
        };
        if d.count == 0 {
            return;
        }
        pass.set_pipeline(pipeline);
        pass.set_bind_group(1, g, &[]);
        pass.set_bind_group(2, &self.post.motion_group, &[0]);
        pass.set_vertex_buffer(0, v.slice(..));
        pass.set_vertex_buffer(1, previous.unwrap_or(v).slice(..));
        pass.set_index_buffer(i.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(
            d.first..d.first + d.count,
            d.base_vertex,
            d.motion..d.motion + 1,
        );
        crate::count_draws(1);
    }
}
