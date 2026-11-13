//! The probes' ambient at listed points (`gi_query.wgsl`): the forward pass's lookup run on points and normals the caller
//! gives, for the checks that compare the probes with a reference.

use torque_render_core::wgpu;

use crate::shaders::Permutation;

/// The query's pipeline and its layout.
pub(crate) struct QueryPipe {
    layout: wgpu::BindGroupLayout,
    pipeline: wgpu::ComputePipeline,
}

impl QueryPipe {
    pub(crate) fn new(device: &wgpu::Device) -> QueryPipe {
        let compute = wgpu::ShaderStages::COMPUTE;
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("hd gi query"),
            entries: &[
                crate::storage(0, true, compute),
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: compute,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: compute,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                crate::storage(3, true, compute),
                crate::storage(4, false, compute),
            ],
        });
        let module = crate::module(device, Permutation::GiQuery);
        let pipeline =
            crate::compute_filter(device, None, "hd gi query", &layout, &module, "cs_query");
        QueryPipe { layout, pipeline }
    }

    /// The ambient's radiance (linear) the probes give each point facing its normal; black past them. Only as many points as
    /// one buffer binding and one dispatch row of the device hold are answered.
    pub(crate) fn run(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        (atlas, state, sampler): (&wgpu::TextureView, &wgpu::Buffer, &wgpu::Sampler),
        points: &[([f32; 3], [f32; 3])],
    ) -> Result<Vec<[f32; 3]>, String> {
        if points.is_empty() {
            return Ok(Vec::new());
        }
        // Within one binding and one dispatch row of the device (32 bytes in, 16 out a point).
        let limits = device.limits();
        let most = (limits
            .max_storage_buffer_binding_size
            .min(limits.max_buffer_size)
            / 32)
            .min(u64::from(limits.max_compute_workgroups_per_dimension) * 64)
            as usize;
        let points = &points[..points.len().min(most)];
        let mut bytes = Vec::with_capacity(points.len() * 32);
        for (p, n) in points {
            crate::push_f32(&mut bytes, &[p[0], p[1], p[2], 0.0, n[0], n[1], n[2], 0.0]);
        }
        let input = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("hd gi query points"),
            size: bytes.len() as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&input, 0, &bytes);
        let size = points.len() as u64 * 16;
        let output = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("hd gi query answers"),
            size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("hd gi query"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: state.as_entire_binding(),
                },
                crate::tv(1, atlas),
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: input.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: output.as_entire_binding(),
                },
            ],
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("hd gi query"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &group, &[]);
            pass.dispatch_workgroups((points.len() as u32).div_ceil(64), 1, 1);
        }
        queue.submit([encoder.finish()]);
        let data = super::read_buffer(device, queue, &output, size)?;
        Ok(data
            .as_chunks::<16>()
            .0
            .iter()
            .map(|b| {
                std::array::from_fn(|k| {
                    f32::from_le_bytes([b[4 * k], b[4 * k + 1], b[4 * k + 2], b[4 * k + 3]])
                })
            })
            .collect())
    }
}
