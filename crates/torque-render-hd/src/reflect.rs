//! The enhanced renderer's reflections (the ladder and the cameras are `torque_render_scene::hd::reflect`): what a frame draws besides the main view, and the forward passes' group 3.
//!
//! **A frame's reflections** ([`HdWorld::plan_reflections`], then the encoders in their places in [`HdWorld::draw`]):
//! 1. The secondary views, before the main view: the sky cube's faces when the scene's draws changed, the light capture's face (all six
//!    when it starts again) when a reflective surface ([`crate::ext::HdExtension::reflective`]) is in sight at High and up, the planar reflection when the scene has water, the
//!    quality reflects it in a plane and the eye is above it. Each is the opaque scene (sky, opaque cells, ground, opaque shapes)
//!    through the forward pass's secondary-view module: every light in turn, no contact shadows, AO or SSR, front faces turned (a
//!    mirrored or cube camera is left-handed), the planar one cut at the water.
//! 2. After the depth prepass: the Hi-Z pyramid, the screen-space reflections' trace and accumulation ([`crate::ssr`]) when a
//!    reflective surface is in sight at High and up, or the water is traced at Medium.
//! 3. The forward passes read group 3: this view's `Refl` (common.wgsl) uniform, the SSR result, the capture, the sky
//!    cube, the extension's slot, the planar reflection, the depth's copy (the water's path) and the clouds' cube (a face of it
//!    marched after the clouds' march when the water is drawn: [`crate::clouds`]).
//! 4. Before the translucent pass the depth is copied (the water reads it); the water plane is drawn by `fs_water`.
//! 5. After the anti-aliasing the frame (the TAA's, else the translucent pass's) is copied for the next frame's rays, with its mip
//!    chain.
//!
//! The histories (the colour copy, the SSR accumulation, the capture) start again with the TAA's ([`HdWorld::reset_history`]) and when
//! a frame does not keep them.

use std::collections::HashMap;

use glam::Vec3;
use torque_render_scene::hd::reflect::QualityReflections;

use crate::ext::{HdExtension, Surfaces};
use torque_render_core::wgpu;
use torque_render_scene::hd::reflect::{
    CAPTURE_MIPS, CAPTURE_NEAR, SsrResolution, WaterReflection, cube_face_camera, mirror_camera,
    water_level,
};
use torque_render_scene::hd::{HdCamera, HdMode};
use torque_render_scene::{Blend, Cull};

use super::{DebugView, DrawRef, HdWorld, PipelineKey, SamplerKey, TexRef};
use crate::Stage;
use crate::capture::{CUBE_FORMAT, DownPipe, LightCapture, PrefilterPipe};
use crate::shaders::Permutation;
use crate::ssr::{SsrInputs, SsrPipes, SsrTargets};
use crate::water::Planar;

/// The reflections' uniform's bytes (`common.wgsl`'s `Refl`).
const REFL_BYTES: usize = 176;
/// A shape whose bounding sphere comes this near the capture's eye (units) is left out of the capture's faces: the client's own
/// Player and held item in first person, drawn from inside them. A camera-local capture shows what is near its eye hugely and in
/// the wrong place for every surface away from the eye, and those shapes swing and bob with every step and jump.
pub const CAPTURE_SHAPE_CLEAR: f32 = 1.0;
/// The uniforms: the main view's, the planar view's, the capture faces', the sky faces'.
const U_MAIN: usize = 0;
const U_PLANAR: usize = 1;
const U_CAPTURE: usize = 2;
const U_SKY: usize = 3;

/// A draw of a secondary view: what, through which pipeline (of the secondary-view module) and texture.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ViewDraw {
    pub source: Source,
    pub key: PipelineKey,
    pub group: (TexRef, SamplerKey),
}

// A secondary view holds a handful of these draws: the view's planes ride in its first.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, Copy)]
pub(crate) enum Source {
    Loose(DrawRef),
    /// A draw group's clusters seen by the view.
    Group(crate::arena::GroupKey),
    /// The view the groups are culled for (the first of its draws): its cull slot when the GPU culls them into the scratch
    /// lists, and its planes (the CPU's test otherwise).
    View(Option<u32>, crate::cull::ViewPlanes),
    /// The physical sky (a full-frame pass) in the sky's place.
    PhysicalSky,
}

/// What this frame's reflections draw.
#[derive(Default)]
pub(crate) struct ReflectPlan {
    /// The Hi-Z, the trace and the accumulation; then the colour copy after the translucent pass.
    pub ssr: bool,
    /// The water is drawn by `fs_water` (and the depth copied for it).
    pub water: bool,
    pub sky_faces: Vec<(usize, Vec<ViewDraw>)>,
    pub capture_faces: Vec<(usize, Vec<ViewDraw>)>,
    /// The capture (a cube of every direction from the eye) is drawn this frame.
    pub capture: bool,
    pub planar: Option<Vec<ViewDraw>>,
    /// The scene's clear colour (linear).
    pub clear: [f64; 3],
}

impl ReflectPlan {
    fn has_views(&self) -> bool {
        !self.sky_faces.is_empty() || !self.capture_faces.is_empty() || self.planar.is_some()
    }
}

/// What the reflections' targets are made for: the internal size, the SSR scale, the planar reflection's width (0: none) and the
/// colour copy's size; Whether the SSR's targets are made, whether the scene has water (its depth copy).
type MadeFor = (u32, u32, u32, u32, (u32, u32), bool, bool);

/// The reflections' GPU state.
pub(crate) struct Reflections {
    pub layout: wgpu::BindGroupLayout,
    pub sampler: wgpu::Sampler,
    dummy_2d: wgpu::TextureView,
    dummy_cube: wgpu::TextureView,
    dummy_depth: wgpu::TextureView,
    uniforms: Vec<wgpu::Buffer>,
    /// The extension's buffer in group 3's binding 5 ([`crate::ExtSlots::reflect`]; the groups are made again when it changes).
    ext: wgpu::Buffer,
    pub capture: LightCapture,
    pub down: DownPipe,
    /// The capture's and the sky cube's mips.
    pub prefilter: PrefilterPipe,
    ssr_pipes: SsrPipes,
    pub ssr: Option<SsrTargets>,
    pub planar: Option<Planar>,
    refract_depth: Option<(wgpu::Texture, wgpu::TextureView)>,
    view_module: wgpu::ShaderModule,
    water_module: wgpu::ShaderModule,
    pub view_pipelines: HashMap<PipelineKey, wgpu::RenderPipeline>,
    pub water_pipelines: HashMap<Cull, wgpu::RenderPipeline>,
    main_groups: Option<[wgpu::BindGroup; 2]>,
    view_groups: Option<[wgpu::BindGroup; 3]>,
    /// The internal size and the SSR scale the targets were made for.
    made_for: Option<MadeFor>,
    /// Whether last frame left the colour copy and the SSR history (the next frame's rays read them).
    pub history_valid: bool,
    /// What this frame planned (the encoders read it).
    pub plan: ReflectPlan,
}

fn plain_texture(
    device: &wgpu::Device,
    label: &str,
    format: wgpu::TextureFormat,
    layers: u32,
    cube: bool,
) -> wgpu::TextureView {
    let usage = if format == wgpu::TextureFormat::Depth32Float {
        wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::RENDER_ATTACHMENT
    } else {
        wgpu::TextureUsages::TEXTURE_BINDING
    };
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: layers,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage,
            view_formats: &[],
        })
        .create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(if cube {
                wgpu::TextureViewDimension::Cube
            } else {
                wgpu::TextureViewDimension::D2
            }),
            ..wgpu::TextureViewDescriptor::default()
        })
}

fn tex_entry(
    binding: u32,
    view_dimension: wgpu::TextureViewDimension,
    sample_type: wgpu::TextureSampleType,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type,
            view_dimension,
            multisampled: false,
        },
        count: None,
    }
}

/// A cull mode as a left-handed camera sees it.
fn turned(c: Cull) -> Cull {
    match c {
        Cull::None => Cull::None,
        Cull::BackCw => Cull::BackCcw,
        Cull::BackCcw => Cull::BackCw,
    }
}

/// The reflections group's bindings (group 3 of the forward pipelines).
pub(crate) fn layout_entries() -> Vec<wgpu::BindGroupLayoutEntry> {
    use wgpu::TextureSampleType as S;
    use wgpu::TextureViewDimension as D;
    let float = S::Float { filterable: true };
    vec![
        crate::uniform(0, wgpu::ShaderStages::FRAGMENT),
        tex_entry(1, D::D2, float),
        tex_entry(2, D::Cube, float),
        tex_entry(3, D::Cube, float),
        wgpu::BindGroupLayoutEntry {
            binding: 4,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        },
        crate::storage(5, true, wgpu::ShaderStages::FRAGMENT),
        tex_entry(6, D::D2, float),
        tex_entry(7, D::D2, S::Depth),
        tex_entry(8, D::Cube, float),
    ]
}

impl Reflections {
    pub fn new(
        device: &wgpu::Device,
        cache: Option<&wgpu::PipelineCache>,
        frame_bytes: u64,
    ) -> Reflections {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("hd reflections"),
            entries: &layout_entries(),
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("hd reflections"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..wgpu::SamplerDescriptor::default()
        });
        let uniforms = (0..4)
            .map(|_| {
                device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("hd reflections"),
                    size: REFL_BYTES as u64,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                })
            })
            .collect();
        let down = DownPipe::new(device, cache);
        let prefilter = PrefilterPipe::new(device, cache);
        let module = |p: Permutation| {
            device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(&p.name()),
                source: wgpu::ShaderSource::Wgsl(p.source().into()),
            })
        };
        Reflections {
            layout,
            sampler,
            dummy_2d: plain_texture(device, "hd dummy", CUBE_FORMAT, 1, false),
            dummy_cube: plain_texture(device, "hd dummy cube", CUBE_FORMAT, 6, true),
            dummy_depth: plain_texture(
                device,
                "hd dummy depth",
                wgpu::TextureFormat::Depth32Float,
                1,
                false,
            ),
            uniforms,
            ext: crate::ext::ExtSlots::empty(device),
            capture: LightCapture::new(device, &prefilter, frame_bytes),
            down,
            prefilter,
            ssr_pipes: SsrPipes::new(device, cache),
            ssr: None,
            planar: None,
            refract_depth: None,
            view_module: module(Permutation::View),
            water_module: module(Permutation::Water),
            view_pipelines: HashMap::new(),
            water_pipelines: HashMap::new(),
            main_groups: None,
            view_groups: None,
            made_for: None,
            history_valid: false,
            plan: ReflectPlan::default(),
        }
    }

    /// The histories start again (a cut).
    pub fn reset(&mut self) {
        self.history_valid = false;
        self.capture.valid = false;
    }

    #[allow(clippy::too_many_arguments)]
    fn group(
        &self,
        device: &wgpu::Device,
        uniform: usize,
        ssr: &wgpu::TextureView,
        capture: &wgpu::TextureView,
        sky: &wgpu::TextureView,
        planar: &wgpu::TextureView,
        depth: &wgpu::TextureView,
        clouds: &wgpu::TextureView,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("hd reflections"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.uniforms[uniform].as_entire_binding(),
                },
                crate::tv(1, ssr),
                crate::tv(2, capture),
                crate::tv(3, sky),
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: self.ext.as_entire_binding(),
                },
                crate::tv(6, planar),
                crate::tv(7, depth),
                crate::tv(8, clouds),
            ],
        })
    }

    /// The extension's buffer for group 3's binding 5: the groups are made again when it is another.
    pub fn set_ext(&mut self, buffer: &wgpu::Buffer) {
        if self.ext != *buffer {
            self.ext = buffer.clone();
            self.main_groups = None;
            self.view_groups = None;
        }
    }

    /// The main view's group 3 of parity `p`.
    pub fn main_group(&self, p: usize) -> Option<&wgpu::BindGroup> {
        self.main_groups.as_ref().map(|g| &g[p])
    }

    /// `clouds`: the clouds' cube ([`crate::clouds::Clouds::cube_view`]).
    fn ensure_groups(&mut self, device: &wgpu::Device, clouds: &wgpu::TextureView) {
        if self.main_groups.is_none() {
            let groups = [0, 1].map(|p| {
                let ssr = self.ssr.as_ref().map_or(&self.dummy_2d, |s| &s.history[p]);
                let planar = self.planar.as_ref().map_or(&self.dummy_2d, |s| &s.view);
                let depth = self
                    .refract_depth
                    .as_ref()
                    .map_or(&self.dummy_depth, |d| &d.1);
                self.group(
                    device,
                    U_MAIN,
                    ssr,
                    &self.capture.scene.view,
                    &self.capture.sky.view,
                    planar,
                    depth,
                    clouds,
                )
            });
            self.main_groups = Some(groups);
        }
        if self.view_groups.is_none() {
            let d = (&self.dummy_2d, &self.dummy_depth);
            self.view_groups = Some([
                self.group(
                    device,
                    U_PLANAR,
                    d.0,
                    &self.capture.scene.view,
                    &self.capture.sky.view,
                    d.0,
                    d.1,
                    clouds,
                ),
                // A capture face reads the sky cube in the capture's place (it is drawing the capture).
                self.group(
                    device,
                    U_CAPTURE,
                    d.0,
                    &self.capture.sky.view,
                    &self.capture.sky.view,
                    d.0,
                    d.1,
                    clouds,
                ),
                self.group(
                    device,
                    U_SKY,
                    d.0,
                    &self.dummy_cube,
                    &self.dummy_cube,
                    d.0,
                    d.1,
                    &self.dummy_cube,
                ),
            ]);
        }
    }

    pub fn water_pipeline(&self, cull: Cull) -> Option<&wgpu::RenderPipeline> {
        self.water_pipelines.get(&cull)
    }

    /// The GPU memory of the reflections' own textures (bytes).
    pub fn bytes(&self) -> u64 {
        self.capture.bytes() + self.planar.as_ref().map_or(0, Planar::bytes)
    }
}

/// The reflections' uniform.
#[allow(clippy::too_many_arguments)]
fn refl_bytes(
    ssr_on: bool,
    water_mode: u32,
    capture_valid: bool,
    mirror: bool,
    level: f32,
    above: bool,
    clear_water: bool,
    specular: f32,
    scale: u32,
    ext: [f32; 4],
    color_levels: u32,
    steps: u32,
    history: bool,
    time: f32,
    capture_face: bool,
    cloud_cube: bool,
    shifts: &[[f32; 3]; 6],
) -> Vec<u8> {
    let b = |v: bool| if v { 1.0 } else { 0.0 };
    let mut out = Vec::with_capacity(REFL_BYTES);
    crate::push_f32(
        &mut out,
        &[
            b(ssr_on),
            water_mode as f32,
            b(capture_valid),
            b(mirror),
            level,
            b(above),
            // The water: clear, or its texture its face.
            if clear_water { 1.0 } else { 0.0 },
            scale as f32,
            color_levels as f32,
            steps as f32,
            b(history),
            time,
            b(capture_face),
            specular,
            CAPTURE_MIPS as f32,
            b(cloud_cube),
        ],
    );
    for s in shifts {
        crate::push_f32(&mut out, &[s[0], s[1], s[2], 0.0]);
    }
    // The extension's block (`ReflExt`).
    crate::push_f32(&mut out, &ext);
    debug_assert_eq!(out.len(), REFL_BYTES);
    out
}

impl<E: HdExtension> HdWorld<E> {
    /// The frame uniform of a secondary view through `camera` (no AO, the view flag set, cut at `clip` when given).
    fn view_frame(&self, scene: &E::Scene, camera: &HdCamera, clip: Option<f32>) -> Vec<u8> {
        let mut post = Vec::with_capacity(256);
        crate::push_f32(&mut post, &camera.view_proj);
        crate::push_f32(&mut post, &camera.view_proj);
        crate::push_f32(&mut post, &crate::invert(&camera.view_proj));
        crate::push_f32(&mut post, &[0.0; 4]);
        crate::push_f32(
            &mut post,
            &[torque_render_scene::hd::post::AO_RADIUS, 0.0, 0.0, 1.0],
        );
        crate::push_f32(
            &mut post,
            &[
                0.0,
                1.0,
                if clip.is_some() { 1.0 } else { 0.0 },
                clip.unwrap_or(0.0),
            ],
        );
        let (w, h) = (camera.width as f32, camera.height as f32);
        crate::push_f32(&mut post, &[w, h, 1.0 / w, 1.0 / h]);
        self.frame_bytes(scene, camera, &post, false)
    }

    /// The draws of a secondary view through `camera` (left-handed: front faces turned): the sky, and unless `sky_only` the opaque
    /// cells it sees, the ground and the opaque shapes. Their pipelines and texture groups are made here.
    /// `clear_of`: a capture face's eye (the shapes [`CAPTURE_SHAPE_CLEAR`] near it are left out).
    pub(crate) fn view_draws(
        &mut self,
        camera: &HdCamera,
        sky_only: bool,
        physical_sky: bool,
        clear_of: Option<[f32; 3]>,
    ) -> Vec<ViewDraw> {
        let mut sources: Vec<(Source, Blend, Cull, bool, bool, TexRef, SamplerKey)> = Vec::new();
        let loose = |d: &DrawRef| {
            (
                Source::Loose(*d),
                d.blend,
                d.cull,
                d.depth_test,
                d.depth_write,
                d.tex,
                d.sampler,
            )
        };
        // The server's sky, with the physical sky its box shaded by the atmosphere (no sun disk here).
        let order = torque_render_scene::hd::sky::order_labels(&self.loose.sky_labels);
        for n in 0..=self.loose.sky.len() {
            if n == order.shade && physical_sky {
                sources.push((
                    Source::PhysicalSky,
                    Blend::Opaque,
                    Cull::None,
                    false,
                    false,
                    TexRef::White,
                    SamplerKey::PLAIN,
                ));
            }
            if let Some(d) = self.loose.sky.get(n) {
                sources.push(loose(d));
            }
        }
        let culled = if sky_only {
            None
        } else {
            Some(self.cull_view(camera))
        };
        if culled.is_some() {
            // The opaque groups culled for this view (the near clusters left out, as the extra draws are).
            let groups: Vec<crate::arena::GroupKey> = self
                .table
                .groups()
                .map(|(g, _)| *g)
                .filter(|g| !g.0 && !E::near_slot(g.1))
                .collect();
            for g in groups {
                let (tex, s) = self
                    .slots
                    .get(&g.1)
                    .copied()
                    .unwrap_or((TexRef::Empty, SamplerKey::PLAIN));
                sources.push((
                    Source::Group(g),
                    Blend::Opaque,
                    Cull::BackCw,
                    true,
                    true,
                    tex,
                    s,
                ));
            }
        }
        if !sky_only {
            sources.extend(self.loose.planes.iter().map(loose));
            let planes = camera.planes();
            for o in &self.loose.objects {
                let seen = crate::sphere_seen(&planes, o.centre, o.radius);
                // The objects the extension keeps stay (a floor the eye stands over holds the eye in its sphere).
                let near = !E::kept_near_capture(o.ghost)
                    && clear_of.is_some_and(|e| {
                        Vec3::from(o.centre).distance(e.into()) < o.radius + CAPTURE_SHAPE_CLEAR
                    });
                if seen && !near {
                    sources.extend(
                        o.draws
                            .iter()
                            .filter(|d| d.blend == Blend::Opaque && d.depth_write && !d.shadow_only)
                            .map(loose),
                    );
                }
            }
        }
        let mut out = Vec::with_capacity(sources.len() + 1);
        if let Some((slot, view)) = culled {
            out.push(ViewDraw {
                source: Source::View(slot, view),
                key: PipelineKey {
                    blend: Blend::Opaque,
                    cull: Cull::None,
                    depth_test: false,
                    depth_write: false,
                    clustered: false,
                    front_layer: false,
                    stage: Stage::Main,
                },
                group: (TexRef::White, SamplerKey::PLAIN),
            });
        }
        for (source, blend, cull, depth_test, depth_write, tex, s) in sources {
            let key = PipelineKey {
                blend,
                cull: turned(cull),
                depth_test,
                depth_write,
                clustered: false,
                front_layer: false,
                stage: Stage::Main,
            };
            self.ensure_view_pipeline(key);
            self.ensure_group(tex, s);
            out.push(ViewDraw {
                source,
                key,
                group: (tex, s),
            });
        }
        out
    }

    /// Plans this frame's reflections (before anything is encoded): makes the targets and groups they need, writes the uniforms of
    /// every view, and lists the secondary views' draws. `icam` is the internal camera, `jcam` the same jittered; `remade` says the
    /// frame's targets were made anew.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn plan_reflections(
        &mut self,
        scene: &E::Scene,
        icam: &HdCamera,
        jcam: &HdCamera,
        mode: HdMode,
        surfaces: &Surfaces,
        remade: bool,
        color_size: (u32, u32),
    ) {
        let quality = mode.quality;
        let physical_sky = mode.image.physical_sky;
        let rq = quality.reflections();
        let on = self.features.reflections;
        let level = self.planar_override.or_else(|| water_level(scene));
        let water = level.is_some() && !self.loose.water.is_empty();
        let above = level.is_some_and(|l| icam.eye[2] > l + 1e-3);
        // A reflective surface in sight.
        let shiny = on && self.grid.reflective_seen(&|a, b| icam.sees_box(a, b));
        let ssr_surfaces = shiny && rq.ssr != SsrResolution::Off;
        let mut water_mode = match (water || self.planar_override.is_some(), on, rq.water) {
            (false, _, _) => 0,
            (true, false, _) => 1,
            (true, true, WaterReflection::Sky) => 1,
            (true, true, WaterReflection::Ssr) => 2,
            (true, true, WaterReflection::Planar { .. }) => 3,
        };
        if water_mode == 3 && !above {
            water_mode = 1;
        }
        let ssr = ssr_surfaces || (water_mode == 2 && above);
        // Where the extension asks, the capture is drawn at every quality: indoors the sky cube is no stand-in for what is around
        // the eye.
        let capture = (shiny && rq.capture) || (on && E::capture_always(scene));
        let scale = match rq.ssr {
            SsrResolution::Full => 1,
            _ => 2,
        };
        let planar_size = match rq.water {
            WaterReflection::Planar { half: true } => {
                (icam.width.div_ceil(2), icam.height.div_ceil(2))
            }
            _ => (icam.width, icam.height),
        };
        // The targets.
        let (iw, ih) = (icam.width, icam.height);
        // The SSR's targets (the Hi-Z, its trace and history, the last frame's colour pyramid) only where this quality traces
        // screen-space rays (the reflective surfaces' SSR, or the water's), and the water's depth only with water.
        let needs_ssr = on && (rq.ssr != SsrResolution::Off || rq.water == WaterReflection::Ssr);
        let want = (
            iw,
            ih,
            scale,
            if water_mode == 3 { planar_size.0 } else { 0 },
            color_size,
            needs_ssr,
            water,
        );
        if remade || self.refl.made_for != Some(want) {
            self.refl.ssr = None;
            if let Some(t) = self.targets.as_ref().filter(|_| needs_ssr) {
                let inputs = SsrInputs {
                    frame: &self.frame,
                    refl: &self.refl.uniforms[U_MAIN],
                    depth: &t.depth_view,
                    normals: &t.normal_view,
                    sampler: &self.refl.sampler,
                };
                let made = SsrTargets::new(
                    &self.device,
                    &self.refl.ssr_pipes,
                    &self.refl.down,
                    (iw, ih, scale, color_size.0, color_size.1),
                    &inputs,
                );
                self.refl.ssr = Some(made);
            }
            self.refl.planar = (water_mode == 3).then(|| {
                Planar::new(
                    &self.device,
                    planar_size.0,
                    planar_size.1,
                    super::frame_size::<E>() as u64,
                )
            });
            self.refl.refract_depth = None;
            let depth = water.then(|| {
                self.device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("hd water depth"),
                    size: wgpu::Extent3d {
                        width: iw,
                        height: ih,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Depth32Float,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                })
            });
            self.refl.refract_depth = depth.map(|d| {
                let view = d.create_view(&wgpu::TextureViewDescriptor::default());
                (d, view)
            });
            self.refl.main_groups = None;
            self.refl.made_for = Some(want);
            self.refl.history_valid = false;
        }
        self.refl
            .ensure_groups(&self.device, self.clouds.cube_view());
        if !ssr {
            self.refl.history_valid = false;
        }
        // The secondary views.
        let mut plan = ReflectPlan {
            ssr,
            capture,
            water: water && on,
            clear: scene
                .clear
                .map(|c| f64::from(torque_render_scene::hd::srgb_to_linear(c))),
            ..ReflectPlan::default()
        };
        let far = scene.visible_distance.max(1.0);
        // The sky's draws change only with a whole sync (a control sync, every frame of a window, moves only its objects).
        let draws_gen = self.loose.static_generation;
        if self.refl.capture.sky_generation != draws_gen {
            for face in 0..6 {
                let cam = cube_face_camera(
                    icam.eye,
                    face,
                    CAPTURE_NEAR,
                    far,
                    torque_render_scene::hd::reflect::CAPTURE_SIZE,
                );
                let bytes = self.view_frame(scene, &cam, None);
                self.queue
                    .write_buffer(&self.refl.capture.frames[6 + face], 0, &bytes);
                let draws = self.view_draws(&cam, true, physical_sky, None);
                plan.sky_faces.push((face, draws));
            }
            self.refl.capture.sky_generation = draws_gen;
        }
        if capture {
            if self.capture_every_frame {
                self.refl.capture.refresh = true;
            }
            for face in self.refl.capture.faces() {
                // (The moving shapes were kept for other faces: this one again in full the next frame.)
                if !E::capture_cover(scene)[face] {
                    self.refl.capture.refresh = true;
                }
                let cam = cube_face_camera(
                    icam.eye,
                    face,
                    CAPTURE_NEAR,
                    far,
                    torque_render_scene::hd::reflect::CAPTURE_SIZE,
                );
                let bytes = self.view_frame(scene, &cam, None);
                self.queue
                    .write_buffer(&self.refl.capture.frames[face], 0, &bytes);
                let draws = self.view_draws(&cam, false, physical_sky, Some(icam.eye));
                self.refl.capture.eyes[face] = icam.eye;
                plan.capture_faces.push((face, draws));
            }
        }
        if water_mode == 3
            && let (Some(l), Some(pl)) = (level, &self.refl.planar)
        {
            let cam = mirror_camera(jcam, l).sized(pl.width, pl.height);
            let bytes = self.view_frame(scene, &cam, Some(l));
            self.queue.write_buffer(&pl.frame, 0, &bytes);
            plan.planar = Some(self.view_draws(&cam, false, physical_sky, None));
        }
        // The water's pipelines.
        if plan.water {
            for d in self.loose.water.clone() {
                self.ensure_water_pipeline(d.cull);
            }
        }
        // The uniforms.
        let capture_valid = capture && self.refl.capture.valid;
        let levels = self.refl.ssr.as_ref().map_or(1, SsrTargets::color_levels);
        let time = E::time(scene);
        let ext = self.ext.refl_ext(scene);
        let lv = level.unwrap_or(0.0);
        let mirror = self.mirror_reflections || self.debug == DebugView::Reflection;
        // The clouds' cube is read where the water is drawn, the eye is above it and the cube holds the clouds (its faces drawn
        // by the frames before); under the water it is held, not drawn, so it is not read either.
        let cloud_cube =
            plan.water && self.features.cloud_cube && above && self.clouds.cube_ready();
        let shifts: [[f32; 3]; 6] = if capture_valid {
            std::array::from_fn(|f| {
                let e = self.refl.capture.eyes[f];
                [0, 1, 2].map(|k| icam.eye[k] - e[k])
            })
        } else {
            [[0.0; 3]; 6]
        };
        let main = refl_bytes(
            ssr_surfaces,
            water_mode,
            capture_valid,
            mirror,
            lv,
            above,
            mode.image.clear_water,
            surfaces.specular,
            scale,
            ext,
            levels,
            rq.ssr_steps.max(32),
            self.refl.history_valid && ssr && self.histories.ssr,
            time,
            false,
            cloud_cube,
            &shifts,
        );
        self.queue
            .write_buffer(&self.refl.uniforms[U_MAIN], 0, &main);
        let view = |capture: bool, face: bool, cloud_cube: bool| {
            refl_bytes(
                false,
                0,
                capture,
                false,
                lv,
                false,
                mode.image.clear_water,
                surfaces.specular,
                1,
                ext,
                1,
                0,
                false,
                time,
                face,
                cloud_cube,
                &[[0.0; 3]; 6],
            )
        };
        self.queue.write_buffer(
            &self.refl.uniforms[U_PLANAR],
            0,
            &view(capture_valid, false, cloud_cube),
        );
        self.queue.write_buffer(
            &self.refl.uniforms[U_CAPTURE],
            0,
            &view(false, true, cloud_cube),
        );
        self.queue
            .write_buffer(&self.refl.uniforms[U_SKY], 0, &view(false, false, false));
        // A face of the clouds' cube when the water is drawn (what it reflects off screen); not while the eye is under the water,
        // which shows no reflection of the sky (the cube keeps what it holds for the eye's return).
        self.clouds.cube_wanted = match (plan.water && self.features.cloud_cube, above) {
            (false, _) => crate::clouds::CubeWanted::No,
            (true, false) => crate::clouds::CubeWanted::Held,
            (true, true) => crate::clouds::CubeWanted::Draw,
        };
        self.refl.plan = plan;
    }

    #[allow(clippy::too_many_arguments)]
    fn encode_view(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        color: &wgpu::TextureView,
        depth: &wgpu::TextureView,
        clear: [f64; 4],
        frame: &wgpu::Buffer,
        refl: &wgpu::BindGroup,
        draws: &[ViewDraw],
        stamp: Option<wgpu::RenderPassTimestampWrites<'_>>,
    ) {
        let frame_group = self.frame_group_with(
            frame,
            &self.refl.dummy_2d,
            &self.refl.dummy_2d,
            &self.refl.dummy_2d,
            self.dummy_depth(),
        );
        // The view's lists, culled before its pass (the GPU's), or its planes for the CPU's test at the draws.
        let view = match draws.first() {
            Some(ViewDraw {
                source: Source::View(slot, view),
                ..
            }) => Some((*slot, *view)),
            _ => None,
        };
        if let Some((Some(slot), _)) = view
            && let Some(c) = &self.cull
        {
            c.encode(
                &self.device,
                encoder,
                slot,
                &self.table,
                &self.cell_table,
                &self.scratch_lists,
                false,
            );
        }
        crate::count_pass();
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("hd secondary view"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: color,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: clear[0],
                        g: clear[1],
                        b: clear[2],
                        a: clear[3],
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(0.0),
                    store: wgpu::StoreOp::Discard,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: stamp,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_bind_group(0, &frame_group, &[]);
        pass.set_bind_group(2, &self.sky.group, &[]);
        pass.set_bind_group(3, refl, &[]);
        let mut bound: Option<(PipelineKey, (TexRef, SamplerKey))> = None;
        // Which buffers are bound (Some(true): the static arena's).
        let mut buffers: Option<bool> = None;
        let mut page: Option<u32> = None;
        for d in draws {
            if let Source::View(..) = d.source {
                continue;
            }
            if let Source::PhysicalSky = d.source {
                pass.set_pipeline(&self.sky.shade_pipeline);
                pass.set_bind_group(1, &self.groups[&d.group], &[]);
                pass.draw(0..3, 0..1);
                bound = None;
                continue;
            }
            if bound.is_none_or(|(k, _)| k != d.key) {
                pass.set_pipeline(&self.refl.view_pipelines[&d.key]);
            }
            if bound.is_none_or(|(_, g)| g != d.group) {
                pass.set_bind_group(1, &self.groups[&d.group], &[]);
            }
            bound = Some((d.key, d.group));
            match d.source {
                Source::PhysicalSky | Source::View(..) => {}
                Source::Group(g) => {
                    if buffers != Some(true) || page != Some(g.2) {
                        self.arena.bind(&mut pass, g.2, &[0]);
                        buffers = Some(true);
                        page = Some(g.2);
                    }
                    match view {
                        Some((Some(_), _)) => {
                            self.scratch_lists
                                .draw_group(&mut pass, &self.table, g, self.draw_way);
                        }
                        Some((None, v)) => {
                            crate::arena::Lists::draw_group_direct(
                                &mut pass,
                                &self.table,
                                g,
                                self.first_instance,
                                |r| crate::cull::cpu_pass(&v, &self.cell_table, r),
                            );
                        }
                        None => {}
                    }
                }
                Source::Loose(r) => {
                    let (Some(v), Some(i)) = (&self.loose.vertices, &self.loose.indices) else {
                        continue;
                    };
                    if buffers != Some(false) {
                        pass.set_vertex_buffer(0, v.slice(..));
                        pass.set_index_buffer(i.slice(..), wgpu::IndexFormat::Uint32);
                        buffers = Some(false);
                    }
                    if r.count > 0 {
                        pass.draw_indexed(r.first..r.first + r.count, r.base_vertex, 0..1);
                    }
                }
            }
        }
    }

    /// The secondary views (the sky cube's and the capture's faces with their mips, the planar reflection); timed from `begin` to `end`
    /// when given. Returns whether anything was drawn (and timed).
    pub(crate) fn encode_views(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        stamps: Option<(&wgpu::QuerySet, u32, u32)>,
    ) -> bool {
        let plan = &self.refl.plan;
        if !plan.has_views() {
            return false;
        }
        let Some(groups) = &self.refl.view_groups else {
            return false;
        };
        let total =
            plan.sky_faces.len() + plan.capture_faces.len() + usize::from(plan.planar.is_some());
        let mut n = 0;
        let stamp = |n: usize| {
            stamps.and_then(|(q, b, e)| {
                let first = n == 0;
                let last = n + 1 == total;
                (first || last).then_some(wgpu::RenderPassTimestampWrites {
                    query_set: q,
                    beginning_of_pass_write_index: first.then_some(b),
                    end_of_pass_write_index: last.then_some(e),
                })
            })
        };
        let capture = &self.refl.capture;
        for (face, draws) in &plan.sky_faces {
            self.encode_view(
                encoder,
                &capture.sky.faces[*face][0],
                &capture.depth,
                [plan.clear[0], plan.clear[1], plan.clear[2], 1.0],
                &capture.frames[6 + face],
                &groups[2],
                draws,
                stamp(n),
            );
            n += 1;
        }
        capture.sky.encode_prefilter(
            encoder,
            plan.sky_faces.iter().map(|(f, _)| *f),
            &self.refl.prefilter,
        );
        for (face, draws) in &plan.capture_faces {
            self.encode_view(
                encoder,
                &capture.scene.faces[*face][0],
                &capture.depth,
                [plan.clear[0], plan.clear[1], plan.clear[2], 1.0],
                &capture.frames[*face],
                &groups[1],
                draws,
                stamp(n),
            );
            n += 1;
        }
        capture.scene.encode_prefilter(
            encoder,
            plan.capture_faces.iter().map(|(f, _)| *f),
            &self.refl.prefilter,
        );
        if let (Some(draws), Some(pl)) = (&plan.planar, &self.refl.planar) {
            self.encode_view(
                encoder,
                &pl.view,
                &pl.depth,
                // Its alpha -1 where nothing but the sky is drawn (the sky keeps the alpha; the opaque scene and the terrain's
                // layers over it leave it at 0 or more: the float target holds the sign): the water lays the clouds over the sky
                // it reflects there (`water.wgsl`'s `reflected_clouds`).
                [plan.clear[0], plan.clear[1], plan.clear[2], -1.0],
                &pl.frame,
                &groups[0],
                draws,
                stamp(n),
            );
        }
        true
    }

    /// This frame's face of the clouds' cube (after the clouds' march: it reads the sky group they bound): the face's camera
    /// around the eye `eye` (reaching `far`), then its march and mips.
    pub(crate) fn encode_cloud_face(
        &self,
        scene: &E::Scene,
        encoder: &mut wgpu::CommandEncoder,
        eye: [f32; 3],
        far: f32,
    ) {
        let Some((face, frame)) = self.clouds.cube_face() else {
            return;
        };
        let cam = cube_face_camera(eye, face, CAPTURE_NEAR, far, crate::clouds::CLOUD_CUBE_SIZE);
        let bytes = self.view_frame(scene, &cam, None);
        self.queue.write_buffer(frame, 0, &bytes);
        let frame_group = self.frame_group_with(
            frame,
            &self.refl.dummy_2d,
            &self.refl.dummy_2d,
            &self.refl.dummy_2d,
            self.dummy_depth(),
        );
        self.clouds
            .encode_cube(encoder, &frame_group, &self.sky.group);
    }

    /// The Hi-Z, the trace and the accumulation into this frame's SSR history (after the prepass).
    pub(crate) fn encode_ssr(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        p: usize,
        stamps: Option<(&wgpu::QuerySet, u32, u32)>,
    ) -> bool {
        match (&self.refl.ssr, self.refl.plan.ssr) {
            (Some(s), true) => {
                s.encode(encoder, &self.refl.ssr_pipes, p, stamps);
                true
            }
            _ => false,
        }
    }

    /// The depth copied for the water (before the translucent pass).
    pub(crate) fn encode_water_depth(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        depth: &wgpu::Texture,
    ) {
        if !self.refl.plan.water {
            return;
        }
        if let Some((d, _)) = &self.refl.refract_depth
            && d.size() == depth.size()
        {
            encoder.copy_texture_to_texture(depth.as_image_copy(), d.as_image_copy(), d.size());
        }
    }

    /// The frame copied for the next frame's rays (`source`: the TAA's anti-aliased frame, or the frame after the translucent pass),
    /// with its mip chain. Returns whether it was.
    pub(crate) fn encode_color_copy(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        source: &wgpu::Texture,
    ) -> bool {
        match (&self.refl.ssr, self.refl.plan.ssr) {
            (Some(s), true)
                if s.color.width() == source.width() && s.color.height() == source.height() =>
            {
                encoder.copy_texture_to_texture(
                    source.as_image_copy(),
                    s.color.as_image_copy(),
                    wgpu::Extent3d {
                        width: source.width(),
                        height: source.height(),
                        depth_or_array_layers: 1,
                    },
                );
                s.encode_color_mips(encoder, &self.refl.down);
                true
            }
            _ => false,
        }
    }

    /// Whether the water item `d` is drawn by `fs_water` this frame.
    pub(crate) fn water_drawn(&self, d: &DrawRef) -> bool {
        self.refl.plan.water && self.refl.water_pipelines.contains_key(&d.cull)
    }

    /// The last frame's planar reflection (the internal frame's size or half of it), RGBA as floats.
    pub fn read_planar(&self) -> Result<(u32, u32, Vec<[f32; 4]>), crate::HdError> {
        let p = self
            .refl
            .planar
            .as_ref()
            .ok_or("no planar reflection was drawn")?;
        let (w, h, bytes) = self.read_texture(&p.color, 8)?;
        Ok((w, h, crate::halves(&bytes)))
    }

    /// The reflections' own textures' GPU memory (the cube maps, the planar reflection; bytes).
    pub fn reflection_bytes(&self) -> u64 {
        self.refl.bytes()
    }

    /// What the last frame's reflections drew: (SSR traced, planar drawn, capture faces drawn, sky faces drawn).
    pub fn reflection_stats(&self) -> (bool, bool, usize, usize) {
        let p = &self.refl.plan;
        (
            p.ssr,
            p.planar.is_some(),
            p.capture_faces.len(),
            p.sky_faces.len(),
        )
    }
}

impl<E: HdExtension> HdWorld<E> {
    /// A reflection view's pipeline for `key` (made once).
    pub(crate) fn ensure_view_pipeline(&mut self, key: PipelineKey) {
        if self.refl.view_pipelines.contains_key(&key) {
            return;
        }
        let p = crate::forward_pipeline(
            &self.device,
            self.pipeline_cache.as_ref(),
            &self.forward_layout,
            &self.refl.view_module,
            key,
            "fs_main",
        );
        self.refl.view_pipelines.insert(key, p);
        self.pipelines_made += 1;
    }

    /// The water's pipeline for `cull` (made once).
    pub(crate) fn ensure_water_pipeline(&mut self, cull: Cull) {
        if self.refl.water_pipelines.contains_key(&cull) {
            return;
        }
        let key = PipelineKey {
            blend: Blend::Opaque,
            cull,
            depth_test: true,
            depth_write: false,
            clustered: true,
            front_layer: false,
            stage: Stage::Main,
        };
        let p = crate::forward_pipeline(
            &self.device,
            self.pipeline_cache.as_ref(),
            &self.forward_layout,
            &self.refl.water_module,
            key,
            "fs_water",
        );
        self.refl.water_pipelines.insert(cull, p);
        self.pipelines_made += 1;
    }
}
