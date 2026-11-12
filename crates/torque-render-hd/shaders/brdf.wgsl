// The shading's BRDF, shared by the forward pass, the water and the environment's prefilter: our own code from the published
// techniques named where they are used. Prepended after common.wgsl (it takes PI from there). Roughness is the perceptual one
// throughout (GGX's alpha is its square).
//
// - Specular: GGX (Walter et al. 2007) with Smith's height-correlated visibility (Heitz 2014) and Schlick's Fresnel.
// - The split sum's environment BRDF (Karis 2014, "Physically Based Shading on Mobile", after Lazarov 2013): the single-scatter
//   lobe's directional albedo as a scale and bias of F0.
// - Multiple scattering (Fdez-Aguera 2019; the compensation in the form of Turquin 2019): the energy a single-scatter GGX lobe
//   loses to its microfacets' shadowing, 1 - E(mu), given back in proportion to F0. A rough metal keeps its brightness.
// - The diffuse takes what the specular leaves (1 - its directional albedo), so a white surface under a uniform sky gives back
//   exactly the sky: the white furnace (`tests/white_furnace.rs`) holds every case within 1 %.
// - Specular occlusion from the ambient occlusion (Lagarde and de Rousiers 2014): an open cone of directions for a smooth lobe
//   facing the eye, the AO itself for a rough one.
// - The horizon fade of a reflection under the face's own normal (a bent normal's reflection that points into the surface sees
//   the surface, not the sky).
// - The environment's prefiltered mips (prefilter.wgsl) hold the GGX lobe of the roughness (m / (levels - 1))^2 at level m:
//   a lookup reads level sqrt(roughness) x (levels - 1).

// Schlick's Fresnel toward the grazing reflectance `f90`.
fn fresnel(f0: vec3<f32>, f90: vec3<f32>, vh: f32) -> vec3<f32> {
    return f0 + (f90 - f0) * pow(1.0 - clamp(vh, 0.0, 1.0), 5.0);
}

// GGX's distribution, Smith's height-correlated visibility, Schlick's Fresnel: the specular BRDF times pi (the diffuse convention of
// the fixed-function lighting: radiance = albedo x irradiance).
fn specular(n: vec3<f32>, v: vec3<f32>, l: vec3<f32>, f0: vec3<f32>, rough: f32) -> vec3<f32> {
    let h = normalize(v + l);
    let nl = max(dot(n, l), 0.0);
    let nv = max(dot(n, v), 1e-4);
    let nh = max(dot(n, h), 0.0);
    let vh = max(dot(v, h), 0.0);
    let a = rough * rough;
    let a2 = a * a;
    let d = nh * nh * (a2 - 1.0) + 1.0;
    let dist = a2 / (PI * d * d);
    let gv = nl * sqrt(nv * nv * (1.0 - a2) + a2);
    let gl = nv * sqrt(nl * nl * (1.0 - a2) + a2);
    let vis = 0.5 / max(gv + gl, 1e-6);
    // The grazing reflectance lowered for rough surfaces (F90 = max(1 - roughness, F0)): a rough plate seen edge-on does not turn
    // into a mirror under the sun.
    let f = fresnel(f0, max(vec3<f32>(1.0 - rough), f0), vh);
    return PI * dist * vis * f;
}

// The split sum's scale and bias of F0 (Karis 2014): the single-scatter lobe's directional albedo is f0 x ab.x + ab.y, and
// ab.x + ab.y is the albedo of a lobe of F0 1.
fn env_ab(rough: f32, nv: f32) -> vec2<f32> {
    let c0 = vec4<f32>(-1.0, -0.0275, -0.572, 0.022);
    let c1 = vec4<f32>(1.0, 0.0425, 1.04, -0.04);
    let r = rough * c0 + c1;
    let a004 = min(r.x * r.x, exp2(-9.28 * nv)) * r.x + r.y;
    return vec2<f32>(-1.04, 1.04) * a004 + r.zw;
}

// The split sum's environment BRDF: the share of the environment a single-scatter lobe of reflectance `f0` and roughness `rough`
// reflects at N.V `nv`.
fn env_brdf(f0: vec3<f32>, rough: f32, nv: f32) -> vec3<f32> {
    let ab = env_ab(rough, nv);
    return f0 * ab.x + ab.y;
}

// The multiple-scattering compensation of a lobe whose F0 1 albedo is `e` (1 + F0 (1 / E - 1)): the specular times it keeps the
// energy the single-scatter lobe loses.
fn multi_scatter(f0: vec3<f32>, e: f32) -> vec3<f32> {
    return 1.0 + f0 * (1.0 / max(e, 1e-3) - 1.0);
}

// The whole specular lobe's directional albedo (single and multiple scattering): what it reflects of a uniform environment, and
// what the diffuse under it does not get.
struct SpecAlbedo {
    // The environment's share (the split sum's, compensated).
    e: vec3<f32>,
    // The multiple-scattering compensation (the direct lights' specular times it).
    comp: vec3<f32>,
};

fn spec_albedo(f0: vec3<f32>, rough: f32, nv: f32) -> SpecAlbedo {
    let ab = env_ab(rough, nv);
    var out: SpecAlbedo;
    out.comp = multi_scatter(f0, ab.x + ab.y);
    out.e = min((f0 * ab.x + ab.y) * out.comp, vec3<f32>(1.0));
    return out;
}

// Lagarde and de Rousiers 2014's specular occlusion from the ambient occlusion `ao` at N.V `nv` (alpha the GGX roughness).
fn spec_occlusion(nv: f32, ao: f32, rough: f32) -> f32 {
    let a = rough * rough;
    return clamp(pow(nv + ao, exp2(-16.0 * a - 1.0)) - 1.0 + ao, 0.0, 1.0);
}

// The share of a reflection along `r` left by the horizon of the face's own normal `ng` (none of it once `r` points into it).
fn horizon_fade(r: vec3<f32>, ng: vec3<f32>) -> f32 {
    let h = clamp(1.0 + 1.1 * dot(r, ng), 0.0, 1.0);
    return h * h;
}

// The prefiltered chain's level for roughness `rough` (the chain has `levels` levels).
fn env_lod(rough: f32, levels: f32) -> f32 {
    return sqrt(clamp(rough, 0.0, 1.0)) * (levels - 1.0);
}

// The roughness a prefiltered level `m` of a chain of `levels` holds (env_lod's inverse).
fn level_rough(m: f32, levels: f32) -> f32 {
    let t = m / max(levels - 1.0, 1.0);
    return t * t;
}

// The i-th of n points of the Hammersley set (the radical inverse in base 2).
fn hammersley(i: u32, n: u32) -> vec2<f32> {
    return vec2<f32>(f32(i) / f32(n), f32(reverseBits(i)) * 2.3283064365386963e-10);
}

// A half vector about +z drawn from GGX's distribution of normals for alpha `a` (Karis 2013).
fn ggx_half(xi: vec2<f32>, a: f32) -> vec3<f32> {
    let phi = 2.0 * PI * xi.x;
    let cos_t = sqrt((1.0 - xi.y) / (1.0 + (a * a - 1.0) * xi.y));
    let sin_t = sqrt(max(1.0 - cos_t * cos_t, 0.0));
    return vec3<f32>(sin_t * cos(phi), sin_t * sin(phi), cos_t);
}
