// The probes' grid (after gi_common.wgsl): each cascade's 24 x 24 x 12 probes about the eye, kept toroidally (a probe's slot is
// its world probe coordinates modulo the grid), their state the forward pass reads, and their places in the atlas: a cascade's
// irradiance tiles in rows of one probe's y, its depth tiles below every cascade's irradiance.

const GI_PROBES_X: i32 = 24;
const GI_PROBES_Y: i32 = 24;
const GI_PROBES_Z: i32 = 12;
const GI_CASCADE_PROBES: u32 = 6912u;
// A probe sits this share of a spacing past its grid point on each axis (between the walls on its cascade's grid, not in
// them).
const GI_PROBE_SHIFT: f32 = 0.5;
// Texels a side of a probe's irradiance (its interior and a border each side) and of its depth moments.
const GI_IRR_INNER: f32 = 6.0;
const GI_IRR_TILE: i32 = 8;
const GI_DEPTH_INNER: f32 = 8.0;
const GI_DEPTH_TILE: i32 = 10;

struct GiCascade {
    // xyz the lowest probe's coordinates (probe units), w unused.
    origin: vec4<i32>,
    // x the spacing, y one over it, zw unused.
    spacing: vec4<f32>,
};

struct GiProbes {
    cascades: array<GiCascade, 4>,
    // x the cascades in use, yzw unused.
    grid: vec4<u32>,
    // x, y one over the atlas's width and height, z the depth moments' first row, w unused.
    atlas: vec4<f32>,
};

struct GiState {
    probes: GiProbes,
    // Each probe's offset from its grid point (xyz) and its weight (w: 0 not yet traced or inside a wall).
    probe: array<vec4<f32>>,
};

// A probe's slot in its cascade for world probe coordinates `c` (toroidal: the coordinates modulo the grid).
fn gi_probe_slot(c: vec3<i32>) -> vec3<i32> {
    let g = vec3<i32>(GI_PROBES_X, GI_PROBES_Y, GI_PROBES_Z);
    return ((c % g) + g) % g;
}

// The probe's tile's column and row in the atlas.
fn gi_probe_tile(cascade: u32, s: vec3<i32>) -> vec2<i32> {
    return vec2<i32>(s.x + GI_PROBES_X * s.z, s.y + GI_PROBES_Y * i32(cascade));
}

fn gi_probe_index(cascade: u32, s: vec3<i32>) -> u32 {
    return cascade * GI_CASCADE_PROBES + u32(s.x + GI_PROBES_X * (s.y + GI_PROBES_Y * s.z));
}

