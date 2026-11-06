// The enhanced renderer's sky passes, full-frame triangles drawn in the opaque pass in the sky's place (no depth):
//
// - fs_atmosphere_sky (the physical sky): where it is drawn in the box's place (the uniform's params.w 2), in its own colours
//   with the stars, in place of the box. Else drawn over the server's sky box (before the remote flare, the clouds and the bans),
//   with the
//   blend `src + dst * src.a`: the alpha is the atmosphere's shading of the box (skycommon.wgsl's sky_shading: brighter toward the
//   sun, darker away from it, the box's colours kept), the colour procedural stars where the server's sky is dark: one in a few
//   hundred cells of a cube-map-like grid of directions holds a star at a hashed spot, its brightness a hashed power law, dimmed by
//   the transmittance toward it (so they fade at the horizon) and twinkling with the effect time.
// - fs_sun_disk (always): only the disk with limb darkening (a linear law per channel, I(mu) = 1 - u (1 - mu)), dimmed toward the
//   horizon by the air it shines through (sun_air_mass_dimming) and made white-hot on its own path to white (disk_white_hot),
//   added over the
//   sky box before the clouds and the fog bans (they cover it as they cover the box), its colour the fixed-function sun's, at the sun as
//   the sky shows it (sky.drawn: on the sun the server's box paints where it paints one off the Sun's direction).
// - fs_clouds (where the settings and quality have them): the volumetric clouds' image (clouds.wgsl, its own history at a reduced
//   size) upsampled through a Catmull-Rom filter over the sky behind them with the same blend as fs_atmosphere_sky (`src + dst * src.a`: their light and
//   their transmittance), in front of the server's flat cloud layers (the sky's high layer) and behind the fog bans; the frame's
//   jitter undone (they were marched with the unjittered camera).
//
// Prepended: common.wgsl, skycommon.wgsl.

@group(0) @binding(0) var<uniform> frame: Frame;
@group(2) @binding(1) var<uniform> sky: Sky;
@group(2) @binding(2) var sky_view_lut: texture_2d<f32>;
@group(2) @binding(3) var transmittance_lut: texture_2d<f32>;
@group(2) @binding(4) var sky_linear: sampler;
// The volumetric clouds' image (rgb their light, a their transmittance).
@group(2) @binding(6) var cloud_image: texture_2d<f32>;

@vertex
fn vs_full(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let x = f32((i << 1u) & 2u);
    let y = f32(i & 2u);
    // At the far plane (reversed depth 0).
    return vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
}

// An eye-space direction in world space: the view's rotation transposed (eye axis k in world is the view's column k's xyz).
fn eye_to_world_dir(d: vec3<f32>) -> vec3<f32> {
    let v = frame.view;
    return vec3<f32>(dot(v[0].xyz, d), dot(v[1].xyz, d), dot(v[2].xyz, d));
}

// The world direction through a pixel (the frame's jittered projection, so the disk and the stars are anti-aliased with the rest).
fn view_dir(frag: vec2<f32>) -> vec3<f32> {
    let ndc = vec2<f32>(frag.x * frame.screen.z * 2.0 - 1.0, 1.0 - frag.y * frame.screen.w * 2.0);
    let e = frame.inv_proj * vec4<f32>(ndc, 1.0, 1.0);
    let d_eye = e.xyz / e.w;
    return normalize(eye_to_world_dir(d_eye));
}

// The disk's light toward dir (0 off the disk): its radiance, the limb darkening and an anti-aliased edge.
fn sun_disk(dir: vec3<f32>, frag_angle: f32) -> vec3<f32> {
    let radius = sky.sun.w;
    let c = dot(dir, normalize(sky.drawn.xyz));
    let ang = acos(clamp(c, -1.0, 1.0));
    if ang > radius + 2.0 * frag_angle {
        return vec3<f32>(0.0);
    }
    let x = clamp(ang / radius, 0.0, 1.0);
    let mu = sqrt(max(1.0 - x * x, 0.0));
    let limb = vec3<f32>(1.0) - sky.limb.rgb * (1.0 - mu);
    let edge = clamp((radius - ang) / max(frag_angle, 1e-6) + 0.5, 0.0, 1.0);
    return disk_white_hot(sky.disk.rgb * limb * sun_air_mass_dimming(dir)) * edge;
}

// The neutral tonemap's shoulder (tonemap.wgsl's SHOULDER) and the disk's overshoot past white at which its color has gone half
// way to white (hd::sky's DISK_SHOULDER and DISK_WHITE_SPILL; a test compares them).
const DISK_SHOULDER: f32 = 0.8;
const DISK_WHITE_SPILL: f32 = 16.0;

fn disk_shoulder(v: f32) -> f32 {
    if v <= DISK_SHOULDER {
        return v;
    }
    let span = 1.0 - DISK_SHOULDER;
    return DISK_SHOULDER + span * (1.0 - exp(-(v - DISK_SHOULDER) / span));
}

// The disk's light made white-hot (hd::sky::disk_white_hot, its CPU form): the tonemap keeps a color's hue however far past white
// it is, so a saturated disk showed as the same orange as the sky about it and darker than a paler sky. The disk's hue goes
// q / (1 + q) of the way (q the square of the overshoot over DISK_WHITE_SPILL) to each channel's own shoulder, its largest
// channel kept: white overhead, cream, peach and orange as the air dims and reddens it. Before the edge's coverage, so the
// anti-aliased rim is as white as the disk.
fn disk_white_hot(d: vec3<f32>) -> vec3<f32> {
    let m = max(d.r, max(d.g, d.b));
    if m <= 1.0 {
        return d;
    }
    let x = (m - 1.0) / DISK_WHITE_SPILL;
    let q = x * x;
    let own = vec3<f32>(disk_shoulder(d.r), disk_shoulder(d.g), disk_shoulder(d.b));
    let shown = mix(d / m, own, q / (1.0 + q));
    return shown * (m / max(max(shown.r, max(shown.g, shown.b)), 1e-6));
}

// How much dimmer the disk is toward dir than overhead: the luminance of the atmosphere's transmittance toward it over the
// zenith's (1 overhead, falling toward the horizon, 0 behind the planet). A scalar: the disk's color (the sun's) is already
// reddened.
fn sun_air_mass_dimming(dir: vec3<f32>) -> f32 {
    let zenith = sky_luminance(sky_transmittance_toward(vec3<f32>(0.0, 0.0, 1.0)));
    return clamp(sky_luminance(sky_transmittance_toward(dir)) / max(zenith, 1e-6), 0.0, 1.0);
}

// A hash of a cell (Hugo Elias's integer hash, our arrangement).
fn hash3(p: vec3<i32>) -> f32 {
    var n = (u32(p.x) * 73856093u) ^ (u32(p.y) * 19349663u) ^ (u32(p.z) * 83492791u);
    n = (n << 13u) ^ n;
    n = n * (n * n * 15731u + 789221u) + 1376312589u;
    return f32(n & 0x7fffffffu) / 2147483647.0;
}

// The stars toward dir: the direction's cube face cell (512 a side), a star in it at a hashed spot with a hashed brightness.
fn stars(dir: vec3<f32>, frag_angle: f32) -> f32 {
    let a = abs(dir);
    var face: i32;
    var uv: vec2<f32>;
    if a.x >= a.y && a.x >= a.z {
        face = select(1, 0, dir.x > 0.0);
        uv = dir.yz / a.x;
    } else if a.y >= a.z {
        face = select(3, 2, dir.y > 0.0);
        uv = dir.xz / a.y;
    } else {
        face = select(5, 4, dir.z > 0.0);
        uv = dir.xy / a.z;
    }
    let cells = 512.0;
    let g = (uv * 0.5 + 0.5) * cells;
    let cell = vec2<i32>(floor(g));
    let h = hash3(vec3<i32>(cell, face));
    if h > sky.night.w {
        return 0.0;
    }
    let spot = vec2<f32>(hash3(vec3<i32>(cell, face + 11)), hash3(vec3<i32>(cell, face + 23)));
    let off = (g - vec2<f32>(cell) - (0.15 + 0.7 * spot)) / cells * 2.0;
    // The star's apparent size: about a pixel.
    let w = max(frag_angle, 1e-5);
    let fall = exp(-dot(off, off) / (w * w));
    let mag = pow(hash3(vec3<i32>(cell, face + 37)), 6.0);
    let twinkle = 0.8 + 0.2 * sin(sky.params.z * (2.0 + 5.0 * h) + 40.0 * h);
    return sky.stars.x * mag * fall * twinkle;
}

// The angle a pixel spans.
fn pixel_angle() -> f32 {
    return 2.0 / max(frame.proj[1].y * frame.screen.y, 1.0);
}

@fragment
fn fs_atmosphere_sky(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {
    let dir = view_dir(p.xy);
    let fa = pixel_angle();
    let horizon = sky_horizon(sky.radii.z);
    let el = asin(clamp(dir.z, -1.0, 1.0));
    if sky.params.w > 1.5 {
        // The atmosphere's own sky in place of the server's box (the blend's alpha 0: the box is replaced): its light, below
        // the horizon its colour just above it (a world's ground ends before the planet's horizon), and the
        // stars where it is dark. Under a low sun the clouds cover, the glow about it fades with it (cloudshadow.wgsl).
        if el <= horizon {
            return vec4<f32>(atmosphere_drawn(dir) * cloud_aureole(dir), 0.0);
        }
        var own = atmosphere_radiance(dir) * cloud_aureole(dir);
        let dark = clamp(1.0 - sky_luminance(own) / sky.stars.y, 0.0, 1.0);
        if dark > 0.0 {
            own = own + vec3<f32>(stars(dir, fa)) * sky_transmittance_toward(dir) * dark;
        }
        return vec4<f32>(own, 0.0);
    }
    var c = vec3<f32>(0.0);
    if el > horizon {
        let dark = clamp(1.0 - sky.disk.w / sky.stars.y, 0.0, 1.0);
        if dark > 0.0 {
            c = vec3<f32>(stars(dir, fa)) * sky_transmittance_toward(dir) * dark;
        }
    }
    return vec4<f32>(c, sky_shading(dir));
}

@fragment
fn fs_clouds(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {
    let px = p.xy - frame.temporal.xy;
    let uv = px * frame.screen.zw * sky.cloud_use.zw;
    // A Catmull-Rom upsample (16 loads; the edges stay crisp where bilinear softened them), held within the four nearest texels.
    let dims = vec2<i32>(textureDimensions(cloud_image));
    let q = uv * vec2<f32>(dims) - 0.5;
    let base = floor(q);
    let f = q - base;
    let f2 = f * f;
    let f3 = f2 * f;
    let w = array<vec2<f32>, 4>(
        -0.5 * f3 + f2 - 0.5 * f,
        1.5 * f3 - 2.5 * f2 + 1.0,
        -1.5 * f3 + 2.0 * f2 + 0.5 * f,
        0.5 * f3 - 0.5 * f2,
    );
    var sum = vec4<f32>(0.0);
    var lo = vec4<f32>(1e9);
    var hi = vec4<f32>(-1e9);
    for (var j = 0; j < 4; j = j + 1) {
        for (var i = 0; i < 4; i = i + 1) {
            let t = clamp(vec2<i32>(base) + vec2<i32>(i - 1, j - 1), vec2<i32>(0), dims - 1);
            let v = textureLoad(cloud_image, t, 0);
            sum = sum + v * (w[i].x * w[j].y);
            if (i == 1 || i == 2) && (j == 1 || j == 2) {
                lo = min(lo, v);
                hi = max(hi, v);
            }
        }
    }
    return clamp(sum, lo, hi);
}

@fragment
fn fs_sun_disk(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {
    let dir = view_dir(p.xy);
    let c = sun_disk(dir, pixel_angle());
    if max(c.r, max(c.g, c.b)) <= 0.0 {
        discard;
    }
    return vec4<f32>(c, 1.0);
}
