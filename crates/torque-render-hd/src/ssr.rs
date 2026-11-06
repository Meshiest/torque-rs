//! The screen-space reflections on the GPU: the Hi-Z pyramid of the depth prepass (`hiz.wgsl`), the
//! trace at half or full resolution and its accumulation (`ssr.wgsl`), and the previous frame's colour with its mip chain (the TAA's
//! anti-aliased frame, or the frame after the translucent pass without it: what the next frame's rays see).
//!
//! The Hi-Z texture is the internal frame padded to a multiple of its coarsest level's texel ([`HIZ_LEVELS`] levels), so level `l`'s
//! texel `c` covers exactly the pixels `c 2^l .. (c + 1) 2^l` (the padding holds the far plane).

use torque_render_core::wgpu;

use crate::capture::{CUBE_FORMAT, DownPipe};
use crate::shaders::Permutation;

/// The Hi-Z levels (the shader's `HIZ_TOP` is the last) and their format.
pub const HIZ_LEVELS: u32 = 8;
const HIZ_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R32Float;
/// The trace's targets: the reflected colour (premultiplied, its weight) and the reflector's previous uv.
const RAW_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
const UV_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rg32Float;
/// The previous frame's colour pyramid's most levels.
pub const COLOR_LEVELS: u32 = 6;

fn texture_entry(binding: u32, filterable: bool) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

/// The pipelines and layouts (made once).
pub struct SsrPipes {
    hiz_first_layout: wgpu::BindGroupLayout,
    hiz_down_layout: wgpu::BindGroupLayout,
    trace_layout: wgpu::BindGroupLayout,
    resolve_layout: wgpu::BindGroupLayout,
    hiz_first: wgpu::ComputePipeline,
    hiz_down: wgpu::ComputePipeline,
    trace: wgpu::RenderPipeline,
    resolve: wgpu::RenderPipeline,
}

impl SsrPipes {
    pub fn new(device: &wgpu::Device, cache: Option<&wgpu::PipelineCache>) -> SsrPipes {
        let hiz_first_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("hd hiz first"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                crate::storage_target(2, HIZ_FORMAT),
            ],
        });
        let hiz_down_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("hd hiz down"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ..texture_entry(1, false)
                },
                crate::storage_target(2, HIZ_FORMAT),
            ],
        });
        let trace_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("hd ssr trace"),
            entries: &[
                crate::uniform(0, wgpu::ShaderStages::FRAGMENT),
                crate::uniform(1, wgpu::ShaderStages::FRAGMENT),
                texture_entry(2, false),
                texture_entry(3, false),
                texture_entry(4, true),
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let resolve_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("hd ssr resolve"),
            entries: &[
                crate::uniform(1, wgpu::ShaderStages::FRAGMENT),
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                texture_entry(6, false),
                texture_entry(7, false),
                texture_entry(8, true),
            ],
        });
        let module = |p: Permutation| {
            device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(&p.name()),
                source: wgpu::ShaderSource::Wgsl(p.source().into()),
            })
        };
        let hiz = module(Permutation::Hiz);
        let ssr = module(Permutation::Ssr);
        let full = crate::full_frame;
        SsrPipes {
            hiz_first: crate::compute_filter(
                device,
                cache,
                "hd hiz first",
                &hiz_first_layout,
                &hiz,
                "cs_first",
            ),
            hiz_down: crate::compute_filter(
                device,
                cache,
                "hd hiz down",
                &hiz_down_layout,
                &hiz,
                "cs_down",
            ),
            trace: full(
                device,
                cache,
                "hd ssr trace",
                &trace_layout,
                &ssr,
                "fs_trace",
                &[RAW_FORMAT, UV_FORMAT],
            ),
            resolve: full(
                device,
                cache,
                "hd ssr resolve",
                &resolve_layout,
                &ssr,
                "fs_resolve",
                &[RAW_FORMAT],
            ),
            hiz_first_layout,
            hiz_down_layout,
            trace_layout,
            resolve_layout,
        }
    }
}

/// The SSR's targets for one internal frame size and SSR scale, with their groups.
pub struct SsrTargets {
    /// The Hi-Z pyramid's level 0 size (its levels are written by compute filters through the groups).
    hiz_size: (u32, u32),
    hiz_first: wgpu::BindGroup,
    hiz_down: Vec<wgpu::BindGroup>,
    raw_view: wgpu::TextureView,
    uv_view: wgpu::TextureView,
    /// The accumulated result, two copies (this frame's and last frame's by parity).
    pub history: [wgpu::TextureView; 2],
    trace: wgpu::BindGroup,
    resolve: [wgpu::BindGroup; 2],
    /// The previous frame's colour (mip 0 the copy) and its level views and filter groups.
    pub color: wgpu::Texture,
    color_levels: Vec<wgpu::TextureView>,
    color_down: Vec<wgpu::BindGroup>,
}

/// What the SSR's groups read from the frame.
pub struct SsrInputs<'a> {
    pub frame: &'a wgpu::Buffer,
    pub refl: &'a wgpu::Buffer,
    pub depth: &'a wgpu::TextureView,
    pub normals: &'a wgpu::TextureView,
    pub sampler: &'a wgpu::Sampler,
}

impl SsrTargets {
    pub fn new(
        device: &wgpu::Device,
        pipes: &SsrPipes,
        down: &DownPipe,
        (iw, ih, scale, cw, ch): (u32, u32, u32, u32, u32),
        inputs: &SsrInputs<'_>,
    ) -> SsrTargets {
        let tex = |label: &str, w: u32, h: u32, mips: u32, format, usage| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: w.max(1),
                    height: h.max(1),
                    depth_or_array_layers: 1,
                },
                mip_level_count: mips,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
        };
        let level_view = |t: &wgpu::Texture, m: u32| {
            t.create_view(&wgpu::TextureViewDescriptor {
                base_mip_level: m,
                mip_level_count: Some(1),
                ..wgpu::TextureViewDescriptor::default()
            })
        };
        let target = wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING;
        let pad = 1u32 << (HIZ_LEVELS - 1);
        let hiz_size = (iw.div_ceil(pad) * pad, ih.div_ceil(pad) * pad);
        let hiz = tex(
            "hd hiz",
            hiz_size.0,
            hiz_size.1,
            HIZ_LEVELS,
            HIZ_FORMAT,
            wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING,
        );
        let hiz_levels: Vec<wgpu::TextureView> =
            (0..HIZ_LEVELS).map(|m| level_view(&hiz, m)).collect();
        let hiz_all = hiz.create_view(&wgpu::TextureViewDescriptor::default());
        let hiz_first = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("hd hiz first"),
            layout: &pipes.hiz_first_layout,
            entries: &[crate::tv(0, inputs.depth), crate::tv(2, &hiz_levels[0])],
        });
        let hiz_down = hiz_levels
            .windows(2)
            .map(|v| {
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("hd hiz down"),
                    layout: &pipes.hiz_down_layout,
                    entries: &[crate::tv(1, &v[0]), crate::tv(2, &v[1])],
                })
            })
            .collect();
        let (sw, sh) = (iw.div_ceil(scale), ih.div_ceil(scale));
        let raw_view = tex("hd ssr raw", sw, sh, 1, RAW_FORMAT, target)
            .create_view(&wgpu::TextureViewDescriptor::default());
        let uv_view = tex("hd ssr uv", sw, sh, 1, UV_FORMAT, target)
            .create_view(&wgpu::TextureViewDescriptor::default());
        let history = [0, 1].map(|_| {
            tex("hd ssr history", sw, sh, 1, RAW_FORMAT, target)
                .create_view(&wgpu::TextureViewDescriptor::default())
        });
        let mips = COLOR_LEVELS.min(32 - cw.max(ch).max(1).leading_zeros());
        let color = tex(
            "hd previous colour",
            cw,
            ch,
            mips,
            CUBE_FORMAT,
            target | wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::STORAGE_BINDING,
        );
        let color_levels: Vec<wgpu::TextureView> =
            (0..mips).map(|m| level_view(&color, m)).collect();
        let color_all = color.create_view(&wgpu::TextureViewDescriptor::default());
        let color_down = color_levels
            .windows(2)
            .map(|v| down.group(device, &v[0], &v[1]))
            .collect();
        let sampler = wgpu::BindGroupEntry {
            binding: 5,
            resource: wgpu::BindingResource::Sampler(inputs.sampler),
        };
        let trace = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("hd ssr trace"),
            layout: &pipes.trace_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: inputs.frame.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: inputs.refl.as_entire_binding(),
                },
                crate::tv(2, &hiz_all),
                crate::tv(3, inputs.normals),
                crate::tv(4, &color_all),
                sampler.clone(),
            ],
        });
        let resolve = [0, 1].map(|p| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("hd ssr resolve"),
                layout: &pipes.resolve_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: inputs.refl.as_entire_binding(),
                    },
                    sampler.clone(),
                    crate::tv(6, &raw_view),
                    crate::tv(7, &uv_view),
                    crate::tv(8, &history[1 - p]),
                ],
            })
        });
        SsrTargets {
            hiz_size,
            hiz_first,
            hiz_down,
            raw_view,
            uv_view,
            history,
            trace,
            resolve,
            color,
            color_levels,
            color_down,
        }
    }

    /// The Hi-Z pyramid, the trace and the accumulation into `history[p]` (the first pass starts `begin`'s timestamp, the last ends
    /// `end`'s).
    pub fn encode(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        pipes: &SsrPipes,
        p: usize,
        stamps: Option<(&wgpu::QuerySet, u32, u32)>,
    ) {
        let pass = |encoder: &mut wgpu::CommandEncoder,
                    label: &str,
                    targets: &[Option<wgpu::RenderPassColorAttachment<'_>>],
                    pipeline: &wgpu::RenderPipeline,
                    group: &wgpu::BindGroup,
                    stamp: Option<wgpu::RenderPassTimestampWrites<'_>>| {
            crate::count_pass();
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some(label),
                color_attachments: targets,
                depth_stencil_attachment: None,
                timestamp_writes: stamp,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, group, &[]);
            pass.draw(0..3, 0..1);
        };
        let begin = stamps.map(|(q, b, _)| wgpu::ComputePassTimestampWrites {
            query_set: q,
            beginning_of_pass_write_index: Some(b),
            end_of_pass_write_index: None,
        });
        let end = stamps.map(|(q, _, e)| wgpu::RenderPassTimestampWrites {
            query_set: q,
            beginning_of_pass_write_index: None,
            end_of_pass_write_index: Some(e),
        });
        // The pyramid in one compute pass (it was a render pass a level).
        let (w, h) = self.hiz_size;
        let mut chain = vec![crate::Dispatch {
            pipeline: &pipes.hiz_first,
            group: &self.hiz_first,
            size: (w, h),
        }];
        chain.extend(
            self.hiz_down
                .iter()
                .enumerate()
                .map(|(m, g)| crate::Dispatch {
                    pipeline: &pipes.hiz_down,
                    group: g,
                    size: crate::capture::mip_size(w, h, m as u32 + 1),
                }),
        );
        crate::encode_chain(encoder, "hd hiz", &chain, begin);
        pass(
            encoder,
            "hd ssr trace",
            &[
                Some(crate::colour(&self.raw_view)),
                Some(crate::colour(&self.uv_view)),
            ],
            &pipes.trace,
            &self.trace,
            None,
        );
        pass(
            encoder,
            "hd ssr resolve",
            &[Some(crate::colour(&self.history[p]))],
            &pipes.resolve,
            &self.resolve[p],
            end,
        );
    }

    /// The colour pyramid's mip chain from its level 0 (after the frame was copied in).
    pub fn encode_color_mips(&self, encoder: &mut wgpu::CommandEncoder, down: &DownPipe) {
        let (w, h) = (self.color.width(), self.color.height());
        let sizes: Vec<(u32, u32)> = (1..=self.color_down.len() as u32)
            .map(|m| crate::capture::mip_size(w, h, m))
            .collect();
        down.encode_chain(encoder, &self.color_down, &sizes);
    }

    pub fn color_levels(&self) -> u32 {
        self.color_levels.len() as u32
    }
}
