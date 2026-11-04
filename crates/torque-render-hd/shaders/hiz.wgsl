// The hierarchical depth (Hi-Z) the screen-space reflections are traced through: level 0 the depth prepass's reversed depth
// (0, the far plane, outside the frame: the texture is padded to a multiple of its coarsest texel), each level above it the nearest
// (largest reversed) depth of the 2 x 2 texels below. Our own code; the pyramid as in Uludag 2014 ("Hi-Z Screen-Space Cone-Traced
// Reflections", GPU Pro 5), kept with the near depth for a reversed buffer. Compute filters, the whole pyramid one pass.
// `cs_down_far` makes the occlusion culling's pyramid from the same first level.

@group(0) @binding(0) var src_depth: texture_depth_2d;
@group(0) @binding(1) var src: texture_2d<f32>;
@group(0) @binding(2) var dst: texture_storage_2d<r32float, write>;

@compute @workgroup_size(8, 8)
fn cs_first(@builtin(global_invocation_id) id: vec3<u32>) {
    if any(id.xy >= textureDimensions(dst)) {
        return;
    }
    let p = vec2<i32>(id.xy);
    let dims = vec2<i32>(textureDimensions(src_depth));
    var d = 0.0;
    if all(p < dims) {
        d = textureLoad(src_depth, p, 0);
    }
    textureStore(dst, p, vec4<f32>(d, 0.0, 0.0, 0.0));
}

@compute @workgroup_size(8, 8)
fn cs_down(@builtin(global_invocation_id) id: vec3<u32>) {
    if any(id.xy >= textureDimensions(dst)) {
        return;
    }
    let p = vec2<i32>(id.xy) * 2;
    let a = textureLoad(src, p, 0).r;
    let b = textureLoad(src, p + vec2<i32>(1, 0), 0).r;
    let c = textureLoad(src, p + vec2<i32>(0, 1), 0).r;
    let d = textureLoad(src, p + vec2<i32>(1, 1), 0).r;
    textureStore(dst, vec2<i32>(id.xy), vec4<f32>(max(max(a, b), max(c, d)), 0.0, 0.0, 0.0));
}

// The occlusion test's first level: the depth, and past the frame the nearest depth (1): nothing is drawn there and a box's
// rectangle is cut to the frame, so a coarse texel across the frame's edge is judged by the frame's own pixels (with the far
// plane there, every box touching the bottom or right edge at a coarse level was kept).
@compute @workgroup_size(8, 8)
fn cs_first_cull(@builtin(global_invocation_id) id: vec3<u32>) {
    if any(id.xy >= textureDimensions(dst)) {
        return;
    }
    let p = vec2<i32>(id.xy);
    let dims = vec2<i32>(textureDimensions(src_depth));
    var d = 1.0;
    if all(p < dims) {
        d = textureLoad(src_depth, p, 0);
    }
    textureStore(dst, p, vec4<f32>(d, 0.0, 0.0, 0.0));
}

// The occlusion test's pyramid: the same levels with the farthest (smallest reversed) depth of the 2 x 2 texels below, so a
// box whose nearest depth lies beyond a texel's value is behind everything drawn there.
@compute @workgroup_size(8, 8)
fn cs_down_far(@builtin(global_invocation_id) id: vec3<u32>) {
    if any(id.xy >= textureDimensions(dst)) {
        return;
    }
    // Reads past an odd source side take its last texel (never past its edge).
    let m = vec2<i32>(textureDimensions(src)) - 1;
    let p = vec2<i32>(id.xy) * 2;
    let a = textureLoad(src, min(p, m), 0).r;
    let b = textureLoad(src, min(p + vec2<i32>(1, 0), m), 0).r;
    let c = textureLoad(src, min(p + vec2<i32>(0, 1), m), 0).r;
    let d = textureLoad(src, min(p + vec2<i32>(1, 1), m), 0).r;
    textureStore(dst, vec2<i32>(id.xy), vec4<f32>(min(min(a, b), min(c, d)), 0.0, 0.0, 0.0));
}
