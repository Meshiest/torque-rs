// The temporal upscaler's gather (upscale.rs's TemporalUpscaler; its blend is tsr.wgsl): taa.wgsl's reprojection and tests
// (after Karis 2014, "High Quality Temporal Supersampling", and Salvi 2016's variance clipping) at the output's resolution from
// the smaller internal frame, with the moments and the ring that make a still pixel's luma the mean of its own samples. The
// current sample, the history's place and the tests that drop it; taa.wgsl itself stays the TAA's (Medium, and High and Ultra
// without the upscaler).
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
// - Stillness: s = exp(-speed / SLOW_PIXELS) (speed in output pixels a frame; 1 still, under 0.15 from two pixels on) sets the
//   clip's gamma and the frames the history may hold (MOVING_FRAMES to STILL_FRAMES). The sky's speed is how far its
//   reprojection moved it (a turning camera's sky is resampled every frame, so it holds few frames).
// - The clip, everywhere and every frame: the history is clipped toward the neighborhood's middle, into the box of its mean +-
//   gamma standard deviations (in YCoCg, at most the box of its extremes). The history only ever averages the jitter's
//   phases of what the current frame shows: a change of light (a shadow sweeping past, the sun moving, a light switched) leaves
//   the box the frame it happens, whatever moved or did not. The lighting has no noise of its own from frame to frame (the
//   shadows', the occlusion's and the contact shadows' patterns are fixed per pixel and filtered in space), so the box only
//   spans edges and texture.
// - The sun's mask (the contact pass's) holds the cascades' visibility looked up at each pixel's unjittered place, filtered in
//   space: the same every frame for a still shadow, however hard its edge. Where it changed since last frame (read at the
//   history's place), a shadow's edge passed (the sun moved, a caster moved) and the history starts again: the box alone would
//   keep an old edge's colour inside it for the frames the history holds (a band a pixel wide lagging the edge).
// - The blend is 1 / (n + 1) with n the frames the history holds (its alpha), and a history clipped far holds fewer.
// - A still pixel's luma is the mean of its own samples over the jitter's last period (`phase_mean`, from the ring of the last
//   frames' luma), clipped into the box: a change of light (the sun moving, the sky's light with it) lags half the period at
//   most, where a history of 8 frames lagged it by 8 (the sun-fast row's trail).
// - Left pixels: a static surface where a shape's part was around the history's texel last frame and is nowhere in the 3 x 3 now
//   (a swing's last place: the outline's pixels held a share of it though their texel never was its) is clipped into a small
//   box about the neighborhood's middle and holds a few frames.
// - See-through shapes and particles have no motion of their own in the history: the clip catches them as any other change.

@group(0) @binding(0) var<uniform> frame: Frame;
@group(0) @binding(1) var current: texture_2d<f32>;
@group(0) @binding(2) var depth_now: texture_depth_2d;
@group(0) @binding(3) var depth_before: texture_depth_2d;
@group(0) @binding(4) var motion_now: texture_2d<f32>;
@group(0) @binding(5) var motion_before: texture_2d<f32>;
@group(0) @binding(6) var history: texture_2d<f32>;
@group(0) @binding(8) var linear_clamp: sampler;
// The sun's mask of this frame and the last (the internal size; 1 x 1 where the contact pass is off). A frame that drew no mask
// clears its copy: b 0, no test.
@group(0) @binding(9) var mask_now: texture_2d<f32>;
@group(0) @binding(10) var mask_before: texture_2d<f32>;
// Last frame's moments (the output's size): x and y the sum of the pixel's current luma over the frames it has been still, up to
// the jitter's period (split in two half floats, `split`), z those frames, w the 3 x 3's mean luma (the change test).
@group(0) @binding(11) var moments_before: texture_2d<f32>;
// The ring: the current luma of PHASES frames ago, the sample of this frame's jitter phase (the pass binds that layer beside the
// layer it writes).
@group(0) @binding(12) var ring: texture_2d<f32>;

// The jitter's period (the phases of the scene crate's Halton sequence): the ring keeps one layer more (upscale.rs's RING_FRAMES), so the
// layer written this frame is not the one read.
const PHASES: u32 = 8u;
// The stillness over which a pixel's moments are kept (else they start again), and the most frames they count it still.
const FLICKER_STILL: f32 = 0.9;
const STILL_COUNT: f32 = 64.0;
// The change of a still pixel's luma over the jitter's period (this frame's sample against the one of the same phase) under
// which its light holds (compressed luma: a quarter of a level).
const STEADY_CHANGE: f32 = 0.001;

// Whether a pixel's moments go on: all but still, and no shape's part gone from or left around it.
fn is_kept(t: Taa, still: f32) -> bool {
    return still > FLICKER_STILL && !t.gone && !t.left;
}

// A sum of up to PHASES compressed lumas in two half floats: the whole 256ths (exact in a half float up to 8) and the rest (a
// half float's 11 bits over a 256th): a running sum kept in one half float would walk off by its rounding every frame.
fn split(sum: f32) -> vec2<f32> {
    let hi = floor(sum * 256.0) / 256.0;
    return vec2<f32>(hi, sum - hi);
}

// A still pixel's luma without the jitter: the renderer draws the same sample at a still pixel whenever the scene and the
// jitter's phase are the same, so the mean of its own samples over the jitter's period is the resolved luma itself, exact (an
// 8-frame running mean keeps a sixth of a flicker's swing, and a change of light lags it by the frames it holds, while this
// mean lags by half the period); and the samples are this pixel's own, with none of the blur a history takes while it moves.
struct Phase {
    // The mean over the frames the pixel has been still, up to the period, and the share of the period they fill (0 where
    // the pixel is not kept): the resolve blends the mean in by that share.
    mean: f32,
    filled: f32,
    // The sum the mean is of (the next moments').
    sum: f32,
    // Whether the period is full, and the light's change over it: steady where it holds (nothing at the pixel changed but the
    // jitter's phase, so the history is a running mean and neither it nor the phase mean is clipped: what differs from them
    // is the jitter's own, a thin pole hit or missed, and the clip's reductions and the change test are its false alarms), else
    // the change (a change of light, smoke drifting over a still pixel: the mean is clipped into the box as the history is).
    known: bool,
    steady: bool,
    change: f32,
};

fn phase_of(x: f32, mom: vec4<f32>, kept: bool, q: vec2<i32>) -> Phase {
    var p: Phase;
    p.mean = x;
    p.filled = 0.0;
    p.sum = x;
    p.known = false;
    p.steady = false;
    p.change = 0.0;
    if !kept {
        return p;
    }
    let before = mom.z;
    var sum = mom.x + mom.y + x;
    var frames = before + 1.0;
    if before >= f32(PHASES) {
        let x8 = textureLoad(ring, q, 0).x;
        sum = sum - x8;
        frames = f32(PHASES);
        p.known = true;
        p.change = abs(x - x8);
        p.steady = p.change <= STEADY_CHANGE;
    }
    p.sum = sum;
    p.mean = sum / frames;
    p.filled = frames / f32(PHASES);
    return p;
}

// The next moments: the phase's sum and frames where the pixel's moments go on (`go_on`: kept, and no change dropped its
// history), else started again from this frame's sample.
fn moments_out(p: Phase, mom: vec4<f32>, x: f32, level: f32, go_on: bool) -> vec4<f32> {
    if !go_on {
        return vec4<f32>(split(x), 1.0, level);
    }
    return vec4<f32>(split(p.sum), min(mom.z + 1.0, STILL_COUNT), level);
}

// A still pixel's luma: the phase mean, clipped into the box `blo.x`..`bhi.x` unless its light holds, blended into the
// clipped history's `clipped_x` as the period fills.
fn phase_luma(p: Phase, clipped_x: f32, blo: f32, bhi: f32) -> f32 {
    let mean = select(clamp(p.mean, blo, bhi), p.mean, p.steady);
    return mix(clipped_x, mean, p.filled);
}


// The current sample's rebuilding filter: exp(-k d^2) of a jittered sample's distance d (pixels) from the pixel's centre (Karis's
// fit of a Blackman-Harris window; narrower ones leave edges of moving shapes behind longer).
const RECONSTRUCTION: f32 = 2.29;
const STILL_FRAMES: f32 = 8.0;
const MOVING_FRAMES: f32 = 5.0;
const GAMMA_MOVING: f32 = 1.0;
const GAMMA_STILL: f32 = 1.0;
// The frames a history may hold where its texel was another object's or part's (what a limb passed over).
const OTHER_FRAMES: f32 = 3.0;
// The speed (output pixels a frame) over which the stillness decays.
const SLOW_PIXELS: f32 = 1.0;
// The speed (output pixels a frame) over which a history's cap falls from the still frames to the moving ones.
const HISTORY_STILL_PIXELS: f32 = 0.05;
// The motion (internal pixels a frame) under which a shape's part counts as still for the object test.
const STILL_PART_PIXELS: f32 = 0.01;
// The sky's reprojection shift (output pixels) under which it counts as not moved at all.
const SKY_STILL_PIXELS: f32 = 0.001;
// The box (a share of the neighbourhood's) around the neighbourhood's middle a static pixel's history is clipped into where a
// shape's part has just left.
const LEFT_BOX: f32 = 0.25;
// The change of a neighborhood's mean luma since last frame that starts its history again: a share of the larger, and a margin
// (compressed luma).
const CHANGE_SHARE: f32 = 0.4;
const CHANGE_MARGIN: f32 = 0.02;
// The change of the sun's unjittered visibility (the mask's a) at a pixel since last frame that starts its history again, and how
// much of its filter's tent the mask must have kept (its b) for the test: less, and the mask may step at the pixel (a crease,
// another surface nearby), which a read between the pixels or a moving camera's reprojection moves.
const MASK_CHANGE: f32 = 0.15;
const MASK_SMOOTH: f32 = 0.98;
// Where the mask's change is read: the pixel and its four neighbors (internal pixels).
const MASK_TAPS: array<vec2<f32>, 5> = array<vec2<f32>, 5>(
    vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(-1.0, 0.0), vec2<f32>(0.0, 1.0), vec2<f32>(0.0, -1.0),
);

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

// The history at `uv` through a cubic filter (4 x 4 texels; the history is read with loads, so a 32-bit one needs no filtering):
// one texel where `uv` is a texel's centre (a still pixel's history is read exactly).
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

// What a pixel's resolve starts from: the current sample, its neighborhood, where its history lies and how far it is trusted.
struct Taa {
    uv: vec2<f32>,
    // The current colour rebuilt from the 3 x 3 (compressed), and the 3 x 3's mean luma.
    cur: vec3<f32>,
    level: f32,
    // The 3 x 3's sums (YCoCg, compressed) and squares, and its extremes; and its mean and mean square weighted as the
    // current colour is (the samples nearest the pixel's centre count most).
    sum1: vec3<f32>,
    sum2: vec3<f32>,
    near1: vec3<f32>,
    near2: vec3<f32>,
    lo: vec3<f32>,
    hi: vec3<f32>,
    // The nearest surface's motion (the sky's: none) and where the history lies.
    m: vec4<f32>,
    prev_uv: vec2<f32>,
    sky: bool,
    // The disocclusion's and the object tests' trust (0: dropped), and whether the history's texel was another object's that is
    // gone from the 3 x 3 (`gone`) or a shape's part around it is (`left`).
    trust: f32,
    gone: bool,
    left: bool,
    // Whether a sky pixel's history was dropped for the surface in its 3 x 3 last frame (`sky_dropped`), and whether what made
    // that drop, `gone` or `left` moved (the sky's reprojection, this pixel, the part last frame): with nothing moving, a part
    // only drops in and out of the 3 x 3 with the jitter's phases (a thin pole).
    sky_dropped: bool,
    moved_part: bool,
    // Whether the 3 x 3 is one object (none of it sky): where the sun mask's test applies.
    one_object: bool,
    // False where the history cannot be used at all (no history, the history's place off the frame).
    usable: bool,
};

// The current sample, its neighborhood and the history's place and trust at the fragment `frag`.
fn gather(frag: vec4<f32>) -> Taa {
    var t: Taa;
    let uv = frag.xy * frame.output.zw;
    t.uv = uv;
    let isize = vec2<i32>(frame.screen.xy);
    let j = frame.temporal.xy;
    let pin = uv * frame.screen.xy;
    // The image is drawn moved by the jitter: pixel q's sample lies at q + 1/2 - j of the unjittered frame.
    let base = vec2<i32>(floor(pin + j));
    var sum = vec3<f32>(0.0);
    var wsum = 0.0;
    var sum1 = vec3<f32>(0.0);
    var sum2 = vec3<f32>(0.0);
    var near1 = vec3<f32>(0.0);
    var near2 = vec3<f32>(0.0);
    var lo = vec3<f32>(1e9);
    var hi = vec3<f32>(-1e9);
    var near_d = 0.0;
    var near_q = clamp(base, vec2<i32>(0), isize - 1);
    // The 3 x 3's object ids (the object test).
    var ids: array<f32, 9>;
    for (var k = 0u; k < 9u; k = k + 1u) {
        let o = vec2<i32>(i32(k % 3u) - 1, i32(k / 3u) - 1);
        let q = clamp(base + o, vec2<i32>(0), isize - 1);
        let c = compress(max(textureLoad(current, q, 0).rgb, vec3<f32>(0.0)));
        let d = vec2<f32>(q) + 0.5 - j - pin;
        let w = exp(-RECONSTRUCTION * dot(d, d));
        sum = sum + c * w;
        wsum = wsum + w;
        let y = to_ycocg(c);
        sum1 = sum1 + y;
        sum2 = sum2 + y * y;
        near1 = near1 + y * w;
        near2 = near2 + y * y * w;
        lo = min(lo, y);
        hi = max(hi, y);
        let z = textureLoad(depth_now, q, 0);
        if z > near_d {
            near_d = z;
            near_q = q;
        }
        ids[k] = textureLoad(motion_now, q, 0).w;
    }
    t.cur = sum / max(wsum, 1e-6);
    // The neighborhood's mean luma: the jitter barely moves it, a change of light does.
    t.level = sum1.x / 9.0;
    t.sum1 = sum1;
    t.sum2 = sum2;
    t.near1 = near1 / max(wsum, 1e-6);
    t.near2 = near2 / max(wsum, 1e-6);
    t.lo = lo;
    t.hi = hi;
    t.trust = 1.0;
    t.usable = false;
    t.sky_dropped = false;
    if frame.temporal.w < 0.5 {
        return t;
    }
    // The motion: the nearest surface's, or the sky's (the far plane's point under the two cameras).
    let m = textureLoad(motion_now, near_q, 0);
    t.m = m;
    var prev_uv = uv - m.xy;
    let sky = near_d <= 0.0 || m.w < 0.5;
    t.sky = sky;
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
    t.prev_uv = prev_uv;
    if any(prev_uv < vec2<f32>(0.0)) || any(prev_uv > vec2<f32>(1.0)) {
        return t;
    }
    t.usable = true;
    var trust = 1.0;
    // Whether the history's texel was another object's or part's than the surface here that is gone from the 3 x 3 (the object
    // test below), and whether a part around it is.
    var gone = false;
    var left = false;
    var moved_part = false;
    if sky {
        // A pixel all sky now whose history's 3 x 3 held a surface last frame: its history may be that surface's (an edge pixel
        // follows the surface in front), and a pale one (a lamp's head) passes as a cloud in the clip's box, so it trailed over the
        // sky for many frames behind a lamp post walked past. Dropped.
        let qs = vec2<i32>(floor(prev_uv * frame.screen.xy));
        moved_part = any(prev_uv != uv);
        for (var k = 0u; k < 9u; k = k + 1u) {
            let q = clamp(qs + vec2<i32>(i32(k % 3u) - 1, i32(k / 3u) - 1), vec2<i32>(0), isize - 1);
            if textureLoad(depth_before, q, 0) > 0.0 {
                trust = 0.0;
                if length(textureLoad(motion_before, q, 0).xy * frame.screen.xy) > STILL_PART_PIXELS {
                    moved_part = true;
                }
            }
        }
        t.sky_dropped = trust == 0.0;
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
        //   frames at most.
        gone = abs(mv_before.w - m.w) > 0.5;
        moved_part = length(m.xy * frame.screen.xy) > STILL_PART_PIXELS
            || length(mv_before.xy * frame.screen.xy) > STILL_PART_PIXELS;
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
            let mb = textureLoad(motion_before, q, 0);
            let wb = mb.w;
            if wb > 1.5 {
                var here = false;
                for (var j = 0u; j < 9u; j = j + 1u) {
                    if abs(ids[j] - wb) < 0.5 {
                        here = true;
                    }
                }
                if !here {
                    left = true;
                    if length(mb.xy * frame.screen.xy) > STILL_PART_PIXELS {
                        moved_part = true;
                    }
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
    t.trust = trust;
    t.gone = gone;
    t.left = left;
    t.moved_part = moved_part;
    var one_object = !sky;
    for (var k = 0u; k < 9u; k = k + 1u) {
        if abs(ids[k] - ids[4]) > 0.5 {
            one_object = false;
        }
    }
    t.one_object = one_object;
    return t;
}

// The neighborhood's mean luma since last frame (`level_before`, kept in the shown color's alpha): a change the jitter cannot make
// is a change of light (a shadow's edge passed, a light switched), and the history starts again.
fn light_changed(level: f32, level_before: f32) -> bool {
    return abs(level - level_before) > CHANGE_SHARE * max(level, level_before) + CHANGE_MARGIN;
}

// The sun's unjittered visibility now and at the history's place last frame, only where the 3 x 3 is one object (none of it
// sky) and the mask's filter kept its whole tent there (no crease, no other surface within its reach: there the mask steps,
// and a read between its pixels moves with the reprojection). A shadow's edge is wider than a pixel, so its change is read at
// the pixel and its four neighbors and taken where most of them show it.
fn shadow_passed(t: Taa) -> bool {
    if !t.one_object {
        return false;
    }
    var changed = 0u;
    var whole = true;
    for (var k = 0u; k < 5u; k = k + 1u) {
        let o = MASK_TAPS[k] * frame.screen.zw;
        let mask = textureSampleLevel(mask_now, linear_clamp, t.uv + o, 0.0);
        let mask_last = textureSampleLevel(mask_before, linear_clamp, t.prev_uv + o, 0.0);
        if min(mask.b, mask_last.b) < MASK_SMOOTH {
            whole = false;
        }
        if abs(mask.a - mask_last.a) > MASK_CHANGE {
            changed = changed + 1u;
        }
    }
    return whole && changed >= 3u;
}

// The pixel's speed in output pixels a frame (the sky's is its reprojection's: it has no motion of its own).
fn speed_of(t: Taa) -> f32 {
    return select(length(t.m.xy * frame.output.xy), length((t.uv - t.prev_uv) * frame.output.xy), t.sky);
}
