// The forward pass's global illumination (with gi_common.wgsl, gi_grid.wgsl and gi_lookup.wgsl): the probes' bindings and the
// ambient the main view's shading takes in place of the sky's irradiance where the probes reach (crate::gi).

@group(0) @binding(22) var gi_atlas: texture_2d<f32>;
@group(0) @binding(23) var<storage, read> gi_state: GiState;
@group(0) @binding(24) var gi_samp: sampler;

// The ambient's radiance at `p` with normal `n`: the probes', `sky` (the sky's irradiance) past them.
fn gi_ambient(p: vec3<f32>, n: vec3<f32>, sky: vec3<f32>) -> vec3<f32> {
    return gi_irradiance(p, n, normalize(frame.eye.xyz - p), sky);
}
