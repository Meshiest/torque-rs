// The enhanced renderer's volumetric light: the sun's light a thin medium scatters toward the eye, shadowed by the sun's
// cascades: god rays.
//
// - fs_march, at a reduced size (one texel per `sky.vol_config.z` internal pixels a side): the view ray through the texel's centre
//   pixel, from the eye to its surface (the prepass's depth; the sky: the reach), no farther than the medium's reach and the shadows'
//   distance, in `vol_config.y` steps offset by a fixed per-pixel dither (interleaved gradient noise of the texel: the same every frame,
//   so the result is still when the camera is). Each step: the medium (constant, or with the height fog falling off with
//   height as it does), its transmittance so far, and the sun's visibility from the cascade that holds the point (one hardware 2 x 2 comparison).
//   The in-scatter is pi times the Henyey-Greenstein phase toward the sun times the sun's light (the radiance convention of the forward
//   pass) times the medium's strength. rgb: that light; a: the centre pixel's eye distance (the upsample's depth test). The sun's
//   light is also dimmed by the volumetric clouds' shadow (no god rays under a cloud; cloudshadow.wgsl).
// - fs_composite, at the internal size, added over the opaque frame: the four nearest texels weighted bilinearly and by how close their
//   eye distance is to the pixel's (no near texel: the nearest one's).
//
// The march only ever samples the air in front of a surface, so a closed room's inside gets none of the sun's light outside it.
// Prepended: common.wgsl, skycommon.wgsl.

@group(0) @binding(0) var<uniform> frame: Frame;
@group(0) @binding(4) var<uniform> shadows: Shadows;
@group(0) @binding(5) var csm: texture_depth_2d_array;
@group(0) @binding(6) var shadow_cmp: sampler_comparison;
@group(1) @binding(0) var depth_tex: texture_depth_2d;
@group(1) @binding(1) var vol_tex: texture_2d<f32>;
@group(2) @binding(1) var<uniform> sky: Sky;
@group(2) @binding(2) var sky_view_lut: texture_2d<f32>;
@group(2) @binding(3) var transmittance_lut: texture_2d<f32>;
@group(2) @binding(4) var sky_linear: sampler;

// The world point of an internal pixel at reversed depth d (the unjittered inverse: half a pixel off at most).
fn world_at(px: vec2<f32>, d: f32) -> vec3<f32> {
    let ndc = vec2<f32>(px.x * frame.screen.z * 2.0 - 1.0, 1.0 - px.y * frame.screen.w * 2.0);
    let w = frame.inv_unjit_view_proj * vec4<f32>(ndc, d, 1.0);
    return w.xyz / w.w;
}

// The sun's visibility at p from the first cascade that holds it (1 outside them all, or with no sun shadows).
fn sun_visible(p: vec3<f32>) -> f32 {
    let count = min(shadows.config.x, MAX_CASCADES);
    for (var i = 0u; i < count; i = i + 1u) {
        let c = shadows.cascades[i] * vec4<f32>(p, 1.0);
        let uv = vec2<f32>(c.x * 0.5 + 0.5, 0.5 - c.y * 0.5);
        if all(uv > vec2<f32>(0.002)) && all(uv < vec2<f32>(0.998)) && c.z < 1.0 && c.z > 0.0 {
            let bias = 2.0 * shadows.params[i].y / max(shadows.params[i].z, 1e-3);
            return textureSampleCompareLevel(csm, shadow_cmp, uv, i32(i), c.z - bias);
        }
    }
    return 1.0;
}

fn density(z: f32) -> f32 {
    if sky.height_fog.w > 0.0 {
        return sky.vol.x * exp(-max(z - sky.height_fog.z, 0.0) / max(sky.height_fog.y, 1e-3));
    }
    return sky.vol.x;
}

@fragment
fn fs_march(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let scale = max(sky.vol_config.z, 1.0);
    let dims = vec2<i32>(textureDimensions(depth_tex));
    let px = min(vec2<i32>(floor(frag.xy) * scale + floor(scale * 0.5)), dims - 1);
    let d = textureLoad(depth_tex, px, 0);
    let centre = vec2<f32>(px) + 0.5;
    let eye = frame.eye.xyz;
    var end: vec3<f32>;
    var dist: f32;
    if d <= 0.0 {
        let far = world_at(centre, 1e-7);
        let dir = normalize(far - eye);
        dist = 1e9;
        end = eye + dir;
    } else {
        end = world_at(centre, d);
        dist = distance(end, eye);
    }
    let dir = normalize(end - eye);
    let shadow_reach = select(sky.vol.w, shadows.sun.y, shadows.config.x > 0u);
    let reach = min(min(dist, sky.vol.w), shadow_reach);
    let steps = max(u32(sky.vol_config.y), 1u);
    let dt = reach / f32(steps);
    let jitter = ign(floor(frag.xy));
    var tr = 1.0;
    var lit = 0.0;
    for (var i = 0u; i < steps; i = i + 1u) {
        let t = (f32(i) + jitter) * dt;
        let p = eye + dir * t;
        let sigma = density(p.z);
        let vis = sun_visible(p) * cloud_sun(p, sky.cloud_use.y);
        lit = lit + tr * sigma * vis * dt;
        tr = tr * exp(-sigma * dt);
    }
    let phase = henyey_greenstein(dot(dir, normalize(sky.drawn.xyz)), sky.vol.y);
    let light = sky.sun_light.rgb * (SKY_PI * phase * sky.vol.z * lit);
    return vec4<f32>(light, select(dist, 1e9, d <= 0.0));
}

@fragment
fn fs_composite(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let scale = max(sky.vol_config.z, 1.0);
    let d = textureLoad(depth_tex, vec2<i32>(frag.xy), 0);
    var lin = 1e9;
    if d > 0.0 {
        lin = distance(world_at(frag.xy, d), frame.eye.xyz);
    }
    let dims = vec2<i32>(textureDimensions(vol_tex));
    let u = (frag.xy - 0.5 * scale) / scale;
    let base = vec2<i32>(floor(u));
    let f = u - floor(u);
    var sum = vec3<f32>(0.0);
    var weight = 0.0;
    var nearest = vec3<f32>(0.0);
    var best = 1e30;
    for (var k = 0u; k < 4u; k = k + 1u) {
        let o = vec2<i32>(i32(k & 1u), i32(k >> 1u));
        let q = clamp(base + o, vec2<i32>(0), dims - 1);
        let t = textureLoad(vol_tex, q, 0);
        let bw = select(1.0 - f.x, f.x, o.x == 1) * select(1.0 - f.y, f.y, o.y == 1);
        let gap = abs(min(t.a, 1e6) - min(lin, 1e6));
        let tol = 0.05 * min(lin, 1e6) + 0.05;
        let dw = clamp(1.0 - gap / tol, 0.0, 1.0);
        let w = (bw + 1e-3) * dw;
        sum = sum + t.rgb * w;
        weight = weight + w;
        if gap < best {
            best = gap;
            nearest = t.rgb;
        }
    }
    var c = nearest;
    if weight > 1e-4 {
        c = sum / weight;
    }
    return vec4<f32>(c, 0.0);
}
