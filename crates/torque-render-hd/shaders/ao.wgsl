// Ambient occlusion: one pass at the AO's resolution (half the internal frame's at High, the whole at Ultra) that estimates
// the occlusion at each AO texel's pixel (the top left of its block) from the depth prepass and its normals. Each frame's estimate
// stands alone: its noise is fixed per AO texel (the same every frame) and the spatial filter after this pass (`ao_filter.wgsl`)
// averages it out, so nothing lags behind a moving shape, a moving light or a moving camera.
//
// The noise is interleaved (Keller and Heidrich 2001): each texel of a 4 x 4 tile takes one of 16 turns of the directions and one
// of 16 offsets of the steps (an ordered-dither matrix, so neighbors differ most), all 16 moved by a share of a sixteenth hashed
// from the tile, and the filter weighs every texel of a tile alike, so on a flat patch the tile's 16 estimates average to one
// estimate of 16 times the directions and steps. (A noise without that period, such as interleaved gradient noise taken at the
// AO texels, leaves what the filter does not average as fixed stripes that wave across flat walls; the same 16 turns in every
// tile leave, where the occlusion changes, a faint crosshatch of the tile's period.)
//
// - GTAO (GTAO = true; Jimenez et al. 2016, "Practical Realtime Strategies for Accurate Indirect Occlusion"): in each of SLICES
//   screen-space directions (turned per texel by the interleaved noise) the highest horizon on either side is found from STEPS
//   depth samples a side within the reach (a sample's horizon fades to the tangent plane's from 3/4 of the reach to 5/4 of it, so
//   what lies farther in depth, such as the background around an object's outline, occludes nothing, and the steps crossing the
//   reach leave no contour of their own: a fade over its last tenth drew bands along the edges of what occludes), clamped to the
//   hemisphere of the normal projected into the slice, and the cosine-weighted visible arc integrated in closed form. The result
//   is the cosine-weighted visibility of rays of about the reach's length against the depth buffer taken as a height field.
// - SSAO (GTAO = false; a normal-oriented hemisphere after Mittring 2007 and Bavoil and Sainz 2008): TAPS points in the hemisphere
//   (cosine-distributed, turned per texel), each occluded where the depth buffer holds a surface in front of it that lies within the
//   reach of the pixel's point.
//
// Writes r: the occlusion (1 none), g: the pixel's eye distance (the forward pass's upsampling by depth), b: 1.

@group(0) @binding(0) var<uniform> frame: Frame;
@group(0) @binding(1) var depth: texture_depth_2d;
@group(0) @binding(2) var normals: texture_2d<f32>;
// (Bindings 3 and 4, the motion and last frame's AO, are the layout's; this pass reads neither.)

const SLICES: u32 = 3u;
const STEPS: u32 = 6u;
const TAPS: u32 = 8u;
// The share of the reach over which a sample's weight fades out, centered on the reach (full to 3/4 of it, none past 5/4).
const FALLOFF: f32 = 0.5;
// The reach's largest screen size (internal pixels) and the closest a sample is taken (pixels).
const MAX_RADIUS_PX: f32 = 192.0;
const MIN_STEP_PX: f32 = 1.3;
const HALF_PI: f32 = 1.5707963;

fn size() -> vec2<i32> {
    return vec2<i32>(textureDimensions(depth));
}

// A hash of an integer pair to [0, 1) (PCG's output permutation).
fn hash2(v: vec2<u32>) -> f32 {
    var h = v.x * 747796405u + v.y * 2891336453u + 2654435769u;
    h = ((h >> ((h >> 28u) + 4u)) ^ h) * 277803737u;
    h = (h >> 22u) ^ h;
    return f32(h >> 8u) / 16777216.0;
}

// The interleaved noise of AO texel `t`: x the turn of the directions, y the offset of the steps, each one of 16 evenly spaced
// values in (0, 1) that every 4 x 4 tile takes once (the offset's order a permutation of the turn's, so the two do not move
// together), all moved by the tile's own share of a sixteenth (neighboring tiles do not repeat each other's directions).
fn interleaved(t: vec2<i32>) -> vec2<f32> {
    let k = bayer(vec2<u32>(t));
    let tile = vec2<u32>(t) >> vec2<u32>(2u);
    let turn = vec2<f32>(hash2(tile), hash2(tile + vec2<u32>(7919u, 104729u)));
    return fract((vec2<f32>(f32(k), f32((k * 5u + 3u) & 15u)) + 0.5 + turn) / 16.0);
}

// A sample's weight at distance `dl` from the point: 1 within 3/4 of the reach, falling to none at 5/4 of it.
fn fade(dl: f32, reach: f32) -> f32 {
    return clamp((reach * (1.0 + 0.5 * FALLOFF) - dl) / (FALLOFF * reach), 0.0, 1.0);
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
                let w = fade(dl, reach);
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
            occ = occ + fade(dq, reach);
        }
    }
    return 1.0 - occ / f32(TAPS);
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
    let noise = interleaved(vec2<i32>(frag.xy));
    let lin = -p.z;
    var raw = 1.0;
    let radius_px = frame.ao.x * (1.0 + 0.5 * FALLOFF) * frame.proj[0][0] * 0.5 * frame.screen.x / max(lin, 1e-4);
    if radius_px >= 1.0 {
        if GTAO {
            raw = gtao(centre, p, n, noise, min(radius_px, MAX_RADIUS_PX));
        } else {
            raw = ssao(p, n, noise);
        }
    }
    return vec4<f32>(raw, lin, 1.0, 1.0);
}
