//! The irradiance probes (`gi_grid.wgsl`, `gi_probe.wgsl`, `gi_lookup.wgsl`): each cascade's 24 x 24 x 12 probes about the eye,
//! a cascade's spacing twice the last's, kept toroidally; their state (offsets and weights, with the cascades' places in front:
//! what the forward pass reads), their words (coordinates, age, irradiance and depth moments, blended frame to frame) and the
//! atlas of their octahedral tiles the forward pass and the light pass sample.

use torque_render_core::wgpu;

use super::voxels::{ROW, VoxelGpu};
use crate::shaders::Permutation;

/// Probes along x, y and z of a cascade, and a cascade's probes (`gi_grid.wgsl`'s).
pub(crate) const PROBES: [i32; 3] = [24, 24, 12];
pub(crate) const CASCADE_PROBES: u32 = (PROBES[0] * PROBES[1] * PROBES[2]) as u32;
/// The most cascades.
pub(crate) const MAX_CASCADES: usize = 4;
/// The finest cascade's spacing (units): four of the finest voxels.
pub(crate) const BASE_SPACING: f32 = 0.5;
/// A probe's words in its buffer (`gi_probe.wgsl`'s `PROBE_WORDS`).
const PROBE_WORDS: u64 = 240;
/// Texels a side of a probe's irradiance tile and of its depth tile (each with a border).
pub(crate) const IRR_TILE: u32 = 8;
pub(crate) const DEPTH_TILE: u32 = 10;
/// The state's bytes before the probes' (`GiProbes`).
pub(crate) const HEADER_BYTES: u64 = (MAX_CASCADES as u64) * 32 + 32;
/// The probes' params' bytes (`ProbeParams`).
const PARAMS_BYTES: u64 = 64 + 48;

/// The atlas's format (rgb irradiance; the depth moments in rg).
pub(crate) const ATLAS_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

/// A cascade's spacing.
pub(crate) fn spacing(cascade: u32) -> f32 {
    BASE_SPACING * (1u32 << cascade) as f32
}

/// The atlas's size for `cascades` cascades: as wide as a cascade's depth tiles side by side (x times z), as tall as every
/// cascade's irradiance rows and then their depth rows.
pub(crate) fn atlas_size(cascades: u32) -> (u32, u32) {
    let columns = (PROBES[0] * PROBES[2]) as u32;
    let rows = PROBES[1] as u32 * cascades;
    (columns * DEPTH_TILE, rows * (IRR_TILE + DEPTH_TILE))
}

/// The lowest probe's coordinates of each cascade about `eye` (half the grid each way).
pub(crate) fn origins(eye: [f32; 3], cascades: u32) -> [[i32; 3]; MAX_CASCADES] {
    std::array::from_fn(|c| {
        if c as u32 >= cascades {
            return [0; 3];
        }
        let h = spacing(c as u32);
        std::array::from_fn(|a| (eye[a] / h).floor().clamp(-1e8, 1e8) as i32 - PROBES[a] / 2)
    })
}

/// The state's header bytes (`GiProbes`): each cascade's lowest probe and spacing, the cascades in use, the atlas's texel
/// size and the depth rows' first row.
pub(crate) fn header_bytes(origins: &[[i32; 3]; MAX_CASCADES], cascades: u32) -> Vec<u8> {
    let mut b = Vec::with_capacity(HEADER_BYTES as usize);
    for (c, o) in origins.iter().enumerate() {
        for v in o {
            b.extend_from_slice(&v.to_le_bytes());
        }
        b.extend_from_slice(&0i32.to_le_bytes());
        let h = spacing(c as u32);
        crate::push_f32(&mut b, &[h, 1.0 / h, 0.0, 0.0]);
    }
    crate::push_u32(&mut b, &[cascades, 0, 0, 0]);
    let (w, h) = atlas_size(cascades.max(1));
    let depth_row = PROBES[1] as u32 * cascades.max(1) * IRR_TILE;
    crate::push_f32(
        &mut b,
        &[1.0 / w as f32, 1.0 / h as f32, depth_row as f32, 0.0],
    );
    b
}

/// A whole buffer at `binding`.
fn b(binding: u32, buffer: &wgpu::Buffer) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry {
        binding,
        resource: buffer.as_entire_binding(),
    }
}

/// A rotation of the ray set from the frame number (a hash of it as a uniform rotation, Shoemake's): the same frames turn the
/// rays the same way.
fn rotation(frame: u32) -> [f32; 16] {
    let hash = |k: u32| {
        let mut x = frame.wrapping_mul(0x9e37_79b9) ^ k.wrapping_mul(0x85eb_ca6b);
        x ^= x >> 16;
        x = x.wrapping_mul(0x7feb_352d);
        x ^= x >> 15;
        x = x.wrapping_mul(0x846c_a68b);
        x ^= x >> 16;
        (x >> 8) as f32 / (1u32 << 24) as f32
    };
    let (u1, u2, u3) = (hash(1), hash(2), hash(3));
    let tau = std::f32::consts::TAU;
    let (a, b) = ((1.0 - u1).sqrt(), u1.sqrt());
    let q = [
        a * (tau * u2).sin(),
        a * (tau * u2).cos(),
        b * (tau * u3).sin(),
        b * (tau * u3).cos(),
    ];
    let m = glam::Mat3::from_quat(glam::Quat::from_xyzw(q[0], q[1], q[2], q[3]).normalize());
    let c = m.to_cols_array();
    [
        c[0], c[1], c[2], 0.0, c[3], c[4], c[5], 0.0, c[6], c[7], c[8], 0.0, 0.0, 0.0, 0.0, 1.0,
    ]
}

/// What a frame traces of the probes: the rays a probe, the frames between a probe's turns, the frame, whether every probe is
/// traced, and the cascades.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Trace {
    pub(crate) rays: u32,
    pub(crate) period: u32,
    pub(crate) frame: u32,
    pub(crate) all: bool,
    pub(crate) cascades: u32,
}

/// The hysteresis a probe keeps of its last irradiance, the one after a large change and the change that is large (a share of
/// its luminance); a depth tile's reach (spacings).
const KEEP: f32 = 0.9;
const KEEP_FAST: f32 = 0.5;
const LARGE: f32 = 0.25;
const DEPTH_REACH: f32 = 2.0;

/// The probes' buffers, atlas, pipeline and group.
pub(crate) struct ProbeGpu {
    pub(crate) cascades: u32,
    pub(crate) state: wgpu::Buffer,
    data: wgpu::Buffer,
    pub(crate) atlas_view: wgpu::TextureView,
    storage_view: wgpu::TextureView,
    params: wgpu::Buffer,
    layout: wgpu::BindGroupLayout,
    pipeline: wgpu::ComputePipeline,
    group: Option<(wgpu::Buffer, wgpu::BindGroup)>,
    count: u32,
}

impl ProbeGpu {
    pub(crate) fn new(
        device: &wgpu::Device,
        cache: Option<&wgpu::PipelineCache>,
        cascades: u32,
    ) -> ProbeGpu {
        let probes = u64::from(cascades) * u64::from(CASCADE_PROBES);
        let compute = wgpu::ShaderStages::COMPUTE;
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("hd gi probes"),
            entries: &[
                crate::uniform(0, compute),
                crate::uniform(1, compute),
                crate::storage(2, true, compute),
                crate::storage(3, true, compute),
                crate::storage(4, true, compute),
                crate::storage(5, false, compute),
                crate::storage(6, false, compute),
                crate::storage_target(7, ATLAS_FORMAT),
                crate::uniform(8, compute),
            ],
        });
        let module = crate::module(device, Permutation::GiProbe);
        let pipeline =
            crate::compute_filter(device, cache, "hd gi probes", &layout, &module, "cs_probe");
        let (w, h) = atlas_size(cascades);
        let atlas = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("hd gi atlas"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: ATLAS_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING,
            view_formats: &[],
        });
        let view = atlas.create_view(&wgpu::TextureViewDescriptor::default());
        let buffer = |label, size, usage| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage,
                mapped_at_creation: false,
            })
        };
        ProbeGpu {
            cascades,
            state: buffer(
                "hd gi probe state",
                HEADER_BYTES + probes * 16,
                wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::COPY_DST
                    | wgpu::BufferUsages::COPY_SRC,
            ),
            data: buffer(
                "hd gi probe words",
                probes * PROBE_WORDS * 4,
                wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            ),
            atlas_view: view.clone(),
            storage_view: view,
            params: buffer(
                "hd gi probe params",
                PARAMS_BYTES,
                wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            ),
            layout,
            pipeline,
            group: None,
            count: 0,
        }
    }

    /// The GPU bytes the probes take.
    pub(crate) fn bytes(&self) -> u64 {
        let (w, h) = atlas_size(self.cascades);
        self.state.size() + self.data.size() + u64::from(w) * u64::from(h) * 8
    }

    /// Writes the cascades' places about `eye` and the frame's trace.
    pub(crate) fn write(&mut self, queue: &wgpu::Queue, eye: [f32; 3], trace: Trace) {
        let o = origins(eye, self.cascades);
        queue.write_buffer(&self.state, 0, &header_bytes(&o, self.cascades));
        let mut p = Vec::with_capacity(PARAMS_BYTES as usize);
        crate::push_f32(&mut p, &rotation(trace.frame));
        crate::push_u32(
            &mut p,
            &[trace.rays, trace.period, trace.frame, u32::from(trace.all)],
        );
        crate::push_f32(&mut p, &[KEEP, KEEP_FAST, LARGE, DEPTH_REACH]);
        crate::push_u32(&mut p, &[trace.cascades, 0, 0, 0]);
        queue.write_buffer(&self.params, 0, &p);
        self.count = trace.cascades * CASCADE_PROBES;
    }

    /// Traces and blends the frame's probes (`sky`: the irradiance's coefficients).
    pub(crate) fn encode(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        voxels: &VoxelGpu,
        sky: &wgpu::Buffer,
    ) {
        if self.count == 0 {
            return;
        }
        if self.group.as_ref().is_none_or(|(k, _)| k != sky) {
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("hd gi probes"),
                layout: &self.layout,
                entries: &[
                    b(0, &voxels.volume),
                    b(1, &self.params),
                    b(2, &voxels.tiles),
                    b(3, &voxels.occ),
                    b(4, &voxels.voxels),
                    b(5, &self.state),
                    b(6, &self.data),
                    crate::tv(7, &self.storage_view),
                    b(8, sky),
                ],
            });
            self.group = Some((sky.clone(), group));
        }
        let Some((_, group)) = &self.group else {
            return;
        };
        crate::count_pass();
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("hd gi probes"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, group, &[]);
        pass.dispatch_workgroups(self.count.min(ROW), self.count.div_ceil(ROW), 1);
        self.count = 0;
    }
}
