//! The projected decals ([`HdScene::decals`](torque_render_scene::hd::HdScene::decals), the rules are
//! `torque_render_scene::hd::decals`'): each decal an oriented box the GPU projects onto the opaque frame's depth, in place of
//! the CPU path's clipped triangles.
//!
//! - **When**: where [`HdFeatures::projected_decals`] runs at the scene's quality ([`DecalProjection::Auto`]: Medium and up)
//!   and the device has room for the pass's bindings ([`HdWorld::projected_decals_at`]); the client keeps its decals as boxes
//!   there and as triangles elsewhere. A frame without a box in sight makes and records nothing here.
//! - **The pass**: after the opaque stage's passes (where the CPU path's multiplied decals are drawn), into the HDR frame with
//!   no depth attached: each box's inside faces (front faces culled, so a box around the eye still draws), the opaque frame's
//!   depth and the prepass's normals read in group 1. The multiplied decals first, then the alpha-blended ones; one instanced
//!   draw a run of boxes of one texture and sampler (the boxes sorted by blend and texture, oldest first within a run), never
//!   one a decal.
//! - **What takes them**: the prepass writes the normal's `w` 1 for the static geometry and the objects that take decals
//!   (`DynamicObject::takes_decals`, the motion slot's `id.y`), 0.5 for the other objects (Players, vehicles, items): a box
//!   over a Player leaves it as it is. A cutout (left out of the prepass) takes the decal of the surface behind it.
//! - **Limits**: the boxes are culled to the view on the CPU; at most [`decal_capacity`] a frame (the hard cap
//!   [`MAX_PROJECTED_DECALS`], fewer where the device's largest buffer or storage binding holds fewer: the newest kept). Their
//!   screen area is held to [`DECAL_FILL_BUDGET`] screens a frame (each box's bounding sphere projected, at most a screen; the
//!   nearest first, a box past the budget left out and counted): a server's decals, up to 64 units across, around the eye
//!   cannot make every pixel run every box. At most [`MAX_DECAL_RUNS`] runs (textures and blends) draw a frame. A pixel
//!   outside a box is discarded after two loads and the box test, before any texture is read or shaded. Every box passes
//!   [`HdDecal::sanitized`] when the scene takes it.
//!
//! # Maintaining
//!
//! - Module map: this file (the feature switch, the capacity, the boxes' bytes, the pipelines, `prepare_decals`,
//!   `encode_decals`); `shaders/decals.wgsl` (`vs_decal`, `fs_decal_multiply`, `fs_decal_alpha`, composed after the forward
//!   pass's text as `Permutation::Decals`); `forward.wgsl`'s `fs_depth` (the normal's `w`); `torque_render_scene::hd::decals`
//!   (the box, its sanitizing) and `torque_render_scene::decals` (the client's boxes, `DecalManager::boxes`).
//! - Invariants: a frame with no box in sight is the frame without the pass, byte for byte (Low never runs it); the shader's
//!   clip cosine and fade are the scene crate's (`the_decal_shaders_constants_are_the_scene_crates`); a decal on flat ground,
//!   over a corner and on a curved surface matches the CPU path's picture (`tests/projected_decals.rs`), and none lands on an
//!   object that takes no decals.
//! - Extension points: a per-decal normal map (another binding in group 1); clustered lists in place of the per-texture runs
//!   if decals ever take many textures.
//! - Checks: `cargo test -p torque-render-hd --test projected_decals`, `cargo test -p torque-render-hd --lib shaders`,
//!   `cargo test -p torque-render-scene decals`, and `cargo run -p torque-render-hd --release --example decals-profile` (under
//!   the GPU bench lock: it prints the pass's median against its 0.5 ms mark).

use torque_render_scene::decals::MAX_PROJECTED_DECALS;
use torque_render_scene::hd::decals::HdDecal;
use torque_render_scene::hd::material;

use super::*;

/// One box's bytes in the boxes' buffer (`decals.wgsl`'s `DecalBox`).
pub const BOX_BYTES: u64 = 64;
/// The screen area the boxes drawn in a frame may cover together, in screens (see the module docs).
pub const DECAL_FILL_BUDGET: f32 = 8.0;
/// The most runs of one texture and blend a frame draws (a bind group and a draw each); the boxes of the runs past it are
/// left out.
pub const MAX_DECAL_RUNS: usize = 64;

/// Whether the frames project their decals ([`HdFeatures::projected_decals`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DecalProjection {
    /// At [`Quality::Medium`] and up; Low draws the CPU path's triangles.
    #[default]
    Auto,
    /// At every quality.
    On,
    /// At none.
    Off,
}

impl DecalProjection {
    /// Whether the decals are projected at `quality`.
    pub fn runs_at(self, quality: Quality) -> bool {
        match self {
            DecalProjection::Auto => quality != Quality::Low,
            DecalProjection::On => true,
            DecalProjection::Off => false,
        }
    }
}

/// The most boxes a frame draws on a device of `limits`: [`MAX_PROJECTED_DECALS`], or fewer where its largest buffer or
/// storage binding holds fewer.
pub fn decal_capacity(limits: &wgpu::Limits) -> usize {
    let room = limits
        .max_buffer_size
        .min(limits.max_storage_buffer_binding_size)
        / BOX_BYTES;
    usize::try_from(room).map_or(MAX_PROJECTED_DECALS, |r| r.min(MAX_PROJECTED_DECALS))
}

/// The decal group's bindings (group 1): the texture and sampler, the boxes (both stages), the opaque frame's depth and the
/// prepass's normals.
pub(crate) fn decal_entries() -> Vec<wgpu::BindGroupLayoutEntry> {
    let texture = |binding, sample_type| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type,
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    };
    let mut e = texture_entries();
    e.push(crate::storage(2, true, wgpu::ShaderStages::VERTEX_FRAGMENT));
    e.push(texture(3, wgpu::TextureSampleType::Depth));
    e.push(texture(
        4,
        wgpu::TextureSampleType::Float { filterable: true },
    ));
    e
}

/// Whether `device` has room for the decal pass's bindings (else the client keeps the CPU path).
pub(super) fn decals_supported(device: &wgpu::Device) -> bool {
    let groups = [
        frame_entries(),
        decal_entries(),
        crate::sky::group_entries(),
        reflect::layout_entries(),
    ];
    let groups: Vec<&[wgpu::BindGroupLayoutEntry]> = groups.iter().map(Vec::as_slice).collect();
    let needs = crate::limits::PipelineNeeds {
        name: "projected decals",
        vertex: crate::limits::count(&groups, wgpu::ShaderStages::VERTEX),
        fragment: crate::limits::count(&groups, wgpu::ShaderStages::FRAGMENT),
        compute: crate::limits::StageCounts::default(),
    };
    crate::limits::check_needs(&[needs], &device.limits()).is_ok()
}

/// The material word the CPU path's decal draw has (`HdDraw::assign_fixed` of `DecalManager::draws`' draw, a decal among the
/// particles): the alpha-blended pass shades the point with it.
pub fn decal_material() -> u32 {
    (material::KIND_SKY << material::KIND_SHIFT)
        | material::tex_env(torque_render_scene::TexEnv::Modulate)
        | material::PARTICLE
        | material::DECAL
        | material::UNLIT
}

/// A box's bytes, member by member as `DecalBox` lays them out.
pub fn box_bytes(d: &HdDecal) -> [u8; BOX_BYTES as usize] {
    let mut out = [0u8; BOX_BYTES as usize];
    let words = d
        .center
        .iter()
        .chain([&d.opacity])
        .chain(&d.half_x)
        .chain([&0.0])
        .chain(&d.half_y)
        .chain([&0.0])
        .chain(&d.half_z)
        .map(|v| v.to_le_bytes())
        .chain([decal_material().to_le_bytes()]);
    for (bytes, w) in out.as_chunks_mut::<4>().0.iter_mut().zip(words) {
        *bytes = w;
    }
    out
}

/// A run of boxes drawn by one instanced draw.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DecalRun {
    multiply: bool,
    tex: TexRef,
    sampler: SamplerKey,
    first: u32,
    count: u32,
}

/// What the last frame drew of the projected decals ([`HdWorld::decal_counts`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DecalCounts {
    /// The boxes drawn.
    pub boxes: u32,
    /// The instanced draws they took (one a run).
    pub draws: usize,
    /// The boxes in the view left out: past the fill budget, the device's room or the runs' cap.
    pub dropped: u32,
}

/// The screen area (pixels) of a sphere at `center` of `radius` seen by `camera`: its projected disk, at most the screen.
pub fn screen_area(camera: &HdCamera, center: [f32; 3], radius: f32) -> f32 {
    let screen = camera.width as f32 * camera.height as f32;
    let dist = Vec3::from(center).distance(Vec3::from(camera.eye));
    if dist <= radius || !dist.is_finite() {
        return screen;
    }
    // Pixels a unit at the near plane covers: the near plane's top is `frustum[3]` above the axis, half the frame's height.
    let pixels = 0.5 * camera.height as f32 * camera.near / camera.frustum[3].max(1e-6);
    let r = radius * pixels / dist;
    (std::f32::consts::PI * r * r).min(screen)
}

/// The decal pass on the GPU: its pipelines, the boxes' buffer and the frame's runs.
pub(crate) struct GpuDecals {
    layout: wgpu::BindGroupLayout,
    multiply: wgpu::RenderPipeline,
    alpha: wgpu::RenderPipeline,
    boxes: wgpu::Buffer,
    bytes: Vec<u8>,
    runs: Vec<DecalRun>,
    dropped: u32,
}

impl GpuDecals {
    fn new(
        device: &wgpu::Device,
        cache: Option<&wgpu::PipelineCache>,
        groups: [&wgpu::BindGroupLayout; 3],
    ) -> GpuDecals {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("hd decals"),
            entries: &decal_entries(),
        });
        let [frame, sky, refl] = groups;
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("hd decals"),
            bind_group_layouts: &[Some(frame), Some(&layout), Some(sky), Some(refl)],
            immediate_size: 0,
        });
        let module = crate::module(device, Permutation::Decals);
        let pipeline = |entry: &str, blend: Blend| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("hd decals"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vs_decal"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    buffers: &[],
                },
                // The inside faces: one cover a pixel, the eye in the box or not.
                primitive: wgpu::PrimitiveState {
                    front_face: wgpu::FrontFace::Ccw,
                    cull_mode: Some(wgpu::Face::Front),
                    ..wgpu::PrimitiveState::default()
                },
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some(entry),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: HDR_FORMAT,
                        blend: blend_state(blend),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache,
            })
        };
        GpuDecals {
            multiply: pipeline("fs_decal_multiply", Blend::Modulate2x),
            alpha: pipeline("fs_decal_alpha", Blend::Alpha),
            layout,
            boxes: Self::boxes_buffer(device, BOX_BYTES * 64),
            bytes: Vec::new(),
            runs: Vec::new(),
            dropped: 0,
        }
    }

    fn boxes_buffer(device: &wgpu::Device, size: u64) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("hd decal boxes"),
            size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }
}

impl<E: HdExtension> HdWorld<E> {
    /// Whether the frames at `quality` project their decals ([`HdFeatures::projected_decals`], on a device with room for the
    /// pass): the client then hands its decals over as boxes ([`HdScene::decals`](torque_render_scene::hd::HdScene::decals)).
    pub fn projected_decals_at(&self, quality: Quality) -> bool {
        self.features.projected_decals.runs_at(quality) && self.decals_supported
    }

    /// What the last frame drew of the projected decals.
    pub fn decal_counts(&self) -> DecalCounts {
        self.decals
            .as_ref()
            .map_or(DecalCounts::default(), |d| DecalCounts {
                boxes: d.runs.iter().map(|r| r.count).sum(),
                draws: d.runs.len(),
                dropped: d.dropped,
            })
    }

    /// The frame's boxes in the view of `camera`, within the fill budget, sorted into runs and written (module docs); none at a
    /// quality that does not project them.
    pub(super) fn prepare_decals(&mut self, scene: &E::Scene, quality: Quality, camera: &HdCamera) {
        if let Some(d) = &mut self.decals {
            d.runs.clear();
            d.dropped = 0;
        }
        if !self.projected_decals_at(quality) || scene.decals.is_empty() {
            return;
        }
        let planes = camera.planes();
        let mut seen: Vec<(&HdDecal, f32)> = scene
            .decals
            .iter()
            .filter(|d| crate::sphere_seen(&planes, d.center, d.radius()))
            .map(|d| (d, Vec3::from(d.center).distance(Vec3::from(camera.eye))))
            .collect();
        if seen.is_empty() {
            return;
        }
        let in_view = seen.len();
        // Past the device's room the newest are kept.
        let capacity = decal_capacity(&self.device.limits());
        seen.drain(..seen.len().saturating_sub(capacity));
        // The fill budget, the nearest first (the sort is stable: the oldest first at a distance).
        seen.sort_by(|a, b| a.1.total_cmp(&b.1));
        let budget = DECAL_FILL_BUDGET * camera.width as f32 * camera.height as f32;
        let mut filled = 0.0;
        let mut order: Vec<&HdDecal> = Vec::with_capacity(seen.len());
        for (d, _) in seen {
            let area = screen_area(camera, d.center, d.radius());
            if filled + area <= budget {
                filled += area;
                order.push(d);
            }
        }
        // The multiplied decals first, then by texture and sampler; oldest first within a run (the sort is stable).
        order.sort_by_key(|d| {
            let s = &d.texture.1;
            let filters = [s.mag, s.min, s.mip].map(|f| f == Filter::Linear);
            (
                d.blend != Blend::Modulate2x,
                d.texture.0,
                s.repeat,
                filters,
                s.mipmaps,
            )
        });
        let mut runs: Vec<DecalRun> = Vec::new();
        let mut bytes = self
            .decals
            .as_mut()
            .map(|d| std::mem::take(&mut d.bytes))
            .unwrap_or_default();
        bytes.clear();
        for d in order {
            let run = DecalRun {
                multiply: d.blend == Blend::Modulate2x,
                tex: TexRef::Table(d.texture.0),
                sampler: SamplerKey::of(&d.texture.1),
                first: (bytes.len() as u64 / BOX_BYTES) as u32,
                count: 1,
            };
            let full = runs.len() == MAX_DECAL_RUNS;
            match runs.last_mut() {
                Some(r)
                    if (r.multiply, r.tex, r.sampler) == (run.multiply, run.tex, run.sampler) =>
                {
                    r.count += 1;
                }
                _ if full => continue,
                _ => runs.push(run),
            }
            bytes.extend_from_slice(&box_bytes(d));
        }
        let drawn: u32 = runs.iter().map(|r| r.count).sum();
        let dropped = in_view as u32 - drawn;
        for r in &runs {
            self.sampler(r.sampler);
        }
        let device = &self.device;
        let decals = self.decals.get_or_insert_with(|| {
            GpuDecals::new(
                device,
                self.pipeline_cache.as_ref(),
                [&self.frame_layout, &self.sky.layout, &self.refl.layout],
            )
        });
        let need = bytes.len() as u64;
        if decals.boxes.size() < need {
            // Grown with room, within the capacity (the device's largest binding).
            let room = need
                .next_power_of_two()
                .min(capacity as u64 * BOX_BYTES)
                .max(need);
            decals.boxes = GpuDecals::boxes_buffer(device, room);
        }
        self.queue.write_buffer(&decals.boxes, 0, &bytes);
        decals.bytes = bytes;
        decals.runs = runs;
        decals.dropped = dropped;
    }

    /// Records the frame's prepared decals into `hdr` (module docs), the forward groups `groups` (frame, sky, reflections)
    /// bound, reading `depth` and `normals`: the draws made.
    pub(super) fn encode_decals(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        groups: [&wgpu::BindGroup; 3],
        hdr: &wgpu::TextureView,
        depth: &wgpu::TextureView,
        normals: &wgpu::TextureView,
    ) -> usize {
        let Some(decals) = self.decals.as_ref().filter(|d| !d.runs.is_empty()) else {
            return 0;
        };
        crate::count_pass();
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("hd projected decals"),
            color_attachments: &[Some(color_load(hdr))],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        set_groups(&mut pass, groups);
        let mut bound: Option<bool> = None;
        for r in &decals.runs {
            let view = match r.tex {
                TexRef::White => &self.white,
                TexRef::Empty => &self.empty,
                TexRef::Table(i) => self.textures.get(i).unwrap_or(&self.white),
            };
            let Some(sampler) = self.samplers.get(&r.sampler) else {
                continue;
            };
            let group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("hd decals"),
                layout: &decals.layout,
                entries: &[
                    tv(0, view),
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: decals.boxes.as_entire_binding(),
                    },
                    tv(3, depth),
                    tv(4, normals),
                ],
            });
            if bound != Some(r.multiply) {
                pass.set_pipeline(if r.multiply {
                    &decals.multiply
                } else {
                    &decals.alpha
                });
                bound = Some(r.multiply);
            }
            pass.set_bind_group(1, &group, &[]);
            pass.draw(0..36, r.first..r.first + r.count);
        }
        crate::count_draws(decals.runs.len());
        decals.runs.len()
    }
}

/// A color attachment of `view`, loaded and kept.
fn color_load(view: &wgpu::TextureView) -> wgpu::RenderPassColorAttachment<'_> {
    wgpu::RenderPassColorAttachment {
        view,
        depth_slice: None,
        resolve_target: None,
        ops: wgpu::Operations {
            load: wgpu::LoadOp::Load,
            store: wgpu::StoreOp::Store,
        },
    }
}

#[cfg(test)]
#[path = "decals_tests.rs"]
mod tests;
