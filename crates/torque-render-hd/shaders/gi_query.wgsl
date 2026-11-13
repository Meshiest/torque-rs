// The probes' ambient at listed points (after gi_common.wgsl, gi_grid.wgsl and gi_lookup.wgsl): what the forward pass's
// lookup gives each point and normal, black past the probes (the checks compare it with the reference).

@group(0) @binding(0) var<storage, read> gi_state: GiState;
@group(0) @binding(1) var gi_atlas: texture_2d<f32>;
@group(0) @binding(2) var gi_samp: sampler;
// Each point (xyz) and its normal (the next vec4's xyz).
@group(0) @binding(3) var<storage, read> points: array<vec4<f32>>;
@group(0) @binding(4) var<storage, read_write> answers: array<vec4<f32>>;

@compute @workgroup_size(64)
fn cs_query(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = id.x;
    if 2u * i + 1u >= arrayLength(&points) {
        return;
    }
    let p = points[2u * i].xyz;
    let n = normalize(points[2u * i + 1u].xyz);
    answers[i] = vec4<f32>(gi_irradiance(p, n, n, vec3<f32>(0.0)), 1.0);
}
