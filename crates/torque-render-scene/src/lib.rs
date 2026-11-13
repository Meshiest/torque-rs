//! A frame as plain data: the camera's matrices, the sun as GL light 0, the textures with their mip chains, and a list of
//! [`Draw`]s, each a triangle list with the fixed-function state it is drawn with (lighting, texture environment, sampler,
//! culling, depth, blending), so a renderer only has to reproduce OpenGL 1.x's per-vertex lighting, texture environments and
//! rasterisation; the enhanced renderer's data types ([`hd`]), the scene's source ([`source`]), what a frame reads of the
//! client's world of ghosts ([`world`]), the environment ([`env`](mod@env)), the sun's flare ([`flare`]), the lights' coronas ([`lights`]), the decals on surfaces ([`decals`]), the ribbons along moving things ([`ribbons`]) and
//! their lines of sight ([`sight`]), the camera ([`camera`]) and TGE's matrix math ([`math`]). No GPU type appears here, and every type is `Send + Sync`.
//!
//! Files come through [`Assets`] (the caller's file reader).
//!
//! # Maintaining
//!
//! Rules a change must keep, each guarded by a test:
//! - A frame's distorters ([`hd::distortion`]) are plain data: world-space triangles with a look held to the wire block's ranges
//!   ([`hd::distortion::DistortionLook::sanitize`], the constants torque-ghosts' `datablocks::distortion` defines), moved with
//!   the render origin (`a_distorter_moves_with_the_render_origin`), their maps' indices remapped into the scene's texture table
//!   ([`hd::HdScene::set_distorters`]). The datablock fields reach them through [`distorters`]: read and held again, none when
//!   they change nothing, a shape drawn as the bend alone taken out of the objects, a decal's fading with it
//!   (`a_datablocks_distortion_is_read_in_range_and_none_when_it_changes_nothing`,
//!   `a_bending_decal_is_a_distorter_that_fades_with_it_and_one_that_only_bends_draws_no_texture`; torque-app's
//!   `a_shape_whose_datablock_bends_is_a_distorter_of_the_frame`). A new look field goes in torque-ghosts' block first, then
//!   [`hd::distortion::DistortionLook`], [`distorters::DistortionData::read`] and the shader's `Look`.
//! - The decals have two paths kept apart by [`decals::DecalManager::set_projected`]: the CPU path's triangles clipped to the
//!   surfaces (Low; TGE's cap and draws, unchanged), and the projected path's boxes ([`hd::decals::HdDecal`], up to
//!   [`decals::MAX_PROJECTED_DECALS`]) laid with no mesh and handed over as the renderer's ([`decals::DecalManager::boxes`],
//!   [`hd::HdScene::set_decals`]), each kept only when [`hd::decals::HdDecal::sanitized`] keeps it
//!   (`the_projected_path_keeps_boxes_to_its_cap_and_the_cpu_path_drops_them`,
//!   `a_decal_projects_as_its_square_with_the_clip_depth_and_the_cpu_paths_alpha`, `only_a_box_of_finite_capped_axes_is_drawn`).
//!   A decal's distorter is copied only in the view, nearest first within the distortion budget
//!   (`the_bending_decals_are_culled_to_the_view_and_capped_nearest_first`).
//! - The engine protocol's particle fields ([`particles::ParticleExtras`], [`particles::EmitterExtras`]) are read from the client's
//!   datablock as torque-ghosts' extension block made it and held to its ranges again (`hostile_engine_fields_are_held_to_their_ranges_as_they_are_read`);
//!   a datablock that sent none reads the defaults that leave TGE's look and motion. A new field is added in torque-ghosts'
//!   `effects.rs` first, then in `particles_extras.rs` (the reader) and in whatever draws or steps with it.
//! - The air carries a particle: `p += (v + wind * windCoefficient) dt`, in [`particles::Emitter::set_wind`] (a set's
//!   [`particles::Emitters::set_wind`]) and in the pool's step, the Sky's [`sky::wind`] held to [`sky::MAX_WIND`]
//!   (`the_wind_carries_a_dragless_particle_its_coefficient_of_the_winds_speed`; torque-render-hd's
//!   `the_pool_carries_its_particles_by_the_wind_as_the_cpu_path_does`).
//! - A ghosted `ParticleEmitterNode` emits (`emitter_nodes`, run by torque-app's `GhostScene`): `1000 / ejectionPeriodMS`
//!   particles a second at any frame rate, `timeMultiple` (an unset one 1, held to `0.01..=100`) scaling its clock, at most
//!   [`emitter_nodes::MAX_NODES`] nodes making at most [`emitter_nodes::MAX_SPAWNS_PER_NODE`] particles each and
//!   [`emitter_nodes::MAX_SPAWNS_PER_FRAME`] together a frame, a node whose pose is not finite emitting nothing (`a_ghosted_emitter_node_spawns_a_request_every_ejection_period`,
//!   `hostile_and_missing_nodes_make_little_and_a_gone_node_is_forgotten`, `a_frames_particles_are_capped_a_node_and_in_all`,
//!   `nodes_whose_particles_die_at_birth_do_not_leak_the_live_count`). A datablock's `windCoefficient`, `constantAcceleration` and
//!   `spinSpeed` are held to [`particles::MAX_WIND_COEFFICIENT`], [`particles::MAX_CONSTANT_ACCELERATION`] and
//!   [`particles::MAX_SPIN_SPEED`] as they are read (`hostile_wind_acceleration_and_spin_are_held_to_their_ranges`).
//! - A frame's moving shapes under [`moving::with_budget`] build at most [`moving::ShapeBudget::max_vertices`], the nearest first
//!   (the control object always), the rest lowered or left out and counted
//!   (`a_frames_moving_shapes_stay_within_the_vertex_budget_nearest_first`).
//! - A build parses at most its budget of shape files; the rest wait for a later one
//!   (`a_frame_parses_at_most_its_budget_of_new_shapes_and_the_rest_wait`); the control object's shapes, the vehicle it drives
//!   (given to [`moving::with_budget`]) and its mounts are not under it, and no other vehicle is
//!   (`the_control_objects_shape_loads_whatever_the_parse_budget_has_left`,
//!   `only_the_control_object_the_vehicle_it_drives_and_its_mounts_load_past_the_parse_budget`). What the budgets did
//!   is counted in the frame's [`moving::ShapeDraws`] alone.
//! - A scene-hierarchy child is drawn at its parent's render transform times the parent's posed node times its blended local
//!   pose, by `torque_hierarchy::compose` as the server composes it ([`mounts::mounted_transform`]), a chain of links and
//!   mounts held to [`mounts::MAX_CARRY_DEPTH`] (torque-plugin-hierarchy's
//!   `the_renderer_draws_each_child_where_the_server_has_it`). A ghost follows one carrier (its link when it has one, else its
//!   mount), and a frame places each carrier once through one [`mounts::Carried`]: a frame's placements cost each ghost once,
//!   whatever its chain (`mounts::tests::a_chain_of_unplaceable_links_costs_one_placement_a_ghost`).
//! - Ghosts of one shape posed the same share one kept build ([`shapecache`]'s content key)
//!   (`ghosts_of_one_shape_posed_the_same_share_one_kept_build`).
//! - A plugin ghost whose draw rests ([`moving::resting`]: a sleeping prop) while nothing draws it anywhere but where its
//!   record puts it (no prediction's pose, no blend left, not mounted, uniform scale, no skin) may be drawn by its host with
//!   its still geometry (torque-app's resting batches): the frame's shapes leave out exactly the ghosts the host says it drew
//!   so ([`moving::Hooks::still`]), so a ghost the host could not keep is still drawn here
//!   (`a_resting_plugin_ghost_is_left_out_only_where_its_host_draws_it_still`).
//! - [`hd::view_cull::ViewCull`] leaves a moving shape out only when no view, no sun shadow's sweep within the shadows' reach,
//!   no water reflection, no seen light's reach and (while the light capture is drawn) no visible distance can show it; never on
//!   the view alone (`a_shape_out_of_view_whose_shadow_falls_in_view_is_kept`,
//!   `a_shape_seen_in_the_water_or_the_capture_or_in_a_lights_reach_is_kept`). A shape left out keeps time: its live pose and
//!   images' clocks advance ([`moving::Hooks::advance_unseen`]) with nothing posed or built
//!   (`a_player_the_cull_leaves_out_keeps_time_but_is_not_posed_or_built`).
//! - Every world position a frame holds moves with [`hd::origin::Shift`], so a frame measured from a [`hd::RenderOrigin`] near
//!   the eye draws the same pixels as one built about the world's origin (`every_position_a_frame_holds_moves_with_it`;
//!   torque-render-hd's `render_origin` test draws it). A new position field joins its type's implementation.
//! - This crate has no GPU and no wgpu in its normal dependencies (`cargo tree -p torque-render-scene -e normal | rg wgpu`
//!   prints nothing), so the traits a game implements on its own scene types live here, not in torque-render-hd: the orphan
//!   rule lets a crate built on this one implement [`hd::ViewScene`] (the scene a world view fills, mode, time and the cells
//!   static batches go in) for its types, and torque-render-hd re-exports it as `ext::ViewScene`
//!   (`hd::view_scene::tests::a_scene_without_batch_cells_takes_the_modes_and_time_and_offers_no_cells` implements it from here).
//! - A dissolve ([`dissolve::Dissolves`], [`hd::dissolve`]) clips by the share eaten's quantile of its object's sampled field, so
//!   the surface drawn is `1 - eaten` of its area whatever the noise's spread
//!   (`the_drawn_share_of_the_samples_is_one_less_the_eaten_share`), and its front emits the samples it crossed since the last
//!   frame: the count tracks the area swept, at most [`dissolve::MAX_SPAWNS_PER_OBJECT`] an object and
//!   [`dissolve::MAX_SPAWNS_PER_FRAME`] in all (`the_fronts_particles_track_the_area_it_swept_and_its_caps`); a look read from a
//!   record is held to its ranges (`hostile_looks_are_held_to_their_ranges`). The field ([`hd::dissolve::field`]) is the
//!   renderer's `dissolve_common.wgsl`'s; a change goes in both (torque-render-hd's `the_cpu_field_is_the_shaders`). Its
//!   particles go through [`particles::Emitter::emit_at`] and the emitters' own hand-over or draws: the pool's public way in.
//! - [`particles::Emitter::hand_over`] (the GPU particle pool's way) leaves the emission as the CPU path has it: the same random
//!   numbers drawn, the same particles made, each handed over in the state the CPU path draws it in
//!   (`handing_over_keeps_the_emission_and_the_particles_state`). The pool's record table ([`hd::sprites`]) only grows, so a record
//!   a particle in the pool names stays valid (`looks_become_records_once_and_their_frames_buckets`).
//!
//! Costs a frame must not grow back (the benches in `benches/scene.rs`, timed under the bench lock):
//! - A ghost posed as it was costs its cache key: its object is the last frame's, its draws moved over, not converted
//!   (`a_ghost_posed_as_it_was_takes_its_last_objects_draws_and_converts_nothing`; bench `ghost frame 128 kept and culled`).
//!   [`hd::objects::carry_motion`] leaves an object with a stamp as [`hd::objects::objects_of`] made it.
//! - [`hd::HdScene::set_particles_and_decals`] makes the frame's particles again in the last frame's draws (no new lists once
//!   their room is there: [`hd::HdDraw::assign_fixed`]) and counts no new set when there was none and is none; the lit sprites
//!   are shaped with no list a quad (bench `particles draw+upload 10k`).
//! - [`hd::TextureTable::intern`] finds a texture it holds by its levels' address with no copy of its path.
//! - Physically based materials ([`hd::pbr`]): a scene's [`hd::pbr::MaterialTable`] beside its texture table, a draw's
//!   [`hd::pbr::PbrDraw`] (its material and one tangent a vertex; a draw whose tangents are not one a vertex is drawn plain).
//!   A material's draw keeps its own base texture so every pass that reads no maps draws it as before. The maps' chains are
//!   made here: color in linear light (`color_maps_are_filtered_in_linear_light`), normals renormalized with their lost length
//!   widening the ORM map's roughness (Toksvig; `a_bumpy_normal_maps_lower_levels_are_unit_and_widen_the_roughness`), a
//!   DirectX map's green flipped as it loads (`a_directx_normal_map_is_flipped_to_opengls_convention`). Maps are hostile
//!   files: [`hd::pbr::load`] refuses a missing, unknown, oversize or broken map and adds nothing for that material
//!   (`maps_that_are_missing_unknown_huge_or_broken_fail_the_material`), and counts every map's working memory
//!   ([`hd::pbr::working_bytes`]) from the headers before any is decoded (`maps_over_the_working_memory_are_refused_from_their_headers`);
//!   the chains keep two float levels alive at a time, and every byte a build allocates is under that count. A material is
//!   refused when the scene's texture table would pass [`hd::pbr::MAX_SCENE_TEXTURE_BYTES`] with it
//!   (`a_scene_of_many_large_maps_is_refused_past_its_texture_budget`) or its factors are out of range
//!   (`tests/pbr_memory.rs`). The uniform's layout is `pbr.wgsl`'s
//!   `PbrMaterial` (`the_uniform_holds_the_factors_maps_and_alpha_mode`): a field added goes in both.
//! - A cutout material's color chain keeps level 0's coverage on every level ([`hd::pbr::coverage`]; ties split evenly, no
//!   alpha left within 8 steps of the cutoff so a block encoder flips none: `a_cutout_keeps_its_coverage_down_the_chain`).
//! - Baked maps ([`hd::pbr::bake`], [`hd::pbr::bake_files`]): the same chains, then BC7 (color, ORM), BC5 (normal) or BC4
//!   (height) in KTX 2.0 files with Zstandard levels; a map whose sides are not whole blocks stays RGBA8
//!   (`sides_that_are_not_whole_blocks_bake_uncompressed`). [`hd::pbr::load`] takes a role's baked file beside its images
//!   ([`hd::pbr::baked_path`]) only when its source stamp (the images' bytes and the chains' settings) matches, or when the
//!   images are not there (`a_baked_file_of_other_sources_is_passed_over_for_the_maps`), and a sidecar may name `.ktx2`
//!   (and `.exr`) maps itself; a KTX 2.0 map's format must fit its role, its header is checked against the map side cap
//!   before a level is decoded, and its levels count against the material's working memory and the scene's budget as they
//!   are (`a_sidecar_may_name_ktx2_maps_and_a_wrong_or_broken_one_fails_the_material`). A [`Texture`]'s levels are RGBA8
//!   unless it is such a map ([`TexelFormat`]); every other reader of a texture's levels may keep assuming RGBA8. The
//!   pass condition is `just texture-verify` (the `texture-verify` example and `torque-images`' fuzz).

pub mod camera;
pub mod decals;
pub mod dissolve;
pub mod distorters;
pub mod emitter_nodes;
pub mod env;
pub mod first_person;
pub mod flare;
pub mod hd;
pub mod images;
pub mod lamps;
pub mod lights;
pub mod map;
pub mod math;
pub mod mounts;
pub mod moving;
pub mod par;
pub mod particles;
pub mod player;
pub mod ribbons;
pub mod shapecache;
pub mod shapes;
pub mod sight;
pub mod sky;
pub mod source;
pub mod textures;
pub mod tge;
pub mod tracks;
pub mod vehicle_dust;
pub mod wheels;
pub mod world;

#[cfg(test)]
mod tests;

/// The caller's file reader: the bytes of `path` (such as `data/shapes/rock.png`, matched ignoring case as TGE's
/// resource manager does), `None` when there is no such file.
pub trait Assets {
    fn read(&self, path: &str) -> Option<Vec<u8>>;
}

/// The frame's default size (16:9).
pub const DEFAULT_SIZE: (u32, u32) = (1280, 720);

/// A vertex as a fixed-function vertex buffer holds one (position, normal, RGBA8 colour, UV).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vertex {
    /// World space.
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub color: [u8; 4],
    pub uv: [f32; 2],
}

/// `GL_TEXTURE_ENV_MODE`, or no texture at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TexEnv {
    /// No texture bound: the fragment's colour.
    None,
    /// `GL_DECAL`: colour x (1 - texture alpha) + texture x texture alpha; the fragment's alpha.
    Decal,
    /// `GL_MODULATE`: colour x texture.
    Modulate,
    /// `GL_REPLACE`: the texture.
    Replace,
}

/// A texture filter (`GL_NEAREST` / `GL_LINEAR`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Filter {
    Nearest,
    Linear,
}

/// How a draw samples its texture: wrap (`GL_REPEAT`, else `GL_CLAMP`, which NVIDIA's GL treats as clamp to edge), the
/// magnification filter, and the minification filter as its two halves (`GL_NEAREST_MIPMAP_LINEAR` is `Nearest` within a level,
/// `Linear` between levels).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sampler {
    pub repeat: bool,
    pub mag: Filter,
    pub min: Filter,
    pub mip: Filter,
    /// Whether the levels below the first are sampled (a `NoMipMap` material's texture is drawn from level 0 only).
    pub mipmaps: bool,
}

/// Blending (`glBlendFunc`), when enabled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Blend {
    Opaque,
    /// `SRC_ALPHA, ONE_MINUS_SRC_ALPHA`.
    Alpha,
    /// `SRC_ALPHA, ONE` (additive).
    Additive,
    /// `ONE, ONE` (an fxLight's corona with `BlendMode` 2).
    One,
    /// `DST_COLOR, ONE_MINUS_SRC_ALPHA` (multiplies the destination).
    Multiply,
    /// `DST_COLOR, SRC_COLOR`: twice the fragment's color times the destination ("mod2x"), the color first mixed toward 0.5
    /// gray by one minus its alpha (0.5 leaves the destination as it is; darker darkens, lighter brightens). Fog mixes it
    /// toward 0.5 too. A decal that takes the color, light and shadow of the surface it lies on.
    Modulate2x,
}

/// A point light as TGE's `LightManager::installGLLight` sets a GL light: world position (w = 1), the diffuse colour (zero
/// ambient), and the constant, linear and quadratic attenuation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PointLight {
    pub position: [f32; 3],
    pub diffuse: [f32; 3],
    pub attenuation: [f32; 3],
}

/// Face culling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Cull {
    None,
    /// Cull back faces, the front being clockwise.
    BackCw,
    /// Cull back faces, the front being counter-clockwise.
    BackCcw,
}

/// One draw call: triangles (three vertices each, a quad split as GL splits `GL_QUADS`: 0 1 2, 0 2 3) and their state.
#[derive(Debug, Clone, PartialEq)]
pub struct Draw {
    /// What it is, for the notes and the per-pass debugging images.
    pub label: String,
    pub vertices: Vec<Vertex>,
    /// GL lighting (`GL_LIGHTING` with `GL_COLOR_MATERIAL`: the vertex colour is the ambient and diffuse material).
    pub lighting: bool,
    pub tex_env: TexEnv,
    /// An index into [`Scene::textures`] and how it is sampled (`None` with [`TexEnv::None`]).
    pub texture: Option<(usize, Sampler)>,
    pub blend: Blend,
    pub cull: Cull,
    pub depth_test: bool,
    pub depth_write: bool,
    /// Whether linear distance fog applies ([`Scene::fog`]).
    pub fog: bool,
    /// The point lights installed beside the sun (GL lights 1 on).
    pub lights: Vec<PointLight>,
    /// The vertices' identity across builds (a live window's static chunks): the renderer keeps the
    /// buffers it made for a key and uses them again for a draw with the same key and no vertices of its own. None: none kept.
    pub vertex_key: Option<u64>,
}

/// A texture as TGE uploads it: RGBA8 levels from the full size down to 1 x 1 (the CPU mip chain of `extrudeMipLevels`).
#[derive(Debug, Clone, PartialEq)]
pub struct Texture {
    /// The path it was loaded from.
    pub path: String,
    pub width: u32,
    pub height: u32,
    /// Level 0 first; each level's bytes in [`Texture::format`], rows top to bottom. Shared: the decoded-texture cache and
    /// every frame that draws the file hold one copy.
    pub levels: std::sync::Arc<Vec<Vec<u8>>>,
    /// How the levels hold their texels: RGBA8 for every texture but a physically based material's maps
    /// ([`hd::pbr`]), which may be block-compressed.
    pub format: TexelFormat,
}

/// How a [`Texture`]'s levels hold their texels.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum TexelFormat {
    /// Four bytes a texel: R, G, B, A.
    #[default]
    Rgba8,
    /// Rows of 4 x 4 blocks, top to bottom ([`::images::bc`]).
    Block(::images::bc::BlockFormat),
}

impl TexelFormat {
    /// The bytes of a level of `w` x `h`.
    pub const fn level_bytes(self, w: u32, h: u32) -> u64 {
        match self {
            TexelFormat::Rgba8 => 4 * w as u64 * h as u64,
            TexelFormat::Block(b) => b.level_bytes(w, h),
        }
    }
}

/// The sun as GL light 0: a directional light with no specular term in effect (the materials' specular is GL's default black).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sun {
    /// The unit vector towards the sun, world space (the light's `GL_POSITION` with w = 0).
    pub towards: [f32; 3],
    pub diffuse: [f32; 3],
    pub ambient: [f32; 3],
}

/// Linear distance fog (when a draw has it): the colour reaches `color` at `end` from nothing at `start` (eye distance).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fog {
    pub color: [f32; 3],
    pub start: f32,
    pub end: f32,
}

/// The camera: the matrices and what they were made from.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera {
    /// World position of the eye.
    pub eye: [f32; 3],
    /// World to GL clip space (GL's depth range: z in [-w, w]), column-major as GL takes it.
    pub view_proj: [f32; 16],
    /// Horizontal field of view (radians), the near and far planes.
    pub fov: f32,
    pub near: f32,
    pub far: f32,
}

/// A frame.
#[derive(Debug, Clone, PartialEq)]
pub struct Scene {
    pub width: u32,
    pub height: u32,
    pub camera: Camera,
    /// The colour the frame starts from (`glClear`), RGB 0..1.
    pub clear: [f32; 3],
    pub sun: Sun,
    pub fog: Option<Fog>,
    pub textures: Vec<Texture>,
    pub draws: Vec<Draw>,
    /// What the build could not draw or had to guess, one line each.
    pub notes: Vec<String>,
}
