//! The integration tests, linked as one test program: one module per file.

#[path = "../examples/decal_kit/mod.rs"]
mod decal_kit;

mod ao_banding;
mod ao_sun;
mod area_lights;
mod cloud_cube;
mod dissolve;
mod distortion;
mod gpu_particles;
mod gpu_particles_motion;
mod grain_still;
mod half_history;
mod offscreen_casters;
mod pbr_materials;
mod pipeline_cache;
mod planar_fog;
mod planes_upload;
mod projected_decals;
mod render_origin;
mod shadow_overlays;
mod shadow_reach;
mod smooth_occlusion;
mod still_shadow_edge;
mod sun_disk;
mod white_furnace;
