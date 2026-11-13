// The tonemap after the temporal upscaler (after tonemap.wgsl, whose sharpen's weight is multiplied by `confidence`, and
// SHARPEN_FULL_FRAMES): Ultra's sharpen is as strong as the history is long. None where the history has just started (a
// disocclusion's pixel is this frame's upscaled sample alone, and sharpening it sharpens its aliasing), full from
// SHARPEN_FULL_FRAMES frames held on.

// The upscaler's output at the output's size (its alpha the frames its history holds).
@group(0) @binding(6) var upscaled: texture_2d<f32>;

// How much of the sharpen a pixel takes: the share of SHARPEN_FULL_FRAMES frames its history holds past its first.
fn confidence(xy: vec2<i32>) -> f32 {
    let q = clamp(xy, vec2<i32>(0), vec2<i32>(textureDimensions(upscaled)) - 1);
    return clamp((textureLoad(upscaled, q, 0).a - 1.0) / (SHARPEN_FULL_FRAMES - 1.0), 0.0, 1.0);
}
