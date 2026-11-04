// Screen-space reflections: traced through the Hi-Z pyramid at half (High) or full (Ultra) resolution, then accumulated over
// the frames. Our own code from the published techniques: the hierarchical traversal of Uludag 2014 ("Hi-Z Screen-Space Cone-Traced
// Reflections", GPU Pro 5), written for both ray directions and a reversed depth; the rough reflection as its cone's footprint in a
// mip chain of the colour (the same paper); the accumulation's neighbourhood clamp as in Karis 2014's TAA. Prepended: common.wgsl.
//
// - The reflector at an SSR texel's pixel: the water plane (Medium's water: the view ray meets the plane before the opaque depth),
//   else a surface of a reflective class (the prepass's class), else nothing (weight 0).
// - The ray: from the reflector along the mirror direction to the screen's edge (or the near plane), in pixels and reversed depth
//   (linear in screen space). The traversal starts at the finest level; a cell the ray stays in front of all through (its nearest
//   depth behind the ray's farthest point in it) is skipped and the next one looked at a level up, else it looks a level down; at the
//   finest level the ray has reached the surface: a hit when it is not more than a thickness behind it, else it goes on behind it.
// - The texels sit still under the TAA's jitter: texel i holds the reflection at its own place in the frame without the jitter, traced
//   from where the ray through that place meets the plane of the jittered pixel whose sample is nearest to it (its depth and normal;
//   a face is taken as flat), and its history is read where that place was last frame (the reflector's motion). The reflection is then
//   a texture under the TAA, which smooths it like any other.
// - The colour of a hit: the previous frame's (the TAA's anti-aliased colour, else the frame after the translucent pass) where the hit
//   was then (the camera's motion), from the mip whose texel is the reflection cone's footprint (roughness), faded at the screen's
//   edges; a back face is a miss.
// - Accumulated: this frame's (premultiplied colour, weight) blended half and half with the history at the reflector's previous place,
//   the history clamped to this frame's 3 x 3 neighbourhood. A slower blend lags far behind: a metal reflects last frame's colour,
//   the metal among it, so a still frame converges only as fast as that loop: the half and half blend keeps a still frame steady.

@group(0) @binding(0) var<uniform> frame: Frame;
@group(0) @binding(1) var<uniform> rf: Refl;
@group(0) @binding(2) var hiz: texture_2d<f32>;
@group(0) @binding(3) var normals: texture_2d<f32>;
@group(0) @binding(4) var prev_color: texture_2d<f32>;
@group(0) @binding(5) var lin_samp: sampler;
@group(0) @binding(6) var raw: texture_2d<f32>;
@group(0) @binding(7) var prev_uv: texture_2d<f32>;
@group(0) @binding(8) var history: texture_2d<f32>;

const HIZ_TOP: i32 = 7;

@vertex
fn vs_full(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let x = f32((i << 1u) & 2u);
    let y = f32(i & 2u);
    return vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
}

// The view-space point at internal pixel `px` (continuous) with reversed depth `d`.
fn view_point(px: vec2<f32>, d: f32) -> vec3<f32> {
    let ndc = vec2<f32>(px.x * frame.screen.z * 2.0 - 1.0, 1.0 - px.y * frame.screen.w * 2.0);
    let p = frame.inv_proj * vec4<f32>(ndc, d, 1.0);
    return p.xyz / p.w;
}

// A view-space point in internal pixels and reversed depth.
fn to_screen(p: vec3<f32>) -> vec3<f32> {
    let c = frame.proj * vec4<f32>(p, 1.0);
    let n = c.xyz / c.w;
    return vec3<f32>((n.x * 0.5 + 0.5) * frame.screen.x, (0.5 - n.y * 0.5) * frame.screen.y, n.z);
}

fn view_rot() -> mat3x3<f32> {
    return mat3x3<f32>(frame.view[0].xyz, frame.view[1].xyz, frame.view[2].xyz);
}

fn to_world(p: vec3<f32>) -> vec3<f32> {
    return transpose(view_rot()) * (p - frame.view[3].xyz);
}

// A world point's uv in the previous frame (-1 when it was behind the eye).
fn prev_uv_of(w: vec3<f32>) -> vec2<f32> {
    let c = frame.prev_view_proj * vec4<f32>(w, 1.0);
    if c.w <= 1e-6 {
        return vec2<f32>(-1.0);
    }
    let n = c.xy / c.w;
    return vec2<f32>(n.x * 0.5 + 0.5, 0.5 - n.y * 0.5);
}

fn hiz_at(cell: vec2<f32>, level: i32) -> f32 {
    let dims = vec2<i32>(textureDimensions(hiz, level));
    return textureLoad(hiz, clamp(vec2<i32>(cell), vec2<i32>(0), dims - 1), level).r;
}

// How far behind a surface (linear depth `lin`) a ray still hits it.
fn thickness(lin: f32) -> f32 {
    return 0.3 + 0.03 * lin;
}

// The Hi-Z traversal from `o` to `e` (internal pixels, reversed depth): xy the hit's pixel, z its depth, w 1 on a hit.
fn hiz_trace(o: vec3<f32>, e: vec3<f32>, steps: u32) -> vec4<f32> {
    let delta = e - o;
    let span = max(abs(delta.x), abs(delta.y));
    if span < 1.0 {
        return vec4<f32>(0.0);
    }
    let t_px = 1.0 / span;
    let eps = t_px * 0.01;
    let forward = delta.xy >= vec2<f32>(0.0);
    let moving = abs(delta.xy) > vec2<f32>(1e-6);
    var t = t_px * 1.5;
    var level = 0;
    for (var i = 0u; i < steps; i = i + 1u) {
        if t >= 1.0 {
            break;
        }
        let p = o + delta * t;
        let cell_size = f32(1 << u32(level));
        let cell = floor(p.xy / cell_size);
        let next = (cell + select(vec2<f32>(0.0), vec2<f32>(1.0), forward)) * cell_size;
        let tb = select(vec2<f32>(1e9), (next - o.xy) / select(vec2<f32>(1.0), delta.xy, moving), moving);
        let t_exit = min(min(tb.x, tb.y), 1.0);
        let z_tile = hiz_at(cell, level);
        let z_far = min(p.z, o.z + delta.z * t_exit);
        if z_far > z_tile {
            t = t_exit + eps;
            level = min(level + 1, HIZ_TOP);
        } else if level > 0 {
            level = level - 1;
        } else {
            let behind = linear_depth(z_far) - linear_depth(z_tile);
            if behind < thickness(linear_depth(z_tile)) {
                var th = t;
                if abs(delta.z) > 1e-12 {
                    th = clamp((z_tile - o.z) / delta.z, t, t_exit);
                }
                let q = o + delta * th;
                return vec4<f32>(q.xy, z_tile, 1.0);
            }
            t = t_exit + eps;
        }
    }
    return vec4<f32>(0.0);
}

struct TraceOut {
    // rgb: the reflected colour times its weight, a: the weight.
    @location(0) color: vec4<f32>,
    // xy: the reflector's uv in the previous frame (-1: none).
    @location(1) prev: vec4<f32>,
};

@fragment
fn fs_trace(@builtin(position) frag: vec4<f32>) -> TraceOut {
    var out: TraceOut;
    out.color = vec4<f32>(0.0);
    out.prev = vec4<f32>(-1.0, -1.0, 0.0, 0.0);
    let size = vec2<i32>(frame.screen.xy);
    let scale = max(rf.water.w, 1.0);
    let j = frame.temporal.xy;
    let place = (floor(frag.xy) + 0.5) * scale;
    let px = clamp(vec2<i32>(floor(place + j)), vec2<i32>(0), size - 1);
    let pxc = vec2<f32>(px) + 0.5;
    let d = textureLoad(hiz, px, 0).r;
    let nrm = textureLoad(normals, px, 0);
    let rot = view_rot();
    var p = vec3<f32>(0.0);
    var nv = vec3<f32>(0.0);
    var rough = 0.0;
    var found = false;
    // The water plane (Medium: its reflection is traced here).
    if u32(rf.mode.y + 0.5) == 2u && rf.water.y > 0.5 {
        let dir = normalize(view_point(place + j, 1.0));
        let np = rot * vec3<f32>(0.0, 0.0, 1.0);
        let c = dot(np, (frame.view * vec4<f32>(0.0, 0.0, rf.water.x, 1.0)).xyz);
        let den = dot(np, dir);
        if abs(den) > 1e-6 {
            let t = c / den;
            if t > 0.0 && (d <= 0.0 || -(dir * t).z < linear_depth(d)) {
                p = dir * t;
                nv = np;
                found = true;
            }
        }
    }
    if !found {
        let kind = u32(nrm.z + 0.5);
        if kind == 0u || d <= 0.0 || rf.mode.x < 0.5 {
            return out;
        }
        let sample = view_point(pxc, d);
        nv = normalize(rot * oct_decode(nrm.xy));
        rough = select(rf.ext.classes.y, rf.ext.classes.x, kind == 1u);
        // The surface's plane met by the ray through the texel's own place (a face is taken as flat), unless that is far off.
        p = sample;
        let ray = normalize(view_point(place + j, 1.0));
        let den = dot(nv, ray);
        if abs(den) > 0.05 {
            let at = ray * (dot(nv, sample) / den);
            if distance(at, sample) < 0.02 * -sample.z + 0.01 {
                p = at;
            }
        }
    }
    let before = prev_uv_of(to_world(p));
    if all(before >= vec2<f32>(0.0)) {
        let now = to_screen(p).xy - j;
        out.prev = vec4<f32>(place * frame.screen.zw + before - now * frame.screen.zw, 0.0, 0.0);
    }
    if rf.ssr.z < 0.5 {
        return out;
    }
    let vdir = normalize(p);
    let r = reflect(vdir, nv);
    // The ray's end: far enough to leave the screen, kept in front of the near plane.
    let near = frame.cluster_z.x;
    var len = 0.5 * frame.cluster_z.y;
    let start = p + nv * (0.002 + 0.002 * -p.z);
    if r.z > 1e-6 {
        len = min(len, (-near * 1.01 - start.z) / r.z);
    }
    if len <= 0.0 {
        return out;
    }
    let o = to_screen(start);
    var e = to_screen(start + r * len);
    // Kept on the screen.
    let dxy = e.xy - o.xy;
    var tmax = 1.0;
    let lim = frame.screen.xy - 0.5;
    if dxy.x > 1e-6 {
        tmax = min(tmax, (lim.x - o.x) / dxy.x);
    } else if dxy.x < -1e-6 {
        tmax = min(tmax, (0.5 - o.x) / dxy.x);
    }
    if dxy.y > 1e-6 {
        tmax = min(tmax, (lim.y - o.y) / dxy.y);
    } else if dxy.y < -1e-6 {
        tmax = min(tmax, (0.5 - o.y) / dxy.y);
    }
    if tmax <= 0.0 {
        return out;
    }
    e = o + (e - o) * tmax;
    // Rougher surfaces take fewer steps.
    let steps = u32(max(rf.ssr.y * (1.0 - 0.5 * rough), 16.0));
    let hit = hiz_trace(o, e, steps);
    if hit.w < 0.5 {
        return out;
    }
    let hp = view_point(hit.xy, hit.z);
    let hw = to_world(hp);
    // A back face (the ray went through a surface) is a miss.
    let hn = oct_decode(textureLoad(normals, clamp(vec2<i32>(hit.xy), vec2<i32>(0), size - 1), 0).xy);
    let rw = transpose(rot) * r;
    if dot(hn, rw) > 0.1 {
        return out;
    }
    let uv = prev_uv_of(hw);
    if any(uv < vec2<f32>(0.0)) || any(uv > vec2<f32>(1.0)) {
        return out;
    }
    let now = hit.xy * frame.screen.zw;
    let edge = min(min(now.x, 1.0 - now.x), min(now.y, 1.0 - now.y));
    let fade = clamp(edge / 0.06, 0.0, 1.0);
    // The cone's footprint (GGX's alpha as its tangent) over the ray's length in pixels picks the mip.
    var lod = 0.0;
    if rf.mode.w < 0.5 {
        let a = rough * rough;
        let foot = 2.0 * a * 0.5 * length(hit.xy - o.xy);
        lod = clamp(log2(max(foot, 1.0)), 0.0, rf.ssr.x - 1.0);
    }
    let c = textureSampleLevel(prev_color, lin_samp, uv, lod).rgb;
    let w = fade;
    out.color = vec4<f32>(max(c, vec3<f32>(0.0)) * w, w);
    return out;
}

@fragment
fn fs_resolve(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let p = vec2<i32>(frag.xy);
    let dims = vec2<i32>(textureDimensions(raw));
    let c = textureLoad(raw, p, 0);
    var lo = c;
    var hi = c;
    for (var dy = -1; dy <= 1; dy = dy + 1) {
        for (var dx = -1; dx <= 1; dx = dx + 1) {
            let s = textureLoad(raw, clamp(p + vec2<i32>(dx, dy), vec2<i32>(0), dims - 1), 0);
            lo = min(lo, s);
            hi = max(hi, s);
        }
    }
    let uv = textureLoad(prev_uv, p, 0).xy;
    if rf.ssr.z < 0.5 || any(uv < vec2<f32>(0.0)) || any(uv > vec2<f32>(1.0)) {
        return c;
    }
    let h = clamp(textureSampleLevel(history, lin_samp, uv, 0.0), lo, hi);
    return mix(h, c, 0.5);
}
