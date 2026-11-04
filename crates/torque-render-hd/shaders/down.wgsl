// A mip level from the one above it: each texel the mean of the 2 x 2 texels it covers (one bilinear tap at their shared
// corner). The previous frame's colour (the screen-space reflections' rough lookups) and the cube maps' faces (the environment's
// rough lookups) are filtered down this way. Our own code. A compute filter, each level of a chain one dispatch of one pass.

@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(2) var dst: texture_storage_2d<rgba16float, write>;
@group(1) @binding(0) var samp: sampler;

@compute @workgroup_size(8, 8)
fn cs_down(@builtin(global_invocation_id) id: vec3<u32>) {
    if any(id.xy >= textureDimensions(dst)) {
        return;
    }
    let dims = vec2<f32>(textureDimensions(src));
    let uv = (vec2<f32>(id.xy) * 2.0 + 1.0) / dims;
    textureStore(dst, id.xy, textureSampleLevel(src, samp, uv, 0.0));
}
