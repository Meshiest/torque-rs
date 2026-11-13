// The probes' irradiance at a point (after gi_common.wgsl and gi_grid.wgsl, with `gi_state`, `gi_atlas` and `gi_samp` bound by
// the module): the
// finest cascade holding the point, its eight probes about it weighted by their distance (trilinear), by how far the normal
// turns from each (a probe behind the surface counts little) and by each one's depth moments seen from the point's side
// (Chebyshev's bound: a probe beyond a wall, which saw the wall nearer than the point, counts next to nothing), each probe's
// octahedral irradiance sampled at the normal; the outer two probes of a cascade cross-faded to the next, the last's to
// `fallback` (the sky's irradiance).

// A direction's place in a probe's octahedral tile of `inner` texels a side (and a texel of border), as atlas coordinates.
fn gi_oct_uv(tile: vec2<i32>, size: i32, inner: f32, row0: f32, d: vec3<f32>) -> vec2<f32> {
    let e = gi_oct(d) * 0.5 + 0.5;
    let texel = vec2<f32>(tile * size) + vec2<f32>(1.0, 1.0 + row0) + e * inner;
    return texel * gi_state.probes.atlas.xy;
}

// One cascade's irradiance at `p` with normal `n`, seen along `v` (toward the eye: the point is looked up a little toward it as
// well as off the surface, so a probe just behind a grazing surface is not the one its Chebyshev test is asked about) (rgb),
// and its weight (a: 0 outside it, 1 inside but for its outer probes).
fn gi_cascade(cascade: u32, p: vec3<f32>, n: vec3<f32>, v: vec3<f32>) -> vec4<f32> {
    let c = gi_state.probes.cascades[cascade];
    let h = c.spacing.x;
    let q = p + (n * 0.3 + v * 0.1) * h;
    let local = q * c.spacing.y - GI_PROBE_SHIFT - vec3<f32>(c.origin.xyz);
    let g = vec3<f32>(f32(GI_PROBES_X), f32(GI_PROBES_Y), f32(GI_PROBES_Z));
    let inside = min(local, g - 1.0 - local);
    let edge = min(inside.x, min(inside.y, inside.z));
    if edge <= 0.0 {
        return vec4<f32>(0.0);
    }
    let fade = clamp(edge / 2.0, 0.0, 1.0);
    let base = vec3<i32>(floor(local));
    let f = local - vec3<f32>(base);
    var sum = vec3<f32>(0.0);
    var total = 0.0;
    for (var k = 0u; k < 8u; k = k + 1u) {
        let off = vec3<i32>(i32(k & 1u), i32((k >> 1u) & 1u), i32(k >> 2u));
        let coord = c.origin.xyz + base + off;
        let s = gi_probe_slot(coord);
        let probe = gi_state.probe[gi_probe_index(cascade, s)];
        if probe.w <= 0.0 {
            continue;
        }
        let at = (vec3<f32>(coord) + GI_PROBE_SHIFT) * h + probe.xyz;
        let to = at - q;
        let dist = max(length(to), 1e-4);
        let dir = to / dist;
        let tri = mix(1.0 - f, f, vec3<f32>(off));
        let wrap = (dot(dir, n) + 1.0) * 0.5;
        var w = wrap * wrap + 0.02;
        let tile = gi_probe_tile(cascade, s);
        let m = textureSampleLevel(gi_atlas, gi_samp, gi_oct_uv(tile, GI_DEPTH_TILE, GI_DEPTH_INNER, gi_state.probes.atlas.z, -dir), 0.0).xy;
        if dist > m.x {
            // A floor of a tenth of a spacing on the spread: a point just past what a probe saw fades, not cuts.
            let variance = max(abs(m.y - m.x * m.x), 0.01 * h * h);
            let d = dist - m.x;
            let cheb = variance / (variance + d * d);
            w = w * max(cheb * cheb * cheb, 0.0);
        }
        w = max(w, 1e-6) * probe.w;
        // The crush (a probe the surface barely sees counts far less) before the distance's weight: after it, the trilinear
        // blend between neighbors would turn into steps.
        if w < 0.2 {
            w = w * w * w / 0.04;
        }
        w = w * tri.x * tri.y * tri.z;
        let irr = textureSampleLevel(gi_atlas, gi_samp, gi_oct_uv(tile, GI_IRR_TILE, GI_IRR_INNER, 0.0, n), 0.0).rgb;
        sum = sum + irr * w;
        total = total + w;
    }
    if total <= 1e-9 {
        return vec4<f32>(0.0);
    }
    return vec4<f32>(sum / total, fade);
}

// The ambient's radiance (irradiance over pi) at `p` with normal `n` seen along `v` (toward the eye; the normal where there is
// no eye): the probes' where they reach, `fallback` past them.
fn gi_irradiance(p: vec3<f32>, n: vec3<f32>, v: vec3<f32>, fallback: vec3<f32>) -> vec3<f32> {
    var left = 1.0;
    var out = vec3<f32>(0.0);
    let count = gi_state.probes.grid.x;
    for (var c = 0u; c < count; c = c + 1u) {
        if left <= 0.0 {
            break;
        }
        let r = gi_cascade(c, p, n, v);
        let take = left * r.w;
        out = out + r.rgb * take;
        left = left - take;
    }
    return out + fallback * left;
}
