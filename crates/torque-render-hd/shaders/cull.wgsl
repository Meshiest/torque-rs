// The static cells' clusters' draw lists, made on the GPU (our own code). The cluster table is kept in segments, each holding the clusters
// of one draw group (a texture slot and translucency; a group has one or more segments); each slot of the table holds one cluster
// or nothing. A cull writes one pass flag a slot; the lists are then compacted with no atomic appends, so the same flags make the
// same lists byte for byte every run:
//   cs_cells        each cell against the view (its planes and sphere) and whether it lies within the view's near distance;
//   cs_clusters     each slot's pass flag: a live cluster of a kind the view draws, its cell seen, its own box seen; for the
//                   main view in two phases (Haar and Aaltonen 2015, "GPU-Driven Rendering Pipelines"): phase 1 the opaque
//                   clusters seen last frame; phase 2, after their depth and its farthest-depth pyramid, those not drawn in phase 1
//                   and not wholly behind it (`flags`), the frame's lists (`flags2`: every cluster not wholly behind it, phase 1's
//                   and translucent ones too: one of phase 1's the depth hides has no fragment the forward pass keeps) and the
//                   next frame's seen bits;
//   cs_scan_blocks  each block of SCAN_BLOCK slots: the exclusive prefix sum of its flags, and the block's total;
//   cs_scan_top     the exclusive prefix sum of the block totals (at most SCAN_BLOCK blocks: 2^20 slots);
//   cs_scatter      each passing slot's indexed indirect draw at its place in its segment of the list, in slot order;
//   cs_counts       each segment's draw count (its passing slots).
// A view with VIEW_WHOLE (a depth-only shadow map's, whose depth does not depend on its draws' order) gets one list an arena page
// instead: cs_counts writes each segment's count at counts[MAX_SEGMENTS + segment], cs_page_offsets each segment's place in its
// page's list (its page's earlier segments' counts, in segment order) at counts[2 MAX_SEGMENTS + segment] and each page's count
// at counts[page], then cs_scatter puts each passing slot at its page's list's start (segments[MAX_SEGMENTS + segment].y) plus its
// segment's place plus its place in its segment. Still no atomic appends: the same flags make the same lists.
// A slot's prefix over the whole table is blocks[slot / SCAN_BLOCK] + prefix[slot].

const SCAN_THREADS: u32 = 256u;
const SCAN_PER_THREAD: u32 = 4u;
const SCAN_BLOCK: u32 = 1024u;
// A cluster record's flags.
const CLUSTER_LIVE: u32 = 1u;
const CLUSTER_TRANSLUCENT: u32 = 2u;
const CLUSTER_NEAR: u32 = 4u;
// What a view draws (View.counts.y).
const VIEW_OPAQUE: u32 = 1u;
const VIEW_TRANSLUCENT: u32 = 2u;
const VIEW_NEAR: u32 = 4u;
const VIEW_WHOLE: u32 = 8u;
const MAX_SEGMENTS: u32 = 4096u;
const MAX_PAGES: u32 = 64u;
const MAX_PLANES: u32 = 8u;

struct Cluster {
    lo: vec3<f32>,
    cell: u32,
    hi: vec3<f32>,
    segment: u32,
    first_index: u32,
    index_count: u32,
    base_vertex: i32,
    flags: u32,
}

struct ScanParams {
    // The table's slots and segments.
    slots: u32,
    segments: u32,
    pad0: u32,
    pad1: u32,
}

// A unit's box (the cull's), live or not: a cell (level 0) or a level of detail's block, its grid key at its level, and whether
// the coarser levels' blocks holding it have meshes (lod: 1 its L1 block's, 2 its L2 block's).
struct CellBox {
    lo: vec3<f32>,
    lod: u32,
    hi: vec3<f32>,
    live: u32,
    key: vec3<i32>,
    level: u32,
}

// A view the lists are made for: its planes (a box wholly on the negative side of one is not seen), an optional sphere (radius
// above 0: a box beyond it is not seen), the eye and the near distance (w: the near-only clusters are drawn within it; below 0
// nowhere), the planes in use, what it draws, the cells.
// counts: x planes, y kinds, z cells, w the phase (0 a plain view, 1 and 2 the main view's); the world-to-clip matrix and the
// farthest-depth pyramid's frame size and levels (phase 2's occlusion test; its first level is the frame padded with the far plane
// to a multiple of its coarsest texel, so a pixel's texel at every level is its place halved).
struct View {
    planes: array<vec4<f32>, 8>,
    sphere: vec4<f32>,
    eye: vec4<f32>,
    counts: vec4<u32>,
    view_proj: mat4x4<f32>,
    hiz: vec4<u32>,
    // The levels of detail: the distances past which L1 (x) and L2 (y) are drawn, z 1 where they are (else L0 alone).
    lod: vec4<f32>,
}

@group(0) @binding(0) var<storage, read> clusters: array<Cluster>;
@group(0) @binding(1) var<storage, read_write> flags: array<u32>;
@group(0) @binding(2) var<storage, read_write> prefix: array<u32>;
@group(0) @binding(3) var<storage, read_write> blocks: array<u32>;
// Each segment of the table: its first slot and its slots; at MAX_SEGMENTS + the segment, its arena page and the first slot of
// its page's list (VIEW_WHOLE).
@group(0) @binding(4) var<storage, read> segments: array<vec2<u32>>;
// The list: five words a draw (index count, instance count, first index, base vertex, first instance) at the slots of the
// draw's segment, and a count a segment.
@group(0) @binding(5) var<storage, read_write> args: array<u32>;
@group(0) @binding(6) var<storage, read_write> counts: array<u32>;
@group(0) @binding(7) var<uniform> scan: ScanParams;
@group(0) @binding(8) var<storage, read> cells: array<CellBox>;
// A cell's view flags: 1 seen, 2 within the near distance.
@group(0) @binding(9) var<storage, read_write> cellvis: array<u32>;
@group(0) @binding(10) var<uniform> view: View;
// The report's sums (the cells seen, the draws listed): atomic adds, read only by the reports (never a place in a list).
@group(0) @binding(11) var<storage, read_write> report: array<atomic<u32>, 2>;
// The main view's phase-1 flags (read in phase 2), its seen bits (written in phase 2, read in the next frame's phase 1), phase 2's
// second flags, the farthest-depth pyramid of phase 1's depth.
@group(0) @binding(12) var<storage, read> prev: array<u32>;
@group(0) @binding(13) var<storage, read_write> vis: array<u32>;
@group(0) @binding(14) var<storage, read_write> flags2: array<u32>;
@group(0) @binding(15) var hiz: texture_2d<f32>;

var<workgroup> partial: array<u32, SCAN_THREADS>;

fn flag_at(i: u32) -> u32 {
    if i >= scan.slots {
        return 0u;
    }
    return select(0u, 1u, flags[i] != 0u);
}

// The exclusive prefix sum of `v` over the workgroup's threads (Hillis and Steele's doubling, in workgroup memory); every
// thread gets its own and the workgroup's total in `.y`.
fn workgroup_exclusive(t: u32, v: u32) -> vec2<u32> {
    partial[t] = v;
    workgroupBarrier();
    var step = 1u;
    while step < SCAN_THREADS {
        var add = 0u;
        if t >= step {
            add = partial[t - step];
        }
        workgroupBarrier();
        partial[t] = partial[t] + add;
        workgroupBarrier();
        step = step * 2u;
    }
    let inclusive = partial[t];
    let total = partial[SCAN_THREADS - 1u];
    return vec2<u32>(inclusive - v, total);
}

@compute @workgroup_size(256)
fn cs_scan_blocks(@builtin(local_invocation_id) lid: vec3<u32>, @builtin(workgroup_id) wid: vec3<u32>) {
    let t = lid.x;
    let first = wid.x * SCAN_BLOCK + t * SCAN_PER_THREAD;
    var own: array<u32, 4>;
    var sum = 0u;
    for (var k = 0u; k < SCAN_PER_THREAD; k = k + 1u) {
        own[k] = flag_at(first + k);
        sum = sum + own[k];
    }
    let s = workgroup_exclusive(t, sum);
    var run = s.x;
    for (var k = 0u; k < SCAN_PER_THREAD; k = k + 1u) {
        if first + k < scan.slots {
            prefix[first + k] = run;
        }
        run = run + own[k];
    }
    if t == 0u {
        blocks[wid.x] = s.y;
    }
}

@compute @workgroup_size(256)
fn cs_scan_top(@builtin(local_invocation_id) lid: vec3<u32>) {
    let t = lid.x;
    let n = (scan.slots + SCAN_BLOCK - 1u) / SCAN_BLOCK;
    let first = t * SCAN_PER_THREAD;
    var own: array<u32, 4>;
    var sum = 0u;
    for (var k = 0u; k < SCAN_PER_THREAD; k = k + 1u) {
        own[k] = 0u;
        if first + k < n {
            own[k] = blocks[first + k];
        }
        sum = sum + own[k];
    }
    let s = workgroup_exclusive(t, sum);
    var run = s.x;
    for (var k = 0u; k < SCAN_PER_THREAD; k = k + 1u) {
        if first + k < n {
            blocks[first + k] = run;
        }
        run = run + own[k];
    }
}

// Whether a box may be seen: not wholly behind a plane (with a margin of the planes' rounding), not beyond the sphere.
fn box_seen(lo: vec3<f32>, hi: vec3<f32>) -> bool {
    let c = 0.5 * (lo + hi);
    let e = 0.5 * (hi - lo);
    let n = min(view.counts.x, MAX_PLANES);
    for (var i = 0u; i < n; i = i + 1u) {
        let p = view.planes[i];
        let s = dot(p.xyz, c) + p.w;
        let r = dot(abs(p.xyz), e);
        let margin = 1e-5 * (abs(dot(p.xyz, c)) + abs(p.w) + r);
        if s + r < -margin {
            return false;
        }
    }
    let radius = view.sphere.w;
    if radius > 0.0 {
        let d = max(max(lo - view.sphere.xyz, view.sphere.xyz - hi), vec3<f32>(0.0));
        if dot(d, d) > radius * radius * (1.0 + 1e-5) {
            return false;
        }
    }
    return true;
}

// A cell's side (hd::cells::CELL_SIZE).
const CELL_SIDE: f32 = 32.0;

// The distance from the eye to the grid box of block `key`, `side` units a side.
fn block_distance(key: vec3<f32>, side: f32) -> f32 {
    let lo = key * side;
    let d = max(max(lo - view.eye.xyz, view.eye.xyz - (lo + side)), vec3<f32>(0.0));
    return sqrt(dot(d, d));
}

// Whether a block `d` from the eye is drawn coarse at a switch `at` units away, `stay` when it was at the last choice (cull.rs's
// lod_coarse and LOD_HYSTERESIS: back to the finer level only once the eye is that share nearer).
fn lod_coarse(d: f32, at: f32, stay: bool) -> bool {
    return d >= select(at, at * (1.0 - 0.15), stay);
}

// Whether a unit's level is the one drawn where it stands (cull.rs's lod_selected, the same arithmetic): the level the distance
// asks for, made finer while that level's block has no mesh; with the levels off, L0.
fn lod_selected(c: CellBox) -> bool {
    if view.lod.z < 0.5 {
        return c.level == 0u;
    }
    let key = vec3<f32>(c.key);
    var b1 = floor(key / 2.0);
    var b2 = floor(key / 4.0);
    if c.level == 1u {
        b1 = key;
        b2 = floor(key / 2.0);
    } else if c.level >= 2u {
        b1 = key;
        b2 = key;
    }
    let d2 = block_distance(b2, 4.0 * CELL_SIDE);
    var d1 = d2;
    if c.level < 2u {
        d1 = block_distance(b1, 2.0 * CELL_SIDE);
    }
    var has1 = (c.lod & 1u) != 0u;
    var has2 = (c.lod & 2u) != 0u;
    if c.level == 1u {
        has1 = true;
    } else if c.level >= 2u {
        has1 = true;
        has2 = true;
    }
    var chosen = 0u;
    if lod_coarse(d2, view.lod.y, (c.lod & 8u) != 0u) {
        chosen = 2u;
    } else if lod_coarse(d1, view.lod.x, (c.lod & 4u) != 0u) {
        chosen = 1u;
    }
    if chosen == 2u && !has2 {
        chosen = 1u;
    }
    if chosen == 1u && !has1 {
        chosen = 0u;
    }
    return chosen == c.level;
}

@compute @workgroup_size(64)
fn cs_cells(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = id.x;
    if i >= view.counts.z {
        return;
    }
    let c = cells[i];
    var v = 0u;
    if c.live != 0u && box_seen(c.lo, c.hi) {
        // 4: its opaque clusters drawn (its level the one drawn here).
        v = select(1u, 5u, lod_selected(c));
        let far = view.eye.w;
        let d = max(max(c.lo - view.eye.xyz, view.eye.xyz - c.hi), vec3<f32>(0.0));
        if far >= 0.0 && dot(d, d) <= far * far {
            v = v | 2u;
        }
    }
    cellvis[i] = v;
    if v != 0u {
        atomicAdd(&report[0], 1u);
    }
}

// Whether every point of a box lies behind the farthest depth drawn at each texel its rectangle covers (dilated by a texel):
// then none of its fragments passes the depth test. A box reaching the eye's plane, or with no finite corner, never is.
fn occluded(lo: vec3<f32>, hi: vec3<f32>) -> bool {
    let size = vec2<f32>(f32(view.hiz.x), f32(view.hiz.y));
    var rmin = vec2<f32>(1e30);
    var rmax = vec2<f32>(-1e30);
    var znear = 0.0;
    for (var k = 0u; k < 8u; k = k + 1u) {
        let p = vec3<f32>(
            select(lo.x, hi.x, (k & 1u) != 0u),
            select(lo.y, hi.y, (k & 2u) != 0u),
            select(lo.z, hi.z, (k & 4u) != 0u),
        );
        let c = view.view_proj * vec4<f32>(p, 1.0);
        if !(c.w > 1e-4) {
            return false;
        }
        let n = c.xyz / c.w;
        let s = vec2<f32>((n.x * 0.5 + 0.5) * size.x, (0.5 - n.y * 0.5) * size.y);
        rmin = min(rmin, s);
        rmax = max(rmax, s);
        znear = max(znear, n.z);
    }
    if !(znear >= 0.0 && znear <= 1.0) || any(rmin != rmin) || any(rmax != rmax) {
        return false;
    }
    let top = vec2<i32>(i32(view.hiz.x) - 1, i32(view.hiz.y) - 1);
    let a = clamp(vec2<i32>(floor(rmin)) - 1, vec2<i32>(0), top);
    let b = clamp(vec2<i32>(floor(rmax)) + 1, vec2<i32>(0), top);
    // The finest level where the rectangle covers at most 4 x 4 texels (at 2 x 2, a box was judged by texels up to twice as
    // wide, holding more of what lies around it).
    var level = 0u;
    let last = max(view.hiz.z, 1u) - 1u;
    loop {
        let w = (b >> vec2<u32>(level)) - (a >> vec2<u32>(level));
        if (w.x <= 3 && w.y <= 3) || level >= last {
            break;
        }
        level = level + 1u;
    }
    let la = a >> vec2<u32>(level);
    let lb = b >> vec2<u32>(level);
    // Wider than the coarsest level's 4 texels (128 pixels a texel): up to 16 of them each way are read.
    if any(lb - la > vec2<i32>(15)) {
        return false;
    }
    let dims = vec2<i32>(textureDimensions(hiz, level)) - 1;
    var far = 1.0;
    for (var y = la.y; y <= lb.y; y = y + 1) {
        for (var x = la.x; x <= lb.x; x = x + 1) {
            far = min(far, textureLoad(hiz, min(vec2<i32>(x, y), dims), i32(level)).r);
        }
    }
    return znear * (1.0 + 1e-5) + 1e-7 < far;
}

@compute @workgroup_size(256)
fn cs_clusters(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = id.x;
    if i >= scan.slots {
        return;
    }
    let r = clusters[i];
    var keep = (r.flags & CLUSTER_LIVE) != 0u && r.cell < view.counts.z;
    let kinds = view.counts.y;
    let translucent = (r.flags & CLUSTER_TRANSLUCENT) != 0u;
    if keep {
        keep = select((kinds & VIEW_OPAQUE) != 0u, (kinds & VIEW_TRANSLUCENT) != 0u, translucent);
    }
    if keep {
        let cv = cellvis[r.cell];
        keep = (cv & 1u) != 0u && (translucent || (cv & 4u) != 0u);
        if (r.flags & CLUSTER_NEAR) != 0u {
            keep = keep && (kinds & VIEW_NEAR) != 0u && (cv & 2u) != 0u;
        }
    }
    keep = keep && box_seen(r.lo, r.hi);
    let phase = view.counts.w;
    if phase == 1u {
        flags[i] = select(0u, 1u, keep && !translucent && vis[i] != 0u);
    } else if phase == 2u {
        let hidden = keep && occluded(r.lo, r.hi);
        let first = prev[i] != 0u;
        flags[i] = select(0u, 1u, keep && !translucent && !first && !hidden);
        flags2[i] = select(0u, 1u, keep && !hidden);
        vis[i] = select(0u, 1u, keep && !translucent && !hidden);
    } else {
        flags[i] = select(0u, 1u, keep);
    }
}

fn prefix_at(i: u32) -> u32 {
    return blocks[i / SCAN_BLOCK] + prefix[i];
}

@compute @workgroup_size(256)
fn cs_scatter(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = id.x;
    if i >= scan.slots || flag_at(i) == 0u {
        return;
    }
    let c = clusters[i];
    if c.segment >= scan.segments {
        return;
    }
    let seg = segments[c.segment];
    if i < seg.x || i >= seg.x + seg.y {
        return;
    }
    var at = (seg.x + prefix_at(i) - prefix_at(seg.x)) * 5u;
    if (view.counts.y & VIEW_WHOLE) != 0u {
        let page = segments[MAX_SEGMENTS + c.segment];
        at = (page.y + counts[2u * MAX_SEGMENTS + c.segment] + prefix_at(i) - prefix_at(seg.x)) * 5u;
    }
    args[at] = c.index_count;
    args[at + 1u] = 1u;
    args[at + 2u] = c.first_index;
    args[at + 3u] = bitcast<u32>(c.base_vertex);
    // The draw's identity: the instance-rate cluster index buffer holds 0, 1, 2, ... (instance_index is never read).
    args[at + 4u] = i;
}

@compute @workgroup_size(64)
fn cs_counts(@builtin(global_invocation_id) id: vec3<u32>) {
    let g = id.x;
    if g >= scan.segments {
        return;
    }
    // A page's list's segment counts are kept apart from the page counts the draws read.
    let whole = (view.counts.y & VIEW_WHOLE) != 0u;
    let at = select(g, MAX_SEGMENTS + g, whole);
    let seg = segments[g];
    if seg.y == 0u || seg.x + seg.y > scan.slots {
        counts[at] = 0u;
        return;
    }
    let last = seg.x + seg.y - 1u;
    let n = prefix_at(last) + flag_at(last) - prefix_at(seg.x);
    counts[at] = n;
    atomicAdd(&report[1], n);
}

// VIEW_WHOLE: each segment's place in its page's list and each page's count (one thread: the segments in order).
@compute @workgroup_size(1)
fn cs_page_offsets() {
    var run: array<u32, MAX_PAGES>;
    for (var p = 0u; p < MAX_PAGES; p = p + 1u) {
        run[p] = 0u;
    }
    for (var s = 0u; s < scan.segments; s = s + 1u) {
        let p = min(segments[MAX_SEGMENTS + s].x, MAX_PAGES - 1u);
        counts[2u * MAX_SEGMENTS + s] = run[p];
        run[p] = run[p] + counts[MAX_SEGMENTS + s];
    }
    for (var p = 0u; p < MAX_PAGES; p = p + 1u) {
        counts[p] = run[p];
    }
}
