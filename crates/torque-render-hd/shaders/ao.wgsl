// Ambient occlusion: one pass at the AO's resolution (half the internal frame's at High, the whole at Ultra) that estimates
// the occlusion at each AO texel's pixel (the top left of its block) from the depth prepass and its normals, then accumulates it with
// the texel's history.
//
// - GTAO (GTAO = true; Jimenez et al. 2016, "Practical Realtime Strategies for Accurate Indirect Occlusion"): in each of SLICES
//   screen-space directions (turned per pixel and per frame by interleaved gradient noise) the highest horizon on either side is found
//   from STEPS depth samples a side within the reach (a sample's horizon fades to the tangent plane's over the last tenth of the reach,
//   so what lies farther in depth, such as the background around an object's outline, occludes nothing), clamped to the hemisphere
//   of the normal projected into the slice, and the cosine-weighted visible arc integrated in closed form. The result is the
//   cosine-weighted visibility of rays of the reach's length against the depth buffer taken as a height field: what the CPU
//   reference measures with rays against the boxes.
// - SSAO (GTAO = false; a normal-oriented hemisphere after Mittring 2007 and Bavoil and Sainz 2008): TAPS points in the hemisphere
//   (cosine-distributed, turned per pixel), each occluded where the depth buffer holds a surface in front of it that lies within the
//   reach of the pixel's point.
// - Temporal accumulation: the texel's point is found in the last frame by its motion; where the history's depth there is the point's
//   previous distance (within 5 %) the estimate is averaged in (1 / n, n up to HISTORY), else the history starts again. With
//   MOVING_CAP (the scene's smooth_occlusion) a texel within the occlusion's reach of a shape that moved this frame keeps none
//   (MOVING_HISTORY 1): the occlusion a moving shape casts moves with it, which the depth test does not see, and a history on the
//   ground it passes trailed it in bands (even three frames of it at a car's speed); the spatial filter after this pass keeps
//   the single frame's estimate smooth.
//
// Writes r: the occlusion (1 none), g: the pixel's eye distance (the forward pass's upsampling by depth), b: the frames averaged.

@group(0) @binding(0) var<uniform> frame: Frame;
@group(0) @binding(1) var depth: texture_depth_2d;
@group(0) @binding(2) var normals: texture_2d<f32>;
@group(0) @binding(3) var motion_tex: texture_2d<f32>;
@group(0) @binding(4) var history: texture_2d<f32>;

const SLICES: u32 = 2u;
const STEPS: u32 = 6u;
const TAPS: u32 = 8u;
const HISTORY: f32 = 16.0;
const MOVING_HISTORY: f32 = 1.0;
// The share of the reach over which a sample's weight fades out.
const FALLOFF: f32 = 0.1;
// The reach's largest screen size (internal pixels) and the closest a sample is taken (pixels).
const MAX_RADIUS_PX: f32 = 192.0;
const MIN_STEP_PX: f32 = 1.3;
const HALF_PI: f32 = 1.5707963;

@vertex
fn vs_full(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let x = f32((i << 1u) & 2u);
    let y = f32(i & 2u);
    return vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
}

fn size() -> vec2<i32> {
    return vec2<i32>(textureDimensions(depth));
}

// The view-space point at pixel position `px` with reversed depth `d`.
fn view_point(px: vec2<f32>, d: f32) -> vec3<f32> {
    let ndc = vec2<f32>(px.x * frame.screen.z * 2.0 - 1.0, 1.0 - px.y * frame.screen.w * 2.0);
    let p = frame.inv_proj * vec4<f32>(ndc, d, 1.0);
    return p.xyz / p.w;
}

fn safe_acos(x: f32) -> f32 {
    return acos(clamp(x, -1.0, 1.0));
}

// The view-space point at the sample position `sp` (pixels), or w = 0 when there is none (off the frame or the sky).
fn sample_point(sp: vec2<f32>) -> vec4<f32> {
    let s = size();
    let q = vec2<i32>(floor(sp));
    if any(q < vec2<i32>(0)) || any(q >= s) {
        return vec4<f32>(0.0);
    }
    let d = textureLoad(depth, q, 0);
    if d <= 0.0 {
        return vec4<f32>(0.0);
    }
    return vec4<f32>(view_point(vec2<f32>(q) + 0.5, d), 1.0);
}

fn gtao(centre: vec2<f32>, p: vec3<f32>, n: vec3<f32>, noise: vec2<f32>, radius_px: f32) -> f32 {
    let v = normalize(-p);
    let reach = frame.ao.x;
    var vis = 0.0;
    for (var s = 0u; s < SLICES; s = s + 1u) {
        let phi = (f32(s) + noise.x) * PI / f32(SLICES);
        let dir = vec3<f32>(cos(phi), sin(phi), 0.0);
        // The slice's direction on the screen (y down).
        let omega = vec2<f32>(dir.x, -dir.y);
        let ortho = dir - dot(dir, v) * v;
        let axis = normalize(cross(ortho, v));
        let pn = n - axis * dot(n, axis);
        let pn_len = length(pn);
        let cos_n = clamp(dot(pn, v) / max(pn_len, 1e-6), 0.0, 1.0);
        let ang_n = sign(dot(ortho, pn)) * safe_acos(cos_n);
        let low_pos = cos(ang_n + HALF_PI);
        let low_neg = cos(ang_n - HALF_PI);
        var h_pos = low_pos;
        var h_neg = low_neg;
        for (var j = 0u; j < STEPS; j = j + 1u) {
            let t = (f32(j) + noise.y) / f32(STEPS);
            let dist = (t * t + MIN_STEP_PX / radius_px) * radius_px;
            for (var side = 0u; side < 2u; side = side + 1u) {
                let sgn = select(-1.0, 1.0, side == 0u);
                let q = sample_point(centre + omega * (dist * sgn));
                if q.w == 0.0 {
                    continue;
                }
                let delta = q.xyz - p;
                let dl = length(delta);
                if dl < 1e-5 {
                    continue;
                }
                let c = dot(delta / dl, v);
                let w = clamp((reach - dl) / (FALLOFF * reach), 0.0, 1.0);
                if side == 0u {
                    h_pos = max(h_pos, mix(low_pos, c, w));
                } else {
                    h_neg = max(h_neg, mix(low_neg, c, w));
                }
            }
        }
        // The horizons' angles from the view vector (positive toward the slice's direction), within the normal's hemisphere.
        let h0 = ang_n + clamp(-safe_acos(h_neg) - ang_n, -HALF_PI, HALF_PI);
        let h1 = ang_n + clamp(safe_acos(h_pos) - ang_n, -HALF_PI, HALF_PI);
        let sn = sin(ang_n);
        let arc0 = (cos_n + 2.0 * h0 * sn - cos(2.0 * h0 - ang_n)) * 0.25;
        let arc1 = (cos_n + 2.0 * h1 * sn - cos(2.0 * h1 - ang_n)) * 0.25;
        vis = vis + pn_len * (arc0 + arc1);
    }
    return clamp(vis / f32(SLICES), 0.0, 1.0);
}

fn ssao(p: vec3<f32>, n: vec3<f32>, noise: vec2<f32>) -> f32 {
    let reach = frame.ao.x;
    let helper = select(vec3<f32>(0.0, 0.0, 1.0), vec3<f32>(1.0, 0.0, 0.0), abs(n.z) > 0.9);
    let t = normalize(cross(helper, n));
    let b = cross(n, t);
    let s = size();
    var occ = 0.0;
    for (var k = 0u; k < TAPS; k = k + 1u) {
        let u = (f32(k) + 0.5) / f32(TAPS);
        let a = f32(k) * 2.39996323 + noise.x * 6.2831853;
        let r = sqrt(u);
        let h = vec3<f32>(r * cos(a), r * sin(a), sqrt(max(1.0 - u, 0.0)));
        let l = fract(u + noise.y);
        let len = reach * mix(0.1, 1.0, l * l);
        let sp = p + (t * h.x + b * h.y + n * h.z) * len;
        let clip = frame.proj * vec4<f32>(sp, 1.0);
        if clip.w <= 1e-6 {
            continue;
        }
        let ndc = clip.xy / clip.w;
        let px = vec2<f32>((ndc.x + 1.0) * 0.5, (1.0 - ndc.y) * 0.5) * frame.screen.xy;
        let q = sample_point(px);
        if q.w == 0.0 {
            continue;
        }
        let bias = 0.002 + 0.001 * (-p.z);
        if q.z >= sp.z + bias {
            // Only a surface within the reach of the point occludes it (no halo from what lies far in front).
            let dq = length(q.xyz - p);
            occ = occ + clamp((reach - dq) / (FALLOFF * reach), 0.0, 1.0);
        }
    }
    return 1.0 - occ / f32(TAPS);
}

// Whether the world point `w` is within the occlusion's reach of a shape that moved this frame (`frame.movers`, the particles'
// negative radii aside).
fn near_mover(w: vec3<f32>) -> bool {
    let n = min(u32(max(frame.sky_shift.w, 0.0)), MOVING_CASTERS);
    for (var i = 0u; i < n; i = i + 1u) {
        let c = frame.movers[i];
        let r = c.w + frame.ao.x;
        let v = c.xyz - w;
        if c.w > 0.0 && dot(v, v) < r * r {
            return true;
        }
    }
    return false;
}

@fragment
fn fs_ao(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let scale = i32(max(frame.ao.w, 1.0));
    let rep = clamp(vec2<i32>(frag.xy) * scale, vec2<i32>(0), size() - 1);
    let d = textureLoad(depth, rep, 0);
    if d <= 0.0 {
        return vec4<f32>(1.0, 0.0, 0.0, 1.0);
    }
    let centre = vec2<f32>(rep) + 0.5;
    let p = view_point(centre, d);
    let nw = oct_decode(textureLoad(normals, rep, 0).xy);
    let n = normalize((frame.view * vec4<f32>(nw, 0.0)).xyz);
    let f = f32(u32(frame.temporal.z) % 64u);
    let noise = vec2<f32>(ign(centre + 5.588238 * f), ign(centre.yx * 1.31 + vec2<f32>(11.0, 7.0) + 5.588238 * f));
    let lin = -p.z;
    var raw = 1.0;
    let radius_px = frame.ao.x * frame.proj[0][0] * 0.5 * frame.screen.x / max(lin, 1e-4);
    if radius_px >= 1.0 {
        if GTAO {
            raw = gtao(centre, p, n, noise, min(radius_px, MAX_RADIUS_PX));
        } else {
            raw = ssao(p, n, noise);
        }
    }
    // The history.
    var count = 1.0;
    var ao = raw;
    if frame.flags.x > 0.5 {
        let m = textureLoad(motion_tex, rep, 0);
        let prev = (centre * frame.screen.zw - m.xy) * frame.screen.xy;
        let hp = vec2<i32>(floor(prev / f32(scale)));
        let hs = vec2<i32>(textureDimensions(history));
        if m.w > 0.5 && all(prev >= vec2<f32>(0.0)) && all(hp < hs) {
            let h = textureLoad(history, hp, 0);
            if h.b > 0.0 && abs(h.g - m.z) <= 0.05 * m.z {
                var cap = HISTORY;
                if MOVING_CAP {
                    let ndc = vec2<f32>(centre.x * frame.screen.z * 2.0 - 1.0, 1.0 - centre.y * frame.screen.w * 2.0);
                    let wp = frame.inv_unjit_view_proj * vec4<f32>(ndc, d, 1.0);
                    if near_mover(wp.xyz / wp.w) {
                        cap = MOVING_HISTORY;
                    }
                }
                count = min(h.b + 1.0, cap);
                ao = mix(h.r, raw, 1.0 / count);
            }
        }
    }
    return vec4<f32>(ao, lin, count, 1.0);
}
