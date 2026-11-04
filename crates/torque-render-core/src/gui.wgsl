// The GUI's 2D drawing: an orthographic projection (pixels, y down) and `GL_MODULATE` of one texture by the
// vertex colour (an untextured draw binds a white texel). Blending and clipping are the pipeline's and the scissor's.

struct Screen {
    // The target's size in pixels.
    size: vec4<f32>,
};

@group(0) @binding(0) var<uniform> screen: Screen;
@group(1) @binding(0) var tex: texture_2d<f32>;
@group(1) @binding(1) var samp: sampler;

struct VertexIn {
    @location(0) position: vec2<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec4<f32>,
};

struct VertexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
};

@vertex
fn vs_main(v: VertexIn) -> VertexOut {
    var out: VertexOut;
    out.clip = vec4<f32>(v.position.x / screen.size.x * 2.0 - 1.0, 1.0 - v.position.y / screen.size.y * 2.0, 0.0, 1.0);
    out.uv = v.uv;
    out.color = v.color;
    return out;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    return textureSample(tex, samp, in.uv) * in.color;
}
