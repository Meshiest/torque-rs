//! The enhanced renderer's volumetric light on the GPU: after the prepass the
//! march at the quality's reduced size (High: half, 24 steps), and after the opaque pass its depth-aware upsample added over the
//! frame. The march's texture ([`Volumetric::texture`]) holds the sun's scattered light alone (the leak gate reads it).

use torque_render_core::wgpu;

use crate::HDR_FORMAT;
use crate::shaders::Permutation;

pub const VOLUMETRIC_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

struct Target {
    key: (u32, u32, u32),
    texture: wgpu::Texture,
    view: wgpu::TextureView,
}

pub struct Volumetric {
    layout: wgpu::BindGroupLayout,
    march: wgpu::RenderPipeline,
    composite: wgpu::RenderPipeline,
    dummy: wgpu::TextureView,
    target: Option<Target>,
    /// Whether the last frame marched.
    pub drawn: bool,
}

impl Volumetric {
    pub fn new(
        device: &wgpu::Device,
        frame_layout: &wgpu::BindGroupLayout,
        sky_layout: &wgpu::BindGroupLayout,
    ) -> Volumetric {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("hd volumetric"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("hd volumetric"),
            bind_group_layouts: &[Some(frame_layout), Some(&layout), Some(sky_layout)],
            immediate_size: 0,
        });
        let module = crate::module(device, Permutation::Volumetric);
        let pipeline =
            |entry: &str, format: wgpu::TextureFormat, blend: Option<wgpu::BlendState>| {
                device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some(entry),
                    layout: Some(&pl),
                    vertex: wgpu::VertexState {
                        module: &module,
                        entry_point: Some("vs_full"),
                        compilation_options: wgpu::PipelineCompilationOptions::default(),
                        buffers: &[],
                    },
                    primitive: wgpu::PrimitiveState::default(),
                    depth_stencil: None,
                    multisample: wgpu::MultisampleState::default(),
                    fragment: Some(wgpu::FragmentState {
                        module: &module,
                        entry_point: Some(entry),
                        compilation_options: wgpu::PipelineCompilationOptions::default(),
                        targets: &[Some(wgpu::ColorTargetState {
                            format,
                            blend,
                            write_mask: wgpu::ColorWrites::ALL,
                        })],
                    }),
                    multiview_mask: None,
                    cache: None,
                })
            };
        let add = wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::Zero,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
        };
        let dummy = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("hd volumetric dummy"),
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: VOLUMETRIC_FORMAT,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
            .create_view(&wgpu::TextureViewDescriptor::default());
        Volumetric {
            march: pipeline("fs_march", VOLUMETRIC_FORMAT, None),
            composite: pipeline("fs_composite", HDR_FORMAT, Some(add)),
            layout,
            dummy,
            target: None,
            drawn: false,
        }
    }

    fn group(
        &self,
        device: &wgpu::Device,
        depth: &wgpu::TextureView,
        vol: &wgpu::TextureView,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("hd volumetric"),
            layout: &self.layout,
            entries: &[crate::tv(0, depth), crate::tv(1, vol)],
        })
    }

    /// The march into its texture (`iw` x `ih` over `scale`), from `depth` (the prepass's).
    #[allow(clippy::too_many_arguments)]
    pub fn encode_march(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        frame_group: &wgpu::BindGroup,
        sky_group: &wgpu::BindGroup,
        depth: &wgpu::TextureView,
        (iw, ih): (u32, u32),
        scale: u32,
    ) {
        let key = (iw, ih, scale.max(1));
        if self.target.as_ref().is_none_or(|t| t.key != key) {
            let (w, h) = (iw.div_ceil(key.2), ih.div_ceil(key.2));
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("hd volumetric"),
                size: wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: VOLUMETRIC_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            self.target = Some(Target { key, texture, view });
        }
        let Some(t) = self.target.as_ref() else {
            return;
        };
        let group = self.group(device, depth, &self.dummy);
        crate::count_pass();
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("hd volumetric march"),
            color_attachments: &[Some(crate::colour(&t.view))],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.march);
        pass.set_bind_group(0, frame_group, &[]);
        pass.set_bind_group(1, &group, &[]);
        pass.set_bind_group(2, sky_group, &[]);
        pass.draw(0..3, 0..1);
        crate::count_draws(1);
        self.drawn = true;
    }

    /// The march's light added over `hdr` (after the opaque pass).
    pub fn encode_composite(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        frame_group: &wgpu::BindGroup,
        sky_group: &wgpu::BindGroup,
        depth: &wgpu::TextureView,
        hdr: &wgpu::TextureView,
    ) {
        let Some(t) = self.target.as_ref() else {
            return;
        };
        let group = self.group(device, depth, &t.view);
        crate::count_pass();
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("hd volumetric composite"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: hdr,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.composite);
        pass.set_bind_group(0, frame_group, &[]);
        pass.set_bind_group(1, &group, &[]);
        pass.set_bind_group(2, sky_group, &[]);
        pass.draw(0..3, 0..1);
        crate::count_draws(1);
    }

    /// The march's texture (the last frame's, if it marched).
    pub fn texture(&self) -> Option<&wgpu::Texture> {
        self.target
            .as_ref()
            .filter(|_| self.drawn)
            .map(|t| &t.texture)
    }
}
