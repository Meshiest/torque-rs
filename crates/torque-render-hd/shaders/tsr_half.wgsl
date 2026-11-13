// The upscaler's history read in half floats (after tsr.wgsl): tsr_gather.wgsl's cubic (Keys', of the Catmull-Rom family and sharper)
// over the 4 x 4 texels about `uv` in 9 bilinear fetches (each axis's two middle taps, both weighted positive, share one fetch at
// the offset of their weights; a half-float history is filterable). A texel's centre reads that texel alone (a still pixel's
// history is read exactly).
const HALF: bool = true;

// The pass's targets: the history (the shown colour too: a half-float history is the tonemap's source as it is), the moments
// and the ring's layer.
struct TaaOut {
    @location(0) history: vec4<f32>,
    @location(1) moments: vec4<f32>,
    @location(2) ring: vec4<f32>,
};

fn emit(history: vec4<f32>, level: f32, moments: vec4<f32>, x: f32) -> TaaOut {
    var out: TaaOut;
    out.history = history;
    out.moments = moments;
    out.ring = vec4<f32>(x, 0.0, 0.0, 0.0);
    return out;
}

fn tsr_history(uv: vec2<f32>) -> vec4<f32> {
    let pos = uv * frame.output.xy - 0.5;
    let c = floor(pos);
    let f = pos - c;
    if all(abs(f) < vec2<f32>(1e-4)) {
        let size = vec2<i32>(frame.output.xy);
        return textureLoad(history, clamp(vec2<i32>(c), vec2<i32>(0), size - 1), 0);
    }
    let w0 = keys(1.0 + f);
    let w1 = keys(f);
    let w2 = keys(1.0 - f);
    let w3 = keys(2.0 - f);
    let w12 = w1 + w2;
    let texel = frame.output.zw;
    let p0 = (c - 0.5) * texel;
    let p3 = (c + 2.5) * texel;
    let p12 = (c + 0.5 + w2 / w12) * texel;
    let xs = array<f32, 3>(p0.x, p12.x, p3.x);
    let ys = array<f32, 3>(p0.y, p12.y, p3.y);
    let wx = array<f32, 3>(w0.x, w12.x, w3.x);
    let wy = array<f32, 3>(w0.y, w12.y, w3.y);
    var sum = vec4<f32>(0.0);
    for (var y = 0; y < 3; y = y + 1) {
        for (var x = 0; x < 3; x = x + 1) {
            sum = sum + textureSampleLevel(history, linear_clamp, vec2<f32>(xs[x], ys[y]), 0.0) * (wx[x] * wy[y]);
        }
    }
    return max(sum, vec4<f32>(0.0));
}
