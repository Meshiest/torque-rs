// The GPU particle pool's frame (our own code; composed after gpu_particle_types.wgsl). One compute pass of these dispatches,
// in order:
//   cs_update       each live particle aged by each of the frame's steps in turn and stepped as TGE's `updateSingleParticle` steps it
//                   (`v += (a - v drag - (0, 0, 9.81 gravityCoefficient)) dt; p += (v + wind windCoefficient) dt`, the color and
//                   size between the keys its age reached), or let go past its life; a fresh one aged only; all moved by the
//                   render origin's shift; all let go on a clear;
//   cs_spawn        each of the frame's spawns into a free slot of the last frame's list (in the list's order; past its end
//                   dropped, and counted by its first invocation);
//   cs_fill         each workgroup's sum of the screen area its live particles' sprites cover (clipped to the screen);
//   cs_governor     the frame's total of those, and the scale that brings it to the quality's budget (see `fill`);
//   cs_count        each slot's key (its look's bucket at its age, or the free key) and its rank among the slots of its group with
//                   that key (bit masks: no atomic appends, no order of arrival), and each group's count of each key;
//   cs_scan_blocks  the exclusive prefix sum of the counts (key-major: key k's groups in order), a block of SCAN_BLOCK at a time;
//   cs_scan_top     the prefix sum of the block totals, then each bucket's indirect draw (six vertices a particle, from its first
//                   place in the list) and the free slots' place and count;
//   cs_scatter      each slot at its key's place plus its group's place plus its rank.
// The list holds each bucket's particles in slot order and then the free slots in slot order: the same pool makes the same
// lists and draws byte for byte every run, however the GPU schedules its threads.

struct Sim {
    capacity: u32,
    spawns: u32,
    step_count: u32,
    groups: u32,
    // The keys counted (the buckets and the free slots' key, the last), their counts' length (keys x groups) and where the
    // block totals start in `scan`.
    keys: u32,
    scan_len: u32,
    totals: u32,
    clear: u32,
    // The render origin's move since the last frame (xyz).
    shift: vec4<f32>,
    // The air's velocity (xyz), units a second.
    wind: vec4<f32>,
    // The frame's steps (ms, in order), four a vector.
    steps: array<vec4<u32>, 8>,
    // The main camera's world to clip space (the fill governor projects each sprite with it), its projection's x and y scales and
    // the screen's size in pixels, and the fill budget (x: pixels of sprite area the frame may cover; 0: no budget).
    view_proj: mat4x4<f32>,
    cam: vec4<f32>,
    budget: vec4<f32>,
};

@group(0) @binding(0) var<uniform> sim: Sim;
@group(0) @binding(1) var<storage, read_write> particles: array<PoolParticle>;
@group(0) @binding(2) var<storage, read> spawns: array<PoolParticle>;
@group(0) @binding(3) var<storage, read> looks: array<PoolLook>;
@group(0) @binding(4) var<storage, read_write> keys: array<u32>;
@group(0) @binding(5) var<storage, read_write> scan: array<u32>;
@group(0) @binding(6) var<storage, read_write> instances: array<u32>;
// Four words a bucket (a non-indexed indirect draw), then the free slots' first place in the list and their count, the spawns
// the last pass dropped and those all of them dropped.
@group(0) @binding(7) var<storage, read_write> args: array<u32>;
// The fill governor's result for the sprites' vertex stage: the sprites' size scale, their alpha scale, the frame's summed sprite
// area (pixels) at full size, and the budget.
@group(0) @binding(8) var<storage, read_write> fill: array<f32>;

const GROUP: u32 = 256u;
const MAX_KEYS: u32 = 65u;
const MASK_WORDS: u32 = 8u;
const SCAN_PER_THREAD: u32 = 4u;
const SCAN_BLOCK: u32 = 1024u;
const FREE_AT: u32 = 260u;
const POOL_GRAVITY: f32 = 9.81;
// The least a sprite's alpha is scaled to by the governor.
const FILL_ALPHA_FLOOR: f32 = 0.3;

// One step of `ms` (TGE's `updateSingleParticle`).
fn pool_step(p: ptr<function, PoolParticle>, ms: i32) {
    let li = (*p).look;
    let coef = looks[li].coef;
    let dt = f32(ms) / 1000.0;
    let g = vec3<f32>(0.0, 0.0, POOL_GRAVITY * coef.y);
    let wind = sim.wind.xyz * coef.z;
    let v = (*p).vel + ((*p).acc - (*p).vel * coef.x - g) * dt;
    (*p).vel = v;
    (*p).pos = (*p).pos + (v + wind) * dt;
    var t = 1.0;
    if (*p).life > 0 {
        t = f32((*p).age) / f32((*p).life);
    }
    let times = looks[li].times;
    var k = 1u;
    while k < 4u && times[k] < t {
        k = k + 1u;
    }
    if k >= 4u {
        (*p).color = pack4x8unorm(looks[li].colors[3]);
        (*p).size = looks[li].sizes[3];
        return;
    }
    let span = times[k] - times[k - 1u];
    var f = 1.0;
    if span > 0.0 {
        f = clamp((t - times[k - 1u]) / span, 0.0, 1.0);
    }
    let c = looks[li].colors[k] * f + looks[li].colors[k - 1u] * (1.0 - f);
    (*p).color = pack4x8unorm(c);
    (*p).size = looks[li].sizes[k] * f + looks[li].sizes[k - 1u] * (1.0 - f);
}

@compute @workgroup_size(256)
fn cs_update(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = id.x;
    if i >= sim.capacity {
        return;
    }
    var p = particles[i];
    if (p.flags & POOL_ALIVE) == 0u {
        return;
    }
    if sim.clear != 0u {
        particles[i].flags = 0u;
        return;
    }
    p.pos = p.pos + sim.shift.xyz;
    for (var s = 0u; s < sim.step_count; s = s + 1u) {
        let ms = i32(sim.steps[s / 4u][s % 4u]);
        p.age = p.age + ms;
        if p.age > p.life {
            particles[i].flags = 0u;
            return;
        }
        if (p.flags & POOL_FRESH) != 0u {
            p.flags = p.flags & ~POOL_FRESH;
        } else {
            pool_step(&p, ms);
        }
    }
    particles[i] = p;
}

@compute @workgroup_size(256)
fn cs_spawn(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = id.x;
    let free = args[FREE_AT + 1u];
    if i == 0u {
        let dropped = sim.spawns - min(sim.spawns, free);
        args[FREE_AT + 2u] = dropped;
        args[FREE_AT + 3u] = args[FREE_AT + 3u] + dropped;
    }
    if i >= sim.spawns || i >= free {
        return;
    }
    let slot = instances[args[FREE_AT] + i];
    var p = spawns[i];
    p.flags = p.flags | POOL_ALIVE;
    particles[slot] = p;
}

// The screen area (pixels) `p`'s sprite covers: its square at the size its last step keyed, clipped to the screen; none for a
// particle behind the eye or whose alpha is 0.
fn sprite_area(p: PoolParticle) -> f32 {
    let c = sim.view_proj * vec4<f32>(p.pos, 1.0);
    if !(c.w > 1e-4) || unpack4x8unorm(p.color).a <= 0.0 {
        return 0.0;
    }
    let size = sim.cam.zw;
    let at = vec2<f32>(c.x / c.w * 0.5 + 0.5, 0.5 - c.y / c.w * 0.5) * size;
    let half = 0.25 * p.size * sim.cam.xy * size / c.w;
    let lo = max(at - half, vec2<f32>(0.0));
    let hi = min(at + half, size);
    return max(hi.x - lo.x, 0.0) * max(hi.y - lo.y, 0.0);
}

var<workgroup> fill_sums: array<f32, 256>;

// The sum of `fill_sums` across the workgroup (a fixed tree: the same sum every run): every invocation calls it.
fn sum_fill(li: u32) -> f32 {
    workgroupBarrier();
    for (var s = GROUP / 2u; s > 0u; s = s >> 1u) {
        if li < s {
            fill_sums[li] = fill_sums[li] + fill_sums[li + s];
        }
        workgroupBarrier();
    }
    return fill_sums[0];
}

@compute @workgroup_size(256)
fn cs_fill(@builtin(global_invocation_id) id: vec3<u32>, @builtin(local_invocation_index) li: u32,
           @builtin(workgroup_id) wg: vec3<u32>) {
    var area = 0.0;
    if id.x < sim.capacity {
        let p = particles[id.x];
        if (p.flags & POOL_ALIVE) != 0u {
            area = sprite_area(p);
        }
    }
    fill_sums[li] = area;
    let sum = sum_fill(li);
    if li == 0u {
        // `keys` is the next pass's to fill: its first words hold the groups' sums until then.
        keys[wg.x] = bitcast<u32>(sum);
    }
}

// One workgroup: the groups' sums added (a fixed order), and past the budget the sprites' size scaled by the root of the share the
// budget is of the total (their area goes with its square) and their alpha by its root again (not below FILL_ALPHA_FLOOR).
@compute @workgroup_size(256)
fn cs_governor(@builtin(local_invocation_index) li: u32) {
    var sum = 0.0;
    for (var g = li; g < sim.groups; g = g + GROUP) {
        sum = sum + bitcast<f32>(keys[g]);
    }
    fill_sums[li] = sum;
    let total = sum_fill(li);
    if li == 0u {
        var scale = 1.0;
        var alpha = 1.0;
        if sim.budget.x > 0.0 && total > sim.budget.x {
            scale = sqrt(sim.budget.x / total);
            alpha = max(sqrt(scale), FILL_ALPHA_FLOOR);
        }
        fill[0] = scale;
        fill[1] = alpha;
        fill[2] = total;
        fill[3] = sim.budget.x;
    }
}

var<workgroup> masks: array<atomic<u32>, 520>;

@compute @workgroup_size(256)
fn cs_count(@builtin(local_invocation_index) li: u32, @builtin(workgroup_id) wg: vec3<u32>) {
    for (var k = li; k < MAX_KEYS * MASK_WORDS; k = k + GROUP) {
        atomicStore(&masks[k], 0u);
    }
    workgroupBarrier();
    let i = wg.x * GROUP + li;
    let free_key = sim.keys - 1u;
    let inside = i < sim.capacity;
    var key = free_key;
    if inside {
        let p = particles[i];
        if (p.flags & POOL_ALIVE) != 0u {
            let l = p.look;
            key = min(pool_bucket(looks[l].words, looks[l].coef.w, looks[l].frames[0], looks[l].frames[1], p.age), free_key);
        }
        atomicOr(&masks[key * MASK_WORDS + li / 32u], 1u << (li % 32u));
    }
    workgroupBarrier();
    if inside {
        let w = li / 32u;
        var rank = countOneBits(atomicLoad(&masks[key * MASK_WORDS + w]) & ((1u << (li % 32u)) - 1u));
        for (var k = 0u; k < w; k = k + 1u) {
            rank = rank + countOneBits(atomicLoad(&masks[key * MASK_WORDS + k]));
        }
        keys[i] = key | (rank << 8u);
    }
    if li < sim.keys {
        var n = 0u;
        for (var k = 0u; k < MASK_WORDS; k = k + 1u) {
            n = n + countOneBits(atomicLoad(&masks[li * MASK_WORDS + k]));
        }
        scan[li * sim.groups + wg.x] = n;
    }
}

var<workgroup> sums: array<u32, 256>;

// The inclusive prefix sum of `sums` across the workgroup (Hillis and Steele): every invocation calls it.
fn scan_sums(li: u32) {
    workgroupBarrier();
    for (var off = 1u; off < GROUP; off = off << 1u) {
        var add = 0u;
        if li >= off {
            add = sums[li - off];
        }
        workgroupBarrier();
        sums[li] = sums[li] + add;
        workgroupBarrier();
    }
}

@compute @workgroup_size(256)
fn cs_scan_blocks(@builtin(local_invocation_index) li: u32, @builtin(workgroup_id) wg: vec3<u32>) {
    let base = wg.x * SCAN_BLOCK + li * SCAN_PER_THREAD;
    var own: array<u32, 4>;
    var total = 0u;
    for (var k = 0u; k < SCAN_PER_THREAD; k = k + 1u) {
        let j = base + k;
        var x = 0u;
        if j < sim.scan_len {
            x = scan[j];
        }
        own[k] = total;
        total = total + x;
    }
    sums[li] = total;
    scan_sums(li);
    var before = 0u;
    if li > 0u {
        before = sums[li - 1u];
    }
    for (var k = 0u; k < SCAN_PER_THREAD; k = k + 1u) {
        let j = base + k;
        if j < sim.scan_len {
            scan[j] = own[k] + before;
        }
    }
    if li == GROUP - 1u {
        scan[sim.totals + wg.x] = sums[li];
    }
}

var<workgroup> bases: array<u32, 66>;

@compute @workgroup_size(256)
fn cs_scan_top(@builtin(local_invocation_index) li: u32) {
    let blocks = (sim.scan_len + SCAN_BLOCK - 1u) / SCAN_BLOCK;
    let base = li * SCAN_PER_THREAD;
    var own: array<u32, 4>;
    var total = 0u;
    for (var k = 0u; k < SCAN_PER_THREAD; k = k + 1u) {
        let j = base + k;
        var x = 0u;
        if j < blocks {
            x = scan[sim.totals + j];
        }
        own[k] = total;
        total = total + x;
    }
    sums[li] = total;
    scan_sums(li);
    var before = 0u;
    if li > 0u {
        before = sums[li - 1u];
    }
    for (var k = 0u; k < SCAN_PER_THREAD; k = k + 1u) {
        let j = base + k;
        if j < blocks {
            scan[sim.totals + j] = own[k] + before;
        }
    }
    storageBarrier();
    workgroupBarrier();
    // Each key's first place: its first group's prefix within its block plus the blocks' before it.
    if li < sim.keys {
        let at = li * sim.groups;
        bases[li] = scan[at] + scan[sim.totals + at / SCAN_BLOCK];
    }
    if li == 0u {
        bases[sim.keys] = sim.capacity;
    }
    workgroupBarrier();
    if li < sim.keys {
        let first = bases[li];
        let count = bases[li + 1u] - first;
        if li + 1u == sim.keys {
            args[FREE_AT] = first;
            args[FREE_AT + 1u] = count;
        } else {
            args[li * 4u] = count * 6u;
            args[li * 4u + 1u] = 1u;
            args[li * 4u + 2u] = first * 6u;
            args[li * 4u + 3u] = 0u;
        }
    }
}

@compute @workgroup_size(256)
fn cs_scatter(@builtin(local_invocation_index) li: u32, @builtin(workgroup_id) wg: vec3<u32>) {
    let i = wg.x * GROUP + li;
    if i >= sim.capacity {
        return;
    }
    let k = keys[i];
    let at = (k & 0xffu) * sim.groups + wg.x;
    instances[scan[at] + scan[sim.totals + at / SCAN_BLOCK] + (k >> 8u)] = i;
}
