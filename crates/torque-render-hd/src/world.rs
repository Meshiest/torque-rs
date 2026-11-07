//! The enhanced renderer's world ([`HdWorld`]): a [`torque_render_scene::hd::HdScene`] drawn with our own WGSL ([`shaders`]) on a
//! [`torque_render_core::Gpu`], with what the world's own static geometry brings beside it ([`crate::ext::HdExtension`]: its
//! tables, uniform blocks, extra draws and coarser levels).
//!
//! **The frame** ([`HdWorld::draw`]): the light clusters (a compute pass: 16 x 9 tiles x 24 exponential depth slices, each with a
//! 1,024-bit mask of the point lights whose sphere meets it), then one forward pass into an `Rgba16Float` HDR target with a reversed
//! 32-bit float depth (the sky first, unlit and without depth; the opaque static cells front to back, the ground, the opaque shapes;
//! the multiplied decals over them in their own pass; then the water, the translucent shapes and coronas back to front, the
//! translucent static cells back to front, the sun's flare), then the tonemap pass (exposure, our neutral hue-keeping curve, sRGB
//! encoding) into the caller's `Rgba8Unorm` target. Textures are sampled through sRGB views (linear in the shader); vertex colours
//! are decoded the same way.
//!
//! **Uploads** ([`HdWorld::upload`]) are incremental: new textures of the scene's table, the static cells whose generation changed
//! (removed cells dropped), and the eye-dependent draws (sky, planes, shapes, coronas, flare) when the scene was synced again.
//!
//! **Shadows** ([`shadows`]): before everything else the sun's cascades and the point lights' cubes are brought up to date (the
//! static maps only where they changed, then the moving shapes over a copy); after the clusters a depth prepass of the opaque draws
//! (the forward pass then shades each pixel once) and the contact shadows' screen-space march over it. The forward pass samples the
//! maps and the contact mask ([`HdFeatures`] turns each off; [`DebugView`] shows the visibilities the shadows' checks measure).
//!
//! **Anti-aliasing and AO** ([`post`]; the ladder is `torque_render_scene::hd::post`): the frame is drawn at the quality's internal
//! size (75 %, 85 %, 100 %, 100 %) with, under the TAA, a sub-pixel jitter (Halton(2, 3), 8 phases). The prepass also writes each
//! pixel's motion (the camera's and, for a shape that moved since the last frame, its own: its previous transform) and normal. Before
//! the opaque pass the AO pass (GTAO at High and Ultra, SSAO at Medium; half resolution but at Ultra) estimates the ambient
//! occlusion with a pattern fixed per pixel and its spatial filter smooths it (no history: nothing lags behind a moving shape),
//! which the forward pass upsamples by depth into the ambient light. After the translucent pass the TAA resolve
//! (with TAAU when the internal frame is smaller) writes the history at the output's size; at Low the FXAA-style filter runs
//! instead. The tonemap pass reads that (scaling it up at Low) and at Ultra sharpens it. The specular AA (normal variance) is in the
//! forward pass at every quality.
//!
//! **Reflections and water** (`reflect`; the ladder and cameras are `torque_render_scene::hd::reflect`): before the main view the
//! secondary views (the sky cube, the camera-local capture one face a frame, the planar reflection of the water); after the prepass
//! the Hi-Z pyramid and the screen-space reflections (`ssr`); the forward passes read them in group 3 (the reflective surfaces per
//! pixel);
//! the water plane is drawn as water over the opaque frame's copy (`water`); after the translucent pass the frame is kept for the
//! next frame's rays.
//! **Sky, fog, bloom, exposure, volumetrics** ([`sky`], [`bloom`], [`exposure`], [`volumetric`]; the data is
//! `torque_render_scene::hd::{atmosphere, sky, grade}`): each frame the sky uniform (bind group 2 of the forward pass, the sky passes and the
//! volumetrics) and, with the physical sky, the sky view table (the transmittance and multiple scattering tables once). After the AO
//! the volumetric light's march (a reduced size, the prepass's depth, the sun's cascades). The opaque pass is split after the sky:
//! the server's sky (its box, flare, clouds and bans) with an HDR sun disk before the clouds, with the physical sky its box shaded
//! by the atmosphere (a full-frame pass after the box) and its clouds brightened toward the sun; the frame is copied there with its
//! box-filtered mips (the fog's colour is the sky behind each pixel without its detail, the detail only where a surface wholly fogged
//! ends at the visible distance; the height fog where the image settings have it), and the fog always reaches the visible distance.
//! Glowing surfaces, self-illuminating shape parts, coronas and flares are HDR emitters. The volumetric light is added after the
//! opaque pass. After the anti-aliasing: the bloom chain (and the automatic exposure, a histogram and its adaptation over
//! [`HdWorld::frame_seconds`], when [`HdWorld::auto_exposure`] asks for it); the tonemap adds or mixes in the bloom (the image
//! settings say which), applies the exposure and the neutral hue-keeping curve (a scene's own palette keeps its hues).
//!
//! **Materials and translucency** (the grading is `torque_render_scene::hd::lut`): the frame uniform carries the surface numbers
//! ([`crate::ext::Surfaces`]; the forward pass resolves each surface from them). The translucent pass
//! draws the water and the opaque parts of translucent shapes, then (with [`HdFeatures::wboit`]) the alpha-blended draws as weighted
//! blended transparency behind an exact front layer: their nearest depth into a copy of the depth, everything behind it into the
//! accumulation and revealage targets, the composite over the frame's copy (`wboit.wgsl`), the front layer blended over the
//! composite's copy; then the additive draws and the flare. At High and up the sun's cascades get transmittance maps (coloured
//! shadows, [`shadows`]). The tonemap reads our 32³ grading table (baked per image settings and user `.cube`,
//! [`HdWorld::set_user_lut`]) and adds film grain only where the caller asks ([`HdWorld::grain`]).
//!
//! **Volumetric clouds** ([`clouds`]; the data, the ladder and the noise are `torque_render_scene::hd::clouds`): where the image
//! settings and the quality have them (from Medium; only at Ultra where the settings opt in and [`HdFeatures::opt_in_clouds`] asks)
//! and the scene has a server sky,
//! after the AO the cloud shadow map and the march at a reduced size into the clouds' own history (reprojected each frame); the sky
//! pass composites their image in front of the server's flat cloud layers (the sky's high layer) and behind the fog bans (so the fog
//! toward the sky behind a pixel sees them too, without their detail: the image's low mip). The march starts at the eye wherever it
//! is; a surface the ray reaches through a volume layer's slab has their image laid over it. The volumetric light reads their shadow
//! (no god rays under a cloud), and at Ultra so does the forward pass (their shadows on the ground).
//!
//! **Timing**: with the device's `TIMESTAMP_QUERY` the frame's GPU time is measured from its first pass's start to the tonemap's end
//! ([`HdWorld::gpu_ms`]), and the shadow maps', the contact shadows', the AO's, the anti-aliasing's and the reflections' passes on
//! their own ([`HdWorld::gpu_times`]); without it, `None`.

pub use crate::arena::ARENA_BUDGET;

use std::collections::{BTreeMap, HashMap};

use glam::Vec3;

use torque_render_core::wgpu;
use torque_render_scene::hd::{
    HD_VERTEX_BYTES, HdCamera, HdDraw, HdLight, HdSpot, HdVertex, srgb_to_linear,
};
use torque_render_scene::{Blend, Cull, Filter, Sampler};
use wgpu::util::DeviceExt;

use torque_render_scene::hd::cells::StaticGeometry;
use torque_render_scene::hd::grade::{ExposureMode, Tonemap};
use torque_render_scene::hd::lut::{CubeLut, LUT_SIZE};
use torque_render_scene::hd::post::{AntiAliasing, AoMethod, RenderScale, internal_size, jitter};
use torque_render_scene::hd::settings::ImageSettings;

use crate::ext::{
    ExtCell, ExtSlots, ExtraDrawn, ExtraPrepare, HdExtension, NoExtension, ShadowExtras,
};
use crate::post::{
    AO_FORMAT, HISTORY_FORMAT, MOTION_FORMAT, MOTION_STRIDE, NORMAL_FORMAT, PostPipes,
    SHOWN_FORMAT, Temporal,
};
use crate::shaders::Permutation;
use crate::shadows::{MAX_DIRTY, ObjectCaster, ShadowInput, Shadows};
pub use crate::shadows::{ShadowStats, ShadowSwitches};
use crate::{
    DEPTH_FORMAT, DRAW_KIND_MARKS, HDR_FORMAT, PASS_KIND_MARKS, PASS_KIND_NAMES, PASSES, PassKind,
    arena, bloom, capture, clouds, colour, cull, exposure, invert, module, passes_by_kind, post,
    push_f32, push_u32, set_pass_kind, shaders, shadows, sky, slots, storage, tv, uniform,
    volumetric, water,
};

#[path = "reflect.rs"]
pub(crate) mod reflect;

/// The most bytes of the levels' block meshes a live window's upload puts in (at least one block).
pub const LOD_BYTES_A_FRAME: u64 = 4 << 20;
/// The most static cell bytes a live window's upload writes ([`HdWorld::upload_budget`]).
pub const UPLOAD_BUDGET: u64 = 16 << 20;
/// The output format (the HDR and depth formats are the engine's).
pub const LDR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
/// The cluster grid (tiles across, down, depth slices) and the lights a frame holds.
pub const CLUSTER_GRID: [u32; 3] = [16, 9, 24];
pub const MAX_LIGHTS: usize = 1024;
/// One light's bytes in the lights buffer (`common.wgsl`'s `Light`).
pub const LIGHT_BYTES: usize = 64;

/// The lights a frame uploads, in the buffer's order: the shadowed ones first (the point lights but the glowing surfaces', then
/// the spots that cast shadows), then the spots that do not and the glowing surfaces' lights (`glow`: else left out); each one's
/// packed cone ([`HdSpot::cone`], zeros for a point light) and how many of the first cast shadows. At most [`MAX_LIGHTS`].
pub fn frame_lights(
    lights: &[HdLight],
    glow_lights: usize,
    spots: &[HdSpot],
    glow: bool,
) -> (Vec<HdLight>, Vec<[f32; 4]>, usize) {
    let plain = lights.len() - glow_lights.min(lights.len());
    let mut out: Vec<(HdLight, [f32; 4])> =
        lights[..plain].iter().map(|l| (*l, [0.0; 4])).collect();
    out.extend(
        spots
            .iter()
            .filter(|s| s.shadows)
            .map(|s| (s.light, s.cone())),
    );
    let shadowed = out.len().min(MAX_LIGHTS);
    out.extend(
        spots
            .iter()
            .filter(|s| !s.shadows)
            .map(|s| (s.light, s.cone())),
    );
    if glow {
        out.extend(lights[plain..].iter().map(|l| (*l, [0.0; 4])));
    }
    out.truncate(MAX_LIGHTS);
    let (l, c) = out.into_iter().unzip();
    (l, c, shadowed)
}
const CLUSTER_WORDS: u64 = 32;
/// A mesh's place in the frame's table (`torque_render_scene::hd::NodePlace`): the rows of its placement now, of its normals' turn and of
/// its placement at the last frame, three vec4 each.
pub(crate) const NODE_BYTES: usize = 9 * 16;
/// The frame uniform's bytes before the extension's block (`common.wgsl`'s `Frame`).
pub(crate) const FRAME_CORE_BYTES: usize =
    848 + 4 * torque_render_scene::hd::SURFACE_FLOATS + 32 + 16 + 32 + 16 * MOVING_CASTERS;

/// The frame uniform's bytes, extension `E`'s block included.
pub(crate) fn frame_size<E: HdExtension>() -> usize {
    FRAME_CORE_BYTES + E::FRAME_EXT_BYTES
}

/// The frame uniform's `movers` block (the WGSL's MOVING_CASTERS): no pass reads it now (the histories keep no rules about
/// what moved); it keeps its place for the extension's block after it.
pub const MOVING_CASTERS: usize = 16;
/// The tonemap pass's uniform (`tonemap.wgsl`'s `Post`).
const POST_BYTES: u64 = 160;
/// The weighted blend's accumulation and revealage formats.
pub const ACCUM_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
pub const REVEAL_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R16Float;
/// The grading table's format (filterable).
const LUT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// The contact shadows' mask format.
const CONTACT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R8Unorm;
/// The timestamp queries: the frame's first pass's start, the tonemap's end, the shadow maps' end, the contact pass's start and end,
/// the AO pass's start and its filter's end, the TAA's (or the FXAA-style filter's) start and end, the secondary views' and the
/// SSR's start and end, and the volumetric clouds' start and end.
const QUERIES: u32 = 20;
/// The frame's early submits' timestamps (the end of the part submitted, the start of the next: `Q_GAPS` + 2 k, + 2 k + 1), so
/// the frame's GPU time leaves out the GPU's wait for the CPU between its parts.
const Q_GAPS: u32 = 16;
const MAX_SPLITS: u32 = 2;
const Q_FRAME_BEGIN: u32 = 0;
const Q_FRAME_END: u32 = 1;
const Q_SHADOWS_END: u32 = 2;
const Q_CONTACT_BEGIN: u32 = 3;
const Q_CONTACT_END: u32 = 4;
const Q_AO_BEGIN: u32 = 5;
const Q_AO_END: u32 = 6;
const Q_AA_BEGIN: u32 = 7;
const Q_AA_END: u32 = 8;
const Q_VIEWS_BEGIN: u32 = 10;
const Q_VIEWS_END: u32 = 11;
const Q_SSR_BEGIN: u32 = 12;
const Q_SSR_END: u32 = 13;
const Q_CLOUDS_BEGIN: u32 = 14;
const Q_CLOUDS_END: u32 = 15;
/// The resolve buffer's slots (one a group of queries).
const RESOLVE_SLOTS: u64 = 9;
/// A query resolve's destination alignment.
const RESOLVE_ALIGN: u64 = 256;
/// The largest frame (a side).
pub const MAX_SIDE: u32 = 8192;

/// A texture the draws sample: the white texel (no texture), the transparent one (a static slot whose texture is missing: the decal
/// leaves the paint), or the scene's table's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum TexRef {
    White,
    Empty,
    Table(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct SamplerKey {
    repeat: bool,
    mag: bool,
    min: bool,
    mip: bool,
    mipmaps: bool,
}

impl SamplerKey {
    fn of(s: &Sampler) -> SamplerKey {
        SamplerKey {
            repeat: s.repeat,
            mag: s.mag == Filter::Linear,
            min: s.min == Filter::Linear,
            mip: s.mip == Filter::Linear,
            mipmaps: s.mipmaps,
        }
    }

    const PLAIN: SamplerKey = SamplerKey {
        repeat: true,
        mag: true,
        min: true,
        mip: true,
        mipmaps: true,
    };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct PipelineKey {
    blend: Blend,
    cull: Cull,
    depth_test: bool,
    depth_write: bool,
    clustered: bool,
    /// The translucent pass's front layer, blended exactly over the weighted composite's copy ([`Permutation::Forward`]'s `front_layer`).
    front_layer: bool,
    /// Which of the weighted blend's passes (or the plain forward pass).
    stage: Stage,
}

/// The passes of a forward-pass draw: the plain one; the translucent front layer's depth, the weighted blend of what is
/// behind it, the front layer blended exactly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Stage {
    Main,
    FrontDepth,
    Tail,
    Front,
    /// An additive particle in the late pass (`fs_particle`: faded into the surface behind it).
    Particle,
    /// A late object's draw of a TAA frame ([`HdExtension::drawn_late`], a dying shape among the particles): into the late frame
    /// (premultiplied, over the shown colour after the TAA).
    Over,
}

/// A draw in one of the shared buffers: an index range, its base vertex, texture, sampler and state.
#[derive(Debug, Clone, Copy)]
pub(crate) struct DrawRef {
    first: u32,
    count: u32,
    base_vertex: i32,
    tex: TexRef,
    sampler: SamplerKey,
    blend: Blend,
    cull: Cull,
    depth_test: bool,
    depth_write: bool,
    /// The prepass's motion matrix slot (0: static; a shape's 1 + its index).
    motion: u32,
    /// In the particles' buffers ([`Particles`]), not the loose ones.
    particle: bool,
    /// A late object's or a dying shape's: drawn after the TAA resolve where the frame has one.
    late: bool,
    /// Drawn into the shadow maps only (`torque_render_scene::hd::material::SHADOW_ONLY`: the client's own Player in its first-person
    /// view): no view, the capture's or the mirror's, draws it.
    shadow_only: bool,
    /// Cast into no shadow map (`torque_render_scene::hd::material::VIEW_ONLY`: a first-person image drawn before the eye).
    view_only: bool,
}

/// The frame's particles on the GPU (uploaded again whenever the scene's set changes: every frame they move).
#[derive(Default)]
struct Particles {
    generation: Option<u64>,
    vertices: Option<wgpu::Buffer>,
    indices: Option<wgpu::Buffer>,
    draws: Vec<(f32, DrawRef)>,
    /// The multiplied decals ([`Blend::Modulate2x`]): their own pass over the opaque frame, in any order.
    decals: Vec<DrawRef>,
}

/// A cell on the GPU: its slabs in the static arena ([`arena::Arena`]; its indices relative to its first vertex, `base_vertex`, from
/// `first_index` on), its clusters' slots in the table, and the index ranges its draws take (relative to `first_index`).
/// A unit's place in the cell table, box, key and level (what its table entry is written from).
type UnitBox = (u32, [[f32; 3]; 2], [i32; 3], u32);

pub(crate) struct GpuCell {
    /// The arena's page its slabs are in.
    pub(crate) page: u32,
    generation: u64,
    key: [i32; 3],
    /// 0 a cell, 1 and 2 a level of detail's block (keyed at its level).
    level: u32,
    /// Its place in the cell tables, its slabs' id, their places, its clusters' table slots.
    id: u32,
    slab: u32,
    pub(crate) base_vertex: i32,
    pub(crate) first_index: u32,
    records: Vec<u32>,
    /// Each batch's draws (first index and count relative to `first_index`, base vertex relative to `base_vertex`): its
    /// clusters' runs of 16-bit indices.
    pub(crate) batch_draws: Vec<Vec<(u32, u32, i32)>>,
    batches: Vec<torque_render_scene::hd::cells::Batch>,
    pub(crate) bounds: [[f32; 3]; 2],
    /// The opaque batches' indices (they come first): what casts shadows.
    pub(crate) opaque: u32,
    /// Whether it holds reflective surfaces ([`HdExtension::reflective`]: the reflections run when one is in sight).
    pub(crate) reflective: bool,
    /// Every batch's indices (the translucent ones run from `opaque` to here: the transmittance maps).
    pub(crate) total: u32,
}

impl GpuCell {
    /// The arena's index range of the cell's indices `a .. b`.
    pub(crate) fn range(&self, a: u32, b: u32) -> std::ops::Range<u32> {
        self.first_index + a..self.first_index + b
    }
}

/// A stamped object's run to write ([`HdWorld::upload_loose`]): its run, ghost, draws and stamp, whether its previous positions
/// are where its vertices are, and its first place in the frame's table of meshes' places.
type RunWrite<'a> = (slots::Run, u32, &'a [HdDraw], u64, bool, Option<usize>);

struct GpuObject {
    /// Its ghost (a light attached to it leaves it out of the light's own shadow casters).
    ghost: u32,
    /// A late object ([`HdExtension::drawn_late`]: drawn after the TAA resolve where the frame has one).
    late: bool,
    centre: [f32; 3],
    radius: f32,
    translucent: bool,
    draws: Vec<DrawRef>,
    /// Its points' way back to where they were at the last sync (`DynamicObject::motion`; the identity when its vertices bring
    /// their own previous positions).
    motion: [f32; 16],
    /// Each draw's id in the motion target (from its ghost index and its place among the object's draws, modulo
    /// [`post::MOTION_IDS`]: the TAA's object test), in the order of `draws` (their motion slots follow one another).
    ids: Vec<f32>,
    /// Its first place in the frame's table of meshes' places (its vertices' [`torque_render_scene::hd::material::NODE`] places are
    /// written past it), when it has places.
    node_base: Option<usize>,
}

/// The eye-dependent draws of one sync, in two shared buffers.
#[derive(Default)]
struct Loose {
    generation: Option<u64>,
    /// The scene's generation less its control syncs at this upload (the sky cube is drawn again when it changes).
    static_generation: Option<u64>,
    vertices: Option<wgpu::Buffer>,
    /// The vertices' previous positions, in the vertex layout (the prepass's second stream: a shape's last-frame positions, its
    /// own vertices for everything else).
    previous: Option<wgpu::Buffer>,
    indices: Option<wgpu::Buffer>,
    sky: Vec<DrawRef>,
    /// The labels of the sky's draws (`torque_render_scene::hd::sky::kept` sorts them).
    sky_labels: Vec<String>,
    planes: Vec<DrawRef>,
    water: Vec<DrawRef>,
    objects: Vec<GpuObject>,
    /// Each corona's eye distance, draw and light (its anchor: the opaque shapes in front of it hide it, each frame).
    coronas: Vec<(f32, DrawRef, [f32; 3])>,
    flare: Option<DrawRef>,
    /// Where the planes' vertices and indices were written, for which static sync ([`PlanesAt`]), and whether the previous
    /// positions' buffer holds them too: an upload with the same place leaves them as they are (a terrain is hundreds of
    /// thousands of vertices, and a window uploads every frame).
    planes_at: Option<PlanesAt>,
    planes_previous: bool,
}

/// A part of the loose rest's draws written side by side (each from its vertex), and whether its vertices and its previous
/// positions are written.
type RestPart<'a> = (&'a [(usize, &'a HdDraw, u32)], bool, bool);

/// The place of the planes in the loose buffers: the planes written (the scene's count of their changes, `planes_generation`,
/// and each draw's vertex and index lists by address and length, which a copy of a scene or a builder that forgets the count
/// changes), the vertex the rest starts at, the index the rest's indices start at, and the planes' vertices and indices (from,
/// to) within the rest's.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PlanesAt {
    generation: u64,
    planes: Vec<[usize; 4]>,
    rest_start: usize,
    index_rest: usize,
    vertices: (usize, usize),
    indices: (usize, usize),
}

/// The frame's targets. The output's size (`width` x `height`: the TAA's history, the LDR image) and the internal frame's (`iw` x
/// `ih`: everything drawn); the AO's is the internal one over `ao_scale`. Pairs are the two copies of a history, indexed by the
/// frame's parity (written this frame, read the next).
struct Targets {
    width: u32,
    height: u32,
    iw: u32,
    ih: u32,
    ao_scale: u32,
    /// The features the targets were made for (what a feature off uses is a 1 x 1 stand-in).
    uses: TargetUse,
    hdr: wgpu::Texture,
    hdr_view: wgpu::TextureView,
    /// The working depth (the prepass's, then the forward passes'), and the prepass's depth of this frame and the last (the TAA's
    /// disocclusion test: translucent surfaces write the working depth but have no motion of their own).
    depth: wgpu::Texture,
    depth_view: wgpu::TextureView,
    prepass_depths: [wgpu::Texture; 2],
    /// The prepass's motion of this frame and the last (one texture twice without the TAA).
    motions: [wgpu::Texture; 2],
    motion_views: [wgpu::TextureView; 2],
    normal_view: wgpu::TextureView,
    /// The contact shadows' mask and the bind groups that make it from the depth.
    contact_view: wgpu::TextureView,
    contact_group: wgpu::BindGroup,
    ao_views: [wgpu::TextureView; 2],
    ao_groups: [wgpu::BindGroup; 2],
    /// The AO filtered in space: what the forward pass reads, always.
    ao_smooth_view: wgpu::TextureView,
    history: [wgpu::Texture; 2],
    history_views: [wgpu::TextureView; 2],
    /// The TAA's shown color (and its neighborhood's mean luma), the tonemap's source.
    shown: [wgpu::Texture; 2],
    shown_views: [wgpu::TextureView; 2],
    taa_groups: [wgpu::BindGroup; 2],
    fxaa: wgpu::Texture,
    fxaa_view: wgpu::TextureView,
    fxaa_group: wgpu::BindGroup,
    /// The weighted blend's targets, the translucent front layer's depth and the composite's group.
    accum_view: wgpu::TextureView,
    reveal_view: wgpu::TextureView,
    trans_depth: wgpu::Texture,
    trans_depth_view: wgpu::TextureView,
    comp_group: wgpu::BindGroup,
    /// The opaque frame, copied before the translucent pass (the gamma-space blend reads it).
    background: wgpu::Texture,
    background_view: wgpu::TextureView,
    /// The frame after the sky's draws (the fog's colour: the sky behind each pixel).
    sky_copy: capture::Chained,
    ldr: wgpu::Texture,
    ldr_view: wgpu::TextureView,
    /// A TAA frame's late sprites (the internal size), the shown colour they are added to (the output's) and the
    /// composite's group.
    late_view: wgpu::TextureView,
    /// A TAA frame's late objects and dying shapes (premultiplied, the internal size), over the shown colour after the TAA.
    over_view: wgpu::TextureView,
    /// The shadow maps the TAA's groups read (their count of makes).
    shadow_maps: u64,
    present: wgpu::Texture,
    present_view: wgpu::TextureView,
    late_group: wgpu::BindGroup,
}

/// Which of the frame's optional targets the image settings, the quality and the features use this frame (the rest are made 1 x 1: the
/// TAA's histories, its shown colours, its prepass depths, its late sprites and their composite, the second motion target; the
/// FXAA-style filter's frame; the AO's; the contact shadows' mask). A change makes the targets anew (and the histories start again).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TargetUse {
    pub taa: bool,
    pub fxaa: bool,
    pub ao: bool,
    pub contact: bool,
}

struct Timer {
    queries: wgpu::QuerySet,
    resolve: wgpu::Buffer,
    readback: wgpu::Buffer,
    period_ns: f64,
    pending: bool,
    /// Whether the pending frame timed its shadow maps, its contact pass, its AO pass, its anti-aliasing pass, its secondary views
    /// and its SSR.
    shadows: bool,
    contact: bool,
    ao: bool,
    aa: bool,
    views: bool,
    ssr: bool,
    clouds: bool,
    /// The early submits whose gaps were stamped (`write_timestamp` between encoders needs the device's
    /// `TIMESTAMP_QUERY_INSIDE_ENCODERS`).
    splits: u32,
    /// The frames' times read without waiting ([`HdWorld::take_finished_gpu_ms`]), and the slot this frame copied into (mapped
    /// after its submit).
    ring: Vec<RingSlot>,
    ring_copied: Option<usize>,
    ring_frames: u64,
}

/// The readback slots of the timer's ring ([`HdWorld::take_finished_gpu_ms`]).
const TIME_RING: usize = 6;

/// A ring slot's state: free, copied into by a submitted frame (not mapped yet), mapping, mapped (to be read), mapping failed.
const SLOT_FREE: u8 = 0;
const SLOT_COPIED: u8 = 1;
const SLOT_MAPPING: u8 = 2;
const SLOT_READY: u8 = 3;
const SLOT_FAILED: u8 = 4;

/// One frame's resolved timestamps on their way back without a wait: the frame's place in submission order and its gaps.
struct RingSlot {
    buffer: wgpu::Buffer,
    state: std::sync::Arc<std::sync::atomic::AtomicU8>,
    frame: u64,
    splits: u32,
}

/// A frame's GPU time from its resolved timestamps: the first pass's start to the tonemap's end, less the GPU's waits for the
/// CPU between the frame's submits (`splits` of them stamped).
fn frame_span_ms(data: &[u8], splits: u32, period_ns: f64) -> Option<f64> {
    let at = |o: usize| -> Option<u64> {
        Some(u64::from_le_bytes(data.get(o..o + 8)?.try_into().ok()?))
    };
    let ms = |a: u64, b: u64| b.saturating_sub(a) as f64 * period_ns / 1e6;
    let r = RESOLVE_ALIGN as usize;
    let mut gaps = 0.0;
    for k in 0..splits as usize {
        gaps += ms(at(8 * r + 16 * k)?, at(8 * r + 16 * k + 8)?);
    }
    Some((ms(at(0)?, at(8)?) - gaps).max(0.0))
}

/// The most pass marks a frame writes.
const MAX_MARKS: u32 = 48;

/// GPU timestamps written between the frame's passes (the device's `TIMESTAMP_QUERY_INSIDE_ENCODERS`), when
/// [`HdWorld::profile_passes`] asks: each mark's time since the one before ([`HdWorld::gpu_passes`]).
struct Marks {
    queries: wgpu::QuerySet,
    resolve: wgpu::Buffer,
    readback: wgpu::Buffer,
    period_ns: f64,
    /// The marks of the frame being encoded (a lock: the draw's closures hold the world shared).
    labels: std::sync::Mutex<Vec<&'static str>>,
    /// The marks of the last frame submitted, not read yet.
    pending: Option<Vec<&'static str>>,
}

impl Marks {
    fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Marks {
        let size = u64::from(MAX_MARKS) * 8;
        Marks {
            queries: device.create_query_set(&wgpu::QuerySetDescriptor {
                label: Some("hd pass marks"),
                ty: wgpu::QueryType::Timestamp,
                count: MAX_MARKS,
            }),
            resolve: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("hd marks resolve"),
                size,
                usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            }),
            readback: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("hd marks readback"),
                size,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            }),
            period_ns: f64::from(queue.get_timestamp_period()),
            labels: std::sync::Mutex::new(Vec::new()),
            pending: None,
        }
    }

    /// A mark after everything encoded so far.
    fn mark(&self, encoder: &mut wgpu::CommandEncoder, label: &'static str) {
        let mut labels = self.labels.lock().unwrap_or_else(|e| e.into_inner());
        if (labels.len() as u32) < MAX_MARKS {
            encoder.write_timestamp(&self.queries, labels.len() as u32);
            labels.push(label);
        }
    }

    /// The frame's marks resolved for reading.
    fn finish(&mut self, encoder: &mut wgpu::CommandEncoder) {
        let labels = std::mem::take(&mut *self.labels.lock().unwrap_or_else(|e| e.into_inner()));
        if labels.len() < 2 {
            return;
        }
        let n = labels.len() as u32;
        encoder.resolve_query_set(&self.queries, 0..n, &self.resolve, 0);
        encoder.copy_buffer_to_buffer(&self.resolve, 0, &self.readback, 0, u64::from(n) * 8);
        self.pending = Some(labels);
    }

    /// The last frame's passes: each mark's label and its milliseconds since the mark before (waits for the GPU).
    fn read(&mut self, device: &wgpu::Device) -> Option<Vec<(&'static str, f64)>> {
        let labels = self.pending.take()?;
        let n = labels.len() as u64;
        let slice = self.readback.slice(..n * 8);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        device.poll(wgpu::PollType::wait_indefinitely()).ok()?;
        rx.recv().ok()?.ok()?;
        let out = {
            let data = slice.get_mapped_range().ok()?;
            let at =
                |i: usize| u64::from_le_bytes(data[i * 8..i * 8 + 8].try_into().unwrap_or([0; 8]));
            (1..labels.len())
                .map(|i| {
                    (
                        labels[i],
                        at(i).saturating_sub(at(i - 1)) as f64 * self.period_ns / 1e6,
                    )
                })
                .collect()
        };
        self.readback.unmap();
        Some(out)
    }
}

/// The texture the tonemap pass reads: the TAA's shown colour (of a parity), the FXAA-style filter's frame, the frame itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Output {
    Shown(usize),
    /// The shown colour (of a parity) with the late sprites added.
    Present(usize),
    Fxaa,
    Hdr,
}

/// A frame's GPU times (ms): the whole frame, its shadow maps' passes, its contact shadows' pass, its AO pass and the AO's filter,
/// its anti-aliasing pass (the TAA resolve or the FXAA-style filter), its reflections (the secondary views and the SSR passes).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct GpuTimes {
    pub frame: f64,
    pub shadow_maps: f64,
    pub contact: f64,
    pub ao: f64,
    pub aa: f64,
    pub reflections: f64,
    /// The volumetric clouds' shadow map and march.
    pub clouds: f64,
}

/// What the enhanced frame draws of its shadows, anti-aliasing and AO (all on by default, each as the quality has it; the benches and
/// checks turn them off to measure their cost and their effect).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HdFeatures {
    pub sun_shadows: bool,
    pub point_shadows: bool,
    pub contact_shadows: bool,
    /// The quality's anti-aliasing (the TAA with its jitter, or the FXAA-style filter); off: neither, no jitter.
    pub anti_aliasing: bool,
    pub ambient_occlusion: bool,
    /// The quality's reflections (SSR, the light capture, the planar reflection, the water drawn as water); off: reflective surfaces
    /// reflect the sky cube and the water is blended as before.
    pub reflections: bool,
    /// The bloom and the volumetric light (each where the image settings and the quality have it).
    pub bloom: bool,
    pub volumetrics: bool,
    /// Weighted blended transparency behind an exact front layer; off: the translucent draws sorted and blended in turn (the old
    /// path; the check's comparison).
    pub wboit: bool,
    /// The tonemap through the grading table; off: the analytic curves (the identity check's reference).
    pub grading_lut: bool,
    /// Coloured sun shadows through translucent cells (where the quality has them).
    pub coloured_shadows: bool,
    /// The extension's fine surface detail and its extra draws, where the image settings and the quality have them.
    pub ext_detail: bool,
    /// The volumetric clouds where the image settings and quality have them (off: the server's flat cloud layers).
    pub clouds: bool,
    /// The volumetric clouds where the image settings only have them opted in (`ImageSettings::clouds_opt_in`: at Ultra
    /// only); off by default.
    pub opt_in_clouds: bool,
    /// The cloud layers allowed (bit i: `torque_render_scene::hd::clouds::FAIR_WEATHER[i]`; the quality's own set within it; the bench's
    /// layer by layer cost).
    pub cloud_layers: u32,
    /// A low sun's light through the clouds as the eye sees it (`torque_render_scene::hd::clouds::CLOUD_SUN`: the sun's light, its glint
    /// and the glow about it dim as a cloud covers it); off: the sun's light ignores the clouds where the shadow map holds nothing.
    pub cloud_sun: bool,
    /// The glowing surfaces' lights (the scene's last `glow_lights`; off: the other lights only, the bench's comparison).
    pub glow_lights: bool,
    /// The fog takes the sky behind a surface without its detail (`sky::FOG_LOW_LEVEL`: no cloud shapes on a fogged wall); off: with
    /// it, as before (the checks' before pictures).
    pub fog_sky_mean: bool,
    /// The clouds marched from the eye wherever it is (in a layer, over it: seen from inside and from above, and laid over a surface
    /// the ray reaches through a layer); off: as before, from no higher than a little under the lowest base.
    pub cloud_from_eye: bool,
    /// The clouds' cube (`clouds::CLOUD_CUBE_SIZE`, a face a frame where the water is drawn): the water reflects the clouds
    /// off screen and the reflections' fog takes them; off: the main view's image alone, its edge stretched past it, as before.
    pub cloud_cube: bool,
}

impl Default for HdFeatures {
    fn default() -> HdFeatures {
        HdFeatures {
            sun_shadows: true,
            point_shadows: true,
            contact_shadows: true,
            anti_aliasing: true,
            ambient_occlusion: true,
            reflections: true,
            bloom: true,
            volumetrics: true,
            wboit: true,
            grading_lut: true,
            coloured_shadows: true,
            ext_detail: true,
            clouds: true,
            opt_in_clouds: false,
            cloud_layers: u32::MAX,
            glow_lights: true,
            cloud_sun: true,
            fog_sky_mean: true,
            cloud_from_eye: true,
            cloud_cube: true,
        }
    }
}

impl HdFeatures {
    pub const NO_SHADOWS: HdFeatures = HdFeatures {
        sun_shadows: false,
        point_shadows: false,
        contact_shadows: false,
        anti_aliasing: true,
        ambient_occlusion: true,
        reflections: true,
        bloom: true,
        volumetrics: true,
        wboit: true,
        grading_lut: true,
        coloured_shadows: true,
        ext_detail: true,
        clouds: true,
        opt_in_clouds: false,
        cloud_layers: u32::MAX,
        glow_lights: true,
        cloud_sun: true,
        fog_sky_mean: true,
        cloud_from_eye: true,
        cloud_cube: true,
    };
    /// No anti-aliasing and no AO.
    pub const NO_POST: HdFeatures = HdFeatures {
        sun_shadows: true,
        point_shadows: true,
        contact_shadows: true,
        anti_aliasing: false,
        ambient_occlusion: false,
        reflections: true,
        bloom: true,
        volumetrics: true,
        wboit: true,
        grading_lut: true,
        coloured_shadows: true,
        ext_detail: true,
        clouds: true,
        opt_in_clouds: false,
        cloud_layers: u32::MAX,
        glow_lights: true,
        cloud_sun: true,
        fog_sky_mean: true,
        cloud_from_eye: true,
        cloud_cube: true,
    };
    /// No reflections.
    pub const NO_REFLECTIONS: HdFeatures = HdFeatures {
        sun_shadows: true,
        point_shadows: true,
        contact_shadows: true,
        anti_aliasing: true,
        ambient_occlusion: true,
        reflections: false,
        bloom: true,
        volumetrics: true,
        wboit: true,
        grading_lut: true,
        coloured_shadows: true,
        ext_detail: true,
        clouds: true,
        opt_in_clouds: false,
        cloud_layers: u32::MAX,
        glow_lights: true,
        cloud_sun: true,
        fog_sky_mean: true,
        cloud_from_eye: true,
        cloud_cube: true,
    };
}

/// Which passes read what earlier frames left (all by default). The ghosting check (`examples/ghosting.rs`) turns each off on
/// its own to measure its share of a trail: off, that pass starts its history again every frame (its per-frame noise still turns).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Histories {
    /// The TAA's history (off: each frame's jittered image as it is).
    pub taa: bool,
    /// The screen-space reflections' history.
    pub ssr: bool,
    /// The volumetric clouds' reprojected history.
    pub clouds: bool,
    /// The automatic exposure's adaptation.
    pub exposure: bool,
}

impl Default for Histories {
    fn default() -> Histories {
        Histories {
            taa: true,
            ssr: true,
            clouds: true,
            exposure: true,
        }
    }
}

/// What the forward pass writes: the shaded frame, or a visibility the shadows' checks read from the HDR target.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DebugView {
    #[default]
    Off,
    /// r: the sun's visibility in use (cascades times contact), g: the cascades', b: the contact shadows' (1 where the sun does not
    /// reach the face at all).
    SunVisibility,
    /// r: the first light's shadow visibility, g: its N.L (where it lights the face).
    PointVisibility,
    /// rgb: the ambient occlusion before the image settings' share of it (1 none) on lit surfaces.
    AmbientOcclusion,
    /// rgb: the reflection's radiance before its Fresnel weight on the reflective surfaces and the water (its rays as a mirror's:
    /// no roughness), black elsewhere (the reflections' checks' picture).
    Reflection,
    /// The extension's own view of this code (5 and up: the extension's chunk reads the frame's debug code).
    Extension(u32),
}

impl DebugView {
    fn code(self) -> u32 {
        match self {
            DebugView::Off => 0,
            DebugView::SunVisibility => 1,
            DebugView::PointVisibility => 2,
            DebugView::AmbientOcclusion => 3,
            DebugView::Reflection => 4,
            DebugView::Extension(code) => code,
        }
    }
}

/// What a frame drew (for the reports).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DrawStats {
    pub draw_calls: usize,
    pub cells_drawn: usize,
    pub cells: usize,
    pub lights: usize,
    /// The extension's extra items drawn in the opaque pass, and those it holds.
    pub extras_drawn: usize,
    pub extras: usize,
    /// Of `extras_drawn`, the ground's.
    pub ground_extras_drawn: usize,
    /// The static clusters in the main view's lists, and those the table holds.
    pub clusters_drawn: usize,
    pub clusters: usize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StaticMemory {
    /// The arena's buffers, what its live slabs hold, and the times it was packed again.
    pub arena_bytes: u64,
    pub slab_bytes: u64,
    pub repacks: u64,
    /// The cluster table's and the main view's lists' bytes, the live clusters, and the clusters a full table refused.
    pub table_bytes: u64,
    pub list_bytes: u64,
    pub clusters: u64,
    pub refused: u64,
}

/// What the levels of detail draw ([`HdWorld::lod_report`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LodReport {
    pub units: [usize; 3],
    pub quads: [u64; 3],
    pub clusters: [usize; 3],
    pub both: usize,
    /// What L0 alone would draw there: its seen cells' opaque quads and clusters (translucent ones too).
    pub l0_quads: u64,
    pub l0_clusters: usize,
}

/// An image read back as floats: width, height, RGBA pixels.
pub type FloatImage = (u32, u32, Vec<[f32; 4]>);

/// Why the enhanced renderer could not draw.
pub type HdError = String;

/// The enhanced renderer's GPU state for one scene: textures, static cells, the other draws, lights, clusters and the targets,
/// with the extension `E` beside them.
pub struct HdWorld<E: HdExtension = NoExtension> {
    device: wgpu::Device,
    /// The device's pipeline cache (Vulkan's; [`torque_render_core::Gpu::pipeline_cache`]), for every pipeline made here.
    pipeline_cache: Option<wgpu::PipelineCache>,
    queue: wgpu::Queue,
    timestamps: bool,
    frame_layout: wgpu::BindGroupLayout,
    clusters_layout: wgpu::BindGroupLayout,
    texture_layout: wgpu::BindGroupLayout,
    tonemap_layout: wgpu::BindGroupLayout,
    forward_layout: wgpu::PipelineLayout,
    /// The forward modules by (clustered, gamma blend).
    forward: HashMap<(bool, bool), wgpu::ShaderModule>,
    clusters_pipeline: wgpu::ComputePipeline,
    tonemap_module: wgpu::ShaderModule,
    tonemap_pipeline_layout: wgpu::PipelineLayout,
    tonemap_pipelines: HashMap<wgpu::TextureFormat, wgpu::RenderPipeline>,
    pipelines: HashMap<PipelineKey, wgpu::RenderPipeline>,
    samplers: HashMap<SamplerKey, wgpu::Sampler>,
    groups: HashMap<(TexRef, SamplerKey), wgpu::BindGroup>,
    textures: Vec<wgpu::TextureView>,
    white: wgpu::TextureView,
    empty: wgpu::TextureView,
    frame: wgpu::Buffer,
    /// The frame's uniform with the camera unjittered (a TAA frame's late over draws).
    frame_still: wgpu::Buffer,
    /// The tonemap pass's uniform.
    post_uniform: wgpu::Buffer,
    lights: wgpu::Buffer,
    lights_capacity: usize,
    light_count: usize,
    masks: wgpu::Buffer,
    clusters_group: wgpu::BindGroup,
    cells: BTreeMap<[i32; 3], GpuCell>,
    /// The levels of detail's blocks on the GPU (levels 1 and 2: blocks of 2 and 4 cells a side), by level and block key.
    lod_units: BTreeMap<(u32, [i32; 3]), GpuCell>,
    /// The blocks with meshes drawn coarse at the last frame's choice (`cull::LOD_STAY_L1`, `cull::LOD_STAY_L2`: the switch's
    /// hysteresis).
    lod_stay: std::collections::BTreeSet<(u32, [i32; 3])>,
    /// Whether the units are the scene's levels (built since they were turned on), and the levels' worker (a window's).
    lod_on: bool,
    lod_work: Option<LodWork<E>>,
    /// The levels' blocks finished and not yet put in (under an upload budget, [`LOD_BYTES_A_FRAME`] of their meshes a frame).
    lod_ready: std::collections::VecDeque<LodDone<E>>,
    /// The static cells' slabs, the cluster table, the main view's draw lists, how they are drawn (and whether a draw may name its
    /// cluster by its first instance), the cells' free table places.
    arena: arena::Arena,
    table: arena::Table,
    main_lists: arena::Lists,
    draw_way: arena::DrawWay,
    first_instance: bool,
    free_cell_ids: std::collections::BTreeSet<u32>,
    next_cell_id: u32,
    /// The cells' boxes for the GPU's cull, and the cull (None where the device cannot bind its buffers).
    cell_table: cull::CellTable,
    cull: Option<cull::Cull>,
    /// The cells in blocks (the CPU's questions about every cell).
    grid: cull::CellGrid,
    /// The main view's lists made by the GPU's cull (on by default where the device draws with indirect counts); off: by the
    /// CPU, from the same test (the reference).
    pub gpu_cull: bool,
    /// Whether this frame's lists are the GPU's, and the lists the other views (the reflections, the shadow maps) cull into in
    /// turn.
    lists_on_gpu: bool,
    scratch_lists: arena::Lists,
    /// The shadow maps' lists, one a map culled in a batch ([`SHADOW_LIST_BYTES`] of them at most, and at least
    /// [`SHADOW_LISTS_MIN`]).
    shadow_lists: Vec<arena::Lists>,
    /// The main view's occlusion in two phases where its lists are the GPU's (on by default): phase 1's and phase 2's
    /// lists, and a count of the targets made (the farthest-depth pyramid follows the depth target).
    pub occlusion: bool,
    /// Whether far cells are drawn at the coarser levels of detail ([`HdExtension::build_level`]: built from the cells as they
    /// change, chosen per block by the error they would show).
    pub lod: bool,
    lists_first: arena::Lists,
    lists_second: arena::Lists,
    targets_made: u64,
    /// The most cell bytes one upload writes (the nearest cells first; the rest at the next upload): a live window's. None:
    /// every changed cell at once (the checks').
    pub upload_budget: Option<u64>,
    /// Whether the last upload left changed cells for the next one.
    uploads_pending: bool,
    /// A dense cell being put in slices: its key and generation, its slab and merged runs, and how many of its vertices and
    /// indices are written (its old slab is drawn until the new one is whole).
    putting: Option<Putting<E::CellRuns>>,
    /// Each cell's (by its id) runs in the extension's tables.
    ext_runs: HashMap<u32, E::CellRuns>,
    /// The time an upload under a budget spends putting cells in (`UPLOAD_TIME`; the tests set none, a slice an upload).
    pub upload_time: std::time::Duration,
    /// The resident cells (`Cell::resident`) the last upload found changed with their meshes already dropped: their GPU copies
    /// stay as they were (the mesh must be kept until it is uploaded).
    pub resident_stale: usize,
    slots: BTreeMap<u32, (TexRef, SamplerKey)>,
    loose: Loose,
    /// Where the stamped moving shapes' vertices stay in the loose buffers.
    runs: slots::Runs,
    /// The frame's meshes' places (`torque_render_scene::hd::NodePlace`, [`NODE_BYTES`] each: the vertex shaders' `nodes`), the stamped
    /// objects' runs of them, and the first place each stamped object's vertices were written with.
    nodes: wgpu::Buffer,
    node_runs: slots::Runs,
    node_baked: HashMap<u32, usize>,
    /// The stamped objects' runs of indices at the loose index buffer's start (kept, as their vertices' runs, while their stamps
    /// are: their draws' indices are their own, not where their vertices are).
    index_runs: slots::Runs,
    /// The previous positions' buffer while an upload needs none.
    spare_previous: Option<wgpu::Buffer>,
    particles: Particles,
    targets: Option<Targets>,
    timer: Option<Timer>,
    times: Option<GpuTimes>,
    shadows: Shadows,
    prepass_layout: wgpu::PipelineLayout,
    /// The prepass's pipelines by cull and whether they draw static cells (still: motion slot 0; a shape's draw reads its slot by its
    /// instance index).
    prepass: HashMap<(Cull, bool), wgpu::RenderPipeline>,
    contact_layout: wgpu::BindGroupLayout,
    contact_pipeline: wgpu::RenderPipeline,
    /// The lights as uploaded (their shadow slots patched in each frame) and the slots last written.
    light_bytes: Vec<u8>,
    /// Per light, the static geometry's owners it is mounted in ([`HdExtension::light_hosts`]: left out of its shadow cube).
    light_hosts: Vec<[u32; 4]>,
    written_slots: Option<Vec<[f32; 4]>>,
    /// The lights as uploaded (the scene's point lights and spots in the buffer's order) and each one's packed cone
    /// ([`HdSpot::cone`]; zeros: a point light).
    frame_lights: Vec<HdLight>,
    light_cones: Vec<[f32; 4]>,
    /// The boxes of the cells changed since the last frame (old and new), or every cached shadow to be drawn again.
    shadow_dirty: Vec<[[f32; 3]; 2]>,
    shadow_reset: bool,
    /// The ground's draws' boxes (the scene's planes and water: the shadow range reaches over them), and the draws they were
    /// taken from (each one's vertex list by address and length, and the planes' generation): taken again only when those change.
    ground_boxes: Vec<[[f32; 3]; 2]>,
    ground_key: Vec<[usize; 3]>,
    /// The boxes the last frame's shadow caches were patched in (the reports').
    pub shadow_patched: usize,
    /// The last upload's steps' times (ms), and every upload's since [`HdWorld::take_upload_steps`] summed by step (a frame may
    /// upload more than once: the cells, then the control object).
    pub upload_times: Vec<(&'static str, f64)>,
    upload_steps: Vec<(&'static str, f64)>,
    /// The times an upload's changes were too many to patch and every cached shadow map was drawn again (the reports').
    pub shadow_resets: u64,
    /// The anti-aliasing's and the AO's pipelines and the motion matrices; what carries over between frames.
    post: PostPipes,
    temporal: Temporal,
    /// What the last frame's tonemap read.
    output: Output,
    /// The brute-force light loop instead of the clusters (the clusters check's reference).
    pub brute_force_lights: bool,
    pub features: HdFeatures,
    /// The histories the passes keep ([`Histories`]: all by default).
    pub histories: Histories,
    pub debug: DebugView,
    /// What the last frame drew.
    pub stats: DrawStats,
    /// The sky (uniform, tables, passes), the bloom chain, the automatic exposure, the volumetric light.
    sky: sky::SkyGpu,
    bloom: bloom::Bloom,
    exposure: exposure::Exposure,
    volumetric: volumetric::Volumetric,
    /// The volumetric clouds.
    clouds: clouds::Clouds,
    /// The seconds a frame stands for (the automatic exposure's adaptation): 1/60 unless the caller measures its frames.
    pub frame_seconds: f32,
    /// Automatic exposure, in place of the image settings' fixed one (off by default: a scene keeps its own colours); the
    /// exposure check turns it on.
    pub auto_exposure: bool,
    /// Film grain's strength (0 to 1; 0, the default, none).
    pub grain: f32,
    /// The share of the output's pixels drawn ([`RenderScale`]; a window's setting, `Auto` for the checks).
    pub render_scale: RenderScale,
    /// The reflections.
    refl: reflect::Reflections,
    /// A plane (its height) the planar reflection is drawn for whether or not the scene has water (the SSR check's reference).
    pub planar_override: Option<f32>,
    /// Reflected rays as a mirror's (no roughness; the reflections' checks).
    pub mirror_reflections: bool,
    /// Every face of the reflection capture drawn every frame from this frame's eye (the reference of the reflections' steadiness
    /// check; the window draws one face a frame).
    pub capture_every_frame: bool,
    /// Each moving draw of the prepass binds its motion slot at its own dynamic offset and draws instance 0 (the reference of the
    /// motion-by-instance check; off, a draw's slot is its instance index and the slots are bound once a pass).
    pub motion_per_draw: bool,
    /// The grading table baked with the identity grade (the identity check).
    pub identity_grade: bool,
    /// The weighted blend's composite, its layout and uniform; the grading table, the image settings and user table it was baked for,
    /// the user's `.cube`; a depth stand-in for the frame group's front-layer slot.
    wboit_pipeline: wgpu::RenderPipeline,
    comp_layout: wgpu::BindGroupLayout,
    comp_uniform: wgpu::Buffer,
    /// The late sprites' composite after the TAA and its layout.
    late_pipeline: wgpu::RenderPipeline,
    late_layout: wgpu::BindGroupLayout,
    lut_texture: wgpu::Texture,
    lut_view: wgpu::TextureView,
    lut_key: Option<(ImageSettings, u64, bool)>,
    user_lut: Option<CubeLut>,
    user_lut_generation: u64,
    dummy_depth: wgpu::TextureView,
    /// The extension's buffers in the renderer's groups.
    ext_slots: ExtSlots,
    /// The extension: what the world's static geometry brings beside the engine's frame.
    pub ext: E,
    /// The GPU time between the frame's passes (the device needs
    /// `TIMESTAMP_QUERY_INSIDE_ENCODERS`): [`HdWorld::gpu_passes`].
    pub profile_passes: bool,
    /// The CPU time between the marks without the GPU's timestamps (the window's frame breakdown): [`HdWorld::take_cpu_passes`].
    pub cpu_marks: bool,
    marks: Option<Marks>,
    /// The CPU time of the frame's encoding between the same marks (summed over the frames since the last
    /// [`HdWorld::take_cpu_passes`], with `profile_passes` or `cpu_marks`), and the frames summed.
    cpu_passes: Vec<(&'static str, f64)>,
    cpu_frames: u64,
    /// The render pipelines made so far (a frame that makes one waits for its shader's compile).
    pub pipelines_made: u64,
    /// The effects' pipelines being made on a worker thread ([`HdWorld::warm_effects`]).
    warming: Option<Warming>,
    /// In a browser: the warm-up's pipelines still to be made, [`WEB_WARM_PER_FRAME`] a frame ([`HdWorld::warm_effects`]).
    warm_queue: Vec<PipelineKey>,
    /// The scene's generation and control generation at the last upload.
    uploaded: Option<(u64, u64)>,
    /// The scene's lights generation last uploaded, whether with the glow lights, and how many of them may cast shadows.
    lights_generation: Option<u64>,
    uploaded_glow: bool,
    shadowed_lights: usize,
}

/// The shadow maps' lists' bytes at most (a batch of maps culled in one compute pass, each into its own lists), and the fewest and
/// most lists (a map past a batch's lists is culled by the CPU, as one past the frame's view slots is).
const SHADOW_LIST_BYTES: u64 = 16 << 20;
const SHADOW_LISTS_MIN: usize = 2;
const SHADOW_LISTS_MAX: usize = 32;

/// The shadow maps' lists for a table of `slots`.
fn shadow_lists_for(slots: u32) -> usize {
    let each = u64::from(slots.max(1)) * 20 + u64::from(arena::MAX_SEGMENTS) * 12;
    ((SHADOW_LIST_BYTES / each) as usize).clamp(SHADOW_LISTS_MIN, SHADOW_LISTS_MAX)
}

/// A pass into the HDR frame `color` (cleared to `clear`, or loaded) over `depth` (loaded and kept).
fn hdr_pass<'e>(
    encoder: &'e mut wgpu::CommandEncoder,
    label: &'static str,
    clear: Option<[f64; 3]>,
    color: &wgpu::TextureView,
    depth: &wgpu::TextureView,
) -> wgpu::RenderPass<'e> {
    crate::count_pass();
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: color,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: match clear {
                    Some(c) => wgpu::LoadOp::Clear(wgpu::Color {
                        r: c[0],
                        g: c[1],
                        b: c[2],
                        a: 1.0,
                    }),
                    None => wgpu::LoadOp::Load,
                },
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

/// Sets a forward pass's frame (0), sky (2) and reflection (3) groups.
fn set_groups(pass: &mut wgpu::RenderPass<'_>, groups: [&wgpu::BindGroup; 3]) {
    pass.set_bind_group(0, groups[0], &[]);
    pass.set_bind_group(2, groups[1], &[]);
    pass.set_bind_group(3, groups[2], &[]);
}

/// A weighted blend pass's pipeline of the forward `module` for `cull`: the front layer's depth (depth only, nearer wins),
/// the weighted blend (`fs_wboit` into the accumulation, added, and the revealage, multiplied by 1 - alpha; tested, not written), or
/// the front layer (`fs_main` alpha-blended where the depth equals the front's).
pub(crate) fn trans_pipeline(
    device: &wgpu::Device,
    cache: Option<&wgpu::PipelineCache>,
    layout: &wgpu::PipelineLayout,
    module: &wgpu::ShaderModule,
    cull: Cull,
    stage: Stage,
) -> wgpu::RenderPipeline {
    let (cull_mode, front_face) = match cull {
        Cull::None => (None, wgpu::FrontFace::Cw),
        Cull::BackCw => (Some(wgpu::Face::Back), wgpu::FrontFace::Cw),
        Cull::BackCcw => (Some(wgpu::Face::Back), wgpu::FrontFace::Ccw),
    };
    let add = wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::One,
        dst_factor: wgpu::BlendFactor::One,
        operation: wgpu::BlendOperation::Add,
    };
    let reveal = wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::Zero,
        dst_factor: wgpu::BlendFactor::OneMinusSrc,
        operation: wgpu::BlendOperation::Add,
    };
    let alpha = blend_state(Blend::Alpha);
    let targets: Vec<Option<wgpu::ColorTargetState>> = match stage {
        Stage::FrontDepth | Stage::Main | Stage::Particle | Stage::Over => Vec::new(),
        Stage::Tail => vec![
            Some(wgpu::ColorTargetState {
                format: ACCUM_FORMAT,
                blend: Some(wgpu::BlendState {
                    color: add,
                    alpha: add,
                }),
                write_mask: wgpu::ColorWrites::ALL,
            }),
            Some(wgpu::ColorTargetState {
                format: REVEAL_FORMAT,
                blend: Some(wgpu::BlendState {
                    color: reveal,
                    alpha: reveal,
                }),
                write_mask: wgpu::ColorWrites::ALL,
            }),
        ],
        Stage::Front => vec![Some(wgpu::ColorTargetState {
            format: HDR_FORMAT,
            blend: alpha,
            write_mask: wgpu::ColorWrites::ALL,
        })],
    };
    let (compare, write) = match stage {
        Stage::FrontDepth | Stage::Main | Stage::Particle | Stage::Over => {
            (wgpu::CompareFunction::GreaterEqual, true)
        }
        Stage::Tail => (wgpu::CompareFunction::GreaterEqual, false),
        Stage::Front => (wgpu::CompareFunction::Equal, false),
    };
    let entry = match stage {
        Stage::Tail => Some("fs_wboit"),
        Stage::Front => Some("fs_main"),
        _ => None,
    };
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("hd translucent"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module,
            entry_point: Some("vs_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: &[Some(wgpu::VertexBufferLayout {
                array_stride: HD_VERTEX_BYTES as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &wgpu::vertex_attr_array![
                    0 => Float32x3, 1 => Snorm16x2, 2 => Float32x2, 3 => Unorm8x4, 4 => Uint32, 5 => Uint32
                ],
            })],
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            strip_index_format: None,
            front_face,
            cull_mode,
            unclipped_depth: false,
            polygon_mode: wgpu::PolygonMode::Fill,
            conservative: false,
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(write),
            depth_compare: Some(compare),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState::default(),
        fragment: entry.map(|e| wgpu::FragmentState {
            module,
            entry_point: Some(e),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &targets,
        }),
        multiview_mask: None,
        cache,
    })
}

/// A float as IEEE half (round to nearest even; the grading table's upload).
pub fn f32_to_f16(v: f32) -> u16 {
    let b = v.to_bits();
    let sign = ((b >> 16) & 0x8000) as u16;
    let exp = ((b >> 23) & 0xff) as i32;
    let mant = b & 0x7f_ffff;
    if exp == 0xff {
        return sign | 0x7c00 | if mant != 0 { 0x200 } else { 0 };
    }
    let e = exp - 127 + 15;
    if e >= 0x1f {
        return sign | 0x7c00;
    }
    if e <= 0 {
        if e < -10 {
            return sign;
        }
        let m = mant | 0x80_0000;
        let shift = (14 - e) as u32;
        let half = 1u32 << (shift - 1);
        let rounded = (m + half - 1 + ((m >> shift) & 1)) >> shift;
        return sign | rounded as u16;
    }
    let half_m = mant >> 13;
    let rest = mant & 0x1fff;
    let mut h = (u32::from(sign) | ((e as u32) << 10) | half_m) as u16;
    if rest > 0x1000 || (rest == 0x1000 && (half_m & 1) == 1) {
        h += 1;
    }
    h
}

/// The forward pass's pipeline for `key`: of `module` (the forward module of its `clustered` and `front_layer`), the stage's
/// fragment entry.
fn make_pipeline(
    device: &wgpu::Device,
    cache: Option<&wgpu::PipelineCache>,
    layout: &wgpu::PipelineLayout,
    module: &wgpu::ShaderModule,
    key: PipelineKey,
) -> wgpu::RenderPipeline {
    match key.stage {
        Stage::Main if key.blend == Blend::Modulate2x => {
            forward_pipeline(device, cache, layout, module, key, "fs_modulate2x")
        }
        Stage::Main | Stage::Over => {
            forward_pipeline(device, cache, layout, module, key, "fs_main")
        }
        Stage::Particle => forward_pipeline(device, cache, layout, module, key, "fs_particle"),
        s => trans_pipeline(device, cache, layout, module, key.cull, s),
    }
}

/// The forward pass's states the effects are drawn with: particles, ribbons, bolt trails, decals and tire tracks are two-sided,
/// depth-tested and not written, added (their late pass's own entry too), alpha-blended (the translucent pass's front layer and
/// its weighted-blend stages) or multiplied over the surface.
pub(crate) fn effect_keys(clustered: bool) -> Vec<PipelineKey> {
    let key = |blend, front_layer, stage| PipelineKey {
        blend,
        cull: Cull::None,
        depth_test: true,
        depth_write: false,
        clustered,
        front_layer,
        stage,
    };
    let mut keys = Vec::new();
    for blend in [Blend::Additive, Blend::One] {
        keys.push(key(blend, false, Stage::Main));
        keys.push(key(blend, false, Stage::Particle));
    }
    keys.push(key(Blend::Alpha, false, Stage::Main));
    for stage in [Stage::Main, Stage::FrontDepth, Stage::Tail, Stage::Front] {
        keys.push(key(Blend::Alpha, true, stage));
    }
    keys.push(key(Blend::Modulate2x, false, Stage::Main));
    keys
}

/// The effects' pipelines a frame makes of those a browser's warm-up queued ([`HdWorld::warm_effects`]).
pub const WEB_WARM_PER_FRAME: usize = 2;

/// The warm-up's thread and the pipelines it makes ([`HdWorld::warm_effects`]). Dropped unfinished (the world dropped, the
/// window closed), it tells the thread to stop and waits for the pipeline it is making: a process that ends while a driver
/// compiles on another thread can wait seconds in the driver's shutdown, where Ctrl-C cannot end it either.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
struct Warming {
    keys: Vec<PipelineKey>,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<Vec<(PipelineKey, wgpu::RenderPipeline)>>>,
}

impl Warming {
    /// Whether the thread is done (or there is none).
    fn finished(&self) -> bool {
        self.thread.as_ref().is_none_or(|t| t.is_finished())
    }

    /// The pipelines made, waiting for the thread (none when it panicked).
    fn join(mut self) -> Vec<(PipelineKey, wgpu::RenderPipeline)> {
        self.thread
            .take()
            .and_then(|t| t.join().ok())
            .unwrap_or_default()
    }
}

impl Drop for Warming {
    fn drop(&mut self) {
        if let Some(t) = self.thread.take() {
            self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
            let _ = t.join();
        }
    }
}

/// A forward-pass pipeline of `module`'s `entry` for `key` (HDR target, the forward layout).
pub(crate) fn forward_pipeline(
    device: &wgpu::Device,
    cache: Option<&wgpu::PipelineCache>,
    layout: &wgpu::PipelineLayout,
    module: &wgpu::ShaderModule,
    key: PipelineKey,
    entry: &str,
) -> wgpu::RenderPipeline {
    let (cull_mode, front_face) = match key.cull {
        Cull::None => (None, wgpu::FrontFace::Cw),
        Cull::BackCw => (Some(wgpu::Face::Back), wgpu::FrontFace::Cw),
        Cull::BackCcw => (Some(wgpu::Face::Back), wgpu::FrontFace::Ccw),
    };
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("hd forward"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module,
            entry_point: Some("vs_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: &[Some(wgpu::VertexBufferLayout {
                array_stride: HD_VERTEX_BYTES as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &wgpu::vertex_attr_array![
                    0 => Float32x3, 1 => Snorm16x2, 2 => Float32x2, 3 => Unorm8x4, 4 => Uint32, 5 => Uint32
                ],
            })],
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            strip_index_format: None,
            front_face,
            cull_mode,
            unclipped_depth: false,
            polygon_mode: wgpu::PolygonMode::Fill,
            conservative: false,
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(key.depth_write && key.stage != Stage::Over),
            depth_compare: Some(if key.depth_test {
                wgpu::CompareFunction::GreaterEqual
            } else {
                wgpu::CompareFunction::Always
            }),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module,
            entry_point: Some(entry),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: HDR_FORMAT,
                // The late frame's over draws keep their coverage in the alpha (premultiplied colour).
                blend: if key.stage == Stage::Over {
                    Some(wgpu::BlendState {
                        color: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::SrcAlpha,
                            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                            operation: wgpu::BlendOperation::Add,
                        },
                        alpha: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::One,
                            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                            operation: wgpu::BlendOperation::Add,
                        },
                    })
                } else {
                    blend_state(key.blend)
                },
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache,
    })
}

fn blend_state(b: Blend) -> Option<wgpu::BlendState> {
    let comp = |src, dst| wgpu::BlendComponent {
        src_factor: src,
        dst_factor: dst,
        operation: wgpu::BlendOperation::Add,
    };
    let both = |c: wgpu::BlendComponent| Some(wgpu::BlendState { color: c, alpha: c });
    use wgpu::BlendFactor as F;
    match b {
        Blend::Opaque => None,
        Blend::Alpha => both(comp(F::SrcAlpha, F::OneMinusSrcAlpha)),
        Blend::Additive => both(comp(F::SrcAlpha, F::One)),
        Blend::One => both(comp(F::One, F::One)),
        Blend::Multiply => both(comp(F::Dst, F::OneMinusSrcAlpha)),
        // The destination's alpha kept (a light capture's face keeps a distance there).
        Blend::Modulate2x => Some(wgpu::BlendState {
            color: comp(F::Dst, F::Src),
            alpha: comp(F::Zero, F::One),
        }),
    }
}

/// The share of the visible distance a Sky with no fog before it fogs (the far plane's cut hidden).
pub const FAR_FOG_SHARE: f32 = 0.35;

/// The fog's start and end as the frame draws them: the Sky's, or the last [`FAR_FOG_SHARE`] of the visible distance when its fog
/// starts at or past it.
pub fn fog_range(start: f32, end: f32) -> (f32, f32) {
    if start < end {
        (start, end)
    } else {
        (end * (1.0 - FAR_FOG_SHARE), end)
    }
}

/// A block of a level of detail to build: its cells' inputs and its ticket (only the latest build of a block is put in).
struct LodJob<X: HdExtension> {
    level: u32,
    block: [i32; 3],
    inputs: Vec<X::LevelInput>,
    ticket: u64,
}

/// A built block: its mesh where it pays ([`HdExtension::build_level`]).
struct LodDone<X: HdExtension> {
    level: u32,
    block: [i32; 3],
    mesh: Option<ExtCell<X>>,
    ticket: u64,
}

impl<X: HdExtension> LodJob<X> {
    fn run(self) -> LodDone<X> {
        LodDone {
            level: self.level,
            block: self.block,
            mesh: X::build_level(self.level, self.block, self.inputs),
            ticket: self.ticket,
        }
    }
}

/// The film grain's seed ([`HdWorld::grain`]): one pattern held still, whatever the effect time.
pub const GRAIN_SEED: f32 = 0.0;

/// The levels of detail's worker: a thread building the blocks it is sent (it ends when the world drops its side), and the
/// latest ticket of each block waiting.
struct LodWork<X: HdExtension> {
    jobs: std::sync::mpsc::Sender<LodJob<X>>,
    done: std::sync::mpsc::Receiver<LodDone<X>>,
    latest: HashMap<(u32, [i32; 3]), u64>,
    next: u64,
    /// In a browser (no threads): the jobs' side, built on the calling thread when the results are looked for ([`LodWork::pump`]).
    #[cfg(target_arch = "wasm32")]
    here: (
        std::sync::mpsc::Receiver<LodJob<X>>,
        std::sync::mpsc::Sender<LodDone<X>>,
    ),
}

impl<X: HdExtension> LodWork<X> {
    fn spawn() -> LodWork<X> {
        let (jobs, rx) = std::sync::mpsc::channel::<LodJob<X>>();
        let (tx, done) = std::sync::mpsc::channel::<LodDone<X>>();
        #[cfg(not(target_arch = "wasm32"))]
        std::thread::spawn(move || {
            for job in rx {
                if tx.send(job.run()).is_err() {
                    break;
                }
            }
        });
        LodWork {
            jobs,
            done,
            latest: HashMap::new(),
            next: 0,
            #[cfg(target_arch = "wasm32")]
            here: (rx, tx),
        }
    }

    /// In a browser: the jobs sent so far, built now.
    fn pump(&self) {
        #[cfg(target_arch = "wasm32")]
        while let Ok(job) = self.here.0.try_recv() {
            let _ = self.here.1.send(job.run());
        }
    }
}

/// The most time one upload under a budget spends putting cells in (the rest at the next).
const UPLOAD_TIME: std::time::Duration = std::time::Duration::from_millis(3);

/// A live window's cell of more than this many vertices is put in slices of it across uploads (a dense cell's whole mesh was
/// written in one frame: 49 ms of one).
const SLICE_VERTICES: usize = 1 << 16;

/// A cell's clusters, or its batches as clusters when it was built without (each drawn as one).
fn cell_clusters<X: HdExtension>(
    c: &ExtCell<X>,
) -> Vec<torque_render_scene::hd::clusters::Cluster> {
    if c.clusters.is_empty() {
        c.batches
            .iter()
            .map(|b| torque_render_scene::hd::clusters::Cluster {
                slot: b.slot,
                translucent: b.translucent,
                first: b.first,
                count: b.count,
                bounds: c.bounds,
            })
            .collect()
    } else {
        c.clusters.clone()
    }
}

/// A cell being put in slices ([`HdWorld::putting`]).
struct Putting<R> {
    key: [i32; 3],
    generation: u64,
    slab: u32,
    runs: R,
    vertices: usize,
    indices: usize,
    /// The cell's indices as 16 bits, each run relative to its first vertex, and the runs (cluster, first index, count,
    /// first vertex).
    short: Vec<u16>,
    runs_of_indices: Vec<IndexRun>,
}

/// A run of a cell's 16-bit indices: its cluster, first index, count and first vertex.
type IndexRun = (usize, u32, u32, u32);

/// A cell's clusters (its batches when it has none) cut into runs of indices whose vertices span at most 65,536 (a cluster's
/// do: at most 1,024 quads), each run's indices less its first vertex: (cluster, first index, count, first vertex), and the
/// 16-bit indices. A triangle whose own vertices span more is left out (none of the cells' do).
fn short_indices(
    indices: &[u32],
    clusters: &[torque_render_scene::hd::clusters::Cluster],
) -> (Vec<u16>, Vec<IndexRun>) {
    const RUN: u32 = 6 * 1024;
    let mut short = vec![0u16; indices.len()];
    let mut runs = Vec::new();
    for (k, cl) in clusters.iter().enumerate() {
        let end = (cl.first + cl.count).min(indices.len() as u32);
        let mut at = cl.first;
        while at + 3 <= end {
            let mut n = (end - at).min(RUN);
            n -= n % 3;
            loop {
                let part = &indices[at as usize..(at + n) as usize];
                let lo = part.iter().copied().min().unwrap_or(0);
                let hi = part.iter().copied().max().unwrap_or(0);
                if u16::try_from(hi - lo).is_ok() {
                    for (j, x) in part.iter().enumerate() {
                        short[at as usize + j] = (x - lo) as u16;
                    }
                    runs.push((k, at, n, lo));
                    break;
                }
                if n <= 3 {
                    break;
                }
                n = (n / 2 - (n / 2) % 3).max(3);
            }
            at += n;
        }
    }
    (short, runs)
}

/// Off: the levels' switches hold no hysteresis (the checks' before; `cull::LOD_HYSTERESIS`).
pub static LOD_HOLD: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

/// The block of level `level` (1 or 2: blocks of 2 and 4 cells a side) holding the cell `cell`.
fn block_of(cell: [i32; 3], level: u32) -> [i32; 3] {
    cell.map(|k| k.div_euclid(1 << level))
}

/// The cells of a block of level `level` (their keys).
fn block_cells(block: [i32; 3], level: u32) -> impl Iterator<Item = [i32; 3]> {
    let n = 1 << level;
    (0..n * n * n).map(move |i| {
        [
            block[0] * n + i % n,
            block[1] * n + (i / n) % n,
            block[2] * n + i / (n * n),
        ]
    })
}

fn one_texel(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &str,
    rgba: [u8; 4],
) -> wgpu::TextureView {
    let t = device.create_texture_with_data(
        queue,
        &wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[wgpu::TextureFormat::Rgba8UnormSrgb],
        },
        wgpu::util::TextureDataOrder::LayerMajor,
        &rgba,
    );
    t.create_view(&wgpu::TextureViewDescriptor {
        format: Some(wgpu::TextureFormat::Rgba8UnormSrgb),
        ..wgpu::TextureViewDescriptor::default()
    })
}

fn texture_entry(
    binding: u32,
    sample_type: wgpu::TextureSampleType,
    view_dimension: wgpu::TextureViewDimension,
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

/// The number of clusters.
fn cluster_count() -> u32 {
    CLUSTER_GRID[0] * CLUSTER_GRID[1] * CLUSTER_GRID[2]
}

/// The frame group's bindings (group 0 of the forward, sky and effect pipelines).
pub(crate) fn frame_entries() -> Vec<wgpu::BindGroupLayoutEntry> {
    let vf = wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT;
    vec![
        uniform(0, vf | wgpu::ShaderStages::COMPUTE),
        storage(1, true, wgpu::ShaderStages::FRAGMENT),
        storage(2, true, wgpu::ShaderStages::FRAGMENT),
        texture_entry(
            3,
            wgpu::TextureSampleType::Float { filterable: false },
            wgpu::TextureViewDimension::D2,
        ),
        uniform(4, wgpu::ShaderStages::FRAGMENT),
        texture_entry(
            5,
            wgpu::TextureSampleType::Depth,
            wgpu::TextureViewDimension::D2Array,
        ),
        wgpu::BindGroupLayoutEntry {
            binding: 6,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
            count: None,
        },
        wgpu::BindGroupLayoutEntry {
            binding: 7,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::NonFiltering),
            count: None,
        },
        texture_entry(
            8,
            wgpu::TextureSampleType::Depth,
            wgpu::TextureViewDimension::CubeArray,
        ),
        texture_entry(
            9,
            wgpu::TextureSampleType::Float { filterable: false },
            wgpu::TextureViewDimension::D2,
        ),
        texture_entry(
            10,
            wgpu::TextureSampleType::Float { filterable: false },
            wgpu::TextureViewDimension::D2,
        ),
        // The translucent front layer's depth, the cascades' transmittance and its sampler.
        texture_entry(
            11,
            wgpu::TextureSampleType::Depth,
            wgpu::TextureViewDimension::D2,
        ),
        texture_entry(
            12,
            wgpu::TextureSampleType::Float { filterable: true },
            wgpu::TextureViewDimension::D2Array,
        ),
        wgpu::BindGroupLayoutEntry {
            binding: 13,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        },
        // The extension's slots ([`ExtSlots::forward`]).
        storage(14, true, wgpu::ShaderStages::FRAGMENT),
        storage(15, true, wgpu::ShaderStages::FRAGMENT),
        // The meshes' places the shapes' vertices in their meshes' own space name (torque_render_scene::hd::NodePlace).
        storage(16, true, wgpu::ShaderStages::VERTEX),
        // The point lights' cubes of the second and third tiers (the first is binding 8).
        texture_entry(
            17,
            wgpu::TextureSampleType::Depth,
            wgpu::TextureViewDimension::CubeArray,
        ),
        texture_entry(
            18,
            wgpu::TextureSampleType::Depth,
            wgpu::TextureViewDimension::CubeArray,
        ),
    ]
}

/// The texture group's bindings (group 1 of the forward and sky pipelines).
pub(crate) fn texture_entries() -> Vec<wgpu::BindGroupLayoutEntry> {
    vec![
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
    ]
}

impl<E: HdExtension> HdWorld<E> {
    /// The enhanced renderer on `gpu`'s device (its queue and the optional features it asked for). Panics with
    /// [`crate::limits::LimitError`]'s message where the device cannot run it ([`HdWorld::try_new`]).
    pub fn new(gpu: &torque_render_core::Gpu) -> HdWorld<E> {
        HdWorld::<E>::try_new(gpu).unwrap_or_else(|e| panic!("{e}"))
    }

    /// [`HdWorld::new`], or the limits the device falls short of for the enhanced renderer (nothing is made then): a caller can
    /// draw with a renderer that needs less.
    pub fn try_new(gpu: &torque_render_core::Gpu) -> Result<HdWorld<E>, crate::limits::LimitError> {
        crate::limits::check(&gpu.limits())?;
        // The clouds' noise is made once a process (half a second): begun now beside the caller, so the first frame that draws
        // clouds finds it made (or waits only for the rest of it) instead of making it then.
        // Where no thread can be started (a browser) the frames that want clouds make it a share at a time
        // ([`HdWorld::clouds_waiting`]).
        if !torque_render_scene::hd::clouds::noise_ready() {
            let _ = std::thread::Builder::new()
                .name("cloud noise".into())
                .spawn(|| {
                    let _ = torque_render_scene::hd::clouds::noise();
                });
        }
        HdWorld::<E>::try_from_device(
            gpu.device().clone(),
            gpu.queue().clone(),
            gpu.pipeline_cache().cloned(),
        )
    }

    /// [`HdWorld::new`] on a device and queue (a window may build one on a worker thread), its pipelines made with
    /// `pipeline_cache` (the device's: [`torque_render_core::Gpu::pipeline_cache`]).
    /// Panics with [`crate::limits::LimitError`]'s message where the device cannot run it ([`HdWorld::try_from_device`]).
    pub fn from_device(
        device: wgpu::Device,
        queue: wgpu::Queue,
        pipeline_cache: Option<wgpu::PipelineCache>,
    ) -> HdWorld<E> {
        HdWorld::<E>::try_from_device(device, queue, pipeline_cache)
            .unwrap_or_else(|e| panic!("{e}"))
    }

    /// [`HdWorld::from_device`], or the limits the device falls short of ([`crate::limits::check`]).
    pub fn try_from_device(
        device: wgpu::Device,
        queue: wgpu::Queue,
        pipeline_cache: Option<wgpu::PipelineCache>,
    ) -> Result<HdWorld<E>, crate::limits::LimitError> {
        crate::limits::check(&device.limits())?;
        let cache = pipeline_cache.as_ref();
        // The modules are composed with the extension's chunks.
        let chunks = E::shaders();
        assert!(
            shaders::register(chunks) || shaders::extension() == chunks,
            "the shaders were composed with another extension before this one was registered"
        );
        let timestamps = device.features().contains(wgpu::Features::TIMESTAMP_QUERY);
        let frame_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("hd frame"),
            entries: &frame_entries(),
        });
        let contact_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("hd contact"),
            entries: &[
                uniform(0, wgpu::ShaderStages::FRAGMENT),
                texture_entry(
                    1,
                    wgpu::TextureSampleType::Depth,
                    wgpu::TextureViewDimension::D2,
                ),
                uniform(2, wgpu::ShaderStages::FRAGMENT),
            ],
        });
        let clusters_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("hd clusters"),
            entries: &[
                uniform(0, wgpu::ShaderStages::COMPUTE),
                storage(1, true, wgpu::ShaderStages::COMPUTE),
                storage(2, false, wgpu::ShaderStages::COMPUTE),
            ],
        });
        let texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("hd texture"),
            entries: &texture_entries(),
        });
        let tonemap_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("hd tonemap"),
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
                uniform(1, wgpu::ShaderStages::FRAGMENT),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                // The bloom chain's result and the automatic exposure's state.
                texture_entry(
                    3,
                    wgpu::TextureSampleType::Float { filterable: true },
                    wgpu::TextureViewDimension::D2,
                ),
                storage(4, true, wgpu::ShaderStages::FRAGMENT),
                // The grading table.
                texture_entry(
                    5,
                    wgpu::TextureSampleType::Float { filterable: true },
                    wgpu::TextureViewDimension::D3,
                ),
            ],
        });
        let refl = reflect::Reflections::new(&device, cache, frame_size::<E>() as u64);
        let mut clouds =
            clouds::Clouds::new(&device, cache, &frame_layout, frame_size::<E>() as u64);
        let sky = sky::SkyGpu::new(
            &device,
            cache,
            &frame_layout,
            &texture_layout,
            clouds.views(),
        );
        clouds.set_sky_layout(&device, cache, &sky.layout);
        let forward_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("hd forward"),
            bind_group_layouts: &[
                Some(&frame_layout),
                Some(&texture_layout),
                // The sky (group 2; the prepass's motion matrix is group 2's binding 0 in its own layout).
                Some(&sky.layout),
                Some(&refl.layout),
            ],
            immediate_size: 0,
        });
        let clusters_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("hd clusters"),
                bind_group_layouts: &[Some(&clusters_layout)],
                immediate_size: 0,
            });
        let tonemap_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("hd tonemap"),
                bind_group_layouts: &[Some(&tonemap_layout)],
                immediate_size: 0,
            });
        let clusters_module = module(&device, Permutation::Clusters);
        let clusters_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("hd clusters"),
            layout: Some(&clusters_pipeline_layout),
            module: &clusters_module,
            entry_point: Some("cs_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache,
        });
        let frame = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("hd frame"),
            size: frame_size::<E>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let frame_still = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("hd frame unjittered"),
            size: frame_size::<E>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let post_uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("hd post"),
            size: POST_BYTES,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let masks = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("hd cluster masks"),
            size: u64::from(cluster_count()) * CLUSTER_WORDS * 4,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        let lights_capacity = 64;
        let lights = Self::lights_buffer(&device, lights_capacity);
        let clusters_group =
            Self::clusters_group(&device, &clusters_layout, &frame, &lights, &masks);
        let timer = timestamps.then(|| Timer {
            queries: device.create_query_set(&wgpu::QuerySetDescriptor {
                label: Some("hd frame time"),
                ty: wgpu::QueryType::Timestamp,
                count: QUERIES,
            }),
            resolve: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("hd time resolve"),
                size: RESOLVE_SLOTS * RESOLVE_ALIGN,
                usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            }),
            readback: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("hd time readback"),
                size: RESOLVE_SLOTS * RESOLVE_ALIGN,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            }),
            period_ns: f64::from(queue.get_timestamp_period()),
            ring: (0..TIME_RING)
                .map(|_| RingSlot {
                    buffer: device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some("hd time ring"),
                        size: RESOLVE_SLOTS * RESOLVE_ALIGN,
                        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                        mapped_at_creation: false,
                    }),
                    state: std::sync::Arc::new(std::sync::atomic::AtomicU8::new(SLOT_FREE)),
                    frame: 0,
                    splits: 0,
                })
                .collect(),
            ring_copied: None,
            ring_frames: 0,
            pending: false,
            shadows: false,
            contact: false,
            ao: false,
            aa: false,
            views: false,
            ssr: false,
            clouds: false,
            splits: 0,
        });
        let post = PostPipes::new(&device, cache);
        let prepass_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("hd prepass"),
            bind_group_layouts: &[Some(&frame_layout), None, Some(&post.motion_layout)],
            immediate_size: 0,
        });
        let contact_module = module(&device, Permutation::Contact);
        let contact_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("hd contact"),
                bind_group_layouts: &[Some(&contact_layout)],
                immediate_size: 0,
            });
        let contact_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("hd contact"),
            layout: Some(&contact_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &contact_module,
                entry_point: Some("vs_full"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &contact_module,
                entry_point: Some("fs_contact"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: CONTACT_FORMAT,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache,
        });
        let shadows = Shadows::new(
            &device,
            cache,
            torque_render_scene::hd::Quality::High.shadows(),
            E::extra_casters(),
        );
        let white = one_texel(&device, &queue, "hd white", [255; 4]);
        let empty = one_texel(&device, &queue, "hd empty", [0; 4]);
        let volumetric = volumetric::Volumetric::new(&device, cache, &frame_layout, &sky.layout);
        let bloom = bloom::Bloom::new(&device, cache);
        let exposure = exposure::Exposure::new(&device, cache);
        // The weighted blend's composite.
        let comp_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("hd wboit composite"),
            entries: &[
                uniform(0, wgpu::ShaderStages::FRAGMENT),
                texture_entry(
                    1,
                    wgpu::TextureSampleType::Float { filterable: false },
                    wgpu::TextureViewDimension::D2,
                ),
                texture_entry(
                    2,
                    wgpu::TextureSampleType::Float { filterable: false },
                    wgpu::TextureViewDimension::D2,
                ),
                texture_entry(
                    3,
                    wgpu::TextureSampleType::Float { filterable: false },
                    wgpu::TextureViewDimension::D2,
                ),
            ],
        });
        let comp_module = module(&device, Permutation::Wboit);
        let comp_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("hd wboit composite"),
            bind_group_layouts: &[Some(&comp_layout)],
            immediate_size: 0,
        });
        let wboit_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("hd wboit composite"),
            layout: Some(&comp_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &comp_module,
                entry_point: Some("vs_full"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &comp_module,
                entry_point: Some("fs_composite"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: HDR_FORMAT,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache,
        });
        let comp_uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("hd wboit composite"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let late_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("hd late sprites"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                texture_entry(
                    1,
                    wgpu::TextureSampleType::Float { filterable: false },
                    wgpu::TextureViewDimension::D2,
                ),
                texture_entry(
                    2,
                    wgpu::TextureSampleType::Float { filterable: false },
                    wgpu::TextureViewDimension::D2,
                ),
            ],
        });
        let late_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("hd late sprites"),
            bind_group_layouts: &[None, Some(&late_layout)],
            immediate_size: 0,
        });
        let late_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("hd late sprites"),
            layout: Some(&late_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &comp_module,
                entry_point: Some("vs_full"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &comp_module,
                entry_point: Some("fs_late"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                // The shown colour under the over draws' coverage, plus their colour and the sprites'.
                targets: &[Some(wgpu::ColorTargetState {
                    format: post::SHOWN_FORMAT,
                    blend: Some(wgpu::BlendState {
                        color: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::One,
                            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                            operation: wgpu::BlendOperation::Add,
                        },
                        alpha: wgpu::BlendComponent::REPLACE,
                    }),
                    write_mask: wgpu::ColorWrites::COLOR,
                })],
            }),
            multiview_mask: None,
            cache,
        });
        let n = LUT_SIZE as u32;
        let lut_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("hd grading table"),
            size: wgpu::Extent3d {
                width: n,
                height: n,
                depth_or_array_layers: n,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D3,
            format: LUT_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let lut_view = lut_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let dummy_depth = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("hd depth stand-in"),
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: DEPTH_FORMAT,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
            .create_view(&wgpu::TextureViewDescriptor::default());
        let arena = arena::Arena::new(&device);
        let cell_table = cull::CellTable::new(&device);
        let cull = cull::Cull::new(&device, cache);
        let table = arena::Table::new(&device);
        let main_lists = arena::Lists::new(&device);
        let scratch_lists = arena::Lists::new(&device);
        let lists_first = arena::Lists::new(&device);
        let lists_second = arena::Lists::new(&device);
        let draw_way = arena::DrawWay::of(&device);
        let first_instance = device
            .features()
            .contains(wgpu::Features::INDIRECT_FIRST_INSTANCE);
        let nodes_table = Self::nodes_buffer(&device, 1);
        let ext_slots = ExtSlots::new(&device);
        Ok(HdWorld {
            wboit_pipeline,
            comp_layout,
            comp_uniform,
            late_pipeline,
            late_layout,
            lut_texture,
            lut_view,
            lut_key: None,
            identity_grade: false,
            user_lut: None,
            user_lut_generation: 0,
            dummy_depth,
            ext_slots,
            ext: E::new(&device),
            profile_passes: false,
            cpu_marks: false,
            cpu_passes: Vec::new(),
            cpu_frames: 0,
            pipelines_made: 0,
            warming: None,
            warm_queue: Vec::new(),
            uploaded: None,
            lights_generation: None,
            uploaded_glow: true,
            shadowed_lights: 0,
            marks: device
                .features()
                .contains(wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS)
                .then(|| Marks::new(&device, &queue)),
            sky,
            bloom,
            exposure,
            volumetric,
            clouds,
            frame_seconds: 1.0 / 60.0,
            auto_exposure: false,
            grain: 0.0,
            render_scale: RenderScale::Auto,
            forward: [(true, false), (false, false), (true, true), (false, true)]
                .into_iter()
                .map(|(clustered, front_layer)| {
                    (
                        (clustered, front_layer),
                        module(
                            &device,
                            Permutation::Forward {
                                clustered,
                                front_layer,
                            },
                        ),
                    )
                })
                .collect(),
            tonemap_module: module(&device, Permutation::Tonemap),
            pipeline_cache: pipeline_cache.clone(),
            device,
            queue,
            timestamps,
            frame_layout,
            clusters_layout,
            texture_layout,
            tonemap_layout,
            forward_layout,
            clusters_pipeline,
            tonemap_pipeline_layout,
            tonemap_pipelines: HashMap::new(),
            pipelines: HashMap::new(),
            samplers: HashMap::new(),
            groups: HashMap::new(),
            textures: Vec::new(),
            white,
            empty,
            frame,
            frame_still,
            post_uniform,
            lights,
            lights_capacity,
            light_count: 0,
            masks,
            clusters_group,
            cells: BTreeMap::new(),
            lod_units: BTreeMap::new(),
            lod_stay: std::collections::BTreeSet::new(),
            lod_on: false,
            lod_work: None,
            lod_ready: std::collections::VecDeque::new(),
            arena,
            table,
            main_lists,
            draw_way,
            first_instance,
            free_cell_ids: std::collections::BTreeSet::new(),
            next_cell_id: 0,
            cell_table,
            cull,
            grid: cull::CellGrid::default(),
            gpu_cull: true,
            lists_on_gpu: false,
            scratch_lists,
            shadow_lists: Vec::new(),
            occlusion: true,
            lod: false,
            lists_first,
            lists_second,
            targets_made: 0,
            upload_budget: None,
            uploads_pending: false,
            putting: None,
            ext_runs: HashMap::new(),
            upload_time: UPLOAD_TIME,
            resident_stale: 0,
            slots: BTreeMap::new(),
            loose: Loose::default(),
            runs: slots::Runs::default(),
            nodes: nodes_table,
            node_runs: slots::Runs::default(),
            node_baked: HashMap::new(),
            index_runs: slots::Runs::default(),
            spare_previous: None,
            particles: Particles::default(),
            targets: None,
            timer,
            times: None,
            shadows,
            prepass_layout,
            prepass: HashMap::new(),
            contact_layout,
            contact_pipeline,
            light_bytes: Vec::new(),
            light_hosts: Vec::new(),
            written_slots: None,
            frame_lights: Vec::new(),
            light_cones: Vec::new(),
            shadow_dirty: Vec::new(),
            shadow_reset: true,
            ground_boxes: Vec::new(),
            ground_key: Vec::new(),
            shadow_resets: 0,
            upload_times: Vec::new(),
            upload_steps: Vec::new(),
            shadow_patched: 0,
            post,
            temporal: Temporal::default(),
            output: Output::Hdr,
            brute_force_lights: false,
            features: HdFeatures::default(),
            histories: Histories::default(),
            debug: DebugView::Off,
            stats: DrawStats::default(),
            refl,
            planar_override: None,
            mirror_reflections: false,
            capture_every_frame: false,
            motion_per_draw: false,
        })
    }

    /// What the last frame's shadow passes did.
    pub fn shadow_stats(&self) -> ShadowStats {
        self.shadows.stats
    }

    /// The sun's shadow range the last frame's cascades covered (None before the first frame).
    pub fn shadow_range(&self) -> Option<torque_render_scene::hd::shadow::ShadowRange> {
        self.shadows.range
    }

    /// The shadow maps' GPU memory (bytes).
    pub fn shadow_bytes(&self) -> u64 {
        self.shadows.bytes()
    }

    fn ensure_prepass(&mut self, cull: Cull) {
        for still in [false, true] {
            self.ensure_prepass_of(cull, still);
        }
    }

    fn ensure_prepass_of(&mut self, cull: Cull, still: bool) {
        if self.prepass.contains_key(&(cull, still)) {
            return;
        }
        let (cull_mode, front_face) = match cull {
            Cull::None => (None, wgpu::FrontFace::Cw),
            Cull::BackCw => (Some(wgpu::Face::Back), wgpu::FrontFace::Cw),
            Cull::BackCcw => (Some(wgpu::Face::Back), wgpu::FrontFace::Ccw),
        };
        let module = &self.forward[&(true, false)];
        let pipeline = self
            .device
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("hd prepass"),
                layout: Some(&self.prepass_layout),
                vertex: wgpu::VertexState {
                    module,
                    entry_point: Some(if still { "vs_depth_still" } else { "vs_depth" }),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    buffers: &[
                        Some(wgpu::VertexBufferLayout {
                            array_stride: HD_VERTEX_BYTES as u64,
                            step_mode: wgpu::VertexStepMode::Vertex,
                            attributes: &[
                                wgpu::VertexAttribute {
                                    format: wgpu::VertexFormat::Float32x3,
                                    offset: 0,
                                    shader_location: 0,
                                },
                                wgpu::VertexAttribute {
                                    format: wgpu::VertexFormat::Snorm16x2,
                                    offset: 12,
                                    shader_location: 1,
                                },
                                wgpu::VertexAttribute {
                                    format: wgpu::VertexFormat::Uint32,
                                    offset: 28,
                                    shader_location: 4,
                                },
                                wgpu::VertexAttribute {
                                    format: wgpu::VertexFormat::Uint32,
                                    offset: 32,
                                    shader_location: 5,
                                },
                            ],
                        }),
                        // The previous positions (location 6 of vs_depth), in the same layout.
                        Some(wgpu::VertexBufferLayout {
                            array_stride: HD_VERTEX_BYTES as u64,
                            step_mode: wgpu::VertexStepMode::Vertex,
                            attributes: &[wgpu::VertexAttribute {
                                format: wgpu::VertexFormat::Float32x3,
                                offset: 0,
                                shader_location: 6,
                            }],
                        }),
                    ],
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    strip_index_format: None,
                    front_face,
                    cull_mode,
                    unclipped_depth: false,
                    polygon_mode: wgpu::PolygonMode::Fill,
                    conservative: false,
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: Some(true),
                    depth_compare: Some(wgpu::CompareFunction::GreaterEqual),
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState::default(),
                }),
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module,
                    entry_point: Some("fs_depth"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    targets: &[
                        Some(wgpu::ColorTargetState {
                            format: MOTION_FORMAT,
                            blend: None,
                            write_mask: wgpu::ColorWrites::ALL,
                        }),
                        Some(wgpu::ColorTargetState {
                            format: NORMAL_FORMAT,
                            blend: None,
                            write_mask: wgpu::ColorWrites::ALL,
                        }),
                    ],
                }),
                multiview_mask: None,
                cache: self.pipeline_cache.as_ref(),
            });
        self.prepass.insert((cull, still), pipeline);
        self.pipelines_made += 1;
    }

    /// Makes the pipelines a frame may first need anywhere in a world (each blend, cull and depth state of the
    /// forward pass, its translucent and particle stages, the prepass, and the reflection views' and the water's), so walking into
    /// reflective surfaces, water, lights or particles never stops a frame for seconds to make one (a first view of a reflective scene made four
    /// in one 3.4 s frame). A window may warm them on a worker thread ([`HdWorld::from_device`]), so this costs it no frame.
    pub fn prewarm(&mut self) {
        let culls = [Cull::None, Cull::BackCw, Cull::BackCcw];
        let blends = [Blend::Opaque, Blend::Alpha, Blend::Additive, Blend::One];
        let depths = [(true, true), (true, false), (false, false)];
        for cull in culls {
            self.ensure_prepass(cull);
            self.ensure_pipeline(PipelineKey {
                blend: Blend::Modulate2x,
                cull,
                depth_test: true,
                depth_write: false,
                clustered: true,
                front_layer: false,
                stage: Stage::Main,
            });
            for blend in blends {
                for (depth_test, depth_write) in depths {
                    let key = PipelineKey {
                        blend,
                        cull,
                        depth_test,
                        depth_write,
                        clustered: true,
                        front_layer: false,
                        stage: Stage::Main,
                    };
                    self.ensure_pipeline(key);
                    match blend {
                        Blend::Alpha => {
                            let g = PipelineKey {
                                front_layer: true,
                                ..key
                            };
                            self.ensure_pipeline(g);
                            for stage in [Stage::FrontDepth, Stage::Tail, Stage::Front] {
                                self.ensure_pipeline(PipelineKey { stage, ..g });
                            }
                        }
                        Blend::Additive | Blend::One => self.ensure_pipeline(PipelineKey {
                            stage: Stage::Particle,
                            ..key
                        }),
                        _ => {}
                    }
                    // The reflection views draw the opaque frame's states (turned culls among them: every cull is here).
                    if blend == Blend::Opaque {
                        self.ensure_view_pipeline(PipelineKey {
                            clustered: false,
                            ..key
                        });
                    }
                }
            }
            self.ensure_water_pipeline(cull);
        }
    }

    /// Whether frame times come from GPU timestamps.
    pub fn has_timestamps(&self) -> bool {
        self.timestamps
    }

    /// The device and queue the world draws with.
    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }

    /// The device's pipeline cache, for an extension's own pipelines (`cache: world.pipeline_cache()`).
    pub fn pipeline_cache(&self) -> Option<&wgpu::PipelineCache> {
        self.pipeline_cache.as_ref()
    }

    pub fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }

    /// A table of `entries` meshes' places ([`NODE_BYTES`] each).
    fn nodes_buffer(device: &wgpu::Device, entries: usize) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("hd mesh places"),
            size: (entries.max(1) * NODE_BYTES) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    fn lights_buffer(device: &wgpu::Device, capacity: usize) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("hd lights"),
            size: (capacity.max(1) * LIGHT_BYTES) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    fn clusters_group(
        device: &wgpu::Device,
        clusters_layout: &wgpu::BindGroupLayout,
        frame: &wgpu::Buffer,
        lights: &wgpu::Buffer,
        masks: &wgpu::Buffer,
    ) -> wgpu::BindGroup {
        let entries = [
            wgpu::BindGroupEntry {
                binding: 0,
                resource: frame.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: lights.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: masks.as_entire_binding(),
            },
        ];
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("hd clusters"),
            layout: clusters_layout,
            entries: &entries,
        })
    }

    /// The forward passes' frame group: the uniform, the lights, the cluster masks, the opaque frame's copy, the shadows' uniform,
    /// maps and samplers, the contact mask, the AO.
    fn frame_group(
        &self,
        background: &wgpu::TextureView,
        contact: &wgpu::TextureView,
        ao: &wgpu::TextureView,
        front: Option<&wgpu::TextureView>,
    ) -> wgpu::BindGroup {
        self.frame_group_with(
            &self.frame,
            background,
            contact,
            ao,
            front.unwrap_or(&self.dummy_depth),
        )
    }

    /// The depth stand-in a frame group's front-layer slot takes where the front layer's depth is not read.
    pub(crate) fn dummy_depth(&self) -> &wgpu::TextureView {
        &self.dummy_depth
    }

    /// [`HdWorld::frame_group`] with another frame uniform (a secondary view's).
    pub(crate) fn frame_group_with(
        &self,
        frame: &wgpu::Buffer,
        background: &wgpu::TextureView,
        contact: &wgpu::TextureView,
        ao: &wgpu::TextureView,
        front: &wgpu::TextureView,
    ) -> wgpu::BindGroup {
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("hd frame"),
            layout: &self.frame_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: frame.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: self.lights.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: self.masks.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(background),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: self.shadows.uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::TextureView(self.shadows.csm_view()),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: wgpu::BindingResource::Sampler(&self.shadows.cmp_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 7,
                    resource: wgpu::BindingResource::Sampler(&self.shadows.gather_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 8,
                    resource: wgpu::BindingResource::TextureView(self.shadows.point_view(0)),
                },
                wgpu::BindGroupEntry {
                    binding: 9,
                    resource: wgpu::BindingResource::TextureView(contact),
                },
                wgpu::BindGroupEntry {
                    binding: 10,
                    resource: wgpu::BindingResource::TextureView(ao),
                },
                tv(11, front),
                tv(12, self.shadows.trans_view()),
                wgpu::BindGroupEntry {
                    binding: 13,
                    resource: wgpu::BindingResource::Sampler(&self.shadows.trans_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 14,
                    resource: self.ext_slots.forward[0].as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 15,
                    resource: self.ext_slots.forward[1].as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 16,
                    resource: self.nodes.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 17,
                    resource: wgpu::BindingResource::TextureView(self.shadows.point_view(1)),
                },
                wgpu::BindGroupEntry {
                    binding: 18,
                    resource: wgpu::BindingResource::TextureView(self.shadows.point_view(2)),
                },
            ],
        })
    }

    fn sampler(&mut self, k: SamplerKey) -> &wgpu::Sampler {
        let device = &self.device;
        self.samplers.entry(k).or_insert_with(|| {
            let address = if k.repeat {
                wgpu::AddressMode::Repeat
            } else {
                wgpu::AddressMode::ClampToEdge
            };
            let f = |l: bool| {
                if l {
                    wgpu::FilterMode::Linear
                } else {
                    wgpu::FilterMode::Nearest
                }
            };
            device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("hd"),
                address_mode_u: address,
                address_mode_v: address,
                address_mode_w: address,
                mag_filter: f(k.mag),
                min_filter: f(k.min),
                mipmap_filter: if k.mip {
                    wgpu::MipmapFilterMode::Linear
                } else {
                    wgpu::MipmapFilterMode::Nearest
                },
                lod_max_clamp: if k.mipmaps { 32.0 } else { 0.0 },
                ..wgpu::SamplerDescriptor::default()
            })
        })
    }

    fn ensure_group(&mut self, tex: TexRef, s: SamplerKey) {
        if self.groups.contains_key(&(tex, s)) {
            return;
        }
        self.sampler(s);
        let view = match tex {
            TexRef::White => &self.white,
            TexRef::Empty => &self.empty,
            TexRef::Table(i) => self.textures.get(i).unwrap_or(&self.white),
        };
        let group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("hd texture"),
            layout: &self.texture_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.samplers[&s]),
                },
            ],
        });
        self.groups.insert((tex, s), group);
    }

    fn ensure_pipeline(&mut self, key: PipelineKey) {
        if self.pipelines.contains_key(&key) {
            return;
        }
        // One the warm-up is making: waited for rather than made twice.
        if self.warming.as_ref().is_some_and(|w| w.keys.contains(&key)) {
            self.take_warmed(true);
            if self.pipelines.contains_key(&key) {
                return;
            }
        }
        // One a browser's warm-up has queued: made now, and not again.
        self.warm_queue.retain(|k| *k != key);
        let module = &self.forward[&(key.clustered, key.front_layer)];
        let pipeline = make_pipeline(
            &self.device,
            self.pipeline_cache.as_ref(),
            &self.forward_layout,
            module,
            key,
        );
        self.pipelines.insert(key, pipeline);
        self.pipelines_made += 1;
    }

    /// Starts making the pipelines the effects draw with (`effect_keys`: particles, ribbons, bolt trails, decals and tire
    /// tracks, which a scene often first shows mid-game, at a shot or an impact), on a worker thread where there is one, so their
    /// shaders' compiles cost no frame (a cold first shot made seven in two frames of 0.6 s each). Each frame takes the ones
    /// made; a frame that needs one still being made waits for the rest. In a browser (no threads) they are made a few a frame
    /// ([`WEB_WARM_PER_FRAME`]), so the frame that starts the page's world does not make them all; where no thread can be started
    /// otherwise they are made now.
    pub fn warm_effects(&mut self) {
        if self.warming.is_some() {
            return;
        }
        let keys: Vec<PipelineKey> = effect_keys(!self.brute_force_lights)
            .into_iter()
            .filter(|k| !self.pipelines.contains_key(k))
            .collect();
        if keys.is_empty() {
            return;
        }
        #[cfg(target_arch = "wasm32")]
        {
            self.warm_queue = keys;
        }
        #[cfg(not(target_arch = "wasm32"))]
        self.warm_on_thread(keys);
    }

    /// The warm-up of `keys` on a worker thread ([`HdWorld::warm_effects`]); made now when no thread can be started.
    #[cfg(not(target_arch = "wasm32"))]
    fn warm_on_thread(&mut self, keys: Vec<PipelineKey>) {
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let make = {
            let device = self.device.clone();
            let cache = self.pipeline_cache.clone();
            let layout = self.forward_layout.clone();
            let modules = self.forward.clone();
            let keys = keys.clone();
            let stop = stop.clone();
            move || {
                keys.into_iter()
                    .take_while(|_| !stop.load(std::sync::atomic::Ordering::Relaxed))
                    .map(|k| {
                        let module = &modules[&(k.clustered, k.front_layer)];
                        (
                            k,
                            make_pipeline(&device, cache.as_ref(), &layout, module, k),
                        )
                    })
                    .collect::<Vec<_>>()
            }
        };
        if let Ok(thread) = std::thread::Builder::new()
            .name("pipeline warm-up".into())
            .spawn(make.clone())
        {
            self.warming = Some(Warming {
                keys,
                stop,
                thread: Some(thread),
            });
            return;
        }
        for (k, p) in make() {
            self.pipelines.insert(k, p);
            self.pipelines_made += 1;
        }
    }

    /// Whether the warm-up ([`HdWorld::warm_effects`]) is still making pipelines: its thread (a frame takes them once it is done),
    /// or in a browser the ones still queued.
    pub fn warming(&self) -> bool {
        self.warming.is_some() || !self.warm_queue.is_empty()
    }

    /// In a browser: whether the last frame wanted clouds while their noise was still being made (a share of it each such frame:
    /// [`crate::clouds::WEB_NOISE_BUDGET`]); they are drawn once it is all made. Never natively.
    pub fn clouds_waiting(&self) -> bool {
        self.clouds.waiting
    }

    /// Makes up to `n` of the warm-up's pipelines still queued (in a browser: [`HdWorld::warm_effects`]).
    fn warm_queued(&mut self, n: usize) {
        let keys: Vec<PipelineKey> = self
            .warm_queue
            .drain(..n.min(self.warm_queue.len()))
            .collect();
        for key in keys {
            self.ensure_pipeline(key);
        }
    }

    /// The warm-up's pipelines, once its thread is done (or waiting for it: `wait`).
    fn take_warmed(&mut self, wait: bool) {
        if !self.warming.as_ref().is_some_and(|w| wait || w.finished()) {
            return;
        }
        let Some(w) = self.warming.take() else {
            return;
        };
        // A thread that panicked made none: each is made when first drawn, as before.
        for (k, p) in w.join() {
            if let std::collections::hash_map::Entry::Vacant(e) = self.pipelines.entry(k) {
                e.insert(p);
                self.pipelines_made += 1;
            }
        }
    }

    /// The grade the table holds: the image settings', or the identity for the check.
    fn grade_of(&self, image: &ImageSettings) -> torque_render_scene::hd::lut::Grade {
        if self.identity_grade {
            torque_render_scene::hd::lut::Grade::NEUTRAL
        } else {
            image.grade()
        }
    }

    /// Uses `lut` (a user's `.cube`, already read and checked: [`torque_render_scene::hd::lut::parse_cube`]) after the image settings' grade, or none.
    pub fn set_user_lut(&mut self, lut: Option<CubeLut>) {
        self.user_lut = lut;
        self.user_lut_generation += 1;
        self.lut_key = None;
    }

    /// The grading table of `image` (with the user's table), baked and uploaded when either changed.
    fn ensure_lut(&mut self, image: &ImageSettings) {
        let key = (*image, self.user_lut_generation, self.identity_grade);
        if self.lut_key == Some(key) {
            return;
        }
        let table = torque_render_scene::hd::lut::bake(
            *image,
            &self.grade_of(image),
            self.user_lut.as_ref(),
        );
        let mut bytes = Vec::with_capacity(table.len() * 8);
        for c in &table {
            for v in c {
                bytes.extend_from_slice(&f32_to_f16(*v).to_le_bytes());
            }
        }
        let n = LUT_SIZE as u32;
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.lut_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &bytes,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(8 * n),
                rows_per_image: Some(n),
            },
            wgpu::Extent3d {
                width: n,
                height: n,
                depth_or_array_layers: n,
            },
        );
        self.lut_key = Some(key);
    }

    fn ensure_tonemap(&mut self, format: wgpu::TextureFormat) {
        if self.tonemap_pipelines.contains_key(&format) {
            return;
        }
        let p = self
            .device
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("hd tonemap"),
                layout: Some(&self.tonemap_pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &self.tonemap_module,
                    entry_point: Some("vs_full"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    buffers: &[],
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &self.tonemap_module,
                    entry_point: Some("fs_tonemap"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: self.pipeline_cache.as_ref(),
            });
        self.tonemap_pipelines.insert(format, p);
        self.pipelines_made += 1;
    }

    fn upload_texture(&self, t: &torque_render_scene::Texture) -> wgpu::TextureView {
        let tex = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some(&t.path),
            size: wgpu::Extent3d {
                width: t.width.max(1),
                height: t.height.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: t.levels.len().max(1) as u32,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[wgpu::TextureFormat::Rgba8UnormSrgb],
        });
        for (i, level) in t.levels.iter().enumerate() {
            let (w, h) = ((t.width >> i).max(1), (t.height >> i).max(1));
            if level.len() < (4 * w * h) as usize {
                continue;
            }
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &tex,
                    mip_level: i as u32,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                level,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(4 * w),
                    rows_per_image: Some(h),
                },
                wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
            );
        }
        tex.create_view(&wgpu::TextureViewDescriptor {
            format: Some(wgpu::TextureFormat::Rgba8UnormSrgb),
            ..wgpu::TextureViewDescriptor::default()
        })
    }

    /// Brings the GPU up to `scene`: the new textures, the changed static cells, the lights, and the eye-dependent draws when the
    /// scene was synced since the last upload.
    pub fn upload(&mut self, scene: &E::Scene) {
        while self.textures.len() < scene.textures.textures.len() {
            let view = self.upload_texture(&scene.textures.textures[self.textures.len()]);
            self.textures.push(view);
        }
        // When every sync since the last upload was a control sync (a window's every frame: the control object and the
        // late objects moved, nothing else), the cells and lights are as they were: only the draws go up again.
        let only_control = self.uploaded.is_some_and(|(g, c)| {
            scene.generation > g && scene.generation - g == scene.control_generation.wrapping_sub(c)
        });
        self.uploaded = Some((scene.generation, scene.control_generation));
        if only_control && !self.uploads_pending {
            // The levels' worker's finished blocks go in on any frame.
            self.update_lod(scene, &[]);
            if self.loose.generation != Some(scene.generation) {
                self.upload_loose(scene);
            }
            // A light that followed the client's Player.
            if self.lights_generation != Some(scene.lights_generation) {
                self.upload_lights(scene);
            }
            return;
        }
        // Each step's time (the reports' upload_times, and a live window's slow frames).
        let mut upload_at = web_time::Instant::now();
        self.upload_times.clear();
        macro_rules! upload_mark {
            ($label:expr) => {
                let now = web_time::Instant::now();
                let ms = (now - upload_at).as_secs_f64() * 1000.0;
                self.upload_times.push(($label, ms));
                match self.upload_steps.iter_mut().find(|s| s.0 == $label) {
                    Some(s) => s.1 += ms,
                    None => self.upload_steps.push(($label, ms)),
                }
                upload_at = now;
            };
        }
        // The static cells (each changed one's old and new box marks the cached shadows it touches).
        let mut dirty: Vec<[[f32; 3]; 2]> = Vec::new();
        let gone: Vec<[i32; 3]> = self
            .cells
            .keys()
            .filter(|k| {
                scene
                    .cells
                    .cells()
                    .get(*k)
                    .is_none_or(|c| c.indices.is_empty() && !c.resident)
            })
            .copied()
            .collect();
        // A resident cell changed without its mesh cannot be uploaded: its GPU copy stays.
        self.resident_stale = scene
            .cells
            .cells()
            .iter()
            .filter(|(k, c)| {
                c.resident
                    && c.indices.is_empty()
                    && self
                        .cells
                        .get(*k)
                        .is_some_and(|g| g.generation != c.generation)
            })
            .count();
        upload_mark!("gone");
        // The cells this upload takes out or puts in: their levels of detail's blocks are built again.
        let mut lod_touched: Vec<[i32; 3]> = gone.clone();
        for k in gone {
            self.ext.cell_out(k);
            if let Some(old) = self.cells.remove(&k) {
                dirty.push(old.bounds);
                self.drop_cell(old);
            }
        }
        let mut changed: Vec<&ExtCell<E>> = scene
            .cells
            .cells()
            .iter()
            .filter(|(k, c)| {
                !c.indices.is_empty()
                    && self
                        .cells
                        .get(*k)
                        .is_none_or(|g| g.generation != c.generation)
            })
            .map(|(_, c)| c)
            .collect();
        upload_mark!("changed");
        // Under a budget, the nearest the eye first; the rest at the next upload (the old slabs drawn until then).
        self.uploads_pending = false;
        if let Some(budget) = self.upload_budget {
            let eye = scene.eye.as_ref().map_or([0.0; 3], |e| e.position);
            let d2 = |c: &ExtCell<E>| -> f32 {
                (0..3)
                    .map(|a| {
                        let d = (c.bounds[0][a] - eye[a])
                            .max(0.0)
                            .max(eye[a] - c.bounds[1][a]);
                        d * d
                    })
                    .sum()
            };
            changed.sort_by(|a, b| d2(a).total_cmp(&d2(b)).then(a.key.cmp(&b.key)));
            let mut bytes = 0u64;
            let mut take = 0;
            for c in &changed {
                let b = (c.vertices.len() * HD_VERTEX_BYTES + c.indices.len() * 4) as u64;
                if take > 0 && bytes + b > budget {
                    break;
                }
                bytes += b;
                take += 1;
            }
            self.uploads_pending = take < changed.len();
            changed.truncate(take);
        }
        upload_mark!("budget");
        // Room for them all at once (one repack at most).
        let (nv, ni) = changed.iter().fold((0u64, 0u64), |(v, i), c| {
            (v + c.vertices.len() as u64, i + c.indices.len() as u64)
        });
        // A repack moves every live slab: the unchanged cells' places and records follow (else they draw old ranges, past the
        // index buffer's end when it shrank).
        let repacks = self.arena.repacks;
        self.arena.reserve(&self.device, &self.queue, nv, ni);
        if self.arena.repacks != repacks {
            self.slabs_moved();
        }
        upload_mark!("reserve");
        // Each cell's old slab given back as its new one goes in, and (under a budget) no more than UPLOAD_TIME of them a
        // frame: the rest at the next upload, their old slabs drawn until then.
        let started = web_time::Instant::now();
        // A cell being put in slices goes on first (dropped when the scene's cell changed or went).
        let mut busy = false;
        if let Some(mut p) = self.putting.take() {
            match scene
                .cells
                .cells()
                .get(&p.key)
                .filter(|c| c.generation == p.generation && !c.indices.is_empty())
            {
                None => {
                    self.arena.free(p.slab);
                    self.ext.free_cell(p.runs);
                }
                Some(c) => {
                    if self.write_put(c, &mut p, Some((started, self.upload_time))) {
                        self.ext.cell_out(c.key);
                        if let Some(old) = self.cells.remove(&c.key) {
                            dirty.push(old.bounds);
                            self.drop_cell(old);
                        }
                        dirty.push(c.bounds);
                        lod_touched.push(c.key);
                        self.ext.cell_in(c);
                        if let Some(g) = self.finish_put(c, 0, p) {
                            self.grid.insert(c.key, g.bounds, g.reflective);
                            self.cells.insert(c.key, g);
                        }
                    } else {
                        self.putting = Some(p);
                        self.uploads_pending = true;
                        busy = true;
                    }
                }
            }
        }
        for (n, c) in changed.into_iter().enumerate() {
            if busy
                || (self.upload_budget.is_some()
                    && (n > 0 || self.putting.is_some())
                    && started.elapsed() > self.upload_time)
            {
                self.uploads_pending = true;
                break;
            }
            if self.putting.as_ref().is_some_and(|p| p.key == c.key) {
                continue;
            }
            // A live window's dense cell is put in slices (its old slab drawn until the new one is whole).
            if self.upload_budget.is_some() && c.vertices.len() > SLICE_VERTICES {
                let Some(mut p) = self.begin_put(c) else {
                    continue;
                };
                if !self.write_put(c, &mut p, Some((started, self.upload_time))) {
                    self.putting = Some(p);
                    self.uploads_pending = true;
                    break;
                }
                self.ext.cell_out(c.key);
                if let Some(old) = self.cells.remove(&c.key) {
                    dirty.push(old.bounds);
                    self.drop_cell(old);
                }
                dirty.push(c.bounds);
                lod_touched.push(c.key);
                self.ext.cell_in(c);
                if let Some(g) = self.finish_put(c, 0, p) {
                    self.grid.insert(c.key, g.bounds, g.reflective);
                    self.cells.insert(c.key, g);
                }
                continue;
            }
            self.ext.cell_out(c.key);
            if let Some(old) = self.cells.remove(&c.key) {
                dirty.push(old.bounds);
                self.drop_cell(old);
            }
            dirty.push(c.bounds);
            lod_touched.push(c.key);
            self.ext.cell_in(c);
            if let Some(g) = self.put_cell(c, 0) {
                self.grid.insert(c.key, g.bounds, g.reflective);
                self.cells.insert(c.key, g);
            }
        }
        upload_mark!("puts");
        self.update_lod(scene, &lod_touched);
        upload_mark!("lod");
        if self.table.flush(&self.device, &self.queue) {
            self.main_lists = arena::Lists::new(&self.device);
        }
        self.cell_table.flush(&self.device, &self.queue);
        self.ext.flush_cells(&self.device, &self.queue);
        upload_mark!("flush");
        // What the extension keeps from the cells (with the boxes it marks dirty for the shadows).
        {
            let cells = &self.cells;
            let bounds = |k: [i32; 3]| cells.get(&k).map(|g| g.bounds);
            self.ext
                .cells_done(&self.device, &self.queue, scene, &bounds, &mut dirty);
        }
        upload_mark!("extension");
        if self.shadow_dirty.len() + dirty.len() > MAX_DIRTY {
            if !self.shadow_reset {
                self.shadow_resets += 1;
            }
            self.shadow_reset = true;
            self.shadow_dirty.clear();
        } else if !self.shadow_reset {
            self.shadow_dirty.extend(dirty);
        }
        upload_mark!("dirty");
        self.slots = scene
            .cells
            .slots()
            .iter()
            .map(|(slot, s)| {
                (
                    *slot,
                    (
                        s.texture.map_or(TexRef::Empty, TexRef::Table),
                        SamplerKey::of(&s.sampler),
                    ),
                )
            })
            .collect();
        upload_mark!("slots");
        self.upload_lights(scene);
        if self.loose.generation != Some(scene.generation) {
            self.upload_loose(scene);
        }
        upload_mark!("the rest");
        let _ = upload_at;
    }

    /// A secondary view's planes (its frustum, the opaque clusters but the near ones) and its cull slot for this frame when
    /// the lists are the GPU's.
    pub(crate) fn cull_view(&self, camera: &HdCamera) -> (Option<u32>, cull::ViewPlanes) {
        let v = cull::ViewPlanes::frustum(&camera.view_proj, camera.eye, None, cull::VIEW_OPAQUE);
        let slot = self
            .cull
            .as_ref()
            .filter(|_| self.lists_on_gpu)
            .and_then(|c| c.view(&self.queue, &v));
        (slot, v)
    }

    /// A cell into the arena and its clusters into the table (None when the device cannot hold its slabs).
    fn put_cell(&mut self, c: &ExtCell<E>, level: u32) -> Option<GpuCell> {
        let mut p = self.begin_put(c)?;
        self.write_put(c, &mut p, None);
        self.finish_put(c, level, p)
    }

    /// A cell's slab and the extension's runs taken, nothing written (None: no room, the device's or the budget's; the runs go
    /// back, as a retry takes them again).
    fn begin_put(&mut self, c: &ExtCell<E>) -> Option<Putting<E::CellRuns>> {
        let runs = self.ext.put_cell(c);
        let Some((slab, moved)) = self.arena.alloc(
            &self.device,
            &self.queue,
            c.vertices.len() as u64,
            c.indices.len() as u64,
        ) else {
            self.ext.free_cell(runs);
            return None;
        };
        if moved {
            self.slabs_moved();
        }
        let (short, runs_of_indices) = short_indices(&c.indices, &cell_clusters::<E>(c));
        Some(Putting {
            key: c.key,
            generation: c.generation,
            slab,
            runs,
            vertices: 0,
            indices: 0,
            short,
            runs_of_indices,
        })
    }

    /// The cell's vertices and indices written on from where `p` is, until `until` (None: all of them); whether it is whole.
    fn write_put(
        &mut self,
        c: &ExtCell<E>,
        p: &mut Putting<E::CellRuns>,
        until: Option<(web_time::Instant, std::time::Duration)>,
    ) -> bool {
        let late = || until.is_some_and(|(t, d)| t.elapsed() > d);
        let slice = if until.is_some() {
            SLICE_VERTICES
        } else {
            usize::MAX
        };
        while p.vertices < c.vertices.len() {
            let end = c.vertices.len().min(p.vertices.saturating_add(slice));
            let mut bytes = Vec::with_capacity((end - p.vertices) * HD_VERTEX_BYTES);
            // A vertex the extension rewrites (its index into the tables at the cell's runs) goes as it rewrote it.
            for v in &c.vertices[p.vertices..end] {
                match self.ext.vertex(&p.runs, v) {
                    Some(w) => bytes.extend_from_slice(&w.bytes()),
                    None => bytes.extend_from_slice(&v.bytes()),
                }
            }
            self.arena
                .write_vertices(&self.queue, p.slab, p.vertices as u64, &bytes);
            p.vertices = end;
            if late() {
                return false;
            }
        }
        while p.indices < p.short.len() {
            // (Whole 4 bytes a write: an even count but at the end.)
            let end = p
                .short
                .len()
                .min(p.indices.saturating_add(slice.saturating_mul(2)) & !1);
            let end = if end <= p.indices { p.short.len() } else { end };
            self.arena.write_indices(
                &self.queue,
                p.slab,
                p.indices as u64,
                &p.short[p.indices..end],
            );
            p.indices = end;
            if late() {
                return false;
            }
        }
        true
    }

    /// A cell whose slab is whole into the tables: its id, its box, its clusters' records.
    fn finish_put(
        &mut self,
        c: &ExtCell<E>,
        level: u32,
        p: Putting<E::CellRuns>,
    ) -> Option<GpuCell> {
        let (slab, ext_runs, runs) = (p.slab, p.runs, p.runs_of_indices);
        let Some(s) = self.arena.slab(slab) else {
            self.arena.free(slab);
            self.ext.free_cell(ext_runs);
            return None;
        };
        let id = self.free_cell_ids.pop_first().unwrap_or_else(|| {
            self.next_cell_id += 1;
            self.next_cell_id - 1
        });
        self.ext_runs.insert(id, ext_runs);
        self.cell_table.set(
            id,
            cull::CellBox {
                lo: c.bounds[0],
                hi: c.bounds[1],
                live: true,
                key: c.key,
                level,
                lod: self.lod_flags(c.key, level),
            },
        );
        let clusters = cell_clusters::<E>(c);
        let mut records = Vec::with_capacity(runs.len());
        let mut batch_draws: Vec<Vec<(u32, u32, i32)>> = vec![Vec::new(); c.batches.len()];
        for &(k, first, count, vbase) in &runs {
            let cl = &clusters[k];
            if count == 0 {
                continue;
            }
            if let Some(b) = c
                .batches
                .iter()
                .position(|b| b.slot == cl.slot && b.translucent == cl.translucent)
            {
                batch_draws[b].push((first, count, vbase as i32));
            }
            let record = arena::Record {
                lo: cl.bounds[0],
                cell: id,
                hi: cl.bounds[1],
                segment: 0,
                first_index: s.index + first,
                index_count: count,
                base_vertex: s.vertex as i32 + vbase as i32,
                flags: arena::CLUSTER_LIVE
                    | if cl.translucent {
                        arena::CLUSTER_TRANSLUCENT
                    } else {
                        0
                    }
                    | if E::near_slot(cl.slot) {
                        arena::CLUSTER_NEAR
                    } else {
                        0
                    },
            };
            if let Some(slot) = self.table.insert((cl.translucent, cl.slot, s.page), record) {
                records.push(slot);
            }
        }
        let opaque = c
            .batches
            .iter()
            .filter(|b| !b.translucent)
            .map(|b| b.first + b.count)
            .max()
            .unwrap_or(0);
        let total = c
            .batches
            .iter()
            .map(|b| b.first + b.count)
            .max()
            .unwrap_or(0);
        Some(GpuCell {
            page: s.page,
            generation: c.generation,
            key: c.key,
            level,
            id,
            slab,
            base_vertex: s.vertex as i32,
            first_index: s.index,
            records,
            batch_draws,
            batches: c.batches.clone(),
            bounds: c.bounds,
            opaque,
            reflective: c.vertices.iter().any(|v| E::reflective(v.material)),
            total,
        })
    }

    /// A cell's slabs and clusters given back.
    fn drop_cell(&mut self, g: GpuCell) {
        self.arena.free(g.slab);
        for r in g.records {
            self.table.remove(r);
        }
        self.cell_table.set(g.id, cull::CellBox::default());
        if let Some(runs) = self.ext_runs.remove(&g.id) {
            self.ext.free_cell(runs);
        }
        self.free_cell_ids.insert(g.id);
        if g.level == 0 {
            self.grid.remove(g.key);
        }
    }

    /// What the levels of detail draw from `camera` (the cull's CPU mirror, the main view's frustum): per level, the units seen
    /// and chosen and their opaque quads, and the cells whose L0 and L1 block are both chosen (none when the levels tile).
    pub fn lod_report(
        &self,
        camera: &HdCamera,
        quality: torque_render_scene::hd::Quality,
    ) -> LodReport {
        let [d1, d2] = E::level_distances(camera, quality);
        let view =
            cull::ViewPlanes::frustum(&camera.view_proj, camera.eye, None, cull::VIEW_OPAQUE)
                .with_lod(d1, d2);
        let mut r = LodReport::default();
        let chosen = |g: &GpuCell| {
            self.cell_table
                .mirror
                .get(g.id as usize)
                .is_some_and(|c| c.live && view.seen(c.lo, c.hi) && cull::lod_selected(&view, c))
        };
        for g in self.cells.values().chain(self.lod_units.values()) {
            if chosen(g) {
                let l = g.level as usize;
                r.units[l] += 1;
                r.quads[l] += u64::from(g.opaque / 6);
                r.clusters[l] += g.records.len();
            }
        }
        // What L0 would draw there: every seen cell's opaque quads and clusters.
        for g in self.cells.values() {
            if self
                .cell_table
                .mirror
                .get(g.id as usize)
                .is_some_and(|c| c.live && view.seen(c.lo, c.hi))
            {
                r.l0_quads += u64::from(g.opaque / 6);
                r.l0_clusters += g.records.len();
            }
        }
        for g in self.cells.values() {
            let b1 = g.key.map(|k| k.div_euclid(2));
            if chosen(g) && self.lod_units.get(&(1, b1)).is_some_and(chosen) {
                r.both += 1;
            }
        }
        r
    }

    /// The level the cull draws the cells at point `p` at from `camera` (as [`HdWorld::lod_report`] chooses: the last frame's
    /// hysteresis held), None where no unit holds it.
    pub fn lod_level_at(
        &self,
        p: [f32; 3],
        camera: &HdCamera,
        quality: torque_render_scene::hd::Quality,
    ) -> Option<u32> {
        let [d1, d2] = E::level_distances(camera, quality);
        let view =
            cull::ViewPlanes::frustum(&camera.view_proj, camera.eye, None, cull::VIEW_OPAQUE)
                .with_lod(d1, d2);
        let key = p.map(|v| (v / torque_render_scene::hd::cells::CELL_SIZE).floor() as i32);
        let units = [
            self.cells.get(&key),
            self.lod_units.get(&(1, key.map(|k| k.div_euclid(2)))),
            self.lod_units.get(&(2, key.map(|k| k.div_euclid(4)))),
        ];
        units.into_iter().flatten().find_map(|g| {
            let c = self.cell_table.mirror.get(g.id as usize)?;
            (c.live && cull::lod_selected(&view, c)).then_some(g.level)
        })
    }

    /// Whether the coarser levels' blocks holding the unit `key` at `level` have meshes, and whether they were drawn coarse at the
    /// last choice (`cull::CellBox::lod`).
    fn lod_flags(&self, key: [i32; 3], level: u32) -> u32 {
        let up = |n: i32| key.map(|k| k.div_euclid(n));
        let (b1, b2) = match level {
            0 => (up(2), up(4)),
            1 => (key, up(2)),
            _ => (key, key),
        };
        let mut f = 0;
        if level == 0 && self.lod_units.contains_key(&(1, b1)) {
            f |= cull::LOD_HAS_L1;
        }
        if level <= 1 && self.lod_units.contains_key(&(2, b2)) {
            f |= cull::LOD_HAS_L2;
        }
        if level <= 1 && self.lod_stay.contains(&(1, b1)) {
            f |= cull::LOD_STAY_L1;
        }
        if self.lod_stay.contains(&(2, b2)) {
            f |= cull::LOD_STAY_L2;
        }
        f
    }

    /// The levels' choice of this frame from `eye` with the switches at `d1` and `d2` ([`cull::lod_coarse`]): each block with a
    /// mesh drawn coarse or not, and where that changed, the flags of the units it covers (its cells, an L2 block's L1 blocks, the
    /// block itself) written again, so the cull holds each block's choice until the eye has gone past the hysteresis.
    fn update_lod_stay(&mut self, eye: [f32; 3], d1: f32, d2: f32) {
        let hold = LOD_HOLD.load(std::sync::atomic::Ordering::Relaxed);
        let side = torque_render_scene::hd::cells::CELL_SIZE;
        let mut flipped: Vec<(u32, [i32; 3])> = Vec::new();
        for &(level, block) in self.lod_units.keys() {
            let (at, n) = if level == 1 { (d1, 2.0) } else { (d2, 4.0) };
            let d = cull::block_distance(eye, block.map(|k| k as f32), n * side);
            let stay = self.lod_stay.contains(&(level, block));
            if hold && cull::lod_coarse(d, at, stay) != stay {
                flipped.push((level, block));
            }
        }
        // (A block whose mesh went is no longer held.)
        let gone: Vec<(u32, [i32; 3])> = self
            .lod_stay
            .iter()
            .filter(|k| !self.lod_units.contains_key(k))
            .copied()
            .collect();
        for k in gone {
            self.lod_stay.remove(&k);
        }
        if flipped.is_empty() {
            return;
        }
        let mut under: Vec<UnitBox> = Vec::new();
        for &(level, block) in &flipped {
            if !self.lod_stay.remove(&(level, block)) {
                self.lod_stay.insert((level, block));
            }
            for k in block_cells(block, level) {
                if let Some(g) = self.cells.get(&k) {
                    under.push((g.id, g.bounds, g.key, 0));
                }
            }
            if level == 2 {
                for k in block_cells(block, 2)
                    .map(|k| block_of(k, 1))
                    .collect::<std::collections::BTreeSet<_>>()
                {
                    if let Some(g) = self.lod_units.get(&(1, k)) {
                        under.push((g.id, g.bounds, g.key, 1));
                    }
                }
            }
            if let Some(g) = self.lod_units.get(&(level, block)) {
                under.push((g.id, g.bounds, g.key, level));
            }
        }
        for u in under {
            self.refresh_unit(u);
        }
        self.cell_table.flush(&self.device, &self.queue);
    }

    /// A unit's box written again (its levels of detail's meshes came or went).
    fn refresh_unit(&mut self, g: UnitBox) {
        let (id, bounds, key, level) = g;
        let lod = self.lod_flags(key, level);
        self.cell_table.set(
            id,
            cull::CellBox {
                lo: bounds[0],
                hi: bounds[1],
                live: true,
                key,
                level,
                lod,
            },
        );
    }

    /// The levels of detail's blocks of the cells `touched` built again from the scene's cells' inputs (all of them when the levels
    /// were just turned on; none, and every unit's flags cleared, when they are off), and the flags of the units under every block
    /// whose unit came or went. Without an upload budget (the checks) a block is built here; under one (a window) on the levels'
    /// worker, its old unit drawn until the new one is put in at a later upload (the latest build of a block only).
    fn update_lod(&mut self, scene: &E::Scene, touched: &[[i32; 3]]) {
        use std::collections::BTreeSet;
        let mut stale: BTreeSet<(u32, [i32; 3])> = BTreeSet::new();
        let mut came: Vec<(u32, [i32; 3])> = Vec::new();
        if !self.lod {
            if self.lod_on || !self.lod_units.is_empty() {
                self.lod_on = false;
                self.lod_work = None;
                self.lod_ready.clear();
                let keys: Vec<(u32, [i32; 3])> = self.lod_units.keys().copied().collect();
                for key in keys {
                    if let Some(old) = self.lod_units.remove(&key) {
                        self.drop_cell(old);
                    }
                    came.push(key);
                }
            }
        } else {
            if !self.lod_on {
                self.lod_on = true;
                for k in scene.cells.cells().keys() {
                    for level in [1, 2] {
                        stale.insert((level, block_of(*k, level)));
                    }
                }
            }
            for k in touched {
                for level in [1, 2] {
                    stale.insert((level, block_of(*k, level)));
                }
            }
        }
        let job = |level: u32, block: [i32; 3]| LodJob::<E> {
            level,
            block,
            inputs: block_cells(block, level)
                .filter_map(|k| scene.cells.cells().get(&k))
                .map(E::level_input)
                .collect(),
            ticket: 0,
        };
        if self.upload_budget.is_none() {
            for (level, block) in stale {
                let done = job(level, block).run();
                self.apply_lod(done);
                came.push((level, block));
            }
        } else if !stale.is_empty() {
            let work = self.lod_work.get_or_insert_with(LodWork::spawn);
            for (level, block) in stale {
                let mut j = job(level, block);
                work.next += 1;
                j.ticket = work.next;
                work.latest.insert((level, block), j.ticket);
                if work.jobs.send(j).is_err() {
                    work.latest.remove(&(level, block));
                }
            }
        }
        // The worker's blocks finished since, the latest of each; a live window puts in LOD_BYTES_A_FRAME of their meshes a
        // frame (at least one block; a load's blocks came back dozens at once, 12 to 23 ms of a frame), the rest at the next.
        if let Some(work) = &mut self.lod_work {
            work.pump();
            while let Ok(done) = work.done.try_recv() {
                if work.latest.get(&(done.level, done.block)) == Some(&done.ticket) {
                    work.latest.remove(&(done.level, done.block));
                    self.lod_ready.push_back(done);
                }
            }
        }
        let mut bytes = 0u64;
        while let Some(done) = self.lod_ready.front() {
            let b = done.mesh.as_ref().map_or(0, |m| {
                (m.vertices.len() * HD_VERTEX_BYTES + m.indices.len() * 4) as u64
            });
            if self.upload_budget.is_some() && !came.is_empty() && bytes + b > LOD_BYTES_A_FRAME {
                self.uploads_pending = true;
                break;
            }
            bytes += b;
            let Some(done) = self.lod_ready.pop_front() else {
                break;
            };
            came.push((done.level, done.block));
            self.apply_lod(done);
        }
        if !self.lod_ready.is_empty() {
            self.uploads_pending = true;
        }
        if came.is_empty() {
            return;
        }
        // The flags of every unit under a block that came or went: its cells, and an L2 block's L1 blocks.
        let mut under: Vec<UnitBox> = Vec::new();
        for &(level, block) in &came {
            for k in block_cells(block, level) {
                if let Some(g) = self.cells.get(&k) {
                    under.push((g.id, g.bounds, g.key, 0));
                }
            }
            if level == 2 {
                for k in block_cells(block, 2)
                    .map(|k| block_of(k, 1))
                    .collect::<BTreeSet<_>>()
                {
                    if let Some(g) = self.lod_units.get(&(1, k)) {
                        under.push((g.id, g.bounds, g.key, 1));
                    }
                }
            }
        }
        for u in under {
            self.refresh_unit(u);
        }
        if self.table.flush(&self.device, &self.queue) {
            self.main_lists = arena::Lists::new(&self.device);
        }
        self.cell_table.flush(&self.device, &self.queue);
        self.ext.flush_cells(&self.device, &self.queue);
    }

    /// A block's new unit (or none) in place of its old one.
    fn apply_lod(&mut self, done: LodDone<E>) {
        if let Some(old) = self.lod_units.remove(&(done.level, done.block)) {
            self.drop_cell(old);
        }
        let Some(mesh) = done.mesh else {
            return;
        };
        if let Some(g) = self.put_cell(&mesh, done.level) {
            self.lod_units.insert((done.level, done.block), g);
        }
    }

    /// Whether the levels' worker has blocks still to finish.
    pub fn lod_pending(&self) -> bool {
        self.lod_work.as_ref().is_some_and(|w| !w.latest.is_empty()) || !self.lod_ready.is_empty()
    }

    /// Every slab moved (the arena packed): the cells' places and their clusters' records follow.
    fn slabs_moved(&mut self) {
        for g in self.cells.values_mut().chain(self.lod_units.values_mut()) {
            let Some(s) = self.arena.slab(g.slab) else {
                continue;
            };
            let (old_first, new_first) = (g.first_index, s.index);
            let (old_base, new_base) = (g.base_vertex, s.vertex as i32);
            g.base_vertex = new_base;
            g.first_index = s.index;
            for &r in &g.records {
                self.table.update(r, |rec| {
                    rec.first_index = rec.first_index - old_first + new_first;
                    rec.base_vertex = rec.base_vertex - old_base + new_base;
                });
            }
        }
    }

    /// The scene's lights into their buffer (with the static geometry's owners that host them).
    fn upload_lights(&mut self, scene: &E::Scene) {
        self.lights_generation = Some(scene.lights_generation);
        self.uploaded_glow = self.features.glow_lights;
        let (lights, cones, shadowed) = frame_lights(
            &scene.lights,
            scene.glow_lights,
            &scene.spots,
            self.features.glow_lights,
        );
        let n = lights.len();
        self.shadowed_lights = shadowed;
        if n > self.lights_capacity {
            self.lights_capacity = n.next_power_of_two();
            self.lights = Self::lights_buffer(&self.device, self.lights_capacity);
            self.clusters_group = Self::clusters_group(
                &self.device,
                &self.clusters_layout,
                &self.frame,
                &self.lights,
                &self.masks,
            );
        }
        let mut lb = Vec::with_capacity(n * LIGHT_BYTES);
        for (l, cone) in lights.iter().zip(&cones) {
            push_f32(
                &mut lb,
                &[
                    l.position[0],
                    l.position[1],
                    l.position[2],
                    l.radius,
                    l.color[0],
                    l.color[1],
                    l.color[2],
                    0.0,
                ],
            );
            push_f32(&mut lb, cone);
            // No shadow cube yet (the frame sets them).
            push_f32(&mut lb, &[-1.0, 0.0, -1.0, 0.0]);
        }
        if !lb.is_empty() {
            self.queue.write_buffer(&self.lights, 0, &lb);
        }
        self.light_bytes = lb;
        self.light_hosts = lights
            .iter()
            .map(|l| E::light_hosts(scene, l.position))
            .collect();
        self.written_slots = None;
        self.light_count = n;
        self.light_cones = cones;
        self.frame_lights = lights;
    }

    /// The scene's particles (and any texture they added) into their own buffers, when the set changed.
    fn upload_particles(&mut self, scene: &E::Scene) {
        if self.particles.generation == Some(scene.particles_generation) {
            return;
        }
        while self.textures.len() < scene.textures.textures.len() {
            let view = self.upload_texture(&scene.textures.textures[self.textures.len()]);
            self.textures.push(view);
        }
        let mut vbytes = Vec::new();
        let mut indices: Vec<u32> = Vec::new();
        let mut vcount: u32 = 0;
        let mut draws = Vec::with_capacity(scene.particles.len());
        for (dist, d) in &scene.particles {
            let first = indices.len() as u32;
            for v in &d.vertices {
                v.write(&mut vbytes);
            }
            indices.extend_from_slice(&d.indices);
            draws.push((
                *dist,
                DrawRef {
                    first,
                    count: d.indices.len() as u32,
                    base_vertex: vcount as i32,
                    tex: d.texture.map_or(TexRef::White, |(i, _)| TexRef::Table(i)),
                    sampler: d
                        .texture
                        .map_or(SamplerKey::PLAIN, |(_, s)| SamplerKey::of(&s)),
                    blend: d.blend,
                    cull: d.cull,
                    depth_test: d.depth_test,
                    depth_write: d.depth_write,
                    motion: 0,
                    particle: true,
                    // A dying shape among the particles.
                    late: d.vertices.first().is_some_and(|v| {
                        (v.material >> torque_render_scene::hd::material::KIND_SHIFT) & 3
                            == torque_render_scene::hd::material::KIND_SHAPE
                    }),
                    shadow_only: false,
                    view_only: false,
                },
            ));
            vcount += d.vertices.len() as u32;
        }
        let (decals, draws): (Vec<_>, Vec<_>) = draws
            .into_iter()
            .partition(|(_, r)| r.blend == Blend::Modulate2x);
        let mut particles = Particles {
            generation: Some(scene.particles_generation),
            draws,
            decals: decals.into_iter().map(|(_, r)| r).collect(),
            ..Particles::default()
        };
        if !indices.is_empty() {
            particles.vertices = Some(self.device.create_buffer_init(
                &wgpu::util::BufferInitDescriptor {
                    label: Some("hd particle vertices"),
                    contents: &vbytes,
                    usage: wgpu::BufferUsages::VERTEX,
                },
            ));
            let mut ib = Vec::with_capacity(indices.len() * 4);
            push_u32(&mut ib, &indices);
            particles.indices = Some(self.device.create_buffer_init(
                &wgpu::util::BufferInitDescriptor {
                    label: Some("hd particle indices"),
                    contents: &ib,
                    usage: wgpu::BufferUsages::INDEX,
                },
            ));
        } else {
            particles.draws.clear();
            particles.decals.clear();
        }
        self.particles = particles;
    }

    /// Whether the last frame drew the light capture: a cube of every direction from the eye, so every moving shape in the
    /// visible distance may show in it.
    pub fn capture_on(&self) -> bool {
        self.refl.plan.capture
    }

    /// The light capture's faces the next frame draws: one in turn, all six when it starts again.
    pub fn capture_faces_next(&self) -> [bool; 6] {
        self.refl.capture.faces_next()
    }

    fn upload_loose(&mut self, scene: &E::Scene) {
        // The stamped objects' runs first (torque_render_scene::hd::DynamicObject::stamp), then everything else.
        let stamped: Vec<(u32, usize)> = scene
            .objects
            .iter()
            .filter(|o| o.stamp.is_some())
            .map(|o| (o.ghost, o.draws.iter().map(|d| d.vertices.len()).sum()))
            .collect();
        let (runs, moved) = self.runs.place(&stamped);
        if moved {
            self.runs.forget_vertices();
        }
        // The stamped objects' places keep their runs too (a run's vertices are written with its first place in them); the rest's
        // follow.
        let noded: Vec<(u32, usize)> = scene
            .objects
            .iter()
            .filter(|o| o.stamp.is_some() && !o.nodes.is_empty())
            .map(|o| (o.ghost, o.nodes.len()))
            .collect();
        let (node_runs, _) = self.node_runs.place(&noded);
        let mut next_node_run = node_runs.iter();
        let mut node_end = self.node_runs.capacity;
        // Their indices' runs (the rest's indices after them).
        let stamped_indices: Vec<(u32, usize)> = scene
            .objects
            .iter()
            .filter(|o| o.stamp.is_some())
            .map(|o| (o.ghost, o.draws.iter().map(|d| d.indices.len()).sum()))
            .collect();
        let (index_runs, index_moved) = self.index_runs.place(&stamped_indices);
        if index_moved {
            self.index_runs.forget_vertices();
        }
        let mut next_index_run = index_runs.iter();
        let index_rest = self.index_runs.capacity;
        // The stamped objects' index runs, ghosts, draws and stamps.
        let mut index_writes: Vec<(slots::Run, u32, &[HdDraw], u64)> = Vec::new();
        // (After the runs' room even with no stamped object now: a whole sync's upload leaves the runs as they are.)
        let rest_start = self.runs.capacity;
        // The rest's draws in order (written after the runs) with their objects' first places, each stamped object's draws at its
        // run.
        let mut rest: Vec<(&HdDraw, u32)> = Vec::new();
        let mut rest_count = 0usize;
        let mut indices: Vec<u32> = Vec::new();
        let draw_ref = |d: &HdDraw, base: usize, first: usize| -> DrawRef {
            DrawRef {
                first: first as u32,
                count: d.indices.len() as u32,
                base_vertex: base as i32,
                tex: d.texture.map_or(TexRef::White, |(i, _)| TexRef::Table(i)),
                sampler: d
                    .texture
                    .map_or(SamplerKey::PLAIN, |(_, s)| SamplerKey::of(&s)),
                blend: d.blend,
                cull: d.cull,
                depth_test: d.depth_test,
                depth_write: d.depth_write,
                motion: 0,
                particle: false,
                late: false,
                shadow_only: d
                    .vertices
                    .first()
                    .is_some_and(|v| torque_render_scene::hd::material::shadow_only(v.material)),
                view_only: d
                    .vertices
                    .first()
                    .is_some_and(|v| torque_render_scene::hd::material::view_only(v.material)),
            }
        };
        let draw_at = |d: &HdDraw, base: usize, indices: &mut Vec<u32>| -> DrawRef {
            let first = index_rest + indices.len();
            indices.extend_from_slice(&d.indices);
            draw_ref(d, base, first)
        };
        let mut loose = Loose {
            generation: Some(scene.generation),
            static_generation: Some(scene.generation.wrapping_sub(scene.control_generation)),
            ..Loose::default()
        };
        let add_rest = |d: &HdDraw, rest_count: &mut usize, indices: &mut Vec<u32>| -> DrawRef {
            let r = draw_at(d, rest_start + *rest_count, indices);
            *rest_count += d.vertices.len();
            r
        };
        for d in &scene.sky {
            loose.sky.push(add_rest(d, &mut rest_count, &mut indices));
            rest.push((d, 0));
        }
        loose.sky_labels = scene.sky.iter().map(|d| d.label.clone()).collect();
        let (planes_from, planes_indices_from, planes_rest_from) =
            (rest_count, indices.len(), rest.len());
        for d in &scene.planes {
            loose
                .planes
                .push(add_rest(d, &mut rest_count, &mut indices));
            rest.push((d, 0));
        }
        let planes_rest = planes_rest_from..rest.len();
        let planes_at = PlanesAt {
            generation: scene.planes_generation,
            planes: scene
                .planes
                .iter()
                .map(|d| {
                    [
                        d.vertices.as_ptr() as usize,
                        d.vertices.len(),
                        d.indices.as_ptr() as usize,
                        d.indices.len(),
                    ]
                })
                .collect(),
            rest_start,
            index_rest,
            vertices: (planes_from, rest_count),
            indices: (planes_indices_from, indices.len()),
        };
        for d in &scene.water {
            loose.water.push(add_rest(d, &mut rest_count, &mut indices));
            rest.push((d, 0));
        }
        // Where each shape that brings its own previous positions starts (a vertex of the buffer).
        let mut own_previous: Vec<(usize, &[[f32; 3]])> = Vec::new();
        // The stamped objects: their runs, ghosts, draws, stamps, whether their previous positions are where they are, and their
        // first places.
        let mut writes: Vec<RunWrite<'_>> = Vec::new();
        let mut next_run = runs.iter();
        // The objects whose own previous positions are where their vertices are (they did not move).
        let mut still_objects: Vec<bool> = Vec::with_capacity(scene.objects.len());
        // Each shape's draw has a motion slot of its own (its object's matrix and the draw's id: a Player's parts are told apart).
        let mut slot = 0u32;
        for o in &scene.objects {
            let n: usize = o.draws.iter().map(|d| d.vertices.len()).sum();
            let previous = o.previous.as_deref().filter(|p| p.len() == n);
            let stamp = o.stamp;
            let run = stamp.and_then(|_| next_run.next().copied());
            let node_base = if o.nodes.is_empty() {
                None
            } else if let Some(r) = stamp.and_then(|_| next_node_run.next()) {
                Some(r.start)
            } else {
                node_end += o.nodes.len();
                Some(node_end - o.nodes.len())
            };
            let based = node_base.map_or(0, |b| b as u32);
            let index_run = stamp.and_then(|_| next_index_run.next().copied());
            if let (Some(r), Some(s)) = (index_run, stamp) {
                index_writes.push((r, o.ghost, &o.draws, s));
            }
            let mut at = run.map(|r| r.start);
            let mut index_at = index_run.map_or(0, |r| r.start);
            let mut draws: Vec<DrawRef> = Vec::with_capacity(o.draws.len());
            for d in &o.draws {
                slot += 1;
                let r = match &mut at {
                    Some(a) if index_run.is_some() => {
                        let r = draw_ref(d, *a, index_at);
                        *a += d.vertices.len();
                        index_at += d.indices.len();
                        r
                    }
                    Some(a) => {
                        let r = draw_at(d, *a, &mut indices);
                        *a += d.vertices.len();
                        r
                    }
                    None => {
                        rest.push((d, based));
                        add_rest(d, &mut rest_count, &mut indices)
                    }
                };
                draws.push(DrawRef {
                    motion: slot,
                    late: E::drawn_late(o.ghost),
                    ..r
                });
            }
            // A draw's id is its object's, its place among them and its mesh's size: an item swapped for another in the same
            // place is another shape to the TAA (its pixels hold none of the old one's history), not the same shape changed.
            let ids = o
                .draws
                .iter()
                .enumerate()
                .map(|(k, d)| {
                    let mesh = (d.vertices.len() as u32)
                        .wrapping_mul(2_654_435_761)
                        .wrapping_add((d.indices.len() as u32).wrapping_mul(40_503));
                    let n = o
                        .ghost
                        .wrapping_mul(31)
                        .wrapping_add(k as u32)
                        .wrapping_add(mesh);
                    (1 + n % post::MOTION_IDS) as f32
                })
                .collect();
            if let (Some(p), Some(first)) = (previous, draws.first()) {
                own_previous.push((first.base_vertex as usize, p));
            }
            let mut own_still = false;
            if let (Some(r), Some(s)) = (run, stamp) {
                // Still: its previous positions are where its vertices are.
                own_still = previous.is_some_and(|p| {
                    o.draws
                        .iter()
                        .flat_map(|d| &d.vertices)
                        .zip(p)
                        .all(|(v, q)| v.position.map(f32::to_bits) == q.map(f32::to_bits))
                });
                writes.push((
                    r,
                    o.ghost,
                    &o.draws,
                    s,
                    previous.is_none() || own_still,
                    node_base,
                ));
            }
            still_objects.push(own_still);
            loose.objects.push(GpuObject {
                ghost: o.ghost,
                late: E::drawn_late(o.ghost),
                centre: o.centre,
                radius: o.radius,
                translucent: o.translucent,
                draws,
                motion: if previous.is_some() {
                    torque_render_scene::hd::post::COLUMN_IDENTITY
                } else {
                    o.motion()
                },
                ids,
                node_base,
            });
        }
        for (d, c) in &scene.coronas {
            let anchor = c.vertices.first().map_or([f32::NAN; 3], |v| v.position);
            loose
                .coronas
                .push((*d, add_rest(c, &mut rest_count, &mut indices), anchor));
            rest.push((c, 0));
        }
        if let Some(d) = &scene.flare {
            loose.flare = Some(add_rest(d, &mut rest_count, &mut indices));
            rest.push((d, 0));
        }
        // The meshes' places: each object's at its first, the rows of its placement now, its normals' turn and its placement at
        // the last frame.
        self.upload_nodes(scene, &loose.objects, node_end);
        if !indices.is_empty() || !index_writes.is_empty() {
            // A window syncs its control object every frame: the buffers are written in place while they are big enough (made
            // anew, with room to grow, only when they are not), the runs that hold their stamps' vertices not at all.
            let old = std::mem::take(&mut self.loose);
            let total = (rest_start + rest_count) * HD_VERTEX_BYTES;
            let (vertices, made) = self.buffer_of(
                old.vertices,
                total,
                wgpu::BufferUsages::VERTEX,
                "hd loose vertices",
            );
            if made {
                self.runs.forget_vertices();
            }
            // The planes as the last upload wrote them, in the buffer kept: not written again.
            let planes_kept = !made && old.planes_at.as_ref() == Some(&planes_at);
            let old_previous_planes = old.previous.is_some() && old.planes_previous;
            // (An upload that needs none keeps the buffer aside, its runs as they are: a cells-only sync clears the shapes' own
            // previous positions for a frame.)
            let kept_previous = old.previous.or_else(|| self.spare_previous.take());
            let mut previous_made = false;
            let previous = if own_previous.is_empty() {
                self.spare_previous = kept_previous;
                None
            } else {
                let (b, made) = self.buffer_of(
                    kept_previous,
                    total,
                    wgpu::BufferUsages::VERTEX,
                    "hd loose previous positions",
                );
                if made {
                    self.runs.forget_previous();
                }
                previous_made = made;
                Some(b)
            };
            let previous_planes_kept = planes_kept && old_previous_planes && !previous_made;
            // The shapes' own last-frame positions written over the vertex bytes `bytes` (from vertex `start` on).
            let patch = |bytes: &mut [u8], start: usize| {
                let end = start + bytes.len() / HD_VERTEX_BYTES;
                for (base, positions) in &own_previous {
                    if base + positions.len() <= start || *base >= end {
                        continue;
                    }
                    for (k, p) in positions.iter().enumerate() {
                        let v = base + k;
                        if v < start || v >= end {
                            continue;
                        }
                        let at = (v - start) * HD_VERTEX_BYTES;
                        for (c, x) in p.iter().enumerate() {
                            bytes[at + 4 * c..at + 4 * c + 4].copy_from_slice(&x.to_le_bytes());
                        }
                    }
                }
            };
            // The runs to write, in spans of runs side by side (a few held ones between two to write go too: one write a span).
            let need_vertices: Vec<bool> = writes
                .iter()
                .map(|w| w.0.written != Some(w.3) || self.node_baked.get(&w.1).copied() != w.5)
                .collect();
            let need_previous: Vec<bool> = writes
                .iter()
                .map(|w| previous.is_some() && !(w.4 && w.0.previous == Some(w.3)))
                .collect();
            let mut order: Vec<usize> = (0..writes.len()).collect();
            order.sort_by_key(|&i| writes[i].0.start);
            let need_any: Vec<bool> = need_vertices
                .iter()
                .zip(&need_previous)
                .map(|(v, p)| *v || *p)
                .collect();
            // Each span encoded once (the spans over a few threads: torque_render_scene::par), into whichever buffers it has runs to
            // write in (a held run in it is written again as it is); then the rest's draws after the runs.
            let spans = slots::spans(&order, |i| writes[i].0, &need_any);
            let has_previous = previous.is_some();
            let encoded = torque_render_scene::par::map(
                &spans,
                |span| span.iter().map(|&i| writes[i].0.len).sum(),
                |span| {
                    let start = writes[span[0]].0.start;
                    let len: usize = span.iter().map(|&i| writes[i].0.len).sum();
                    let mut bytes: Vec<u8> = Vec::with_capacity(len * HD_VERTEX_BYTES);
                    for &i in span {
                        let base = writes[i].5.map_or(0, |b| b as u32);
                        for d in writes[i].2 {
                            HdVertex::extend_bytes_based(&d.vertices, base, &mut bytes);
                        }
                    }
                    let write_vertices = span.iter().any(|&i| need_vertices[i]);
                    let prev =
                        (has_previous && span.iter().any(|&i| need_previous[i])).then(|| {
                            let mut p = bytes.clone();
                            patch(&mut p, start);
                            p
                        });
                    (start, write_vertices.then_some(bytes), prev)
                },
            );
            for (start, bytes, prev) in &encoded {
                let offset = (start * HD_VERTEX_BYTES) as u64;
                if let Some(b) = bytes.as_ref().filter(|b| !b.is_empty()) {
                    self.queue.write_buffer(&vertices, offset, b);
                }
                if let (Some(pb), Some(p)) = (&previous, prev.as_ref().filter(|p| !p.is_empty())) {
                    self.queue.write_buffer(pb, offset, p);
                }
            }
            for (_, ghost, _, stamp, still, base) in &writes {
                let held_previous = (previous.is_some() && *still).then_some(*stamp);
                self.runs.set(*ghost, Some(*stamp), held_previous);
                match base {
                    Some(b) => self.node_baked.insert(*ghost, *b),
                    None => self.node_baked.remove(ghost),
                };
            }
            // The rest's draws in a few parts side by side (each part's vertices from its first on), the planes' parts apart: a
            // buffer that keeps them is not written there.
            let mut firsts = Vec::with_capacity(rest.len());
            let mut at = rest_start;
            for (d, base) in &rest {
                firsts.push((at, *d, *base));
                at += d.vertices.len();
            }
            let chunk = |v: &[(usize, &HdDraw, u32)]| {
                v.len()
                    .div_ceil(torque_render_scene::par::MAX_THREADS)
                    .max(1)
            };
            let (head, tail) = firsts.split_at(planes_rest.end);
            let (before, planes) = head.split_at(planes_rest.start);
            let mut parts: Vec<RestPart<'_>> = Vec::new();
            parts.extend(before.chunks(chunk(before)).map(|p| (p, true, true)));
            parts.extend(
                planes
                    .chunks(chunk(planes))
                    .map(|p| (p, !planes_kept, !previous_planes_kept)),
            );
            parts.extend(tail.chunks(chunk(tail)).map(|p| (p, true, true)));
            parts.retain(|p| p.1 || (p.2 && previous.is_some()));
            let rest_bytes = torque_render_scene::par::map(
                &parts,
                |p| p.0.iter().map(|(_, d, _)| d.vertices.len()).sum(),
                |&(p, write_vertices, write_previous)| {
                    let start = p.first().map_or(rest_start, |(s, _, _)| *s);
                    let mut bytes = Vec::new();
                    for (_, d, base) in p.iter() {
                        HdVertex::extend_bytes_based(&d.vertices, *base, &mut bytes);
                    }
                    let prev = (has_previous && write_previous).then(|| {
                        let mut b = bytes.clone();
                        patch(&mut b, start);
                        b
                    });
                    (start, write_vertices.then_some(bytes), prev)
                },
            );
            for (start, bytes, prev) in &rest_bytes {
                let offset = (start * HD_VERTEX_BYTES) as u64;
                if let Some(b) = bytes.as_ref().filter(|b| !b.is_empty()) {
                    self.queue.write_buffer(&vertices, offset, b);
                }
                if let (Some(pb), Some(p)) = (&previous, prev.as_ref().filter(|p| !p.is_empty())) {
                    self.queue.write_buffer(pb, offset, p);
                }
            }
            loose.vertices = Some(vertices);
            loose.planes_previous = previous.is_some();
            loose.previous = previous;
            // The indices: the stamped objects' runs that do not hold their stamps' (all when the buffer is new), then the rest's.
            let (ibuf, made) = self.buffer_of(
                old.indices,
                (index_rest + indices.len()) * 4,
                wgpu::BufferUsages::INDEX,
                "hd loose indices",
            );
            if made {
                self.index_runs.forget_vertices();
            }
            let index_planes_kept = planes_kept && !made;
            for (r, ghost, draws, stamp) in &index_writes {
                if r.written == Some(*stamp) && !made {
                    continue;
                }
                let mut ib = Vec::with_capacity(r.len * 4);
                for d in *draws {
                    push_u32(&mut ib, &d.indices);
                }
                if !ib.is_empty() {
                    self.queue.write_buffer(&ibuf, (r.start * 4) as u64, &ib);
                }
                self.index_runs.set(*ghost, Some(*stamp), None);
            }
            // The rest's indices, the planes' left out when the buffer keeps them.
            let (pf, pt) = planes_at.indices;
            loose.planes_at = Some(planes_at);
            let spans: Vec<(usize, &[u32])> = if index_planes_kept {
                vec![(0, &indices[..pf]), (pt, &indices[pt..])]
            } else {
                vec![(0, &indices[..])]
            };
            for (from, part) in spans {
                if part.is_empty() {
                    continue;
                }
                let mut ib = Vec::with_capacity(part.len() * 4);
                push_u32(&mut ib, part);
                self.queue
                    .write_buffer(&ibuf, ((index_rest + from) * 4) as u64, &ib);
            }
            loose.indices = Some(ibuf);
        }
        self.loose = loose;
    }

    /// The meshes' places of the frame's objects into the table (`end` places in all; made anew with room to grow when it is too
    /// small): each object's at its first place.
    fn upload_nodes(&mut self, scene: &E::Scene, objects: &[GpuObject], end: usize) {
        if end == 0 {
            return;
        }
        if (self.nodes.size() as usize) < end * NODE_BYTES {
            self.nodes = Self::nodes_buffer(&self.device, end + end / 4 + 64);
        }
        let mut bytes = vec![0u8; end * NODE_BYTES];
        for (o, g) in scene.objects.iter().zip(objects) {
            let Some(base) = g.node_base else {
                continue;
            };
            for (k, p) in o.nodes.iter().enumerate() {
                let at = (base + k) * NODE_BYTES;
                let Some(out) = bytes.get_mut(at..at + NODE_BYTES) else {
                    continue;
                };
                let rows = [&p.now, &p.normal, &p.before]
                    .into_iter()
                    .flat_map(|m| m.m[..12].iter().copied());
                for (c, v) in out.as_chunks_mut::<4>().0.iter_mut().zip(rows) {
                    c.copy_from_slice(&v.to_le_bytes());
                }
            }
        }
        self.queue.write_buffer(&self.nodes, 0, &bytes);
    }

    /// `old` when it holds `size` bytes, else a new buffer with a quarter more room (and true); nothing written.
    fn buffer_of(
        &self,
        old: Option<wgpu::Buffer>,
        size: usize,
        usage: wgpu::BufferUsages,
        label: &'static str,
    ) -> (wgpu::Buffer, bool) {
        let size = (size as u64).next_multiple_of(wgpu::COPY_BUFFER_ALIGNMENT);
        if let Some(b) = old.filter(|b| {
            b.size() >= size && b.usage().contains(usage | wgpu::BufferUsages::COPY_DST)
        }) {
            return (b, false);
        }
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: (size + size / 4)
                .next_multiple_of(wgpu::COPY_BUFFER_ALIGNMENT)
                .max(16),
            // (Read back by `HdWorld::check_loose`.)
            usage: usage | wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        (buffer, true)
    }

    /// The targets for an output of `width` x `height` drawn at `iw` x `ih`, the AO at 1 / `ao_scale` of that; `true` when they
    /// were made anew (the histories start again).
    fn ensure_targets(
        &mut self,
        width: u32,
        height: u32,
        iw: u32,
        ih: u32,
        ao_scale: u32,
        uses: TargetUse,
    ) -> Result<bool, HdError> {
        if width == 0 || height == 0 || width > MAX_SIDE || height > MAX_SIDE {
            return Err(format!("a frame of {width}x{height} cannot be drawn"));
        }
        let maps = self.shadows.maps_made();
        if self.targets.as_ref().is_some_and(|t| {
            (
                t.width,
                t.height,
                t.iw,
                t.ih,
                t.ao_scale,
                t.uses,
                t.shadow_maps,
            ) == (width, height, iw, ih, ao_scale, uses, maps)
        }) {
            return Ok(false);
        }
        // The old targets go first (a switch never holds both sets).
        self.targets = None;
        let device = &self.device;
        let tex = |label: &str, (w, h): (u32, u32), format, usage| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
        };
        let view = |t: &wgpu::Texture| t.create_view(&wgpu::TextureViewDescriptor::default());
        let target = wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING;
        let inner = (iw, ih);
        let outer = (width, height);
        let ao_size = (iw.div_ceil(ao_scale), ih.div_ceil(ao_scale));
        // A feature off gets 1 x 1 stand-ins (its passes do not run; the groups that name them stay valid).
        let one = (1, 1);
        let taa_inner = if uses.taa { inner } else { one };
        let taa_outer = if uses.taa { outer } else { one };
        let ao_size = if uses.ao { ao_size } else { one };
        let hdr = tex(
            "hd hdr",
            inner,
            HDR_FORMAT,
            target | wgpu::TextureUsages::COPY_SRC,
        );
        let hdr_view = view(&hdr);
        let depth = tex(
            "hd depth",
            inner,
            DEPTH_FORMAT,
            target | wgpu::TextureUsages::COPY_SRC,
        );
        let depth_view = view(&depth);
        let prepass_depths = [0, 1].map(|_| {
            tex(
                "hd prepass depth",
                taa_inner,
                DEPTH_FORMAT,
                wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            )
        });
        let prepass_depth_views = [view(&prepass_depths[0]), view(&prepass_depths[1])];
        // The prepass writes this frame's motion; only the TAA reads the last frame's (without it one target serves both parities).
        let motion_use = target | wgpu::TextureUsages::COPY_SRC;
        let motion = tex("hd motion", inner, MOTION_FORMAT, motion_use);
        let motions = if uses.taa {
            [
                motion.clone(),
                tex("hd motion", inner, MOTION_FORMAT, motion_use),
            ]
        } else {
            [motion.clone(), motion]
        };
        let motion_views = [view(&motions[0]), view(&motions[1])];
        let normal_view = view(&tex("hd normal", inner, NORMAL_FORMAT, target));
        let contact_view = view(&tex(
            "hd contact",
            if uses.contact { inner } else { one },
            CONTACT_FORMAT,
            target,
        ));
        let ao_views = [0, 1].map(|_| view(&tex("hd ao", ao_size, AO_FORMAT, target)));
        let ao_smooth_view = view(&tex("hd ao smooth", ao_size, AO_FORMAT, target));
        let history = [0, 1].map(|_| {
            tex(
                "hd taa history",
                taa_outer,
                HISTORY_FORMAT,
                target | wgpu::TextureUsages::COPY_SRC,
            )
        });
        let history_views = [view(&history[0]), view(&history[1])];
        let readable = target | wgpu::TextureUsages::COPY_SRC;
        let shown = [0, 1].map(|_| tex("hd taa shown", taa_outer, SHOWN_FORMAT, readable));
        let shown_views = [view(&shown[0]), view(&shown[1])];
        let fxaa = tex(
            "hd fxaa",
            if uses.fxaa { inner } else { one },
            HDR_FORMAT,
            readable,
        );
        let fxaa_view = view(&fxaa);
        let frame_entry = |binding: u32| wgpu::BindGroupEntry {
            binding,
            resource: self.frame.as_entire_binding(),
        };
        let contact_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("hd contact"),
            layout: &self.contact_layout,
            entries: &[
                frame_entry(0),
                tv(1, &depth_view),
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: self.shadows.uniform.as_entire_binding(),
                },
            ],
        });
        let ao_groups = [0, 1].map(|p| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("hd ao"),
                layout: &self.post.ao_layout,
                entries: &[
                    frame_entry(0),
                    tv(1, &depth_view),
                    tv(2, &normal_view),
                    tv(3, &motion_views[p]),
                    tv(4, &ao_views[1 - p]),
                ],
            })
        });
        let taa_groups = [0, 1].map(|p| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("hd taa"),
                layout: &self.post.taa_layout,
                entries: &[
                    frame_entry(0),
                    tv(1, &hdr_view),
                    tv(2, &prepass_depth_views[p]),
                    tv(3, &prepass_depth_views[1 - p]),
                    tv(4, &motion_views[p]),
                    tv(5, &motion_views[1 - p]),
                    tv(6, &history_views[1 - p]),
                    tv(7, &shown_views[1 - p]),
                    wgpu::BindGroupEntry {
                        binding: 8,
                        resource: wgpu::BindingResource::Sampler(&self.post.linear),
                    },
                ],
            })
        });
        let accum_view = view(&tex("hd wboit accum", inner, ACCUM_FORMAT, target));
        let reveal_view = view(&tex("hd wboit reveal", inner, REVEAL_FORMAT, target));
        let trans_depth = tex(
            "hd translucent front depth",
            inner,
            DEPTH_FORMAT,
            target | wgpu::TextureUsages::COPY_DST,
        );
        let trans_depth_view = view(&trans_depth);
        let fxaa_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("hd fxaa"),
            layout: &self.post.fxaa_layout,
            entries: &[
                tv(0, &hdr_view),
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.post.linear),
                },
            ],
        });
        let background = tex(
            "hd background",
            inner,
            HDR_FORMAT,
            wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        );
        // With its box-filtered mips: the fog's sky without its detail.
        let sky_copy = capture::Chained::new(
            device,
            &self.refl.down,
            "hd sky copy",
            inner,
            sky::FOG_LOW_LEVEL + 1,
            wgpu::TextureUsages::COPY_DST,
        );
        self.sky
            .bind_sky_copy(device, &view(&sky_copy.texture), sky_copy.levels - 1);
        let background_view = view(&background);
        let comp_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("hd wboit composite"),
            layout: &self.comp_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.comp_uniform.as_entire_binding(),
                },
                tv(1, &background_view),
                tv(2, &accum_view),
                tv(3, &reveal_view),
            ],
        });
        let ldr = tex(
            "hd ldr",
            outer,
            LDR_FORMAT,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let late_view = view(&tex("hd late sprites", taa_inner, HDR_FORMAT, target));
        let over_view = view(&tex("hd late over", taa_inner, HDR_FORMAT, target));
        let present = tex(
            "hd present",
            taa_outer,
            post::SHOWN_FORMAT,
            readable | wgpu::TextureUsages::COPY_DST,
        );
        let present_view = view(&present);
        let late_uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("hd late sprites"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut scale = Vec::new();
        push_f32(
            &mut scale,
            &[
                iw as f32 / width as f32,
                ih as f32 / height as f32,
                iw as f32,
                ih as f32,
            ],
        );
        self.queue.write_buffer(&late_uniform, 0, &scale);
        let late_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("hd late sprites"),
            layout: &self.late_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: late_uniform.as_entire_binding(),
                },
                tv(1, &late_view),
                tv(2, &over_view),
            ],
        });
        self.targets = Some(Targets {
            width,
            height,
            iw,
            ih,
            ao_scale,
            uses,
            hdr_view,
            hdr,
            depth,
            depth_view,
            prepass_depths,
            motions,
            motion_views,
            normal_view,
            contact_view,
            contact_group,
            ao_views,
            ao_groups,
            ao_smooth_view,
            history,
            history_views,
            shown,
            shown_views,
            taa_groups,
            fxaa,
            fxaa_view,
            fxaa_group,
            accum_view,
            reveal_view,
            trans_depth,
            trans_depth_view,
            comp_group,
            background_view,
            background,
            sky_copy,
            ldr_view: view(&ldr),
            ldr,
            late_view,
            over_view,
            shadow_maps: maps,
            present,
            present_view,
            late_group,
        });
        Ok(true)
    }

    /// The frame uniform for the (jittered, internal) `camera`; `post` turns on `common.wgsl`'s `prev_view_proj`; `main` the main
    /// view's (a secondary view, the water's mirrored camera or a capture face, is never under water: its eye is not the camera's).
    fn frame_bytes(&self, scene: &E::Scene, camera: &HdCamera, post: &[u8], main: bool) -> Vec<u8> {
        let mut b = Vec::with_capacity(frame_size::<E>());
        push_f32(&mut b, &camera.view_proj);
        push_f32(&mut b, &camera.view);
        push_f32(&mut b, &invert(&camera.proj));
        let e = camera.eye;
        push_f32(&mut b, &[e[0], e[1], e[2], E::time(scene)]);
        let s = &scene.sun;
        push_f32(&mut b, &[s.towards[0], s.towards[1], s.towards[2], 0.0]);
        push_f32(&mut b, &[s.diffuse[0], s.diffuse[1], s.diffuse[2], 0.0]);
        push_f32(&mut b, &[s.ambient[0], s.ambient[1], s.ambient[2], 0.0]);
        let lin = [0, 1, 2].map(|k| {
            (srgb_to_linear((s.ambient[k] + s.diffuse[k]).max(0.0))
                - srgb_to_linear(s.ambient[k].max(0.0)))
            .max(0.0)
        });
        push_f32(&mut b, &[lin[0], lin[1], lin[2], 0.0]);
        for c in &scene.sky_sh {
            push_f32(&mut b, &[c[0], c[1], c[2], 0.0]);
        }
        // The fog always hides the far plane's cut: a Sky whose fog starts at or past its visible distance
        // fogs the last [`FAR_FOG_SHARE`] of it.
        let (fog_start, fog_end) = fog_range(scene.fog.start, scene.fog.end);
        let fog_on = (fog_end > fog_start && fog_start < camera.far) || E::fog_fades(scene);
        let fc = scene.fog.color.map(srgb_to_linear);
        push_f32(
            &mut b,
            &[fc[0], fc[1], fc[2], if fog_on { 1.0 } else { 0.0 }],
        );
        // z: 1 for TGE's linear fog (`HdFog::linear`).
        let linear = if scene.fog.linear { 1.0 } else { 0.0 };
        // w: 1 when some heading fades the extension's static geometry before the fog (its fog ends, in the sky's uniform).
        let fades = if E::fog_fades(scene) { 1.0 } else { 0.0 };
        push_f32(&mut b, &[fog_start, fog_end, linear, fades]);
        let (w, h) = (camera.width as f32, camera.height as f32);
        push_f32(&mut b, &[w, h, 1.0 / w, 1.0 / h]);
        push_u32(
            &mut b,
            &[
                CLUSTER_GRID[0],
                CLUSTER_GRID[1],
                CLUSTER_GRID[2],
                self.light_count as u32,
            ],
        );
        let (near, far) = (camera.near, camera.far.max(camera.near * 2.0));
        push_f32(
            &mut b,
            &[near, far, CLUSTER_GRID[2] as f32 / (far / near).ln(), 0.0],
        );
        let mode = E::mode(scene).unwrap_or_else(E::fallback_mode);
        let quality = mode.quality;
        let surfaces = varied_ground(self.ext.surfaces(scene), scene.ground_variation);
        push_f32(&mut b, &surfaces.light);
        let per_cluster = quality.lights_per_cluster();
        push_u32(&mut b, &[per_cluster, 0, 0, 0]);
        push_f32(&mut b, &camera.proj);
        b.extend_from_slice(post);
        // The surfaces.
        let coloured = self.features.coloured_shadows && quality.coloured_shadows();
        push_f32(&mut b, &surfaces.table);
        // x 1 for WBOIT, y 1 for coloured sun shadows; z 1 where no TAA averages a per-pixel pattern (the anti-aliasing off or
        // the FXAA-style filter, a debug view): the near detail's fade is a cut there; w 1 where the frame is shown through the
        // FXAA-style filter (Low): the shadows' filter keeps a fixed pattern (no speckle). Then last frame's effect time.
        let taa = self.features.anti_aliasing
            && quality.post().aa == torque_render_scene::hd::post::AntiAliasing::Taa
            && self.debug == DebugView::Off;
        let fxaa = self.features.anti_aliasing
            && quality.post().aa == torque_render_scene::hd::post::AntiAliasing::Fxaa;
        let flag = |v: bool| if v { 1.0 } else { 0.0 };
        push_f32(
            &mut b,
            &[
                flag(self.features.wboit),
                flag(coloured),
                flag(!taa),
                flag(fxaa),
            ],
        );
        push_f32(
            &mut b,
            &[
                self.temporal.prev_time.unwrap_or(E::time(scene)),
                0.0,
                0.0,
                0.0,
            ],
        );
        // The sky's way from where its draws were built to this camera's eye.
        let e = camera.eye;
        let s = scene.sky_eye;
        // (w: the count of `movers`, none: no pass reads them now; the block keeps its place for the extension's after it.)
        push_f32(&mut b, &[e[0] - s[0], e[1] - s[1], e[2] - s[2], 0.0]);
        let drawn_as_water = self.features.reflections && !self.loose.water.is_empty();
        let terms = water::frame_terms(
            &**scene,
            E::water_zone(scene),
            camera.eye,
            drawn_as_water,
            main,
        );
        push_f32(&mut b, &terms.water);
        push_f32(&mut b, &terms.under);
        b.resize(b.len() + 16 * MOVING_CASTERS, 0);
        b.extend_from_slice(&self.ext.frame_ext(scene));
        debug_assert_eq!(b.len(), frame_size::<E>());
        b
    }

    /// Starts the histories again (as a cut does): the next frame is the first of a new history, its jitter phase 0. A frame drawn
    /// after the same frames since a reset is the same frame.
    pub fn reset_history(&mut self) {
        self.temporal = Temporal::default();
        self.refl.reset();
        self.exposure.reset();
        self.clouds.reset();
    }

    /// Draws `scene` (uploaded with [`HdWorld::upload`]) from `camera` and tonemaps it into `target` (an `Rgba8Unorm` view, or
    /// `None` for the world's own image, [`HdWorld::read_ldr`]) at `origin` (the viewport's top left in the target). Submits the work.
    pub fn draw(
        &mut self,
        scene: &E::Scene,
        camera: &HdCamera,
        target: Option<(&wgpu::TextureView, wgpu::TextureFormat, [u32; 2])>,
    ) -> Result<(), HdError> {
        let cpu_start = web_time::Instant::now();
        let mut cpu_at = cpu_start;
        let mut cpu_marks: Vec<(&'static str, f64)> = Vec::new();
        self.take_warmed(false);
        self.warm_queued(WEB_WARM_PER_FRAME);
        let passes_before = PASSES.load(std::sync::atomic::Ordering::Relaxed);
        let kinds_before = passes_by_kind();
        let _ = set_pass_kind(PassKind::Post);
        // The CPU's time since the last mark under `label` (the steps before the frame's encoder: no GPU timestamp).
        macro_rules! cpu_mark {
            ($label:expr) => {
                if self.profile_passes || self.cpu_marks {
                    let now = web_time::Instant::now();
                    cpu_marks.push(($label, (now - cpu_at).as_secs_f64() * 1000.0));
                    cpu_at = now;
                }
            };
        }
        self.ext.set_detail(self.features.ext_detail);
        self.ext.fill_slots(&mut self.ext_slots);
        self.refl.set_ext(&self.ext_slots.reflect);
        self.upload_particles(scene);
        // The glow lights switched on or off since the lights went up.
        if self.uploaded_glow != self.features.glow_lights {
            self.upload_lights(scene);
        }
        cpu_mark!("begin: particles");
        let mode = E::mode(scene).unwrap_or_else(E::fallback_mode);
        let (quality, image) = (mode.quality, mode.image);
        let surfaces = varied_ground(self.ext.surfaces(scene), scene.ground_variation);
        let pq = quality.post();
        let (iw, ih) = internal_size(
            camera.width,
            camera.height,
            self.render_scale.scale(&pq, camera.width, camera.height),
        );
        let ao_scale = if pq.ao_full_res { 1 } else { 2 };
        let ao_on = self.features.ambient_occlusion && pq.ao != AoMethod::Off;
        let plain = self.debug == DebugView::Off;
        let mut extras = ExtraDrawn::default();
        let wboit = self.features.wboit && plain;
        let taa = self.features.anti_aliasing && pq.aa == AntiAliasing::Taa && plain;
        let fxaa = self.features.anti_aliasing && pq.aa == AntiAliasing::Fxaa && plain;
        let uses = TargetUse {
            taa,
            fxaa,
            ao: ao_on,
            contact: self.features.contact_shadows && quality.shadows().contact_steps > 0,
        };
        // The shadow maps first: the targets' TAA groups read them (and are made again with them).
        self.shadows.ensure(
            &self.device,
            quality.shadows().for_distance(scene.shadow_distance),
            self.features.coloured_shadows
                && quality.coloured_shadows()
                && self.features.sun_shadows,
        );
        let remade = self.ensure_targets(camera.width, camera.height, iw, ih, ao_scale, uses)?;
        if remade {
            self.targets_made += 1;
        }
        if remade || self.temporal.is_cut(E::mode(scene), camera.eye) {
            self.reset_history();
        }
        if !self.histories.clouds {
            self.clouds.reset();
        }
        if !self.histories.exposure {
            self.exposure.reset();
        }
        cpu_mark!("begin: targets");
        let icam = camera.sized(iw, ih);
        let j = if taa {
            jitter(self.temporal.frame)
        } else {
            [0.0, 0.0]
        };
        let jcam = icam.jittered(j);
        let p = self.temporal.parity;
        // The shapes moved between the last frame and this one when the draws were synced again in between.
        let moved = self
            .temporal
            .generation
            .is_some_and(|g| Some(g) != self.loose.generation);
        let motions: Vec<([f32; 16], f32)> = self
            .loose
            .objects
            .iter()
            .flat_map(|o| {
                let m = if moved {
                    o.motion
                } else {
                    torque_render_scene::hd::post::COLUMN_IDENTITY
                };
                o.ids.iter().map(move |&id| (m, id))
            })
            .collect();
        self.post.write_motion(&self.device, &self.queue, &motions);
        cpu_mark!("begin: motion");
        let mut post_bytes = Vec::with_capacity(256);
        push_f32(
            &mut post_bytes,
            &self.temporal.prev_view_proj.unwrap_or(icam.view_proj),
        );
        push_f32(&mut post_bytes, &icam.view_proj);
        push_f32(&mut post_bytes, &invert(&icam.view_proj));
        push_f32(
            &mut post_bytes,
            &[
                j[0],
                j[1],
                (self.temporal.frame % 1024) as f32,
                if self.temporal.taa_written && self.histories.taa {
                    1.0
                } else {
                    0.0
                },
            ],
        );
        let method = match pq.ao {
            AoMethod::Off => 0.0,
            AoMethod::Ssao => 1.0,
            AoMethod::Gtao => 2.0,
        };
        push_f32(
            &mut post_bytes,
            &[
                scene.ao_radius,
                image.ao_strength(),
                if ao_on { method } else { 0.0 },
                ao_scale as f32,
            ],
        );
        push_f32(
            &mut post_bytes,
            &[
                if self.temporal.ao_written { 1.0 } else { 0.0 },
                0.0,
                0.0,
                0.0,
            ],
        );
        let (ow, oh) = (camera.width as f32, camera.height as f32);
        push_f32(&mut post_bytes, &[ow, oh, 1.0 / ow, 1.0 / oh]);
        let frame = self.frame_bytes(scene, &jcam, &post_bytes, true);
        self.queue.write_buffer(&self.frame, 0, &frame);
        if taa {
            let still = self.frame_bytes(scene, &icam, &post_bytes, true);
            self.queue.write_buffer(&self.frame_still, 0, &still);
        }
        cpu_mark!("begin: frame bytes");
        let format = target.map_or(LDR_FORMAT, |t| t.1);
        self.ensure_tonemap(format);
        self.ensure_lut(&image);

        // What is drawn, in order, with its pipeline and texture.
        let clustered = !self.brute_force_lights;
        let eye = Vec3::from(camera.eye);
        let dist = |c: [f32; 3]| Vec3::from(c).distance(eye);
        let planes = camera.planes();
        let sees_sphere = |c: [f32; 3], r: f32| crate::sphere_seen(&planes, c, r);
        enum Item {
            Loose(DrawRef),
            /// A cell's batch, drawn on its own (the sorted translucent pass).
            Cell([i32; 3], usize),
            /// A draw group's clusters in the main view's lists.
            Group(arena::GroupKey),
            /// a sky pass (the physical sky, the sun disk).
            Sky(sky::SkyPass),
        }
        // The volumetric clouds this frame (the image settings' and the quality's, a server sky).
        let cloud_quality = if plain && self.features.clouds && scene.clouds.is_some() {
            image.clouds(quality, self.features.opt_in_clouds).map(|q| {
                torque_render_scene::hd::clouds::CloudQuality {
                    layers: q.layers & self.features.cloud_layers,
                    ..q
                }
            })
        } else {
            None
        };
        // The main view's lists: the clusters in sight (their cells' boxes, then their own), the near clusters only in the cells
        // within the near detail's distance. The sorted translucent pass (no weighted blend) draws the cells' batches back to
        // front.
        let built_far = self.ext.near_detail(scene).map(|l| l[1]);
        // The view the lists are culled for: the jittered camera the frame is drawn with.
        let mut main_view = cull::ViewPlanes::frustum(
            &jcam.view_proj,
            camera.eye,
            built_far,
            cull::VIEW_OPAQUE | cull::VIEW_TRANSLUCENT | cull::VIEW_NEAR,
        );
        if self.lod {
            let [d1, d2] = E::level_distances(camera, quality);
            self.update_lod_stay(camera.eye, d1, d2);
            main_view = main_view.with_lod(d1, d2);
        }
        let mut gpu_lists =
            self.gpu_cull && self.cull.is_some() && self.draw_way == arena::DrawWay::Count;
        let mut main_slot = None;
        let mut main_second = None;
        let mut occluding = false;
        if gpu_lists && let Some(c) = &mut self.cull {
            c.begin_frame(&self.device, &self.queue, &self.table, &self.cell_table);
            if self.occlusion {
                // The main view in two phases, the second's test against the frame's own depth.
                main_slot = c.view(&self.queue, &main_view.phased(1, &jcam.view_proj, [iw, ih]));
                main_second = c.view(&self.queue, &main_view.phased(2, &jcam.view_proj, [iw, ih]));
                occluding = main_second.is_some();
            } else {
                main_slot = c.view(&self.queue, &main_view);
            }
            gpu_lists = main_slot.is_some();
        }
        self.lists_on_gpu = gpu_lists;
        let mut cells_drawn = 0;
        let mut sorted_cells: Vec<(f32, [i32; 3])> = Vec::new();
        if !wboit {
            for (k, c) in &self.cells {
                if c.total > c.opaque && main_view.seen(c.bounds[0], c.bounds[1]) {
                    let centre = [0, 1, 2].map(|i| 0.5 * (c.bounds[0][i] + c.bounds[1][i]));
                    sorted_cells.push((dist(centre), *k));
                }
            }
            sorted_cells.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        }
        if gpu_lists {
            self.main_lists.for_gpu(&self.device, self.table.capacity());
            self.scratch_lists
                .for_gpu(&self.device, self.table.capacity());
            let n = shadow_lists_for(self.table.capacity());
            self.shadow_lists.truncate(n);
            while self.shadow_lists.len() < n {
                self.shadow_lists.push(arena::Lists::new(&self.device));
            }
            for l in &mut self.shadow_lists {
                l.for_gpu(&self.device, self.table.capacity());
            }
            self.lists_first
                .for_gpu(&self.device, self.table.capacity());
            self.lists_second
                .for_gpu(&self.device, self.table.capacity());
            if let Some(c) = &self.cull {
                cells_drawn = c.seen.map_or(0, |s| s.0 as usize);
            }
        } else {
            cells_drawn = self
                .cells
                .values()
                .filter(|c| main_view.seen(c.bounds[0], c.bounds[1]))
                .count();
            let cells = &self.cell_table;
            self.main_lists.write_cpu(
                &self.device,
                &self.queue,
                &self.table,
                self.first_instance,
                |r| cull::cpu_pass(&main_view, cells, r),
            );
        }
        cpu_mark!("begin: lists");
        let mut items: Vec<Item> = Vec::new();
        // The sky (the server's sky): its draws in order, the physical sky's atmospheric shading
        // after the box, the sun disk before the clouds and the bans; the volumetric clouds in front of the server's flat
        // cloud layers, behind the bans.
        {
            let order = torque_render_scene::hd::sky::order_labels(&self.loose.sky_labels);
            for n in 0..=self.loose.sky.len() {
                if n == order.shade && image.physical_sky && plain {
                    items.push(Item::Sky(sky::SkyPass::Shade));
                }
                if n == order.disk && plain {
                    items.push(Item::Sky(sky::SkyPass::Disk));
                }
                if n == order.clouds && cloud_quality.is_some() {
                    items.push(Item::Sky(sky::SkyPass::Clouds));
                }
                if let Some(d) = self.loose.sky.get(n) {
                    items.push(Item::Loose(*d));
                }
            }
        }
        // The sky's items end here: the frame is copied after them (the fog's sky behind each pixel).
        let sky_end = items.len();
        // The opaque cells: a group at a time, in the groups' order.
        for (g, _) in self.table.groups() {
            if !g.0 && self.main_lists.group_drawn(&self.table, *g) {
                items.push(Item::Group(*g));
            }
        }
        items.extend(self.loose.planes.iter().map(|d| Item::Loose(*d)));
        for o in &self.loose.objects {
            if !o.translucent && sees_sphere(o.centre, o.radius) {
                items.extend(
                    o.draws
                        .iter()
                        .filter(|d| !d.shadow_only)
                        .map(|d| Item::Loose(*d)),
                );
            }
        }
        // The multiplied decals, over the opaque frame in their own pass.
        let decal_start = items.len();
        items.extend(self.particles.decals.iter().map(|d| Item::Loose(*d)));
        // The translucent pass from here: drawn over a copy of the opaque frame.
        let split = items.len();
        items.extend(self.loose.water.iter().map(|d| Item::Loose(*d)));
        // With the TAA, the late objects and the dying shapes are drawn after its resolve (`over`), back to front too.
        let mut sorted: Vec<(f32, Vec<DrawRef>)> = self
            .loose
            .objects
            .iter()
            .filter(|o| o.translucent && sees_sphere(o.centre, o.radius))
            .filter(|o| !(taa && o.late))
            .map(|o| {
                (
                    dist(o.centre),
                    o.draws.iter().filter(|d| !d.shadow_only).copied().collect(),
                )
            })
            .collect();
        cpu_mark!("begin: items");
        // A corona with an opaque shape (a held item, a Player, a vehicle) on its line of sight from this frame's eye is hidden.
        let mut corona_work = 0u64;
        sorted.extend(
            self.loose
                .coronas
                .iter()
                .filter(|(_, _, at)| {
                    !torque_render_scene::hd::shapes_hide(
                        &scene.objects,
                        icam.eye,
                        *at,
                        &mut corona_work,
                    )
                })
                .map(|(d, c, _)| (*d, vec![*c])),
        );
        cpu_mark!("begin: coronas");
        sorted.extend(
            self.particles
                .draws
                .iter()
                .filter(|(_, c)| !(taa && c.late))
                .map(|(d, c)| (*d, vec![*c])),
        );
        let mut over_sorted: Vec<(f32, Vec<DrawRef>)> = Vec::new();
        if taa {
            over_sorted.extend(
                self.loose
                    .objects
                    .iter()
                    .filter(|o| o.late && sees_sphere(o.centre, o.radius))
                    .map(|o| {
                        (
                            dist(o.centre),
                            o.draws.iter().filter(|d| !d.shadow_only).copied().collect(),
                        )
                    }),
            );
            over_sorted.extend(
                self.particles
                    .draws
                    .iter()
                    .filter(|(_, c)| c.late)
                    .map(|(d, c)| (*d, vec![*c])),
            );
            over_sorted.sort_by(|a, b| b.0.total_cmp(&a.0));
        }
        sorted.sort_by(|a, b| b.0.total_cmp(&a.0));
        for (_, ds) in sorted {
            items.extend(ds.into_iter().map(Item::Loose));
        }
        if wboit {
            for (g, _) in self.table.groups() {
                if g.0 && self.main_lists.group_drawn(&self.table, *g) {
                    items.push(Item::Group(*g));
                }
            }
        } else {
            for (_, k) in sorted_cells.iter().rev() {
                for (i, b) in self.cells[k].batches.iter().enumerate() {
                    if b.translucent {
                        items.push(Item::Cell(*k, i));
                    }
                }
            }
        }
        if let Some(f) = self.loose.flare {
            items.push(Item::Loose(f));
        }
        // The over draws last (outside the translucent pass).
        let over_start = items.len();
        for (_, ds) in over_sorted {
            items.extend(ds.into_iter().map(Item::Loose));
        }
        cpu_mark!("begin: sort");
        // Pipelines and bind groups first (they need the world mutably).
        let mut prepared = Vec::with_capacity(items.len());
        for (n, it) in items.iter().enumerate() {
            let (mut key, tex, s) = match it {
                Item::Loose(d) => (
                    PipelineKey {
                        blend: d.blend,
                        cull: d.cull,
                        depth_test: d.depth_test,
                        depth_write: d.depth_write,
                        clustered,
                        front_layer: false,
                        stage: Stage::Main,
                    },
                    d.tex,
                    d.sampler,
                ),
                Item::Cell(..) | Item::Group(_) => {
                    let (translucent, slot) = match it {
                        Item::Cell(k, i) => {
                            let b = self.cells[k].batches[*i];
                            (b.translucent, b.slot)
                        }
                        Item::Group(g) => (g.0, g.1),
                        _ => (false, 0),
                    };
                    let (tex, s) = self
                        .slots
                        .get(&slot)
                        .copied()
                        .unwrap_or((TexRef::Empty, SamplerKey::PLAIN));
                    (
                        PipelineKey {
                            blend: if translucent {
                                Blend::Alpha
                            } else {
                                Blend::Opaque
                            },
                            cull: Cull::BackCw,
                            depth_test: true,
                            depth_write: true,
                            clustered,
                            front_layer: false,
                            stage: Stage::Main,
                        },
                        tex,
                        s,
                    )
                }
                // Drawn with the sky's own pipelines; a plain sky key keeps the lists aligned.
                Item::Sky(_) => (
                    PipelineKey {
                        blend: Blend::Opaque,
                        cull: Cull::None,
                        depth_test: false,
                        depth_write: false,
                        clustered,
                        front_layer: false,
                        stage: Stage::Main,
                    },
                    TexRef::White,
                    SamplerKey::PLAIN,
                ),
            };
            key.front_layer = n >= split && n < over_start && key.blend == Blend::Alpha;
            if n >= over_start {
                key.stage = Stage::Over;
            }
            self.ensure_pipeline(key);
            // An alpha-blended translucent draw's weighted-blend passes.
            if wboit
                && key.front_layer
                && n < over_start
                && !(split..split + self.loose.water.len()).contains(&n)
            {
                for stage in [Stage::FrontDepth, Stage::Tail, Stage::Front] {
                    self.ensure_pipeline(PipelineKey { stage, ..key });
                }
            }
            // An additive particle's own fragment entry (the late pass).
            if matches!(it, Item::Loose(d) if d.particle) && key.blend != Blend::Alpha && n >= split
            {
                self.ensure_pipeline(PipelineKey {
                    stage: Stage::Particle,
                    ..key
                });
            }
            self.ensure_group(tex, s);
            prepared.push((key, (tex, s)));
        }
        cpu_mark!("begin: pipelines");
        // The depth prepass: the opaque draws that test and write depth (the opaque cells, and the opaque parts of the shapes in
        // either pass: a Player's body is drawn in the translucent pass's order, but its depth and motion belong in the prepass).
        let prepass: Vec<usize> = (0..items.len())
            .filter(|&n| match &items[n] {
                Item::Loose(d) => {
                    d.blend == Blend::Opaque
                        && d.depth_test
                        && d.depth_write
                        && !d.particle
                        && n < over_start
                }
                Item::Cell(..) | Item::Group(_) => n < split,
                Item::Sky(_) => false,
            })
            .collect();
        for &n in &prepass {
            let cull = prepared[n].0.cull;
            self.ensure_prepass(cull);
        }

        let color_size = if taa {
            (camera.width, camera.height)
        } else {
            (iw, ih)
        };
        cpu_mark!("begin: prepass list");
        self.plan_reflections(scene, &icam, &jcam, mode, &surfaces, remade, color_size);
        cpu_mark!("begin: reflections");
        // The extension's extra draws: their pipelines, and the cells near enough to draw them.
        let near_detail = self.ext.near_detail(scene);
        if let Some(reach) = near_detail {
            self.ensure_group(TexRef::White, SamplerKey::PLAIN);
            let near = self.grid.near(camera.eye, reach[1]);
            let cells = &self.cells;
            let bounds = |k: [i32; 3]| cells.get(&k).map(|g| g.bounds);
            self.ext.prepare_extras(
                scene,
                Some(ExtraPrepare {
                    device: &self.device,
                    forward_layout: &self.forward_layout,
                    prepass_layout: &self.prepass_layout,
                    forward: &self.forward[&(clustered, false)],
                    prepass: &self.forward[&(true, false)],
                    clustered,
                    camera,
                    near,
                    cells: &bounds,
                    reach,
                }),
            );
        } else {
            self.ext.prepare_extras(scene, None);
        }
        cpu_mark!("begin: extras");
        let has_extras = self.ext.has_extras();
        let coloured = self.features.coloured_shadows && quality.coloured_shadows();
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("hd frame"),
            });
        // a GPU timestamp after what was encoded so far (`profile_passes`).
        macro_rules! mark {
            ($label:expr) => {
                if self.profile_passes || self.cpu_marks {
                    if self.profile_passes
                        && let Some(m) = &self.marks
                    {
                        m.mark(&mut encoder, $label);
                    }
                    let now = web_time::Instant::now();
                    cpu_marks.push(($label, (now - cpu_at).as_secs_f64() * 1000.0));
                    cpu_at = now;
                }
            };
        }
        mark!("begin");
        let _ = set_pass_kind(PassKind::Cull);
        // The main view's lists, culled on the GPU before anything draws from them.
        if let (Some(slot), Some(c)) = (main_slot, &mut self.cull) {
            if occluding {
                c.encode_first(
                    &self.device,
                    &mut encoder,
                    slot,
                    &self.table,
                    &self.cell_table,
                    &self.lists_first,
                );
            } else {
                c.encode(
                    &self.device,
                    &mut encoder,
                    slot,
                    &self.table,
                    &self.cell_table,
                    &self.main_lists,
                    true,
                );
                c.keep_report(&mut encoder);
            }
        }
        mark!("cull");
        let _ = set_pass_kind(PassKind::Cascade);
        // What is encoded so far goes to the GPU now, so it works on it while the rest is encoded and submitted (each command
        // buffer's finish is most of the frame's CPU cost). The same commands in the same order on one queue: the frame is the same.
        let gap_stamps = self.timer.is_some()
            && self
                .device
                .features()
                .contains(wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS);
        let mut splits = 0u32;
        macro_rules! submit_so_far {
            ($next:expr) => {
                if gap_stamps
                    && splits < MAX_SPLITS
                    && let Some(t) = &self.timer
                {
                    encoder.write_timestamp(&t.queries, Q_GAPS + 2 * splits);
                }
                let done = std::mem::replace(
                    &mut encoder,
                    self.device
                        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                            label: Some($next),
                        }),
                );
                self.queue.submit([done.finish()]);
                if gap_stamps
                    && splits < MAX_SPLITS
                    && let Some(t) = &self.timer
                {
                    encoder.write_timestamp(&t.queries, Q_GAPS + 2 * splits + 1);
                    splits += 1;
                }
            };
        }
        // The ground's boxes for the shadows' range (a terrain is hundreds of thousands of vertices: taken again only when it changes).
        let ground = scene.planes.iter().chain(&scene.water);
        let key: Vec<[usize; 3]> = ground
            .clone()
            .map(|d| {
                [
                    d.vertices.as_ptr() as usize,
                    d.vertices.len(),
                    scene.planes_generation as usize,
                ]
            })
            .collect();
        if key != self.ground_key {
            self.ground_boxes = ground
                .filter_map(|d| {
                    let mut it = d.vertices.iter().map(|v| v.position);
                    let first = it.next()?;
                    Some(it.fold([first, first], |[lo, hi], p| {
                        [
                            [0, 1, 2].map(|k| lo[k].min(p[k])),
                            [0, 1, 2].map(|k| hi[k].max(p[k])),
                        ]
                    }))
                })
                .filter(|b| b.iter().flatten().all(|v| v.is_finite()))
                .collect();
            self.ground_key = key;
        }
        // The shadow maps: every shape's opaque draws cast (a Player is a translucent object for its draw order, its body opaque).
        let objects: Vec<ObjectCaster> = self
            .loose
            .objects
            .iter()
            .map(|o| ObjectCaster {
                ghost: o.ghost,
                own: o.draws.iter().any(|d| d.shadow_only),
                centre: o.centre,
                radius: o.radius,
                draws: o
                    .draws
                    .iter()
                    .filter(|d| d.blend == Blend::Opaque && d.depth_write && !d.view_only)
                    .map(|d| (d.first, d.count, d.base_vertex))
                    .collect(),
            })
            .filter(|o| !o.draws.is_empty())
            .collect();
        let input = ShadowInput {
            camera: &icam,
            sun: &scene.sun,
            // The glowing surfaces' lights at the end cast no shadows.
            lights: &self.frame_lights[..self.shadowed_lights.min(self.frame_lights.len())],
            cones: &self.light_cones,
            hosts: &self.light_hosts,
            cells: &self.cells,
            grid: &self.grid,
            ext: &self.ext_slots.shadow,
            nodes: &self.nodes,
            gpu: shadows::ShadowLists {
                device: &self.device,
                cull: self.cull.as_ref().filter(|_| self.lists_on_gpu),
                table: &self.table,
                cells: &self.cell_table,
                lists: &self.shadow_lists,
                first_instance: self.first_instance,
                way: self.draw_way,
                whole: self.arena.pages.len() <= cull::MAX_PAGES,
            },
            arena: &self.arena,
            loose: self
                .loose
                .vertices
                .as_ref()
                .zip(self.loose.indices.as_ref()),
            objects,
            dirty: &self.shadow_dirty,
            reset: self.shadow_reset,
            switches: ShadowSwitches {
                sun: self.features.sun_shadows,
                points: self.features.point_shadows,
                contact: self.features.contact_shadows,
            },
            debug: self.debug.code(),
            coloured,
            // The extension's casters cast within the near detail's distance, capped by their shadow's reach.
            extras: near_detail
                .zip(self.ext.casters())
                .map(|(l, (casters, texel))| ShadowExtras {
                    casters,
                    reach: l[0],
                    texel,
                    cells: self.grid.near(icam.eye, l[0]),
                }),
            caster_reach: E::caster_reach(quality),
            shadow_distance: scene.shadow_distance,
            ground: &self.ground_boxes,
            ao_sun: scene.ao_sun,
            stamps: self
                .timer
                .as_ref()
                .map(|t| (&t.queries, Q_FRAME_BEGIN, Q_SHADOWS_END)),
        };
        let done = self
            .shadows
            .encode(&self.queue, &mut encoder, &input, scene.sun_radius);
        self.shadow_patched = self.shadow_dirty.len();
        self.shadow_dirty.clear();
        self.shadow_reset = false;
        mark!("shadows");
        let _ = set_pass_kind(PassKind::Post);
        // The sky: its uniform, and the physical sky's tables.
        let vol_quality = quality.volumetrics();
        let vol_on = plain
            && self.features.volumetrics
            && vol_quality.is_some()
            && image.volumetrics_on(scene.fog.start, scene.fog.end);
        // The clouds' uniform and targets (the sky group bound again when their image changed).
        self.clouds.from_eye = self.features.cloud_from_eye;
        let sky_clouds = self.clouds.prepare(
            &self.device,
            &self.queue,
            scene,
            // (A clock that is not a finite number is not sent to the GPU: the effect time instead.)
            scene
                .cloud_time
                .filter(|t| t.is_finite())
                .unwrap_or_else(|| E::time(scene)),
            &icam,
            (iw, ih),
            cloud_quality,
            self.features.cloud_sun,
        );
        if self.clouds.take_rebind() {
            let (image, shadow, sun) = self.clouds.views();
            let (image, shadow, sun) = (image.clone(), shadow.clone(), sun.clone());
            self.sky.bind_clouds(&self.device, &image, &shadow, &sun);
        }
        self.sky.fog_mean = self.features.fog_sky_mean;
        self.sky.write(
            &self.queue,
            &**scene,
            &image,
            quality,
            E::time(scene),
            vol_on,
            sky_clouds,
            &E::fog_ends(scene),
        );
        self.sky.encode_tables(&mut encoder);
        mark!("sky tables");
        // The lights' shadow slots.
        if self.written_slots.as_ref() != Some(&self.shadows.light_slots) {
            let mut lb = self.light_bytes.clone();
            for (i, s) in self.shadows.light_slots.iter().enumerate() {
                let at = i * LIGHT_BYTES + 48;
                if at + 16 <= lb.len() {
                    for (k, v) in s.iter().enumerate() {
                        lb[at + 4 * k..at + 4 * k + 4].copy_from_slice(&v.to_le_bytes());
                    }
                }
            }
            if !lb.is_empty() {
                self.queue.write_buffer(&self.lights, 0, &lb);
            }
            self.written_slots = Some(self.shadows.light_slots.clone());
        }

        let targets = self.targets.as_ref().ok_or("no targets")?;
        // The AO the forward pass reads: the AO pass's filtered copy (each frame's estimate stands alone, smoothed in space).
        let ao_shown = &targets.ao_smooth_view;
        {
            crate::count_pass();
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("hd clusters"),
                timestamp_writes: self.timer.as_ref().filter(|_| !done.timed).map(|t| {
                    wgpu::ComputePassTimestampWrites {
                        query_set: &t.queries,
                        beginning_of_pass_write_index: Some(Q_FRAME_BEGIN),
                        end_of_pass_write_index: None,
                    }
                }),
            });
            pass.set_pipeline(&self.clusters_pipeline);
            pass.set_bind_group(0, &self.clusters_group, &[]);
            pass.dispatch_workgroups(cluster_count(), 1, 1);
        }
        mark!("clusters");
        let _ = set_pass_kind(PassKind::View);
        // The reflections' secondary views (the sky cube, the capture, the planar reflection).
        let views_timed = self.encode_views(
            &mut encoder,
            self.timer
                .as_ref()
                .map(|t| (&t.queries, Q_VIEWS_BEGIN, Q_VIEWS_END)),
        ) && self.timer.is_some();
        mark!("views");
        let _ = set_pass_kind(PassKind::Main);
        let clear_colour = scene.clear.map(|c| f64::from(srgb_to_linear(c)));
        let frame_group = self.frame_group(
            &targets.background_view,
            &targets.contact_view,
            ao_shown,
            Some(&targets.trans_depth_view),
        );
        // The same with the camera without its jitter (the late over draws).
        let frame_group_still = self.frame_group_with(
            &self.frame_still,
            &targets.background_view,
            &targets.contact_view,
            ao_shown,
            &targets.trans_depth_view,
        );
        // The same without the front layer's depth (the passes that draw into it).
        let frame_group_front = self.frame_group(
            &targets.background_view,
            &targets.contact_view,
            ao_shown,
            None,
        );
        self.queue.write_buffer(
            &self.comp_uniform,
            0,
            &[
                if surfaces.gamma_translucency {
                    1.0f32
                } else {
                    0.0
                },
                0.0,
                0.0,
                0.0,
            ]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect::<Vec<u8>>(),
        );
        // The prepass's draws into `pass`: the cell groups from `$lists` (when `$groups`), the other draws and the extra draws (when
        // `$rest`), in the items' order.
        macro_rules! prepass_draws {
            ($pass:ident, $lists:expr, $groups:expr, $rest:expr) => {{
                let pass = &mut $pass;
                let mut bound: Option<(Cull, bool)> = None;
                // Which buffers are bound (Some(true): the static arena's, of page `page`).
                let mut buffers: Option<bool> = None;
                let mut page: Option<u32> = None;
                // The motion slots: all of them once (a draw's slot is its instance), or one slot at its offset (the reference).
                let per_draw = self.motion_per_draw;
                let mut slot: Option<u32> = None;
                for &n in &prepass {
                    let group = matches!(items[n], Item::Group(_));
                    if (group && !$groups) || (!group && !$rest) {
                        continue;
                    }
                    let still = !matches!(items[n], Item::Loose(_));
                    let key = (prepared[n].0.cull, still);
                    if bound != Some(key) {
                        pass.set_pipeline(&self.prepass[&key]);
                        bound = Some(key);
                    }
                    let motion = match &items[n] {
                        Item::Loose(d) => d.motion,
                        Item::Cell(..) | Item::Group(_) | Item::Sky(_) => 0,
                    };
                    let bind = if per_draw { motion } else { 0 };
                    if slot != Some(bind) {
                        let group = if per_draw {
                            &self.post.motion_one
                        } else {
                            &self.post.motion_group
                        };
                        pass.set_bind_group(2, group, &[(u64::from(bind) * MOTION_STRIDE) as u32]);
                        slot = Some(bind);
                    }
                    let instance = if per_draw { 0 } else { motion };
                    match &items[n] {
                        Item::Loose(d) => {
                            let (Some(v), Some(i)) = (&self.loose.vertices, &self.loose.indices) else {
                                continue;
                            };
                            if buffers != Some(false) {
                                pass.set_vertex_buffer(0, v.slice(..));
                                // The shapes' own previous positions only when they moved since the last frame (else each is
                                // where it was).
                                let previous = self.loose.previous.as_ref().filter(|_| moved);
                                pass.set_vertex_buffer(1, previous.unwrap_or(v).slice(..));
                                pass.set_index_buffer(i.slice(..), wgpu::IndexFormat::Uint32);
                                buffers = Some(false);
                            }
                            if d.count > 0 {
                                pass.draw_indexed(
                                    d.first..d.first + d.count,
                                    d.base_vertex,
                                    instance..instance + 1,
                                );
                                crate::count_draws(1);
                            }
                        }
                        Item::Cell(..) | Item::Group(_) => {
                            // The cells' own positions are their previous ones (they do not move).
                            let at = match &items[n] {
                                Item::Cell(k, _) => self.cells[k].page,
                                Item::Group(g) => g.2,
                                _ => 0,
                            };
                            if buffers != Some(true) || page != Some(at) {
                                self.arena.bind(pass, at, &[0, 1]);
                                buffers = Some(true);
                                page = Some(at);
                            }
                            match &items[n] {
                                Item::Cell(k, i) => {
                                    let c = &self.cells[k];
                                    for &(first, count, base) in &c.batch_draws[*i] {
                                        pass.draw_indexed(
                                            c.range(first, first + count),
                                            c.base_vertex + base,
                                            0..1,
                                        );
                                        crate::count_draws(1);
                                    }
                                }
                                Item::Group(g) => {
                                    $lists.draw_group(pass, &self.table, *g, self.draw_way);
                                }
                                _ => {}
                            }
                        }
                        Item::Sky(_) => {}
                    }
                }
                // The extension's extra draws (static: the identity motion).
                if $rest && has_extras {
                    if slot != Some(0) {
                        let group = if per_draw {
                            &self.post.motion_one
                        } else {
                            &self.post.motion_group
                        };
                        pass.set_bind_group(2, group, &[0]);
                    }
                    self.ext.draw_extras(pass, true, clustered);
                }
            }};
        }
        // The prepass: depth, motion and normals.
        {
            crate::count_pass();
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("hd prepass"),
                color_attachments: &[
                    Some(colour(&targets.motion_views[p])),
                    Some(colour(&targets.normal_view)),
                ],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &targets.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(0.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_bind_group(0, &frame_group, &[]);
            // Occluding: phase 1's clusters alone (the rest after the pyramid, the other draws after them, in the order without
            // occlusion: an extra draw ties with the face it stands on, and the one drawn last keeps the normal).
            let lists = if occluding {
                &self.lists_first
            } else {
                &self.main_lists
            };
            prepass_draws!(pass, lists, true, !occluding);
        }
        // The main view's phase 2: the farthest-depth pyramid of the depth so far, the clusters it does not wholly hide and the
        // frame's lists; then phase 2's clusters and the prepass's other draws over phase 1's.
        if let (Some(slot), true) = (main_second, occluding)
            && let Some(c) = &mut self.cull
        {
            c.ensure_hiz(
                &self.device,
                (targets.iw, targets.ih),
                &targets.depth_view,
                self.targets_made,
            );
            c.encode_second(
                &self.device,
                &mut encoder,
                slot,
                &self.table,
                &self.cell_table,
                &self.lists_second,
                &self.main_lists,
            );
            c.keep_report(&mut encoder);
            {
                let load = |view| {
                    Some(wgpu::RenderPassColorAttachment {
                        view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: wgpu::StoreOp::Store,
                        },
                    })
                };
                crate::count_pass();
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("hd prepass second"),
                    color_attachments: &[
                        load(&targets.motion_views[p]),
                        load(&targets.normal_view),
                    ],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &targets.depth_view,
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
                pass.set_bind_group(0, &frame_group, &[]);
                prepass_draws!(pass, &self.lists_second, true, true);
            }
        }
        // The prepass's depth kept for the next frame's TAA (the forward passes go on writing the working depth).
        if taa {
            encoder.copy_texture_to_texture(
                targets.depth.as_image_copy(),
                targets.prepass_depths[p].as_image_copy(),
                wgpu::Extent3d {
                    width: targets.iw,
                    height: targets.ih,
                    depth_or_array_layers: 1,
                },
            );
        }
        mark!("prepass");
        // The shadows, the views and the prepass to the GPU at once (the screen-space passes after them read the prepass).
        submit_so_far!("hd frame (screen space)");
        mark!("prepass submit");
        let _ = set_pass_kind(PassKind::Post);
        // The Hi-Z and the screen-space reflections.
        let ssr_timed = self.encode_ssr(
            &mut encoder,
            p,
            self.timer
                .as_ref()
                .map(|t| (&t.queries, Q_SSR_BEGIN, Q_SSR_END)),
        ) && self.timer.is_some();
        mark!("ssr");
        // The contact shadows.
        let contact_timed = done.contact && self.timer.is_some();
        if done.contact {
            crate::count_pass();
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("hd contact"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &targets.contact_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::WHITE),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: self
                    .timer
                    .as_ref()
                    .map(|t| wgpu::RenderPassTimestampWrites {
                        query_set: &t.queries,
                        beginning_of_pass_write_index: Some(Q_CONTACT_BEGIN),
                        end_of_pass_write_index: Some(Q_CONTACT_END),
                    }),
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.contact_pipeline);
            pass.set_bind_group(0, &targets.contact_group, &[]);
            pass.draw(0..3, 0..1);
            crate::count_draws(1);
        }
        mark!("contact");
        // The ambient occlusion and its spatial filter.
        let ao_timed = ao_on && self.timer.is_some();
        if ao_on {
            crate::count_pass();
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("hd ao"),
                color_attachments: &[Some(colour(&targets.ao_views[p]))],
                depth_stencil_attachment: None,
                timestamp_writes: self
                    .timer
                    .as_ref()
                    .map(|t| wgpu::RenderPassTimestampWrites {
                        query_set: &t.queries,
                        beginning_of_pass_write_index: Some(Q_AO_BEGIN),
                        end_of_pass_write_index: None,
                    }),
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.post.ao[&(pq.ao == AoMethod::Gtao)]);
            pass.set_bind_group(0, &targets.ao_groups[p], &[]);
            pass.draw(0..3, 0..1);
            crate::count_draws(1);
        }
        if ao_on {
            // The other parity's group reads this frame's AO as its history binding.
            crate::count_pass();
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("hd ao filter"),
                color_attachments: &[Some(colour(&targets.ao_smooth_view))],
                depth_stencil_attachment: None,
                timestamp_writes: self
                    .timer
                    .as_ref()
                    .map(|t| wgpu::RenderPassTimestampWrites {
                        query_set: &t.queries,
                        beginning_of_pass_write_index: None,
                        end_of_pass_write_index: Some(Q_AO_END),
                    }),
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.post.ao_filter);
            pass.set_bind_group(0, &targets.ao_groups[1 - p], &[]);
            pass.draw(0..3, 0..1);
            crate::count_draws(1);
        }
        mark!("ao");
        // The volumetric clouds' shadow map and march (before the volumetric light, which reads their shadow).
        let clouds_timed = self.clouds.drawn && self.timer.is_some();
        self.clouds.encode(
            &mut encoder,
            &frame_group,
            &self.sky.group,
            self.timer
                .as_ref()
                .map(|t| (&t.queries, Q_CLOUDS_BEGIN, Q_CLOUDS_END)),
        );
        mark!("clouds");
        self.encode_cloud_face(
            scene,
            &mut encoder,
            icam.eye,
            scene.visible_distance.max(1.0),
        );
        mark!("clouds' cube");
        // The volumetric light's march, from the prepass's depth.
        if let (true, Some(vq)) = (vol_on, vol_quality) {
            self.volumetric.encode_march(
                &self.device,
                &mut encoder,
                &frame_group,
                &self.sky.group,
                &targets.depth_view,
                (targets.iw, targets.ih),
                vq.scale,
            );
        } else {
            self.volumetric.drawn = false;
        }
        mark!("volumetric march");
        // The screen-space passes before the frame's colour to the GPU (with the prepass, about half its GPU time).
        submit_so_far!("hd frame (colour)");
        mark!("early submit");
        let _ = set_pass_kind(PassKind::Main);
        let mut draw_calls = 0;
        let refl_group = self.refl.main_group(p).ok_or("no reflection groups")?;
        let water_items = split..split + self.loose.water.len();
        // Draws a list of items (each with the pipeline it is drawn with) into a pass.
        let draw_list = |pass: &mut wgpu::RenderPass<'_>,
                         list: &[(usize, PipelineKey)],
                         draw_calls: &mut usize| {
            let mut bound: Option<(PipelineKey, (TexRef, SamplerKey))> = None;
            // Which loose buffers are bound (Some(true): the particles').
            let mut loose_bound: Option<bool> = None;
            let mut arena_bound = false;
            let mut arena_page: Option<u32> = None;
            for &(n, key) in list {
                let it = &items[n];
                let group = prepared[n].1;
                // The sky's own passes: the physical sky, the sun disk, the volumetric clouds.
                if let Item::Sky(which) = it {
                    pass.set_pipeline(match which {
                        sky::SkyPass::Shade => &self.sky.shade_pipeline,
                        sky::SkyPass::Disk => &self.sky.disk_pipeline,
                        sky::SkyPass::Clouds => &self.sky.cloud_pipeline,
                    });
                    // The sky passes' layout has the forward pass's group 1 (unread: a sky pass can come first).
                    pass.set_bind_group(1, &self.groups[&(TexRef::White, SamplerKey::PLAIN)], &[]);
                    pass.draw(0..3, 0..1);
                    crate::count_draws(1);
                    *draw_calls += 1;
                    bound = None;
                    continue;
                }
                // The water plane, drawn as water.
                if water_items.contains(&n)
                    && let Item::Loose(d) = it
                    && self.water_drawn(d)
                    && let (Some(v), Some(i), Some(pipe)) = (
                        &self.loose.vertices,
                        &self.loose.indices,
                        self.refl.water_pipeline(d.cull),
                    )
                {
                    pass.set_pipeline(pipe);
                    pass.set_bind_group(1, &self.groups[&group], &[]);
                    if loose_bound != Some(false) {
                        pass.set_vertex_buffer(0, v.slice(..));
                        pass.set_index_buffer(i.slice(..), wgpu::IndexFormat::Uint32);
                        loose_bound = Some(false);
                        arena_bound = false;
                    }
                    if d.count > 0 {
                        pass.draw_indexed(d.first..d.first + d.count, d.base_vertex, 0..1);
                        crate::count_draws(1);
                        *draw_calls += 1;
                    }
                    bound = None;
                    continue;
                }
                if bound.is_none_or(|(k, _)| k != key) {
                    pass.set_pipeline(&self.pipelines[&key]);
                }
                if bound.is_none_or(|(_, g)| g != group) {
                    pass.set_bind_group(1, &self.groups[&group], &[]);
                }
                bound = Some((key, group));
                match it {
                    Item::Loose(d) => {
                        let (v, i) = if d.particle {
                            (&self.particles.vertices, &self.particles.indices)
                        } else {
                            (&self.loose.vertices, &self.loose.indices)
                        };
                        let (Some(v), Some(i)) = (v, i) else {
                            continue;
                        };
                        if loose_bound != Some(d.particle) {
                            pass.set_vertex_buffer(0, v.slice(..));
                            pass.set_index_buffer(i.slice(..), wgpu::IndexFormat::Uint32);
                            loose_bound = Some(d.particle);
                            arena_bound = false;
                        }
                        if d.count > 0 {
                            pass.draw_indexed(d.first..d.first + d.count, d.base_vertex, 0..1);
                            crate::count_draws(1);
                            *draw_calls += 1;
                        }
                    }
                    Item::Cell(..) | Item::Group(_) => {
                        let at = match it {
                            Item::Cell(k, _) => self.cells[k].page,
                            Item::Group(g) => g.2,
                            _ => 0,
                        };
                        if !arena_bound || arena_page != Some(at) {
                            self.arena.bind(pass, at, &[0]);
                            arena_bound = true;
                            arena_page = Some(at);
                            loose_bound = None;
                        }
                        match it {
                            Item::Cell(k, i) => {
                                let c = &self.cells[k];
                                for &(first, count, base) in &c.batch_draws[*i] {
                                    pass.draw_indexed(
                                        c.range(first, first + count),
                                        c.base_vertex + base,
                                        0..1,
                                    );
                                    crate::count_draws(1);
                                    *draw_calls += 1;
                                }
                            }
                            Item::Group(g) => {
                                *draw_calls += self.main_lists.draw_group(
                                    pass,
                                    &self.table,
                                    *g,
                                    self.draw_way,
                                );
                            }
                            _ => {}
                        }
                    }
                    Item::Sky(_) => {}
                }
            }
        };
        let full = wgpu::Extent3d {
            width: targets.iw,
            height: targets.ih,
            depth_or_array_layers: 1,
        };
        let sky_group = &self.sky.group;
        // A pass into the HDR frame (cleared to the sky's clear colour, or loaded) over a depth, with the frame's groups.
        macro_rules! begin {
            ($label:expr, $clear:expr, $depth:expr, $group:expr) => {{
                let mut pass = hdr_pass(
                    &mut encoder,
                    $label,
                    if $clear { Some(clear_colour) } else { None },
                    &targets.hdr_view,
                    $depth,
                );
                set_groups(&mut pass, [$group, sky_group, refl_group]);
                pass
            }};
        }
        let keyed = |r: std::ops::Range<usize>| -> Vec<(usize, PipelineKey)> {
            r.map(|n| (n, prepared[n].0)).collect()
        };
        // The sky, then the opaque pass (the frame copied between them for the fog's sky behind each pixel).
        {
            let mut pass = begin!("hd sky", true, &targets.depth_view, &frame_group);
            draw_list(&mut pass, &keyed(0..sky_end), &mut draw_calls);
        }
        encoder.copy_texture_to_texture(
            targets.hdr.as_image_copy(),
            targets.sky_copy.texture.as_image_copy(),
            full,
        );
        targets.sky_copy.encode(&mut encoder, &self.refl.down);
        mark!("sky");
        {
            let mut pass = begin!("hd opaque", false, &targets.depth_view, &frame_group);
            draw_list(&mut pass, &keyed(sky_end..decal_start), &mut draw_calls);
            if has_extras {
                pass.set_bind_group(1, &self.groups[&(TexRef::White, SamplerKey::PLAIN)], &[]);
                extras = self.ext.draw_extras(&mut pass, false, clustered);
                draw_calls += extras.calls;
            }
        }
        // The multiplied decals over the whole opaque frame (lit, shadowed and fogged), before the volumetric light.
        if decal_start < split {
            let mut pass = begin!("hd decals", false, &targets.depth_view, &frame_group);
            draw_list(&mut pass, &keyed(decal_start..split), &mut draw_calls);
        }
        mark!("opaque");
        // The volumetric light over the opaque frame, the water's depth, the opaque frame's copy.
        if vol_on {
            self.volumetric.encode_composite(
                &self.device,
                &mut encoder,
                &frame_group,
                &self.sky.group,
                &targets.depth_view,
                &targets.hdr_view,
            );
        }
        self.encode_water_depth(&mut encoder, &targets.depth);
        encoder.copy_texture_to_texture(
            targets.hdr.as_image_copy(),
            targets.background.as_image_copy(),
            full,
        );
        mark!("volumetric/water depth/copy");
        let mut late_sprites: Vec<(usize, PipelineKey)> = Vec::new();
        if !wboit {
            // The sorted translucent pass (the weighted blend off: the reference, and the debug views).
            let mut pass = begin!("hd translucent", false, &targets.depth_view, &frame_group);
            draw_list(&mut pass, &keyed(split..over_start), &mut draw_calls);
        } else {
            // The water and the opaque parts of translucent shapes, in order.
            let (mut first, mut oit, mut late) = (Vec::new(), Vec::new(), Vec::new());
            let particle = |n: usize| matches!(items[n], Item::Loose(d) if d.particle);
            for (n, &(key, _)) in prepared.iter().enumerate().take(over_start).skip(split) {
                if water_items.contains(&n) || key.blend == Blend::Opaque {
                    first.push((n, key));
                } else if key.blend == Blend::Alpha {
                    oit.push(n);
                } else if particle(n) {
                    late.push((
                        n,
                        PipelineKey {
                            stage: Stage::Particle,
                            ..key
                        },
                    ));
                } else {
                    late.push((n, key));
                }
            }
            {
                let mut pass = begin!(
                    "hd translucent first",
                    false,
                    &targets.depth_view,
                    &frame_group
                );
                draw_list(&mut pass, &first, &mut draw_calls);
            }
            // The weighted blend's composite lays its tail over the frame's copy: the copy taken again with the opaque parts of
            // translucent shapes in it (taken before them, a Player with a translucent material showed as the bare pre-pass
            // silhouette, pale, wherever a particle cloud was weighted over it). The water drawn just now read the first copy.
            if !first.is_empty() && !oit.is_empty() {
                encoder.copy_texture_to_texture(
                    targets.hdr.as_image_copy(),
                    targets.background.as_image_copy(),
                    full,
                );
            }
            // The front layer's depth starts as the opaque depth (every frame, the particles' fade reads it).
            encoder.copy_texture_to_texture(
                targets.depth.as_image_copy(),
                targets.trans_depth.as_image_copy(),
                full,
            );
            // A particle is never the front layer (its whole quad would be: its clear texels hid what is behind it).
            let front: Vec<usize> = oit.iter().copied().filter(|&n| !particle(n)).collect();
            if !oit.is_empty() {
                let stage = |n: usize, s: Stage| PipelineKey {
                    stage: s,
                    front_layer: true,
                    blend: Blend::Alpha,
                    ..prepared[n].0
                };
                // Then every translucent fragment nearer.
                {
                    crate::count_pass();
                    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("hd translucent front depth"),
                        color_attachments: &[],
                        depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                            view: &targets.trans_depth_view,
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
                    pass.set_bind_group(0, &frame_group_front, &[]);
                    pass.set_bind_group(2, &self.sky.group, &[]);
                    pass.set_bind_group(3, refl_group, &[]);
                    let list: Vec<(usize, PipelineKey)> = front
                        .iter()
                        .map(|&n| (n, stage(n, Stage::FrontDepth)))
                        .collect();
                    draw_list(&mut pass, &list, &mut draw_calls);
                }
                // Everything behind the front layer into the weighted blend.
                {
                    crate::count_pass();
                    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("hd translucent weighted"),
                        color_attachments: &[
                            Some(wgpu::RenderPassColorAttachment {
                                view: &targets.accum_view,
                                depth_slice: None,
                                resolve_target: None,
                                ops: wgpu::Operations {
                                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                                    store: wgpu::StoreOp::Store,
                                },
                            }),
                            Some(wgpu::RenderPassColorAttachment {
                                view: &targets.reveal_view,
                                depth_slice: None,
                                resolve_target: None,
                                ops: wgpu::Operations {
                                    load: wgpu::LoadOp::Clear(wgpu::Color::WHITE),
                                    store: wgpu::StoreOp::Store,
                                },
                            }),
                        ],
                        depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                            view: &targets.depth_view,
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
                    pass.set_bind_group(0, &frame_group, &[]);
                    pass.set_bind_group(2, &self.sky.group, &[]);
                    pass.set_bind_group(3, refl_group, &[]);
                    let list: Vec<(usize, PipelineKey)> =
                        oit.iter().map(|&n| (n, stage(n, Stage::Tail))).collect();
                    draw_list(&mut pass, &list, &mut draw_calls);
                }
                // The composite over the frame's copy, then the frame copied again for the front layer.
                {
                    crate::count_pass();
                    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("hd translucent composite"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &targets.hdr_view,
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
                    pass.set_pipeline(&self.wboit_pipeline);
                    pass.set_bind_group(0, &targets.comp_group, &[]);
                    pass.draw(0..3, 0..1);
                    crate::count_draws(1);
                }
                encoder.copy_texture_to_texture(
                    targets.hdr.as_image_copy(),
                    targets.background.as_image_copy(),
                    full,
                );
                // The front layer, blended exactly.
                {
                    let mut pass = begin!(
                        "hd translucent front",
                        false,
                        &targets.trans_depth_view,
                        &frame_group_front
                    );
                    let list: Vec<(usize, PipelineKey)> =
                        front.iter().map(|&n| (n, stage(n, Stage::Front))).collect();
                    draw_list(&mut pass, &list, &mut draw_calls);
                }
            }
            // The additive draws (coronas, additive shape parts) and the flare, in order. With the TAA, the additive
            // ones into their own frame, added after the TAA (they have no motion: in its history they streaked).
            if taa {
                let (sprites, rest): (Vec<_>, Vec<_>) = late
                    .iter()
                    .partition(|(_, k)| matches!(k.blend, Blend::Additive | Blend::One));
                late = rest;
                late_sprites = sprites;
            }
            if !late.is_empty() {
                let mut pass = begin!(
                    "hd translucent late",
                    false,
                    &targets.depth_view,
                    &frame_group
                );
                draw_list(&mut pass, &late, &mut draw_calls);
            }
        }
        // A TAA frame's late frames (cleared whenever either has draws: the composite reads both): the sprites, and the late
        // objects and dying shapes over (unjittered: they are not in the TAA's history, so its jitter would only shake their edges).
        let over: Vec<(usize, PipelineKey)> = keyed(over_start..items.len());
        if taa && (!late_sprites.is_empty() || !over.is_empty()) {
            {
                let mut pass = hdr_pass(
                    &mut encoder,
                    "hd late sprites",
                    Some([0.0; 3]),
                    &targets.late_view,
                    &targets.depth_view,
                );
                // Unjittered as the over draws: the jitter only shook a spinning sprite's edges from frame to frame.
                set_groups(&mut pass, [&frame_group_still, sky_group, refl_group]);
                draw_list(&mut pass, &late_sprites, &mut draw_calls);
            }
            crate::count_pass();
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("hd late over"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &targets.over_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &targets.depth_view,
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
            set_groups(&mut pass, [&frame_group_still, sky_group, refl_group]);
            draw_list(&mut pass, &over, &mut draw_calls);
        }
        mark!("translucent");
        let _ = set_pass_kind(PassKind::Post);
        // The anti-aliasing: the TAA resolve into this frame's history, or the FXAA-style filter.
        let aa_timed = (taa || fxaa) && self.timer.is_some();
        if taa || fxaa {
            let view = if taa {
                &targets.history_views[p]
            } else {
                &targets.fxaa_view
            };
            crate::count_pass();
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some(if taa { "hd taa" } else { "hd fxaa" }),
                color_attachments: &if taa {
                    vec![Some(colour(view)), Some(colour(&targets.shown_views[p]))]
                } else {
                    vec![Some(colour(view))]
                },
                depth_stencil_attachment: None,
                timestamp_writes: self
                    .timer
                    .as_ref()
                    .map(|t| wgpu::RenderPassTimestampWrites {
                        query_set: &t.queries,
                        beginning_of_pass_write_index: Some(Q_AA_BEGIN),
                        end_of_pass_write_index: Some(Q_AA_END),
                    }),
                occlusion_query_set: None,
                multiview_mask: None,
            });
            if taa {
                pass.set_pipeline(&self.post.taa);
                pass.set_bind_group(0, &targets.taa_groups[p], &[]);
            } else {
                pass.set_pipeline(&self.post.fxaa);
                pass.set_bind_group(0, &targets.fxaa_group, &[]);
            }
            pass.draw(0..3, 0..1);
            crate::count_draws(1);
        }
        // The late sprites added to a copy of the shown colour (the TAA reads the shown colour back next frame).
        let present = taa && (!late_sprites.is_empty() || !over.is_empty());
        if present {
            encoder.copy_texture_to_texture(
                targets.shown[p].as_image_copy(),
                targets.present.as_image_copy(),
                wgpu::Extent3d {
                    width: targets.width,
                    height: targets.height,
                    depth_or_array_layers: 1,
                },
            );
            crate::count_pass();
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("hd late sprites composite"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &targets.present_view,
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
            pass.set_pipeline(&self.late_pipeline);
            pass.set_bind_group(1, &targets.late_group, &[]);
            pass.draw(0..3, 0..1);
            crate::count_draws(1);
        }
        mark!("anti-aliasing");
        // The frame kept for the next frame's reflected rays (the TAA's, anti-aliased, else the frame).
        let refl_kept = self.encode_color_copy(
            &mut encoder,
            if taa { &targets.shown[p] } else { &targets.hdr },
        );
        // The tonemap: from the history (output size), the filtered frame or the frame itself (internal size, scaled up).
        let (source, scaled) = if present {
            (&targets.present_view, false)
        } else if taa {
            (&targets.shown_views[p], false)
        } else if fxaa {
            (
                &targets.fxaa_view,
                (iw, ih) != (camera.width, camera.height),
            )
        } else {
            (&targets.hdr_view, (iw, ih) != (camera.width, camera.height))
        };
        let output = if present {
            Output::Present(p)
        } else if taa {
            Output::Shown(p)
        } else if fxaa {
            Output::Fxaa
        } else {
            Output::Hdr
        };
        let sharpen = if taa && pq.sharpen {
            image.sharpen()
        } else {
            0.0
        };
        // The automatic exposure and the bloom chain, over the tonemap's source.
        let source_size = if taa {
            (camera.width, camera.height)
        } else {
            (iw, ih)
        };
        let mode = if self.auto_exposure {
            ExposureMode::Auto
        } else {
            image.exposure()
        };
        let (fixed_exposure, auto) = match mode {
            ExposureMode::Fixed(e) => (e, false),
            ExposureMode::Auto => (1.0, plain),
        };
        if auto {
            self.exposure.encode(
                &self.device,
                &self.queue,
                &mut encoder,
                source,
                source_size,
                self.frame_seconds,
            );
        }
        mark!("reflection copy/exposure");
        let bloom_settings = image.bloom();
        let bloom_on = plain && self.features.bloom;
        if bloom_on {
            self.bloom.encode(
                &self.device,
                &self.queue,
                &mut encoder,
                source,
                source_size,
                quality.bloom_levels(),
                &bloom_settings,
            );
        } else {
            self.bloom.drawn_levels = 0;
        }
        mark!("bloom");
        let origin = target.map_or([0, 0], |t| t.2);
        let mut post = Vec::new();
        push_f32(
            &mut post,
            &[
                fixed_exposure,
                sharpen,
                if scaled { 1.0 } else { 0.0 },
                if auto { 1.0 } else { 0.0 },
            ],
        );
        push_u32(&mut post, &[origin[0], origin[1], 0, 0]);
        push_f32(&mut post, &[ow, oh, 1.0 / ow, 1.0 / oh]);
        push_f32(
            &mut post,
            &[
                bloom_settings.strength,
                if bloom_settings.mix { 1.0 } else { 0.0 },
                if bloom_on { 1.0 } else { 0.0 },
                0.0,
            ],
        );
        let agx = image.tonemap() == Tonemap::AgxStyle && plain;
        push_f32(&mut post, &[if agx { 1.0 } else { 0.0 }, 0.0, 0.0, 0.0]);
        // The grading table (the debug views keep the analytic curves), the grade's linear part, the grain.
        let lut_on = plain && self.features.grading_lut;
        push_f32(
            &mut post,
            &[
                if lut_on { 1.0 } else { 0.0 },
                image.lut_axis().code(),
                0.0,
                0.0,
            ],
        );
        let grade = self.grade_of(&image);
        push_f32(
            &mut post,
            &[
                grade.white_balance[0],
                grade.white_balance[1],
                grade.white_balance[2],
                grade.saturation,
            ],
        );
        // The surfaces' or the caller's `grain` (0 by default).
        let grain = if plain {
            surfaces.grain.max(self.grain)
        } else {
            0.0
        };
        // The grain's pattern is fixed: it does not move with the effect time (no shimmer frame to frame).
        push_f32(&mut post, &[grain, GRAIN_SEED, 0.0, 0.0]);
        // The tint from under water and the waterline (none in the debug views).
        let (tint, waterline) = if plain {
            water::post_terms(E::water_zone(scene), camera)
        } else {
            ([0.0; 4], [0.0, 0.0, 1.0, 1.0])
        };
        push_f32(&mut post, &tint);
        push_f32(&mut post, &waterline);
        debug_assert_eq!(post.len() as u64, POST_BYTES);
        self.queue.write_buffer(&self.post_uniform, 0, &post);
        let tonemap_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("hd tonemap"),
            layout: &self.tonemap_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(source),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: self.post_uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.post.linear),
                },
                tv(3, self.bloom.result().unwrap_or(&self.white)),
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: self.exposure.state.as_entire_binding(),
                },
                tv(5, &self.lut_view),
            ],
        });
        {
            let view = target.map_or(&targets.ldr_view, |t| t.0);
            crate::count_pass();
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("hd tonemap"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: self
                    .timer
                    .as_ref()
                    .map(|t| wgpu::RenderPassTimestampWrites {
                        query_set: &t.queries,
                        beginning_of_pass_write_index: None,
                        end_of_pass_write_index: Some(Q_FRAME_END),
                    }),
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_viewport(
                origin[0] as f32,
                origin[1] as f32,
                camera.width as f32,
                camera.height as f32,
                0.0,
                1.0,
            );
            pass.set_pipeline(&self.tonemap_pipelines[&format]);
            pass.set_bind_group(0, &tonemap_group, &[]);
            pass.draw(0..3, 0..1);
            crate::count_draws(1);
        }
        mark!("tonemap");
        if self.profile_passes
            && let Some(m) = &mut self.marks
        {
            m.finish(&mut encoder);
        }
        if let Some(t) = &mut self.timer {
            encoder.resolve_query_set(&t.queries, Q_FRAME_BEGIN..Q_FRAME_END + 1, &t.resolve, 0);
            if done.timed {
                encoder.resolve_query_set(
                    &t.queries,
                    Q_SHADOWS_END..Q_SHADOWS_END + 1,
                    &t.resolve,
                    RESOLVE_ALIGN,
                );
            }
            if contact_timed {
                encoder.resolve_query_set(
                    &t.queries,
                    Q_CONTACT_BEGIN..Q_CONTACT_END + 1,
                    &t.resolve,
                    2 * RESOLVE_ALIGN,
                );
            }
            if ao_timed {
                encoder.resolve_query_set(
                    &t.queries,
                    Q_AO_BEGIN..Q_AO_END + 1,
                    &t.resolve,
                    3 * RESOLVE_ALIGN,
                );
            }
            if aa_timed {
                encoder.resolve_query_set(
                    &t.queries,
                    Q_AA_BEGIN..Q_AA_END + 1,
                    &t.resolve,
                    4 * RESOLVE_ALIGN,
                );
            }
            if views_timed {
                encoder.resolve_query_set(
                    &t.queries,
                    Q_VIEWS_BEGIN..Q_VIEWS_END + 1,
                    &t.resolve,
                    5 * RESOLVE_ALIGN,
                );
            }
            if ssr_timed {
                encoder.resolve_query_set(
                    &t.queries,
                    Q_SSR_BEGIN..Q_SSR_END + 1,
                    &t.resolve,
                    6 * RESOLVE_ALIGN,
                );
            }
            if clouds_timed {
                encoder.resolve_query_set(
                    &t.queries,
                    Q_CLOUDS_BEGIN..Q_CLOUDS_END + 1,
                    &t.resolve,
                    7 * RESOLVE_ALIGN,
                );
            }
            if splits > 0 {
                encoder.resolve_query_set(
                    &t.queries,
                    Q_GAPS..Q_GAPS + 2 * splits,
                    &t.resolve,
                    8 * RESOLVE_ALIGN,
                );
            }
            encoder.copy_buffer_to_buffer(
                &t.resolve,
                0,
                &t.readback,
                0,
                RESOLVE_SLOTS * RESOLVE_ALIGN,
            );
            // The same timestamps into a free slot of the ring (none free: this frame is not read without a wait).
            t.ring_copied = None;
            let free = t
                .ring
                .iter()
                .position(|s| s.state.load(std::sync::atomic::Ordering::Acquire) == SLOT_FREE);
            if let Some(i) = free {
                let slot = &mut t.ring[i];
                encoder.copy_buffer_to_buffer(
                    &t.resolve,
                    0,
                    &slot.buffer,
                    0,
                    RESOLVE_SLOTS * RESOLVE_ALIGN,
                );
                slot.state
                    .store(SLOT_COPIED, std::sync::atomic::Ordering::Release);
                slot.frame = t.ring_frames;
                slot.splits = splits;
                t.ring_frames += 1;
                t.ring_copied = Some(i);
            }
            t.pending = true;
            t.splits = splits;
            t.shadows = done.timed;
            t.contact = contact_timed;
            t.ao = ao_timed;
            t.aa = aa_timed;
            t.views = views_timed;
            t.ssr = ssr_timed;
            t.clouds = clouds_timed;
        }
        let commands = encoder.finish();
        if self.profile_passes || self.cpu_marks {
            let now = web_time::Instant::now();
            cpu_marks.push(("finish", (now - cpu_at).as_secs_f64() * 1000.0));
            cpu_at = now;
        }
        self.queue.submit([commands]);
        if let Some(c) = &mut self.cull {
            c.map_report();
        }
        // The ring slot this frame copied into is mapped once its copy is submitted (read by take_finished_gpu_ms).
        if let Some(t) = &mut self.timer
            && let Some(i) = t.ring_copied.take()
        {
            let slot = &t.ring[i];
            slot.state
                .store(SLOT_MAPPING, std::sync::atomic::Ordering::Release);
            let state = slot.state.clone();
            slot.buffer
                .slice(..)
                .map_async(wgpu::MapMode::Read, move |r| {
                    state.store(
                        if r.is_ok() { SLOT_READY } else { SLOT_FAILED },
                        std::sync::atomic::Ordering::Release,
                    );
                });
        }
        if self.profile_passes || self.cpu_marks {
            let now = web_time::Instant::now();
            cpu_marks.push(("submit", (now - cpu_at).as_secs_f64() * 1000.0));
            cpu_marks.push((
                "passes",
                (PASSES.load(std::sync::atomic::Ordering::Relaxed) - passes_before) as f64,
            ));
            let kinds = passes_by_kind();
            for k in 0..PASS_KIND_NAMES.len() {
                cpu_marks.push((
                    PASS_KIND_MARKS[k],
                    kinds.0[k].saturating_sub(kinds_before.0[k]) as f64,
                ));
                cpu_marks.push((
                    DRAW_KIND_MARKS[k],
                    kinds.1[k].saturating_sub(kinds_before.1[k]) as f64,
                ));
            }
            for (name, ms) in cpu_marks {
                match self.cpu_passes.iter_mut().find(|(n, _)| *n == name) {
                    Some((_, sum)) => *sum += ms,
                    None => self.cpu_passes.push((name, ms)),
                }
            }
            self.cpu_frames += 1;
        }
        let _ = cpu_start;
        // The arena's next buffers made while the GPU draws this frame and the window steps the next.
        self.arena.make_ahead(&self.device);
        self.refl.history_valid = refl_kept;
        self.output = output;
        self.clouds.advance();
        self.temporal.advance(
            icam.view_proj,
            camera.eye,
            taa,
            ao_on,
            self.loose.generation,
            E::mode(scene),
            E::time(scene),
        );
        self.stats = DrawStats {
            draw_calls,
            cells_drawn,
            cells: self.cells.len(),
            lights: self.light_count,
            extras_drawn: extras.drawn,
            ground_extras_drawn: extras.ground,
            extras: self.ext.extras_held(),
            clusters_drawn: if self.main_lists.gpu {
                self.cull
                    .as_ref()
                    .and_then(|c| c.seen)
                    .map_or(0, |s| s.1 as usize)
            } else {
                self.main_lists.cpu.iter().map(Vec::len).sum()
            },
            clusters: self.table.live() as usize,
        };
        Ok(())
    }

    /// Every upload's steps since the last call, summed by step (ms).
    pub fn take_upload_steps(&mut self) -> Vec<(&'static str, f64)> {
        std::mem::take(&mut self.upload_steps)
    }

    /// Whether the last upload left changed cells for the next one ([`HdWorld::upload_budget`]).
    pub fn uploads_pending(&self) -> bool {
        self.uploads_pending
    }

    /// Whether the GPU holds the cell `key` as built at `generation` (its upload put it in place): only then may its CPU mesh be
    /// dropped ([`torque_render_scene::hd::cells::Cell::resident`]); a cell that did not fit (a full arena) is not held and is sent again.
    pub fn holds(&self, key: [i32; 3], generation: u64) -> bool {
        self.cells
            .get(&key)
            .is_some_and(|g| g.generation == generation)
    }

    /// The most vertices and indices one of the static arena's pages holds ([`arena::PAGE_VERTICES`], [`arena::PAGE_INDICES`] by default;
    /// the tests set fewer), and the pages made so far.
    pub fn set_arena_pages(&mut self, vertices: u64, indices: u64) {
        self.arena.page_vertices = vertices;
        self.arena.page_indices = indices;
    }

    pub fn arena_pages(&self) -> usize {
        self.arena.pages.len()
    }

    /// The most bytes the static arena's vertex and index buffers may take ([`ARENA_BUDGET`] by default): an upload past it
    /// leaves the cells that do not fit for later.
    pub fn set_static_budget(&mut self, bytes: u64) {
        self.arena.budget = bytes;
    }

    /// The renderer's CPU-side copies by part, roughly (each vector's capacity times its element's size, maps by their entries):
    /// the cluster table's mirror, the CPU's draw lists, the cells' table and grid, the cells' and levels' records, the
    /// extension's parts and the lights.
    pub fn cpu_memory(&self) -> Vec<(&'static str, u64)> {
        use std::mem::size_of;
        let cell = |g: &GpuCell| {
            (size_of::<GpuCell>()
                + g.records.capacity() * 4
                + g.batches.capacity() * size_of::<torque_render_scene::hd::cells::Batch>()
                + 48) as u64
        };
        let mut parts = vec![
            ("cluster table", self.table.cpu_bytes()),
            (
                "draw lists",
                self.main_lists.cpu_bytes()
                    + self.lists_first.cpu_bytes()
                    + self.lists_second.cpu_bytes()
                    + self.scratch_lists.cpu_bytes(),
            ),
            (
                "cell table and grid",
                self.cell_table.cpu_bytes() + self.grid.cpu_bytes(),
            ),
            ("cells", self.cells.values().map(cell).sum()),
            ("levels", self.lod_units.values().map(cell).sum()),
        ];
        parts.extend(self.ext.cpu_memory());
        parts.push((
            "lights",
            (self.light_bytes.capacity()
                + self.light_hosts.capacity() * 16
                + self.frame_lights.capacity() * std::mem::size_of::<HdLight>()
                + self.light_cones.capacity() * 16) as u64,
        ));
        parts
    }

    /// The static geometry's GPU memory and table now.
    pub fn static_memory(&self) -> StaticMemory {
        StaticMemory {
            arena_bytes: self.arena.bytes(),
            slab_bytes: self.arena.used_bytes(),
            repacks: self.arena.repacks,
            table_bytes: self.table.buffer.size()
                + self.cell_table.bytes()
                + self.cull.as_ref().map_or(0, cull::Cull::bytes),
            list_bytes: self.main_lists.bytes()
                + self.lists_first.bytes()
                + self.lists_second.bytes()
                + self.scratch_lists.bytes()
                + self
                    .shadow_lists
                    .iter()
                    .map(arena::Lists::bytes)
                    .sum::<u64>(),
            clusters: self.table.live(),
            refused: self.table.refused,
        }
    }

    /// Waits for the GPU and returns the last frame's GPU time in ms (timestamps; `None` without them).
    pub fn gpu_ms(&mut self) -> Option<f64> {
        let t = self.timer.as_mut()?;
        if !t.pending {
            return None;
        }
        t.pending = false;
        let slice = t.readback.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        self.device.poll(wgpu::PollType::wait_indefinitely()).ok()?;
        rx.recv().ok()?.ok()?;
        let times = {
            let data = slice.get_mapped_range().ok()?;
            let at = |o: usize| -> Option<u64> {
                Some(u64::from_le_bytes(data[o..o + 8].try_into().ok()?))
            };
            let ms = |a: u64, b: u64| b.saturating_sub(a) as f64 * t.period_ns / 1e6;
            let begin = at(0)?;
            let r = RESOLVE_ALIGN as usize;
            // Less the GPU's waits for the CPU between the frame's submits.
            let mut gaps = 0.0;
            for k in 0..t.splits as usize {
                gaps += ms(at(8 * r + 16 * k)?, at(8 * r + 16 * k + 8)?);
            }
            GpuTimes {
                frame: (ms(begin, at(8)?) - gaps).max(0.0),
                shadow_maps: if t.shadows { ms(begin, at(r)?) } else { 0.0 },
                contact: if t.contact {
                    ms(at(2 * r)?, at(2 * r + 8)?)
                } else {
                    0.0
                },
                ao: if t.ao {
                    ms(at(3 * r)?, at(3 * r + 8)?)
                } else {
                    0.0
                },
                aa: if t.aa {
                    ms(at(4 * r)?, at(4 * r + 8)?)
                } else {
                    0.0
                },
                reflections: if t.views {
                    ms(at(5 * r)?, at(5 * r + 8)?)
                } else {
                    0.0
                } + if t.ssr {
                    ms(at(6 * r)?, at(6 * r + 8)?)
                } else {
                    0.0
                },
                clouds: if t.clouds {
                    ms(at(7 * r)?, at(7 * r + 8)?)
                } else {
                    0.0
                },
            }
        };
        t.readback.unmap();
        self.times = Some(times);
        Some(times.frame)
    }

    /// The GPU time (ms, as [`HdWorld::gpu_ms`] measures it) of every frame whose timestamps came back since the last call, in the
    /// order they were drawn; never waits (polls the device once). A frame drawn while every slot of the ring was still on its
    /// way back is not in it. Empty without timestamps.
    pub fn take_finished_gpu_ms(&mut self) -> Vec<f64> {
        let Some(t) = self.timer.as_mut() else {
            return Vec::new();
        };
        let _ = self.device.poll(wgpu::PollType::Poll);
        let mut done: Vec<(u64, f64)> = Vec::new();
        for slot in &t.ring {
            match slot.state.load(std::sync::atomic::Ordering::Acquire) {
                SLOT_READY => {
                    let ms = slot
                        .buffer
                        .slice(..)
                        .get_mapped_range()
                        .ok()
                        .and_then(|data| frame_span_ms(&data, slot.splits, t.period_ns));
                    slot.buffer.unmap();
                    if let Some(ms) = ms {
                        done.push((slot.frame, ms));
                    }
                    slot.state
                        .store(SLOT_FREE, std::sync::atomic::Ordering::Release);
                }
                SLOT_FAILED => slot
                    .state
                    .store(SLOT_FREE, std::sync::atomic::Ordering::Release),
                _ => {}
            }
        }
        done.sort_by_key(|d| d.0);
        done.into_iter().map(|d| d.1).collect()
    }

    /// The last frame's GPU time by pass (waits for the GPU; `None` unless [`HdWorld::profile_passes`] and the device can
    /// write timestamps between passes).
    pub fn gpu_passes(&mut self) -> Option<Vec<(&'static str, f64)>> {
        let device = self.device.clone();
        self.marks.as_mut()?.read(&device)
    }

    /// The frame's CPU encoding time by mark, the mean over the frames since the last call (ms; "begin" is everything before
    /// the first pass: the draw lists, the pipelines, the uploads; "submit" the encoder's finish and the queue's submit).
    pub fn take_cpu_passes(&mut self) -> Vec<(&'static str, f64)> {
        let n = self.cpu_frames.max(1) as f64;
        self.cpu_frames = 0;
        std::mem::take(&mut self.cpu_passes)
            .into_iter()
            .map(|(name, ms)| (name, ms / n))
            .collect()
    }

    /// The last frame's GPU times read by [`HdWorld::gpu_ms`] (`None` before it or without timestamps).
    pub fn gpu_times(&self) -> Option<GpuTimes> {
        self.times
    }

    /// Waits until the GPU has done everything submitted.
    /// The GPU memory the device's allocator holds now (bytes; None where the backend gives no report).
    pub fn allocated_bytes(&self) -> Option<u64> {
        self.device
            .generate_allocator_report()
            .map(|r| r.total_allocated_bytes)
    }

    /// The main view's last cull report that came back (the cells seen and the clusters drawn), read now: after
    /// [`HdWorld::finish`], the frame just drawn.
    pub fn cull_report(&mut self) -> Option<(u32, u32)> {
        let c = self.cull.as_mut()?;
        c.read_report(&self.device);
        c.seen
    }

    pub fn finish(&self) -> Result<(), HdError> {
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    pub(crate) fn read_texture(
        &self,
        texture: &wgpu::Texture,
        bpp: u32,
    ) -> Result<(u32, u32, Vec<u8>), HdError> {
        let (w, h) = (texture.width(), texture.height());
        let row = bpp * w;
        let padded =
            row.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("hd readback"),
            size: u64::from(padded) * u64::from(h),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("hd readback"),
            });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded),
                    rows_per_image: Some(h),
                },
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit([encoder.finish()]);
        let slice = buffer.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|e| e.to_string())?;
        rx.recv()
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?;
        let data = slice.get_mapped_range().map_err(|e| e.to_string())?;
        let mut out = Vec::with_capacity((row * h) as usize);
        for y in 0..h as usize {
            let start = y * padded as usize;
            out.extend_from_slice(&data[start..start + row as usize]);
        }
        drop(data);
        buffer.unmap();
        Ok((w, h, out))
    }

    /// The world's own tonemapped image (the last [`HdWorld::draw`] with no target).
    pub fn read_ldr(&self) -> Result<torque_render_core::Frame, HdError> {
        let t = self.targets.as_ref().ok_or("nothing drawn yet")?;
        let (width, height, rgba) = self.read_texture(&t.ldr, 4)?;
        Ok(torque_render_core::Frame {
            width,
            height,
            rgba,
        })
    }

    /// What the last frame's tonemap read (the anti-aliased frame before its tonemap and 8-bit encoding: the TAA's shown colour
    /// at the output's size, the FXAA-style filter's or the plain frame at the internal size), RGBA as floats.
    pub fn read_output(&self) -> Result<(u32, u32, Vec<[f32; 4]>), HdError> {
        let t = self.targets.as_ref().ok_or("nothing drawn yet")?;
        let texture = match self.output {
            Output::Shown(p) => &t.shown[p],
            Output::Present(_) => &t.present,
            Output::Fxaa => &t.fxaa,
            Output::Hdr => &t.hdr,
        };
        let (w, h, bytes) = self.read_texture(texture, 8)?;
        Ok((w, h, halves(&bytes)))
    }

    /// The TAA's history the last frame wrote (the output's size): RGB and, in alpha, the frames it holds.
    pub fn read_history(&self) -> Result<(u32, u32, Vec<[f32; 4]>), HdError> {
        let t = self.targets.as_ref().ok_or("nothing drawn yet")?;
        let p = match self.output {
            Output::Shown(p) | Output::Present(p) => p,
            _ => return Err("the last frame had no TAA".to_string()),
        };
        let (w, h, bytes) = self.read_texture(&t.history[p], 16)?;
        let px = bytes
            .as_chunks::<16>()
            .0
            .iter()
            .map(|c| {
                [0, 1, 2, 3].map(|k| {
                    f32::from_le_bytes([c[4 * k], c[4 * k + 1], c[4 * k + 2], c[4 * k + 3]])
                })
            })
            .collect();
        Ok((w, h, px))
    }

    /// The last frame's prepass motion (the internal frame's size): xy the motion in uv (now less before), z the previous eye
    /// distance, w 1 plus the object's id (0 where nothing was drawn).
    pub fn read_motion(&self) -> Result<(u32, u32, Vec<[f32; 4]>), HdError> {
        let t = self.targets.as_ref().ok_or("nothing drawn yet")?;
        let p = self.temporal.parity ^ 1;
        let (w, h, bytes) = self.read_texture(&t.motions[p], 8)?;
        Ok((w, h, halves(&bytes)))
    }

    /// The face of the clouds' cube the last frame drew, if it drew one (+x, -x, +y, -y, +z as 0, 1, 2, 3, 4), and whether the
    /// cube holds all its faces.
    pub fn cloud_cube_state(&self) -> (Option<usize>, bool) {
        (
            self.clouds.cube_face().map(|(f, _)| f),
            self.clouds.cube_ready(),
        )
    }

    /// The last frame's HDR target (the internal frame's size, before the anti-aliasing), RGBA as floats.
    pub fn read_hdr(&self) -> Result<(u32, u32, Vec<[f32; 4]>), HdError> {
        let t = self.targets.as_ref().ok_or("nothing drawn yet")?;
        let (w, h, bytes) = self.read_texture(&t.hdr, 8)?;
        Ok((w, h, halves(&bytes)))
    }

    /// Draws `scene` (uploaded) from `camera` with the tonemap into a float target and reads back its display values (the
    /// grading table's identity check compares them unrounded).
    pub fn draw_display(
        &mut self,
        scene: &E::Scene,
        camera: &HdCamera,
    ) -> Result<(u32, u32, Vec<[f32; 4]>), HdError> {
        let format = wgpu::TextureFormat::Rgba32Float;
        let t = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("hd display"),
            size: wgpu::Extent3d {
                width: camera.width,
                height: camera.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = t.create_view(&wgpu::TextureViewDescriptor::default());
        self.draw(scene, camera, Some((&view, format, [0, 0])))?;
        let (w, h, bytes) = self.read_texture(&t, 16)?;
        let px = bytes
            .as_chunks::<16>()
            .0
            .iter()
            .map(|c| {
                let f = |i: usize| f32::from_le_bytes([c[i], c[i + 1], c[i + 2], c[i + 3]]);
                [f(0), f(4), f(8), f(12)]
            })
            .collect();
        Ok((w, h, px))
    }

    /// Uploads, draws into the world's own image a frame that starts a history ([`HdWorld::reset_history`]: no earlier frame in
    /// it, the jitter's first phase) and reads it back.
    pub fn render(
        &mut self,
        scene: &E::Scene,
        camera: &HdCamera,
    ) -> Result<torque_render_core::Frame, HdError> {
        self.reset_history();
        self.upload(scene);
        self.draw(scene, camera, None)?;
        self.read_ldr()
    }

    /// The physical sky's sky view table (192 x 108, for the scene's sun of the last frame that drew it) as floats.
    pub fn read_sky_view(&self) -> Result<(u32, u32, Vec<[f32; 4]>), HdError> {
        let (w, h, bytes) = self.read_texture(&self.sky.sky_view, 8)?;
        Ok((w, h, halves(&bytes)))
    }

    /// The transmittance (`multiscatter` false) or the multiple scattering table as floats.
    pub fn read_sky_table(&self, multiscatter: bool) -> Result<(u32, u32, Vec<[f32; 4]>), HdError> {
        let t = if multiscatter {
            &self.sky.multiscatter
        } else {
            &self.sky.transmittance
        };
        let (w, h, bytes) = self.read_texture(t, 8)?;
        Ok((w, h, halves(&bytes)))
    }

    /// a bloom level of the last frame (the down chain's or the up chain's level `k`; level 0 is half the source's size) as
    /// linear RGB floats.
    pub fn read_bloom(&self, k: usize, up: bool) -> Result<(u32, u32, Vec<[f32; 3]>), HdError> {
        let t = self.bloom.level(k, up).ok_or("no such bloom level")?;
        let (w, h, bytes) = self.read_texture(t, 8)?;
        Ok((
            w,
            h,
            halves(&bytes)
                .into_iter()
                .map(|p| [p[0], p[1], p[2]])
                .collect(),
        ))
    }

    /// The bloom chain's levels the last frame drew (0: none).
    pub fn bloom_levels(&self) -> u32 {
        self.bloom.drawn_levels
    }

    /// The volumetric march's texture of the last frame (rgb: the sun's scattered light; a: the eye distance), if it marched.
    pub fn read_volumetric(&self) -> Result<Option<FloatImage>, HdError> {
        let Some(t) = self.volumetric.texture() else {
            return Ok(None);
        };
        let (w, h, bytes) = self.read_texture(t, 8)?;
        Ok(Some((w, h, halves(&bytes))))
    }

    /// The volumetric clouds' image after the last frame (rgb: their light, a: their transmittance; the internal size over the
    /// quality's scale), if they were drawn.
    pub fn read_clouds(&self) -> Result<Option<FloatImage>, HdError> {
        let Some(t) = self.clouds.image() else {
            return Ok(None);
        };
        let (w, h, bytes) = self.read_texture(t, 8)?;
        Ok(Some((w, h, halves(&bytes))))
    }

    /// The sun's transmittance through the volumetric clouds as the eye saw it last frame (`torque_render_scene::hd::clouds::CLOUD_SUN`),
    /// None without clouds.
    pub fn read_cloud_sun(&self) -> Result<Option<f32>, HdError> {
        let Some(t) = self.clouds.sun_image() else {
            return Ok(None);
        };
        let (_, _, bytes) = self.read_texture(t, 8)?;
        Ok(halves(&bytes).first().map(|p| p[0]))
    }

    /// The automatic exposure's state after the last frame: (EV, target EV, set, the mean log2 luminance).
    pub fn exposure_state(&self) -> Result<[f32; 4], HdError> {
        let bytes = self.read_buffer(&self.exposure.state, 16)?;
        Ok([0, 1, 2, 3]
            .map(|k| f32::from_le_bytes(bytes[4 * k..4 * k + 4].try_into().unwrap_or([0; 4]))))
    }

    /// The automatic exposure's histogram of the last frame.
    pub fn exposure_histogram(&self) -> Result<Vec<u32>, HdError> {
        let n = torque_render_scene::hd::grade::HISTOGRAM_BINS;
        let bytes = self.read_buffer(&self.exposure.bins, (n * 4) as u64)?;
        Ok(bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .collect())
    }

    /// Reads the loose vertices back and checks every draw of `scene` (as last uploaded) against them: its vertices, and with the
    /// previous positions' buffer its vertices there with each shape's own previous positions. The vertices checked.
    pub fn check_loose(&self, scene: &E::Scene) -> Result<usize, HdError> {
        let Some(vb) = &self.loose.vertices else {
            return Ok(0);
        };
        let vertices = self.read_buffer(vb, vb.size())?;
        let previous = match &self.loose.previous {
            Some(b) => Some(self.read_buffer(b, b.size())?),
            None => None,
        };
        let mut checked = 0;
        let mut check = |what: &str,
                         d: &HdDraw,
                         r: &DrawRef,
                         own: Option<&[[f32; 3]]>,
                         base: u32|
         -> Result<(), HdError> {
            let mut want = Vec::new();
            HdVertex::extend_bytes_based(&d.vertices, base, &mut want);
            let at = r.base_vertex as usize * HD_VERTEX_BYTES;
            let got = vertices
                .get(at..at + want.len())
                .ok_or_else(|| format!("{what}: past the vertices"))?;
            if got != want.as_slice() {
                return Err(format!("{what}: its vertices differ"));
            }
            if let Some(pb) = &previous {
                for (k, v) in d.vertices.iter().enumerate() {
                    let p = own.map_or(v.position, |o| o[k]);
                    let mut b = v.bytes();
                    b[32..36]
                        .copy_from_slice(&want[k * HD_VERTEX_BYTES + 32..k * HD_VERTEX_BYTES + 36]);
                    for (c, x) in p.iter().enumerate() {
                        b[4 * c..4 * c + 4].copy_from_slice(&x.to_le_bytes());
                    }
                    let at = at + k * HD_VERTEX_BYTES;
                    if pb.get(at..at + HD_VERTEX_BYTES) != Some(&b[..]) {
                        return Err(format!("{what}: its previous vertex {k} differs"));
                    }
                }
            }
            checked += d.vertices.len();
            Ok(())
        };
        let lists = [
            ("sky", &scene.sky, &self.loose.sky),
            ("plane", &scene.planes, &self.loose.planes),
            ("water", &scene.water, &self.loose.water),
        ];
        for (what, draws, refs) in lists {
            for (d, r) in draws.iter().zip(refs.iter()) {
                check(what, d, r, None, 0)?;
            }
        }
        for (o, g) in scene.objects.iter().zip(&self.loose.objects) {
            let n: usize = o.draws.iter().map(|d| d.vertices.len()).sum();
            let own = o.previous.as_deref().filter(|p| p.len() == n);
            let mut from = 0;
            for (d, r) in o.draws.iter().zip(&g.draws) {
                let part = own.map(|p| &p[from..from + d.vertices.len()]);
                check(
                    &format!("object {}", o.ghost),
                    d,
                    r,
                    part,
                    g.node_base.map_or(0, |b| b as u32),
                )?;
                from += d.vertices.len();
            }
        }
        for ((_, d), (_, r, _)) in scene.coronas.iter().zip(&self.loose.coronas) {
            check("corona", d, r, None, 0)?;
        }
        Ok(checked)
    }

    fn read_buffer(&self, buffer: &wgpu::Buffer, size: u64) -> Result<Vec<u8>, HdError> {
        let staging = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("hd buffer readback"),
            size,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("hd buffer readback"),
            });
        encoder.copy_buffer_to_buffer(buffer, 0, &staging, 0, size);
        self.queue.submit([encoder.finish()]);
        let slice = staging.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|e| e.to_string())?;
        rx.recv()
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?;
        let data = slice
            .get_mapped_range()
            .map_err(|e| e.to_string())?
            .to_vec();
        staging.unmap();
        Ok(data)
    }

    /// The vertices and indices on the GPU (cells and the other draws), for the reports.
    pub fn geometry_bytes(&self) -> u64 {
        // The static arena's slabs (their tables and lists: [`HdWorld::static_memory`]).
        let cells: u64 = self.arena.bytes();
        let loose = self.loose.vertices.as_ref().map_or(0, wgpu::Buffer::size)
            + self.loose.indices.as_ref().map_or(0, wgpu::Buffer::size);
        let particles = self
            .particles
            .vertices
            .as_ref()
            .map_or(0, wgpu::Buffer::size)
            + self
                .particles
                .indices
                .as_ref()
                .map_or(0, wgpu::Buffer::size);
        // And the extension's.
        cells + loose + particles + self.ext.gpu_bytes()
    }
}

/// RGBA halves as floats.
pub fn halves(bytes: &[u8]) -> Vec<[f32; 4]> {
    bytes
        .as_chunks::<8>()
        .0
        .iter()
        .map(|p| [0, 1, 2, 3].map(|k| f16_to_f32(u16::from_le_bytes([p[2 * k], p[2 * k + 1]]))))
        .collect()
}

/// An IEEE half as a float.
pub fn f16_to_f32(h: u16) -> f32 {
    let sign = if h & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exp = (h >> 10) & 0x1f;
    let frac = f32::from(h & 0x3ff);
    match exp {
        0 => sign * frac * 2f32.powi(-24),
        0x1f if frac == 0.0 => sign * f32::INFINITY,
        0x1f => f32::NAN,
        e => sign * (1.0 + frac / 1024.0) * 2f32.powi(i32::from(e) - 15),
    }
}

/// The HDR pixels that are not finite (any channel NaN or infinite).
pub fn non_finite(px: &[[f32; 4]]) -> usize {
    px.iter()
        .filter(|p| p.iter().any(|c| !c.is_finite()))
        .count()
}

#[cfg(test)]
#[path = "world_tests.rs"]
mod tests;

/// `surfaces` with the ground's variation asked for (`surf2.y` 2: [`torque_render_scene::hd::HdScene::ground_variation`]) when `on`.
fn varied_ground(mut surfaces: crate::ext::Surfaces, on: bool) -> crate::ext::Surfaces {
    if on {
        surfaces.table[9] = 2.0;
    }
    surfaces
}
