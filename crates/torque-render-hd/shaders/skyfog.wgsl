// The forward pass's sky parts, appended to forward.wgsl: the sky's group, the fog, the emitters' gains and the lit cloud layers.
//
// - Fog: in amount 1 - ((end - d) / (end - start))^2 from the Sky's fog distance to the visible distance (with the height fog setting times
//   an exponential height fog's transmittance, its optical depth along the segment in closed form) toward the sky drawn behind the
//   pixel (the horizon ban's colour at the horizon; a fogged-out surface is the sky beside it), the server's sky too;
//   without the sky's detail (its low mip: no clouds' shapes on a fogged wall), the detail only where a surface wholly fogged ends
//   at the visible distance.
// - The clouds over a surface the eye sees through a cloud layer's slab (clouds_over_share).
// - Emitters: glow surfaces are their paint times the settings' glow gain; self-illuminating shape parts and the coronas and flares are
//   scaled into the HDR range (the bloom then picks them up).
// - Clouds (linear light): a sky's cloud layer as the server colours it, its alpha faded toward the horizon, brightened toward the sun (a
//   share of a forward-scattering lobe, held under a cap) while the sun is up.

@group(2) @binding(1) var<uniform> sky: Sky;
@group(2) @binding(2) var sky_view_lut: texture_2d<f32>;
@group(2) @binding(3) var transmittance_lut: texture_2d<f32>;
@group(2) @binding(4) var sky_linear: sampler;
// The frame after the sky's draws: the sky behind each pixel (the fog's colour).
@group(2) @binding(5) var sky_behind: texture_2d<f32>;

// The share of a surface's colour the fog leaves at `p` (1 none).
fn fog_keep(p: vec3<f32>) -> f32 {
    let d = distance(p, frame.eye.xyz);
    let span = max(frame.fog.y - frame.fog.x, 1e-3);
    let near = clamp((frame.fog.y - d) / span, 0.0, 1.0);
    // A linear fog from the Sky's fog distance to its visible distance (TGE's).
    if frame.fog.z > 0.5 {
        return near;
    }
    var keep = near * near;
    if sky.height_fog.w > 0.0 {
        let h = max(sky.height_fog.y, 1e-3);
        let z0 = frame.eye.z - sky.height_fog.z;
        let z1 = p.z - sky.height_fog.z;
        let dz = z1 - z0;
        let e0 = exp(-max(z0, 0.0) / h);
        var od: f32;
        if abs(dz) < 1e-3 {
            od = sky.height_fog.x * d * e0;
        } else {
            let e1 = exp(-max(z1, 0.0) / h);
            od = sky.height_fog.x * d * h * (e0 - e1) / dz;
        }
        keep = keep * exp(-max(od, 0.0));
    }
    return keep;
}

// Where the fog's sky is held to white (with the atmosphere's own sky: to the atmosphere's light there, the disk left out): within
// FOG_SUN_INNER radians of the sun wholly, eased out to FOG_SUN_OUTER; past it the sky behind is taken as it is, up to FOG_SKY_CAP
// (the clouds' bright sides, the glow).
const FOG_SUN_INNER: f32 = 0.08;
const FOG_SUN_OUTER: f32 = 0.2;
const FOG_SKY_CAP: f32 = 64.0;

// A mip level of `t` at `uv` through a cubic B-spline (4 bilinear taps; smooth across its texels, so a low level's cells leave no
// creases).
fn smooth_level(t: texture_2d<f32>, uv: vec2<f32>, level: f32) -> vec4<f32> {
    let dims = vec2<f32>(textureDimensions(t, i32(level)));
    let st = uv * dims - 0.5;
    let i = floor(st);
    let f = st - i;
    let f2 = f * f;
    let f3 = f2 * f;
    let w0 = (1.0 - f) * (1.0 - f) * (1.0 - f) / 6.0;
    let w1 = (4.0 - 6.0 * f2 + 3.0 * f3) / 6.0;
    let w2 = (1.0 + 3.0 * f + 3.0 * f2 - 3.0 * f3) / 6.0;
    let w3 = f3 / 6.0;
    let g0 = w0 + w1;
    let g1 = w2 + w3;
    let h0 = (i - 0.5 + w1 / g0) / dims;
    let h1 = (i + 1.5 + w3 / g1) / dims;
    let a = textureSampleLevel(t, sky_linear, vec2<f32>(h0.x, h0.y), level);
    let b = textureSampleLevel(t, sky_linear, vec2<f32>(h1.x, h0.y), level);
    let c = textureSampleLevel(t, sky_linear, vec2<f32>(h0.x, h1.y), level);
    let d = textureSampleLevel(t, sky_linear, vec2<f32>(h1.x, h1.y), level);
    return g0.y * (g0.x * a + g1.x * b) + g1.y * (g0.x * c + g1.x * d);
}

// The share of a surface's own colour under which a surface at the far end of the visible distance takes the sky's detail behind it
// (sky.fog_low.zw: from where to where along that distance).
const FOG_DETAIL_KEEP: f32 = 0.02;

// How much of the sky's detail behind it a surface `d` away keeping `keep` of its colour takes (1 all of it): only one the fog wholly
// hides at the far end of the visible distance (it ends there, and meets the sky beside it without a seam); HdFeatures::fog_sky_mean
// off (sky.fog_low.z < 0): every surface.
fn fog_detail_share(d: f32, keep: f32) -> f32 {
    if sky.fog_low.z < 0.0 {
        return 1.0;
    }
    return smoothstep(sky.fog_low.z, sky.fog_low.w, d / max(frame.fog.y, 1e-3)) * (1.0 - smoothstep(0.0, FOG_DETAIL_KEEP, keep));
}

// The low mip spreads the sun disk's light (and the flares') over the cells about it: there it is held as the sky near the sun is,
// over this wider cone (radians).
const FOG_LOW_SUN: f32 = 0.45;

// The fog's colour toward pixel `frag` (the surface at `p`, keeping `keep` of its colour): the sky drawn behind it (the box, the
// clouds and the bans as the sky draws them; the clear where there is no sky) without its detail: the sky copy's low mip (the mean over
// cells of 64 pixels, through a B-spline), so the clouds' shapes and the box's painted clouds do not show through a fogged wall or
// the underside of a deck; the detail only as fog_detail_share says. Near the sun no more than white: the sun disk and the flares
// (HDR emitters drawn with the sky) do not shine through a fogged surface. Elsewhere the sky's own light, so a fogged-out surface
// is the sky beside it (all of it held to white cut the clouds' highlights: a fogged wall showed as a lighter patch).
fn fog_color_toward(frag: vec2<f32>, p: vec3<f32>, keep: f32) -> vec3<f32> {
    // A secondary view (the water's mirrored camera, a capture face) has no sky copy of its own: the horizon ban's colour.
    if CLIP {
        return frame.fog_color.rgb;
    }
    let dir = normalize(p - frame.eye.xyz);
    let to_sun = dot(dir, normalize(sky.drawn.xyz));
    let near_sun = smoothstep(cos(FOG_SUN_OUTER), cos(FOG_SUN_INNER), to_sun);
    // The atmosphere's own sky: held to its light there as the sky draws it (only the disk held back; the glow keeps its
    // colour, so the fogged ground under a low sun meets the sky at the horizon without a white band); the server's: to white.
    var hold = vec3<f32>(1.0);
    if sky.params.w > 1.5 {
        hold = atmosphere_drawn(dir) * cloud_aureole(dir);
    }
    let dims = vec2<i32>(textureDimensions(sky_behind));
    var behind = textureLoad(sky_behind, clamp(vec2<i32>(frag), vec2<i32>(0), dims - 1), 0).rgb;
    let detail = fog_detail_share(distance(p, frame.eye.xyz), keep);
    if detail < 1.0 {
        let near_wide = smoothstep(cos(FOG_LOW_SUN), cos(FOG_SUN_OUTER), to_sun);
        let low = smooth_level(sky_behind, frag / vec2<f32>(dims), sky.fog_low.x).rgb;
        behind = mix(min(low, mix(vec3<f32>(FOG_SKY_CAP), hold, near_wide)), behind, detail);
    }
    let fogged = min(behind, mix(vec3<f32>(FOG_SKY_CAP), hold, near_sun));
    return fog_under_clouds(fogged, dir, detail);
}

// The volumetric clouds' image (sky.wgsl's: rgb their light, a their transmittance; marched with the unjittered camera).
@group(2) @binding(6) var cloud_image: texture_2d<f32>;

// The clouds are marched above the horizon only, so the sky behind a fogged surface under it holds none: the far ground toward a
// low sun met the clouds at the horizon in a hard line. Under the horizon the fog takes the clouds just above it (the same
// azimuth), eased out over HORIZON_CLOUDS radians below it. With the clouds and the sun's light through them only (sky.cloud_sun).
const HORIZON_CLOUDS: f32 = 0.15;
const HORIZON_CLOUD_SINE: f32 = 0.006;
fn fog_under_clouds(fogged: vec3<f32>, dir: vec3<f32>, detail: f32) -> vec3<f32> {
    if sky.cloud_sun.x <= 0.0 || dir.z >= HORIZON_CLOUD_SINE {
        return fogged;
    }
    let h = normalize(vec2<f32>(dir.x, dir.y) + vec2<f32>(1e-6, 0.0));
    let up = vec3<f32>(h * sqrt(1.0 - HORIZON_CLOUD_SINE * HORIZON_CLOUD_SINE), HORIZON_CLOUD_SINE);
    let clip = frame.unjit_view_proj * vec4<f32>(frame.eye.xyz + up * 1000.0, 1.0);
    if clip.w <= 1e-4 {
        return fogged;
    }
    let ndc = clip.xy / clip.w;
    let uv = vec2<f32>(0.5 + 0.5 * ndc.x, 0.5 - 0.5 * ndc.y);
    if any(uv < vec2<f32>(0.0)) || any(uv > vec2<f32>(1.0)) {
        return fogged;
    }
    // (The clouds without their detail, the image's low mip, but as much of it as fog_detail_share gives.)
    var c = textureSampleLevel(cloud_image, sky_linear, uv * sky.cloud_use.zw, 0.0);
    if detail < 1.0 {
        c = mix(smooth_level(cloud_image, uv * sky.cloud_use.zw, sky.fog_low.y), c, detail);
    }
    let w = 1.0 - smoothstep(0.0, HORIZON_CLOUDS, HORIZON_CLOUD_SINE - dir.z);
    return mix(fogged, fogged * clamp(c.a, 0.0, 1.0) + max(c.rgb, vec3<f32>(0.0)), w);
}

// The clouds between the eye and a surface (the eye in a cloud layer or over one, or a surface up in one): the sky pass draws the
// clouds behind the world, so a surface past a layer's slab would show through them. Where the ray to the surface runs through a
// volume layer's slab (sky.cloud_slabs: up to two, base and top each), the clouds' image there (their light and transmittance along
// that ray) is laid over it by that run's length over CLOUD_OVER_SPAN (none for a surface short of every slab: the ground under a
// deck a kilometre up stays clear).
const CLOUD_OVER_SPAN: f32 = 150.0;

// The length of the segment from `e` to `p` inside the heights `base` to `top`.
fn slab_run(e: vec3<f32>, p: vec3<f32>, base: f32, top: f32) -> f32 {
    if top <= base {
        return 0.0;
    }
    let len = distance(e, p);
    let dz = p.z - e.z;
    if abs(dz) < 1e-3 {
        return select(0.0, len, e.z > base && e.z < top);
    }
    let ta = clamp((base - e.z) / dz, 0.0, 1.0);
    let tb = clamp((top - e.z) / dz, 0.0, 1.0);
    return abs(tb - ta) * len;
}

// How much of the clouds' image lies over a surface at `p` (0 none).
fn clouds_over_share(p: vec3<f32>) -> f32 {
    if CLIP {
        return 0.0;
    }
    let e = frame.eye.xyz;
    let s = sky.cloud_slabs;
    let run = slab_run(e, p, s.x, s.y) + slab_run(e, p, s.z, s.w);
    return 1.0 - exp(-run / CLOUD_OVER_SPAN);
}

// The clouds' image at pixel `frag` (rgb their light, a their transmittance; marched with the unjittered camera).
fn clouds_at(frag: vec2<f32>) -> vec4<f32> {
    let uv = (frag - frame.temporal.xy) * frame.screen.zw * sky.cloud_use.zw;
    return textureSampleLevel(cloud_image, sky_linear, uv, 0.0);
}

// An unlit surface's gain: glow surfaces, self-illuminating shape parts, emitter sprites.
fn emissive_gain(material: u32) -> f32 {
    if (material & MAT_GLOW) != 0u {
        return sky.emissive.x;
    }
    if (material & MAT_EMITTER) != 0u {
        return sky.emissive.z;
    }
    let kind = (material >> MAT_KIND_SHIFT) & 3u;
    if kind == KIND_SHAPE && (material & MAT_UNLIT) != 0u {
        return sky.emissive.y;
    }
    return 1.0;
}

// A lit cloud layer's colour at `p` from its texture-modulated base (rgb: the texture's colour, a: its alpha).
fn lit_cloud(p: vec3<f32>, base: vec4<f32>) -> vec4<f32> {
    let dir = normalize(p - frame.eye.xyz);
    let el = asin(clamp(dir.z, -1.0, 1.0));
    let fade = clamp(el / max(sky.stars.z, 1e-3), 0.0, 1.0);
    let s = normalize(sky.drawn.xyz);
    let lobe = henyey_greenstein(dot(dir, s), 0.6) * 4.0 * SKY_PI;
    let up = smoothstep(-0.05, 0.1, s.z);
    let gain = min(mix(1.0, lobe, sky.sun_light.w * up), sky.vol_config.w);
    return vec4<f32>(base.rgb * gain, base.a * fade);
}
