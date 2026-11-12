// The physically based materials' forward pass (the PBR permutations: forward.wgsl's module with this file after it). A draw of a
// material reads its maps here and is lit by forward.wgsl's own shade_surf, with the inputs from the maps in place of the class
// constants: the normal through the tangent frame, roughness and metalness from the ORM map times the factors, F0 the dielectric's
// 0.04 or the metal's albedo, the occlusion map on the ambient and the environment's light only (the least of it and the screen's
// AO), the emission added. The linear response always (a material's maps are physically based). Nothing here changes a draw
// without a material: those are drawn by the other permutations, whose source this file is not part of.

// Group 1 of a material's draw: forward.wgsl's base color (binding 0, `tex`) and sampler (1, `samp`), then the maps and the
// material's uniform (torque_render_scene::hd::pbr::HdMaterial::uniform_bytes).
struct PbrMaterial {
    base_factor: vec4<f32>,
    emissive: vec4<f32>,
    // Roughness, metalness, the occlusion's strength, the normal's scale.
    params: vec4<f32>,
    // Height scale (meters at 1), alpha cutoff, UV scale.
    params2: vec4<f32>,
    // x: the maps present (PBR_*), y: the alpha mode (0 opaque, 1 cutout, 2 blend), z: two-sided (1).
    flags: vec4<u32>,
};
@group(1) @binding(2) var pbr_normal: texture_2d<f32>;
@group(1) @binding(3) var pbr_orm: texture_2d<f32>;
@group(1) @binding(4) var pbr_height: texture_2d<f32>;
@group(1) @binding(5) var<uniform> pbr: PbrMaterial;
@group(1) @binding(6) var pbr_emissive: texture_2d<f32>;

const PBR_NORMAL: u32 = 1u;
const PBR_ORM: u32 = 2u;
const PBR_HEIGHT: u32 = 4u;
const PBR_EMISSIVE: u32 = 8u;
const PBR_ALPHA_CUTOUT: u32 = 1u;
// The parallax march's steps (fewest seen head on, most at grazing angles) and the distance it fades out by.
const PARALLAX_MIN_STEPS: f32 = 8.0;
const PARALLAX_MAX_STEPS: f32 = 32.0;
const PARALLAX_FADE: f32 = 30.0;

struct PbrVertexIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec2<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) color: vec4<f32>,
    @location(4) material: u32,
    @location(5) extra: u32,
    // The second stream: the tangent octahedral in xy, the bitangent's sign in z.
    @location(6) tangent: vec4<f32>,
};

// forward.wgsl's VertexOut and the tangent (xyz, the bitangent's sign in w).
struct PbrOut {
    @builtin(position) @invariant clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) color: vec4<f32>,
    @location(4) @interpolate(flat) material: u32,
    @location(5) @interpolate(flat) extra: u32,
    @location(6) rest: vec3<f32>,
    @location(7) lod: f32,
    @location(8) tangent: vec4<f32>,
};

// vs_main's position (the same expression: the prepass's depth is met exactly) with the tangent turned as the normal is.
@vertex
fn vs_pbr(v: PbrVertexIn) -> PbrOut {
    var out: PbrOut;
    let at = placed(v.position, v.material, v.extra);
    let p = fx_position(at, v.material);
    out.clip = frame.view_proj * p;
    out.world = p.xyz;
    out.rest = at;
    out.lod = 0.0;
    out.normal = placed_normal(oct_decode(v.normal), v.material, v.extra);
    out.tangent = vec4<f32>(placed_normal(oct_decode(v.tangent.xy), v.material, v.extra), select(1.0, -1.0, v.tangent.z < 0.0));
    out.uv = v.uv;
    out.color = v.color;
    out.material = v.material;
    out.extra = select(v.extra, 0u, (v.material & MAT_NODE) != 0u);
    return out;
}

fn pbr_vertex_out(in: PbrOut) -> VertexOut {
    var out: VertexOut;
    out.clip = in.clip;
    out.world = in.world;
    out.normal = in.normal;
    out.uv = in.uv * pbr.params2.zw;
    out.color = in.color;
    out.material = in.material;
    out.extra = in.extra;
    out.rest = in.rest;
    out.lod = in.lod;
    return out;
}

// The tangent frame at a fragment: its tangent, bitangent and geometric normal (a two-sided material's turned to the eye's side
// where it faces away: by the normal, not the winding, which a mesh's maker may have either way).
struct Frame3 {
    t: vec3<f32>,
    b: vec3<f32>,
    n: vec3<f32>,
};

fn pbr_frame(in: PbrOut) -> Frame3 {
    var f: Frame3;
    var n = normalize(in.normal);
    let v = normalize(frame.eye.xyz - in.world);
    if (in.material & MAT_ZERO_NORMAL) != 0u || dot(in.normal, in.normal) < 1e-8 {
        n = v;
    }
    if pbr.flags.z != 0u && dot(n, v) < 0.0 {
        n = -n;
    }
    var t = in.tangent.xyz - n * dot(n, in.tangent.xyz);
    if dot(t, t) < 1e-10 {
        t = select(vec3<f32>(1.0, 0.0, 0.0), vec3<f32>(0.0, 1.0, 0.0), abs(n.x) > 0.9);
        t = t - n * dot(n, t);
    }
    f.t = normalize(t);
    f.n = n;
    f.b = cross(n, f.t) * in.tangent.w;
    return f;
}

// Parallax occlusion mapping: the UV where the eye's ray meets the height field under `uv` (the map's 1 the surface, 0 its
// depth), faded out with distance. `m_per_uv` is the surface's meters per unit of UV at the pixel.
fn pbr_parallax(uv: vec2<f32>, f: Frame3, v: vec3<f32>, d: Derivs, dist: f32) -> vec2<f32> {
    let fade = 1.0 - smoothstep(0.7 * PARALLAX_FADE, PARALLAX_FADE, dist);
    let area_uv = abs(d.duvx.x * d.duvy.y - d.duvx.y * d.duvy.x);
    let area_p = length(cross(d.dpx, d.dpy));
    if fade <= 0.0 || area_uv < 1e-14 || pbr.params2.x <= 0.0 {
        return uv;
    }
    let m_per_uv = sqrt(area_p / area_uv);
    let depth = pbr.params2.x / max(m_per_uv, 1e-6) * fade;
    let vt = vec3<f32>(dot(v, f.t), dot(v, f.b), dot(v, f.n));
    let vz = max(vt.z, 0.05);
    let steps = mix(PARALLAX_MAX_STEPS, PARALLAX_MIN_STEPS, clamp(vz, 0.0, 1.0));
    let layer = 1.0 / steps;
    // UV moves against the eye's direction as the ray goes down (the map's V and the bitangent run the same way).
    let delta = vt.xy / vz * depth * layer;
    var cur = uv;
    var below = 0.0;
    var h = 1.0 - textureSampleGrad(pbr_height, samp, cur, d.duvx, d.duvy).r;
    var prev_uv = cur;
    var prev_gap = h;
    for (var i = 0; i < i32(PARALLAX_MAX_STEPS); i = i + 1) {
        if below >= h || f32(i) >= steps {
            break;
        }
        prev_uv = cur;
        prev_gap = h - below;
        cur = cur - delta;
        below = below + layer;
        h = 1.0 - textureSampleGrad(pbr_height, samp, cur, d.duvx, d.duvy).r;
    }
    // Between the last step above the field and the first below it.
    let after = h - below;
    let k = clamp(prev_gap / max(prev_gap - after, 1e-5), 0.0, 1.0);
    return mix(prev_uv, cur, k);
}

// What a material's fragment is shaded with.
struct PbrSurface {
    s: Surf,
    emission: vec3<f32>,
    // The ambient's occlusion as the map gives it (1 none).
    map_occlusion: f32,
};

fn pbr_surface(in: PbrOut, vin: VertexOut, d: Derivs) -> PbrSurface {
    var out: PbrSurface;
    let f = pbr_frame(in);
    let v = normalize(frame.eye.xyz - in.world);
    var uv = vin.uv;
    let has = pbr.flags.x;
    if PARALLAX && (has & PBR_HEIGHT) != 0u {
        uv = pbr_parallax(uv, f, v, d, distance(in.world, frame.eye.xyz));
    }
    let base = textureSampleGrad(tex, samp, uv, d.duvx, d.duvy) * pbr.base_factor;
    let paint = vec4<f32>(srgb_to_linear(in.color.rgb), in.color.a);
    let albedo = base * paint;
    var n = f.n;
    if (has & PBR_NORMAL) != 0u {
        let raw = textureSampleGrad(pbr_normal, samp, uv, d.duvx, d.duvy).xy * 2.0 - 1.0;
        let z = sqrt(clamp(1.0 - dot(raw, raw), 0.0, 1.0));
        let m = normalize(vec3<f32>(raw * pbr.params.w, z));
        n = normalize(f.t * m.x + f.b * m.y + f.n * m.z);
    }
    var orm = vec3<f32>(1.0);
    if (has & PBR_ORM) != 0u {
        orm = textureSampleGrad(pbr_orm, samp, uv, d.duvx, d.duvy).rgb;
    }
    let metal = clamp(orm.b * pbr.params.y, 0.0, 1.0);
    var s: Surf;
    s.albedo = albedo.rgb;
    s.alpha = albedo.a;
    s.n = n;
    s.ng = f.n;
    s.rough = clamp(orm.g * pbr.params.x, 0.02, 1.0);
    s.f0 = mix(vec3<f32>(0.04), albedo.rgb, metal);
    s.metal = metal;
    s.reflects = true;
    s.clearcoat = 0.0;
    s.occlusion = 1.0;
    s.sun = 1.0;
    s.rim = 0.0;
    s.linear = true;
    s.hollow = 0.0;
    s.spec_level = 1.0;
    out.s = s;
    out.map_occlusion = mix(1.0, orm.r, pbr.params.z);
    out.emission = pbr.emissive.rgb;
    if (has & PBR_EMISSIVE) != 0u {
        out.emission = out.emission * textureSampleGrad(pbr_emissive, samp, uv, d.duvx, d.duvy).rgb;
    }
    return out;
}

// forward.wgsl's `shaded` for a material's fragment: lit (the map's occlusion as the least of it and the screen's on the
// ambient), the emission added, then fogged, clouded and under the water as every surface is.
fn pbr_shaded(in: PbrOut, vin: VertexOut, d: Derivs, spec_kernel: f32, opaque_pass: bool) -> Shaded {
    var out: Shaded;
    let material = in.material;
    let kind = (material >> MAT_KIND_SHIFT) & 3u;
    if shadows.config.w == 4u || shadows.config.w == 5u || shadows.config.w == 6u {
        debug_out = vec4<f32>(0.0);
    }
    var ps = pbr_surface(in, vin, d);
    out.alpha = ps.s.alpha;
    if (pbr.flags.y == PBR_ALPHA_CUTOUT) {
        out.alpha = 1.0;
    }
    out.n = ps.s.n;
    let screen = ambient_occlusion(in.clip.xy, in.clip.z);
    // shade_surf takes mix(1, screen, strength) x s.occlusion: s.occlusion makes that product the least of the two.
    let g = mix(1.0, screen, frame.ao.y);
    ps.s.occlusion = min(ps.map_occlusion, g) / max(g, 1e-4);
    // A cutout's pixels are not in the prepass (nor its contact shadows' mask): shaded as a translucent surface's are.
    let opaque = opaque_pass && pbr.flags.y == 0u && out.alpha >= 0.999;
    let l = shade_surf(in.clip.xy, in.world, in.rest, ps.s, material, opaque, screen, spec_kernel);
    out.diffuse = l.diffuse + ps.emission;
    out.spec = l.spec;
    if shadows.config.w == 3u {
        debug_out = vec4<f32>(screen, screen, screen, 1.0);
    }
    out.debug = shadows.config.w != 0u && frame.flags.y < 0.5;
    if frame.fog_color.w > 0.0 {
        var keep = fog_keep(in.world);
        keep = ext_fog_keep(kind, in.world, keep);
        out.diffuse = mix(fog_color_toward(in.clip.xy, in.world, keep), out.diffuse, keep);
        out.spec = out.spec * keep;
    }
    let over = clouds_over_share(in.world);
    if over > 0.0 {
        let c = clouds_at(in.clip.xy);
        let t = mix(1.0, clamp(c.a, 0.0, 1.0), over);
        out.diffuse = out.diffuse * t + max(c.rgb, vec3<f32>(0.0)) * over;
        out.spec = out.spec * t;
    }
    if frame.water.z > 0.5 {
        let keep = under_water_keep(in.world);
        out.diffuse = mix(frame.under.rgb, out.diffuse, keep);
        out.spec = out.spec * keep;
    }
    return out;
}

// The derivatives and the specular AA's kernel, in uniform control flow (as forward_color takes them).
fn pbr_derivs(vin: VertexOut) -> Derivs {
    return derivs(vin);
}

fn pbr_kernel(in: PbrOut) -> f32 {
    let dndx = dpdx(in.normal);
    let dndy = dpdy(in.normal);
    return min(2.0 * 0.25 * (dot(dndx, dndx) + dot(dndy, dndy)), 0.18);
}

// Whether a cutout material's fragment is cut (the base's alpha under the cutoff).
fn pbr_cut(vin: VertexOut, d: Derivs, color_a: f32) -> bool {
    if pbr.flags.y != PBR_ALPHA_CUTOUT {
        return false;
    }
    let a = textureSampleGrad(tex, samp, vin.uv, d.duvx, d.duvy).a * pbr.base_factor.a * color_a;
    return a < pbr.params2.y;
}

@fragment
fn fs_pbr(in: PbrOut) -> @location(0) vec4<f32> {
    let vin = pbr_vertex_out(in);
    let spec_kernel = pbr_kernel(in);
    let d = pbr_derivs(vin);
    if pbr_cut(vin, d, in.color.a) {
        discard;
    }
    let s = pbr_shaded(in, vin, d, spec_kernel, true);
    if s.debug {
        return debug_out;
    }
    let a = s.alpha;
    if FRONT_LAYER && a > 1e-3 && a < 0.999 {
        let px = vec2<i32>(in.clip.xy);
        let dims = vec2<i32>(textureDimensions(background));
        let nv = (frame.view * vec4<f32>(s.n, 0.0)).xy;
        let off = vec2<i32>(round(nv * vec2<f32>(1.0, -1.0) * frame.surf1.w * (1.0 - a)));
        let bg = textureLoad(background, px, 0).rgb;
        let bgr = textureLoad(background, clamp(px + off, vec2<i32>(0), dims - 1), 0).rgb;
        var wanted: vec3<f32>;
        if frame.surf2.z > 0.5 {
            wanted = srgb_to_linear(premultiplied(s) + linear_to_srgb(bgr) * (1.0 - a));
        } else {
            wanted = premultiplied(s) + bgr * (1.0 - a);
        }
        return vec4<f32>((wanted - bg * (1.0 - a)) / a, a);
    }
    return vec4<f32>(s.diffuse + s.spec, a);
}

// A blended material's fragment behind the front layer into the weighted blend (forward.wgsl's fs_wboit).
@fragment
fn fs_pbr_wboit(in: PbrOut) -> WboitOut {
    let vin = pbr_vertex_out(in);
    let spec_kernel = pbr_kernel(in);
    let d = pbr_derivs(vin);
    let s = pbr_shaded(in, vin, d, spec_kernel, false);
    let front_depth = textureLoad(trans_front, vec2<i32>(in.clip.xy), 0);
    if in.clip.z >= front_depth || s.alpha <= 1e-3 {
        discard;
    }
    let a = clamp(s.alpha, 0.0, 1.0);
    let z = linear_depth(in.clip.z);
    let w = a * clamp(80.0 / (1.0 + pow(z / 8.0, 3.0)), 0.01, 100.0);
    var out: WboitOut;
    out.accum = vec4<f32>(premultiplied(s) * w, a * w);
    out.reveal = vec4<f32>(a, 0.0, 0.0, 0.0);
    return out;
}
