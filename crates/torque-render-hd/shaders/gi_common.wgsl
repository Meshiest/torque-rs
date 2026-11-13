// The global illumination's voxel scene (our own code): a clipmap of GI_MAX_LEVELS levels of GI_N^3 voxels about the eye, each
// level's voxels twice the size of the level below's, stored toroidally in tiles of GI_TILE^3 voxels (crate::gi). A tile slot
// holds the tile whose world tile coordinates are its slot modulo GI_TILES on each axis; its info words say which tile that is
// (its tag) and whether any of its voxels are filled (0: none), so a reader never takes a stale tile from the other side of the level for
// the one it wants. A voxel is four words: its albedo (rgba8, linear), its normal (octahedral, two snorm16), its emission and
// its lit radiance (rgb9e5 each); its occupancy is one bit of the tile's GI_TILE_WORDS words.

const GI_N: i32 = 128;
const GI_TILE: i32 = 8;
const GI_TILES: i32 = 16;
const GI_TILE_VOXELS: u32 = 512u;
const GI_TILE_WORDS: u32 = 16u;
const GI_LEVEL_TILES: u32 = 4096u;
const GI_MAX_LEVELS: u32 = 4u;
// The most steps of a march (voxels and empty tiles crossed).
const GI_MAX_STEPS: u32 = 192u;

struct GiLevel {
    // xyz: the level's low corner in its voxels (a multiple of GI_TILE); w unused.
    origin: vec4<i32>,
    // x: the voxel's size, y: one over it; zw unused.
    size: vec4<f32>,
};

struct GiVolume {
    levels: array<GiLevel, 4>,
    // x: the levels in use, y: the frame number, z: the voxel pass's jobs; w unused.
    count: vec4<u32>,
};

// The tag of the world tile `t` (its coordinates modulo 1024 and a set high bit: an empty slot's zero tag matches no tile).
fn gi_tag(t: vec3<i32>) -> u32 {
    let u = vec3<u32>(t) & vec3<u32>(1023u);
    return u.x | (u.y << 10u) | (u.z << 20u) | 0x80000000u;
}

// The slot of world tile `t` of `level` (its index among every level's tiles).
fn gi_slot(level: u32, t: vec3<i32>) -> u32 {
    let s = ((t % GI_TILES) + GI_TILES) % GI_TILES;
    return level * GI_LEVEL_TILES + u32(s.x + GI_TILES * (s.y + GI_TILES * s.z));
}

// A voxel's index within its tile.
fn gi_local(v: vec3<i32>) -> u32 {
    let l = v & vec3<i32>(GI_TILE - 1);
    return u32(l.x + GI_TILE * (l.y + GI_TILE * l.z));
}

// rgb9e5: three 9-bit mantissas over a shared exponent (bias 15), as the texture format packs them.
fn gi_pack_rgb9e5(c_in: vec3<f32>) -> u32 {
    let c = clamp(c_in, vec3<f32>(0.0), vec3<f32>(65408.0));
    let m = max(c.x, max(c.y, c.z));
    if m < 1.0 / 16777216.0 {
        return 0u;
    }
    var e = max(-16, i32(floor(log2(m)))) + 16;
    var denom = exp2(f32(e - 24));
    let top = floor(m / denom + 0.5);
    if top >= 512.0 {
        e = e + 1;
        denom = denom * 2.0;
    }
    let q = vec3<u32>(min(floor(c / denom + 0.5), vec3<f32>(511.0)));
    return q.x | (q.y << 9u) | (q.z << 18u) | (u32(e) << 27u);
}

fn gi_unpack_rgb9e5(v: u32) -> vec3<f32> {
    let e = i32(v >> 27u);
    let scale = exp2(f32(e - 24));
    return vec3<f32>(f32(v & 511u), f32((v >> 9u) & 511u), f32((v >> 18u) & 511u)) * scale;
}

// Whether a triangle meets the voxel with the voxel's middle well behind it (the normal word's lowest bit): the voxel may lie in
// a solid where blocks meet.
fn gi_behind(normal_word: u32) -> bool {
    return (normal_word & 1u) != 0u;
}

// How far a voxel's surface lies from its middle along its normal, in voxels (-1 to 1: the albedo word's top byte). A surface on
// a voxel boundary fills the voxels on both sides; both shade and stop rays at the surface itself.
fn gi_plane(albedo_word: u32) -> f32 {
    return (f32(albedo_word >> 24u) - 127.0) / 127.0;
}

// An albedo word (rgba8 of linear light; its top byte the surface's place, `gi_plane`) as a color.
fn gi_unpack_albedo(v: u32) -> vec3<f32> {
    return unpack4x8unorm(v).rgb;
}

// Octahedral normals in two snorm16 halves.
fn gi_oct(n: vec3<f32>) -> vec2<f32> {
    let l1 = abs(n.x) + abs(n.y) + abs(n.z);
    var e = n.xy / max(l1, 1e-20);
    if n.z < 0.0 {
        let s = select(vec2<f32>(-1.0), vec2<f32>(1.0), e >= vec2<f32>(0.0));
        e = (vec2<f32>(1.0) - abs(e.yx)) * s;
    }
    return e;
}

fn gi_unoct(e: vec2<f32>) -> vec3<f32> {
    var n = vec3<f32>(e.x, e.y, 1.0 - abs(e.x) - abs(e.y));
    if n.z < 0.0 {
        let s = select(vec2<f32>(-1.0), vec2<f32>(1.0), n.xy >= vec2<f32>(0.0));
        n = vec3<f32>((vec2<f32>(1.0) - abs(n.yx)) * s, n.z);
    }
    return normalize(n);
}
