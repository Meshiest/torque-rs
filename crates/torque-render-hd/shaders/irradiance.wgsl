// The diffuse ambient's irradiance (compute): the environment cubes projected on the nine real L2 spherical harmonics and
// convolved with the clamped cosine (Ramamoorthi and Hanrahan 2001), as the ambient's radiance (irradiance over pi) the forward
// pass reads (`forward.wgsl`'s `sky_irradiance`).
//
// - cs_faces: one workgroup a chunk of CHUNK texels of a cube face (x the face, y the cube: 0 the sky cube, 1 the light capture,
//   z the chunk, CHUNKS of them), only the faces `params.todo.x` names (bits 0-5 the sky cube's, 6-11 the capture's: the faces
//   drawn this frame); the others keep their sums. The chunk is read into the workgroup's memory (each texel's radiance times its
//   solid angle, its class, its direction), then each of the 28 sums is added up over it by LANES threads. A chunk's sums: the
//   sky's radiance on each basis function, the capture's geometry's (a texel whose alpha holds a distance, 2 or more; no
//   brighter than SCENE_LIGHT_MAX), the basis over the texels taken as the ground, and the solid angles' sum.
// - cs_resolve: the chunks' sums (RESOLVE_LANES threads a sum) into the main view's and the sky's coefficients, a thread a
//   coefficient. The sky's radiance becomes light by the physical sky's scale (`params.mode.x`), or (0 there: the ratio) by its
//   luminance alone times the scene's ambient over what the horizontal normals take on average, so they take the scene's ambient.
//   With `params.mode.z` the sky below the horizon is a ground of `params.ground`'s albedo lit by the sun and the sky above.
//   With the capture (`params.mode.y`) the main view's take its sums (the sky it sees through the scene's gaps, the ground
//   below them and the scene around the eye, whose radiance is the scene's light already) and the secondary views' its gaps
//   alone: the scene around the eye blocks the sky for the capture's own faces too (a closed room bounces no sky light), and
//   the capture is not lit by itself (no feedback: a light that changes settles once its faces are drawn). Without it both
//   are the sky cube's. The secondary views are drawn before this pass, so they read the last frame's (zero on the first).
//
// Prepended: cubeface.wgsl.

struct Params {
    // x: the sky's light per unit of its drawn radiance (0: the ratio), y: 1 when the main view takes the capture's sums, z: 1
    // when the sky below the horizon is the ground.
    mode: vec4<f32>,
    // rgb: the scene's ambient (linear), the horizontal normals' mean light under the ratio.
    ambient: vec4<f32>,
    // rgb: the sun's light (linear) on a face toward it; w: towards the sun's z.
    sun: vec4<f32>,
    // rgb: the ground's albedo.
    ground: vec4<f32>,
    // x: the faces to project (bits 0-5 the sky cube's, 6-11 the capture's).
    todo: vec4<u32>,
};

// A face's sums: 0-8 the sky's radiance on each basis function, 9-17 the capture's geometry's, 18-26 (x) the basis over the
// ground, 27 the solid angles' sum (x) and that of the texels not of the scene (y: the open share).
struct Sums {
    c: array<vec4<f32>, 28>,
};

// 0-8 the main view's coefficients, 9-17 the sky's (rgb: the ambient's radiance).
struct Irradiance {
    sh: array<vec4<f32>, 18>,
};

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var sky_cube: texture_2d_array<f32>;
@group(0) @binding(2) var capture_cube: texture_2d_array<f32>;
// Each face's chunks' sums: [(cube x 6 + face) x CHUNKS + chunk].
@group(0) @binding(3) var<storage, read_write> sums: array<Sums, 192>;
@group(0) @binding(4) var<storage, read_write> out: Irradiance;

const SUMS: u32 = 28u;
// The texels a chunk reads (a thread each), a face's chunks at most (64 x 64 texels), and the threads that add up each sum over a
// chunk (SUMS x LANES of them).
const CHUNK: u32 = 256u;
const CHUNKS: u32 = 16u;
const LANES: u32 = 9u;
// The threads that add up each sum over the chunks in cs_resolve (SUMS x RESOLVE_LANES of its 256).
const RESOLVE_LANES: u32 = 8u;
// A capture texel's alpha over this holds a distance (2 + the distance: the scene), at or under it the sky (1 or less).
const GEOMETRY_ALPHA: f32 = 1.5;
// The brightest a capture texel of the scene lights (luminance): a lit surface (a white one in the sun is about 1.2) passes,
// an emitter's surface (glow and lit panels: their light is the scene's lights' already) is held to it.
const SCENE_LIGHT_MAX: f32 = 1.5;
// A chunk entry's kind (its light's w).
const SKY: f32 = 0.0;
const GEOMETRY: f32 = 1.0;
const GROUND: f32 = 2.0;
const NONE: f32 = 3.0;
const IRR_PI: f32 = 3.14159265358979;
// The largest finite half float.
const HALF_MAX: f32 = 65504.0;

// A chunk: each texel's radiance times its solid angle (rgb) and kind (w), its direction (xyz) and solid angle (w).
var<workgroup> light: array<vec4<f32>, CHUNK>;
var<workgroup> dirs: array<vec4<f32>, CHUNK>;
// The lanes' sums.
var<workgroup> lanes: array<vec4<f32>, CHUNK>;
// cs_resolve's: the two cubes' sums.
var<workgroup> cubes: array<array<vec4<f32>, SUMS>, 2>;

// The real SH basis function `i` at unit direction `d` (the order of torque_render_scene::hd::sh::basis).
fn sh_one(i: u32, d: vec3<f32>) -> f32 {
    switch i {
        case 0u: {
            return 0.2820948;
        }
        case 1u: {
            return 0.4886025 * d.y;
        }
        case 2u: {
            return 0.4886025 * d.z;
        }
        case 3u: {
            return 0.4886025 * d.x;
        }
        case 4u: {
            return 1.0925484 * d.x * d.y;
        }
        case 5u: {
            return 1.0925484 * d.y * d.z;
        }
        case 6u: {
            return 0.3153916 * (3.0 * d.z * d.z - 1.0);
        }
        case 7u: {
            return 1.0925484 * d.x * d.z;
        }
        default: {
            return 0.5462742 * (d.x * d.x - d.y * d.y);
        }
    }
}

// The clamped cosine's band of coefficient `i` over pi: 1, 2/3, 1/4.
fn band(i: u32) -> f32 {
    if i == 0u {
        return 1.0;
    }
    if i < 4u {
        return 2.0 / 3.0;
    }
    return 0.25;
}

fn irr_luminance(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
}

@compute @workgroup_size(256)
fn cs_faces(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_index) t: u32) {
    let face = wg.x;
    let cube = wg.y;
    let slot = cube * 6u + face;
    // (Uniform across the workgroup: the barriers below stay in uniform control flow.)
    if ((params.todo.x >> slot) & 1u) == 0u {
        return;
    }
    let chunk = slot * CHUNKS + wg.z;
    let size = select(textureDimensions(sky_cube).x, textureDimensions(capture_cube).x, cube == 1u);
    let count = size * size;
    // This thread's sum (k: 0-8 the sky's, 9-17 the geometry's, 18-26 the ground's basis, 27 the solid angles) over its lane.
    let k = t / LANES;
    let lane = t % LANES;
    let i = k % 9u;
    let wanted = select(SKY, GEOMETRY, k >= 9u);
    var acc = vec3<f32>(0.0);
    {
        let n = wg.z * CHUNK + t;
        if n < count {
            let x = n % size;
            let y = n / size;
            let uv = (vec2<f32>(f32(x), f32(y)) + 0.5) / f32(size) * 2.0 - 1.0;
            let raw = face_dir(face, uv);
            let r2 = dot(raw, raw);
            // The texel's solid angle (its area over the squared distance, slanted by the cosine).
            let w = 4.0 / (f32(count) * r2 * sqrt(r2));
            let d = raw * inverseSqrt(r2);
            var texel: vec4<f32>;
            if cube == 0u {
                texel = textureLoad(sky_cube, vec2<u32>(x, y), face, 0);
            } else {
                texel = textureLoad(capture_cube, vec2<u32>(x, y), face, 0);
            }
            var kind = SKY;
            // A texel that is not a finite, non-negative half (an overflow, a NaN) adds nothing: one would poison the face's sums
            // until it is drawn again.
            let finite = all(texel.rgb >= vec3<f32>(0.0)) && all(texel.rgb <= vec3<f32>(HALF_MAX));
            var rgb = select(vec3<f32>(0.0), texel.rgb, finite);
            if cube == 1u && texel.a > GEOMETRY_ALPHA {
                kind = GEOMETRY;
                rgb = rgb * min(1.0, SCENE_LIGHT_MAX / max(irr_luminance(rgb), 1e-6));
            } else if params.mode.z > 0.5 && d.z < 0.0 {
                kind = GROUND;
            }
            light[t] = vec4<f32>(rgb * w, kind);
            dirs[t] = vec4<f32>(d, w);
        } else {
            light[t] = vec4<f32>(0.0, 0.0, 0.0, NONE);
            dirs[t] = vec4<f32>(0.0);
        }
        workgroupBarrier();
        if k < SUMS {
            for (var j = lane; j < CHUNK; j = j + LANES) {
                let c = light[j];
                let dw = dirs[j];
                if k == 27u {
                    acc.x = acc.x + dw.w;
                    if c.w != GEOMETRY {
                        acc.y = acc.y + dw.w;
                    }
                } else if k >= 18u {
                    if c.w == GROUND {
                        acc.x = acc.x + sh_one(i, dw.xyz) * dw.w;
                    }
                } else if c.w == wanted {
                    acc = acc + c.rgb * sh_one(i, dw.xyz);
                }
            }
        }
        workgroupBarrier();
    }
    lanes[t] = vec4<f32>(acc, 0.0);
    workgroupBarrier();
    if t < SUMS {
        var sum = vec4<f32>(0.0);
        for (var l = 0u; l < LANES; l = l + 1u) {
            sum = sum + lanes[t * LANES + l];
        }
        sums[chunk].c[t] = sum;
    }
}

// The light of a sum of the sky's radiance: times the scale, or (the ratio) its luminance times the scale.
fn sky_light(c: vec3<f32>, scale: vec3<f32>, ratio: bool) -> vec3<f32> {
    return select(c, vec3<f32>(irr_luminance(c)), ratio) * scale;
}

@compute @workgroup_size(256)
fn cs_resolve(@builtin(local_invocation_index) t: u32) {
    let bounce = params.mode.y > 0.5;
    // Each cube's chunks summed: RESOLVE_LANES threads a sum, then their lanes.
    let k = t / RESOLVE_LANES;
    let lane = t % RESOLVE_LANES;
    for (var c = 0u; c < 2u; c = c + 1u) {
        var sum = vec4<f32>(0.0);
        if k < SUMS && (c == 0u || bounce) {
            for (var e = lane; e < 6u * CHUNKS; e = e + RESOLVE_LANES) {
                sum = sum + sums[c * 6u * CHUNKS + e].c[k];
            }
        }
        lanes[t] = sum;
        workgroupBarrier();
        if t < SUMS {
            var whole = vec4<f32>(0.0);
            for (var l = 0u; l < RESOLVE_LANES; l = l + 1u) {
                whole = whole + lanes[t * RESOLVE_LANES + l];
            }
            cubes[c][t] = whole;
        }
        workgroupBarrier();
    }
    if t >= 9u {
        return;
    }
    // Each cube's sums by 4 pi over its solid angles' sum (the texels' own add to a little off the sphere's).
    let sky_norm = 4.0 * IRR_PI / max(cubes[0][27].x, 1e-6);
    let cap_norm = 4.0 * IRR_PI / max(cubes[1][27].x, 1e-6);
    let up = vec3<f32>(0.0, 0.0, 1.0);
    // The sky's scale (the ratio's: the mean of the horizontal normals' light takes the scene's ambient, as the fixed-function
    // ambient's shape is), and the base a black sky leaves under the ratio (the scene's ambient whatever the direction).
    let ratio = params.mode.x <= 0.0;
    var scale = vec3<f32>(params.mode.x);
    var base = vec3<f32>(0.0);
    if ratio {
        // The mean over the horizontal normals: Y00 and Y20 there (-0.3153916; the others average to 0 about the vertical).
        let y20 = band(6u) * irr_luminance(cubes[0][6].rgb) * 0.3153916;
        let level = (irr_luminance(cubes[0][0].rgb) * 0.2820948 - y20) * sky_norm;
        if level > 1e-6 {
            scale = params.ambient.rgb / level;
        } else {
            scale = vec3<f32>(0.0);
            base = params.ambient.rgb / 0.2820948;
        }
    }
    // A level face's light from the sky above it (the sky cube's), and the ground's radiance it lights with the sun.
    var level_light = base * 0.2820948;
    for (var j = 0u; j < 9u; j = j + 1u) {
        level_light = level_light + band(j) * sky_light(cubes[0][j].rgb * sky_norm, scale, ratio) * sh_one(j, up);
    }
    let ground = params.ground.rgb * (params.sun.rgb * max(params.sun.w, 0.0) + level_light);
    // w (of each set's first coefficient): 1 where the sky's lower half is the sky as drawn (no ground, no capture), which the
    // forward pass shares with the extension's ground bounce; else the SH holds the ground's light already.
    let drawn_below = select(0.0, 1.0, t == 0u && params.mode.z < 0.5 && !bounce);
    if !bounce {
        let lead = select(vec3<f32>(0.0), base, t == 0u);
        let sky = sky_light(cubes[0][t].rgb * sky_norm, scale, ratio);
        let own = band(t) * (sky + ground * cubes[0][18u + t].x * sky_norm) + lead;
        out.sh[t] = vec4<f32>(own, drawn_below);
        out.sh[9u + t] = vec4<f32>(own, drawn_below);
        return;
    }
    // The capture: the sky it sees through the scene's gaps and the ground below them (the secondary views': the scene
    // around the eye blocks the rest, so a closed room takes no sky's light, and its own light is not lit by itself), and the
    // scene's light besides (the main view's). The black sky's base by the share of the capture that is open.
    let open = cubes[1][27].y / max(cubes[1][27].x, 1e-6);
    let lead = select(vec3<f32>(0.0), base * open, t == 0u);
    let gaps = sky_light(cubes[1][t].rgb, scale, ratio) + ground * cubes[1][18u + t].x;
    out.sh[9u + t] = vec4<f32>(band(t) * gaps * cap_norm + lead, 0.0);
    out.sh[t] = vec4<f32>(band(t) * (gaps + cubes[1][9u + t].rgb) * cap_norm + lead, 0.0);
}
