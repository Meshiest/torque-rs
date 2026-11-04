// The physical sky's tables, compute passes after Hillaire 2020: the transmittance (256 x 64, Bruneton's mapping), the multiple
// scattering (32 x 32 over the sun's cosine and the height) and the sky view (192 x 108 for the camera's height). Each step of a march
// integrates its source analytically over the step's constant medium, (S - S e^(-σt dt)) / σt (Hillaire's energy-conserving form).
// hd::atmosphere holds the same equations in f64 (the gate's reference). Prepended: skycommon.wgsl.

@group(0) @binding(0) var<uniform> sky: Sky;
@group(0) @binding(1) var transmittance_lut: texture_2d<f32>;
@group(0) @binding(2) var multiscatter_lut: texture_2d<f32>;
@group(0) @binding(3) var sky_linear: sampler;
@group(0) @binding(4) var out_lut: texture_storage_2d<rgba16float, write>;
// Declared for skycommon's lookups (not read here).
@group(0) @binding(5) var sky_view_lut: texture_2d<f32>;

fn integrate(src: vec3<f32>, ext: vec3<f32>, dt: f32) -> vec3<f32> {
    let e = max(ext, vec3<f32>(1e-9));
    return (src - src * exp(-ext * dt)) / e;
}

// The transmittance toward the sun from (r, mu_s) through the table (0 where the planet hides it).
fn sun_transmittance(r: f32, mu_s: f32) -> vec3<f32> {
    if sky_hits_ground(r, mu_s) {
        return vec3<f32>(0.0);
    }
    return textureSampleLevel(transmittance_lut, sky_linear, transmittance_uv(r, mu_s), 0.0).rgb;
}

@compute @workgroup_size(8, 8, 1)
fn cs_transmittance(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= u32(TRANSMITTANCE_W) || id.y >= u32(TRANSMITTANCE_H) {
        return;
    }
    let uv = (vec2<f32>(id.xy) + 0.5) / vec2<f32>(TRANSMITTANCE_W, TRANSMITTANCE_H);
    let rm = transmittance_r_mu(uv);
    let r = rm.x;
    let mu = rm.y;
    let d = sky_distance_to_top(r, mu);
    let dt = d / f32(TRANSMITTANCE_STEPS);
    var depth = vec3<f32>(0.0);
    for (var i = 0u; i < TRANSMITTANCE_STEPS; i = i + 1u) {
        let t = (f32(i) + 0.5) * dt;
        let h = sqrt(r * r + t * t + 2.0 * r * mu * t) - sky.radii.x;
        depth = depth + sky_medium(h).extinction * dt;
    }
    textureStore(out_lut, vec2<i32>(id.xy), vec4<f32>(exp(-depth), 1.0));
}

// Spherical Fibonacci direction k of n.
fn fibonacci(k: u32, n: u32) -> vec3<f32> {
    let z = 1.0 - 2.0 * (f32(k) + 0.5) / f32(n);
    let s = sqrt(max(1.0 - z * z, 0.0));
    let phi = 2.39996323 * f32(k);
    return vec3<f32>(s * cos(phi), s * sin(phi), z);
}

@compute @workgroup_size(8, 8, 1)
fn cs_multiscatter(@builtin(global_invocation_id) id: vec3<u32>) {
    let n = u32(MULTISCATTER_N);
    if id.x >= n || id.y >= n {
        return;
    }
    let uv = (vec2<f32>(id.xy) + 0.5) / MULTISCATTER_N;
    let mu_s = multiscatter_mu(sky_from_unit(uv.x, MULTISCATTER_N));
    let hy = sky_from_unit(uv.y, MULTISCATTER_N);
    let r = sky.radii.x + (sky.radii.y - sky.radii.x) * hy * hy;
    let sun = vec3<f32>(sqrt(max(1.0 - mu_s * mu_s, 0.0)), 0.0, mu_s);
    let iso = 1.0 / (4.0 * SKY_PI);
    var l2 = vec3<f32>(0.0);
    var fms = vec3<f32>(0.0);
    for (var k = 0u; k < MULTISCATTER_DIRECTIONS; k = k + 1u) {
        let w = fibonacci(k, MULTISCATTER_DIRECTIONS);
        let ground = sky_hits_ground(r, w.z);
        let d = select(sky_distance_to_top(r, w.z), sky_distance_to_bottom(r, w.z), ground);
        let dt = d / f32(MULTISCATTER_STEPS);
        var tp = vec3<f32>(1.0);
        var l = vec3<f32>(0.0);
        var f = vec3<f32>(0.0);
        for (var i = 0u; i < MULTISCATTER_STEPS; i = i + 1u) {
            let t = (f32(i) + 0.5) * dt;
            let p = vec3<f32>(w.x * t, w.y * t, r + w.z * t);
            let rx = length(p);
            let m = sky_medium(rx - sky.radii.x);
            let sigma_s = m.rayleigh + vec3<f32>(m.mie);
            let tsun = sun_transmittance(rx, dot(p, sun) / rx);
            l = l + tp * integrate(sigma_s * tsun * iso, m.extinction, dt);
            f = f + tp * integrate(sigma_s, m.extinction, dt);
            tp = tp * exp(-m.extinction * dt);
        }
        if ground {
            let p = vec3<f32>(w.x * d, w.y * d, r + w.z * d);
            let nrm = normalize(p);
            let mu_g = dot(nrm, sun);
            let tsun = sun_transmittance(sky.radii.x, mu_g);
            l = l + tp * tsun * sky.ozone_ground.yzw * (clamp(mu_g, 0.0, 1.0) / SKY_PI);
        }
        l2 = l2 + l;
        fms = fms + f;
    }
    l2 = l2 / f32(MULTISCATTER_DIRECTIONS);
    fms = fms / f32(MULTISCATTER_DIRECTIONS);
    let psi = l2 / max(vec3<f32>(1.0) - fms, vec3<f32>(1e-6));
    textureStore(out_lut, vec2<i32>(id.xy), vec4<f32>(psi, 1.0));
}

@compute @workgroup_size(8, 8, 1)
fn cs_sky_view(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= u32(SKY_VIEW_W) || id.y >= u32(SKY_VIEW_H) {
        return;
    }
    let uv = (vec2<f32>(id.xy) + 0.5) / vec2<f32>(SKY_VIEW_W, SKY_VIEW_H);
    let r = sky.radii.z;
    let ang = sky_view_angles(uv, r);
    // In the sun's vertical plane: the sun along +x (its elevation), the view at the azimuth from it.
    let sz = clamp(sky.sun.z, -1.0, 1.0);
    let sun = vec3<f32>(sqrt(max(1.0 - sz * sz, 0.0)), 0.0, sz);
    let ce = cos(ang.y);
    let view = vec3<f32>(ce * cos(ang.x), ce * sin(ang.x), sin(ang.y));
    let ground = sky_hits_ground(r, view.z);
    let d = select(sky_distance_to_top(r, view.z), sky_distance_to_bottom(r, view.z), ground);
    let nu = dot(view, sun);
    let pr = rayleigh_phase(nu);
    let pm = cornette_shanks(nu, sky.mie.w);
    var tp = vec3<f32>(1.0);
    var l = vec3<f32>(0.0);
    let n = f32(SKY_VIEW_STEPS);
    for (var i = 0u; i < SKY_VIEW_STEPS; i = i + 1u) {
        // Steps crowded near the camera: from d (i / n)^2 to d ((i + 1) / n)^2.
        let a = f32(i) / n;
        let b = (f32(i) + 1.0) / n;
        let t0 = d * a * a;
        let t1 = d * b * b;
        let t = 0.5 * (t0 + t1);
        let dt = t1 - t0;
        let p = vec3<f32>(view.x * t, view.y * t, r + view.z * t);
        let rx = length(p);
        let mu_s = dot(p, sun) / rx;
        let m = sky_medium(rx - sky.radii.x);
        let tsun = sun_transmittance(rx, mu_s);
        let psi = textureSampleLevel(multiscatter_lut, sky_linear, multiscatter_uv(rx, mu_s), 0.0).rgb;
        let src = (m.rayleigh * pr + vec3<f32>(m.mie * pm)) * tsun + (m.rayleigh + vec3<f32>(m.mie)) * psi;
        l = l + tp * integrate(src, m.extinction, dt);
        tp = tp * exp(-m.extinction * dt);
    }
    textureStore(out_lut, vec2<i32>(id.xy), vec4<f32>(l, 1.0));
}
