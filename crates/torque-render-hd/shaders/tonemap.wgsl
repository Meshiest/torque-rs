// The enhanced renderer's last pass: the HDR frame times the exposure, our own neutral hue-keeping tonemap, sRGB encoded
// into the 8-bit target (the GUI then draws over it in gamma space).
//
// The tonemap is the identity up to SHOULDER (so the scene's colours stay theirs) and above it an exponential shoulder towards
// 1 applied to the largest channel, the others scaled with it: the hue and saturation of a bright colour are kept (a per-channel curve
// would bleach and shift them).
//
// The source is the anti-aliased frame (the TAA's history at the output's size, or the FXAA-style filter's frame at the
// internal size, scaled up bilinearly when it is smaller). After the TAA (Ultra) our own contrast-adaptive sharpen runs on the
// tonemapped colours, in the manner of Lottes 2019's CAS: each channel's negative lobe on the four neighbours is weighted by
// sqrt(min(lo, 1 - hi) / hi) of the cross's extremes (strong where the neighbourhood has room, none where it is at black or white,
// so no halo can overshoot), its peak set by the strength (-1/8 at 0 to -1/5 at 1).
//
// Before the exposure the bloom chain's last level (half the output's size, bilinear) is added times the settings' strength
// (a thresholded bloom: what passed its soft threshold) or mixed in at its share (a mixed bloom: the whole frame's light, so the
// frame's light is kept). The exposure is the settings' fixed one or the automatic one (exposure.wgsl's state). The AgX-style curve is
// our own formation (hd::grade::agx_style, its CPU form): the colour pulled a little toward its luminance, each channel's log2 exposure
// about mid grey (clamped to the range) through a logistic sigmoid with its ends pinned to black and white, the saturation given
// back; its output is already display-encoded.
//
// The view from under water: the tint, its colour and alpha, over the display colour, as a quad blended over the frame would
// be; only where the pixel's near-plane point is in the zone (a camera across the surface sees the waterline).
//
// The grading table (hd::lut, its CPU form): the grade's linear part (white balance, saturation), the tonemap's shaper (the neutral
// curve's hue-keeping shoulder, or the AgX-style pull toward the luminance), then our 32³ table of the rest of the curve, the grade
// and any user .cube, read trilinearly on its display-encoded axes (neutral: the sRGB transfer; AgX-style: the log exposure through
// its sigmoid), clamped after the lookup. Without the table (the identity gate's reference) the analytic curves. Then film grain
// (where asked for): a triangular noise of the pixel in display units, its pattern fixed (it does not move with the effect
// time).

const SHOULDER: f32 = 0.8;
// The AgX-style constants (hd::grade's; a test compares them).
const AGX_INSET: f32 = 0.14;
const AGX_MIN_EV: f32 = -11.0;
const AGX_MAX_EV: f32 = 7.0;
const AGX_SLOPE: f32 = 0.7;
const AGX_MID_GREY_DISPLAY: f32 = 0.461;
const AGX_OUTSET: f32 = 1.12;

struct Post {
    // x: the fixed exposure (a multiplier), y: the sharpen's strength (0 off), z: 1 when the source is scaled to the output, w: 1 for
    // the automatic exposure.
    exposure: vec4<f32>,
    // xy: the viewport's origin in the target.
    origin: vec4<u32>,
    // The output's width, height, 1 / width, 1 / height.
    output: vec4<f32>,
    // x: the bloom's strength (or share), y: 1 to mix it in (else added), z: 1 when there is a bloom.
    bloom: vec4<f32>,
    // x: 1 for the AgX-style curve (else the neutral one).
    curve: vec4<f32>,
    // X 1 when the grading table is read, y its axis (0 the sRGB transfer, 1 the AgX-style sigmoid).
    lut: vec4<f32>,
    // rgb: the grade's white balance, a: its saturation.
    grade: vec4<f32>,
    // x: the grain's amplitude (display units), y: its seed (fixed: the pattern holds still).
    grain: vec4<f32>,
    // The tint from under water (its colour in display units and its alpha; 0 when the eye is not in
    // the zone's sides), and the near plane's height over the zone's top (z) and bottom (w) as `x ndc.x + y ndc.y + z|w`: a pixel
    // is under water where it is under the top and over the bottom.
    under: vec4<f32>,
    waterline: vec4<f32>,
};

struct ExposureState {
    v: vec4<f32>,
};

@group(0) @binding(0) var hdr: texture_2d<f32>;
@group(0) @binding(1) var<uniform> post: Post;
@group(0) @binding(2) var linear_clamp: sampler;
@group(0) @binding(3) var bloom_tex: texture_2d<f32>;
@group(0) @binding(4) var<storage, read> exposure_state: ExposureState;
// The grading table (read with binding 2's sampler).
@group(0) @binding(5) var lut_tex: texture_3d<f32>;
const LUT_N: f32 = 32.0;

@vertex
fn vs_full(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let x = f32((i << 1u) & 2u);
    let y = f32(i & 2u);
    return vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
}

fn tonemap(c: vec3<f32>) -> vec3<f32> {
    let m = max(c.r, max(c.g, c.b));
    if m <= SHOULDER {
        return c;
    }
    let span = 1.0 - SHOULDER;
    let mapped = SHOULDER + span * (1.0 - exp(-(m - SHOULDER) / span));
    return c * (mapped / m);
}

fn lum(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
}

fn sigmoid(ev: f32, pivot: f32) -> f32 {
    return 1.0 / (1.0 + exp(-AGX_SLOPE * (ev - pivot)));
}

fn agx_style(c_in: vec3<f32>) -> vec3<f32> {
    let c0 = max(c_in, vec3<f32>(0.0));
    let l = lum(c0);
    let c = c0 + (vec3<f32>(l) - c0) * AGX_INSET;
    let pivot = log(1.0 / AGX_MID_GREY_DISPLAY - 1.0) / AGX_SLOPE;
    let lo = sigmoid(AGX_MIN_EV, pivot);
    let hi = sigmoid(AGX_MAX_EV, pivot);
    let ev = clamp(log2(max(c, vec3<f32>(1e-10)) / 0.18), vec3<f32>(AGX_MIN_EV), vec3<f32>(AGX_MAX_EV));
    let s = vec3<f32>(sigmoid(ev.r, pivot), sigmoid(ev.g, pivot), sigmoid(ev.b, pivot));
    let y = clamp((s - vec3<f32>(lo)) / (hi - lo), vec3<f32>(0.0), vec3<f32>(1.0));
    let ly = lum(y);
    return clamp(vec3<f32>(ly) + (y - vec3<f32>(ly)) * AGX_OUTSET, vec3<f32>(0.0), vec3<f32>(1.0));
}

fn encode(c: vec3<f32>) -> vec3<f32> {
    let x = clamp(c, vec3<f32>(0.0), vec3<f32>(1.0));
    return select(1.055 * pow(x, vec3<f32>(1.0 / 2.4)) - 0.055, x * 12.92, x <= vec3<f32>(0.0031308));
}

fn exposure() -> f32 {
    if post.exposure.w > 0.5 {
        return exp2(exposure_state.v.x);
    }
    return post.exposure.x;
}

// The source's colour with the bloom at output uv, exposed.
fn exposed(src: vec3<f32>, uv: vec2<f32>) -> vec3<f32> {
    var c = max(src, vec3<f32>(0.0));
    if post.bloom.z > 0.5 {
        let b = textureSampleLevel(bloom_tex, linear_clamp, uv, 0.0).rgb;
        if post.bloom.y > 0.5 {
            c = mix(c, b, post.bloom.x);
        } else {
            c = c + b * post.bloom.x;
        }
    }
    return c * exposure();
}

// The AgX-style curve's per-channel part (hd::grade::agx_channel): the grading table's AgX-style axis.
fn agx_channel(c: vec3<f32>) -> vec3<f32> {
    let pivot = log(1.0 / AGX_MID_GREY_DISPLAY - 1.0) / AGX_SLOPE;
    let lo = sigmoid(AGX_MIN_EV, pivot);
    let hi = sigmoid(AGX_MAX_EV, pivot);
    let ev = clamp(log2(max(c, vec3<f32>(1e-10)) / 0.18), vec3<f32>(AGX_MIN_EV), vec3<f32>(AGX_MAX_EV));
    let s = vec3<f32>(sigmoid(ev.r, pivot), sigmoid(ev.g, pivot), sigmoid(ev.b, pivot));
    return clamp((s - vec3<f32>(lo)) / (hi - lo), vec3<f32>(0.0), vec3<f32>(1.0));
}

// The grading table's display values (hd::lut::lookup): the grade's linear part, the shaper, the table.
fn graded(c_in: vec3<f32>) -> vec3<f32> {
    var x = max(c_in, vec3<f32>(0.0)) * post.grade.rgb;
    let l = lum(x);
    x = max(vec3<f32>(l) + (x - vec3<f32>(l)) * post.grade.a, vec3<f32>(0.0));
    var t: vec3<f32>;
    if post.lut.y > 0.5 {
        let lx = lum(x);
        t = agx_channel(x + (vec3<f32>(lx) - x) * AGX_INSET);
    } else {
        t = encode(tonemap(x));
    }
    let uvw = t * ((LUT_N - 1.0) / LUT_N) + vec3<f32>(0.5 / LUT_N);
    let o = textureSampleLevel(lut_tex, linear_clamp, uvw, 0.0).rgb;
    return clamp(o, vec3<f32>(0.0), vec3<f32>(1.0));
}

// The curve: the neutral one's display-linear value, or the AgX-style one's display-encoded value (through the grading table
// when it is read; the neutral table's display values taken back to display-linear for the sharpen and the encoding).
fn curve(c: vec3<f32>) -> vec3<f32> {
    if post.lut.x > 0.5 {
        let d = graded(c);
        if post.curve.x > 0.5 {
            return d;
        }
        return decode(d);
    }
    if post.curve.x > 0.5 {
        return agx_style(c);
    }
    return clamp(tonemap(c), vec3<f32>(0.0), vec3<f32>(1.0));
}

fn decode(c: vec3<f32>) -> vec3<f32> {
    let x = clamp(c, vec3<f32>(0.0), vec3<f32>(1.0));
    return select(pow((x + 0.055) / 1.055, vec3<f32>(2.4)), x / 12.92, x <= vec3<f32>(0.04045));
}

// Film grain: a triangular noise in display units of the output pixel and the seed.
fn grain(c: vec3<f32>, xy: vec2<i32>) -> vec3<f32> {
    if post.grain.x <= 0.0 {
        return c;
    }
    let seed = u32(post.grain.y);
    var h = (u32(xy.x) * 0x8da6b343u) ^ (u32(xy.y) * 0xd8163841u) ^ (seed * 0xcb1ab31fu);
    h = (h ^ (h >> 15u)) * 0x2c1b3c6du;
    h = (h ^ (h >> 12u)) * 0x297a2d39u;
    h = h ^ (h >> 15u);
    let a = f32(h & 0xffffu) / 65535.0;
    let b = f32(h >> 16u) / 65535.0;
    return clamp(c + vec3<f32>((a + b - 1.0) * post.grain.x), vec3<f32>(0.0), vec3<f32>(1.0));
}

// The underwater tint over the pixel `xy` of the output where its near-plane point is under water (the waterline antialiased over
// a pixel), in display units as a quad blended over the frame.
fn under_water(c: vec3<f32>, xy: vec2<i32>) -> vec3<f32> {
    if post.under.w <= 0.0 {
        return c;
    }
    let ndc = vec2<f32>(
        (f32(xy.x) + 0.5) * post.output.z * 2.0 - 1.0,
        1.0 - (f32(xy.y) + 0.5) * post.output.w * 2.0,
    );
    let w = post.waterline;
    // The heights' change across a pixel (NDC spans 2 over the output).
    let px = abs(w.x) * post.output.z * 2.0 + abs(w.y) * post.output.w * 2.0 + 1e-6;
    let top = w.x * ndc.x + w.y * ndc.y + w.z;
    let bottom = w.x * ndc.x + w.y * ndc.y + w.w;
    let k = (1.0 - smoothstep(-0.5 * px, 0.5 * px, top)) * smoothstep(-0.5 * px, 0.5 * px, bottom);
    return mix(c, post.under.rgb, post.under.w * k);
}

fn finish(c: vec3<f32>) -> vec3<f32> {
    if post.curve.x > 0.5 {
        return clamp(c, vec3<f32>(0.0), vec3<f32>(1.0));
    }
    return encode(c);
}

fn mapped_at(p: vec2<i32>) -> vec3<f32> {
    let s = vec2<i32>(textureDimensions(hdr));
    let q = clamp(p, vec2<i32>(0), s - 1);
    let uv = (vec2<f32>(q) + 0.5) / vec2<f32>(s);
    return curve(exposed(textureLoad(hdr, q, 0).rgb, uv));
}

@fragment
fn fs_tonemap(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {
    let xy = vec2<i32>(p.xy) - vec2<i32>(post.origin.xy);
    if post.exposure.z > 0.5 {
        let uv = (vec2<f32>(xy) + 0.5) * post.output.zw;
        let c = textureSampleLevel(hdr, linear_clamp, uv, 0.0).rgb;
        return vec4<f32>(grain(under_water(finish(curve(exposed(c, uv))), xy), xy), 1.0);
    }
    let c = mapped_at(xy);
    let strength = post.exposure.y;
    if strength <= 0.0 {
        return vec4<f32>(grain(under_water(finish(c), xy), xy), 1.0);
    }
    let n = mapped_at(xy + vec2<i32>(0, -1));
    let s = mapped_at(xy + vec2<i32>(0, 1));
    let e = mapped_at(xy + vec2<i32>(1, 0));
    let w = mapped_at(xy + vec2<i32>(-1, 0));
    let lo = min(min(min(n, s), min(e, w)), c);
    let hi = max(max(max(n, s), max(e, w)), c);
    let amp = sqrt(clamp(min(lo, vec3<f32>(1.0) - hi) / max(hi, vec3<f32>(1e-5)), vec3<f32>(0.0), vec3<f32>(1.0)));
    let peak = -1.0 / mix(8.0, 5.0, clamp(strength, 0.0, 1.0));
    let wt = amp * peak;
    let out = (c + (n + s + e + w) * wt) / (vec3<f32>(1.0) + 4.0 * wt);
    return vec4<f32>(grain(under_water(finish(clamp(out, vec3<f32>(0.0), vec3<f32>(1.0))), xy), xy), 1.0);
}
