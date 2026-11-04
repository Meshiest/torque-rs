// The volumetric clouds' shadow, appended to the modules that read it (the forward passes: the clouds' shadows on the ground
// at Ultra; the volumetric light: no god rays under a cloud; the sky: the glow about a covered sun): the sun's transmittance through
// the cloud slab toward a point, from the cloud shadow map (clouds.wgsl's fs_shadow: the base's plane around the eye). The point is
// carried along the sun to the base's plane; outside the map nothing. Under a low sun the map holds nothing (the ground's ray meets
// the deck tens of kilometres off): there the sun's transmittance as the eye sees it (clouds.wgsl's fs_sun_mean: rays across the
// sun's disc) takes its place, handed over between the sines sky.cloud_sun.yz of the sun's elevation.
//
// Needs the sky group's `sky` and `sky_linear` (skycommon.wgsl's Sky: cloud_map, cloud_use, cloud_sun).

@group(2) @binding(7) var cloud_shadow_tex: texture_2d<f32>;
@group(2) @binding(8) var cloud_sun_tex: texture_2d<f32>;

// How much the eye's view of the sun through the clouds counts (0 without clouds, or with the sun high enough for the map).
fn cloud_low_sun() -> f32 {
    if sky.cloud_sun.x <= 0.0 {
        return 0.0;
    }
    let s = normalize(sky.sun.xyz);
    return sky.cloud_sun.x * (1.0 - smoothstep(sky.cloud_sun.y, sky.cloud_sun.z, s.z));
}

// The sun's transmittance through the clouds as the eye sees it.
fn cloud_sun_seen() -> f32 {
    return textureLoad(cloud_sun_tex, vec2<i32>(0), 0).r;
}

// The share of the sun's light that reaches `p` through the clouds: the map's (its share `strength`; 0 no lookup) and, at a low sun,
// the eye's view of the sun.
fn cloud_sun(p: vec3<f32>, strength: f32) -> f32 {
    let low = cloud_low_sun();
    var t = 1.0;
    if low > 0.0 {
        t = mix(1.0, cloud_sun_seen(), low);
    }
    if strength <= 0.0 {
        return t;
    }
    let s = normalize(sky.sun.xyz);
    if s.z < 0.05 {
        return t;
    }
    let q = p.xy + s.xy * ((sky.cloud_map.w - p.z) / s.z);
    let uv = (q - sky.cloud_map.xy) * sky.cloud_map.z + 0.5;
    // Faded out toward the map's edge (no seam where it ends).
    let edge = min(min(uv.x, 1.0 - uv.x), min(uv.y, 1.0 - uv.y));
    if edge <= 0.0 {
        return t;
    }
    let m = textureSampleLevel(cloud_shadow_tex, sky_linear, uv, 0.0).r;
    // (Where the eye's view counts wholly, the map gives way to it.)
    let hand = select(1.0, 1.0 - low / sky.cloud_sun.x, sky.cloud_sun.x > 0.0);
    return t * mix(1.0, m, strength * clamp(edge * 16.0, 0.0, 1.0) * hand);
}

// The share of the atmosphere's light toward `dir` the clouds leave (the glow about the sun is the air the sun lights: under a
// covered low sun it fades with it; none of it past sky.cloud_sun.w radians from the sun).
fn cloud_aureole(dir: vec3<f32>) -> f32 {
    let low = cloud_low_sun();
    if low <= 0.0 {
        return 1.0;
    }
    let s = normalize(sky.sun.xyz);
    let ang = acos(clamp(dot(normalize(dir), s), -1.0, 1.0));
    let near = 1.0 - smoothstep(0.0, sky.cloud_sun.w, ang);
    return mix(1.0, cloud_sun_seen(), low * near);
}
