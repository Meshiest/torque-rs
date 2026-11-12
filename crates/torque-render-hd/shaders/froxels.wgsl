// The enhanced renderer's froxel volumetrics: the point lights' light the fog scatters toward the eye. The view frustum is cut
// into a grid of cells (froxels: `fx.dims.x` across, `.y` down, `.z` slices), each cell's medium and the lights' light in it
// gathered once a frame, then summed along each column front to back, and the sum at each pixel's depth added over the opaque
// frame. The sun's god rays stay volumetric.wgsl's march, drawn beside it (a pixel's own ray keeps the shafts' edges).
//
// - Slices: the view depth `z = far (s / slices)^2` for the slice coordinate s (0 at the eye, `slices` at the grid's far end): thin
//   near the eye, about 3 units deep at 50 for a far end of 220.
// - cs_inject, a thread a froxel, at its center: the medium, the god rays' thin medium (volumetric.wgsl's: sky.vol, thinning with
//   the height fog's falloff) and the fog's: the distance fog's extinction (its keep, `((end - d) / (end - start))^2`, is
//   exp(-optical depth): the extinction 2 / (end - d) inside the fog's range, 1 / (end - d) for the linear fog) and the height
//   fog's (its extinction at the center's height). The sun's and the sky's light in the fog is already its color; what the
//   medium adds here is the point lights' light it scatters, from the lights the froxel's cluster holds (the forward pass's
//   masks), each with its falloff, its cone and its shadow cubes' soft taps, times `fx.grid.w`, in the forward pass's radiance
//   convention (pi times the phase times the light). rgb: that light per unit length, a: the extinction. Every frame from the
//   scene alone (no noise, no history): a light switched on or moved shows in the fog at once.
// - cs_integrate, a thread a column: the froxels summed front to back (each slice's light integrated over its depth against its
//   own extinction, Hillaire 2015), every froxel holding the sum up to its far boundary (rgb) and the transmittance there (a).
// - fs_composite, at the internal size, added over the opaque frame: the sum at the pixel's depth (the sky: the far end), the grid
//   read trilinearly between the slice boundaries about it. A pixel takes only the light of the air in front of it (the cells past
//   its surface are not summed into it): a light behind a wall adds nothing to the wall.
// Prepended: common.wgsl (with shadowcommon.wgsl), fullscreen.wgsl, skycommon.wgsl.

@group(0) @binding(0) var<uniform> frame: Frame;
@group(0) @binding(1) var<storage, read> lights: array<Light>;
@group(0) @binding(2) var<storage, read> masks: array<u32>;
@group(0) @binding(4) var<uniform> shadows: Shadows;
@group(0) @binding(6) var shadow_cmp: sampler_comparison;
@group(0) @binding(7) var shadow_gather: sampler;
@group(0) @binding(8) var point_maps: texture_depth_cube_array;
@group(0) @binding(17) var point_maps_half: texture_depth_cube_array;
@group(0) @binding(18) var point_maps_quarter: texture_depth_cube_array;
@group(1) @binding(0) var depth_tex: texture_depth_2d;
@group(1) @binding(2) var<uniform> fx: Froxels;
@group(1) @binding(3) var froxel_in: texture_3d<f32>;
@group(1) @binding(4) var froxel_out: texture_storage_3d<rgba16float, write>;
@group(1) @binding(5) var froxel_linear: sampler;
@group(2) @binding(1) var<uniform> sky: Sky;
@group(2) @binding(2) var sky_view_lut: texture_2d<f32>;
@group(2) @binding(3) var transmittance_lut: texture_2d<f32>;
@group(2) @binding(4) var sky_linear: sampler;

// The fog's soft shadows: the cone of directions a point looks for blockers in (radians), a lamp's size (units: the penumbra
// behind a blocker grows with it), the widest penumbra (radians) and the taps; and how far off the glow's peak is taken.
const FOG_SEARCH: f32 = 0.12;
const FOG_LIGHT_SIZE: f32 = 0.15;
const FOG_PENUMBRA_MAX: f32 = 0.3;
const FOG_SHADOW_TAPS: u32 = 8u;
const FOG_CORE: f32 = 1.5;

// The grid's uniform (froxels.rs::encode writes it).
struct Froxels {
    // Froxels across, down, slices; w unused.
    dims: vec4<u32>,
    // x: the grid's far end (view depth), yz unused, w: the medium's share of the lights.
    grid: vec4<f32>,
    // x: the lights' phase asymmetry in the fog, yzw unused.
    look: vec4<f32>,
};

// The view depth of slice coordinate `s` in a grid whose far end is `far`, and the slice coordinate of depth `z`.
fn depth_slice(z: f32, far: f32) -> f32 {
    return f32(fx.dims.z) * sqrt(max(z, 0.0) / max(far, 1e-3));
}

fn slice_depth(s: f32, far: f32) -> f32 {
    let u = s / f32(fx.dims.z);
    return far * u * u;
}

// The world point at view depth `z` on the unjittered ray through `uv` (the internal frame, 0 to 1, y down).
fn world_at_depth(uv: vec2<f32>, z: f32) -> vec3<f32> {
    let n = frame.cluster_z.x;
    let f = frame.cluster_z.y;
    let d = n * (f - z) / (max(z, 1e-4) * (f - n));
    let w = frame.inv_unjit_view_proj * vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, d, 1.0);
    return w.xyz / w.w;
}

// The god rays' thin medium at height z (volumetric.wgsl's).
fn thin_density(z: f32) -> f32 {
    if sky.height_fog.w > 0.0 {
        return sky.vol.x * exp(-max(z - sky.height_fog.z, 0.0) / max(sky.height_fog.y, 1e-3));
    }
    return sky.vol.x;
}

// The fog's extinction at `p`, `t` from the eye (skyfog.wgsl's fog_keep as a medium: its optical depth's rate along the ray),
// held under what a cell `cell` deep can take (the far end of the fog's range is opaque).
fn fog_density(p: vec3<f32>, t: f32, cell: f32) -> f32 {
    var sigma = 0.0;
    let start = frame.fog.x;
    let end = frame.fog.y;
    if frame.fog_color.w > 0.5 && t > start && end > start {
        let k = select(2.0, 1.0, frame.fog.z > 0.5);
        sigma = k / max(end - t, max(cell, 0.25));
    }
    if sky.height_fog.w > 0.0 && frame.fog.z < 0.5 {
        sigma = sigma + sky.height_fog.x * exp(-max(p.z - sky.height_fog.z, 0.0) / max(sky.height_fog.y, 1e-3));
    }
    return sigma;
}

// One comparison in cube slot `slot` (its tier's array) along `d` at `depth`.
fn cube_tap(slot: i32, d: vec3<f32>, depth: f32) -> f32 {
    let first = i32(shadows.tiers.x);
    let second = i32(shadows.tiers.y);
    if slot < first {
        return textureSampleCompareLevel(point_maps, shadow_cmp, d, slot, depth);
    }
    if slot < second {
        return textureSampleCompareLevel(point_maps_half, shadow_cmp, d, slot - first, depth);
    }
    return textureSampleCompareLevel(point_maps_quarter, shadow_cmp, d, slot - second, depth);
}

// The distance (over the light's radius) a cube in slot `slot` holds along `d`.
fn cube_depth(slot: i32, d: vec3<f32>) -> f32 {
    let first = i32(shadows.tiers.x);
    let second = i32(shadows.tiers.y);
    if slot < first {
        return textureSampleLevel(point_maps, shadow_gather, d, slot, 0);
    }
    if slot < second {
        return textureSampleLevel(point_maps_half, shadow_gather, d, slot - first, 0);
    }
    return textureSampleLevel(point_maps_quarter, shadow_gather, d, slot - second, 0);
}

// One tap of each of a light's cubes along `d` (from the light, unit) at `dist` from it (each cube by its share).
fn light_tap(light: Light, d: vec3<f32>, dist: f32) -> f32 {
    let s = light.shadow;
    let depth = (dist - 0.05 - 0.02 * dist) / max(light.pos_radius.w, 1e-4);
    var dark = 0.0;
    if s.x >= 0.0 && s.y > 0.0 {
        dark = dark + s.y * (1.0 - cube_tap(i32(s.x), d, depth));
    }
    if s.z >= 0.0 && s.w > 0.0 {
        dark = dark + s.w * (1.0 - cube_tap(i32(s.z), d, depth));
    }
    return clamp(1.0 - dark, 0.0, 1.0);
}

// A light's shadow in the fog at `p` (`ax`, `ay`: the froxel's width and height there), as a lamp of FOG_LIGHT_SIZE casts it
// (Fernando 2005): the blockers found over a cone of directions about the light's, their mean distance setting the penumbra's
// width at p (none behind a wall right in front of p, wide behind a lamp's own shade), then the cubes' taps over that cone,
// each compared at p's own distance. A soft edge, not a froxel-sized staircase or a hard line, and a point shut in a room
// whose walls are near it lies in no penumbra: no light leaks in.
fn light_visible(light: Light, p: vec3<f32>, ax: vec3<f32>, ay: vec3<f32>) -> f32 {
    let s = light.shadow;
    if shadows.point.z <= 0.0 || (s.y <= 0.0 && s.w <= 0.0) {
        return 1.0;
    }
    let to = p - light.pos_radius.xyz;
    let dist = length(to);
    if dist < 1e-4 {
        return 1.0;
    }
    let w = to / dist;
    let helper = select(vec3<f32>(0.0, 0.0, 1.0), vec3<f32>(1.0, 0.0, 0.0), abs(w.z) > 0.9);
    let t1 = normalize(cross(w, helper));
    let t2 = cross(w, t1);
    let radius = max(light.pos_radius.w, 1e-4);
    let slot = i32(select(s.z, s.x, s.x >= 0.0 && s.y > 0.0));
    let receiver = dist - 0.05 - 0.02 * dist;
    var blockers = 0.0;
    var found = 0u;
    for (var k = 0u; k < FOG_SHADOW_TAPS; k = k + 1u) {
        let o = vogel(k, FOG_SHADOW_TAPS, 0.0) * FOG_SEARCH;
        let b = cube_depth(slot, normalize(w + t1 * o.x + t2 * o.y)) * radius;
        if b < receiver {
            blockers = blockers + b;
            found = found + 1u;
        }
    }
    if found == 0u {
        return 1.0;
    }
    let blocker = max(blockers / f32(found), 0.05);
    let froxel = 0.5 * max(length(ax), length(ay)) / dist;
    let spread = min(max(FOG_LIGHT_SIZE * (dist - blocker) / (blocker * dist), froxel), FOG_PENUMBRA_MAX);
    var lit = 0.0;
    for (var k = 0u; k < FOG_SHADOW_TAPS; k = k + 1u) {
        let o = vogel(k, FOG_SHADOW_TAPS, 0.0) * spread;
        lit = lit + light_tap(light, normalize(w + t1 * o.x + t2 * o.y), dist);
    }
    return lit / f32(FOG_SHADOW_TAPS);
}

// A spot's cone in the fog: its soft edge twice as wide, reaching half its width past the outer angle (the fog's own spread
// of the beam, so a beam seen end-on is no hard disc), smootherstep across it; 1 for a point light.
fn fog_cone(light: Light, l: vec3<f32>) -> f32 {
    let k = length(light.spot.xyz);
    if k == 0.0 {
        return 1.0;
    }
    let edge = 1.0 / k;
    let x = clamp((dot(-l, light.spot.xyz) / k - light.spot.w + 0.5 * edge) / (2.0 * edge), 0.0, 1.0);
    return x * x * x * (x * (x * 6.0 - 15.0) + 10.0);
}

// A point light's falloff in the fog: the surfaces' (light_falloff) with its inverse square taken FOG_CORE further off, so the
// glow about a bright light's source has no hard peak.
fn fog_falloff(d: f32, radius: f32) -> f32 {
    let x = d / max(radius, 1e-4);
    let w = clamp(1.0 - x * x * x * x, 0.0, 1.0);
    return w * w / (1.0 + 0.25 * (d * d + FOG_CORE * FOG_CORE));
}

// The point lights' light at `p` (depth `z`, on the ray `dir`; `ax`, `ay` the froxel's width and height there) the medium
// scatters toward the eye, per unit of its scattering.
fn lights_at(p: vec3<f32>, z: f32, uv: vec2<f32>, dir: vec3<f32>, ax: vec3<f32>, ay: vec3<f32>) -> vec3<f32> {
    let tiles = frame.clusters.xyz;
    let count = min(frame.clusters.w, MAX_LIGHTS);
    let tx = min(u32(uv.x * f32(tiles.x)), tiles.x - 1u);
    let ty = min(u32(uv.y * f32(tiles.y)), tiles.y - 1u);
    let s = log(max(z, frame.cluster_z.x) / frame.cluster_z.x) * frame.cluster_z.z;
    let tz = min(u32(max(s, 0.0)), tiles.z - 1u);
    let base = ((tz * tiles.y + ty) * tiles.x + tx) * CLUSTER_WORDS;
    var sum = vec3<f32>(0.0);
    var shaded = 0u;
    for (var w = 0u; w < CLUSTER_WORDS; w = w + 1u) {
        var bits = masks[base + w];
        loop {
            if bits == 0u || shaded >= frame.limits.x {
                break;
            }
            let b = firstTrailingBit(bits);
            bits = bits & (bits - 1u);
            let i = w * 32u + b;
            if i >= count {
                continue;
            }
            shaded = shaded + 1u;
            let light = lights[i];
            let to = light.pos_radius.xyz - p;
            let d = length(to);
            if d >= light.pos_radius.w {
                continue;
            }
            let l = select(vec3<f32>(0.0, 0.0, 1.0), to / d, d > 1e-6);
            let e = fog_falloff(d, light.pos_radius.w) * fog_cone(light, l);
            if e <= 0.0 {
                continue;
            }
            let phase = henyey_greenstein(dot(dir, l), fx.look.x);
            sum = sum + light.color.rgb * (e * phase * light_visible(light, p, ax, ay));
        }
    }
    return SKY_PI * sum;
}

@compute @workgroup_size(8, 8, 1)
fn cs_inject(@builtin(global_invocation_id) id: vec3<u32>) {
    if any(id >= fx.dims.xyz) {
        return;
    }
    let dims = vec3<f32>(fx.dims.xyz);
    let far = fx.grid.x;
    let uv = (vec2<f32>(id.xy) + 0.5) / dims.xy;
    let k = f32(id.z);
    let z = slice_depth(k + 0.5, far);
    let p = world_at_depth(uv, z);
    let eye = frame.eye.xyz;
    let t = distance(p, eye);
    let dir = (p - eye) / max(t, 1e-6);
    let sigma = thin_density(p.z) + fog_density(p, t, slice_depth(k + 1.0, far) - slice_depth(k, far));
    var light = vec3<f32>(0.0);
    let medium = sigma * fx.grid.w;
    if medium > 0.0 && frame.clusters.w > 0u {
        let ax = world_at_depth(uv + vec2<f32>(1.0 / dims.x, 0.0), z) - p;
        let ay = world_at_depth(uv + vec2<f32>(0.0, 1.0 / dims.y), z) - p;
        light = medium * lights_at(p, z, uv, dir, ax, ay);
    }
    textureStore(froxel_out, id, vec4<f32>(light, sigma));
}

// One slice's light `v` (per unit length) over `depth` against extinction `sigma` (Hillaire 2015: integrated against its own
// extinction; its whole depth's worth where it is clear).
fn slice_light(v: vec3<f32>, sigma: f32, depth: f32) -> vec3<f32> {
    if sigma > 1e-5 {
        return v * ((1.0 - exp(-sigma * depth)) / sigma);
    }
    return v * depth;
}

@compute @workgroup_size(8, 8, 1)
fn cs_integrate(@builtin(global_invocation_id) id: vec3<u32>) {
    if any(id.xy >= fx.dims.xy) {
        return;
    }
    let far = fx.grid.x;
    var sum = vec3<f32>(0.0);
    var tr = 1.0;
    for (var k = 0u; k < fx.dims.z; k = k + 1u) {
        let q = vec3<u32>(id.xy, k);
        let v = textureLoad(froxel_in, q, 0);
        let depth = slice_depth(f32(k) + 1.0, far) - slice_depth(f32(k), far);
        let sigma = max(v.a, 0.0);
        sum = sum + tr * slice_light(v.rgb, sigma, depth);
        tr = tr * exp(-sigma * depth);
        textureStore(froxel_out, q, vec4<f32>(sum, tr));
    }
}

@fragment
fn fs_composite(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let d = textureLoad(depth_tex, vec2<i32>(frag.xy), 0);
    let far = fx.grid.x;
    var z = far;
    if d > 0.0 {
        z = min(linear_depth(d), far);
    }
    // The grid holds the sum to each slice's far boundary: boundary j is texel j - 1 (boundary 0, the eye, holds nothing).
    let s = depth_slice(z, far);
    let uv = frag.xy * frame.screen.zw;
    let w = (max(s, 1.0) - 0.5) / f32(fx.dims.z);
    let c = textureSampleLevel(froxel_in, froxel_linear, vec3<f32>(uv, w), 0.0).rgb * min(s, 1.0);
    return vec4<f32>(c, 0.0);
}
