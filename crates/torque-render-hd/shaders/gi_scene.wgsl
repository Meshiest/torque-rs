// The voxel scene's lookups and its march (after gi_common.wgsl, with `vol`, `tiles`, `occ` and `voxels` bound by the pass).

// Whether `level` holds world voxel `v`.
fn gi_inside(level: u32, v: vec3<i32>) -> bool {
    let o = vol.levels[level].origin.xyz;
    return all(v >= o) && all(v < o + vec3<i32>(GI_N));
}

// The world voxel of `level` the point `p` lies in.
fn gi_voxel_of(level: u32, p: vec3<f32>) -> vec3<i32> {
    return vec3<i32>(floor(p * vol.levels[level].size.y));
}

// Whether world voxel `v` of `level` is filled (a tile that does not hold it now is empty).
fn gi_filled(level: u32, v: vec3<i32>) -> bool {
    let t = v >> vec3<u32>(3u);
    let slot = gi_slot(level, t);
    let info = tiles[slot];
    if info.x != gi_tag(t) || info.y == 0u {
        return false;
    }
    let i = gi_local(v);
    return (occ[slot * GI_TILE_WORDS + (i >> 5u)] & (1u << (i & 31u))) != 0u;
}

// Whether `p` lies behind the surface of the voxel of `level` it is in (a filled voxel): inside a wall, not in front of one whose
// surface lies on the voxel's far side.
fn gi_solid_at(level: u32, p: vec3<f32>) -> bool {
    let v = gi_voxel_of(level, p);
    if !gi_filled(level, v) {
        return false;
    }
    let s = vol.levels[level].size.x;
    let w = voxels[gi_slot(level, v >> vec3<u32>(3u)) * GI_TILE_VOXELS + gi_local(v)];
    let n = gi_unoct(unpack2x16snorm(w.y));
    let q = (vec3<f32>(v) + 0.5) * s + n * (gi_plane(w.x) * s);
    return dot(p - q, n) < 0.0;
}

// Where a march stopped: whether it met a filled voxel, the voxel (level, index of its words), and how far along the ray.
struct GiHit {
    hit: bool,
    level: u32,
    voxel: u32,
    t: f32,
};

// How far along the ray from `o` along `d` (its inverse `inv`) it leaves the box from `lo` to `hi`.
fn gi_exit(o: vec3<f32>, inv: vec3<f32>, d: vec3<f32>, lo: vec3<f32>, hi: vec3<f32>) -> f32 {
    let far = select(lo, hi, d > vec3<f32>(0.0));
    let t = (far - o) * inv;
    let tx = select(t.x, 1e30, d.x == 0.0);
    let ty = select(t.y, 1e30, d.y == 0.0);
    let tz = select(t.z, 1e30, d.z == 0.0);
    return min(tx, min(ty, tz));
}

// The first voxel surface the ray from `o` along the unit `d` meets within `t_max` (see the filled voxel's rule below), marched in the finest level from
// `first_level` up that holds each point (a level the ray leaves hands it to the next; past the last it is a miss): an empty
// tile is crossed in one step, a voxel of a filled one in one.
fn gi_march(o: vec3<f32>, d: vec3<f32>, t_max: f32, first_level: u32) -> GiHit {
    var out: GiHit;
    out.hit = false;
    out.t = t_max;
    let inv = 1.0 / d;
    var level = first_level;
    var t = 0.0;
    let count = vol.count.x;
    for (var step = 0u; step < GI_MAX_STEPS; step = step + 1u) {
        if t >= t_max {
            return out;
        }
        let p = o + d * t;
        var v = gi_voxel_of(level, p);
        loop {
            if level >= count || gi_inside(level, v) {
                break;
            }
            level = level + 1u;
            if level < count {
                v = gi_voxel_of(level, p);
            }
        }
        if level >= count {
            return out;
        }
        let s = vol.levels[level].size.x;
        let tile = v >> vec3<u32>(3u);
        let slot = gi_slot(level, tile);
        let info = tiles[slot];
        var lo: vec3<f32>;
        var side: f32;
        if info.x != gi_tag(tile) || info.y == 0u {
            lo = vec3<f32>(tile * GI_TILE) * s;
            side = s * f32(GI_TILE);
        } else {
            let i = gi_local(v);
            lo = vec3<f32>(v) * s;
            side = s;
            if (occ[slot * GI_TILE_WORDS + (i >> 5u)] & (1u << (i & 31u))) != 0u {
                // A filled voxel stops the ray at its surface where the ray crosses it inside the voxel, else where the ray
                // enters it. A surface through the voxel's inside (not on its side) splits it: a ray that stays in front of it
                // goes on, so a door's jambs and lintel part-way into their voxels do not narrow the opening. A voxel whose
                // surface lies on its side and that a triangle meets from behind is whole: where blocks meet, it holds a hidden
                // face, and a ray let through it would slip through the walls along their seams.
                let at = slot * GI_TILE_VOXELS + i;
                let w = voxels[at];
                let n = gi_unoct(unpack2x16snorm(w.y));
                let along = dot(d, n);
                let exit = max(gi_exit(o, inv, d, lo, lo + vec3<f32>(s)), t);
                let plane = gi_plane(w.x);
                let q = lo + 0.5 * s + n * (plane * s);
                let ahead_in = dot(o + d * t - q, n);
                let ahead_out = dot(o + d * exit - q, n);
                let split = abs(plane) < 0.5 * (abs(n.x) + abs(n.y) + abs(n.z)) - 0.02;
                // In front by a hair: a ray leaving the voxel on a surface on its side meets it there.
                let clear = 0.01 * s;
                if (!split && gi_behind(w.y)) || ahead_in < clear || ahead_out < clear {
                    out.hit = true;
                    out.level = level;
                    out.voxel = at;
                    out.t = t;
                    if along < -1e-4 {
                        out.t = clamp(dot(q - o, n) / along, t, exit);
                    }
                    return out;
                }
            }
        }
        t = max(gi_exit(o, inv, d, lo, lo + vec3<f32>(side)), t) + 1e-3 * s;
    }
    return out;
}
