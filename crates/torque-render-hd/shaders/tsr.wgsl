// The temporal upscaler's resolve (High and Ultra with the upscaler; after tsr_gather.wgsl, then tsr_half.wgsl or tsr_full.wgsl for the history's read).
//
// - The light's change, measured without the jitter: the renderer draws the same sample at a still pixel whenever the scene
//   and the jitter's phase are the same, so this frame's sample against the one of the same phase PHASES frames ago (the ring
//   keeps the last frames' luma) is the light's change over those frames alone, with none of the jitter's noise in it (a
//   test against a noise estimate missed a slow drift wherever the jitter's noise at an edge or over a busy texture was wider
//   than the lag it hid). The ring keeps the last frames' luma, a layer a frame. A history of n frames lags a light changing d
//   a frame by n d: the frames a still pixel's history may hold are LAG_TOLERANCE over the change a frame, between
//   CHANGE_FRAMES (a light changing fast: the history follows it as a moving image's does) and LONG_FRAMES (the light holds).
//   The change is known once the pixel has been still and kept for PHASES frames (the moments count them); until then the
//   history holds the long frames (the lag a change of light builds meanwhile is caught, and cut, when the ring knows).
// - The still pixel's luma is the mean of its own samples over the jitter's last period (tsr_gather.wgsl's `phase_of`: exact, from the
//   frames the pixel has been still, blended in as they fill the period; clipped into the box unless the light holds): no
//   ripple of the phases (an 8-frame running mean keeps a sixth of a flicker's swing, and at the reduced size an edge's swing is
//   wide), no lag past half the period, and none of the blur a history takes while it moves (the samples are this pixel's own).
//   The chroma, and a moving pixel, are the history's.
// - Two boxes. A pixel that moves, or whose light changes, is clipped into the 3 x 3's mean +- deviation weighted as the current
//   colour is (the samples nearest the pixel's centre count most): at a reduced internal size the plain 3 x 3 reaches far past
//   the output pixel, and its box let a change of light linger. A still pixel whose light holds is clipped into the plain
//   3 x 3's box, which the jitter's phases do not shake.
// - No decision reads the history itself, so a half-float history decides as a 32-bit one would (a test on the history's own
//   luma flips with its rounding, and the two then show different phases of a flicker).
// - A shape's part that drops in and out of the 3 x 3 with the jitter alone (nothing moved) neither caps the history nor, for
//   a sky pixel beside it, drops it: the TAA's object and sky tests are for parts that moved.
// - Half floats (HALF): the resolve rounds the history to a half float itself, up or down by a dither (a hash of the pixel
//   and the frame's index in its history: the same frames on every run) so that on average it rounds to the value itself. The
//   running mean multiplies any rounding error that leans one way by the frames it holds: rounded to the nearest, a step under
//   half a spacing is lost every frame; the target's own conversion may round toward zero; and a dither that follows the
//   jitter's phases leans with them.
//
// Writes the history (rgb, frames held; in half floats it is the shown colour too, the tonemap's source), the moments (the
// phase mean's sum and frames, and the 3 x 3's mean luma for the next frame's change test) and this frame's luma into the ring's
// slot of the frame; the 32-bit variant writes the shown colour besides (`emit`, in tsr_half.wgsl and tsr_full.wgsl).

// The most frames a still pixel's history holds where its light holds.
const LONG_FRAMES: f32 = 32.0;
// The lag of light (compressed luma) a history may hold: the frames it holds times the light's change a frame stays under it.
// Under the ghosting check's tolerance (8 of 255 levels, about 0.01 at mid gray) with room for the lag to build up.
const LAG_TOLERANCE: f32 = 0.004;
// The fewest frames a still pixel's history holds under a change of light (fewer would show the jitter's aliasing).
const CHANGE_FRAMES: f32 = 4.0;
// The least half width of the clip's box (compressed YCoCg): the chroma's a few half-float spacings at mid gray, the luma's two
// spacings of the brightest values (the dither's rounding stays inside the box; a wider floor let a flat patch's history lag
// a light changing on it by as much, over a level).
const BOX_FLOOR: f32 = 0.002;
const LUMA_FLOOR: f32 = 0.0005;

// A history started again at this frame's sample, its moments with it.
fn fresh(t: Taa) -> TaaOut {
    let x = to_ycocg(t.cur).x;
    return emit(vec4<f32>(expand(t.cur), 1.0), t.level, vec4<f32>(split(x), 1.0, t.level), x);
}

// The spacing of the half floats about `v` (the normal ones from 2^-14 up).
fn half_step(v: f32) -> f32 {
    // The exponent's bits (the sign's masked off: an abs inside the bitcast breaks the D3D compiler).
    let e = i32((bitcast<u32>(v) >> 23u) & 0xffu) - 127;
    return exp2(f32(max(e, -14) - 10));
}

// A value in [0, 1) from the pixel `p` and the frame `k` (the PCG hash: no pattern in space or time for the jitter to lean on).
fn dither(p: vec2<u32>, k: u32) -> f32 {
    var h = p.x * 1664525u + p.y * 1013904223u + k * 2654435769u;
    h = h * 747796405u + 2891336453u;
    h = ((h >> ((h >> 28u) + 4u)) ^ h) * 277803737u;
    h = (h >> 22u) ^ h;
    return f32(h >> 8u) / 16777216.0;
}

// The history's next value `v` as the half float it is stored as: rounded down or up by the dither `u` in [0, 1), exactly (a
// power-of-two scale, a floor and the scale back), so its mean over the frames is `v`.
fn stored(v: vec3<f32>, u: f32) -> vec3<f32> {
    if !HALF {
        return v;
    }
    let x = max(v, vec3<f32>(0.0));
    let step = vec3<f32>(half_step(x.x), half_step(x.y), half_step(x.z));
    return floor(x / step + u) * step;
}

@fragment
fn fs_taa(@builtin(position) frag: vec4<f32>) -> TaaOut {
    var t = gather(frag);
    t.gone = t.gone && t.moved_part;
    t.left = t.left && t.moved_part;
    if t.sky_dropped && !t.moved_part {
        t.trust = 1.0;
    }
    let cur = t.cur;
    let level = t.level;
    let x = to_ycocg(cur).x;
    if !t.usable {
        return fresh(t);
    }
    let h = tsr_history(t.prev_uv);
    var n = h.a * t.trust;
    if n < 0.5 {
        return fresh(t);
    }
    let speed = speed_of(t);
    let still = exp(-speed / SLOW_PIXELS);
    let gamma = mix(GAMMA_MOVING, GAMMA_STILL, still);
    let hy = to_ycocg(compress(h.rgb));
    // The moments at the history's texel, and the pixel's own luma over the jitter's period.
    let size = vec2<i32>(frame.output.xy);
    let qh = clamp(vec2<i32>(floor(t.prev_uv * frame.output.xy)), vec2<i32>(0), size - 1);
    let mom = textureLoad(moments_before, qh, 0);
    let kept = is_kept(t, still);
    let k = u32(frame.temporal.z);
    let q = vec2<i32>(frag.xy);
    let phase = phase_of(x, mom, kept, q);
    // The light's change a frame at this pixel, from the sample of the same jitter phase (known once the pixel has been still
    // that long), and the frames a still history may hold under it. Before it is known the history holds the long frames: a
    // pixel just come to rest needs them against the jitter's ripple (its 8-frame mean at the reduced size is off at edges),
    // the blur its history took while moving is clipped out, and a change of light meanwhile is caught when the ring knows.
    var still_cap = LONG_FRAMES;
    if phase.known {
        let drift = phase.change / f32(PHASES);
        still_cap = clamp(LAG_TOLERANCE / max(drift, 1e-6), CHANGE_FRAMES, LONG_FRAMES);
    }
    let steady = phase.steady;
    var mean = t.near1;
    var sd = sqrt(max(t.near2 - mean * mean, vec3<f32>(0.0)));
    if steady {
        mean = t.sum1 / 9.0;
        sd = sqrt(max(t.sum2 / 9.0 - mean * mean, vec3<f32>(0.0)));
    }
    let blo = max(t.lo, mean - gamma * sd);
    let bhi = min(t.hi, mean + gamma * sd);
    // A static pixel a shape's part has left: clipped close about the neighborhood's middle. The box is never thinner than
    // the floors a side: a gray patch's chroma box is all but flat, and a half float's rounding of the history's chroma would
    // leave it, and the clip would pull the luma along.
    let left_static = t.left && t.m.w <= 1.5;
    let extent = max(0.5 * (bhi - blo), vec3<f32>(LUMA_FLOOR, BOX_FLOOR, BOX_FLOOR));
    var clipped = clip_box(hy, 0.5 * (blo + bhi), extent);
    if left_static {
        clipped = clip_box(hy, 0.5 * (blo + bhi), 2.0 * LEFT_BOX * extent);
    }
    // A history clipped far (a change of light, a ghost) holds fewer frames. Where the pixel's light holds (steady) the
    // history is not clipped at all: it is the running mean of a scene that does not change, and the box of a frame whose
    // jitter missed a thin pole would clip the pole away, frame after frame (the flicker the mean is there to end).
    let moved = abs(clipped.x - hy.x) / max(bhi.x - blo.x, 0.02);
    if steady {
        clipped = hy;
    } else {
        n = n * clamp(1.0 - moved, 0.0, 1.0);
    }
    clipped.x = phase_luma(phase, clipped.x, blo.x, bhi.x);
    let level_before = mom.w;
    let dropped = (!steady && light_changed(level, level_before)) || shadow_passed(t);
    if dropped {
        n = 0.0;
    }
    // The frames a history may hold: the still cap only for a pixel all but still.
    var cap = mix(MOVING_FRAMES, still_cap, exp(-speed / HISTORY_STILL_PIXELS));
    if t.gone || t.left {
        cap = OTHER_FRAMES;
    }
    n = min(n, cap);
    let alpha = 1.0 / (n + 1.0);
    let u = dither(vec2<u32>(q), k);
    let out = stored(expand(max(mix(from_ycocg(clipped), cur, alpha), vec3<f32>(0.0))), u);
    let moments = moments_out(phase, mom, x, level, kept && !dropped);
    return emit(vec4<f32>(out, min(n + 1.0, cap)), level, moments, x);
}
