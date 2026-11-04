// The reflections the forward pass and the water read: group 3 of their pipelines. Our own code from the published
// techniques named where they are used. Prepended to forward.wgsl (after common.wgsl).
//
// - The environment: the camera-local light capture (a cube of the opaque scene drawn around the eye, one face a frame) at High
//   and up, else the sky cube (the sky's draws around the eye); a rough surface reads a blurrier mip (roughness times the levels).
// - Screen-space reflections: ssr.wgsl's accumulated result (premultiplied colour and its hit weight), sampled bilinearly at the
//   pixel; where it misses (weight below 1) the environment shows through.
// - The split-sum's environment BRDF in closed form (Karis 2014, "Physically Based Shading on Mobile", after Lazarov 2013).
// - Binding 5 is the extension's (its chunk declares it).

@group(3) @binding(0) var<uniform> refl: Refl;
@group(3) @binding(1) var ssr_tex: texture_2d<f32>;
@group(3) @binding(2) var capture_cube: texture_cube<f32>;
@group(3) @binding(3) var sky_cube: texture_cube<f32>;
@group(3) @binding(4) var refl_samp: sampler;
@group(3) @binding(6) var planar_tex: texture_2d<f32>;
@group(3) @binding(7) var refract_depth: texture_depth_2d;


// The sky cube in direction `r` at roughness `rough`.
fn sky_radiance(r: vec3<f32>, rough: f32) -> vec3<f32> {
    let lod = select(rough * (refl.capture.z - 1.0), 0.0, refl.mode.w > 0.5);
    return textureSampleLevel(sky_cube, refl_samp, r, lod).rgb;
}

// The light capture's face a direction falls in (+x, -x, +y, -y, +z, -z).
fn cube_face(d: vec3<f32>) -> u32 {
    let a = abs(d);
    if a.x >= a.y && a.x >= a.z {
        return select(1u, 0u, d.x >= 0.0);
    }
    if a.y >= a.z {
        return select(3u, 2u, d.y >= 0.0);
    }
    return select(5u, 4u, d.z >= 0.0);
}

// The light capture's direction showing what lies along `r` from this view's eye: its faces are drawn one a frame, each from the eye
// of its own frame, so as the eye moves a face's content stood still between its draws and jumped at each (the reflections shook
// while walking and jumping). Each face's texels hold the distance of what they show: the point along `r` at that distance is
// looked up from the face's own eye, twice (the second distance from the face that point falls in). The sky (no distance) and a
// face drawn this frame need no change; a still eye's lookups are the plain ones.
fn capture_dir(r: vec3<f32>, lod: f32) -> vec3<f32> {
    var d = r;
    for (var i = 0; i < 2; i = i + 1) {
        let shift = refl.capture_shift[cube_face(d)].xyz;
        if dot(shift, shift) < 1e-8 {
            break;
        }
        // At the colour's own level: a coarser one moved silhouettes by its mean distance.
        let a = textureSampleLevel(capture_cube, refl_samp, d, lod).a;
        if a < 2.0 {
            break;
        }
        d = r * max(a - 2.0, 0.1) + shift;
    }
    return d;
}

// The environment in direction `r`: the light capture when it holds the scene, else the sky cube.
fn env_radiance(r: vec3<f32>, rough: f32) -> vec3<f32> {
    let lod = select(rough * (refl.capture.z - 1.0), 0.0, refl.mode.w > 0.5);
    if refl.mode.z > 0.5 {
        return textureSampleLevel(capture_cube, refl_samp, capture_dir(r, lod), lod).rgb;
    }
    return textureSampleLevel(sky_cube, refl_samp, r, lod).rgb;
}

// The screen-space reflection at pixel `frag` (internal pixels, jittered; the SSR texels hold the frame without the jitter): rgb its
// colour, a its weight (0 where it missed).
fn ssr_at(frag: vec2<f32>) -> vec4<f32> {
    let s = textureSampleLevel(ssr_tex, refl_samp, (frag - frame.temporal.xy) * frame.screen.zw, 0.0);
    if s.a < 1e-3 {
        return vec4<f32>(0.0);
    }
    return vec4<f32>(s.rgb / s.a, clamp(s.a, 0.0, 1.0));
}

// What a surface at pixel `frag` reflects toward the eye along `r` (unit, world): the screen-space reflection in the main view, the
// environment where it misses.
fn reflection(frag: vec2<f32>, r: vec3<f32>, rough: f32) -> vec3<f32> {
    var c = env_radiance(r, rough);
    if refl.mode.x > 0.5 && frame.flags.y < 0.5 {
        let s = ssr_at(frag);
        c = mix(c, s.rgb, s.a);
    }
    return c;
}

// The split-sum's environment BRDF (Karis 2014): the share of the environment a surface of reflectance `f0` and roughness `rough`
// reflects at N.V `nv`.
fn env_brdf(f0: vec3<f32>, rough: f32, nv: f32) -> vec3<f32> {
    let c0 = vec4<f32>(-1.0, -0.0275, -0.572, 0.022);
    let c1 = vec4<f32>(1.0, 0.0425, 1.04, -0.04);
    let r = rough * c0 + c1;
    let a004 = min(r.x * r.x, exp2(-9.28 * nv)) * r.x + r.y;
    let ab = vec2<f32>(-1.04, 1.04) * a004 + r.zw;
    return f0 * ab.x + ab.y;
}

