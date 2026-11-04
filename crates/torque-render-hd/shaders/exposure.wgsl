// The automatic exposure: a 256-bin histogram of the frame's log2 luminance (each workgroup counts its 16 x 16
// pixels in shared memory, then adds them into the frame's bins: integer atomics, so the counts are the same however the GPU orders
// them), then one workgroup's pass that takes the mean log2 luminance of the bins between the low and the high percentile, the exposure
// that puts it at the key (clamped), and adapts the frame's exposure toward it over the frame's time (a history's first frame takes
// it at once). hd::grade holds the CPU form (the gate's reference).

const BINS: u32 = 256u;

struct Params {
    // x: the histogram's lowest log2 luminance, y: its highest, z: the low percentile, w: the high one.
    range: vec4<f32>,
    // x: log2 of the key, y: the clamp (EV), z: the adaptation's time constant (s), w: the frame's time (s).
    adapt: vec4<f32>,
    // x: 1 when the history starts again.
    flags: vec4<f32>,
};

// x: the exposure (EV), y: the target, z: 1 once set, w: the mean log2 luminance.
struct State {
    v: vec4<f32>,
};

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var source: texture_2d<f32>;
@group(0) @binding(2) var<storage, read_write> bins: array<atomic<u32>, 256>;
@group(0) @binding(3) var<storage, read_write> state: State;

var<workgroup> local_bins: array<atomic<u32>, 256>;
var<workgroup> counts: array<u32, 256>;

fn bin_of(l: f32) -> u32 {
    if !(l > 0.0) {
        return 0u;
    }
    let t = (log2(l) - params.range.x) / (params.range.y - params.range.x);
    return u32(clamp(i32(t * f32(BINS)), 0, i32(BINS) - 1));
}

@compute @workgroup_size(16, 16, 1)
fn cs_histogram(@builtin(global_invocation_id) id: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    atomicStore(&local_bins[li], 0u);
    workgroupBarrier();
    let dims = textureDimensions(source);
    if id.x < dims.x && id.y < dims.y {
        let c = max(textureLoad(source, vec2<i32>(id.xy), 0).rgb, vec3<f32>(0.0));
        let l = dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
        atomicAdd(&local_bins[bin_of(l)], 1u);
    }
    workgroupBarrier();
    let n = atomicLoad(&local_bins[li]);
    if n > 0u {
        atomicAdd(&bins[li], n);
    }
}

@compute @workgroup_size(256, 1, 1)
fn cs_adapt(@builtin(local_invocation_index) li: u32) {
    counts[li] = atomicLoad(&bins[li]);
    workgroupBarrier();
    if li != 0u {
        return;
    }
    var total = 0.0;
    for (var i = 0u; i < BINS; i = i + 1u) {
        total = total + f32(counts[i]);
    }
    var mean = 0.0;
    if total > 0.0 {
        let lo = total * params.range.z;
        let hi = total * params.range.w;
        var below = 0.0;
        var sum = 0.0;
        var weight = 0.0;
        for (var i = 0u; i < BINS; i = i + 1u) {
            let c = f32(counts[i]);
            let start = below;
            let end = below + c;
            below = end;
            let w = max(min(end, hi) - max(start, lo), 0.0);
            let ev = params.range.x + (f32(i) + 0.5) / f32(BINS) * (params.range.y - params.range.x);
            sum = sum + w * ev;
            weight = weight + w;
        }
        if weight > 0.0 {
            mean = sum / weight;
        }
    }
    let target_ev = clamp(params.adapt.x - mean, -params.adapt.y, params.adapt.y);
    var ev = state.v.x;
    if params.flags.x > 0.5 || state.v.z < 0.5 {
        ev = target_ev;
    } else {
        let k = 1.0 - exp(-max(params.adapt.w, 0.0) / max(params.adapt.z, 1e-3));
        ev = ev + (target_ev - ev) * k;
    }
    state.v = vec4<f32>(ev, target_ev, 1.0, mean);
}
