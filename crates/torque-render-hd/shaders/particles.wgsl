// Lit particles (composed after forward.wgsl): a particle sprite marked MAT_LIT is lit as a small volume instead of drawn at its
// color. Its own functions only; forward.wgsl's `shaded` calls `particle_lit` and fogs it.
//
// - The sprite's center: the pixel less the interpolated normal (each corner's direction from the center) times the half
//   diagonal (`extra`). The shading normal is the eye's direction bent outward by that normal: a dome over the sprite.
// - The sun: one bilinear comparison in the first cascade that holds the center (no filter, no contact shadows), faded out toward
//   the shadow distance, times the clouds' shadow, through the shadow color as surfaces take it. Half-Lambert wrap (Mitchell,
//   McTaggart and Green 2006) on the bent normal plus a forward-scattering lobe when the eye looks toward the sun.
// - The ambient: the sky's (`ambient`, the scene's ambient shaped by the sky's SH) on the bent normal; it dims at dusk and night
//   with everything else.
// - The lights: at most PARTICLE_LIGHTS of the pixel's cluster (in order without clusters), wrapped; a shadowed light's cube
//   compared once at the pixel.

const PARTICLE_LIGHTS: u32 = 8u;
// How far the sprite's corners bend the normal away from the eye.
const PARTICLE_BEND: f32 = 1.0;
// The forward-scattering lobe: its strength and power over the eye's direction toward the sun.
const PARTICLE_FORWARD: f32 = 0.6;
const PARTICLE_FORWARD_POWER: f32 = 6.0;

// Whether a material word is a lit particle (not a decal).
fn lit_particle(material: u32) -> bool {
    return (material & (MAT_PARTICLE | MAT_LIT | MAT_DECAL)) == (MAT_PARTICLE | MAT_LIT);
}

fn particle_wrap(n: vec3<f32>, l: vec3<f32>) -> f32 {
    return 0.5 + 0.5 * dot(n, l);
}

// The sun's visibility at the sprite's center `c`: one bilinear comparison in the first cascade that holds it.
fn particle_sun_shadow(c: vec3<f32>) -> f32 {
    let count = min(shadows.config.x, MAX_CASCADES);
    if count == 0u {
        return 1.0;
    }
    let view_depth = -(frame.view * vec4<f32>(c, 1.0)).z;
    if view_depth >= shadows.sun.y {
        return 1.0;
    }
    for (var i = 0u; i < count; i = i + 1u) {
        let prm = shadows.params[i];
        let q = cascade_uvz(i, c);
        let m = prm.x * 2.0;
        if all(q.xy > vec2<f32>(m)) && all(q.xy < vec2<f32>(1.0 - m)) && q.z < 1.0 {
            let lit = csm_tap(i, q.xy, q.z - 2e-4 / prm.z, vec2<f32>(0.0));
            return mix(lit, 1.0, smoothstep(shadows.sun.z, shadows.sun.y, view_depth));
        }
    }
    return 1.0;
}

// Light `i`'s light at `p` on the bent normal `n` (linear), its cube compared once when it casts shadows.
fn particle_point(i: u32, p: vec3<f32>, n: vec3<f32>) -> vec3<f32> {
    let light = lights[i];
    let to = light.pos_radius.xyz - p;
    let d = length(to);
    if d >= light.pos_radius.w || d < 1e-6 {
        return vec3<f32>(0.0);
    }
    let l = to / d;
    let share = particle_wrap(n, l) * cone_share(light, l) * falloff(d, light.pos_radius.w);
    if share <= 0.0 {
        return vec3<f32>(0.0);
    }
    var vis = 1.0;
    let slot = i32(light.color.w);
    if slot >= 0 && shadows.point.z > 0.0 {
        let radius = max(light.pos_radius.w, 1e-4);
        let bias = 0.05 + 2.0 * d / max(shadows.point.y, 1.0);
        vis = textureSampleCompareLevel(point_maps, shadow_cmp, -l, slot, (d - bias) / radius);
    }
    return light.color.rgb * (share * vis);
}

// The lit color (linear) of a lit particle's pixel of linear color `albedo`.
fn particle_lit(in: VertexOut, albedo: vec3<f32>) -> vec3<f32> {
    let p = in.world;
    let v = normalize(frame.eye.xyz - p);
    let n = normalize(v + in.normal * PARTICLE_BEND);
    let c = p - in.normal * bitcast<f32>(in.extra);
    let l = frame.sun_dir.xyz;
    let vis = particle_sun_shadow(c) * cloud_sun(c, sky.cloud_use.x);
    let forward = PARTICLE_FORWARD * pow(max(dot(-v, l), 0.0), PARTICLE_FORWARD_POWER);
    let w = max(particle_wrap(n, l), 0.0) + forward;
    let amb_g = ambient(n);
    var out: vec3<f32>;
    if frame.surf0.w > 0.5 {
        let through = vis + (1.0 - vis) * srgb_to_linear(shadows.tint.rgb);
        let amb = srgb_to_linear(amb_g);
        let sun = max(srgb_to_linear(amb_g + frame.sun_diffuse.rgb * w) - amb, vec3<f32>(0.0));
        out = albedo * (amb + sun * through);
    } else {
        let through = vis + (1.0 - vis) * shadows.tint.rgb;
        out = srgb_to_linear(linear_to_srgb(albedo) * (amb_g + frame.sun_diffuse.rgb * (w * through)));
    }
    let count = min(frame.clusters.w, MAX_LIGHTS);
    let cap = min(frame.limits.x, PARTICLE_LIGHTS);
    var e = vec3<f32>(0.0);
    var shaded = 0u;
    if CLUSTERED {
        let base = cluster_of(in.clip.xy, p) * CLUSTER_WORDS;
        for (var k = 0u; k < CLUSTER_WORDS && shaded < cap; k = k + 1u) {
            var bits = masks[base + k];
            loop {
                if bits == 0u || shaded >= cap {
                    break;
                }
                let i = k * 32u + firstTrailingBit(bits);
                bits = bits & (bits - 1u);
                if i < count {
                    e = e + particle_point(i, p, n);
                    shaded = shaded + 1u;
                }
            }
        }
    } else {
        for (var i = 0u; i < count && shaded < cap; i = i + 1u) {
            if distance(lights[i].pos_radius.xyz, p) < lights[i].pos_radius.w {
                e = e + particle_point(i, p, n);
                shaded = shaded + 1u;
            }
        }
    }
    return out + albedo * e;
}
