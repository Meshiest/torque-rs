// Temporal anti-aliasing (after Karis 2014, "High Quality Temporal Supersampling", and Salvi 2016's variance clipping), at the
// output's resolution: with the internal frame at the same size it is TAA, smaller TAAU (the history keeps the output's detail).
//
// - Each frame is drawn with a sub-pixel jitter (Halton(2, 3), 8 phases). The current colour at an output pixel is rebuilt from the
//   3 x 3 internal pixels around it, each weighted by a Gaussian (close to Blackman-Harris) of its jittered sample's distance from the
//   output pixel's centre (the jitter taken out).
// - Colours are compared and blended in a reversible tonemapped space (c / (1 + luma)): the luminance-weighted resolve, so a bright
//   highlight does not outweigh its neighbours and flicker.
// - The history is found by the motion of the nearest surface among the 3 x 3 (the edges follow the surface in front); the sky
//   (nothing drawn) moves with the camera's turn. It is fetched through Keys' cubic (sharp: little blur from resampling).
// - Disocclusion: where the previous prepass depth around the history's position is all nearer than the point was (by 2 % and a
//   centimetre), the point was hidden then and the history is dropped; where the motion there differs from the point's by more than a
//   pixel or two, it is faded out (velocity rejection). A sky pixel whose history's 3 x 3 held a surface is dropped, and so
//   is a surface pixel whose history's 3 x 3 held none of its object (the motion target's object id).
// - A shape's motion is its vertices' own (their positions at the last frame: its animation as well as its pose), so a walking
//   Player's limbs are found where they were.
// - Stillness: s = exp(-speed / SLOW_PIXELS) (speed in output pixels a frame; 1 still, about 0.8 at a fifth of a pixel, under 0.15
//   from two pixels on) sets the clip's gamma, the frames the history may hold (MOVING_FRAMES to STILL_FRAMES) and whether the
//   temporal test below applies (s > 1/2). The sky's speed is how far its reprojection moved it. Its history is resampled by a
//   cubic that sharpens a little (a gain up to 1.03 a pass at some frequencies), so only a history held for a few frames may be
//   resampled each frame: one held for the still cap would grow a fine wave frame after frame. A kept sky pixel's history stays
//   within its neighbourhood's extremes grown by their spread.
// - Moving pixels: the history is clipped toward the neighbourhood's mean, into the box of its mean +- gamma standard deviations (in
//   YCoCg, at most the box of its extremes).
// - Slow and still pixels: a feature thinner than a pixel is in the 3 x 3 in some jitter phases and not in others, and clipping to the
//   neighbourhood of a phase without it would flicker. So such a pixel also keeps its samples' luma variance over time (averaged as
//   the colour is; reset on a fast pixel, whose samples are not one point's), and its history is clipped only when the current sample
//   leaves the history's mean by more than three of its temporal deviations and a small margin: a real change (light, shadow,
//   something new, a ghost) is caught the frame it happens, the phases' own variation never is.
// - The blend is 1 / (n + 1) with n the frames the history holds (its alpha): a still image converges to the average of every phase.
// - A moving shape's shadow on still ground: the penumbra's slow change, a little every frame, lagged the history and grew the
//   temporal variance with the lag, so the temporal test never caught it and the old shading trailed. A point within the shadow's
//   reach of a shape that moved this frame (the cylinder away from the sun behind its bound, both poses' in one) is always
//   clipped, holds MOVING_SHADOW_FRAMES at most and starts its variance again.
// - What is shown: a pixel whose history was kept shows a short average of its shown colour (1/4 a frame, the last one fetched
//   through the history's cubic: a bilinear fetch, averaged frame after frame, blurred a walking camera's image), which removes the
//   running mean's wobble over the jitter's cycle (a slowly waving surface's too); anything else shows its history as it is.
// - Left pixels: a static surface where a shape's part was around the history's texel last frame and is nowhere in the 3 x 3 now
//   (a swing's last place: the outline's pixels held a share of it though their texel never was its) is tested on its colour as
//   well as its luma (a part may differ from the surface in chroma alone); a change drops its variance and its shown average and
//   clips it into a small box about the neighbourhood's middle. (Not untested: the edge of a still, idle shape comes and goes
//   from the 3 x 3 with the jitter.)
// - Seen-through pixels: where the line of sight crosses a see-through shape that moved, a particle, or the bound of a shape (or
//   a part: an item put away) the last upload had and this one lacks, and the same of last frame, the history is clipped close
//   around the current sample and holds one frame (their colour is blended over a surface with no motion of theirs, or is gone).
// - Water: where the line of sight crosses the water drawn as water (its waves, the reflection and the refraction change every
//   frame with no motion of their own: a history kept by the change test lagged them and smeared), the history is always clipped
//   into the neighbourhood's box, holds WATER_FRAMES at most and starts its variance again. Only while the effect time moves (the
//   waves with it): water whose time stands still is a static surface, and its history averages the jitter as any other's.
//
// Writes the history (rgb, frames held; 32-bit float: the running mean's small steps are not lost to half-float rounding) and the
// shown colour with the luma's temporal variance.

@group(0) @binding(0) var<uniform> frame: Frame;
@group(0) @binding(1) var current: texture_2d<f32>;
@group(0) @binding(2) var depth_now: texture_depth_2d;
@group(0) @binding(3) var depth_before: texture_depth_2d;
@group(0) @binding(4) var motion_now: texture_2d<f32>;
@group(0) @binding(5) var motion_before: texture_2d<f32>;
@group(0) @binding(6) var history: texture_2d<f32>;
@group(0) @binding(7) var shown_before: texture_2d<f32>;
@group(0) @binding(8) var linear_clamp: sampler;


// The current sample's rebuilding filter: exp(-k d^2) of a jittered sample's distance d (pixels) from the pixel's centre (Karis's
// fit of a Blackman-Harris window; narrower ones leave edges of moving shapes behind longer).
const RECONSTRUCTION: f32 = 2.29;
const STILL_FRAMES: f32 = 64.0;
const MOVING_FRAMES: f32 = 5.0;
const GAMMA_MOVING: f32 = 1.0;
const GAMMA_STILL: f32 = 1.5;
// The temporal change test: deviations and margin (compressed luma).
const CHANGE_DEVIATIONS: f32 = 3.0;
const CHANGE_MARGIN: f32 = 0.004;
// The shown colour's average of a kept history.
const SHOWN_BLEND: f32 = 0.25;
// The frames a history may hold where its texel was another object's or part's (what a limb passed over).
const OTHER_FRAMES: f32 = 3.0;
// The speed (output pixels a frame) under which a shape's pixel counts as standing still for the temporal test.
const SHAPE_STILL_PIXELS: f32 = 0.01;
// The speed (output pixels a frame) over which the stillness decays.
const SLOW_PIXELS: f32 = 1.0;
// The speed (output pixels a frame) over which a history's cap falls from the still frames to the moving ones.
const HISTORY_STILL_PIXELS: f32 = 0.05;
// The sky's reprojection shift (output pixels) under which it counts as not moved at all.
const SKY_STILL_PIXELS: f32 = 0.001;
// The least spread (compressed YCoCg) a kept sky pixel's history may stray past its neighbourhood's extremes.
const SKY_KEPT_FLOOR: f32 = 0.01;
// The moving shapes' shadows: the frames such a point's history holds, and the shadow's reach past a shape's bound (a share of
// its radius and world units: the penumbra).
const MOVING_SHADOW_FRAMES: f32 = 1.0;
// The frames a moving shape's own pixel holds within that reach (its self-shadow; clipped into its neighbourhood as ever).
const SELF_SHADOW_FRAMES: f32 = 2.0;
const MOVING_SHADOW_GROWTH: f32 = 1.25;
const MOVING_SHADOW_MARGIN: f32 = 0.25;
// The half size of the box such a history is clipped into around the current sample (a share of the neighbourhood's).
const MOVING_SHADOW_BOX: f32 = 0.25;
// How far past a moving translucent shape's or a particle's bound its pixels are taken as its (world units), the box (a share of
// the neighbourhood's) such a pixel's history is clipped into around the current sample, and the frames it holds.
const TRANSLUCENT_MARGIN: f32 = 0.1;
const SEEN_THROUGH_BOX: f32 = 0.1;
const SEEN_THROUGH_FRAMES: f32 = 1.0;
// The box (a share of the neighbourhood's) around the neighbourhood's middle a static pixel's history is clipped into where a
// shape's part has just left.
const LEFT_BOX: f32 = 0.25;
// The frames a history may hold where the view crosses the water.
const WATER_FRAMES: f32 = 3.0;

// Whether the world point `p` is within the shadow's reach of a shape that moved this frame.
fn under_mover(p: vec3<f32>) -> bool {
    let n = min(u32(max(frame.sky_shift.w, 0.0)), MOVING_CASTERS);
    let l = normalize(frame.sun_dir.xyz);
    for (var i = 0u; i < n; i = i + 1u) {
        let c = frame.movers[i];
        let r = abs(c.w) * MOVING_SHADOW_GROWTH + MOVING_SHADOW_MARGIN;
        let v = c.xyz - p;
        let t = dot(v, l);
        if t > -r {
            let q = v - t * l;
            if dot(q, q) < r * r {
                return true;
            }
        }
    }
    return false;
}

// Whether the line of sight from the eye to the world point `p` passes through a translucent shape that moved this frame or a
// particle sprite (the movers whose radius is negative): their colour is blended over the surface behind, which has no motion of
// theirs, so a history kept there held their old places.
fn through_translucent(p: vec3<f32>) -> bool {
    let n = min(u32(max(frame.sky_shift.w, 0.0)), MOVING_CASTERS);
    let e = frame.eye.xyz;
    let d = p - e;
    let dd = max(dot(d, d), 1e-8);
    for (var i = 0u; i < n; i = i + 1u) {
        let c = frame.movers[i];
        if c.w < 0.0 {
            let t = clamp(dot(c.xyz - e, d) / dd, 0.0, 1.0);
            let q = e + t * d - c.xyz;
            let r = -c.w + TRANSLUCENT_MARGIN;
            if dot(q, q) < r * r {
                return true;
            }
        }
    }
    return false;
}

// Whether the line of sight from the eye toward `toward` (a point on it past the eye) crosses the water drawn as water before
// `toward`, or anywhere along it for the sky (`sky`: nothing drawn there, the water reaching the horizon under it), while its
// waves move (the effect time, `eye.w`, differs from last frame's, `times.x`).
fn through_water(toward: vec3<f32>, sky: bool) -> bool {
    if frame.water.y < 0.5 || frame.eye.w == frame.times.x {
        return false;
    }
    let h = frame.water.x;
    let e = frame.eye.z - h;
    let p = toward.z - h;
    if sky {
        return (e > 0.0 && p < e) || (e < 0.0 && p > e);
    }
    return e * p < 0.0;
}

struct TaaOut {
    @location(0) history: vec4<f32>,
    @location(1) shown: vec4<f32>,
};

@vertex
fn vs_full(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let x = f32((i << 1u) & 2u);
    let y = f32(i & 2u);
    return vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
}

fn to_ycocg(c: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(
        0.25 * c.r + 0.5 * c.g + 0.25 * c.b,
        0.5 * c.r - 0.5 * c.b,
        -0.25 * c.r + 0.5 * c.g - 0.25 * c.b,
    );
}

fn from_ycocg(c: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(c.x + c.y - c.z, c.x + c.z, c.x - c.y - c.z);
}

// The reversible tonemap (Karis) and its inverse.
fn compress(c: vec3<f32>) -> vec3<f32> {
    return c / (1.0 + luminance(c));
}

fn expand(c: vec3<f32>) -> vec3<f32> {
    return c / max(1.0 - luminance(c), 1e-4);
}

// Keys' cubic convolution weight at distance `x` (texels), its parameter HISTORY_CUBIC: sharper than Catmull-Rom's -0.5, so a history
// resampled every frame as the camera moves keeps more of its detail.
const HISTORY_CUBIC: f32 = -0.75;
fn keys(x: vec2<f32>) -> vec2<f32> {
    let a = HISTORY_CUBIC;
    let d = abs(x);
    let near = ((a + 2.0) * d - (a + 3.0)) * d * d + 1.0;
    let far = ((a * d - 5.0 * a) * d + 8.0 * a) * d - 4.0 * a;
    return select(select(vec2<f32>(0.0), far, d < vec2<f32>(2.0)), near, d <= vec2<f32>(1.0));
}

// The history at `uv` through a cubic filter (4 x 4 texels; the history is 32-bit float, not filterable): one texel where
// `uv` is a texel's centre (a still pixel's history is read exactly).
fn history_at(uv: vec2<f32>) -> vec4<f32> {
    return cubic_at(history, uv);
}

// A texture of the output's size at `uv` through the same cubic filter (the shown colour too: a bilinear fetch of it, averaged
// frame after frame, blurred a moving image).
fn cubic_at(t: texture_2d<f32>, uv: vec2<f32>) -> vec4<f32> {
    let size = vec2<i32>(frame.output.xy);
    let pos = uv * frame.output.xy - 0.5;
    let c = floor(pos);
    let f = pos - c;
    let ci = vec2<i32>(c);
    if all(abs(f) < vec2<f32>(1e-4)) {
        return textureLoad(t, clamp(ci, vec2<i32>(0), size - 1), 0);
    }
    // Keys' cubic with a = HISTORY_CUBIC (Catmull-Rom is -0.5): the taps at distances 1 + f, f, 1 - f and 2 - f.
    let w0 = keys(1.0 + f);
    let w1 = keys(f);
    let w2 = keys(1.0 - f);
    let w3 = keys(2.0 - f);
    let wx = array<f32, 4>(w0.x, w1.x, w2.x, w3.x);
    let wy = array<f32, 4>(w0.y, w1.y, w2.y, w3.y);
    var sum = vec4<f32>(0.0);
    for (var y = 0; y < 4; y = y + 1) {
        for (var x = 0; x < 4; x = x + 1) {
            let q = clamp(ci + vec2<i32>(x - 1, y - 1), vec2<i32>(0), size - 1);
            sum = sum + textureLoad(t, q, 0) * (wx[x] * wy[y]);
        }
    }
    return max(sum, vec4<f32>(0.0));
}

// Clip `h` toward `centre` into the box `centre +- extent` (Playdead's clip toward the box's centre).
fn clip_box(h: vec3<f32>, centre: vec3<f32>, extent: vec3<f32>) -> vec3<f32> {
    let d = h - centre;
    let e = max(extent, vec3<f32>(1e-5));
    let unit = abs(d / e);
    let m = max(unit.x, max(unit.y, unit.z));
    if m > 1.0 {
        return centre + d / m;
    }
    return h;
}

fn fresh(cur: vec3<f32>) -> TaaOut {
    var out: TaaOut;
    let c = expand(cur);
    let y = to_ycocg(cur).x;
    out.history = vec4<f32>(c, 1.0);
    out.shown = vec4<f32>(c, 0.0);
    return out;
}

@fragment
fn fs_taa(@builtin(position) frag: vec4<f32>) -> TaaOut {
    let uv = frag.xy * frame.output.zw;
    let isize = vec2<i32>(frame.screen.xy);
    let j = frame.temporal.xy;
    let pin = uv * frame.screen.xy;
    // The image is drawn moved by the jitter: pixel q's sample lies at q + 1/2 - j of the unjittered frame.
    let base = vec2<i32>(floor(pin + j));
    var sum = vec3<f32>(0.0);
    var wsum = 0.0;
    var sum1 = vec3<f32>(0.0);
    var sum2 = vec3<f32>(0.0);
    var lo = vec3<f32>(1e9);
    var hi = vec3<f32>(-1e9);
    var near_d = 0.0;
    var near_q = clamp(base, vec2<i32>(0), isize - 1);
    // The 3 x 3's object ids (the object test).
    var ids: array<f32, 9>;
    for (var k = 0u; k < 9u; k = k + 1u) {
        let o = vec2<i32>(i32(k % 3u) - 1, i32(k / 3u) - 1);
        let q = clamp(base + o, vec2<i32>(0), isize - 1);
        let t = compress(max(textureLoad(current, q, 0).rgb, vec3<f32>(0.0)));
        let d = vec2<f32>(q) + 0.5 - j - pin;
        let w = exp(-RECONSTRUCTION * dot(d, d));
        sum = sum + t * w;
        wsum = wsum + w;
        let y = to_ycocg(t);
        sum1 = sum1 + y;
        sum2 = sum2 + y * y;
        lo = min(lo, y);
        hi = max(hi, y);
        let z = textureLoad(depth_now, q, 0);
        if z > near_d {
            near_d = z;
            near_q = q;
        }
        ids[k] = textureLoad(motion_now, q, 0).w;
    }
    let cur = sum / max(wsum, 1e-6);
    if frame.temporal.w < 0.5 {
        return fresh(cur);
    }
    // The motion: the nearest surface's, or the sky's (the far plane's point under the two cameras).
    let m = textureLoad(motion_now, near_q, 0);
    var prev_uv = uv - m.xy;
    let sky = near_d <= 0.0 || m.w < 0.5;
    if sky {
        // The sky is at infinity: its direction under the previous camera (w 0: the camera's own move takes nothing from it; the
        // far plane's point moved by the walk smeared it).
        let ndc = vec2<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0);
        let wp = frame.inv_unjit_view_proj * vec4<f32>(ndc, 0.0, 1.0);
        let pc = frame.prev_view_proj * vec4<f32>(wp.xyz / wp.w - frame.eye.xyz, 0.0);
        let pn = pc.xy / max(pc.w, 1e-6);
        prev_uv = vec2<f32>(pn.x * 0.5 + 0.5, 0.5 - pn.y * 0.5);
        // A camera that did not turn reads its sky's history exactly (the matrices' rounding moved it a few thousandths of a
        // pixel, and every such fetch resamples it).
        if length((uv - prev_uv) * frame.output.xy) < SKY_STILL_PIXELS {
            prev_uv = uv;
        }
    }
    if any(prev_uv < vec2<f32>(0.0)) || any(prev_uv > vec2<f32>(1.0)) {
        return fresh(cur);
    }
    var trust = 1.0;
    // Whether the history's texel was another object's or part's than the surface here, and whether that one is gone from the
    // 3 x 3 (the object test below).
    var other = false;
    var gone = false;
    var left = false;
    if sky {
        // A pixel all sky now whose history's 3 x 3 held a surface last frame: its history may be that surface's (an edge pixel
        // follows the surface in front), and a pale one (a lamp's head) passes as a cloud in the clip's box, so it trailed over the
        // sky for many frames behind a lamp post walked past. Dropped.
        let qs = vec2<i32>(floor(prev_uv * frame.screen.xy));
        for (var k = 0u; k < 9u; k = k + 1u) {
            let q = clamp(qs + vec2<i32>(i32(k % 3u) - 1, i32(k / 3u) - 1), vec2<i32>(0), isize - 1);
            if textureLoad(depth_before, q, 0) > 0.0 {
                trust = 0.0;
            }
        }
    } else {
        // Disocclusion: the previous depths around the history's position against the point's previous distance.
        let pp = prev_uv * frame.screen.xy - 0.5;
        let b = vec2<i32>(floor(pp));
        var farthest = 0.0;
        for (var k = 0u; k < 4u; k = k + 1u) {
            let q = clamp(b + vec2<i32>(i32(k & 1u), i32(k >> 1u)), vec2<i32>(0), isize - 1);
            let dz = textureLoad(depth_before, q, 0);
            let lin = select(linear_depth(dz), 1e9, dz <= 0.0);
            farthest = max(farthest, lin);
        }
        if farthest < m.z * 0.98 - 0.01 {
            trust = 0.0;
        }
        let qn = clamp(vec2<i32>(floor(prev_uv * frame.screen.xy)), vec2<i32>(0), isize - 1);
        let mv_before = textureLoad(motion_before, qn, 0);
        if mv_before.w > 0.5 {
            let dv = length((m.xy - mv_before.xy) * frame.screen.xy);
            trust = trust * clamp(1.0 - (dv - 1.0) * 0.5, 0.0, 1.0);
        }
        // The object test (the motion's w: 1 for everything static, 1 + a shape's part's id; `ids` the 3 x 3's).
        // - A pixel inside a shape's part (its 3 x 3 all that part) whose history's 3 x 3 last frame held none of it: the part
        //   moved over something else (a limb swung across the torso): its history is dropped. (On an outline the texel found
        //   for the history may be a row off the jitter's, so the test is left to the clip there.)
        // - A history whose texel was another object's or part's that is gone from the 3 x 3 (what a swinging limb passed over
        //   or just left: at a high frame rate neither the depth test nor the velocity test tells them apart) holds a few
        //   frames at most, is always clipped and starts its temporal variance again (the variance the limb brought is not the
        //   pixel's own); a static pixel whose history's texel was a shape's part is always clipped too.
        other = abs(mv_before.w - m.w) > 0.5;
        gone = other;
        var inside = true;
        for (var k = 0u; k < 9u; k = k + 1u) {
            if abs(ids[k] - mv_before.w) < 0.5 {
                gone = false;
            }
            if abs(ids[k] - m.w) > 0.5 {
                inside = false;
            }
        }
        // A shape's part last frame around the history's texel that is in none of the 3 x 3 now (a part that swung on): an
        // outline's pixels held a share of it in their history though their own texel never was its.
        for (var k = 0u; k < 9u; k = k + 1u) {
            let q = clamp(qn + vec2<i32>(i32(k % 3u) - 1, i32(k / 3u) - 1), vec2<i32>(0), isize - 1);
            let wb = textureLoad(motion_before, q, 0).w;
            if wb > 1.5 && !left {
                var here = false;
                for (var j = 0u; j < 9u; j = j + 1u) {
                    if abs(ids[j] - wb) < 0.5 {
                        here = true;
                    }
                }
                if !here {
                    left = true;
                }
            }
        }
        // A static surface a shape's part has left: the history held the part, and over a busy surface the clip's box
        // spans its colour too. Dropped, as a disocclusion is.
        if m.w > 1.5 && inside {
            var same = false;
            for (var k = 0u; k < 9u; k = k + 1u) {
                let q = clamp(qn + vec2<i32>(i32(k % 3u) - 1, i32(k / 3u) - 1), vec2<i32>(0), isize - 1);
                if abs(textureLoad(motion_before, q, 0).w - m.w) < 0.5 {
                    same = true;
                }
            }
            if !same {
                trust = 0.0;
            }
        }
    }
    let h = history_at(prev_uv);
    var n = h.a * trust;
    if n < 0.5 {
        return fresh(cur);
    }
    let before = cubic_at(shown_before, prev_uv);
    // The sky's speed is its reprojection's (it has no motion of its own): a turning camera's sky moves, and a history held as
    // long as a still one's while it is resampled every frame by the sharpening cubic grew a fine wave over the clouds.
    let speed = select(length(m.xy * frame.output.xy), length((uv - prev_uv) * frame.output.xy), sky);
    // How still the pixel is (see the header).
    let still = exp(-speed / SLOW_PIXELS);
    let gamma = mix(GAMMA_MOVING, GAMMA_STILL, still);
    let mean = sum1 / 9.0;
    let sd = sqrt(max(sum2 / 9.0 - mean * mean, vec3<f32>(0.0)));
    let blo = max(lo, mean - gamma * sd);
    let bhi = min(hi, mean + gamma * sd);
    let hy = to_ycocg(compress(h.rgb));
    let cy = to_ycocg(cur);
    // The temporal change test of a still pixel.
    let t_sd = sqrt(max(before.a, 0.0));
    // A moving shape's pixel is always clipped (the temporal test keeps a still pixel's history while the jitter's phases vary
    // it; a limb moving under a pixel a frame at a high frame rate looks still to it, and its edge's history then trailed), as is
    // a static pixel whose history was a shape's.
    let shape_moving = m.w > 1.5 && speed > SHAPE_STILL_PIXELS;
    let ndc = vec2<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0);
    let wp = frame.inv_unjit_view_proj * vec4<f32>(ndc, near_d, 1.0);
    let mover_near = frame.sky_shift.w > 0.5 && under_mover(wp.xyz / wp.w);
    // A static receiver there, and a shape's pixel there (its self-shadow).
    let shaded = mover_near && m.w <= 1.5;
    let self_shaded = mover_near && m.w > 1.5;
    // A static pixel a shape's part has left (see the header), and what a moving see-through shape, a particle or a vanished shape
    // covers: only the current sample's colour is right.
    let left_static = left && m.w <= 1.5;
    let seen_through = frame.sky_shift.w > 0.5 && through_translucent(wp.xyz / wp.w);
    // The water: the surface's point, or for the sky a point on the ray past the near plane.
    var toward = wp.xyz / wp.w;
    if sky {
        let a = frame.inv_unjit_view_proj * vec4<f32>(ndc, 1.0, 1.0);
        let b = frame.inv_unjit_view_proj * vec4<f32>(ndc, 0.5, 1.0);
        toward = frame.eye.xyz + normalize(b.xyz / b.w - a.xyz / a.w);
    }
    let watery = through_water(toward, sky);
    let testable = !gone && !seen_through && !shape_moving && !mover_near && !(m.w <= 1.5 && other) && !watery;
    var kept = still > 0.5 && testable && abs(cy.x - hy.x) <= CHANGE_DEVIATIONS * t_sd + CHANGE_MARGIN;
    // Where a shape's part has just left a static pixel, its colour too must be the current sample's (a part's colour may
    // differ from the surface's in chroma alone).
    let dc = abs(cy - hy);
    let left_static_changed = left_static && max(dc.y, dc.z) > CHANGE_DEVIATIONS * t_sd + CHANGE_MARGIN;
    kept = kept && !left_static_changed;
    var clipped = hy;
    if seen_through {
        clipped = clip_box(hy, cy, SEEN_THROUGH_BOX * (bhi - blo));
    } else if shaded {
        // Around the current sample (the neighbourhood's box spans the penumbra's gradient and kept the old shading).
        clipped = clip_box(hy, cy, MOVING_SHADOW_BOX * (bhi - blo));
    } else if left_static && !kept {
        clipped = clip_box(hy, 0.5 * (blo + bhi), LEFT_BOX * (bhi - blo));
    } else if !kept {
        clipped = clip_box(hy, 0.5 * (blo + bhi), 0.5 * (bhi - blo));
    } else if sky {
        // A kept sky pixel stays within its neighbourhood's extremes grown by their spread (the sky has nothing thinner than a
        // pixel for the temporal test to keep: a history past them fed on itself).
        let spread = max(hi - lo, vec3<f32>(SKY_KEPT_FLOOR));
        clipped = clip_box(hy, 0.5 * (lo + hi), 0.5 * (hi - lo) + spread);
    }
    // A history clipped far (a change of light, a ghost) holds fewer frames.
    let moved = abs(clipped.x - hy.x) / max(bhi.x - blo.x, 0.02);
    n = n * clamp(1.0 - moved, 0.25, 1.0);
    // The frames a history may hold: the still cap only for a pixel all but still; one moving at all is resampled every frame and
    // each resample softens it, so it holds fewer (a walk's far pixels move a fraction of a pixel a frame and blurred).
    var cap = mix(MOVING_FRAMES, STILL_FRAMES, exp(-speed / HISTORY_STILL_PIXELS));
    if gone || (left && !kept) {
        cap = OTHER_FRAMES;
    }
    if seen_through {
        cap = min(cap, SEEN_THROUGH_FRAMES);
    }
    if watery {
        cap = min(cap, WATER_FRAMES);
    }
    if shaded {
        cap = min(cap, MOVING_SHADOW_FRAMES);
    }
    if self_shaded {
        cap = min(cap, SELF_SHADOW_FRAMES);
    }
    n = min(n, cap);
    let alpha = 1.0 / (n + 1.0);
    let out = mix(from_ycocg(clipped), cur, alpha);
    // The luma's variance over time (an exponential mean's, West 1979).
    let dy = cy.x - hy.x;
    // (Kept only for pixels slow enough for the change test: a fast one's samples are not one point's.)
    var variance = (1.0 - alpha) * (max(before.a, 0.0) + alpha * dy * dy);
    if still < 0.5 || !testable || left_static_changed {
        variance = 0.0;
    }
    var shown = out;
    // (Not where a part has just left: the shown colour's short average still held it.)
    if kept && !left_static {
        shown = mix(compress(max(before.rgb, vec3<f32>(0.0))), out, SHOWN_BLEND);
    }
    var o: TaaOut;
    o.history = vec4<f32>(expand(max(out, vec3<f32>(0.0))), min(n + 1.0, cap));
    o.shown = vec4<f32>(expand(max(shown, vec3<f32>(0.0))), variance);
    return o;
}
