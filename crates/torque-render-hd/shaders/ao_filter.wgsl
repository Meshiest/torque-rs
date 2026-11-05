// The ambient occlusion's spatial filter (the scene's smooth_occlusion), after the AO pass at its resolution: each AO texel becomes
// the average of the 5 x 5 texels about it, weighted by a Gaussian of their distance and by how near each one's point lies to the
// texel's plane (its point and normal from the prepass) and how alike their normals are: a cross-bilateral filter (Tomasi and
// Manduchi 1998; Petschnigg et al. 2004) that keeps the occlusion off other surfaces and across creases and removes the per-pixel
// noise of the estimate that the history alone only averages over frames. The history keeps the unfiltered estimate; the forward
// pass upsamples this one.
//
// Writes r: the filtered occlusion, g and b as the AO pass wrote them.

@group(0) @binding(0) var<uniform> frame: Frame;
@group(0) @binding(1) var depth: texture_depth_2d;
@group(0) @binding(2) var normals: texture_2d<f32>;
// This frame's AO (the AO pass's output).
@group(0) @binding(4) var ao_in: texture_2d<f32>;

const RADIUS: i32 = 2;
// The Gaussian's sigma (AO texels), the plane's tolerance (a share of the distance, units), the normals' cosine below which a
// neighbor has no weight and the one above which it has all.
const SIGMA: f32 = 1.2;
const PLANE_TOL: vec2<f32> = vec2<f32>(0.01, 0.004);
const NORMAL_COS: vec2<f32> = vec2<f32>(0.8, 0.95);

@vertex
fn vs_full(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let x = f32((i << 1u) & 2u);
    let y = f32(i & 2u);
    return vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
}

// The view-space point at pixel position `px` with reversed depth `d`.
fn view_point(px: vec2<f32>, d: f32) -> vec3<f32> {
    let ndc = vec2<f32>(px.x * frame.screen.z * 2.0 - 1.0, 1.0 - px.y * frame.screen.w * 2.0);
    let p = frame.inv_proj * vec4<f32>(ndc, d, 1.0);
    return p.xyz / p.w;
}

// The internal pixel AO texel `t` stands for (the top left of its block).
fn rep(t: vec2<i32>) -> vec2<i32> {
    let scale = i32(max(frame.ao.w, 1.0));
    return clamp(t * scale, vec2<i32>(0), vec2<i32>(textureDimensions(depth)) - 1);
}

@fragment
fn fs_ao_filter(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let o = vec2<i32>(frag.xy);
    let c = textureLoad(ao_in, o, 0);
    let rp = rep(o);
    let d = textureLoad(depth, rp, 0);
    if d <= 0.0 {
        return c;
    }
    let p = view_point(vec2<f32>(rp) + 0.5, d);
    let n = normalize((frame.view * vec4<f32>(oct_decode(textureLoad(normals, rp, 0).xy), 0.0)).xyz);
    let tol = PLANE_TOL.x * -p.z + PLANE_TOL.y;
    let dims = vec2<i32>(textureDimensions(ao_in));
    var sum = 0.0;
    var weight = 0.0;
    for (var y = -RADIUS; y <= RADIUS; y = y + 1) {
        for (var x = -RADIUS; x <= RADIUS; x = x + 1) {
            let t = o + vec2<i32>(x, y);
            if any(t < vec2<i32>(0)) || any(t >= dims) {
                continue;
            }
            let tp = rep(t);
            let td = textureLoad(depth, tp, 0);
            if td <= 0.0 {
                continue;
            }
            let q = view_point(vec2<f32>(tp) + 0.5, td);
            let tn = normalize((frame.view * vec4<f32>(oct_decode(textureLoad(normals, tp, 0).xy), 0.0)).xyz);
            let pw = clamp(1.0 - abs(dot(q - p, n)) / tol, 0.0, 1.0);
            let nw = smoothstep(NORMAL_COS.x, NORMAL_COS.y, dot(n, tn));
            let r2 = f32(x * x + y * y);
            let w = exp(-r2 / (2.0 * SIGMA * SIGMA)) * pw * nw;
            sum = sum + textureLoad(ao_in, t, 0).r * w;
            weight = weight + w;
        }
    }
    if weight < 1e-4 {
        return c;
    }
    return vec4<f32>(sum / weight, c.g, c.b, 1.0);
}
