// The upscaler's history read in 32-bit floats (after tsr.wgsl; the half floats' reference in the precision test):
// tsr_gather.wgsl's cubic with loads (a 32-bit history is not filterable).
const HALF: bool = false;

// The pass's targets: the history, the shown colour (a 32-bit history is not filterable, so the tonemap reads a copy of it in
// half floats, with the frames held in alpha as a half-float history has), the moments and the ring's layer.
struct TaaOut {
    @location(0) history: vec4<f32>,
    @location(1) shown: vec4<f32>,
    @location(2) moments: vec4<f32>,
    @location(3) ring: vec4<f32>,
};

fn emit(history: vec4<f32>, level: f32, moments: vec4<f32>, x: f32) -> TaaOut {
    var out: TaaOut;
    out.history = history;
    out.shown = history;
    out.moments = moments;
    out.ring = vec4<f32>(x, 0.0, 0.0, 0.0);
    return out;
}

fn tsr_history(uv: vec2<f32>) -> vec4<f32> {
    return history_at(uv);
}
