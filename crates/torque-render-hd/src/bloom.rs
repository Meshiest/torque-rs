//! The enhanced renderer's bloom on the GPU: the dual-filter chain over the anti-aliased
//! frame, its levels in half floats ([`BLOOM_FORMAT`]), level 0 half the output's size. The way
//! down: the prefilter (the settings' soft threshold) into level 0, then each level from the one above; the way up from the smallest:
//! each level's up texel the mix of its down texel and the tent of the level below's up texels. The tonemap reads the up chain's
//! level 0 ([`Bloom::result`]).

use torque_render_core::wgpu;
use torque_render_scene::hd::grade::BloomSettings;

use crate::push_f32;
use crate::shaders::Permutation;

/// The chain's format. `Rg11b10Ufloat` lost about 2 % of the light over the chain's twelve stores to its rounding; half floats keep
/// it within 0.3 %, for twice the bytes of a small chain.
pub const BLOOM_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// The up mix's spread (the share of each level from the level below: wider tails as it grows).
pub const BLOOM_SPREAD: f32 = 0.6;
/// One pass's uniform slot.
const SLOT: u64 = 256;
/// The most levels.
pub const MAX_LEVELS: u32 = 8;

struct Chain {
    key: (u32, u32, u32),
    sizes: Vec<(u32, u32)>,
    down: Vec<(wgpu::Texture, wgpu::TextureView)>,
    up: Vec<(wgpu::Texture, wgpu::TextureView)>,
    /// The groups of the down passes from level 1 and of the up passes (index k: up level k from level k + 1).
    down_groups: Vec<wgpu::BindGroup>,
    up_groups: Vec<wgpu::BindGroup>,
    /// The prefilter's group and the source it reads (made again only when the source is another).
    prefilter: Option<(wgpu::TextureView, wgpu::BindGroup)>,
}

pub struct Bloom {
    format: wgpu::TextureFormat,
    layout: wgpu::BindGroupLayout,
    prefilter: wgpu::ComputePipeline,
    down: wgpu::ComputePipeline,
    up: wgpu::ComputePipeline,
    mirror: wgpu::Sampler,
    uniforms: wgpu::Buffer,
    /// What `uniforms` holds (written again only when it changes).
    written: Vec<u8>,
    dummy: wgpu::TextureView,
    chain: Option<Chain>,
    /// The levels of the last frame drawn (0: none).
    pub drawn_levels: u32,
}

fn tex_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

impl Bloom {
    pub fn new(device: &wgpu::Device, cache: Option<&wgpu::PipelineCache>) -> Bloom {
        let format = BLOOM_FORMAT;
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("hd bloom"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(32),
                    },
                    count: None,
                },
                tex_entry(1),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                tex_entry(3),
                crate::storage_target(4, BLOOM_FORMAT),
            ],
        });
        let module = crate::module(device, Permutation::Bloom);
        let pipe =
            |entry: &str| crate::compute_filter(device, cache, "hd bloom", &layout, &module, entry);
        let mirror = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("hd bloom mirror"),
            address_mode_u: wgpu::AddressMode::MirrorRepeat,
            address_mode_v: wgpu::AddressMode::MirrorRepeat,
            address_mode_w: wgpu::AddressMode::MirrorRepeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..wgpu::SamplerDescriptor::default()
        });
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("hd bloom levels"),
            size: SLOT * u64::from(2 * MAX_LEVELS + 1),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let dummy = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("hd bloom dummy"),
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
            .create_view(&wgpu::TextureViewDescriptor::default());
        Bloom {
            format,
            prefilter: pipe("cs_prefilter"),
            down: pipe("cs_down"),
            up: pipe("cs_up"),
            layout,
            mirror,
            uniforms,
            written: Vec::new(),
            dummy,
            chain: None,
            drawn_levels: 0,
        }
    }

    fn slot(&self, k: u64) -> wgpu::BindingResource<'_> {
        wgpu::BindingResource::Buffer(wgpu::BufferBinding {
            buffer: &self.uniforms,
            offset: k * SLOT,
            size: wgpu::BufferSize::new(32),
        })
    }

    fn group(
        &self,
        device: &wgpu::Device,
        slot: u64,
        src: &wgpu::TextureView,
        below: Option<&wgpu::TextureView>,
        dst: &wgpu::TextureView,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("hd bloom"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.slot(slot),
                },
                crate::tv(1, src),
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.mirror),
                },
                crate::tv(3, below.unwrap_or(&self.dummy)),
                crate::tv(4, dst),
            ],
        })
    }

    fn ensure(&mut self, device: &wgpu::Device, w: u32, h: u32, levels: u32) {
        let key = (w, h, levels);
        if self.chain.as_ref().is_some_and(|c| c.key == key) {
            return;
        }
        let mut sizes = Vec::new();
        let (mut lw, mut lh) = (w, h);
        for _ in 0..levels {
            lw = (lw / 2).max(1);
            lh = (lh / 2).max(1);
            sizes.push((lw, lh));
        }
        let make = |label: &str, (w, h): (u32, u32)| {
            let t = device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: self.format,
                usage: wgpu::TextureUsages::STORAGE_BINDING
                    | wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let v = t.create_view(&wgpu::TextureViewDescriptor::default());
            (t, v)
        };
        let down: Vec<_> = sizes.iter().map(|s| make("hd bloom down", *s)).collect();
        // Up levels 0 .. levels - 2 (the smallest level's up is its down).
        let up: Vec<_> = sizes
            .iter()
            .take(sizes.len().saturating_sub(1))
            .map(|s| make("hd bloom up", *s))
            .collect();
        let down_groups = (1..down.len())
            .map(|k| self.group(device, 1 + k as u64, &down[k - 1].1, None, &down[k].1))
            .collect();
        let n = down.len();
        let up_groups = (0..up.len())
            .map(|k| {
                let below = if k + 1 == n - 1 {
                    &down[n - 1].1
                } else {
                    &up[k + 1].1
                };
                self.group(
                    device,
                    1 + u64::from(MAX_LEVELS) + k as u64,
                    &down[k].1,
                    Some(below),
                    &up[k].1,
                )
            })
            .collect();
        self.chain = Some(Chain {
            key,
            sizes,
            down,
            up,
            down_groups,
            up_groups,
            prefilter: None,
        });
    }

    /// Lets go of the prefilter's group over the last source (its frame's targets are being let go).
    pub(crate) fn forget_source(&mut self) {
        if let Some(c) = &mut self.chain {
            c.prefilter = None;
        }
    }

    /// The chain over `source` (`w` x `h`) with `levels` levels and the settings' threshold.
    #[allow(clippy::too_many_arguments)]
    pub fn encode(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        source: &wgpu::TextureView,
        (w, h): (u32, u32),
        levels: u32,
        settings: &BloomSettings,
    ) {
        let levels = levels.clamp(1, MAX_LEVELS);
        self.ensure(device, w, h, levels);
        let stale = self
            .chain
            .as_ref()
            .is_some_and(|c| c.prefilter.as_ref().is_none_or(|(v, _)| v != source));
        if stale {
            let group = self
                .chain
                .as_ref()
                .map(|c| self.group(device, 0, source, None, &c.down[0].1));
            if let (Some(c), Some(group)) = (self.chain.as_mut(), group) {
                c.prefilter = Some((source.clone(), group));
            }
        }
        let Some(chain) = self.chain.as_ref() else {
            return;
        };
        // The uniforms: slot 0 the prefilter, 1 + k the down pass into level k, 1 + MAX_LEVELS + k the up pass into level k.
        let mut bytes = vec![0u8; (SLOT * u64::from(2 * MAX_LEVELS + 1)) as usize];
        let mut put = |slot: u64, src: (u32, u32), dst: (u32, u32), params: [f32; 4]| {
            let mut b = Vec::with_capacity(32);
            push_f32(
                &mut b,
                &[
                    1.0 / src.0 as f32,
                    1.0 / src.1 as f32,
                    1.0 / dst.0 as f32,
                    1.0 / dst.1 as f32,
                ],
            );
            push_f32(&mut b, &params);
            let at = (slot * SLOT) as usize;
            bytes[at..at + 32].copy_from_slice(&b);
        };
        put(
            0,
            (w, h),
            chain.sizes[0],
            [settings.threshold, settings.knee, 0.0, 0.0],
        );
        for k in 1..chain.sizes.len() {
            put(1 + k as u64, chain.sizes[k - 1], chain.sizes[k], [0.0; 4]);
        }
        for k in 0..chain.up.len() {
            put(
                1 + u64::from(MAX_LEVELS) + k as u64,
                chain.sizes[k],
                chain.sizes[k],
                [0.0, 0.0, BLOOM_SPREAD, 0.0],
            );
        }
        if bytes != self.written {
            queue.write_buffer(&self.uniforms, 0, &bytes);
            self.written = bytes;
        }
        let Some((_, prefilter_group)) = chain.prefilter.as_ref() else {
            return;
        };
        // The whole chain in one compute pass (it was a render pass a level).
        let mut dispatches = vec![crate::Dispatch {
            pipeline: &self.prefilter,
            group: prefilter_group,
            size: chain.sizes[0],
        }];
        for k in 1..chain.down.len() {
            dispatches.push(crate::Dispatch {
                pipeline: &self.down,
                group: &chain.down_groups[k - 1],
                size: chain.sizes[k],
            });
        }
        for k in (0..chain.up.len()).rev() {
            dispatches.push(crate::Dispatch {
                pipeline: &self.up,
                group: &chain.up_groups[k],
                size: chain.sizes[k],
            });
        }
        crate::encode_chain(encoder, "hd bloom", &dispatches, None);
        self.drawn_levels = levels;
    }

    /// The chain's result (the up chain's level 0; with one level, its down level), if drawn.
    pub fn result(&self) -> Option<&wgpu::TextureView> {
        let c = self.chain.as_ref()?;
        if self.drawn_levels == 0 {
            return None;
        }
        Some(c.up.first().map_or(&c.down[0].1, |u| &u.1))
    }

    /// A level's texture: the down chain's (`up` false) or the up chain's level `k`.
    pub fn level(&self, k: usize, up: bool) -> Option<&wgpu::Texture> {
        let c = self.chain.as_ref()?;
        if up {
            c.up.get(k)
                .map(|t| &t.0)
                .or_else(|| (k + 1 == c.down.len()).then(|| &c.down[k].0))
        } else {
            c.down.get(k).map(|t| &t.0)
        }
    }
}
