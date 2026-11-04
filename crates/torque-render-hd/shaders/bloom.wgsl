// The enhanced renderer's bloom: a dual-filter mip chain (after Bjørge 2015's dual filtering) that conserves the light it is
// given. Our own code.
//
// - Down (fs_prefilter from the frame, fs_down from the level above): five bilinear taps of the level above, the 2 x 2 block under the
//   texel's centre (weight 1/2) and the four blocks one texel diagonally out (1/8 each): weights summing to 1, and every texel above
//   giving its light to the texels below in equal total (a quarter each: the area's ratio). The prefilter first weighs each tap by the
//   settings' soft threshold (hd::grade::bloom_weight: none below the threshold less the knee, a quadratic knee, then the
//   share of the colour over the threshold).
// - Up (fs_up): each level the mix of its own down texel and a 3 x 3 tent (1 2 1) of the level below's up texels, both conserving,
//   by the spread: the chain's last up level holds the light of its first down level, spread wide.
// - The sampler mirrors at the edges (a mirrored edge reflects what a filter would spill over it back in: none is lost or made).

struct Level {
    // xy: one texel of the source (uv), zw: one texel of the target (uv).
    texel: vec4<f32>,
    // x: the threshold (0: none), y: its knee, z: the up mix's spread, w: unused.
    params: vec4<f32>,
};

@group(0) @binding(0) var<uniform> level: Level;
@group(0) @binding(1) var src: texture_2d<f32>;
@group(0) @binding(2) var mirror: sampler;
@group(0) @binding(3) var below: texture_2d<f32>;
// The level written (compute filters: the whole chain one pass).
@group(0) @binding(4) var dst: texture_storage_2d<rgba16float, write>;

// The texel `id` of the level written, and its centre in pixels (a fragment's position).
fn centre(id: vec3<u32>) -> vec2<f32> {
    return vec2<f32>(id.xy) + vec2<f32>(0.5);
}

fn tap(uv: vec2<f32>) -> vec3<f32> {
    let c = textureSampleLevel(src, mirror, uv, 0.0).rgb;
    // No NaN or infinity into the chain.
    return clamp(select(c, vec3<f32>(0.0), c != c), vec3<f32>(0.0), vec3<f32>(60000.0));
}

fn threshold(c: vec3<f32>) -> vec3<f32> {
    let t = level.params.x;
    if t <= 0.0 {
        return c;
    }
    let k = level.params.y;
    let l = max(dot(c, vec3<f32>(0.2126, 0.7152, 0.0722)), 1e-6);
    var soft = 0.0;
    if k > 0.0 {
        let x = clamp(l - t + k, 0.0, 2.0 * k);
        soft = x * x / (4.0 * k);
    }
    return c * (max(max(soft, l - t), 0.0) / l);
}

fn down(p: vec2<f32>, thresholded: bool) -> vec3<f32> {
    let uv = p * level.texel.zw;
    let o = level.texel.xy;
    var taps = array<vec2<f32>, 5>(uv, uv + vec2<f32>(-o.x, -o.y), uv + vec2<f32>(o.x, -o.y), uv + vec2<f32>(-o.x, o.y), uv + vec2<f32>(o.x, o.y));
    var sum = vec3<f32>(0.0);
    for (var i = 0u; i < 5u; i = i + 1u) {
        var c = tap(taps[i]);
        if thresholded {
            c = threshold(c);
        }
        sum = sum + c * select(0.125, 0.5, i == 0u);
    }
    return sum;
}

@compute @workgroup_size(8, 8)
fn cs_prefilter(@builtin(global_invocation_id) id: vec3<u32>) {
    if any(id.xy >= textureDimensions(dst)) {
        return;
    }
    textureStore(dst, id.xy, vec4<f32>(down(centre(id), true), 1.0));
}

@compute @workgroup_size(8, 8)
fn cs_down(@builtin(global_invocation_id) id: vec3<u32>) {
    if any(id.xy >= textureDimensions(dst)) {
        return;
    }
    textureStore(dst, id.xy, vec4<f32>(down(centre(id), false), 1.0));
}

@compute @workgroup_size(8, 8)
fn cs_up(@builtin(global_invocation_id) id: vec3<u32>) {
    if any(id.xy >= textureDimensions(dst)) {
        return;
    }
    textureStore(dst, id.xy, up(centre(id)));
}

fn up(p: vec2<f32>) -> vec4<f32> {
    let uv = p * level.texel.zw;
    let own = textureSampleLevel(src, mirror, uv, 0.0).rgb;
    // One texel of the level below: twice the target's.
    let o = 2.0 * level.texel.zw;
    var sum = vec3<f32>(0.0);
    for (var j = -1; j <= 1; j = j + 1) {
        for (var i = -1; i <= 1; i = i + 1) {
            let w = f32((2 - abs(i)) * (2 - abs(j))) / 16.0;
            sum = sum + textureSampleLevel(below, mirror, uv + vec2<f32>(f32(i), f32(j)) * o, 0.0).rgb * w;
        }
    }
    return vec4<f32>(mix(own, sum, level.params.z), 1.0);
}
