//! A picture of the voxel scene (`gi_debug.wgsl`): each pixel's ray from a camera marched through the clipmap, the voxel it
//! meets shown by what [`GiView`] names. The checks' and the progress pictures' view of what the voxels hold.

use torque_render_core::wgpu;
use torque_render_scene::hd::HdCamera;

use super::voxels::VoxelGpu;
use crate::shaders::Permutation;

/// What the voxel picture shows of each voxel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GiView {
    /// Its albedo (linear), its faces turned toward the eye lighter.
    Albedo,
    /// Its normal (0.5 + 0.5 n).
    Normal,
    /// Its lit radiance (linear).
    Radiance,
}

impl GiView {
    fn code(self) -> u32 {
        match self {
            GiView::Albedo => 0,
            GiView::Normal => 1,
            GiView::Radiance => 2,
        }
    }
}

const PARAMS_BYTES: u64 = 64 + 16 + 16;

/// The picture's pipeline and its layout.
pub(crate) struct DebugPipe {
    layout: wgpu::BindGroupLayout,
    pipeline: wgpu::ComputePipeline,
}

impl DebugPipe {
    pub(crate) fn new(device: &wgpu::Device) -> DebugPipe {
        let compute = wgpu::ShaderStages::COMPUTE;
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("hd gi debug"),
            entries: &[
                crate::uniform(0, compute),
                crate::uniform(1, compute),
                crate::storage(2, true, compute),
                crate::storage(3, true, compute),
                crate::storage(4, true, compute),
                crate::storage(5, false, compute),
            ],
        });
        let module = crate::module(device, Permutation::GiDebug);
        let pipeline =
            crate::compute_filter(device, None, "hd gi debug", &layout, &module, "cs_debug");
        DebugPipe { layout, pipeline }
    }

    /// The picture of `voxels` from `camera` (its size, halved until it fits a buffer binding of the device), in linear light:
    /// (width, height, the pixels top row first).
    pub(crate) fn draw(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        voxels: &VoxelGpu,
        camera: &HdCamera,
        view: GiView,
    ) -> Result<super::GiImage, String> {
        // Halved until its pixels fit one buffer binding of the device (16 bytes a pixel).
        let limits = device.limits();
        let room = limits
            .max_storage_buffer_binding_size
            .min(limits.max_buffer_size)
            / 16;
        let (mut w, mut h) = (camera.width.max(1), camera.height.max(1));
        while u64::from(w) * u64::from(h) > room && (w > 1 || h > 1) {
            (w, h) = ((w / 2).max(1), (h / 2).max(1));
        }
        let mut params = Vec::with_capacity(PARAMS_BYTES as usize);
        crate::push_f32(&mut params, &crate::invert(&camera.view_proj));
        crate::push_f32(
            &mut params,
            &[camera.eye[0], camera.eye[1], camera.eye[2], 1.0],
        );
        crate::push_u32(&mut params, &[w, h, view.code(), 0]);
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("hd gi debug params"),
            size: PARAMS_BYTES,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&uniform, 0, &params);
        let size = u64::from(w) * u64::from(h) * 16;
        let pixels = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("hd gi debug pixels"),
            size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("hd gi debug"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: voxels.volume.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: voxels.tiles.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: voxels.occ.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: voxels.voxels.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: pixels.as_entire_binding(),
                },
            ],
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("hd gi debug"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &group, &[]);
            pass.dispatch_workgroups(w.div_ceil(8), h.div_ceil(8), 1);
        }
        queue.submit([encoder.finish()]);
        let bytes = super::read_buffer(device, queue, &pixels, size)?;
        let px = bytes
            .as_chunks::<16>()
            .0
            .iter()
            .map(|b| {
                std::array::from_fn(|k| {
                    f32::from_le_bytes([b[4 * k], b[4 * k + 1], b[4 * k + 2], b[4 * k + 3]])
                })
            })
            .collect();
        Ok((w, h, px))
    }
}
