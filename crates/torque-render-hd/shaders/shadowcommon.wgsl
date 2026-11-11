// The sun shadows' uniform and the sampling patterns their filter turns: moved out of common.wgsl so that a pass from outside the
// renderer composes them with sunshadow.wgsl (`passes::SUN_SHADOW_WGSL`); every module gets them with common.wgsl.

// The shadows' uniform (shadows.rs writes it).
const MAX_CASCADES: u32 = 6u;
struct Shadows {
    // The sun cascades' world-to-clip matrices (orthographic: clip z 0 at the light's near plane, 1 at its far one).
    cascades: array<mat4x4<f32>, 6>,
    // Per cascade: x: one texel in uv, y: one texel in world units, z: the depth range in world units, w: uv per world unit.
    params: array<vec4<f32>, 6>,
    // The light's basis: right, up and forward (away from the sun).
    right: vec4<f32>,
    up: vec4<f32>,
    fwd: vec4<f32>,
    // x: cascades (0: no sun shadows), y: PCF taps, z: PCSS blocker-search taps (0: plain PCF), w: the debug view.
    config: vec4<u32>,
    // x: the tangent of the sun's angular radius, y: the shadow distance, z: where its fade starts (both from the eye), w: a
    // cascade's side in texels.
    sun: vec4<f32>,
    // rgb: what of the sun's direct light a shadow lets through (the Sun's shadow colour); w: 1 when contact shadows are on.
    tint: vec4<f32>,
    // x: a point lookup's taps, y: a first-tier cube face's side in texels, z: 1 when point shadows are on, w: the contact march's
    // steps.
    point: vec4<f32>,
    // The point cubes' tiers: x where the first tier's slots end, y where the second's do (the third's follow), z and w the second
    // and third tiers' face sides in texels.
    tiers: vec4<f32>,
    // X 1 when translucent surfaces colour the sun's shadows (the transmittance maps beside the cascades); y: the layer of the
    // first cascade kept without the client's own Player, plus one (the first-person images' lookups), z: how many cascades are,
    // w: the share of the ambient occlusion the sun's light takes.
    colour: vec4<f32>,
};

// Interleaved gradient noise (Jimenez 2014): a per-pixel value in [0, 1) that is well spread over neighbouring pixels.
fn ign(p: vec2<f32>) -> f32 {
    return fract(52.9829189 * fract(dot(p, vec2<f32>(0.06711056, 0.00583715))));
}

// The 4 x 4 ordered-dither (Bayer) matrix's entry at `t`: the bits of x xor y and of y, interleaved and reversed. Every 4 x 4
// tile holds each of the 16 once (the AO's noise, the sun mask's turn: a 5 x 5 tent over it averages all 16 alike, a 3 x 3 box
// nine).
fn bayer(t: vec2<u32>) -> u32 {
    let x = t.x & 3u;
    let y = t.y & 3u;
    let e = x ^ y;
    return ((e & 1u) << 3u) | ((y & 1u) << 2u) | (e & 2u) | ((y >> 1u) & 1u);
}

// Sample i of n on a Vogel (golden-angle) disk of unit radius, turned by `turn` radians.
fn vogel(i: u32, n: u32, turn: f32) -> vec2<f32> {
    let r = sqrt((f32(i) + 0.5) / f32(n));
    let a = f32(i) * 2.39996323 + turn;
    return r * vec2<f32>(cos(a), sin(a));
}
