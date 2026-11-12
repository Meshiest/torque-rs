// The reflections the forward pass and the water read: group 3 of their pipelines. Our own code from the published
// techniques named where they are used. Prepended to forward.wgsl (after common.wgsl).
//
// - The environment: the camera-local light capture (a cube of the opaque scene drawn around the eye, one face a frame) at High
//   and up, else the sky cube (the sky's draws around the eye); their mips are GGX-prefiltered (prefilter.wgsl) and a rough
//   surface reads the level of its roughness (brdf.wgsl's env_lod).
// - Screen-space reflections: ssr.wgsl's accumulated result (premultiplied colour and its hit weight), sampled bilinearly at the
//   pixel; where it misses (weight below 1) the environment shows through.
// - The environment BRDF that weighs them is brdf.wgsl's.
// - Binding 5 is the extension's (its chunk declares it).
// - The clouds' cube (binding 8; clouds.rs): the volumetric clouds around the eye at a low resolution (rgb their light, a their
//   transmittance), read where the main view's image of them does not reach.

@group(3) @binding(0) var<uniform> refl: Refl;
@group(3) @binding(1) var ssr_tex: texture_2d<f32>;
@group(3) @binding(2) var capture_cube: texture_cube<f32>;
@group(3) @binding(3) var sky_cube: texture_cube<f32>;
@group(3) @binding(4) var refl_samp: sampler;
@group(3) @binding(6) var planar_tex: texture_2d<f32>;
@group(3) @binding(7) var refract_depth: texture_depth_2d;
@group(3) @binding(8) var cloud_cube: texture_cube<f32>;

// The clouds' cube's side over the sky cube's (texels; a mip of the one is as wide as the other's this many levels down).
const CLOUD_CUBE_SHIFT: f32 = 2.0;

// The clouds toward `r` from the clouds' cube at the sky cube's mip `lod` (rgb their light, a their transmittance); none where the
// cube does not hold them.
fn cube_clouds(r: vec3<f32>, lod: f32) -> vec4<f32> {
    if refl.capture.w < 0.5 || r.z <= 0.0 {
        return vec4<f32>(0.0, 0.0, 0.0, 1.0);
    }
    return textureSampleLevel(cloud_cube, refl_samp, r, max(lod - CLOUD_CUBE_SHIFT, 0.0));
}


// The sky cube in direction `r` at roughness `rough`.
fn sky_radiance(r: vec3<f32>, rough: f32) -> vec3<f32> {
    let lod = select(env_lod(rough, refl.capture.z), 0.0, refl.mode.w > 0.5);
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
    let lod = select(env_lod(rough, refl.capture.z), 0.0, refl.mode.w > 0.5);
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
