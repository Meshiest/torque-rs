//! The enhanced renderer on the GPU: its WGSL and the shader registry ([`shaders`]), and its passes over a
//! [`torque_render_core::Gpu`]'s device: the bloom chain ([`bloom`]), the automatic exposure ([`exposure`]), the volumetric light
//! ([`volumetric`], and its froxel grid: [`froxels`]), the moving shapes' vertex runs ([`slots`]), the environment's cube maps
//! and the light capture ([`capture`]), the diffuse ambient's irradiance ([`irradiance`]), the screen-space reflections ([`ssr`]),
//! the volumetric clouds ([`clouds`]), the anti-aliasing and AO ([`post`]), the sky ([`sky`]), the water's planar target and its
//! frame terms ([`water`]), and the static geometry's arena ([`arena`]) culled into draw lists ([`cull`]); what a world's static
//! geometry brings beside the engine's frame ([`ext`]); with the helpers they share (bind group entries, compute filter chains,
//! full-frame pipelines, the pass and draw counts by kind).
//!
//! # Maintaining
//!
//! Rules a change must keep, each guarded by a test:
//! - Nothing past the device's largest buffer is asked of it: an upload whose loose draws or particles do not fit leaves them out
//!   for the frame and counts it ([`HdWorld::oversize_uploads`]), the meshes' places too, so no draw reads another frame's places
//!   (`loose_draws_past_the_largest_buffer_are_left_out_and_counted_not_a_panic`,
//!   `a_table_of_mesh_places_past_the_largest_buffer_leaves_the_loose_draws_out`,
//!   `particles_past_the_largest_buffer_are_left_out_and_counted`).
//! - A world view's scene is an extension's `Scene` that is also an [`ext::ViewScene`] (mode, time, the cells static batches
//!   go in); [`ext::PlainScene`]'s is the engine's own. The trait is defined in `torque_render_scene::hd` (free of wgpu, so a
//!   crate built on it can implement it for its scene) and `ext` re-exports it, so a path through either names one trait. The shader modules are composed with one extension's chunks a process
//!   ([`shaders::register`]; [`HdWorld::try_from_device`] panics on a second), so a test comparing two extensions' pictures
//!   draws one of them in another process (`torque-app`'s `tests/render_ext.rs`).
//! - The shading's BRDF is one file, `shaders/brdf.wgsl` (the forward pass, the water and the cubes' prefilter take it): GGX
//!   with the split sum's environment BRDF, the multiple-scattering compensation, the specular occlusion and the horizon fade.
//!   With `linear_light` on (the default) the diffuse is Lambert in linear light and takes what the specular lobe leaves, and a
//!   surface that reflects no environment reflects the ambient as one, so a white surface under a uniform sky gives back the
//!   sky: within 1 % at every pixel for five roughnesses of reflective dielectrics and metals and of plain ones, with all and
//!   with half of the highlights (`tests/white_furnace.rs`). `linear_light` off
//!   keeps the fixed-function gamma-space response (`ImageSettings::linear_light`): its diffuse is not split with the specular
//!   and a metal keeps its key light; only the specular terms are shared. A new lobe term goes in `brdf.wgsl` and must keep the
//!   furnace.
//! - The area lights (`HdScene::area_lights`, rectangles and tubes) share the point lights' buffer (`area_a` and `area_b` of
//!   `common.wgsl`'s `Light`, [`LIGHT_BYTES`] a light), clusters and shadow cubes. They sit at the buffer's end, from
//!   [`FrameLights::area_start`] (the frame uniform's `limits.z`), and the forward pass shades them in a loop of its own after
//!   the point lights' loop, which stops there: a branch inside the point lights' loop changed the compiled shader enough to
//!   move a few pixels of the point and spot lights' pictures by a level, and they must stay byte-identical (`shots-diff`
//!   against main's pictures). Every other reader (the clusters, the froxel fog, the lit particles) sees a point light at its
//!   center, its radius past the shape ([`torque_render_scene::hd::HdAreaLight::gpu_light`]). The shading
//!   is `shaders/area.wgsl`: the diffuse is Lambert's edge sum over the shape's outline clipped to the horizon (a tube's
//!   outline is the strip of its side the point sees with one corner for each end's arc, and an end it sees a square of the
//!   end's area), within 3 % of a 64 x 64 sum over
//!   the shape at 8 points each for rectangles and tubes; the specular is the outline turned by the GGX lobe's fitted inverse
//!   matrix (`torque_render_scene::hd::ltc`'s table, group 0's binding 21, two `Rgba16Float` layers), within 20 % of the
//!   lobe's own sum (`tests/area_lights.rs`). The table is generated, not hand-written: the `ltc-fit` example of
//!   `torque-render-scene` writes `ltc_ggx.bin` again, and `ltc_tests.rs` checks a refit's cells, the moments against a
//!   quadrature and the fit's coverage of each lobe. An area light that casts shadows takes a point light's cube at its center
//!   (a hard-edged shadow from there, not the shape's soft one). Low and Medium upload them without their shapes, as point
//!   lights ([`torque_render_scene::hd::Quality::area_lights`]). Point and spot lights shade as before: main's pictures are
//!   `shots-diff`'s baseline for a change here.
//! - The light capture's and the sky cube's levels are the GGX lobe of each level's roughness over the whole cube
//!   ([`capture::PrefilterPipe`]), read at `sqrt(roughness) x (levels - 1)`; the faces drawn in a frame are all drawn before
//!   their levels are filtered, a level of every face before the next (each reads its neighbors' level above). A face's
//!   orientation against the device's cube sampling and each level's lobe are checked by `capture_tests.rs` (every texel of a
//!   direction-colored cube still points along its direction, and each level's mean length is the product of the documented
//!   schedule's lobes). The clouds' cube and the color pyramid keep the box filter ([`capture::DownPipe`]).
//! - The GPU particle pool (`world.rs`'s `gpu_particles` module, [`HdFeatures::gpu_particles`], on at Medium and up by default) keeps the CPU
//!   path's particles and pictures: the same particles alive, the same picture every run, the CPU path's within a few levels
//!   (`tests/gpu_particles.rs`); its draws are the forward pass's fragment entries over its own vertex entry. The Sky's wind
//!   (`GpuSprites::wind`, a frame's) carries its particles by their `windCoefficient` as the CPU path's does, and a look that
//!   orients its particles turns them along their ejection direction or velocity (`tests/gpu_particles.rs`: the wind's picture
//!   is the CPU path's, an oriented square's box is a turned one's, one at rest draws nothing).
//! - The screen-space distortion ([`distortion`], `shaders/distortion.wgsl`) runs after the TAA resolve into the shown copy and
//!   never into the TAA's history, so a moving bend leaves no trail; a frame with no distorter in the main camera's frustum
//!   makes no target and records no pass (byte for byte the frame without it); FXAA frames (Low, or the TAA off) skip it. At
//!   most `MAX_DISTORTERS` draw, the nearest, within `MAX_VERTICES` and `MAX_INDICES` (`torque_render_scene::hd::distortion`;
//!   `more_distorters_than_the_cap_draw_the_nearest_within_the_caps`), and bytes past the device's largest buffer draw nothing. A
//!   known offset moves the picture behind by the pixels the look gives, what is in front of a distorter is neither bent nor
//!   pulled into the bend, and a distorter out of sight leaves the frame as it was (`tests/distortion.rs`). The look's bytes are
//!   the shader's `Look` (`a_looks_bytes_are_the_shaders_order`); the shader is validated with naga by its own test, outside
//!   the permutation registry. Its GPU time is the `distortion` mark (and `distortion-clip --bench`, under the bench lock).
//!   The GPU particle pool plugs in through [`distortion::Distortion::encode`]'s `extra` callback and
//!   [`distortion::Distortion::look_slot`]: a bending look's bucket draws its quads with `fs_distort` into the same targets.
//! - The AO pass runs at half the internal frame's resolution at every quality (`ImageSettings::ao_full_res` asks for the whole;
//!   `ImageSettings::ao_scale` is the one place that says which): an AO texel stands for the internal pixel at its top left block
//!   (`ao.wgsl`, `ao_filter.wgsl`), and the forward pass upsamples the four texels about a pixel by their bilinear weights times
//!   how near each one's eye distance (the pass's `g`) is the pixel's (`ambient_occlusion` in `forward.wgsl`). A change to the
//!   pass, its noise or its filter keeps the 4 x 4 tile's period in AO texels, and is judged at both scales
//!   (`tests/ao_banding.rs`; `level-profile --scene office --quality ultra` for the `ao` mark, under
//!   the bench lock).
//! - The diffuse ambient (`ImageSettings::sky_irradiance`, on by default; off keeps the fixed-function ambient, the scene's
//!   ambient color times the CPU's sky shape, `torque_render_scene::hd::sh::sky_shape`) is the L2 SH irradiance of the
//!   environment cubes ([`irradiance`], `irradiance.wgsl`): the faces a frame draws are projected (the sky cube's six when the
//!   scene's draws change, the capture's one in turn) at [`irradiance::LEVEL`], their sums kept, then resolved after the
//!   secondary views and before the main view (group 0's binding 20; `frame.limits.y` says the forward pass reads it). The
//!   physical sky drawn in the box's place lights by `sh::sky_light_scale` over a ground of `sh::GROUND_ALBEDO`; the server's
//!   sky by its luminance alone, the horizontal normals taking the scene's ambient on average. At High and up the main view
//!   takes the light capture's bounce (the capture is drawn every frame there, so the ambient does not jump as reflective
//!   surfaces come into sight; the reflections still read it only where they did), the secondary views the sky the capture sees
//!   through the scene's gaps (a closed room bounces no sky light; the capture is not lit by itself, so a light change settles in
//!   six frames; they are drawn before the projection and read the last frame's). The extension's ground bounce shares only a
//!   lower half that is the sky as drawn (the ground model and the capture hold the ground's light already); a texel that is not
//!   finite adds nothing. A uniform sky gives pi L at every normal within 1 % and a two-tone sky each hemisphere's irradiance
//!   within 2 % (`irradiance_tests.rs`, with the ratio's horizontal normals, the ground, the capture's geometry beside its sky
//!   and a closed room); the furnace holds with it on.
//! - A dissolving object ([`HdScene::dissolves`](torque_render_scene::hd::HdScene::dissolves); the world's `dissolve` module)
//!   is clipped by its object-space field in the prepass, the opaque pass and the shadow casters alike, with modules of its own
//!   (`Permutation::Dissolve`, `ShadowDissolve`): every other draw's modules and pipelines are as without it
//!   (`only_the_dissolve_permutations_take_its_chunks_and_their_entries_are_plain_functions_there`), and a frame with no
//!   dissolve or one eating nothing is byte for byte the same (`tests/dissolve.rs`, which also holds the drawn share to the
//!   share left, the prepass's coverage to the colour's and the shadow to the faces left).
//! - A loose shape of radius 4 or more that holds the capture's eye stays in the capture (`reflect::CAPTURE_SHAPE_HELD`): a
//!   room built of shapes would otherwise show the capture the sky through its walls, and the ambient would bounce it. Smaller
//!   ones (the client's own player and held item) are left out
//!   (`a_small_shape_that_holds_the_capture_eye_is_left_out_and_a_room_is_not`).
//! - Physically based materials ([`torque_render_scene::hd::pbr`]) have a path of their own (`pbr.rs`, `shaders/pbr.wgsl`):
//!   [`shaders::Permutation::ForwardPbr`] modules (the forward module of the same constants with `pbr.wgsl` after it), their
//!   pipelines (`vs_pbr` over the vertex buffer and a tangent stream, `fs_pbr`, `fs_pbr_wboit`), a group 1 a material (the base
//!   texture and sampler, then normal, ORM, height, the uniform and emissive) and the loose draws' tangent stream. None of it
//!   is made until a scene holds a material, and no other permutation's source includes `pbr.wgsl`: every existing
//!   permutation's source is pinned by its hash (`the_permutations_without_pbr_materials_are_unchanged`), and a scene's plain
//!   draws draw the same bytes beside a material's (`tests/pbr_materials.rs`). Only the main view's forward pass reads the
//!   maps; the prepass, shadows, captures and mirrors draw a material's draw as any draw of its base texture (the prepass's
//!   normal is the geometric one). `pbr.wgsl` lights through forward.wgsl's `shade_surf` with the maps' inputs and the linear
//!   response, so a roughness by metalness sweep of white spheres under a uniform sky gives back the sky within 2 % at
//!   metalness 0 and 1 (`tests/pbr_materials.rs`); a change to the shading goes in `brdf.wgsl` and keeps both furnaces.
//!   Parallax marches the height map at Ultra only. Any table texture is uploaded within the device's side, its top levels
//!   dropped until it fits and never more levels than its chain has (`texture_fit`;
//!   `textures_past_the_devices_side_or_with_too_many_levels_upload_without_an_error`). Only loose objects' draws (shapes) take materials: planes, cells and the
//!   sky ignore a draw's material. The `pbr-gallery` example draws the look (`--library` a folder of sidecars, `--cost` the
//!   opaque pass with and without the materials).
//!
//! Costs a frame must not grow back (`benches/upload.rs` and the `frame-cpu` example, timed under the bench lock):
//! - No bind group is made a frame while nothing it binds changed: the forward passes' frame groups and the tonemap group are
//!   kept by what they bind (`GroupCache` in `world.rs`), the bloom's prefilter group and the exposure's by the source they
//!   read, the volumetric light's by the depth, and every per-frame value goes through buffers already bound (the bloom's
//!   uniforms written only when they change).
//! - A frame is one command buffer and one submit: a submit costs about a tenth of a millisecond of CPU, and a frame sent in
//!   parts makes the GPU wait for the CPU between them (`level-profile`). Work that can share a pass does: a filter chain's levels and
//!   the sky's tables with the lights' clusters are one compute pass each ([`encode_chain`]), and the decals draw in the
//!   opaque pass unless a pass of the opaque stage ([`passes`]) draws between them.
//! - The particles' vertex and index buffers are kept and written in place (made anew only to grow), their bytes in kept lists;
//!   the shadow casters' bytes go up to the last caster used, the mesh places' table from a kept list (`frame-cpu`).
//! - A stamped object whose stamp its run holds is not encoded or written again (bench `upload_loose 64 figures kept`).
//! - The translucent draws are sorted as one flat list (no list an object); the opaque frame's copy is taken only when a draw
//!   after it or an extension's pass may read it.
//!
//! The froxel volumetrics ([`froxels`], Ultra; `shaders/froxels.wgsl`) put the point lights in the fog beside the march, which
//! keeps the sun's god rays (a froxel's width blurs a shaft's edge a pixel's own ray keeps). They keep these, each held by a test:
//! - A pixel takes only the light of the air in front of it, and a light's shadow cubes are tapped over a cone of directions at
//!   the froxel's own distance, so a closed room's froxels and god rays hold none of the light outside it
//!   (`a_closed_room_holds_none_of_the_light_outside_it`); a point light adds light only within its radius
//!   (`a_point_light_in_fog_adds_light_within_its_radius_only`).
//! - The fog's medium is the distance and height fog's own extinction (`skyfog.wgsl`'s `fog_keep` as a rate along the ray), so
//!   the lights' light it scatters fades as the fog hides what is behind it.
//! - The grid is taken from the scene alone every frame (no noise, no history), so a light switched or moved shows at once (the
//!   `fog-lights` row of the `ghosting` example, gated like the rest). The gather's bindings in the compute stage are counted by
//!   [`limits`] (WebGPU's defaults hold: the grid is `Rgba16Float` write-only storage, which WebGPU allows in 3D). Its GPU time
//!   and the march's beside it (at the quality's `VolumetricQuality::froxels` size: a quarter of the pixels a side, 48 steps,
//!   which keeps the shafts of the half-size march at about half its cost) are the `froxel fog` and `volumetric march` marks of
//!   `track-profile --place crest --quality ultra`. The grid is made when a frame with lights first needs it at Ultra (no
//!   pipelines before), and let go after a few frames that gather none.
//!
//! The volumetric clouds' GPU time (the `clouds` mark of `track-profile --place crest` and `level-profile --view valley-sea`,
//! under the bench lock; the `clouds` row of the `ghosting` example for their history, and the vehicle demo's `--cloud-shots`
//! against main's for their look) rests on `clouds.wgsl`'s refined march looking for cloud in at most `REFINED_STEPS` long steps
//! and finding its edge by halving, fine steps no shorter than a few texels' footprints that lengthen behind the first dense
//! ones and end at `FINE_OPAQUE`, a light march of `LIGHT_SAMPLES` samples whatever its reach, the octaves' phases worked out
//! once a ray, and the volumes over the lowest layer (a layer of elements: always the even march) drawn at a quarter of the
//! march's texels (`UPPER_SCALE`). The shader's
//! constants that the Rust side shares are held to it by `the_sun_rays_constants_are_the_scenes` and
//! `the_cloud_cubes_uniform_places_and_mips_are_the_shaders`.

use glam::Vec3;
use torque_render_core::wgpu;

use crate::shaders::Permutation;

/// Whether the sphere at `c` of radius `r` is not wholly outside any of `planes` (`a x + b y + c z + d >= 0` inside, the normal
/// not unit).
pub(crate) fn sphere_seen(planes: &[[f32; 4]], c: [f32; 3], r: f32) -> bool {
    planes.iter().all(|p| {
        let n = Vec3::new(p[0], p[1], p[2]);
        n.dot(c.into()) + p[3] >= -r * n.length()
    })
}

pub mod arena;
pub mod bloom;
pub mod capture;
pub mod clouds;
pub mod cull;
pub mod distortion;
pub mod exposure;
pub mod ext;
pub mod froxels;
pub mod ghosting;
#[cfg(feature = "gizmos")]
pub mod gizmos;
pub mod irradiance;
pub mod limits;
pub mod passes;
mod pbr;
pub mod post;
pub mod profile;
pub mod shaders;
pub mod shadows;
pub mod sky;
pub mod slots;
pub mod ssr;
pub mod volumetric;
pub mod water;
mod world;

pub use world::*;

/// The HDR and depth formats.
pub const HDR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
/// The fog takes the sky behind a surface without its detail (the clouds' shapes, the box's painted clouds): the sky copy's mip
/// of this level (the mean over 64 x 64 pixels of the internal frame), the clouds' image's level of the same span.
pub const FOG_LOW_LEVEL: u32 = 6;

/// The render and compute passes begun so far (the frame's CPU cost grows with its passes: each is a command list of its own).
pub static PASSES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// What a frame's passes and draw calls are counted as (the breakdown's `passes by kind`): the GPU culls and the Hi-Z, the static
/// sun cascades (and their transmittance), the point lights' static cube faces, the moving shapes' overlays on both, the secondary
/// views (the water's mirror, a light capture's face), the main view's prepass, opaque and translucent passes, and the rest after them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PassKind {
    Cull = 0,
    Cascade = 1,
    Face = 2,
    Overlay = 3,
    View = 4,
    Main = 5,
    Post = 6,
}

/// The kinds' names, in their order.
pub const PASS_KIND_NAMES: [&str; 7] =
    ["cull", "cascade", "face", "overlay", "view", "main", "post"];
/// The kinds' names as the CPU marks carry their counts (the passes' breakdown).
pub const PASS_KIND_MARKS: [&str; 7] = [
    "passes: cull",
    "passes: cascade",
    "passes: face",
    "passes: overlay",
    "passes: view",
    "passes: main",
    "passes: post",
];
pub const DRAW_KIND_MARKS: [&str; 7] = [
    "draws: cull",
    "draws: cascade",
    "draws: face",
    "draws: overlay",
    "draws: view",
    "draws: main",
    "draws: post",
];

/// The kind the passes begun now are counted as ([`set_pass_kind`]), and the passes and draw calls by kind so far.
static PASS_KIND: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(6);
static PASSES_BY_KIND: [std::sync::atomic::AtomicU64; 7] =
    [const { std::sync::atomic::AtomicU64::new(0) }; 7];
static DRAWS_BY_KIND: [std::sync::atomic::AtomicU64; 7] =
    [const { std::sync::atomic::AtomicU64::new(0) }; 7];

/// The passes and draws from now on are `kind`'s (a cull's are always [`PassKind::Cull`]: [`count_pass_as`]).
pub fn set_pass_kind(kind: PassKind) -> usize {
    PASS_KIND.swap(kind as usize, std::sync::atomic::Ordering::Relaxed)
}

/// The kind [`set_pass_kind`] gave back made the passes' kind again.
pub fn restore_pass_kind(kind: usize) {
    PASS_KIND.store(kind.min(6), std::sync::atomic::Ordering::Relaxed);
}

/// The passes and draw calls by kind so far.
pub fn passes_by_kind() -> ([u64; 7], [u64; 7]) {
    let load = |a: &[std::sync::atomic::AtomicU64; 7]| {
        std::array::from_fn(|i| a[i].load(std::sync::atomic::Ordering::Relaxed))
    };
    (load(&PASSES_BY_KIND), load(&DRAWS_BY_KIND))
}

pub fn count_pass() {
    PASSES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let kind = PASS_KIND.load(std::sync::atomic::Ordering::Relaxed);
    PASSES_BY_KIND[kind].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

/// A pass of `kind`, whatever kind the passes around it are.
pub fn count_pass_as(kind: PassKind) {
    PASSES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    PASSES_BY_KIND[kind as usize].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

/// `n` draw calls of the passes' kind now (a multi-draw is one).
pub fn count_draws(n: usize) {
    let kind = PASS_KIND.load(std::sync::atomic::Ordering::Relaxed);
    DRAWS_BY_KIND[kind].fetch_add(n as u64, std::sync::atomic::Ordering::Relaxed);
}

/// A 4 x 4 column-major inverse (f64), the identity when singular.
pub fn invert(m: &[f32; 16]) -> [f32; 16] {
    let a: Vec<f64> = m.iter().map(|v| f64::from(*v)).collect();
    let mut inv = [0.0f64; 16];
    inv[0] = a[5] * a[10] * a[15] - a[5] * a[11] * a[14] - a[9] * a[6] * a[15]
        + a[9] * a[7] * a[14]
        + a[13] * a[6] * a[11]
        - a[13] * a[7] * a[10];
    inv[4] = -a[4] * a[10] * a[15] + a[4] * a[11] * a[14] + a[8] * a[6] * a[15]
        - a[8] * a[7] * a[14]
        - a[12] * a[6] * a[11]
        + a[12] * a[7] * a[10];
    inv[8] = a[4] * a[9] * a[15] - a[4] * a[11] * a[13] - a[8] * a[5] * a[15]
        + a[8] * a[7] * a[13]
        + a[12] * a[5] * a[11]
        - a[12] * a[7] * a[9];
    inv[12] = -a[4] * a[9] * a[14] + a[4] * a[10] * a[13] + a[8] * a[5] * a[14]
        - a[8] * a[6] * a[13]
        - a[12] * a[5] * a[10]
        + a[12] * a[6] * a[9];
    inv[1] = -a[1] * a[10] * a[15] + a[1] * a[11] * a[14] + a[9] * a[2] * a[15]
        - a[9] * a[3] * a[14]
        - a[13] * a[2] * a[11]
        + a[13] * a[3] * a[10];
    inv[5] = a[0] * a[10] * a[15] - a[0] * a[11] * a[14] - a[8] * a[2] * a[15]
        + a[8] * a[3] * a[14]
        + a[12] * a[2] * a[11]
        - a[12] * a[3] * a[10];
    inv[9] = -a[0] * a[9] * a[15] + a[0] * a[11] * a[13] + a[8] * a[1] * a[15]
        - a[8] * a[3] * a[13]
        - a[12] * a[1] * a[11]
        + a[12] * a[3] * a[9];
    inv[13] = a[0] * a[9] * a[14] - a[0] * a[10] * a[13] - a[8] * a[1] * a[14]
        + a[8] * a[2] * a[13]
        + a[12] * a[1] * a[10]
        - a[12] * a[2] * a[9];
    inv[2] = a[1] * a[6] * a[15] - a[1] * a[7] * a[14] - a[5] * a[2] * a[15]
        + a[5] * a[3] * a[14]
        + a[13] * a[2] * a[7]
        - a[13] * a[3] * a[6];
    inv[6] = -a[0] * a[6] * a[15] + a[0] * a[7] * a[14] + a[4] * a[2] * a[15]
        - a[4] * a[3] * a[14]
        - a[12] * a[2] * a[7]
        + a[12] * a[3] * a[6];
    inv[10] = a[0] * a[5] * a[15] - a[0] * a[7] * a[13] - a[4] * a[1] * a[15]
        + a[4] * a[3] * a[13]
        + a[12] * a[1] * a[7]
        - a[12] * a[3] * a[5];
    inv[14] = -a[0] * a[5] * a[14] + a[0] * a[6] * a[13] + a[4] * a[1] * a[14]
        - a[4] * a[2] * a[13]
        - a[12] * a[1] * a[6]
        + a[12] * a[2] * a[5];
    inv[3] = -a[1] * a[6] * a[11] + a[1] * a[7] * a[10] + a[5] * a[2] * a[11]
        - a[5] * a[3] * a[10]
        - a[9] * a[2] * a[7]
        + a[9] * a[3] * a[6];
    inv[7] = a[0] * a[6] * a[11] - a[0] * a[7] * a[10] - a[4] * a[2] * a[11]
        + a[4] * a[3] * a[10]
        + a[8] * a[2] * a[7]
        - a[8] * a[3] * a[6];
    inv[11] = -a[0] * a[5] * a[11] + a[0] * a[7] * a[9] + a[4] * a[1] * a[11]
        - a[4] * a[3] * a[9]
        - a[8] * a[1] * a[7]
        + a[8] * a[3] * a[5];
    inv[15] = a[0] * a[5] * a[10] - a[0] * a[6] * a[9] - a[4] * a[1] * a[10]
        + a[4] * a[2] * a[9]
        + a[8] * a[1] * a[6]
        - a[8] * a[2] * a[5];
    let det = a[0] * inv[0] + a[1] * inv[4] + a[2] * inv[8] + a[3] * inv[12];
    if det == 0.0 || !det.is_finite() {
        let mut id = [0.0f32; 16];
        for i in 0..4 {
            id[i * 5] = 1.0;
        }
        return id;
    }
    inv.map(|v| (v / det) as f32)
}

pub fn push_f32(out: &mut Vec<u8>, v: &[f32]) {
    for x in v {
        out.extend_from_slice(&x.to_le_bytes());
    }
}

pub fn push_u32(out: &mut Vec<u8>, v: &[u32]) {
    for x in v {
        out.extend_from_slice(&x.to_le_bytes());
    }
}

pub fn module(device: &wgpu::Device, p: Permutation) -> wgpu::ShaderModule {
    device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(&p.name()),
        source: wgpu::ShaderSource::Wgsl(p.source().into()),
    })
}

pub fn storage(
    binding: u32,
    read_only: bool,
    visibility: wgpu::ShaderStages,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

/// A write-only storage texture of `format` for a compute filter's output (a mip chain's level).
pub fn storage_target(binding: u32, format: wgpu::TextureFormat) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::StorageTexture {
            access: wgpu::StorageTextureAccess::WriteOnly,
            format,
            view_dimension: wgpu::TextureViewDimension::D2,
        },
        count: None,
    }
}

/// A compute filter's pipeline (`entry` of `module` over the one group `layout`).
pub fn compute_filter(
    device: &wgpu::Device,
    cache: Option<&wgpu::PipelineCache>,
    label: &str,
    layout: &wgpu::BindGroupLayout,
    module: &wgpu::ShaderModule,
    entry: &str,
) -> wgpu::ComputePipeline {
    compute_filter_with(device, cache, label, &[layout], module, entry)
}

/// [`compute_filter`] over several groups (A filter's sampler in a group of its own, shared by its levels).
pub fn compute_filter_with(
    device: &wgpu::Device,
    cache: Option<&wgpu::PipelineCache>,
    label: &str,
    layouts: &[&wgpu::BindGroupLayout],
    module: &wgpu::ShaderModule,
    entry: &str,
) -> wgpu::ComputePipeline {
    let groups: Vec<Option<&wgpu::BindGroupLayout>> = layouts.iter().map(|l| Some(*l)).collect();
    let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(label),
        bind_group_layouts: &groups,
        immediate_size: 0,
    });
    device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some(label),
        layout: Some(&pl),
        module,
        entry_point: Some(entry),
        compilation_options: wgpu::PipelineCompilationOptions::default(),
        cache,
    })
}

/// The side of a compute filter's square workgroup (the shaders' `@workgroup_size(8, 8)`).
pub const FILTER_GROUP: u32 = 8;

/// One level of a filter chain: its pipeline, its group (the level it reads and the one it writes) and the written level's size.
pub struct Dispatch<'a> {
    pub pipeline: &'a wgpu::ComputePipeline,
    pub group: &'a wgpu::BindGroup,
    pub size: (u32, u32),
}

/// A chain of full-frame filters (a mip pyramid, the bloom's levels) as the dispatches of ONE compute pass, each level after the
/// one it reads (the passes' barriers are the pass's own): a render pass a level made a command list a level, most of a big frame's
/// CPU cost. The same arithmetic at the same texel centres as the fragment filters they replace.
pub fn encode_chain(
    encoder: &mut wgpu::CommandEncoder,
    label: &str,
    chain: &[Dispatch<'_>],
    stamps: Option<wgpu::ComputePassTimestampWrites<'_>>,
) {
    encode_chain_with(encoder, label, chain, &[], stamps);
}

/// [`encode_chain`] with groups every level shares (bound once, from group 1).
pub fn encode_chain_with(
    encoder: &mut wgpu::CommandEncoder,
    label: &str,
    chain: &[Dispatch<'_>],
    shared: &[&wgpu::BindGroup],
    stamps: Option<wgpu::ComputePassTimestampWrites<'_>>,
) {
    if chain.is_empty() {
        return;
    }
    count_pass();
    let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
        label: Some(label),
        timestamp_writes: stamps,
    });
    for (k, g) in shared.iter().enumerate() {
        pass.set_bind_group(1 + k as u32, *g, &[]);
    }
    for d in chain {
        pass.set_pipeline(d.pipeline);
        pass.set_bind_group(0, d.group, &[]);
        pass.dispatch_workgroups(
            d.size.0.div_ceil(FILTER_GROUP).max(1),
            d.size.1.div_ceil(FILTER_GROUP).max(1),
            1,
        );
    }
}

pub fn uniform(binding: u32, visibility: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

/// A bind group entry of a texture view.
pub fn tv(binding: u32, v: &wgpu::TextureView) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry {
        binding,
        resource: wgpu::BindingResource::TextureView(v),
    }
}

/// A colour attachment cleared to transparent black.
pub fn colour(view: &wgpu::TextureView) -> wgpu::RenderPassColorAttachment<'_> {
    wgpu::RenderPassColorAttachment {
        view,
        depth_slice: None,
        resolve_target: None,
        ops: wgpu::Operations {
            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
            store: wgpu::StoreOp::Store,
        },
    }
}

/// A full-frame triangle's pipeline of `layout`, `entry` of `module` into `format`.
pub fn full_frame(
    device: &wgpu::Device,
    cache: Option<&wgpu::PipelineCache>,
    label: &str,
    layout: &wgpu::BindGroupLayout,
    module: &wgpu::ShaderModule,
    entry: &str,
    formats: &[wgpu::TextureFormat],
) -> wgpu::RenderPipeline {
    let targets: Vec<Option<wgpu::ColorTargetState>> = formats
        .iter()
        .map(|f| {
            Some(wgpu::ColorTargetState {
                format: *f,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })
        })
        .collect();
    let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(label),
        bind_group_layouts: &[Some(layout)],
        immediate_size: 0,
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(&pl),
        vertex: wgpu::VertexState {
            module,
            entry_point: Some("vs_full"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: &[],
        },
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module,
            entry_point: Some(entry),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &targets,
        }),
        multiview_mask: None,
        cache,
    })
}
