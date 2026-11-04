// The weighted blended transparency's composite (McGuire and Bavoil 2013): the translucent fragments behind the front layer,
// accumulated by forward.wgsl's fs_wboit, over the frame's copy. Our own code. A full-frame triangle writes the frame where anything
// was accumulated (elsewhere it leaves the frame as it is):
//
//   tail = accum.rgb / accum.a (the weighted average of the premultiplied colours over their weighted coverage),
//   frame = tail (1 - revealage) + background revealage,
//
// in the blending space (gamma, or linear light with linear_blend). With one layer it is the exact blend.

struct Composite {
    // x: 1 when the blend is in gamma space.
    mode: vec4<f32>,
};

@group(0) @binding(0) var<uniform> comp: Composite;
@group(0) @binding(1) var background: texture_2d<f32>;
@group(0) @binding(2) var accum_tex: texture_2d<f32>;
@group(0) @binding(3) var reveal_tex: texture_2d<f32>;

@vertex
fn vs_full(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let x = f32((i << 1u) & 2u);
    let y = f32(i & 2u);
    return vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
}

fn to_gamma(c: vec3<f32>) -> vec3<f32> {
    let x = max(c, vec3<f32>(0.0));
    return select(1.055 * pow(x, vec3<f32>(1.0 / 2.4)) - 0.055, x * 12.92, x <= vec3<f32>(0.0031308));
}

fn to_linear(c: vec3<f32>) -> vec3<f32> {
    let x = max(c, vec3<f32>(0.0));
    return select(pow((x + 0.055) / 1.055, vec3<f32>(2.4)), x / 12.92, x <= vec3<f32>(0.04045));
}

// The late sprites (coronas, flares, additive particles) of a TAA frame are drawn into their own frame at the internal size and added to the shown colour after the TAA, bilinearly: they have no motion of their own, so in the TAA's history they
// smeared into streaks as the view moved.
struct LateScale {
    // xy: the internal frame's size over the output's; zw: the internal frame's size.
    scale: vec4<f32>,
};

@group(1) @binding(0) var<uniform> late_scale: LateScale;
@group(1) @binding(1) var late_tex: texture_2d<f32>;
// The extension's late draws of a TAA frame, drawn into a frame of their own (premultiplied, its alpha their
// coverage) without the jitter, over the shown colour after the TAA: in its history they and what was behind them smeared.
@group(1) @binding(2) var over_tex: texture_2d<f32>;

@fragment
fn fs_late(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {
    let at = p.xy * late_scale.scale.xy - vec2<f32>(0.5);
    let base = floor(at);
    let f = at - base;
    let hi = vec2<i32>(late_scale.scale.zw) - vec2<i32>(1);
    let a = clamp(vec2<i32>(base), vec2<i32>(0), hi);
    let b = clamp(vec2<i32>(base) + vec2<i32>(1), vec2<i32>(0), hi);
    let c00 = textureLoad(late_tex, a, 0).rgb;
    let c10 = textureLoad(late_tex, vec2<i32>(b.x, a.y), 0).rgb;
    let c01 = textureLoad(late_tex, vec2<i32>(a.x, b.y), 0).rgb;
    let c11 = textureLoad(late_tex, b, 0).rgb;
    let over00 = textureLoad(over_tex, a, 0);
    let over10 = textureLoad(over_tex, vec2<i32>(b.x, a.y), 0);
    let over01 = textureLoad(over_tex, vec2<i32>(a.x, b.y), 0);
    let over11 = textureLoad(over_tex, b, 0);
    let over = mix(mix(over00, over10, f.x), mix(over01, over11, f.x), f.y);
    // The pipeline blends one and one less the alpha: the shown colour under the over draws, then the sprites added.
    return vec4<f32>(mix(mix(c00, c10, f.x), mix(c01, c11, f.x), f.y) + over.rgb, clamp(over.a, 0.0, 1.0));
}

@fragment
fn fs_composite(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {
    let px = vec2<i32>(p.xy);
    let reveal = textureLoad(reveal_tex, px, 0).r;
    let acc = textureLoad(accum_tex, px, 0);
    if reveal >= 0.99999 || acc.a <= 1e-6 {
        discard;
    }
    let tail = acc.rgb / max(acc.a, 1e-6);
    let bg = textureLoad(background, px, 0).rgb;
    if comp.mode.x > 0.5 {
        return vec4<f32>(to_linear(tail * (1.0 - reveal) + to_gamma(bg) * reveal), 1.0);
    }
    return vec4<f32>(tail * (1.0 - reveal) + bg * reveal, 1.0);
}
