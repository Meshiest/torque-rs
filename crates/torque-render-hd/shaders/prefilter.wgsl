// The environment cubes' prefiltered mips (the light capture and the sky cube): level m of a face is the GGX lobe of the
// roughness level m stands for (brdf.wgsl's level_rough) over the whole cube, so a rough surface reflects the lobe's integral of
// its surroundings, not a box blur of one face. Our own code: the split sum's prefilter (Karis 2013, N = V = R), importance-
// sampled with PREFILTER_SAMPLES of the Hammersley set, each sample's light weighted by N.L. Each level is filtered from the
// one above it with the lobe that widens that one's to its own (the GGX alphas' squares add, as the variances of two narrow
// lobes do), so a sample spans a few of the source's texels at most and needs no lower mip. Prepended: common.wgsl's PI, brdf.wgsl,
// cubeface.wgsl and the cube's size and levels (CUBE_SIZE, CUBE_LEVELS).
//
// A compute filter: a dispatch writes one face's level (FACE, a pipeline constant) from the cube's level above it; the alpha (the
// capture's distance) is filtered as the color is.

override FACE: u32 = 0u;

@group(0) @binding(0) var src: texture_cube<f32>;
@group(0) @binding(2) var dst: texture_storage_2d<rgba16float, write>;
@group(1) @binding(0) var samp: sampler;

const PREFILTER_SAMPLES: u32 = 32u;

@compute @workgroup_size(8, 8)
fn cs_prefilter(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(dst);
    if any(id.xy >= size) {
        return;
    }
    let m = log2(CUBE_SIZE / f32(size.x));
    let a = level_rough(m, CUBE_LEVELS);
    let p = level_rough(m - 1.0, CUBE_LEVELS);
    let alpha = sqrt(max(a * a * a * a - p * p * p * p, 0.0));
    let uv = (vec2<f32>(id.xy) + 0.5) / vec2<f32>(size) * 2.0 - 1.0;
    let n = normalize(face_dir(FACE, uv));
    // A lobe narrower than half the level's texel: the texels it covers (one bilinear tap at their shared corner, a 2 x 2 box).
    var c = textureSampleLevel(src, samp, n, 0.0);
    if alpha * f32(size.x) > 0.5 {
        let up = select(vec3<f32>(1.0, 0.0, 0.0), vec3<f32>(0.0, 0.0, 1.0), abs(n.z) < 0.999);
        let t = normalize(cross(up, n));
        let b = cross(n, t);
        var sum = vec4<f32>(0.0);
        var weight = 0.0;
        for (var i = 0u; i < PREFILTER_SAMPLES; i = i + 1u) {
            let h = ggx_half(hammersley(i, PREFILTER_SAMPLES), alpha);
            let hw = t * h.x + b * h.y + n * h.z;
            let l = 2.0 * dot(n, hw) * hw - n;
            let nl = dot(n, l);
            if nl > 0.0 {
                sum = sum + textureSampleLevel(src, samp, l, 0.0) * nl;
                weight = weight + nl;
            }
        }
        c = sum / max(weight, 1e-6);
    }
    textureStore(dst, id.xy, c);
}
