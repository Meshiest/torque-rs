// The sky's light for the global illumination (with `gi_sky` bound by the module): irradiance.wgsl's coefficients, 0-8 the
// main view's (the sky cube's alone where the probes replace the light capture's bounce).

struct GiSky {
    sh: array<vec4<f32>, 18>,
};

// The sky's irradiance over pi toward `d` (a ray's radiance where it leaves the voxels: the coefficients hold no finer detail).
fn gi_sky_radiance(d: vec3<f32>) -> vec3<f32> {
    var c = gi_sky.sh[0].rgb * 0.2820948;
    c = c + gi_sky.sh[1].rgb * (0.4886025 * d.y);
    c = c + gi_sky.sh[2].rgb * (0.4886025 * d.z);
    c = c + gi_sky.sh[3].rgb * (0.4886025 * d.x);
    c = c + gi_sky.sh[4].rgb * (1.0925484 * d.x * d.y);
    c = c + gi_sky.sh[5].rgb * (1.0925484 * d.y * d.z);
    c = c + gi_sky.sh[6].rgb * (0.3153916 * (3.0 * d.z * d.z - 1.0));
    c = c + gi_sky.sh[7].rgb * (1.0925484 * d.x * d.z);
    c = c + gi_sky.sh[8].rgb * (0.5462742 * (d.x * d.x - d.y * d.y));
    return max(c, vec3<f32>(0.0));
}
