//! The render pass that draws the ropes: at the opaque stage the tubes into the HDR frame, lit by the scene's sun and fogged, at
//! the shadows stage the tubes that cast a shadow into each of the sun's cascades, and at the translucent stage the ribbons and
//! lines (flat strips that face the camera, alpha blended over the frame's depth, so what is in front of them hides them).

use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::{Arc, Mutex, PoisonError};

use torque_app::render::{HdPass, PassCx, Stage};
use torque_render_core::wgpu;
use torque_render_hd::passes::{
    DEPTH_FORMAT, HDR_FORMAT, SUN_SHADOW_WGSL, SunShadows, SunShadowsStandIn,
};

use crate::image::ImageData;
use crate::mesh::{Draw, TubeMesh, VERTEX_BYTES};
use crate::sim::RopeWorld;
use crate::sim::TextureTable;
use crate::strip::{STRIP_VERTEX_BYTES, StripMesh, StripSet, StripView};
use crate::{DrawnShapes, FrameAnchors};
use torque_render_core::wgpu::util::DeviceExt as _;

/// A world the passes and the plugin's natives share.
pub type SharedWorld = Arc<Mutex<RopeWorld>>;

/// The tubes lit as the fixed-function frame lights a surface: the ambient plus the sun's diffuse by the cosine, in display values,
/// then fogged and converted to the HDR frame's linear light.
const COLOR_SHADER: &str = r#"
struct Globals {
    view_proj: mat4x4<f32>,
    sun_towards: vec4<f32>,
    sun_diffuse: vec4<f32>,
    sun_ambient: vec4<f32>,
    fog_color: vec4<f32>,
    // The fog's start and end, and 1 for TGE's linear fog (else the share squared).
    fog: vec4<f32>,
    eye: vec4<f32>,
}

@group(0) @binding(0) var<uniform> g: Globals;
@group(2) @binding(0) var tex: texture_2d<f32>;
@group(2) @binding(1) var tex_samp: sampler;

struct Vertex {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec4<f32>,
    // The place on the texture (u along the rope in repeats, v around it) and the texture's speed.
    @location(3) uv: vec3<f32>,
}

struct Varying {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec3<f32>,
    // 1 where the sun's shadows fall on the rope (the vertex color's alpha).
    @location(3) receive: f32,
    @location(4) uv: vec3<f32>,
}

@vertex
fn vs_tube(v: Vertex) -> Varying {
    var out: Varying;
    out.clip = g.view_proj * vec4<f32>(v.position, 1.0);
    out.world = v.position;
    out.normal = v.normal;
    out.color = v.color.rgb;
    out.receive = v.color.a;
    out.uv = v.uv;
    return out;
}

fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    let x = max(c, vec3<f32>(0.0));
    return select(pow((x + 0.055) / 1.055, vec3<f32>(2.4)), x / 12.92, x <= vec3<f32>(0.04045));
}

@fragment
fn fs_tube(in: Varying) -> @location(0) vec4<f32> {
    // The texture, run along by its speed; a tube cuts out what is under half alpha.
    let t = textureSample(tex, tex_samp, vec2<f32>(in.uv.x - fract(in.uv.z * g.eye.w), in.uv.y));
    if t.a < 0.5 {
        discard;
    }
    let n = normalize(in.normal);
    let nl = dot(n, g.sun_towards.xyz);
    let sun = max(nl, 0.0);
    // What of the sun's light the shadows let through, as the renderer's own surfaces take it.
    var through = vec3<f32>(1.0);
    if in.receive > 0.5 && nl > 0.0 {
        let vis = sun_visibility(in.world, n, nl, in.clip.xy, g.eye.xyz);
        through = vec3<f32>(vis) + (1.0 - vis) * sun_shadow_tint();
    }
    // The sky's light from above a little more than from below.
    let ambient = g.sun_ambient.rgb * (0.8 + 0.2 * n.z);
    var shade = clamp(in.color * t.rgb * (ambient + g.sun_diffuse.rgb * sun * through), vec3<f32>(0.0), vec3<f32>(1.0));
    let span = max(g.fog.y - g.fog.x, 1.0e-3);
    var share = clamp((distance(in.world, g.eye.xyz) - g.fog.x) / span, 0.0, 1.0);
    if g.fog.z < 0.5 {
        share = share * share;
    }
    shade = mix(shade, g.fog_color.rgb, share);
    return vec4<f32>(srgb_to_linear(shade), 1.0);
}
"#;

/// The strips: lit by the scene's light as a surface that faces up and a little sideways (they have no side to shade), or as they
/// are for a glowing one, fogged, and faded across the last pixel of each edge. Alpha blended.
const STRIP_SHADER: &str = r#"
struct Globals {
    view_proj: mat4x4<f32>,
    sun_towards: vec4<f32>,
    sun_diffuse: vec4<f32>,
    sun_ambient: vec4<f32>,
    fog_color: vec4<f32>,
    fog: vec4<f32>,
    eye: vec4<f32>,
}

@group(0) @binding(0) var<uniform> g: Globals;
@group(2) @binding(0) var tex: texture_2d<f32>;
@group(2) @binding(1) var tex_samp: sampler;

struct Vertex {
    @location(0) position: vec3<f32>,
    // The place across the strip (-1 to 1), half its width in pixels, the flags (1: glows, 2: takes shadows) and half its width in
    // the world.
    @location(1) edge: vec4<f32>,
    @location(2) color: vec4<f32>,
    // The place on the texture (u along the strip in repeats, v across it), the texture's speed and the glow's brightness.
    @location(3) uv: vec4<f32>,
}

struct Varying {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) edge: vec4<f32>,
    @location(2) color: vec4<f32>,
    @location(3) uv: vec4<f32>,
}

@vertex
fn vs_strip(v: Vertex) -> Varying {
    var out: Varying;
    out.clip = g.view_proj * vec4<f32>(v.position, 1.0);
    out.world = v.position;
    out.edge = v.edge;
    out.color = v.color;
    out.uv = v.uv;
    return out;
}

fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    let x = max(c, vec3<f32>(0.0));
    return select(pow((x + 0.055) / 1.055, vec3<f32>(2.4)), x / 12.92, x <= vec3<f32>(0.04045));
}

@fragment
fn fs_strip(in: Varying) -> @location(0) vec4<f32> {
    // The pixels from the strip's outer edge; the outermost one is the one the edge half covers.
    let cover = clamp((1.0 - abs(in.edge.x)) * in.edge.y, 0.0, 1.0);
    // The texture, run along by its speed; its alpha blends with the color's.
    let t = textureSample(tex, tex_samp, vec2<f32>(in.uv.x - fract(in.uv.z * g.eye.w), in.uv.y));
    var shade = in.color.rgb * t.rgb;
    let glow = abs(in.edge.z - 1.0) < 0.5 || abs(in.edge.z - 3.0) < 0.5;
    if !glow {
        var through = vec3<f32>(1.0);
        if in.edge.z > 1.5 {
            // The strip faces the camera and its shadow caster the sun: look up a little way toward the sun, off both.
            let sun = g.sun_towards.xyz;
            let vis = sun_visibility(in.world + sun * (in.edge.w * 1.5 + 0.02), sun, 1.0, in.clip.xy, g.eye.xyz);
            through = vec3<f32>(vis) + (1.0 - vis) * sun_shadow_tint();
        }
        shade = clamp(shade * (g.sun_ambient.rgb * 0.9 + g.sun_diffuse.rgb * 0.55 * through), vec3<f32>(0.0), vec3<f32>(1.0));
    }
    let span = max(g.fog.y - g.fog.x, 1.0e-3);
    var share = clamp((distance(in.world, g.eye.xyz) - g.fog.x) / span, 0.0, 1.0);
    if g.fog.z < 0.5 {
        share = share * share;
    }
    shade = mix(shade, g.fog_color.rgb, share);
    // A glowing strip is as bright as it says, less as the fog takes over.
    let boost = mix(in.uv.w, 1.0, share);
    return vec4<f32>(srgb_to_linear(shade) * boost, in.color.a * t.a * cover);
}
"#;

/// The tubes as the sun's cascades see them: depth only, a vertex in front of the light's near plane flattened onto it.
const SHADOW_SHADER: &str = r#"
struct Caster {
    view_proj: mat4x4<f32>,
    // The effects' time in x, to run a texture along.
    time: vec4<f32>,
}

@group(0) @binding(0) var<uniform> caster: Caster;
@group(1) @binding(0) var tex: texture_2d<f32>;
@group(1) @binding(1) var tex_samp: sampler;

struct Out {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn vs_caster(@location(0) position: vec3<f32>, @location(3) uv: vec3<f32>) -> Out {
    var out: Out;
    var c = caster.view_proj * vec4<f32>(position, 1.0);
    c.z = max(c.z, 0.0);
    out.clip = c;
    out.uv = vec2<f32>(uv.x - fract(uv.z * caster.time.x), uv.y);
    return out;
}

// What the texture cuts out casts no shadow.
@fragment
fn fs_caster(in: Out) {
    if textureSample(tex, tex_samp, in.uv).a < 0.5 {
        discard;
    }
}
"#;

/// Bytes of the color pass's uniform: a matrix and six vectors.
const GLOBALS_BYTES: usize = 64 + 6 * 16;

fn push(out: &mut Vec<u8>, values: &[f32]) {
    for v in values {
        out.extend_from_slice(&v.to_le_bytes());
    }
}

/// The vertex and index buffers of a mesh, grown as it grows.
struct Buffers {
    vertex: wgpu::Buffer,
    index: wgpu::Buffer,
    vertex_capacity: u64,
    index_capacity: u64,
    /// The key of what the buffers hold.
    revision: Option<u64>,
    /// The indices the buffers hold, and the runs of them by texture.
    index_count: u32,
    draws: Vec<Draw>,
}

impl Buffers {
    fn new(device: &wgpu::Device) -> Buffers {
        let make = |label, usage, size| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage: usage | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        Buffers {
            vertex: make("rope vertices", wgpu::BufferUsages::VERTEX, 4096),
            index: make("rope indices", wgpu::BufferUsages::INDEX, 4096),
            vertex_capacity: 4096,
            index_capacity: 4096,
            revision: None,
            index_count: 0,
            draws: Vec::new(),
        }
    }

    /// Holds the bytes `make` gives (vertices, indices), written again only when `key` is not the one they were made for.
    fn upload(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        key: u64,
        make: impl FnOnce() -> (Vec<u8>, Vec<u8>, Vec<Draw>),
    ) {
        if self.revision == Some(key) {
            return;
        }
        let (vertices, indices, draws) = make();
        let grow = |buffer: &mut wgpu::Buffer, capacity: &mut u64, need: u64, label, usage| {
            if need > *capacity {
                *capacity = need.next_power_of_two();
                *buffer = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some(label),
                    size: *capacity,
                    usage: usage | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
            }
        };
        grow(
            &mut self.vertex,
            &mut self.vertex_capacity,
            vertices.len() as u64,
            "rope vertices",
            wgpu::BufferUsages::VERTEX,
        );
        grow(
            &mut self.index,
            &mut self.index_capacity,
            indices.len() as u64,
            "rope indices",
            wgpu::BufferUsages::INDEX,
        );
        queue.write_buffer(&self.vertex, 0, &vertices);
        queue.write_buffer(&self.index, 0, &indices);
        self.revision = Some(key);
        self.index_count = (indices.len() / 4) as u32;
        self.draws = draws;
    }
}

/// The textures the ropes are drawn with, on the GPU: a bind group each (group 2 of the color passes, group 1 of the shadow
/// casters), and a white texel for a rope with none.
struct Textures {
    revision: Option<u64>,
    layout: wgpu::BindGroupLayout,
    white: wgpu::BindGroup,
    groups: std::collections::HashMap<u32, wgpu::BindGroup>,
    linear: wgpu::Sampler,
    nearest: wgpu::Sampler,
}

impl Textures {
    fn new(device: &wgpu::Device, queue: &wgpu::Queue, layout: wgpu::BindGroupLayout) -> Textures {
        let sampler = |label, filter: wgpu::FilterMode, mipmap: wgpu::MipmapFilterMode| {
            device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some(label),
                address_mode_u: wgpu::AddressMode::Repeat,
                address_mode_v: wgpu::AddressMode::ClampToEdge,
                mag_filter: filter,
                min_filter: filter,
                mipmap_filter: mipmap,
                ..wgpu::SamplerDescriptor::default()
            })
        };
        let linear = sampler(
            "rope texture linear",
            wgpu::FilterMode::Linear,
            wgpu::MipmapFilterMode::Linear,
        );
        let nearest = sampler(
            "rope texture nearest",
            wgpu::FilterMode::Nearest,
            wgpu::MipmapFilterMode::Nearest,
        );
        let white = Textures::group_of(device, queue, &layout, &ImageData::white(), &linear);
        Textures {
            revision: None,
            layout,
            white,
            groups: std::collections::HashMap::new(),
            linear,
            nearest,
        }
    }

    fn group_of(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        layout: &wgpu::BindGroupLayout,
        image: &ImageData,
        sampler: &wgpu::Sampler,
    ) -> wgpu::BindGroup {
        let texture = device.create_texture_with_data(
            queue,
            &wgpu::TextureDescriptor {
                label: Some("rope texture"),
                size: wgpu::Extent3d {
                    width: image.width,
                    height: image.height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: image.mip_levels(),
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            &image.levels.concat(),
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("rope texture"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
            ],
        })
    }

    /// Holds the world's textures (uploaded again only when the world's table changed).
    fn sync(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, table: &TextureTable) {
        if self.revision == Some(table.revision) {
            return;
        }
        self.groups.clear();
        for (i, entry) in table.entries.iter().enumerate() {
            let sampler = if entry.nearest {
                &self.nearest
            } else {
                &self.linear
            };
            let group = Textures::group_of(device, queue, &self.layout, &entry.image, sampler);
            self.groups.insert(i as u32 + 1, group);
        }
        self.revision = Some(table.revision);
    }

    /// Texture `id`'s group (the white one for 0, and for one the world does not hold).
    fn group(&self, id: u32) -> &wgpu::BindGroup {
        self.groups.get(&id).unwrap_or(&self.white)
    }
}

/// A render pass into the HDR frame over the frame's depth, both kept.
fn begin_color_pass<'e>(
    encoder: &'e mut wgpu::CommandEncoder,
    hdr: &wgpu::TextureView,
    depth: &wgpu::TextureView,
    label: &'static str,
) -> wgpu::RenderPass<'e> {
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: hdr,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Load,
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
            view: depth,
            depth_ops: Some(wgpu::Operations {
                load: wgpu::LoadOp::Load,
                store: wgpu::StoreOp::Store,
            }),
            stencil_ops: None,
        }),
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    })
}

/// A pipeline and what it was made for.
struct Pipeline {
    format: wgpu::TextureFormat,
    pipeline: wgpu::RenderPipeline,
}

/// The color pipelines' layouts: the frame's uniform (group 0) and the sun's shadows (group 1).
#[derive(Clone)]
struct Layouts {
    globals: wgpu::BindGroupLayout,
    sun: wgpu::BindGroupLayout,
    tex: wgpu::BindGroupLayout,
    caster: wgpu::BindGroupLayout,
    pipeline: wgpu::PipelineLayout,
    shadow: wgpu::PipelineLayout,
}

/// Draws the ropes of a [`SharedWorld`] at the opaque and shadows stages (add it at both). Each view makes its own.
pub struct RopePass {
    world: SharedWorld,
    anchors: Arc<dyn FrameAnchors>,
    buffers: Option<Buffers>,
    color: Option<Pipeline>,
    shadow: Option<Pipeline>,
    strip: Option<Pipeline>,
    strip_additive: Option<Pipeline>,
    strip_shadow: Option<Pipeline>,
    /// The strips' shadow casters (the shadows stage's own: its tubes are in `buffers`).
    shadow_buffers: Option<Buffers>,
    layouts: Option<Layouts>,
    textures: Option<Textures>,
    stand_in: Option<SunShadowsStandIn>,
    globals: Option<wgpu::Buffer>,
    /// One uniform for each cascade drawn in a frame (they are all read when the frame's commands run).
    casters: Vec<wgpu::Buffer>,
}

impl RopePass {
    /// A pass over `world`.
    pub fn new(world: SharedWorld) -> RopePass {
        RopePass::with_anchors(world, Arc::new(DrawnShapes))
    }

    /// A pass over `world` whose ropes follow ghosts to where `anchors` places them in each frame.
    pub fn with_anchors(world: SharedWorld, anchors: Arc<dyn FrameAnchors>) -> RopePass {
        RopePass {
            world,
            anchors,
            buffers: None,
            color: None,
            shadow: None,
            strip: None,
            strip_additive: None,
            strip_shadow: None,
            shadow_buffers: None,
            layouts: None,
            textures: None,
            stand_in: None,
            globals: None,
            casters: Vec::new(),
        }
    }

    fn layouts(&mut self, device: &wgpu::Device) -> Layouts {
        self.layouts
            .get_or_insert_with(|| {
                let globals = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("rope globals"),
                    entries: &[wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    }],
                });
                let sun = SunShadows::bind_group_layout(device);
                let tex = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("rope texture"),
                    entries: &[
                        wgpu::BindGroupLayoutEntry {
                            binding: 0,
                            visibility: wgpu::ShaderStages::FRAGMENT,
                            ty: wgpu::BindingType::Texture {
                                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                                view_dimension: wgpu::TextureViewDimension::D2,
                                multisampled: false,
                            },
                            count: None,
                        },
                        wgpu::BindGroupLayoutEntry {
                            binding: 1,
                            visibility: wgpu::ShaderStages::FRAGMENT,
                            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                            count: None,
                        },
                    ],
                });
                let caster = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("rope caster"),
                    entries: &[wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    }],
                });
                let pipeline = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some("rope color"),
                    bind_group_layouts: &[Some(&globals), Some(&sun), Some(&tex)],
                    immediate_size: 0,
                });
                let shadow = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some("rope shadows"),
                    bind_group_layouts: &[Some(&caster), Some(&tex)],
                    immediate_size: 0,
                });
                Layouts {
                    globals,
                    sun,
                    tex,
                    caster,
                    pipeline,
                    shadow,
                }
            })
            .clone()
    }

    /// The group the sun's shadows are read through: this frame's, or the stand-in's when the sun casts none.
    fn sun_group(&mut self, cx: &PassCx<'_>, layout: &wgpu::BindGroupLayout) -> wgpu::BindGroup {
        let device = cx.device;
        let shadows = cx.sun_shadows.unwrap_or_else(|| {
            self.stand_in
                .get_or_insert_with(|| SunShadowsStandIn::new(device))
                .shadows()
        });
        shadows.bind_group(device, layout)
    }

    /// Makes the world's textures ready on the GPU.
    fn sync_textures(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        let table = self
            .world
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .textures();
        let layout = self.layouts(device).tex;
        self.textures
            .get_or_insert_with(|| Textures::new(device, queue, layout))
            .sync(device, queue, &table);
    }

    /// Runs the ropes to this frame: the ends that follow a ghost go where the frame draws it.
    fn step(&self, cx: &PassCx<'_>) {
        let (ghosts, anchors) = (cx.ghosts, &self.anchors);
        self.world
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .step_frame(cx.scene.time, |ghost, node| {
                ghosts
                    .iter()
                    .find(|o| o.ghost == ghost)
                    .and_then(|o| anchors.mount(o, node))
                    .or_else(|| anchors.ghost_mount(ghost, node))
            });
    }

    fn mesh(&self) -> Arc<TubeMesh> {
        self.world
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .mesh()
    }

    fn strips(&self) -> Arc<StripSet> {
        self.world
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .strips()
    }

    fn strip_layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: STRIP_VERTEX_BYTES as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &[
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x3,
                    offset: 0,
                    shader_location: 0,
                },
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x4,
                    offset: 12,
                    shader_location: 1,
                },
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Unorm8x4,
                    offset: 28,
                    shader_location: 2,
                },
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x4,
                    offset: 32,
                    shader_location: 3,
                },
            ],
        }
    }

    fn strip_pipeline(&mut self, device: &wgpu::Device, additive: bool) -> wgpu::RenderPipeline {
        if additive && self.strip_additive.is_none() || !additive && self.strip.is_none() {
            let layouts = self.layouts(device);
            let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("rope strips"),
                source: wgpu::ShaderSource::Wgsl(
                    format!("{STRIP_SHADER}\n{SUN_SHADOW_WGSL}").into(),
                ),
            });
            let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("rope strips"),
                layout: Some(&layouts.pipeline),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vs_strip"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    buffers: &[Some(RopePass::strip_layout())],
                },
                primitive: wgpu::PrimitiveState::default(),
                // Tested against the frame's depth, never written: a strip is see-through at its edges and a later one may
                // overlap it.
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: Some(false),
                    depth_compare: Some(wgpu::CompareFunction::GreaterEqual),
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState::default(),
                }),
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some("fs_strip"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: HDR_FORMAT,
                        // An energy beam adds its light (by its alpha) to what is behind it.
                        blend: Some(if additive {
                            wgpu::BlendState {
                                color: wgpu::BlendComponent {
                                    src_factor: wgpu::BlendFactor::SrcAlpha,
                                    dst_factor: wgpu::BlendFactor::One,
                                    operation: wgpu::BlendOperation::Add,
                                },
                                alpha: wgpu::BlendComponent::OVER,
                            }
                        } else {
                            wgpu::BlendState::ALPHA_BLENDING
                        }),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            });
            let made = Pipeline {
                format: HDR_FORMAT,
                pipeline,
            };
            if additive {
                self.strip_additive = Some(made);
            } else {
                self.strip = Some(made);
            }
        }
        let slot = if additive {
            &self.strip_additive
        } else {
            &self.strip
        };
        slot.as_ref()
            .expect("the pipeline was just made")
            .pipeline
            .clone()
    }

    fn vertex_layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: VERTEX_BYTES as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &[
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x3,
                    offset: 0,
                    shader_location: 0,
                },
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x3,
                    offset: 12,
                    shader_location: 1,
                },
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Unorm8x4,
                    offset: 24,
                    shader_location: 2,
                },
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x3,
                    offset: 28,
                    shader_location: 3,
                },
            ],
        }
    }

    fn color_pipeline(&mut self, device: &wgpu::Device) -> &wgpu::RenderPipeline {
        if self.color.is_none() {
            let layouts = self.layouts(device);
            let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("rope tubes"),
                source: wgpu::ShaderSource::Wgsl(
                    format!("{COLOR_SHADER}\n{SUN_SHADOW_WGSL}").into(),
                ),
            });
            let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("rope tubes"),
                layout: Some(&layouts.pipeline),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vs_tube"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    buffers: &[Some(RopePass::vertex_layout())],
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: Some(true),
                    depth_compare: Some(wgpu::CompareFunction::GreaterEqual),
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState::default(),
                }),
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some("fs_tube"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    targets: &[Some(HDR_FORMAT.into())],
                }),
                multiview_mask: None,
                cache: None,
            });
            self.color = Some(Pipeline {
                format: HDR_FORMAT,
                pipeline,
            });
        }
        &self
            .color
            .as_ref()
            .expect("the pipeline was just made")
            .pipeline
    }

    fn shadow_pipeline(
        &mut self,
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
    ) -> &wgpu::RenderPipeline {
        if self.shadow.as_ref().is_none_or(|p| p.format != format) {
            let layouts = self.layouts(device);
            let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("rope shadows"),
                source: wgpu::ShaderSource::Wgsl(SHADOW_SHADER.into()),
            });
            let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("rope shadows"),
                layout: Some(&layouts.shadow),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vs_caster"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    buffers: &[Some(RopePass::vertex_layout())],
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: Some(wgpu::DepthStencilState {
                    format,
                    depth_write_enabled: Some(true),
                    depth_compare: Some(wgpu::CompareFunction::LessEqual),
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState::default(),
                }),
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some("fs_caster"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    targets: &[],
                }),
                multiview_mask: None,
                cache: None,
            });
            self.shadow = Some(Pipeline { format, pipeline });
        }
        &self
            .shadow
            .as_ref()
            .expect("the pipeline was just made")
            .pipeline
    }

    fn strip_shadow_pipeline(
        &mut self,
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
    ) -> &wgpu::RenderPipeline {
        if self
            .strip_shadow
            .as_ref()
            .is_none_or(|p| p.format != format)
        {
            let layouts = self.layouts(device);
            let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("rope strip shadows"),
                source: wgpu::ShaderSource::Wgsl(SHADOW_SHADER.into()),
            });
            let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("rope strip shadows"),
                layout: Some(&layouts.shadow),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vs_caster"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    buffers: &[Some(RopePass::strip_layout())],
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: Some(wgpu::DepthStencilState {
                    format,
                    depth_write_enabled: Some(true),
                    depth_compare: Some(wgpu::CompareFunction::LessEqual),
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState::default(),
                }),
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some("fs_caster"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    targets: &[],
                }),
                multiview_mask: None,
                cache: None,
            });
            self.strip_shadow = Some(Pipeline { format, pipeline });
        }
        &self
            .strip_shadow
            .as_ref()
            .expect("the pipeline was just made")
            .pipeline
    }

    /// Writes the frame's light, fog and camera into the uniform (made the first time) and returns it.
    fn write_globals(&mut self, cx: &PassCx<'_>) -> wgpu::Buffer {
        let mut bytes = Vec::with_capacity(GLOBALS_BYTES);
        let sun = cx.scene.sun;
        let fog = cx.scene.fog;
        push(&mut bytes, &cx.camera.view_proj);
        push(
            &mut bytes,
            &[sun.towards[0], sun.towards[1], sun.towards[2], 0.0],
        );
        push(
            &mut bytes,
            &[sun.diffuse[0], sun.diffuse[1], sun.diffuse[2], 0.0],
        );
        push(
            &mut bytes,
            &[sun.ambient[0], sun.ambient[1], sun.ambient[2], 0.0],
        );
        push(&mut bytes, &[fog.color[0], fog.color[1], fog.color[2], 0.0]);
        push(
            &mut bytes,
            &[fog.start, fog.end, f32::from(u8::from(fog.linear)), 0.0],
        );
        push(
            &mut bytes,
            &[
                cx.camera.eye[0],
                cx.camera.eye[1],
                cx.camera.eye[2],
                cx.scene.time,
            ],
        );
        let device = cx.device;
        let globals = self.globals.get_or_insert_with(|| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("rope globals"),
                size: GLOBALS_BYTES as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        });
        cx.queue.write_buffer(globals, 0, &bytes);
        globals.clone()
    }

    /// The ribbons and lines into the HDR frame over the frame's depth, blended.
    fn draw_strips(&mut self, cx: &mut PassCx<'_>) {
        let (Some(hdr), Some(depth)) = (cx.hdr, cx.depth) else {
            return;
        };
        if !cx.depth_attachable {
            return;
        }
        let set = self.strips();
        if set.strips.is_empty() {
            return;
        }
        let camera = cx.camera;
        let view = StripView::from_matrices(camera.eye, &camera.view, &camera.proj, camera.height);
        // What the strips are made for: the ropes' state and the camera.
        let mut hasher = DefaultHasher::new();
        set.revision.hash(&mut hasher);
        for v in camera.view.iter().chain(&camera.proj).chain(&camera.eye) {
            v.to_bits().hash(&mut hasher);
        }
        camera.height.hash(&mut hasher);
        let key = hasher.finish();
        let (device, queue) = (cx.device, cx.queue);
        self.buffers
            .get_or_insert_with(|| Buffers::new(device))
            .upload(device, queue, key, || {
                let mesh = StripMesh::build(&set, &view);
                (mesh.vertex_bytes(), mesh.index_bytes(), mesh.draws)
            });
        self.sync_textures(device, queue);
        let globals = self.write_globals(cx);
        let normal = self.strip_pipeline(device, false);
        let additive = self.strip_pipeline(device, true);
        let layouts = self.layouts(device);
        let sun_group = self.sun_group(cx, &layouts.sun);
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("rope strip globals"),
            layout: &layouts.globals,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: globals.as_entire_binding(),
            }],
        });
        let buffers = self.buffers.as_ref().expect("the buffers were just made");
        let mut pass = begin_color_pass(cx.encoder, hdr, depth, "rope strips");
        let [x, y, w, h] = cx.viewport;
        pass.set_viewport(x as f32, y as f32, w as f32, h as f32, 0.0, 1.0);
        pass.set_bind_group(0, &group, &[]);
        pass.set_bind_group(1, &sun_group, &[]);
        pass.set_vertex_buffer(0, buffers.vertex.slice(..));
        pass.set_index_buffer(buffers.index.slice(..), wgpu::IndexFormat::Uint32);
        let textures = self.textures.as_ref().expect("the textures were just made");
        let mut current = None;
        for d in &buffers.draws {
            if current != Some(d.additive) {
                pass.set_pipeline(if d.additive { &additive } else { &normal });
                current = Some(d.additive);
            }
            pass.set_bind_group(2, textures.group(d.texture), &[]);
            pass.draw_indexed(d.first..d.first + d.count, 0, 0..1);
        }
    }

    /// The tubes into the HDR frame over the frame's depth.
    fn draw_color(&mut self, cx: &mut PassCx<'_>) {
        let (Some(hdr), Some(depth)) = (cx.hdr, cx.depth) else {
            return;
        };
        if !cx.depth_attachable {
            return;
        }
        let mesh = self.mesh();
        if mesh.indices.is_empty() {
            return;
        }
        let (device, queue) = (cx.device, cx.queue);
        self.buffers
            .get_or_insert_with(|| Buffers::new(device))
            .upload(device, queue, mesh.revision, || {
                (mesh.vertex_bytes(), mesh.index_bytes(), mesh.draws.clone())
            });
        self.sync_textures(device, queue);
        let globals = self.write_globals(cx);
        let pipeline = self.color_pipeline(device).clone();
        let layouts = self.layouts(device);
        let sun_group = self.sun_group(cx, &layouts.sun);
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("rope globals"),
            layout: &layouts.globals,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: globals.as_entire_binding(),
            }],
        });
        let buffers = self.buffers.as_ref().expect("the buffers were just made");
        let mut pass = begin_color_pass(cx.encoder, hdr, depth, "rope tubes");
        let [x, y, w, h] = cx.viewport;
        pass.set_viewport(x as f32, y as f32, w as f32, h as f32, 0.0, 1.0);
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &group, &[]);
        pass.set_bind_group(1, &sun_group, &[]);
        pass.set_vertex_buffer(0, buffers.vertex.slice(..));
        pass.set_index_buffer(buffers.index.slice(..), wgpu::IndexFormat::Uint32);
        let textures = self.textures.as_ref().expect("the textures were just made");
        for d in &buffers.draws {
            pass.set_bind_group(2, textures.group(d.texture), &[]);
            pass.draw_indexed(d.first..d.first + d.count, 0, 0..1);
        }
    }

    /// The tubes that cast a shadow into each of the sun's cascades.
    fn draw_shadows(&mut self, cx: &mut PassCx<'_>) {
        if cx.cascades.is_empty() {
            return;
        }
        let mesh = self.mesh();
        let set = self.strips();
        let sun = glam::Vec3::from(cx.scene.sun.towards);
        let strips_cast = set.strips.iter().any(|s| s.cast_shadow);
        if mesh.shadow_indices == 0 && !strips_cast {
            return;
        }
        let (device, queue) = (cx.device, cx.queue);
        if mesh.shadow_indices > 0 {
            self.buffers
                .get_or_insert_with(|| Buffers::new(device))
                .upload(device, queue, mesh.revision, || {
                    (mesh.vertex_bytes(), mesh.index_bytes(), mesh.draws.clone())
                });
        }
        if strips_cast {
            // The casters face the sun: made again when the ropes or the sun move.
            let mut hasher = DefaultHasher::new();
            set.revision.hash(&mut hasher);
            for c in sun.to_array() {
                c.to_bits().hash(&mut hasher);
            }
            self.shadow_buffers
                .get_or_insert_with(|| Buffers::new(device))
                .upload(device, queue, hasher.finish(), || {
                    let mesh = StripMesh::build_shadow(&set, sun);
                    (mesh.vertex_bytes(), mesh.index_bytes(), mesh.draws)
                });
        }
        self.sync_textures(device, queue);
        let layouts = self.layouts(device);
        let format = cx.cascades[0].target.texture().format();
        let pipeline = self.shadow_pipeline(device, format).clone();
        let strip_pipeline = self.strip_shadow_pipeline(device, format).clone();
        while self.casters.len() < cx.cascades.len() {
            self.casters
                .push(device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("rope caster"),
                    size: 80,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }));
        }
        let buffers = self.buffers.as_ref();
        let strip_buffers = self.shadow_buffers.as_ref();
        let textures = self.textures.as_ref().expect("the textures were just made");
        for (cascade, caster) in cx.cascades.iter().zip(&self.casters) {
            let mut bytes = Vec::with_capacity(80);
            push(&mut bytes, &cascade.view_proj);
            push(&mut bytes, &[cx.scene.time, 0.0, 0.0, 0.0]);
            queue.write_buffer(caster, 0, &bytes);
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("rope caster"),
                layout: &layouts.caster,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: caster.as_entire_binding(),
                }],
            });
            let mut pass = cx.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("rope shadows"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: cascade.target,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            if let (true, Some(buffers)) = (mesh.shadow_indices > 0, buffers) {
                pass.set_pipeline(&pipeline);
                pass.set_bind_group(0, &group, &[]);
                pass.set_vertex_buffer(0, buffers.vertex.slice(..));
                pass.set_index_buffer(buffers.index.slice(..), wgpu::IndexFormat::Uint32);
                for d in buffers.draws.iter().filter(|d| d.casting) {
                    pass.set_bind_group(1, textures.group(d.texture), &[]);
                    pass.draw_indexed(d.first..d.first + d.count, 0, 0..1);
                }
            }
            if let (true, Some(buffers)) = (strips_cast, strip_buffers) {
                pass.set_pipeline(&strip_pipeline);
                pass.set_bind_group(0, &group, &[]);
                pass.set_vertex_buffer(0, buffers.vertex.slice(..));
                pass.set_index_buffer(buffers.index.slice(..), wgpu::IndexFormat::Uint32);
                for d in &buffers.draws {
                    pass.set_bind_group(1, textures.group(d.texture), &[]);
                    pass.draw_indexed(d.first..d.first + d.count, 0, 0..1);
                }
            }
        }
    }
}

impl HdPass for RopePass {
    fn draw(&mut self, cx: &mut PassCx<'_>) {
        if matches!(
            cx.stage,
            Stage::Shadows | Stage::Opaque | Stage::Translucent
        ) {
            self.step(cx);
        }
        match cx.stage {
            Stage::Opaque => self.draw_color(cx),
            Stage::Shadows => self.draw_shadows(cx),
            Stage::Translucent => self.draw_strips(cx),
            _ => {}
        }
    }
}
