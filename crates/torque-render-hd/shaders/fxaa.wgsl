// The Low quality's anti-aliasing: our own edge filter in the manner of Lottes 2009's FXAA, on the internal HDR frame (the
// final pass then scales it to the output).
//
// Luma is taken of the tonemapped colour (perceptual: an edge between two bright HDR values is still an edge). Where the 3 x 3
// neighbourhood's contrast is under a threshold the pixel is left as it is. Otherwise the edge's orientation comes from the second
// differences across rows and columns, its side from the steeper of the two neighbours across it, and the pixel is resampled
// (bilinearly, half a pixel toward that side) by the larger of two amounts: the sub-pixel blend (how far the pixel stands from its
// neighbourhood's average: single-pixel features) and the edge blend (how near the pixel is to the nearer end of the edge's run, found
// by stepping along the edge up to SEARCH pixels each way).

@group(0) @binding(0) var hdr: texture_2d<f32>;
@group(0) @binding(1) var linear_clamp: sampler;

const EDGE_MIN: f32 = 0.0312;
const EDGE_REL: f32 = 0.125;
const SUBPIXEL: f32 = 0.75;
const SEARCH: u32 = 8u;

fn luma_of(c: vec3<f32>) -> f32 {
    let l = dot(max(c, vec3<f32>(0.0)), vec3<f32>(0.2126, 0.7152, 0.0722));
    return sqrt(l / (1.0 + l));
}

fn luma_at(p: vec2<i32>) -> f32 {
    let s = vec2<i32>(textureDimensions(hdr));
    return luma_of(textureLoad(hdr, clamp(p, vec2<i32>(0), s - 1), 0).rgb);
}

fn luma_uv(uv: vec2<f32>) -> f32 {
    return luma_of(textureSampleLevel(hdr, linear_clamp, uv, 0.0).rgb);
}

@fragment
fn fs_fxaa(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let p = vec2<i32>(frag.xy);
    let texel = 1.0 / vec2<f32>(textureDimensions(hdr));
    let centre = textureLoad(hdr, p, 0);
    let m = luma_of(centre.rgb);
    let n = luma_at(p + vec2<i32>(0, -1));
    let s = luma_at(p + vec2<i32>(0, 1));
    let e = luma_at(p + vec2<i32>(1, 0));
    let w = luma_at(p + vec2<i32>(-1, 0));
    let hi = max(max(max(n, s), max(e, w)), m);
    let lo = min(min(min(n, s), min(e, w)), m);
    let range = hi - lo;
    if range < max(EDGE_MIN, hi * EDGE_REL) {
        return centre;
    }
    let ne = luma_at(p + vec2<i32>(1, -1));
    let nw = luma_at(p + vec2<i32>(-1, -1));
    let se = luma_at(p + vec2<i32>(1, 1));
    let sw = luma_at(p + vec2<i32>(-1, 1));
    // The sub-pixel amount.
    let avg = (2.0 * (n + s + e + w) + ne + nw + se + sw) / 12.0;
    let sub0 = clamp(abs(avg - m) / range, 0.0, 1.0);
    let sub1 = smoothstep(0.0, 1.0, sub0);
    let sub = sub1 * sub1 * SUBPIXEL;
    // Orientation: a horizontal edge changes along y.
    let horiz = abs(n + s - 2.0 * m) * 2.0 + abs(ne + se - 2.0 * e) + abs(nw + sw - 2.0 * w);
    let vert = abs(e + w - 2.0 * m) * 2.0 + abs(ne + nw - 2.0 * n) + abs(se + sw - 2.0 * s);
    let horizontal = horiz >= vert;
    let l_neg = select(w, n, horizontal);
    let l_pos = select(e, s, horizontal);
    let g_neg = abs(l_neg - m);
    let g_pos = abs(l_pos - m);
    let toward_pos = g_pos >= g_neg;
    let gradient = max(g_neg, g_pos);
    let other = select(l_neg, l_pos, toward_pos);
    let edge_luma = 0.5 * (m + other);
    // The step across the edge (toward its steeper side) and along it.
    var across = select(vec2<f32>(texel.x, 0.0), vec2<f32>(0.0, texel.y), horizontal);
    if !toward_pos {
        across = -across;
    }
    let along = select(vec2<f32>(0.0, texel.y), vec2<f32>(texel.x, 0.0), horizontal);
    let uv = (vec2<f32>(p) + 0.5) * texel;
    let on_edge = uv + across * 0.5;
    let threshold = gradient * 0.25;
    var d_neg = 0.0;
    var d_pos = 0.0;
    var done_neg = false;
    var done_pos = false;
    var e_neg = 0.0;
    var e_pos = 0.0;
    for (var i = 1u; i <= SEARCH; i = i + 1u) {
        if !done_neg {
            e_neg = luma_uv(on_edge - along * f32(i)) - edge_luma;
            d_neg = f32(i);
            done_neg = abs(e_neg) >= threshold;
        }
        if !done_pos {
            e_pos = luma_uv(on_edge + along * f32(i)) - edge_luma;
            d_pos = f32(i);
            done_pos = abs(e_pos) >= threshold;
        }
        if done_neg && done_pos {
            break;
        }
    }
    // The nearer end decides: the pixel is blended only when the luma there turns the other way from the centre's.
    let nearer_neg = d_neg < d_pos;
    let end_delta = select(e_pos, e_neg, nearer_neg);
    let dist = min(d_neg, d_pos);
    let total = d_neg + d_pos;
    var edge_blend = 0.0;
    if (end_delta < 0.0) != (m - edge_luma < 0.0) {
        edge_blend = 0.5 - dist / total;
    }
    let amount = max(edge_blend, sub);
    return textureSampleLevel(hdr, linear_clamp, uv + across * amount, 0.0);
}
