// Screen-space distortion (`distortion.rs`; the look's rules in `torque_render_scene::hd::distortion`).
//
// - `vs_distort` / `fs_distort`: a distorter's triangles into the offset and tint targets, over the frame's depth (what is in
//   front hides it). The offset target's rg is the read's move in UV (added up), b the chromatic split's share (added up), a the
//   nearest distorter's reversed depth (the max); the tint target multiplies (cleared to white).
// - `fs_composite`: the anti-aliased frame read through the offsets into the shown copy. A read that lands on something nearer
//   than the distorter moves half as far, then not at all, so the foreground is not pulled into the bend. A pixel no distorter
//   touches is discarded: the copy keeps the frame's own value.

struct Frame {
    view: mat4x4<f32>,
    proj: mat4x4<f32>,
    // x the effects' time (s), y the internal frame's width over its height, z the most a read moves (a share of the height).
    params: vec4<f32>,
    // The internal frame's size and its inverse.
    internal: vec4<f32>,
    // The output's size and its inverse.
    output: vec4<f32>,
};

struct Look {
    // strength (world units), edge, map scale, chroma.
    a: vec4<f32>,
    // scroll (texture units a second), whether it has a map, whether both sides bend.
    b: vec4<f32>,
    tint: vec4<f32>,
};

@group(0) @binding(0) var<uniform> frame: Frame;
@group(0) @binding(1) var<storage, read> looks: array<Look>;
@group(1) @binding(0) var map_tex: texture_2d<f32>;
@group(1) @binding(1) var map_samp: sampler;

struct VsIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec2<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) color: vec4<f32>,
    @location(4) material: u32,
    @location(5) extra: u32,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) eye_pos: vec3<f32>,
    @location(1) eye_n: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) alpha: f32,
    @location(4) @interpolate(flat) look: u32,
};

fn oct_decode(e: vec2<f32>) -> vec3<f32> {
    var n = vec3<f32>(e.x, e.y, 1.0 - abs(e.x) - abs(e.y));
    let t = max(-n.z, 0.0);
    n.x += select(t, -t, n.x >= 0.0);
    n.y += select(t, -t, n.y >= 0.0);
    return normalize(n);
}

fn srgb_encode(c: vec2<f32>) -> vec2<f32> {
    let lo = c * 12.92;
    let hi = 1.055 * pow(max(c, vec2<f32>(0.0031308)), vec2<f32>(1.0 / 2.4)) - 0.055;
    return select(hi, lo, c <= vec2<f32>(0.0031308));
}

@vertex
fn vs_distort(v: VsIn, @builtin(instance_index) look: u32) -> VsOut {
    var o: VsOut;
    let eye = frame.view * vec4<f32>(v.position, 1.0);
    o.clip = frame.proj * eye;
    o.eye_pos = eye.xyz;
    o.eye_n = (frame.view * vec4<f32>(oct_decode(v.normal), 0.0)).xyz;
    o.uv = v.uv;
    o.alpha = v.color.a;
    o.look = look;
    return o;
}

struct Targets {
    @location(0) offset: vec4<f32>,
    @location(1) tint: vec4<f32>,
};

@fragment
fn fs_distort(in: VsOut) -> Targets {
    let l = looks[in.look];
    // Sampled first, in uniform control flow (a look without a map reads the white texel and drops it).
    // (The scene's textures are sampled through sRGB views: the map's bytes are encoded back.)
    let m = srgb_encode(textureSample(map_tex, map_samp, in.uv * l.a.z + l.b.xy * frame.params.x).xy) * 2.0 - 1.0;
    var n = normalize(in.eye_n);
    let v = normalize(-in.eye_pos);
    var ndv = dot(n, v);
    if ndv < 0.0 {
        if l.b.w < 0.5 {
            discard;
        }
        n = -n;
        ndv = -ndv;
    }
    // Every distorter fades out at its silhouette; `edge` gathers the bend toward it.
    // Held to 0..1: interpolated normals can make it a hair past 1, and pow of a negative base is NaN.
    ndv = clamp(ndv, 0.0, 1.0);
    let seen = in.alpha * smoothstep(0.0, 0.2, ndv);
    var w = seen;
    if l.a.y > 0.0 {
        w *= pow(max(1.0 - ndv, 0.0), l.a.y);
    }
    let bend = -n.xy + m * l.b.z;
    // World units at the surface's distance to UV (y down).
    let depth = max(-in.eye_pos.z, 1e-3);
    var off = bend * (l.a.x * w * 0.5 / depth) * vec2<f32>(frame.proj[0][0], -frame.proj[1][1]);
    let in_height = length(vec2<f32>(off.x * frame.params.y, off.y));
    if in_height > frame.params.z {
        off *= frame.params.z / in_height;
    }
    var t: Targets;
    t.offset = vec4<f32>(off, l.a.w * w, in.clip.z);
    t.tint = vec4<f32>(mix(vec3<f32>(1.0), l.tint.rgb, l.tint.a * seen), 1.0);
    return t;
}

@group(0) @binding(2) var source: texture_2d<f32>;
@group(0) @binding(3) var offsets: texture_2d<f32>;
@group(0) @binding(4) var tints: texture_2d<f32>;
@group(0) @binding(5) var depth_tex: texture_depth_2d;
@group(0) @binding(6) var lin_samp: sampler;

// Whether what the frame shows at `p` (UV) is nearer than reversed depth `z`.
fn nearer(p: vec2<f32>, z: f32) -> bool {
    let size = vec2<i32>(frame.internal.xy);
    let q = clamp(vec2<i32>(p * frame.internal.xy), vec2<i32>(0), size - 1);
    return textureLoad(depth_tex, q, 0) > z + 1e-6;
}

@fragment
fn fs_composite(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let uv = frag.xy * frame.output.zw;
    let d = textureSampleLevel(offsets, lin_samp, uv, 0.0);
    let tint = textureSampleLevel(tints, lin_samp, uv, 0.0).rgb;
    if abs(d.x) + abs(d.y) < 1e-6 && all(tint > vec3<f32>(0.999)) {
        discard;
    }
    let split = 1.0 + d.z;
    var off = d.xy;
    if nearer(uv + off * split, d.w) {
        off *= 0.5;
        if nearer(uv + off * split, d.w) {
            off = vec2<f32>(0.0);
        }
    }
    let c = textureSampleLevel(source, lin_samp, uv + off, 0.0);
    var rgb = c.rgb;
    if d.z > 0.0 {
        rgb.r = textureSampleLevel(source, lin_samp, uv + off * split, 0.0).r;
        rgb.b = textureSampleLevel(source, lin_samp, uv + off * (1.0 - d.z), 0.0).b;
    }
    return vec4<f32>(rgb * tint, c.a);
}
