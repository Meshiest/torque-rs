//! Render passes from outside the renderer: a [`HdPass`] added to a [`crate::HdWorld`] at a [`Stage`] of its frame is handed a
//! [`PassCx`] there each frame (the device, the queue, the frame's command encoder, the targets the stage has, the camera the
//! stage draws with, the scene's light and the moving objects) and records what it draws into the encoder. Several passes at a
//! stage run in the order they were added. A stage with no pass costs the frame one check of an empty list.
//!
//! | Stage | When | What it draws into |
//! |-------|------|--------------------|
//! | [`Stage::Shadows`] | after the sun's cascades are drawn | each cascade's map ([`PassCx::cascades`]) |
//! | [`Stage::Prepass`] | after the depth prepass | the depth, the motion and the normals |
//! | [`Stage::Opaque`] | after the opaque pass, before the decals and the volumetric light | the HDR frame over the depth |
//! | [`Stage::Translucent`] | after the translucent passes | the HDR frame over the depth |
//! | [`Stage::AfterTaa`] | after the anti-aliasing, before the exposure, the bloom and the tonemap | the anti-aliased HDR frame |
//! | [`Stage::Post`] | after [`Stage::AfterTaa`] | the same frame, with a copy of it to read ([`PassCx::source`]) |
//! | [`Stage::Overlay`] | after the tonemap | the finished picture ([`PassCx::ldr`]) |
//!
//! **Conventions.** The depth is reversed (1 at the near plane, 0 far away: draw with `GreaterEqual`) in [`DEPTH_FORMAT`]; the
//! HDR frame is linear light in [`HDR_FORMAT`] (the tonemap's exposure comes after); the motion is [`MOTION_FORMAT`] (`xy`: this
//! frame's screen position less the last frame's, in UV; `z`: the last frame's eye distance; `w`: 1 for what is static) and the
//! normals [`NORMAL_FORMAT`] (`xy`: the world normal, octahedral as `common.wgsl`'s `oct_encode`; `z`: 0, no reflection; `w`:
//! 1). Up to [`Stage::Translucent`] the camera is the jittered one the anti-aliasing's history
//! is drawn with; from [`Stage::AfterTaa`] the plain one. A shadow cascade's map is a plain depth (`LessEqual`, cleared to 1);
//! clamp a caster's clip `z` to at least 0 so what lies between the sun and the cascade still casts.
//!
//! **Sampling the sun's shadows.** At [`Stage::Opaque`] and [`Stage::Translucent`] [`PassCx::sun_shadows`] holds what the renderer's
//! own lit shaders read (None when the sun casts no shadows this frame): the shadows' uniform (the cascades' view and projection
//! matrices, their texel sizes and depth ranges, the light's basis, the filter's settings and the shadow's tint), the cascades as
//! one depth array and the two samplers. [`SunShadows::bind_group_layout`] and [`SunShadows::bind_group`] make the group a
//! pipeline reads them through (group [`SUN_SHADOW_GROUP`]), and [`SUN_SHADOW_WGSL`] is the WGSL to add to a shader: the same cascade
//! pick and filter as the forward pass (the file both are made from), as `sun_visibility(p, n, nl, frag, eye)` (1 lit, 0 in
//! shadow; `n` the face's normal, `nl` its N.L, `frag` the fragment's pixel and `eye` the camera's place), with the shadow's tint
//! as `sun_shadow_tint()`. Shade a surface as the forward pass does: `through = vis + (1 - vis) * tint` times the sun's diffuse
//! light. A pass that must draw when the sun casts no shadows makes a [`SunShadowsStandIn`] and binds that (its lookups give 1).
//!
//! The depth at [`Stage::AfterTaa`], [`Stage::Post`] and [`Stage::Overlay`] is the frame's working depth at its internal size
//! ([`PassCx::frame_size`]): a pass attaches it only where [`PassCx::depth_attachable`] says so, else reads it (as the debug
//! lines do, scaled to the viewport).

use torque_render_core::wgpu;
use torque_render_scene::hd::{DynamicObject, HdCamera, HdFog, HdLight, HdSun};

pub use crate::post::{MOTION_FORMAT, NORMAL_FORMAT};
pub use crate::{DEPTH_FORMAT, HDR_FORMAT};

/// Where in the frame a pass runs (in the frame's order).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Stage {
    Shadows,
    Prepass,
    Opaque,
    Translucent,
    AfterTaa,
    Post,
    Overlay,
}

impl Stage {
    /// Every stage, in the frame's order.
    pub const ALL: [Stage; 7] = [
        Stage::Shadows,
        Stage::Prepass,
        Stage::Opaque,
        Stage::Translucent,
        Stage::AfterTaa,
        Stage::Post,
        Stage::Overlay,
    ];

    /// The stage's name in scripts and reports (`"opaque"`, `"after-taa"`).
    pub fn name(self) -> &'static str {
        match self {
            Stage::Shadows => "shadows",
            Stage::Prepass => "prepass",
            Stage::Opaque => "opaque",
            Stage::Translucent => "translucent",
            Stage::AfterTaa => "after-taa",
            Stage::Post => "post",
            Stage::Overlay => "overlay",
        }
    }

    /// The stage named `name` ([`Stage::name`]).
    pub fn of_name(name: &str) -> Option<Stage> {
        Stage::ALL.into_iter().find(|s| s.name() == name)
    }
}

/// One of the sun's cascades as a [`Stage::Shadows`] pass draws into it: its light's view and projection (column major, world to
/// clip) and its map (a layer of the frame's cascades, [`crate::shadows::SHADOW_FORMAT`]).
pub struct ShadowCascade<'a> {
    pub view_proj: [f32; 16],
    pub target: &'a wgpu::TextureView,
    /// The map's side in texels.
    pub size: u32,
}

/// The group [`SUN_SHADOW_WGSL`] reads the sun's shadows through.
pub const SUN_SHADOW_GROUP: u32 = 1;

/// The WGSL to sample the sun's shadows from a pass ([`SunShadows`]): the renderer's own cascade lookup (`sunshadow.wgsl`, the file
/// the forward pass is made from) with its uniform, its structs and the bindings of group [`SUN_SHADOW_GROUP`]; add it to the
/// pass's shader. It defines `Shadows`, `MAX_CASCADES`, `ign`, `vogel`, `shadows`, `csm`, `shadow_cmp`, `shadow_gather`,
/// `sun_shadow_at` (the lookup), and the two functions a pass calls:
///
/// - `sun_visibility(p: vec3<f32>, n: vec3<f32>, nl: f32, frag: vec2<f32>, eye: vec3<f32>) -> f32`: the share of the sun's light
///   that reaches `p` (face normal `n`, N.L `nl` > 0), taps turned per pixel by `frag` (the TAA averages them);
/// - `sun_shadow_tint() -> vec3<f32>`: what of the sun's direct light a shadow lets through.
pub const SUN_SHADOW_WGSL: &str = concat!(
    include_str!("../shaders/shadowcommon.wgsl"),
    "\n",
    include_str!("../shaders/sunshadow.wgsl"),
    r#"
@group(1) @binding(0) var<uniform> shadows: Shadows;
@group(1) @binding(1) var csm: texture_depth_2d_array;
@group(1) @binding(2) var shadow_cmp: sampler_comparison;
@group(1) @binding(3) var shadow_gather: sampler;

fn sun_visibility(p: vec3<f32>, n: vec3<f32>, nl: f32, frag: vec2<f32>, eye: vec3<f32>) -> f32 {
    return sun_shadow_at(p, n, nl, frag, eye, false);
}

fn sun_shadow_tint() -> vec3<f32> {
    return shadows.tint.rgb;
}
"#
);

/// The sun's shadow maps as a pass at [`Stage::Opaque`] or [`Stage::Translucent`] reads them ([`PassCx::sun_shadows`]).
#[derive(Clone, Copy)]
pub struct SunShadows<'a> {
    /// The shadows' uniform (`Shadows` in [`SUN_SHADOW_WGSL`]): fragment stage only.
    pub uniform: &'a wgpu::Buffer,
    /// The cascades, finest first, as a depth array ([`crate::shadows::SHADOW_FORMAT`]).
    pub map: &'a wgpu::TextureView,
    /// A comparison sampler (less or equal, linear) for the maps.
    pub compare: &'a wgpu::Sampler,
    /// The sampler the filter gathers the maps with (nearest).
    pub gather: &'a wgpu::Sampler,
}

impl SunShadows<'_> {
    /// The layout of the group [`SUN_SHADOW_WGSL`] reads (a pipeline's layout has it as group [`SUN_SHADOW_GROUP`]).
    pub fn bind_group_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
        let entry = |binding, ty| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty,
            count: None,
        };
        device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("hd sun shadows"),
            entries: &[
                entry(
                    0,
                    wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                ),
                entry(
                    1,
                    wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                ),
                entry(
                    2,
                    wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
                ),
                entry(
                    3,
                    wgpu::BindingType::Sampler(wgpu::SamplerBindingType::NonFiltering),
                ),
            ],
        })
    }

    /// The group over `layout` ([`SunShadows::bind_group_layout`]). The targets live for this frame: make it each frame.
    pub fn bind_group(
        &self,
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("hd sun shadows"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(self.map),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(self.compare),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(self.gather),
                },
            ],
        })
    }
}

/// Resources a pass binds in place of [`SunShadows`] when the sun casts no shadows: an empty uniform (no cascades), a one texel map
/// and samplers. `sun_visibility` gives 1 with them.
pub struct SunShadowsStandIn {
    uniform: wgpu::Buffer,
    view: wgpu::TextureView,
    compare: wgpu::Sampler,
    gather: wgpu::Sampler,
}

impl SunShadowsStandIn {
    /// The stand-in for `device`.
    pub fn new(device: &wgpu::Device) -> SunShadowsStandIn {
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("hd sun shadows stand-in"),
            size: crate::shadows::SHADOWS_BYTES as u64,
            usage: wgpu::BufferUsages::UNIFORM,
            mapped_at_creation: false,
        });
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("hd sun shadows stand-in"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: crate::shadows::SHADOW_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..wgpu::TextureViewDescriptor::default()
        });
        let compare = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("hd sun shadows stand-in compare"),
            compare: Some(wgpu::CompareFunction::LessEqual),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..wgpu::SamplerDescriptor::default()
        });
        let gather = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("hd sun shadows stand-in gather"),
            ..wgpu::SamplerDescriptor::default()
        });
        SunShadowsStandIn {
            uniform,
            view,
            compare,
            gather,
        }
    }

    /// The stand-in as the pass binds [`SunShadows`].
    pub fn shadows(&self) -> SunShadows<'_> {
        SunShadows {
            uniform: &self.uniform,
            map: &self.view,
            compare: &self.compare,
            gather: &self.gather,
        }
    }
}

/// What the scene's light is this frame: the sun, the fog, the lights and the effects' time (seconds).
#[derive(Clone, Copy)]
pub struct PassScene<'a> {
    pub sun: &'a HdSun,
    pub fog: &'a HdFog,
    pub lights: &'a [HdLight],
    pub time: f32,
    /// The farthest the camera sees.
    pub visible_distance: f32,
}

/// What a pass draws with at its stage. A target the stage does not have is None.
///
/// The targets are the frame's own and live for this call only: they are made again whenever the frame's size, quality or
/// features change (a resize), and the cascades' maps whenever the shadows' quality does. Never keep a view, or a bind group
/// made over one, from one frame to the next; make bind groups that name a target each frame (they are cheap) and keep only
/// what is the pass's own (pipelines, buffers, its own textures).
pub struct PassCx<'a> {
    pub stage: Stage,
    pub device: &'a wgpu::Device,
    pub queue: &'a wgpu::Queue,
    /// The frame's commands (a pass begins its own render passes in it).
    pub encoder: &'a mut wgpu::CommandEncoder,
    /// The size of the stage's targets ([`Stage::Overlay`]: the size of the world's rectangle of the picture).
    pub size: [u32; 2],
    /// The internal frame's size: the depth's, the normals' and the motion's.
    pub frame_size: [u32; 2],
    /// Where the world is in the color target: left, top, width, height (the whole target but at [`Stage::Overlay`]).
    pub viewport: [u32; 4],
    /// The frame's depth ([`DEPTH_FORMAT`], reversed).
    pub depth: Option<&'a wgpu::TextureView>,
    /// Whether [`PassCx::depth`] can be the depth attachment of a pass into the stage's color target (the same size).
    pub depth_attachable: bool,
    /// The view-space normals ([`NORMAL_FORMAT`]): a target at [`Stage::Prepass`], read only at [`Stage::Opaque`] and
    /// [`Stage::Translucent`].
    pub normals: Option<&'a wgpu::TextureView>,
    /// The motion ([`MOTION_FORMAT`]): a target at [`Stage::Prepass`].
    pub motion: Option<&'a wgpu::TextureView>,
    /// The HDR frame ([`HDR_FORMAT`]) from [`Stage::Opaque`] to [`Stage::Post`].
    pub hdr: Option<&'a wgpu::TextureView>,
    /// At [`Stage::Post`]: a copy of [`PassCx::hdr`] as it was before the stage's first pass (to read while writing the frame).
    pub source: Option<&'a wgpu::TextureView>,
    /// At [`Stage::Overlay`]: the finished picture and its format (draw within [`PassCx::viewport`]).
    pub ldr: Option<(&'a wgpu::TextureView, wgpu::TextureFormat)>,
    /// At [`Stage::Shadows`]: the sun's cascades this frame, finest first (none when the sun casts no shadows).
    pub cascades: &'a [ShadowCascade<'a>],
    /// At [`Stage::Opaque`] and [`Stage::Translucent`]: the sun's shadow maps to sample ([`SunShadows`]); None where the sun casts no
    /// shadows (and at the other stages).
    pub sun_shadows: Option<SunShadows<'a>>,
    /// The camera the stage draws with (its size is [`PassCx::size`]).
    pub camera: &'a HdCamera,
    pub scene: PassScene<'a>,
    /// The frame's moving objects (the ghosts' shapes), as the frame draws them.
    pub ghosts: &'a [DynamicObject],
}

/// A pass from outside the renderer: records its draws into the frame at its stage. A closure taking the [`PassCx`] is one.
pub trait HdPass: wgpu::WasmNotSend + 'static {
    /// Records the pass into `cx.encoder`.
    fn draw(&mut self, cx: &mut PassCx<'_>);
}

impl<F: FnMut(&mut PassCx<'_>) + wgpu::WasmNotSend + 'static> HdPass for F {
    fn draw(&mut self, cx: &mut PassCx<'_>) {
        self(cx)
    }
}

/// The passes of a [`crate::HdWorld`] by stage, each stage's in the order they were added.
#[derive(Default)]
pub struct HdPasses {
    stages: [Vec<Box<dyn HdPass>>; 7],
}

impl HdPasses {
    /// Adds `pass` at `stage`, after the ones there.
    pub fn add(&mut self, stage: Stage, pass: Box<dyn HdPass>) {
        self.stages[stage as usize].push(pass);
    }

    /// Takes every pass out.
    pub fn clear(&mut self) {
        self.stages.iter_mut().for_each(Vec::clear);
    }

    /// The passes at `stage`.
    pub fn count(&self, stage: Stage) -> usize {
        self.stages[stage as usize].len()
    }

    /// Whether there is a pass at `stage`.
    pub fn has(&self, stage: Stage) -> bool {
        !self.stages[stage as usize].is_empty()
    }

    /// Whether there is no pass at all.
    pub fn is_empty(&self) -> bool {
        self.stages.iter().all(Vec::is_empty)
    }

    /// Runs the passes at `cx.stage`, in order.
    pub(crate) fn run(&mut self, cx: &mut PassCx<'_>) {
        for pass in &mut self.stages[cx.stage as usize] {
            pass.draw(cx);
        }
    }
}

/// `source` copied into `copy` (made again at its size and format when it differs) with `encoder`: the copy's view.
pub(crate) fn frame_copy<'c>(
    device: &wgpu::Device,
    copy: &'c mut Option<(wgpu::Texture, wgpu::TextureView)>,
    encoder: &mut wgpu::CommandEncoder,
    source: &wgpu::Texture,
) -> &'c wgpu::TextureView {
    let size = source.size();
    if copy
        .as_ref()
        .is_none_or(|(t, _)| t.size() != size || t.format() != source.format())
    {
        let t = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("hd post source"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: source.format(),
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let v = t.create_view(&wgpu::TextureViewDescriptor::default());
        *copy = Some((t, v));
    }
    let (t, v) = copy.as_ref().expect("the copy was just made");
    encoder.copy_texture_to_texture(source.as_image_copy(), t.as_image_copy(), size);
    v
}

#[cfg(test)]
#[path = "passes_tests.rs"]
mod tests;
