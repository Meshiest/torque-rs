// The voxels' light (after gi_common.wgsl, gi_scene.wgsl, area.wgsl and the forward pass's `Light`, `cone_share` and `falloff`,
// taken from common.wgsl and forward.wgsl as they are): eight workgroups a tile of the frame's list (a layer each), each filled voxel's radiance
// its albedo times the light on it (the sun's, the point, spot and area lights' as the forward pass shades a face of the
// voxel's normal at its middle, each shadowed by a march through the voxels toward it) and the ambient the probes hold there,
// plus its emission. The glowing surfaces' lights are left out: their surfaces' voxels emit.

struct LightParams {
    // xyz toward the sun, w 1 when the sun lights.
    sun_dir: vec4<f32>,
    // rgb the sun's linear light (what a face toward it takes), w unused.
    sun: vec4<f32>,
    // x the lights, y where the area lights begin, z, w the glowing surfaces' lights (from, to).
    lights: vec4<u32>,
    // x the tiles listed, y 1 when the probes' ambient is added (the bounce), zw unused.
    flags: vec4<u32>,
};

// A light's share under which a voxel takes none of it (its shadow march is not worth its cost).
const FAINT: f32 = 1e-3;

@group(0) @binding(0) var<uniform> vol: GiVolume;
@group(0) @binding(1) var<uniform> lp: LightParams;
@group(0) @binding(2) var<storage, read> lights: array<Light>;
@group(0) @binding(3) var<storage, read> tiles: array<vec2<u32>>;
@group(0) @binding(4) var<storage, read> occ: array<u32>;
@group(0) @binding(5) var<storage, read_write> voxels: array<vec4<u32>>;
// The tiles to light: xyz the world tile, w its level.
@group(0) @binding(6) var<storage, read> list: array<vec4<i32>>;

// How much of the light along `l` reaches `p` (a voxel's middle, the march from a voxel's step out of its surface along its
// normal `n`) from `far` away: 0 when the march meets a filled voxel within `far` less two voxels.
fn visible(p: vec3<f32>, n: vec3<f32>, l: vec3<f32>, far: f32, level: u32) -> f32 {
    let s = vol.levels[level].size.x;
    let o = p + n * (1.01 * s);
    let reach = far - 2.0 * s;
    if reach <= 0.0 {
        return 1.0;
    }
    let hit = gi_march(o, l, reach, level);
    return select(1.0, 0.0, hit.hit);
}

// The light on a voxel's face of normal `n` at `p` (the forward pass's units: what multiplies the albedo).
fn voxel_light(p: vec3<f32>, n: vec3<f32>, level: u32) -> vec3<f32> {
    var e = vec3<f32>(0.0);
    let sl = dot(n, lp.sun_dir.xyz);
    if lp.sun_dir.w > 0.5 && sl > 0.0 {
        e = e + lp.sun.rgb * (sl * visible(p, n, lp.sun_dir.xyz, 1e6, level));
    }
    let count = lp.lights.x;
    for (var i = 0u; i < count; i = i + 1u) {
        if i >= lp.lights.z && i < lp.lights.w {
            continue;
        }
        let light = lights[i];
        let to = light.pos_radius.xyz - p;
        let d = length(to);
        if d >= light.pos_radius.w || d < 1e-4 {
            continue;
        }
        let l = to / d;
        var lit = vec3<f32>(0.0);
        if i >= lp.lights.y && light.area_a.w != 0.0 {
            let x = d / max(light.pos_radius.w, 1e-4);
            let w = clamp(1.0 - x * x * x * x, 0.0, 1.0);
            let m = area_frame(n, n);
            let shares = area_shares(light.area_a, light.area_b, light.pos_radius.xyz, p, m, m);
            if shares.z <= 0.0 || shares.x <= 0.0 {
                continue;
            }
            lit = light.color.rgb * (PI * shares.z * w * w * shares.x);
            // The march stops short of the shape's own voxels (the light's emitter, drawn about it): a rectangle's two voxels
            // before its plane, a tube's two past its radius.
            let s = vol.levels[level].size.x;
            var far = d - light.area_b.w - s;
            if light.area_a.w == 1.0 {
                let ln = normalize(cross(light.area_a.xyz, light.area_b.xyz));
                let along = dot(l, ln);
                if along < -1e-4 {
                    far = min(d, dot(light.pos_radius.xyz + ln * (2.0 * s) - p, ln) / along + 2.0 * s);
                }
            }
            if max(lit.r, max(lit.g, lit.b)) > FAINT {
                e = e + lit * visible(p, n, l, far, level);
            }
            continue;
        } else {
            let nl = max(dot(n, l), 0.0);
            let cone = cone_share(light, l);
            if nl <= 0.0 || cone <= 0.0 {
                continue;
            }
            lit = light.color.rgb * (nl * cone * falloff(d, light.pos_radius.w));
        }
        if max(lit.r, max(lit.g, lit.b)) > FAINT {
            e = e + lit * visible(p, n, l, d, level);
        }
    }
    return e;
}

@compute @workgroup_size(64)
fn cs_light(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    // Eight workgroups a tile, one a layer of its voxels (one a thread): the voxels' marches spread over the GPU.
    let index = (wg.x + 65535u * wg.y) / 8u;
    let k = (wg.x + 65535u * wg.y) % 8u;
    if index >= lp.flags.x {
        return;
    }
    let entry = list[index];
    let level = u32(entry.w);
    let tile = entry.xyz;
    let slot = gi_slot(level, tile);
    let info = tiles[slot];
    if info.x != gi_tag(tile) || info.y == 0u {
        return;
    }
    let s = vol.levels[level].size.x;
    {
        let i = li + 64u * k;
        if (occ[slot * GI_TILE_WORDS + (i >> 5u)] & (1u << (i & 31u))) == 0u {
            return;
        }
        let at = slot * GI_TILE_VOXELS + i;
        let w = voxels[at];
        let local = vec3<i32>(i32(i & 7u), i32((i >> 3u) & 7u), i32(i >> 6u));
        let n = gi_unoct(unpack2x16snorm(w.y));
        // Shaded at its surface, not at its middle.
        let p = (vec3<f32>(tile * GI_TILE + local) + 0.5) * s + n * (gi_plane(w.x) * s);
        let albedo = gi_unpack_albedo(w.x);
        var e = voxel_light(p, n, level);
        if lp.flags.y != 0u {
            e = e + gi_voxel_ambient(p, n, level);
        }
        voxels[at].w = gi_pack_rgb9e5(albedo * e + gi_unpack_rgb9e5(w.z));
    }
}
