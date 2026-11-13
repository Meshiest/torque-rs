// The probes' frame (after gi_common.wgsl, gi_scene.wgsl, gi_grid.wgsl and gi_sky.wgsl): one workgroup a probe. A probe that a
// cascade's move brought in starts again; a probe traced this frame (a new one, a young one, one whose turn it is, or every
// one when the frame says so) casts its rays through the voxels (a fixed spherical Fibonacci set turned by the frame's
// rotation, so the same frames give the same probes), takes the lit radiance of the voxel each meets (nothing from a voxel's
// back: the inside of a wall) or the sky's past the clipmap, moves out of a wall its rays found it in, and blends what its
// rays saw into its irradiance and depth moments (a large change faster), then writes its atlas tiles with their borders.

struct ProbeParams {
    // The ray set's turn (its upper 3 x 3).
    rotation: mat4x4<f32>,
    // x the rays a probe, y the frames between a probe's turns, z the frame, w 1 when every probe is traced.
    rays: vec4<u32>,
    // x the hysteresis, y the hysteresis after a large change, z the change that is large, w the depth's reach in spacings.
    blend: vec4<f32>,
    // x the cascades in use, yzw unused.
    shape: vec4<u32>,
};

@group(0) @binding(0) var<uniform> vol: GiVolume;
@group(0) @binding(1) var<uniform> pp: ProbeParams;
@group(0) @binding(2) var<storage, read> tiles: array<vec2<u32>>;
@group(0) @binding(3) var<storage, read> occ: array<u32>;
@group(0) @binding(4) var<storage, read> voxels: array<vec4<u32>>;
@group(0) @binding(5) var<storage, read_write> gi_state: GiState;
// Each probe's words: its coordinates and age, its irradiance texels (three floats each), its depth moments (two each).
@group(0) @binding(6) var<storage, read_write> data: array<u32>;
@group(0) @binding(7) var atlas: texture_storage_2d<rgba16float, write>;
@group(0) @binding(8) var<uniform> gi_sky: GiSky;

const PROBE_WORDS: u32 = 240u;
const IRR_TEXELS: u32 = 36u;
const DEPTH_TEXELS: u32 = 64u;
const MAX_RAYS: u32 = 128u;
// A young probe is traced every frame until this age.
const YOUNG: u32 = 8u;

var<workgroup> rays: array<vec4<f32>, 128>;
var<workgroup> dirs: array<vec3<f32>, 128>;
var<workgroup> irr: array<vec3<f32>, 36>;
var<workgroup> depth: array<vec2<f32>, 64>;
var<workgroup> head: vec4<u32>;
var<workgroup> traced: u32;
var<workgroup> spot: vec4<f32>;

// The `i`th of `n` directions of the spherical Fibonacci set.
fn fibonacci(i: u32, n: u32) -> vec3<f32> {
    let z = 1.0 - (2.0 * f32(i) + 1.0) / f32(n);
    let r = sqrt(max(1.0 - z * z, 0.0));
    let phi = f32(i) * 2.399963229728653;
    return vec3<f32>(r * cos(phi), r * sin(phi), z);
}

// A texel's direction in an octahedral tile of `inner` texels a side.
fn texel_dir(i: u32, inner: u32) -> vec3<f32> {
    let t = vec2<f32>(f32(i % inner), f32(i / inner));
    return gi_unoct((t + 0.5) / f32(inner) * 2.0 - 1.0);
}

// The interior texel (0-based) a border texel of a tile of `inner` texels copies (the octahedron's edges fold onto
// themselves), or the texel itself inside.
fn border_source(x: i32, y: i32, inner: i32) -> vec2<i32> {
    let last = inner + 1;
    var sx = x;
    var sy = y;
    if (x == 0 || x == last) && (y == 0 || y == last) {
        sx = select(1, inner, x == 0);
        sy = select(1, inner, y == 0);
    } else if y == 0 || y == last {
        sx = last - x;
        sy = select(inner, 1, y == 0);
    } else if x == 0 || x == last {
        sx = select(inner, 1, x == 0);
        sy = last - y;
    }
    return vec2<i32>(sx - 1, sy - 1);
}

@compute @workgroup_size(64)
fn cs_probe(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let index = wg.x + 65535u * wg.y;
    let cascade = index / GI_CASCADE_PROBES;
    if cascade >= pp.shape.x {
        return;
    }
    let local = index % GI_CASCADE_PROBES;
    let s = vec3<i32>(
        i32(local % u32(GI_PROBES_X)),
        i32((local / u32(GI_PROBES_X)) % u32(GI_PROBES_Y)),
        i32(local / u32(GI_PROBES_X * GI_PROBES_Y)),
    );
    let c = gi_state.probes.cascades[cascade];
    let g = vec3<i32>(GI_PROBES_X, GI_PROBES_Y, GI_PROBES_Z);
    let coord = c.origin.xyz + (((s - c.origin.xyz) % g) + g) % g;
    let base = index * PROBE_WORDS;
    if li == 0u {
        let stored = vec3<i32>(bitcast<i32>(data[base]), bitcast<i32>(data[base + 1u]), bitcast<i32>(data[base + 2u]));
        var age = data[base + 3u];
        if any(stored != coord) {
            age = 0u;
        }
        let phase = u32(coord.x * 7 + coord.y * 13 + coord.z * 29 + 1048576) + pp.rays.z;
        let turn = phase % max(pp.rays.y, 1u) == 0u;
        traced = select(0u, 1u, pp.rays.w != 0u || age < YOUNG || turn);
        head = vec4<u32>(0u, 0u, 0u, age);
        var offset = gi_state.probe[index].xyz;
        if age == 0u {
            offset = vec3<f32>(0.0);
        }
        spot = vec4<f32>(offset, 0.0);
    }
    if workgroupUniformLoad(&traced) == 0u {
        return;
    }
    let age = head.w;
    let h = c.spacing.x;
    // Every cascade marches from the finest voxels (the march moves out to coarser levels as its rays leave them): a coarse
    // cascade sees thin walls as thin as the finest probes do.
    let level = 0u;
    let s0 = vol.levels[level].size.x;
    let origin = (vec3<f32>(coord) + GI_PROBE_SHIFT) * h + spot.xyz;
    let count = min(pp.rays.x, MAX_RAYS);
    let turn = mat3x3<f32>(pp.rotation[0].xyz, pp.rotation[1].xyz, pp.rotation[2].xyz);
    // A probe behind a voxel's surface (in a wall) is switched off: its neighbors in the open stand for it.
    let inside = gi_solid_at(level, origin);
    for (var r = li; r < count; r = r + 64u) {
        let d = normalize(turn * fibonacci(r, count));
        dirs[r] = d;
        let hit = gi_march(origin, d, 1e5, level);
        if hit.hit {
            let w = voxels[hit.voxel];
            let n = gi_unoct(unpack2x16snorm(w.y));
            // A back face, or a probe inside a filled voxel: nothing seen, and a hit the probe moves away from (a probe on a
            // surface sees it at no distance from the front).
            if dot(n, d) > 0.0 || (inside && hit.t <= 0.0) {
                rays[r] = vec4<f32>(0.0, 0.0, 0.0, -max(hit.t, 1e-3));
            } else {
                rays[r] = vec4<f32>(gi_unpack_rgb9e5(w.w), max(hit.t, 1e-3));
            }
        } else {
            rays[r] = vec4<f32>(gi_sky_radiance(d), 1e5);
        }
    }
    workgroupBarrier();
    // The probe moves out of a wall its rays found it in: toward the nearest back face's far side, at most most of the way
    // to its neighbors; a probe still mostly inside is switched off.
    if li == 0u {
        var backs = 0u;
        var near_t = 1e9;
        var near_d = vec3<f32>(0.0);
        for (var r = 0u; r < count; r = r + 1u) {
            if rays[r].w < 0.0 {
                backs = backs + 1u;
                if -rays[r].w < near_t {
                    near_t = -rays[r].w;
                    near_d = dirs[r];
                }
            }
        }
        var offset = spot.xyz;
        let share = f32(backs) / f32(count);
        if share > 0.25 {
            offset = clamp(offset + near_d * (near_t + 0.5 * s0), vec3<f32>(-0.45 * h), vec3<f32>(0.45 * h));
        }
        spot = vec4<f32>(offset, select(1.0, 0.0, share > 0.5 || inside));
    }
    workgroupBarrier();
    let fresh = age == 0u;
    let reach = pp.blend.w * h;
    // Irradiance.
    if li < IRR_TEXELS {
        let t = texel_dir(li, 6u);
        var sum = vec3<f32>(0.0);
        var wsum = 0.0;
        for (var r = 0u; r < count; r = r + 1u) {
            let w = max(dot(t, dirs[r]), 0.0);
            sum = sum + rays[r].rgb * w;
            wsum = wsum + w;
        }
        let seen = sum / max(wsum, 1e-6);
        let at = base + 4u + 3u * li;
        let old = vec3<f32>(bitcast<f32>(data[at]), bitcast<f32>(data[at + 1u]), bitcast<f32>(data[at + 2u]));
        var keep = pp.blend.x;
        let lo = dot(old, vec3<f32>(0.2126, 0.7152, 0.0722));
        let ln = dot(seen, vec3<f32>(0.2126, 0.7152, 0.0722));
        if abs(ln - lo) > pp.blend.z * max(lo, 1e-3) {
            keep = pp.blend.y;
        }
        if fresh {
            keep = 0.0;
        }
        let v = mix(seen, old, keep);
        irr[li] = v;
        data[at] = bitcast<u32>(v.r);
        data[at + 1u] = bitcast<u32>(v.g);
        data[at + 2u] = bitcast<u32>(v.b);
    }
    // Depth moments.
    {
        let t = texel_dir(li, 8u);
        var sum = vec2<f32>(0.0);
        var wsum = 0.0;
        for (var r = 0u; r < count; r = r + 1u) {
            let w = pow(max(dot(t, dirs[r]), 0.0), 50.0);
            let back = rays[r].w < 0.0;
            let d = min(select(rays[r].w, -0.2 * rays[r].w, back), reach);
            sum = sum + vec2<f32>(d, d * d) * w;
            wsum = wsum + w;
        }
        let at = base + 4u + 3u * IRR_TEXELS + 2u * li;
        let old = vec2<f32>(bitcast<f32>(data[at]), bitcast<f32>(data[at + 1u]));
        var v = old;
        if wsum > 1e-6 {
            v = mix(sum / wsum, old, select(pp.blend.x, 0.0, fresh));
        } else if fresh {
            v = vec2<f32>(reach, reach * reach);
        }
        depth[li] = v;
        data[at] = bitcast<u32>(v.x);
        data[at + 1u] = bitcast<u32>(v.y);
    }
    workgroupBarrier();
    // The atlas tiles with their borders.
    let tile = gi_probe_tile(cascade, s);
    {
        let x = i32(li % 8u);
        let y = i32(li / 8u);
        let src = border_source(x, y, 6);
        let v = irr[u32(src.x + 6 * src.y)];
        textureStore(atlas, tile * GI_IRR_TILE + vec2<i32>(x, y), vec4<f32>(v, 1.0));
    }
    let row0 = i32(gi_state.probes.atlas.z);
    for (var i = li; i < 100u; i = i + 64u) {
        let x = i32(i % 10u);
        let y = i32(i / 10u);
        let src = border_source(x, y, 8);
        let v = depth[u32(src.x + 8 * src.y)];
        textureStore(atlas, tile * GI_DEPTH_TILE + vec2<i32>(x, y + row0), vec4<f32>(v, 0.0, 1.0));
    }
    if li == 0u {
        data[base] = bitcast<u32>(coord.x);
        data[base + 1u] = bitcast<u32>(coord.y);
        data[base + 2u] = bitcast<u32>(coord.z);
        data[base + 3u] = min(age + 1u, 255u);
        gi_state.probe[index] = spot;
    }
}
