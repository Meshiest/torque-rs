// The forward pass's surface helpers: value noise, a surface's UV frame from the screen derivatives and stochastic tiling.
// Composed before the extension's forward chunk and forward.wgsl.

// Value noise (quintic fade) and its gradient, our own (prefixed: water.wgsl has its own).
fn mat_hash(p: vec2<i32>) -> f32 {
    var h = (u32(p.x) * 0x27d4eb2du) ^ (u32(p.y) * 0x165667b1u);
    h = (h ^ (h >> 15u)) * 0x2c1b3c6du;
    h = (h ^ (h >> 12u)) * 0x297a2d39u;
    h = h ^ (h >> 15u);
    return f32(h & 0xffffffu) / 16777216.0;
}

fn mat_noise(p: vec2<f32>) -> vec3<f32> {
    let i = vec2<i32>(floor(p));
    let f = p - floor(p);
    let u = f * f * f * (f * (f * 6.0 - 15.0) + 10.0);
    let du = 30.0 * f * f * (f * (f - 2.0) + 1.0);
    let a = mat_hash(i);
    let b = mat_hash(i + vec2<i32>(1, 0));
    let c = mat_hash(i + vec2<i32>(0, 1));
    let d = mat_hash(i + vec2<i32>(1, 1));
    let v = a + (b - a) * u.x + (c - a) * u.y + (a - b - c + d) * u.x * u.y;
    let g = du * vec2<f32>(b - a + (a - b - c + d) * u.y, c - a + (a - b - c + d) * u.x);
    return vec3<f32>(v, g);
}

// What the screen derivatives say of a surface: dp/du and dp/dv (units per UV unit; zero where the UVs are degenerate).
struct UvFrame {
    pu: vec3<f32>,
    pv: vec3<f32>,
    ok: bool,
};

fn uv_frame(dpx: vec3<f32>, dpy: vec3<f32>, duvx: vec2<f32>, duvy: vec2<f32>) -> UvFrame {
    var f: UvFrame;
    let det = duvx.x * duvy.y - duvx.y * duvy.x;
    f.ok = abs(det) > 1e-12;
    let inv = select(0.0, 1.0 / det, f.ok);
    f.pu = (dpx * duvy.y - dpy * duvx.y) * inv;
    f.pv = (dpy * duvx.x - dpx * duvy.x) * inv;
    return f;
}

// Stochastic tiling (Quilez 2017): the texture at `uv` with two random offsets of the noise's region, blended by its fraction.
fn stochastic_sample(uv: vec2<f32>, duvx: vec2<f32>, duvy: vec2<f32>) -> vec4<f32> {
    let k = mat_noise(uv * 0.21).x;
    let index = k * 8.0;
    let i = floor(index);
    let f = fract(index);
    let offa = sin(vec2<f32>(3.0, 7.0) * i);
    let offb = sin(vec2<f32>(3.0, 7.0) * (i + 1.0));
    let ca = textureSampleGrad(tex, samp, uv + offa, duvx, duvy);
    let cb = textureSampleGrad(tex, samp, uv + offb, duvx, duvy);
    let d = ca.rgb - cb.rgb;
    return mix(ca, cb, smoothstep(0.2, 0.8, f - 0.1 * (d.x + d.y + d.z)));
}
