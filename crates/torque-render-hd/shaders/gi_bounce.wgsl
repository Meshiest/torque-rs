// The voxels' bounce (with gi_common.wgsl, gi_grid.wgsl, gi_sky.wgsl and gi_lookup.wgsl, before gi_light.wgsl): the probes' and
// the sky's bindings of the light pass, and the ambient at a voxel (the probes' irradiance last frame, the sky's past them).

@group(0) @binding(7) var<storage, read> gi_state: GiState;
@group(0) @binding(8) var gi_atlas: texture_2d<f32>;
@group(0) @binding(9) var gi_samp: sampler;
@group(0) @binding(10) var<uniform> gi_sky: GiSky;

// The ambient's radiance at a voxel's middle `p` of `level` facing `n` (from half a voxel out of its surface).
fn gi_voxel_ambient(p: vec3<f32>, n: vec3<f32>, level: u32) -> vec3<f32> {
    let s = vol.levels[level].size.x;
    return gi_irradiance(p + n * (0.5 * s), n, n, gi_sky_radiance(n));
}
