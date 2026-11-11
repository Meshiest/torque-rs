//! The enhanced renderer's anti-aliasing and ambient occlusion on the GPU: the pipelines of the AO
//! pass (`ao.wgsl`: GTAO or SSAO, each frame's estimate on its own) and its spatial filter, the TAA resolve (`taa.wgsl`) and the FXAA-style filter (`fxaa.wgsl`), the
//! shapes' motion matrices the prepass reads, and the frame-to-frame state ([`Temporal`]): the jitter's frame count, which of the two
//! copies of each history is written this frame, the last frame's camera and what was drawn.
//!
//! **Histories** (each kept twice, read last frame's and write this frame's): the depth and the motion at the internal frame's
//! size, the TAA's colour at the output's. The lighting keeps none: the AO's estimate and the shadows' patterns are fixed per
//! pixel and filtered in space, so a change of light is in the frame it happens and the TAA's clip catches it there. They start again (a cut) when the targets are made anew
//! (the size or the quality changed), the mode changes, the camera moves more than [`CUT_DISTANCE`] units in a frame, or
//! the renderer resets them; a frame without the TAA leaves its history stale, so the next one with it
//! starts again too.

use std::collections::HashMap;

use glam::Vec3;
use torque_render_core::wgpu;
use torque_render_scene::hd::HdMode;
use torque_render_scene::hd::post::{COLUMN_IDENTITY, CUT_DISTANCE};

use crate::full_frame;
use crate::shaders::Permutation;

/// The prepass's motion target (uv motion, the previous eye distance, drawn), its normal target (octahedral; z the surface's
/// reflection class), the AO's and the TAA history's formats.
pub const MOTION_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
pub const NORMAL_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
pub const AO_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
pub const HISTORY_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba32Float;
/// The TAA's shown color (the tonemap's source) with, in alpha, its 3 x 3 neighborhood's mean luma (the next frame's change
/// test).
pub const SHOWN_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// One motion slot's stride in its storage buffer (forward.wgsl's `Motion`, padded to the dynamic offset's alignment: the
/// reference way binds each slot at its offset).
pub const MOTION_STRIDE: u64 = 256;
/// The most distinct object ids the motion target holds (its w is 1 + the id in a half float: every integer to 2048 is exact).
pub const MOTION_IDS: u32 = 2047;

fn module(device: &wgpu::Device, p: Permutation) -> wgpu::ShaderModule {
    device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(&p.name()),
        source: wgpu::ShaderSource::Wgsl(p.source().into()),
    })
}

fn texture(binding: u32, filterable: bool) -> wgpu::BindGroupLayoutEntry {
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

fn depth(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Depth,
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

fn frame_uniform(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn filtering(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
        count: None,
    }
}

/// The post passes' pipelines and layouts, the linear sampler, and the motion matrices.
pub struct PostPipes {
    pub ao_layout: wgpu::BindGroupLayout,
    pub taa_layout: wgpu::BindGroupLayout,
    pub fxaa_layout: wgpu::BindGroupLayout,
    pub motion_layout: wgpu::BindGroupLayout,
    /// The AO pass by method (true: GTAO, false: SSAO).
    pub ao: HashMap<bool, wgpu::RenderPipeline>,
    /// The AO's spatial filter on the AO pass's layout.
    pub ao_filter: wgpu::RenderPipeline,
    /// The sun's visibility mask's spatial filter, on the AO pass's layout too (the depth, the normals, the mask as its input).
    pub contact_filter: wgpu::RenderPipeline,
    pub taa: wgpu::RenderPipeline,
    pub fxaa: wgpu::RenderPipeline,
    pub linear: wgpu::Sampler,
    pub motion: wgpu::Buffer,
    pub motion_slots: usize,
    /// The whole motion buffer (a draw's slot is its instance index), and one slot of it at a dynamic offset (the reference way:
    /// each moving draw binds its own).
    pub motion_group: wgpu::BindGroup,
    pub motion_one: wgpu::BindGroup,
}

impl PostPipes {
    pub fn new(device: &wgpu::Device, cache: Option<&wgpu::PipelineCache>) -> PostPipes {
        let ao_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("hd ao"),
            entries: &[
                frame_uniform(0),
                depth(1),
                texture(2, false),
                texture(3, false),
                texture(4, false),
            ],
        });
        let taa_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("hd taa"),
            entries: &[
                frame_uniform(0),
                texture(1, false),
                depth(2),
                depth(3),
                texture(4, false),
                texture(5, false),
                texture(6, false),
                texture(7, true),
                filtering(8),
                texture(9, true),
                texture(10, true),
            ],
        });
        let fxaa_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("hd fxaa"),
            entries: &[texture(0, true), filtering(1)],
        });
        let motion_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("hd motion"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: true,
                    min_binding_size: wgpu::BufferSize::new(MOTION_STRIDE),
                },
                count: None,
            }],
        });
        let ao = [true, false]
            .into_iter()
            .map(|gtao| {
                let m = module(device, Permutation::Ao { gtao });
                (
                    gtao,
                    full_frame(
                        device,
                        cache,
                        "hd ao",
                        &ao_layout,
                        &m,
                        "fs_ao",
                        &[AO_FORMAT],
                    ),
                )
            })
            .collect();
        let ao_filter_module = module(device, Permutation::AoFilter);
        let ao_filter = full_frame(
            device,
            cache,
            "hd ao filter",
            &ao_layout,
            &ao_filter_module,
            "fs_ao_filter",
            &[AO_FORMAT],
        );
        let contact_filter_module = module(device, Permutation::ContactFilter);
        let contact_filter = full_frame(
            device,
            cache,
            "hd contact filter",
            &ao_layout,
            &contact_filter_module,
            "fs_contact_filter",
            &[crate::CONTACT_FORMAT],
        );
        let taa_module = module(device, Permutation::Taa);
        let taa = full_frame(
            device,
            cache,
            "hd taa",
            &taa_layout,
            &taa_module,
            "fs_taa",
            &[HISTORY_FORMAT, SHOWN_FORMAT],
        );
        let fxaa_module = module(device, Permutation::Fxaa);
        let fxaa = full_frame(
            device,
            cache,
            "hd fxaa",
            &fxaa_layout,
            &fxaa_module,
            "fs_fxaa",
            &[crate::HDR_FORMAT],
        );
        let linear = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("hd linear clamp"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..wgpu::SamplerDescriptor::default()
        });
        let (motion, motion_group, motion_one) = Self::motion_buffer(device, &motion_layout, 16);
        PostPipes {
            ao_layout,
            taa_layout,
            fxaa_layout,
            motion_layout,
            ao,
            ao_filter,
            contact_filter,
            taa,
            fxaa,
            linear,
            motion,
            motion_slots: 16,
            motion_group,
            motion_one,
        }
    }

    fn motion_buffer(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        slots: usize,
    ) -> (wgpu::Buffer, wgpu::BindGroup, wgpu::BindGroup) {
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("hd motion"),
            size: slots as u64 * MOTION_STRIDE,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let group = |size: u64| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("hd motion"),
                layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &buffer,
                        offset: 0,
                        size: wgpu::BufferSize::new(size),
                    }),
                }],
            })
        };
        let (whole, one) = (group(slots as u64 * MOTION_STRIDE), group(MOTION_STRIDE));
        (buffer, whole, one)
    }

    /// Writes the motion slots (slot 0 the identity and object 0, then one a shape: its matrix and its object id, which the prepass
    /// writes beside each pixel's motion for the TAA's object test), growing the buffer when needed.
    pub fn write_motion(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        matrices: &[([f32; 16], f32)],
    ) {
        let slots = matrices.len() + 1;
        if slots > self.motion_slots {
            self.motion_slots = slots.next_power_of_two();
            let (b, g, one) = Self::motion_buffer(device, &self.motion_layout, self.motion_slots);
            self.motion = b;
            self.motion_group = g;
            self.motion_one = one;
        }
        let mut bytes = vec![0u8; slots * MOTION_STRIDE as usize];
        for (i, (m, id)) in std::iter::once(&(COLUMN_IDENTITY, 0.0))
            .chain(matrices)
            .enumerate()
        {
            let at = i * MOTION_STRIDE as usize;
            for (k, v) in m.iter().chain(std::iter::once(id)).enumerate() {
                bytes[at + 4 * k..at + 4 * k + 4].copy_from_slice(&v.to_le_bytes());
            }
        }
        queue.write_buffer(&self.motion, 0, &bytes);
    }
}

/// What carries over from one frame to the next.
#[derive(Debug, Clone, Default)]
pub struct Temporal {
    /// Frames since the last cut (the jitter's phase, the AO's noise).
    pub frame: u64,
    /// The copy of each history written this frame (the other is last frame's).
    pub parity: usize,
    /// Last frame's internal camera without its jitter, and its eye.
    pub prev_view_proj: Option<[f32; 16]>,
    pub prev_eye: Option<[f32; 3]>,
    /// Last frame's effect time (the time-animated vertices' previous places: the shape fx).
    pub prev_time: Option<f32>,
    /// Whether last frame wrote the TAA's and the AO's histories.
    pub taa_written: bool,
    pub ao_written: bool,
    /// The draws' generation last frame drew (a change: the shapes moved between the two frames) and its mode (None inside: the
    /// fixed-function renderer's).
    pub generation: Option<u64>,
    pub mode: Option<Option<HdMode>>,
}

impl Temporal {
    /// Whether this frame is a cut: the mode changed or the eye jumped.
    pub fn is_cut(&self, mode: Option<HdMode>, eye: [f32; 3]) -> bool {
        let jumped = self
            .prev_eye
            .is_some_and(|p| Vec3::from(eye).distance(p.into()) > CUT_DISTANCE);
        jumped || self.mode.is_some_and(|m| m != mode)
    }

    /// After a frame: the camera, what was written, the next copies.
    #[allow(clippy::too_many_arguments)]
    pub fn advance(
        &mut self,
        view_proj: [f32; 16],
        eye: [f32; 3],
        taa: bool,
        ao: bool,
        generation: Option<u64>,
        mode: Option<HdMode>,
        time: f32,
    ) {
        self.prev_view_proj = Some(view_proj);
        self.prev_eye = Some(eye);
        self.prev_time = Some(time);
        self.taa_written = taa;
        self.ao_written = ao;
        self.generation = generation;
        self.mode = Some(mode);
        self.frame += 1;
        self.parity ^= 1;
    }
}
