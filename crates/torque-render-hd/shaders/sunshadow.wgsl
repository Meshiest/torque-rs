// The sun's cascades filter: the lookup the forward pass shades with, and a pass from outside the renderer with
// (`passes::SUN_SHADOW_WGSL`). It reads the module's `shadows`, `csm` and `shadow_gather` (and common's `Shadows`).

// The sun cascades' filter: radius limits (texels) and plain PCF's radius.
const MAX_FILTER_TEXELS: f32 = 8.0;
const MIN_FILTER_TEXELS: f32 = 1.0;
const PCF_TEXELS: f32 = 1.5;
// The normal offset (texels): small and the same at every angle, since an offset along the normal moves a grazing shadow along the
// ground by the offset over the tangent of the sun's elevation (the receiver plane does the acne's work).
const NORMAL_OFFSET_TEXELS: f32 = 0.3;

fn cascade_uvz(i: u32, p: vec3<f32>) -> vec3<f32> {
    let c = shadows.cascades[i] * vec4<f32>(p, 1.0);
    return vec3<f32>(c.x * 0.5 + 0.5, 0.5 - c.y * 0.5, c.z);
}

// The 2 x 2 texels around `uv` (gather order: x (0, 1), y (1, 1), z (1, 0), w (0, 0)): their stored depths, the receiver's depth
// extended along its plane to each one's centre, and the bilinear weights' fraction.
struct Footprint {
    stored: vec4<f32>,
    receiver: vec4<f32>,
    frac: vec2<f32>,
};

fn footprint(layer: u32, uv: vec2<f32>, z: f32, grad: vec2<f32>) -> Footprint {
    let size = shadows.sun.w;
    let f = uv * size - 0.5;
    let base = floor(f);
    var out: Footprint;
    out.frac = f - base;
    out.stored = textureGather(csm, shadow_gather, (base + 1.0) / size, layer);
    let r00 = z + dot(grad, (base + 0.5) / size - uv);
    let rx = grad.x / size;
    let ry = grad.y / size;
    // Past the far plane nothing is stored but the clear (or casters flattened onto it, all behind the receiver): a grazing receiver
    // near the far end must not compare its plane's extension beyond it with that.
    out.receiver = min(vec4<f32>(r00 + ry, r00 + rx + ry, r00 + rx, r00), vec4<f32>(1.0));
    return out;
}

fn csm_tap(layer: u32, uv: vec2<f32>, z: f32, grad: vec2<f32>) -> f32 {
    let fp = footprint(layer, uv, z, grad);
    let lit = select(vec4<f32>(0.0), vec4<f32>(1.0), fp.receiver <= fp.stored);
    return mix(mix(lit.w, lit.z, fp.frac.x), mix(lit.x, lit.y, fp.frac.x), fp.frac.y);
}

// The blockers of a footprint: the sum of their depths and their count.
fn csm_blockers(layer: u32, uv: vec2<f32>, z: f32, grad: vec2<f32>) -> vec2<f32> {
    let fp = footprint(layer, uv, z, grad);
    let b = select(vec4<f32>(0.0), vec4<f32>(1.0), fp.stored < fp.receiver);
    return vec2<f32>(dot(b, fp.stored), dot(b, vec4<f32>(1.0)));
}

// The cascade and its uv the last sun_shadow looked up (the transmittance's lookup), -1 for none.
var<private> sun_layer: i32 = -1;
var<private> sun_uv: vec2<f32> = vec2<f32>(0.0);
// Set per fragment: a first-person image's lookups in the finest cascade read the map without the client's own Player.
var<private> sun_no_self: bool = false;
// The filter's radius (world units) the last sun_shadow_turned used, 0 for none: the penumbra's half width.
var<private> sun_radius: f32 = 0.0;

// The cascade that holds `p` (normal `n`): its layer in x (-1 for none: no cascades, past the shadow distance, or outside every
// cascade) and its uv and depth in yzw; `sun_layer` and `sun_uv` are set to it (the transmittance's lookup).
fn sun_cascade(p: vec3<f32>, n: vec3<f32>, eye: vec3<f32>) -> vec4<f32> {
    let none = vec4<f32>(-1.0, 0.0, 0.0, 0.0);
    let count = min(shadows.config.x, MAX_CASCADES);
    if count == 0u {
        return none;
    }
    // How far the shadows reach is a distance from the eye (a view depth's end would be a plane that moves as the camera turns).
    if distance(p, eye) >= shadows.sun.y {
        return none;
    }
    for (var i = 0u; i < count; i = i + 1u) {
        let prm = shadows.params[i];
        let q = cascade_uvz(i, p + n * (prm.y * NORMAL_OFFSET_TEXELS));
        let m = prm.x * (MAX_FILTER_TEXELS + 2.0);
        if all(q.xy > vec2<f32>(m)) && all(q.xy < vec2<f32>(1.0 - m)) && q.z < 1.0 {
            sun_layer = i32(i);
            sun_uv = q.xy;
            return vec4<f32>(f32(i), q);
        }
    }
    return none;
}

// The sun's visibility at `p` (normal `n`, N.L `nl` > 0) through the cascades, for an eye at `eye`. `steady`: the taps are not turned
// per pixel (no TAA averages the pattern).
fn sun_shadow_at(p: vec3<f32>, n: vec3<f32>, nl: f32, frag: vec2<f32>, eye: vec3<f32>, steady: bool) -> f32 {
    // The taps turned per pixel only where the TAA averages it (else a speckle).
    let turn = select(ign(frag) * 6.2831853, 0.0, steady);
    return sun_shadow_turned(p, n, nl, turn, eye);
}

// The map that holds cascade `layer` (a first-person image's own, without its Player, where it has one).
fn sun_map(layer: u32) -> u32 {
    if sun_no_self && layer < u32(shadows.colour.z) {
        return u32(shadows.colour.y) - 1u + layer;
    }
    return layer;
}

// The receiver's plane (normal `n`) in cascade `layer`'s light space, as depth (0..1) per uv. A face the sun lights has
// n . fwd = -N.L < 0; kept off the grazing limit.
fn sun_plane(layer: u32, n: vec3<f32>) -> vec2<f32> {
    let prm = shadows.params[layer];
    let nlight = vec3<f32>(dot(n, shadows.right.xyz), dot(n, shadows.up.xyz), dot(n, shadows.fwd.xyz));
    let nz = min(nlight.z, -0.05);
    return vec2<f32>(-nlight.x / nz, nlight.y / nz) / (prm.w * prm.z);
}

// The receiver's depth at `uvz` in cascade `layer`, less the bias.
fn sun_receiver(layer: u32, uvz: vec3<f32>) -> f32 {
    return uvz.z - 2e-4 / shadows.params[layer].z;
}

// The same with the taps turned by `turn` radians.
fn sun_shadow_turned(p: vec3<f32>, n: vec3<f32>, nl: f32, turn: f32, eye: vec3<f32>) -> f32 {
    let found = sun_cascade(p, n, eye);
    if found.x < 0.0 {
        return 1.0;
    }
    let layer = u32(found.x);
    let uvz = found.yzw;
    let reach = distance(p, eye);
    let map = sun_map(layer);
    let prm = shadows.params[layer];
    let grad = sun_plane(layer, n);
    let z = sun_receiver(layer, uvz);
    let texel = prm.x;
    var radius = PCF_TEXELS * texel;
    let blockers = shadows.config.z;
    if blockers > 0u {
        let search = clamp(shadows.sun.x * uvz.z * prm.z * prm.w, 2.0 * texel, MAX_FILTER_TEXELS * texel);
        var sum = vec2<f32>(0.0);
        for (var i = 0u; i < blockers; i = i + 1u) {
            let o = vogel(i, blockers, turn) * search;
            sum = sum + csm_blockers(map, uvz.xy + o, z + dot(grad, o), grad);
        }
        if sum.y <= 0.0 {
            return 1.0;
        }
        let pen = shadows.sun.x * max(uvz.z - sum.x / sum.y, 0.0) * prm.z * prm.w;
        radius = clamp(pen, MIN_FILTER_TEXELS * texel, MAX_FILTER_TEXELS * texel);
    }
    sun_radius = radius / prm.w;
    let taps = max(shadows.config.y, 1u);
    var lit = 0.0;
    for (var i = 0u; i < taps; i = i + 1u) {
        let o = vogel(i, taps, turn) * radius;
        lit = lit + csm_tap(map, uvz.xy + o, z + dot(grad, o), grad);
    }
    lit = lit / f32(taps);
    return mix(lit, 1.0, smoothstep(shadows.sun.z, shadows.sun.y, reach));
}

// The sun's visibility at `p` (normal `n`) through one bilinear 2 x 2 tap of its cascade: no turn and no search, so a still
// point under a still shadow reads the same every frame.
fn sun_shadow_plain(p: vec3<f32>, n: vec3<f32>, eye: vec3<f32>) -> f32 {
    let found = sun_cascade(p, n, eye);
    if found.x < 0.0 {
        return 1.0;
    }
    let layer = u32(found.x);
    let lit = csm_tap(sun_map(layer), found.yz, sun_receiver(layer, found.yzw), sun_plane(layer, n));
    return mix(lit, 1.0, smoothstep(shadows.sun.z, shadows.sun.y, distance(p, eye)));
}

