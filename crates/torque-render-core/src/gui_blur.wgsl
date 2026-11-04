// The GUI's backdrop blur (this crate's own shader): a dual-filter Kawase blur (Bjorge, SIGGRAPH 2015) in linear light.
// A 5-tap downsample chain from the grabbed region (read through an sRGB view: the hardware decodes before it filters), an 8-tap
// upsample chain back to half size, and the composite: the last upsample at each target pixel, encoded to sRGB, the tint laid
// over it, put inside a rounded rectangle (1-pixel anti-aliased edge). Every tap falls on a quarter-texel position, so the
// bilinear weights are exact and the CPU reference (`blur::reference`) computes the same values.

struct Params {
    // The source texture's size and the destination's (texels).
    src_size: vec2<f32>,
    dst_size: vec2<f32>,
    // The grabbed region's top-left in the target (pixels): target pixel p is level-0 texel p - origin.
    origin: vec2<f32>,
    // 1: blurred (the composite samples level 1); 0: the tint only.
    blur: f32,
    corner: f32,
    // The control's rectangle (left, top, right, bottom) in target pixels.
    rect: vec4<f32>,
    // The tint (sRGB, 0..1) and its alpha; the opacity.
    tint: vec4<f32>,
    opacity: f32,
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
};

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var src: texture_2d<f32>;
@group(0) @binding(2) var samp: sampler;

@vertex
fn vs_full(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    // One triangle over the whole target.
    let x = f32((i << 1u) & 2u);
    let y = f32(i & 2u);
    return vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
}

fn tap(p: vec2<f32>) -> vec4<f32> {
    return textureSampleLevel(src, samp, p / params.src_size, 0.0);
}

// Destination texel j (centre j + 0.5) covers source texels 2j and 2j + 1: its centre is the source corner 2j + 1.
@fragment
fn fs_down(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let c = pos.xy * 2.0;
    var sum = tap(c) * 4.0;
    sum += tap(c + vec2<f32>(-1.0, -1.0));
    sum += tap(c + vec2<f32>(1.0, 1.0));
    sum += tap(c + vec2<f32>(1.0, -1.0));
    sum += tap(c + vec2<f32>(-1.0, 1.0));
    return sum / 8.0;
}

fn up(c: vec2<f32>) -> vec4<f32> {
    var sum = tap(c + vec2<f32>(-1.0, 0.0));
    sum += tap(c + vec2<f32>(-0.5, 0.5)) * 2.0;
    sum += tap(c + vec2<f32>(0.0, 1.0));
    sum += tap(c + vec2<f32>(0.5, 0.5)) * 2.0;
    sum += tap(c + vec2<f32>(1.0, 0.0));
    sum += tap(c + vec2<f32>(0.5, -0.5)) * 2.0;
    sum += tap(c + vec2<f32>(0.0, -1.0));
    sum += tap(c + vec2<f32>(-0.5, -0.5)) * 2.0;
    return sum / 12.0;
}

// Destination texel j (centre j + 0.5) sits at source coordinate (j + 0.5) / 2.
@fragment
fn fs_up(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    return up(pos.xy * 0.5);
}

fn encode(l: f32) -> f32 {
    let c = clamp(l, 0.0, 1.0);
    if c <= 0.0031308 {
        return c * 12.92;
    }
    return 1.055 * pow(c, 1.0 / 2.4) - 0.055;
}

// The rounded rectangle's coverage at a pixel centre.
fn coverage(p: vec2<f32>) -> f32 {
    let lo = params.rect.xy;
    let hi = params.rect.zw;
    let centre = (lo + hi) * 0.5;
    let half = (hi - lo) * 0.5;
    let r = min(params.corner, min(half.x, half.y));
    let q = abs(p - centre) - half + vec2<f32>(r, r);
    let d = length(max(q, vec2<f32>(0.0, 0.0))) + min(max(q.x, q.y), 0.0) - r;
    return clamp(0.5 - d, 0.0, 1.0);
}

@fragment
fn fs_composite(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let a = coverage(pos.xy) * params.opacity;
    if params.blur < 0.5 {
        return vec4<f32>(params.tint.rgb, params.tint.a * a);
    }
    let l = up((pos.xy - params.origin) * 0.5);
    let g = vec3<f32>(encode(l.r), encode(l.g), encode(l.b));
    return vec4<f32>(mix(g, params.tint.rgb, params.tint.a), a);
}

// The offscreen GUI target copied to the window's surface, texel for texel.
@fragment
fn fs_blit(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    return textureLoad(src, vec2<i32>(pos.xy), 0);
}
