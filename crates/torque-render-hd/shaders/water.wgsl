// The water: a scene's blended water plane drawn as water in the translucent pass, replacing what is under it (the opaque
// frame's copy) rather than blending over it. Our own code; appended to forward.wgsl (with common.wgsl and reflect.wgsl).
//
// - The surface's normal: our own small sum of Gerstner waves (the normals of the form in Tessendorf 2001 / Finch 2004; the plane is
//   not displaced), two octaves of our own value noise scrolling with the effect time, and the water texture's luminance as a
//   bump (a surface gradient from the screen derivatives, Mikkelsen 2010). The gates pin the time at 0. So that the surface does not
//   repeat: eight waves with no common period, each broken into short-crested groups by a slow noise on its phase, their strength
//   varied across the surface by two slow fields (calm and rough patches), the ripples' noise turned off its lattice; each wave
//   fades out by its own footprint along its direction (not the pixel's longest), and the slope that fades out widens the
//   glint's lobe instead.
// - Under the surface: the opaque frame's copy, offset by the normal (refraction) where what is there is still behind the water, seen
//   through the water's colour by Beer-Lambert (transmittance exp(-sigma d), sigma from the colour's linear channels, d the path in
//   the water from the depth copy). Without clear water the plane's own face stays on top: the texture mixed into the plane's
//   colour (DECAL or MODULATE), lit as the fixed-function pipeline lights it, blended in gamma space by the plane's alpha over the
//   refracted floor. With clear water the lit colour is scattered into the path instead, with the texture as a light detail and foam where the water is shallow.
// - Above it: the reflection by Schlick's Fresnel (F0 0.02): the planar reflection (High and up; the uv offset by the normal), the
//   screen-space reflection over the sky cube (Medium), the sky cube (Low); none with the eye under the water. The sun's glint: GGX on
//   the wave normal at roughness 0.06, through the sun's shadow.
// - A water the extension draws a face for (`ext_water_face`, where `ext_water_faced` says so): that face's normal, occlusion and
//   sun on the water's colour, the texture neither mixed in nor bumped. The face shades the water's colour; the refraction, the
//   reflection's offset, its Fresnel and the glint follow the waves alone (the face is painted relief, not a mirror's facets).
// - Close up: the screen derivatives are the fine ones (per pixel, not per 2 x 2 quad) and the planar reflection is read through a
//   cubic B-spline (it is half the frame's size at High); the texture's bump fades out where its texels are larger than a pixel (a
//   magnified bilinear texture's slope is flat over each texel: facets); the refraction's offset stops growing nearer than
//   REFRACTION_NEAR units, is held to REFRACTION_MAX of the frame's height, fades to none where the water is thinner than
//   REFRACTION_THIN (an object standing in it does not smear), halves and then drops where the offset point is in front of the
//   water, and the opaque copy is read bilinearly.

const WATER_PI2: f32 = 6.2831853;
// The waves, no two with a common period (wavelengths about 1.44 apart, none a ratio of small whole numbers; directions spread
// about the wind's, 0.53 rad from x, none on the axes): x, y the direction, z the wavelength (units), w the slope's amplitude
// (k a).
const WAVES = array<vec4<f32>, 8>(
    vec4<f32>(0.8628, 0.5055, 12.7, 0.012),
    vec4<f32>(0.4176, 0.9086, 8.9, 0.016),
    vec4<f32>(0.9780, -0.2085, 6.1, 0.022),
    vec4<f32>(0.9982, 0.0600, 4.3, 0.028),
    vec4<f32>(-0.0891, 0.9960, 2.97, 0.032),
    vec4<f32>(0.6822, 0.7311, 2.07, 0.034),
    vec4<f32>(0.9211, -0.3894, 1.43, 0.034),
    vec4<f32>(0.2579, 0.9662, 0.97, 0.032),
);
// The share of each wave's strength from the gusts' field (the rest from the swell's).
const WAVE_GUST = array<f32, 8>(0.1, 0.2, 0.35, 0.5, 0.65, 0.8, 0.9, 0.95);
const WAVE_STEEPNESS: f32 = 0.5;
// Each wave is broken into short-crested groups: its phase moved by a slow noise of up to PACKET_REACH cycles over
// PACKET_SPAN of its wavelengths, the groups travelling at the deep-water group speed (half the crests'). A sum of a few plain
// sines lines up again at some shift and the eye finds that as bands; groups lose step with each other within a few wavelengths.
const PACKET_SPAN: f32 = 2.6;
const PACKET_REACH: f32 = 0.6;
// The most a group's phase noise shortens a wave (its fade takes the shortest).
const PACKET_SHORTEN: f32 = 1.5;
// The slow change across the surface: the waves' strength from STRENGTH_LOW to STRENGTH_LOW + STRENGTH_SPAN of their own by the
// swell's field (SWELL_SCALE units; the long waves) and the gusts' (GUST_SCALE units; the short waves and the ripples), both
// drifting slowly with the time: calm and rough patches.
const SWELL_SCALE: f32 = 53.0;
const GUST_SCALE: f32 = 19.7;
const STRENGTH_LOW: f32 = 0.35;
const STRENGTH_SPAN: f32 = 1.3;
// A wave or ripple is whole while its wavelength spans FADE_WHOLE pixels along its direction and gone at FADE_GONE (shorter, it
// would alias into stripes); the slope it loses widens the sun's glint instead.
const FADE_WHOLE: f32 = 8.0;
const FADE_GONE: f32 = 3.0;
// The texture's bump (and the clear water's light detail) from two layers of it: the plane's UVs, and these turned 0.7 rad, at
// TEXTURE_B_SCALE of the scale and moved; crossing over by a field TEXTURE_MIX_SCALE units across, so that its tile does not repeat
// over the surface. The colour of a water that is not clear keeps the single layer.
const TEXTURE_B_TURN: mat2x2<f32> = mat2x2<f32>(0.7648, 0.6442, -0.6442, 0.7648);
const TEXTURE_B_SCALE: f32 = 0.613;
const TEXTURE_B_SHIFT: vec2<f32> = vec2<f32>(0.37, 0.19);
const TEXTURE_MIX_SCALE: f32 = 11.3;
// Beer-Lambert's density (per unit) with and without clear water, the refraction's reach (units), the foam's depth (units).
const WATER_DENSITY_TEXTURED: f32 = 0.25;
const WATER_DENSITY_CLEAR: f32 = 0.6;
const REFRACTION_REACH: f32 = 0.35;
const REFLECTION_REACH: f32 = 0.06;
// The refraction's limits (see the header): the nearest depth its offset grows to (units), the most it moves (a share of the
// frame's height), the water's thickness under which it fades out (units).
const REFRACTION_NEAR: f32 = 2.0;
const REFRACTION_MAX: f32 = 0.02;
const REFRACTION_THIN: f32 = 0.5;
// The texture bump's fade with magnification: texels a pixel over which it is whole, and under which it is gone.
const BUMP_WHOLE_TEXELS: f32 = 1.2;
const BUMP_GONE_TEXELS: f32 = 0.4;
// ... and its fade with minification: whole up to BUMP_FAR_WHOLE texels a pixel, gone from BUMP_FAR_GONE.
const BUMP_FAR_WHOLE: f32 = 3.0;
const BUMP_FAR_GONE: f32 = 8.0;
// ... and seen edge-on: whole from a view sine of BUMP_EDGE_WHOLE, gone under BUMP_EDGE_GONE.
const BUMP_EDGE_WHOLE: f32 = 0.12;
const BUMP_EDGE_GONE: f32 = 0.03;
const FOAM_DEPTH: f32 = 0.4;
// Water's reflectance at normal incidence (index 1.33) and its glint's roughness (hd::reflect's WATER_F0 and
// WATER_ROUGHNESS: a test compares them).
const WATER_F0: f32 = 0.02;
const WATER_ROUGHNESS: f32 = 0.06;

fn hash21(p: vec2<i32>) -> f32 {
    var h = u32(p.x) * 0x8da6b343u ^ u32(p.y) * 0xd8163841u;
    h = (h ^ (h >> 15u)) * 0x2c1b3c6du;
    h = (h ^ (h >> 12u)) * 0x297a2d39u;
    h = h ^ (h >> 15u);
    return f32(h & 0xffffffu) / 16777216.0;
}

// Value noise and its gradient (quintic fade).
fn noise_grad(p: vec2<f32>) -> vec3<f32> {
    let i = vec2<i32>(floor(p));
    let f = p - floor(p);
    let u = f * f * f * (f * (f * 6.0 - 15.0) + 10.0);
    let du = 30.0 * f * f * (f * (f - 2.0) + 1.0);
    let a = hash21(i);
    let b = hash21(i + vec2<i32>(1, 0));
    let c = hash21(i + vec2<i32>(0, 1));
    let d = hash21(i + vec2<i32>(1, 1));
    let v = a + (b - a) * u.x + (c - a) * u.y + (a - b - c + d) * u.x * u.y;
    let g = du * vec2<f32>(b - a + (a - b - c + d) * u.y, c - a + (a - b - c + d) * u.x);
    return vec3<f32>(v, g);
}

// The surface point's change a pixel across and a pixel down (x, y of each), worked out from the view rather than taken from the
// screen derivatives: a fine derivative is one value over each pair of pixels, and the waves' fade and the glint's width from it
// stepped in 2 x 2 blocks close to the water. On the plane z = p.z: dp = (depth / focal) (a - r a.z / r.z) for the camera's
// right (a = R) and down (a = -U) axes, r the ray from the eye.
fn plane_footprint(p: vec3<f32>) -> array<vec2<f32>, 2> {
    let r = p - frame.eye.xyz;
    let right = vec3<f32>(frame.view[0][0], frame.view[1][0], frame.view[2][0]);
    let down = -vec3<f32>(frame.view[0][1], frame.view[1][1], frame.view[2][1]);
    let forward = -vec3<f32>(frame.view[0][2], frame.view[1][2], frame.view[2][2]);
    let depth = max(dot(r, forward), 1e-4);
    let rz = select(min(r.z, -1e-4), max(r.z, 1e-4), r.z > 0.0);
    let fx = frame.proj[0][0] * 0.5 * frame.screen.x;
    let fy = frame.proj[1][1] * 0.5 * frame.screen.y;
    let ds = (depth / fx) * (right - r * (right.z / rz));
    let dt = (depth / fy) * (down - r * (down.z / rz));
    return array<vec2<f32>, 2>(ds.xy, dt.xy);
}

// The texture bump's share at `texels` texels a pixel: none where a texel is magnified past a pixel (a bilinear texel's slope is
// flat: facets) and none where many texels fall in a pixel (the mip's leftovers, and seen edge-on its fine derivatives one value
// over each pair of rows: blocks).
fn bump_share(texels: f32) -> f32 {
    return smoothstep(BUMP_GONE_TEXELS, BUMP_WHOLE_TEXELS, texels) * (1.0 - smoothstep(BUMP_FAR_WHOLE, BUMP_FAR_GONE, texels));
}

// The share of a wave or ripple kept where its wavelength `lambda` spans `along` units a pixel along its direction.
fn wave_keep(lambda: f32, along: f32) -> f32 {
    return smoothstep(FADE_GONE, FADE_WHOLE, lambda / max(along, 1e-6));
}

// The surface's normal from the waves and the ripples at `xy`, time `t`, with the screen derivatives of the surface point
// `ds`, `dt` (units a pixel); and the mean square slope the fading left out (the glint's roughness takes it).
struct Waves {
    n: vec3<f32>,
    lost: f32,
}

fn wave_field(xy: vec2<f32>, t: f32, ds: vec2<f32>, dt: vec2<f32>) -> Waves {
    // The slow fields: the swell's and the gusts' strengths, drifting.
    let swell = noise_grad(xy / SWELL_SCALE + vec2<f32>(0.011, 0.007) * t).x;
    let gust = noise_grad(xy / GUST_SCALE - vec2<f32>(0.013, 0.004) * t).x;
    var n = vec3<f32>(0.0, 0.0, 1.0);
    var lost = 0.0;
    for (var i = 0u; i < 8u; i = i + 1u) {
        let w = WAVES[i];
        let d = w.xy;
        let keep = wave_keep(w.z / PACKET_SHORTEN, max(abs(dot(d, ds)), abs(dot(d, dt))));
        let s = w.w * (STRENGTH_LOW + STRENGTH_SPAN * mix(swell, gust, WAVE_GUST[i]));
        lost = lost + 0.5 * s * s * (1.0 - keep * keep);
        // Far off most waves are gone: their groups and phase are not worked out.
        if keep <= 0.0 {
            continue;
        }
        let k = WATER_PI2 / w.z;
        // Deep water's dispersion (g = 9.8, a unit taken as a metre), slowed by half; the groups at half the crests' speed.
        let omega = 0.5 * sqrt(9.8 * k);
        let span = PACKET_SPAN * w.z;
        let seed = vec2<f32>(f32(i) * 17.31, f32(i) * -9.73);
        let group = noise_grad((xy + d * (0.5 * omega / k) * t) / span + seed);
        let reach = PACKET_REACH * WATER_PI2;
        let phase = k * dot(d, xy) + omega * t + reach * group.x;
        // The local wavenumber (the phase's gradient).
        let kv = k * d + reach * group.yz / span;
        let sk = s * keep;
        let c = cos(phase);
        n.x = n.x - kv.x / k * sk * c;
        n.y = n.y - kv.y / k * sk * c;
        n.z = n.z - WAVE_STEEPNESS * sk * sin(phase);
    }
    // The ripples: two octaves of value noise, each turned off the axes (its lattice's), stronger in the gusts.
    let foot = max(length(ds), length(dt));
    let gust_k = STRENGTH_LOW + STRENGTH_SPAN * gust;
    let ra = mat2x2<f32>(0.8776, 0.4794, -0.4794, 0.8776);
    let rb = mat2x2<f32>(0.6216, -0.7833, 0.7833, 0.6216);
    let ka = wave_keep(1.0 / 1.37, foot);
    let kb = wave_keep(1.0 / 3.3, foot);
    var a = vec2<f32>(0.0);
    var b = vec2<f32>(0.0);
    if ka > 0.0 {
        a = (noise_grad((ra * xy) * 1.37 + vec2<f32>(0.21, 0.13) * t).yz * ra) * 1.37;
    }
    if kb > 0.0 {
        b = (noise_grad((rb * xy) * 3.3 - vec2<f32>(0.17, 0.29) * t).yz * rb) * 3.3;
    }
    let sa = 0.012 * gust_k;
    let sb = 0.005 * gust_k;
    // A value noise's slope: about 1.2 times the frequency in mean square per axis.
    lost = lost + 2.4 * (sa * sa * 1.37 * 1.37 * (1.0 - ka * ka) + sb * sb * 3.3 * 3.3 * (1.0 - kb * kb));
    let slope = a * sa * ka + b * sb * kb;
    return Waves(normalize(vec3<f32>(n.xy - slope, n.z)), lost);
}

fn schlick(f0: f32, nv: f32) -> f32 {
    return f0 + (1.0 - f0) * pow(1.0 - clamp(nv, 0.0, 1.0), 5.0);
}

// shade() with the extension face's share of the sun.
fn shade_water(frag: vec2<f32>, p: vec3<f32>, n: vec3<f32>, albedo: vec3<f32>, material: u32, occlusion: f32, sun: f32) -> vec3<f32> {
    var s: Surf;
    s.albedo = albedo;
    s.alpha = 1.0;
    s.n = normalize(n);
    s.ng = vec3<f32>(0.0, 0.0, 1.0);
    s.rough = frame.surf2.x;
    s.f0 = vec3<f32>(frame.surf0.z);
    s.metal = 0.0;
    s.reflects = false;
    s.clearcoat = 0.0;
    s.occlusion = 1.0;
    s.sun = sun;
    s.rim = 0.0;
    s.linear = false;
    s.hollow = 0.0;
    s.spec_level = 1.0;
    let l = shade_surf(frag, p, p, s, material, false, occlusion, 0.0);
    return l.diffuse + l.spec;
}

// The opaque frame's copy at pixel position `q` (not a texel's centre), bilinearly.
fn background_at(q: vec2<f32>) -> vec3<f32> {
    let size = vec2<i32>(frame.screen.xy);
    let b = q - 0.5;
    let i = vec2<i32>(floor(b));
    let f = b - floor(b);
    let hi = size - 1;
    let c00 = textureLoad(background, clamp(i, vec2<i32>(0), hi), 0).rgb;
    let c10 = textureLoad(background, clamp(i + vec2<i32>(1, 0), vec2<i32>(0), hi), 0).rgb;
    let c01 = textureLoad(background, clamp(i + vec2<i32>(0, 1), vec2<i32>(0), hi), 0).rgb;
    let c11 = textureLoad(background, clamp(i + vec2<i32>(1, 1), vec2<i32>(0), hi), 0).rgb;
    return mix(mix(c00, c10, f.x), mix(c01, c11, f.x), f.y);
}

// The planar reflection at `uv` through a cubic B-spline (four bilinear taps): the target is half the frame's size at High, and a
// bilinear fetch of it, its slope changing at every texel, drew the reflection in 2 x 2 steps close up.
fn planar_at(uv: vec2<f32>) -> vec3<f32> {
    let size = vec2<f32>(textureDimensions(planar_tex));
    let t = uv * size - 0.5;
    let i = floor(t);
    let f = t - i;
    let f2 = f * f;
    let f3 = f2 * f;
    let w0 = (1.0 - 3.0 * f + 3.0 * f2 - f3) / 6.0;
    let w1 = (4.0 - 6.0 * f2 + 3.0 * f3) / 6.0;
    let w2 = (1.0 + 3.0 * f + 3.0 * f2 - 3.0 * f3) / 6.0;
    let w3 = f3 / 6.0;
    let g0 = w0 + w1;
    let g1 = w2 + w3;
    let p0 = (i - 1.0 + w1 / max(g0, vec2<f32>(1e-6)) + 0.5) / size;
    let p1 = (i + 1.0 + w3 / max(g1, vec2<f32>(1e-6)) + 0.5) / size;
    let a = textureSampleLevel(planar_tex, refl_samp, vec2<f32>(p0.x, p0.y), 0.0).rgb;
    let b = textureSampleLevel(planar_tex, refl_samp, vec2<f32>(p1.x, p0.y), 0.0).rgb;
    let c = textureSampleLevel(planar_tex, refl_samp, vec2<f32>(p0.x, p1.y), 0.0).rgb;
    let d = textureSampleLevel(planar_tex, refl_samp, vec2<f32>(p1.x, p1.y), 0.0).rgb;
    return g0.y * (g0.x * a + g1.x * b) + g1.y * (g0.x * c + g1.x * d);
}

// Whether the water's depth copy at pixel position `q` holds something behind the water's depth `lin_w` (else the offset point
// is in front of the water, or nothing is there).
fn behind_water(q: vec2<f32>, lin_w: f32) -> bool {
    let size = vec2<i32>(frame.screen.xy);
    let d = textureLoad(refract_depth, clamp(vec2<i32>(q), vec2<i32>(0), size - 1), 0);
    return d > 0.0 && linear_depth(d) >= lin_w;
}
@fragment
fn fs_water(in: VertexOut) -> @location(0) vec4<f32> {
    let material = in.material;
    let p = in.world;
    let up = vec3<f32>(0.0, 0.0, 1.0);
    let env = (material >> MAT_TEX_ENV_SHIFT) & 3u;
    // A water the extension draws a face for (see the header).
    let ext_water = ext_water_faced(material);
    // The texture and its bump, and the screen derivatives (uniform control flow, before any branch; fine ones: a coarse
    // derivative is one value over each 2 x 2 quad, and the bump's normal, the refraction it offsets and the light were drawn in
    // 2 x 2 blocks).
    let t_raw = textureSample(tex, samp, in.uv);
    let t_b = textureSample(tex, samp, TEXTURE_B_TURN * in.uv * TEXTURE_B_SCALE + TEXTURE_B_SHIFT);
    let sigma_s = dpdxFine(p);
    let sigma_t = dpdyFine(p);
    let duvx = dpdxFine(in.uv);
    let duvy = dpdyFine(in.uv);
    let dims = vec2<f32>(textureDimensions(tex));
    // Texels a pixel: the texture's texels a unit (the plane's UVs are a scale and a turn of x, y: the same over the plane, though
    // each fine derivative is one value over a pair of pixels) times the pixel's footprint worked out from the view.
    let foot = plane_footprint(p);
    let per_unit = max(
        length(duvx * dims) / max(length(sigma_s.xy), 1e-6),
        length(duvy * dims) / max(length(sigma_t.xy), 1e-6),
    );
    let texels = per_unit * max(length(foot[0]), length(foot[1]));
    // The bump's two layers, each faded where its texels are magnified, crossing over by the slow field.
    let layer = smoothstep(0.25, 0.75, noise_grad(p.xy / TEXTURE_MIX_SCALE + vec2<f32>(3.1, 7.7)).x);
    let has_tex = select(0.0, 1.0, env != 0u && !ext_water);
    let bump_a = luminance(t_raw.rgb) * t_raw.a;
    let bump_b = luminance(t_b.rgb) * t_b.a;
    // Seen edge-on the bump fades too: there a texel's worth of rows is a few pixels and the fine derivative, one value over each
    // pair of rows, drew its slope in 2 x 2 blocks.
    let edge_on = smoothstep(BUMP_EDGE_GONE, BUMP_EDGE_WHOLE, abs(normalize(frame.eye.xyz - p).z));
    let share_a = (1.0 - layer) * bump_share(texels) * has_tex * edge_on;
    let share_b = layer * bump_share(texels * TEXTURE_B_SCALE) * has_tex * edge_on;
    let db_s = share_a * dpdxFine(bump_a) + share_b * dpdxFine(bump_b);
    let db_t = share_a * dpdyFine(bump_a) + share_b * dpdyFine(bump_b);
    var t = t_raw;
    if ext_water {
        t = vec4<f32>(1.0, 1.0, 1.0, 0.0);
    }
    let time = refl.ssr.w;
    let clear = refl.water.z > 0.5;
    // The waves' normal: waves, ripples, the texture's bump.
    let waves = wave_field(p.xy, time, foot[0], foot[1]);
    var n = waves.n;
    let r1 = cross(sigma_t, n);
    let r2 = cross(n, sigma_s);
    let det = dot(sigma_s, r1);
    let surf = sign(det) * (db_s * r1 + db_t * r2);
    let bump_k = select(0.04, 0.02, clear);
    n = normalize(abs(det) * n - bump_k * surf);
    if dot(n, n) < 0.5 || !(n.z > 0.0) {
        n = up;
    }
    // The extension's face under the waves (the shading's normal), its occlusion and its share of the sun.
    var face_n = up;
    var face_occ = 1.0;
    var face_sun = 1.0;
    if ext_water {
        let st = ext_water_face(up, p, in.uv, sigma_s, sigma_t, duvx, duvy);
        face_n = st.n;
        face_occ = st.occlusion;
        face_sun = st.sun;
    }
    let shading_n = select(up, normalize(face_n + (n - up)), ext_water);
    let to_eye = frame.eye.xyz - p;
    let dist = length(to_eye);
    let v = to_eye / max(dist, 1e-6);
    let nv = max(dot(n, v), 1e-4);

    // The plane's own water colour: its colour with the texture (gamma), lit as the fixed-function pipeline lights the plane.
    var base = in.color.rgb;
    if env == 1u {
        base = mix(in.color.rgb, linear_to_srgb(t.rgb), t.a);
    } else if env == 2u {
        base = in.color.rgb * linear_to_srgb(t.rgb);
    } else if env == 3u {
        base = linear_to_srgb(t.rgb);
    }
    let alpha = in.color.a;
    let lit = shade_water(in.clip.xy, p, shading_n, srgb_to_linear(base), material, face_occ, face_sun);

    // Under the surface: the opaque copy, refracted while what is there stays behind the water.
    let frag = in.clip.xy;
    let lin_w = linear_depth(in.clip.z);
    let size = vec2<i32>(frame.screen.xy);
    let here = clamp(vec2<i32>(frag), vec2<i32>(0), size - 1);
    let d_here = textureLoad(refract_depth, here, 0);
    // The water's thickness under this pixel along the view (nothing there: deep).
    var thick = 1e3;
    if d_here > 0.0 {
        thick = max(linear_depth(d_here) - lin_w, 0.0) * dist / max(lin_w, 1e-3);
    }
    let nview = (frame.view * vec4<f32>(n - up, 0.0)).xyz;
    let px_per_unit = frame.proj[1][1] * 0.5 * frame.screen.y / max(lin_w, REFRACTION_NEAR);
    var shift = vec2<f32>(nview.x, -nview.y) * REFRACTION_REACH * px_per_unit;
    let lim = REFRACTION_MAX * frame.screen.y;
    let sl = length(shift);
    if sl > lim {
        shift = shift * (lim / sl);
    }
    shift = shift * smoothstep(0.0, REFRACTION_THIN, thick);
    var at = frag + shift;
    if !behind_water(at, lin_w) {
        at = frag + 0.5 * shift;
        if !behind_water(at, lin_w) {
            at = frag;
        }
    }
    let d_floor = textureLoad(refract_depth, clamp(vec2<i32>(at), vec2<i32>(0), size - 1), 0);
    let floor_c = background_at(at);
    // The path through the water to the floor (along the view ray; nothing there: deep).
    var path = 1e3;
    if d_floor > 0.0 {
        path = max(linear_depth(d_floor) - lin_w, 0.0) * dist / max(lin_w, 1e-3);
    }
    let water_lin = srgb_to_linear(in.color.rgb);
    let sigma = -log(clamp(water_lin, vec3<f32>(0.02), vec3<f32>(1.0)));
    var under: vec3<f32>;
    if clear {
        let trans = exp(-sigma * WATER_DENSITY_CLEAR * path);
        let detail = mix(1.0, 0.6 + 0.8 * mix(luminance(t.rgb), luminance(t_b.rgb), layer), 0.3 * select(0.0, 1.0, env != 0u && !ext_water));
        let scatter = shade_water(in.clip.xy, p, shading_n, water_lin * detail, material, face_occ, face_sun);
        under = floor_c * trans + scatter * (1.0 - trans);
        // Foam where the water is shallow, broken by the noise.
        let foam_n = noise_grad(p.xy * 4.0 + vec2<f32>(0.3, 0.1) * time).x;
        let foam = (1.0 - smoothstep(0.0, FOAM_DEPTH, path)) * smoothstep(0.25, 0.6, foam_n);
        let white = shade(in.clip.xy, p, up, vec3<f32>(0.9), material, false, 1.0, 0.0);
        under = mix(under, white, foam * 0.85);
    } else {
        let trans = exp(-sigma * WATER_DENSITY_TEXTURED * path);
        let seen = floor_c * mix(vec3<f32>(1.0), trans, 0.5);
        under = srgb_to_linear(mix(linear_to_srgb(seen), linear_to_srgb(lit), alpha));
    }

    // Above it: the reflection by Fresnel, and the sun's glint.
    var c = under;
    var mirror = vec3<f32>(0.0);
    if refl.water.y > 0.5 && refl.mode.y > 0.5 {
        let r = reflect(-v, n);
        let rr = vec3<f32>(r.xy, abs(r.z));
        let mode = u32(refl.mode.y + 0.5);
        var off = vec2<f32>(nview.x, -nview.y) * REFLECTION_REACH * frame.screen.y;
        let ol = length(off);
        if ol > lim {
            off = off * (lim / ol);
        }
        if mode == 3u {
            let uv = (frag + off) * frame.screen.zw;
            mirror = planar_at(clamp(uv, vec2<f32>(0.0), vec2<f32>(1.0)));
        } else if mode == 2u {
            let s = ssr_at(frag + off);
            mirror = mix(sky_radiance(rr, 0.0), s.rgb, s.a);
        } else {
            mirror = sky_radiance(rr, 0.0);
        }
        var f = schlick(WATER_F0, nv);
        if !clear {
            // The texture stays the water's face: the reflection takes at most three quarters.
            f = f * 0.75;
        }
        c = mix(c, mirror, f);
        let sun_up = dot(up, frame.sun_dir.xyz);
        if sun_up > 0.0 {
            // The clouds between the sun and the water dim the glint as they dim the sun's light on any surface.
            let vis = sun_shadow(p, up, sun_up, frag) * cloud_sun(p, sky.cloud_use.x);
            // The waves and ripples faded out with distance still scatter the glint: their slope widens its lobe.
            let rough = pow(pow(WATER_ROUGHNESS, 4.0) + waves.lost, 0.25);
            c = c + specular(n, v, frame.sun_dir.xyz, vec3<f32>(WATER_F0), rough) * frame.sun_linear.rgb * (max(dot(n, frame.sun_dir.xyz), 0.0) * vis);
        }
    }
    if shadows.config.w == 4u {
        return vec4<f32>(mirror, 1.0);
    }
    if shadows.config.w != 0u {
        return vec4<f32>(1.0);
    }
    if frame.fog_color.w > 0.0 {
        let span = max(frame.fog.y - frame.fog.x, 1e-3);
        let near = clamp((frame.fog.y - dist) / span, 0.0, 1.0);
        c = mix(frame.fog_color.rgb, c, select(near * near, near, frame.fog.z > 0.5));
    }
    return vec4<f32>(c, 1.0);
}
