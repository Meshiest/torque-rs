// The enhanced renderer's sky, shared: the sky uniform, the atmosphere's medium and geometry, the three tables'
// parameterisations, and the lookups the sky, the fog, the clouds and the volumetrics use. Our own code after Hillaire 2020 ("A
// Scalable and Production Ready Sky and Atmosphere Rendering Technique") and Bruneton 2017's table mapping; the CPU form of every
// equation is torque_render_scene::hd::atmosphere (the gate's f64 integrator).
//
// A module that includes this declares the globals it reads: `sky` (the uniform), and for the lookups `sky_view_lut`,
// `transmittance_lut` and `sky_linear` (a linear clamp sampler).

// The tables' sizes and steps (hd::atmosphere's; a test compares them).
const TRANSMITTANCE_W: f32 = 256.0;
const TRANSMITTANCE_H: f32 = 64.0;
const TRANSMITTANCE_STEPS: u32 = 64u;
const MULTISCATTER_N: f32 = 64.0;
const MULTISCATTER_DIRECTIONS: u32 = 128u;
const MULTISCATTER_STEPS: u32 = 64u;
const SKY_VIEW_W: f32 = 192.0;
const SKY_VIEW_H: f32 = 108.0;
const SKY_VIEW_STEPS: u32 = 48u;
const SKY_PI: f32 = 3.14159265358979;

struct Sky {
    // x: the light response (0 gamma space, 1 linear light), y: the atmosphere's sky's scale, z: the effect time, w: 1 when the
    // physical sky is drawn (its sky view table), 2 when it is drawn in place of the server's box.
    params: vec4<f32>,
    // xyz: towards the sun; w: the disk's angular radius (radians).
    sun: vec4<f32>,
    // rgb: the disk's radiance times the sun's colour; w: the server's sky's luminance (linear; the stars show under stars.y).
    disk: vec4<f32>,
    // rgb: the limb darkening's coefficients; w: the atmosphere's shading's strength.
    limb: vec4<f32>,
    // The atmosphere (km): x the ground's radius, y the top's, z the camera's.
    radii: vec4<f32>,
    // rgb: Rayleigh scattering (per km); w: its scale height.
    rayleigh: vec4<f32>,
    // x: Mie scattering, y: Mie absorption, z: its scale height, w: its asymmetry.
    mie: vec4<f32>,
    // rgb: ozone absorption; w: the layer's centre height.
    ozone: vec4<f32>,
    // x: the ozone layer's half width; yzw: the ground's albedo.
    ozone_ground: vec4<f32>,
    // rgb: the night sky's floor (scaled); w: the share of directions with a star.
    night: vec4<f32>,
    // x: a star's brightness, y: the sky luminance under which they show, z: the clouds' horizon fade (radians), w: the atmosphere's
    // shading's floor.
    stars: vec4<f32>,
    // The height fog (also the volumetric medium's falloff): x extinction at the base (per unit), y falloff height, z base height, w 1 when on.
    height_fog: vec4<f32>,
    // The emitters' gains: x glow surfaces, y self-illuminating shape parts, z coronas and flares; w: the atmosphere's shading's cap.
    emissive: vec4<f32>,
    // The volumetric medium: x scattering (per unit), y anisotropy, z strength, w reach (units).
    vol: vec4<f32>,
    // x: 1 when the volumetrics run, y: the march's steps, z: internal pixels per volumetric texel; w: the most the lit clouds'
    // lobe brightens them.
    vol_config: vec4<f32>,
    // rgb: the sun's light the volumetrics take (linear); w: the share of the forward-scattering lobe in the lit clouds.
    sun_light: vec4<f32>,
    // The volumetric clouds' shadow map (cloudshadow.wgsl): xy its centre, z 1 / its span, w the cloud base's height.
    cloud_map: vec4<f32>,
    // X the clouds' shadows on the ground (0 off), y on the volumetric light, zw the cloud image's uv scale (sky.wgsl).
    cloud_use: vec4<f32>,
    // The sun through the clouds as the eye sees it (cloudshadow.wgsl): x its strength (0 off), yz the sines of the sun's elevation
    // over which it hands over to the shadow map, w how far from the sun the atmosphere's glow takes it (radians).
    cloud_sun: vec4<f32>,
    // xyz: towards the sun as the sky shows it (the sun the server's box paints, else the Sun's direction): the disk, the glow about
    // it in the server's sky and the fog's, the volumetric light's lobe; w unused.
    drawn: vec4<f32>,
    // The fog's sky without its detail (skyfog.wgsl): x the sky copy's mip level, y the clouds' image's; zw the shares of the visible
    // distance over which a surface the fog wholly hides takes the sky's detail behind it again.
    fog_low: vec4<f32>,
    // The volumetric clouds' volume layers drawn (skyfog.wgsl's clouds_over_share): xy the first's base and top, zw the second's
    // (units; 0 0 none).
    cloud_slabs: vec4<f32>,
    // Where the extension's geometry is wholly fogged in each of 16 headings about the eye (0: with the rest; the extension's
    // fog hook reads it).
    ext_fog_ends: array<vec4<f32>, 4>,
};

// The rotation taking the sun as the sky shows it (sky.drawn) to the Sun's direction (sky.sun), applied to `dir`: the atmosphere's
// tables are made for the Sun, so their glow is read about the drawn sun through it (the identity when the two are one).
fn toward_light_frame(dir: vec3<f32>) -> vec3<f32> {
    let a = normalize(sky.drawn.xyz);
    let b = normalize(sky.sun.xyz);
    let c = dot(a, b);
    let axis = cross(a, b);
    let s = length(axis);
    if s < 1e-6 {
        return dir;
    }
    let k = axis / s;
    // Rodrigues: v cos + (k x v) sin + k (k . v) (1 - cos).
    return dir * c + cross(k, dir) * s + k * dot(k, dir) * (1.0 - c);
}

fn linear_light() -> bool {
    return sky.params.x > 0.5;
}

// The medium at height h (km): rgb Rayleigh scattering, a Mie scattering, rgb extinction.
struct SkyMedium {
    rayleigh: vec3<f32>,
    mie: f32,
    extinction: vec3<f32>,
};

fn sky_medium(h_in: f32) -> SkyMedium {
    let h = max(h_in, 0.0);
    let dr = exp(-h / sky.rayleigh.w);
    let dm = exp(-h / sky.mie.z);
    let dozone = max(1.0 - abs(h - sky.ozone.w) / sky.ozone_ground.x, 0.0);
    var m: SkyMedium;
    m.rayleigh = sky.rayleigh.rgb * dr;
    m.mie = sky.mie.x * dm;
    m.extinction = m.rayleigh + vec3<f32>((sky.mie.x + sky.mie.y) * dm) + sky.ozone.rgb * dozone;
    return m;
}

fn sky_distance_to_top(r: f32, mu: f32) -> f32 {
    let top = sky.radii.y;
    let disc = r * r * (mu * mu - 1.0) + top * top;
    return max(-r * mu + sqrt(max(disc, 0.0)), 0.0);
}

fn sky_distance_to_bottom(r: f32, mu: f32) -> f32 {
    let bottom = sky.radii.x;
    let disc = r * r * (mu * mu - 1.0) + bottom * bottom;
    return max(-r * mu - sqrt(max(disc, 0.0)), 0.0);
}

fn sky_hits_ground(r: f32, mu: f32) -> bool {
    let bottom = sky.radii.x;
    return mu < 0.0 && r * r * (mu * mu - 1.0) + bottom * bottom >= 0.0;
}

fn sky_to_unit(x: f32, n: f32) -> f32 {
    return 0.5 / n + x * (1.0 - 1.0 / n);
}

fn sky_from_unit(u: f32, n: f32) -> f32 {
    return (u - 0.5 / n) / (1.0 - 1.0 / n);
}

// The transmittance table's uv of (r, mu) (Bruneton 2017's mapping).
fn transmittance_uv(r: f32, mu: f32) -> vec2<f32> {
    let bottom = sky.radii.x;
    let top = sky.radii.y;
    let big_h = sqrt(top * top - bottom * bottom);
    let rho = sqrt(max(r * r - bottom * bottom, 0.0));
    let d = sky_distance_to_top(r, mu);
    let d_min = top - r;
    let d_max = rho + big_h;
    let x_mu = select(0.0, (d - d_min) / (d_max - d_min), d_max > d_min);
    return vec2<f32>(sky_to_unit(x_mu, TRANSMITTANCE_W), sky_to_unit(rho / big_h, TRANSMITTANCE_H));
}

// The (r, mu) of a transmittance table's uv.
fn transmittance_r_mu(uv: vec2<f32>) -> vec2<f32> {
    let bottom = sky.radii.x;
    let top = sky.radii.y;
    let x_mu = sky_from_unit(uv.x, TRANSMITTANCE_W);
    let x_r = sky_from_unit(uv.y, TRANSMITTANCE_H);
    let big_h = sqrt(top * top - bottom * bottom);
    let rho = big_h * x_r;
    let r = sqrt(rho * rho + bottom * bottom);
    let d_min = top - r;
    let d_max = rho + big_h;
    let d = d_min + x_mu * (d_max - d_min);
    var mu = 1.0;
    if d > 0.0 {
        mu = clamp((big_h * big_h - rho * rho - d * d) / (2.0 * r * d), -1.0, 1.0);
    }
    return vec2<f32>(r, mu);
}

// The multiple scattering table's sun cosine as sinh(k t) / sinh(k) of t in [-1, 1] (texels crowded about the terminator).
const MULTISCATTER_CROWD: f32 = 2.0;

fn multiscatter_mu(x: f32) -> f32 {
    return sinh(MULTISCATTER_CROWD * (2.0 * x - 1.0)) / sinh(MULTISCATTER_CROWD);
}

fn multiscatter_uv(r: f32, mu_s: f32) -> vec2<f32> {
    let x = clamp(0.5 + 0.5 * asinh(clamp(mu_s, -1.0, 1.0) * sinh(MULTISCATTER_CROWD)) / MULTISCATTER_CROWD, 0.0, 1.0);
    // The height as its square root (texels crowded near the ground, where the light changes fastest with height).
    let y = sqrt(clamp((r - sky.radii.x) / (sky.radii.y - sky.radii.x), 0.0, 1.0));
    return vec2<f32>(sky_to_unit(x, MULTISCATTER_N), sky_to_unit(y, MULTISCATTER_N));
}

// The horizon's elevation seen from radius r (below 0).
fn sky_horizon(r: f32) -> f32 {
    return asin(clamp(sky.radii.x / r, -1.0, 1.0)) - 0.5 * SKY_PI;
}

// The sky view table's (azimuth from the sun, elevation) of its uv.
fn sky_view_angles(uv: vec2<f32>, r: f32) -> vec2<f32> {
    let az = clamp(sky_from_unit(uv.x, SKY_VIEW_W), 0.0, 1.0) * SKY_PI;
    let vx = clamp(sky_from_unit(uv.y, SKY_VIEW_H), 0.0, 1.0);
    let eh = sky_horizon(r);
    let half = 0.5 * SKY_PI;
    var el: f32;
    if vx < 0.5 {
        let c = 1.0 - 2.0 * vx;
        el = eh + (half - eh) * c * c;
    } else {
        let c = 2.0 * vx - 1.0;
        el = eh - (half + eh) * c * c;
    }
    return vec2<f32>(az, el);
}

fn sky_view_uv(az: f32, el: f32, r: f32) -> vec2<f32> {
    let eh = sky_horizon(r);
    let half = 0.5 * SKY_PI;
    var vx: f32;
    if el >= eh {
        vx = 0.5 * (1.0 - sqrt(clamp((el - eh) / (half - eh), 0.0, 1.0)));
    } else {
        vx = 0.5 * (1.0 + sqrt(clamp((eh - el) / (half + eh), 0.0, 1.0)));
    }
    return vec2<f32>(sky_to_unit(clamp(az / SKY_PI, 0.0, 1.0), SKY_VIEW_W), sky_to_unit(vx, SKY_VIEW_H));
}

fn rayleigh_phase(nu: f32) -> f32 {
    return 3.0 / (16.0 * SKY_PI) * (1.0 + nu * nu);
}

fn cornette_shanks(nu: f32, g: f32) -> f32 {
    let g2 = g * g;
    return 3.0 / (8.0 * SKY_PI) * (1.0 - g2) * (1.0 + nu * nu) / ((2.0 + g2) * pow(max(1.0 + g2 - 2.0 * g * nu, 1e-6), 1.5));
}

fn henyey_greenstein(nu: f32, g: f32) -> f32 {
    let g2 = g * g;
    return (1.0 - g2) / (4.0 * SKY_PI * pow(max(1.0 + g2 - 2.0 * g * nu, 1e-6), 1.5));
}

// The transmittance from the camera toward a direction (0 where the ground is in the way).
fn sky_transmittance_toward(dir: vec3<f32>) -> vec3<f32> {
    let r = sky.radii.z;
    if sky_hits_ground(r, dir.z) {
        return vec3<f32>(0.0);
    }
    return textureSampleLevel(transmittance_lut, sky_linear, transmittance_uv(r, dir.z), 0.0).rgb;
}

// The atmosphere's light at an azimuth from the sun and an elevation (radians), without the sun disk or the stars: the sky view
// table times the scale, and the night's floor.
fn atmosphere_at(az: f32, el: f32) -> vec3<f32> {
    let l = textureSampleLevel(sky_view_lut, sky_linear, sky_view_uv(az, el, sky.radii.z), 0.0).rgb;
    return l * sky.params.y + sky.night.rgb;
}

// The atmosphere's light from a direction.
fn atmosphere_radiance(dir: vec3<f32>) -> vec3<f32> {
    let s = sky.sun.xyz;
    let sh = vec2<f32>(s.x, s.y);
    let dh = vec2<f32>(dir.x, dir.y);
    let ls = length(sh);
    let ld = length(dh);
    var c = 1.0;
    if ls > 1e-4 && ld > 1e-4 {
        c = clamp(dot(sh, dh) / (ls * ld), -1.0, 1.0);
    }
    return atmosphere_at(acos(c), asin(clamp(dir.z, -1.0, 1.0)));
}

// The atmosphere's light toward a direction as its own sky draws it: below the horizon its colour just above it (a world's
// ground ends before the planet's horizon).
fn atmosphere_drawn(dir: vec3<f32>) -> vec3<f32> {
    let horizon = sky_horizon(sky.radii.z);
    if asin(clamp(dir.z, -1.0, 1.0)) <= horizon {
        let h = normalize(vec2<f32>(dir.x, dir.y) + vec2<f32>(1e-6, 0.0));
        let e = horizon + 0.002;
        return atmosphere_radiance(vec3<f32>(h * cos(e), sin(e)));
    }
    return atmosphere_radiance(dir);
}

// Rec. 709 luminance (the module is also used without common.wgsl).
fn sky_luminance(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
}

// How the physical sky shades the server's sky toward a direction: its luminance there over its mean luminance at the
// same elevation (eight azimuths from the sun's side to the far side; the table is symmetric about the sun's azimuth; both read
// about the sun the sky shows, toward_light_frame), raised to
// the shading's strength and held between its floor and its cap. A scalar: the server's colours stay; 1 at and below the horizon.
fn sky_shading(dir: vec3<f32>) -> f32 {
    let el = asin(clamp(dir.z, -1.0, 1.0));
    let eh = sky_horizon(sky.radii.z);
    if el <= eh {
        return 1.0;
    }
    // Read about the sun the sky shows (the box's painted sun): the direction turned into the Sun's frame.
    let r = toward_light_frame(dir);
    let er = asin(clamp(r.z, -1.0, 1.0));
    var mean = 0.0;
    for (var k = 0u; k < 8u; k = k + 1u) {
        mean = mean + sky_luminance(atmosphere_at((f32(k) + 0.5) * SKY_PI / 8.0, er));
    }
    mean = mean / 8.0;
    let here = sky_luminance(atmosphere_radiance(r));
    let ratio = here / max(mean, 1e-8);
    // Toward the horizon the shading eases out (the bans and the box's own horizon take over).
    let rise = smoothstep(eh, eh + 0.06, el);
    let shaped = clamp(pow(max(ratio, 1e-6), sky.limb.w), sky.stars.w, sky.emissive.w);
    return mix(1.0, shaped, rise);
}
