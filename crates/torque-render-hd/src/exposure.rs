//! The automatic exposure on the GPU: each frame the histogram of
//! the tonemap's source and the adaptation, into a state buffer the tonemap reads (its EV). The frame's time is the caller's
//! (1/60 s unless set, so a headless run's frames are the same frames); a history's first frame (and
//! the first after [`Exposure::reset`]) takes the target at once.

use torque_render_core::wgpu;
use torque_render_scene::hd::grade::{
    ADAPT_SECONDS, DEFAULT_KEY, EV_CLAMP, HIGH_PERCENTILE, HISTOGRAM_BINS, HISTOGRAM_MAX_EV,
    HISTOGRAM_MIN_EV, LOW_PERCENTILE,
};

use crate::push_f32;
use crate::shaders::Permutation;

pub struct Exposure {
    layout: wgpu::BindGroupLayout,
    histogram: wgpu::ComputePipeline,
    adapt: wgpu::ComputePipeline,
    params: wgpu::Buffer,
    pub bins: wgpu::Buffer,
    pub state: wgpu::Buffer,
    /// The next frame starts the adaptation again.
    reset: bool,
}

fn storage(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only: false },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

impl Exposure {
    pub fn new(device: &wgpu::Device) -> Exposure {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("hd exposure"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                storage(2),
                storage(3),
            ],
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("hd exposure"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let module = crate::module(device, Permutation::Exposure);
        let compute = |entry: &str| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: Some(&pl),
                module: &module,
                entry_point: Some(entry),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                cache: None,
            })
        };
        let buffer = |label: &str, size: u64, usage: wgpu::BufferUsages| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage,
                mapped_at_creation: false,
            })
        };
        Exposure {
            histogram: compute("cs_histogram"),
            adapt: compute("cs_adapt"),
            params: buffer(
                "hd exposure params",
                48,
                wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            ),
            bins: buffer(
                "hd exposure bins",
                (HISTOGRAM_BINS * 4) as u64,
                wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::COPY_DST
                    | wgpu::BufferUsages::COPY_SRC,
            ),
            state: buffer(
                "hd exposure state",
                16,
                wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::COPY_DST
                    | wgpu::BufferUsages::COPY_SRC,
            ),
            layout,
            reset: true,
        }
    }

    /// The next frame takes its target at once.
    pub fn reset(&mut self) {
        self.reset = true;
    }

    /// The histogram of `source` (`w` x `h`) and the adaptation over `dt` seconds.
    #[allow(clippy::too_many_arguments)]
    pub fn encode(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        source: &wgpu::TextureView,
        (w, h): (u32, u32),
        dt: f32,
    ) {
        let mut p = Vec::with_capacity(48);
        push_f32(
            &mut p,
            &[
                HISTOGRAM_MIN_EV,
                HISTOGRAM_MAX_EV,
                LOW_PERCENTILE,
                HIGH_PERCENTILE,
            ],
        );
        push_f32(&mut p, &[DEFAULT_KEY.log2(), EV_CLAMP, ADAPT_SECONDS, dt]);
        push_f32(&mut p, &[if self.reset { 1.0 } else { 0.0 }, 0.0, 0.0, 0.0]);
        queue.write_buffer(&self.params, 0, &p);
        self.reset = false;
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("hd exposure"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.params.as_entire_binding(),
                },
                crate::tv(1, source),
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: self.bins.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: self.state.as_entire_binding(),
                },
            ],
        });
        encoder.clear_buffer(&self.bins, 0, None);
        crate::count_pass();
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("hd exposure"),
            timestamp_writes: None,
        });
        pass.set_bind_group(0, &group, &[]);
        pass.set_pipeline(&self.histogram);
        pass.dispatch_workgroups(w.div_ceil(16), h.div_ceil(16), 1);
        pass.set_pipeline(&self.adapt);
        pass.dispatch_workgroups(1, 1, 1);
    }
}
