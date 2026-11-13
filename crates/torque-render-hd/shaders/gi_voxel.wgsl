// The voxel scene's tiles made from the static triangles (after gi_common.wgsl): four workgroups a tile of the frame's jobs (two
// layers of its voxels each), each thread a column of its layers. The job's triangles are read 64 at a time into the workgroup's memory and every thread
// tests its column's voxels against each one whose box meets the column (a triangle-box test on the separating axes:
// Akenine-Moller 2001, with the box a hair larger so no voxel the CPU's exact test fills is missed); each voxel keeps the
// triangle whose plane passes nearest its middle among those it lies on or in front of (ties: the first in the job's list), so
// the result does not depend on the order anything ran in, and marks a voxel that a triangle meets from well behind. Then the tile's voxels, occupancy words and info are written whole (an empty voxel cleared).

struct Tri {
    // xyz a corner; a.w the albedo word, b.w the emission (rgb9e5), c.w unused.
    a: vec4<f32>,
    b: vec4<f32>,
    c: vec4<f32>,
};

struct Job {
    // xyz the world tile, w its level.
    tile: vec4<i32>,
    // x the first of its triangles in the list, y how many.
    span: vec4<u32>,
};

@group(0) @binding(0) var<uniform> vol: GiVolume;
@group(0) @binding(1) var<storage, read> tris: array<Tri>;
@group(0) @binding(2) var<storage, read> jobs: array<Job>;
// Each job's triangles (their places in `tris`, ascending).
@group(0) @binding(3) var<storage, read> list: array<u32>;
// The tile infos as words: a slot's tag, then a bit for each quarter of its layers that holds a filled voxel (what the other
// passes read as its info's second word: 0 empty).
@group(0) @binding(4) var<storage, read_write> tiles: array<atomic<u32>>;
@group(0) @binding(5) var<storage, read_write> occ: array<u32>;
@group(0) @binding(6) var<storage, read_write> voxels: array<vec4<u32>>;

var<workgroup> cache: array<Tri, 64>;
// Each voxel's best key so far (a voxel is one thread's: no atomics).
var<workgroup> best: array<u32, 512>;
var<workgroup> words: array<atomic<u32>, 16>;
var<workgroup> filled: atomic<u32>;

// A key's low bits: the triangle's place in the job's list (subtracted from the mask: the first wins a tie).
const ORDINAL_BITS: u32 = 23u;
const ORDINAL_MASK: u32 = 0x7fffffu;
// How much larger than the voxel the box tested is.
const GROW: f32 = 1.0009765625;

// Whether the triangle (corners less the box's middle) lies wholly on one side of the box (half sides `h`) along `axis`.
fn separated(axis: vec3<f32>, v0: vec3<f32>, v1: vec3<f32>, v2: vec3<f32>, h: vec3<f32>) -> bool {
    let p0 = dot(v0, axis);
    let p1 = dot(v1, axis);
    let p2 = dot(v2, axis);
    let r = dot(h, abs(axis)) * GROW;
    return min(p0, min(p1, p2)) > r || max(p0, max(p1, p2)) < -r;
}

// Whether one edge's three cross axes separate the triangle from the box.
fn edge_separates(e: vec3<f32>, v0: vec3<f32>, v1: vec3<f32>, v2: vec3<f32>, h: vec3<f32>) -> bool {
    return separated(vec3<f32>(0.0, -e.z, e.y), v0, v1, v2, h)
        || separated(vec3<f32>(e.z, 0.0, -e.x), v0, v1, v2, h)
        || separated(vec3<f32>(-e.y, e.x, 0.0), v0, v1, v2, h);
}

// Whether the triangle a, b, c overlaps the box about `center` of half sides `h`.
fn tri_box(center: vec3<f32>, h: vec3<f32>, a: vec3<f32>, b: vec3<f32>, c: vec3<f32>) -> bool {
    let v0 = a - center;
    let v1 = b - center;
    let v2 = c - center;
    let e0 = v1 - v0;
    let e1 = v2 - v1;
    let e2 = v0 - v2;
    if edge_separates(e0, v0, v1, v2, h) || edge_separates(e1, v0, v1, v2, h) || edge_separates(e2, v0, v1, v2, h) {
        return false;
    }
    if separated(vec3<f32>(1.0, 0.0, 0.0), v0, v1, v2, h)
        || separated(vec3<f32>(0.0, 1.0, 0.0), v0, v1, v2, h)
        || separated(vec3<f32>(0.0, 0.0, 1.0), v0, v1, v2, h) {
        return false;
    }
    return !separated(cross(e0, e1), v0, v1, v2, h);
}

@compute @workgroup_size(64)
fn cs_voxelize(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let index = (wg.x + 65535u * wg.y) / 4u;
    let part = (wg.x + 65535u * wg.y) % 4u;
    if index >= vol.count.z {
        return;
    }
    let job = jobs[index];
    if li < GI_TILE_WORDS {
        atomicStore(&words[li], 0u);
    }
    if li == 0u {
        atomicStore(&filled, 0u);
    }
    let total = job.span.y;
    let level = u32(job.tile.w);
    let s = vol.levels[level].size.x;
    let lo = vec3<f32>(job.tile.xyz * GI_TILE) * s;
    // This workgroup's layers' heights.
    let z_lo = lo.z + f32(2u * part) * s;
    let z_hi = z_lo + 2.0 * s;
    let h = vec3<f32>(0.5 * s);
    // This thread's column: its x and y in the tile, the part's two layers.
    let col = vec2<i32>(i32(li & 7u), i32(li >> 3u));
    let mid = lo.xy + (vec2<f32>(col) + 0.5) * s;
    let reach = 0.5 * s * GROW;
    for (var z = 2u * part; z < 2u * part + 2u; z = z + 1u) {
        best[li + 64u * z] = 0u;
    }
    // Each of the thread's two voxels: whether a triangle meets it with its middle well behind the triangle (`gi_behind`).
    var behind = 0u;
    for (var first = 0u; first < total; first = first + 64u) {
        workgroupBarrier();
        if first + li < total {
            cache[li] = tris[list[job.span.x + first + li]];
        }
        workgroupBarrier();
        let n = min(64u, total - first);
        for (var k = 0u; k < n; k = k + 1u) {
            let t = cache[k];
            let a = t.a.xyz;
            let b = t.b.xyz;
            let c = t.c.xyz;
            let tmin = min(a, min(b, c));
            let tmax = max(a, max(b, c));
            if any(tmax.xy < mid - reach) || any(tmin.xy > mid + reach) || tmax.z < z_lo - reach || tmin.z > z_hi + reach {
                continue;
            }
            let z0 = clamp(i32(floor((tmin.z - lo.z) / s - 0.01)), i32(2u * part), i32(2u * part + 1u));
            let z1 = clamp(i32(floor((tmax.z - lo.z) / s + 0.01)), i32(2u * part), i32(2u * part + 1u));
            let nv = cross(b - a, c - a);
            let nl = max(length(nv), 1e-20);
            for (var z = z0; z <= z1; z = z + 1) {
                let center = vec3<f32>(mid, lo.z + (f32(z) + 0.5) * s);
                if !tri_box(center, h, a, b, c) {
                    continue;
                }
                // A face with the voxel's middle on or in front of its plane first (where faces meet at a voxel's edge, as a
                // floor meets a wall, the hidden faces of the blocks behind them pass as near: rays from the open would meet
                // their backs), then nearer the plane, higher: 1 (half a diagonal away) to 255 (through the middle).
                let ahead = dot(nv, center - a);
                let front = select(0u, 1u, ahead >= -1e-3 * nl * s);
                let away = abs(ahead) / (nl * 0.8660254 * s);
                let near = 1u + u32(clamp(1.0 - away, 0.0, 1.0) * 254.0);
                let key = (front << 31u) | (near << ORDINAL_BITS) | (ORDINAL_MASK - min(first + k, ORDINAL_MASK));
                if ahead < -0.25 * nl * s {
                    behind = behind | (1u << (u32(z) - 2u * part));
                }
                let at = li + 64u * u32(z);
                best[at] = max(best[at], key);
            }
        }
    }
    let slot = gi_slot(level, job.tile.xyz);
    for (var z = 2u * part; z < 2u * part + 2u; z = z + 1u) {
        let i = li + 64u * z;
        let at = slot * GI_TILE_VOXELS + i;
        let key = best[i];
        if key == 0u {
            voxels[at] = vec4<u32>(0u);
            continue;
        }
        let t = tris[list[job.span.x + ORDINAL_MASK - (key & ORDINAL_MASK)]];
        let n = normalize(cross(t.b.xyz - t.a.xyz, t.c.xyz - t.a.xyz));
        let emission = bitcast<u32>(t.b.w);
        let center = lo + (vec3<f32>(f32(i & 7u), f32((i >> 3u) & 7u), f32(i >> 6u)) + 0.5) * s;
        let plane = u32(round(clamp(dot(n, t.a.xyz - center) / s, -1.0, 1.0) * 127.0 + 127.0));
        let albedo = (bitcast<u32>(t.a.w) & 0xffffffu) | (plane << 24u);
        let mark = (behind >> (z - 2u * part)) & 1u;
        voxels[at] = vec4<u32>(albedo, (pack2x16snorm(gi_oct(n)) & ~1u) | mark, emission, emission);
        atomicOr(&words[i >> 5u], 1u << (i & 31u));
        atomicAdd(&filled, 1u);
    }
    workgroupBarrier();
    // The part's four occupancy words, and its bit of the info (the four parts write the same tag).
    if li < 4u {
        let w = 4u * part + li;
        occ[slot * GI_TILE_WORDS + w] = atomicLoad(&words[w]);
    }
    if li == 0u {
        atomicStore(&tiles[2u * slot], gi_tag(job.tile.xyz));
        let bit = 1u << part;
        atomicAnd(&tiles[2u * slot + 1u], ~bit);
        if atomicLoad(&filled) != 0u {
            atomicOr(&tiles[2u * slot + 1u], bit);
        }
    }
}
