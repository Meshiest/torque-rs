// The shadow maps' casters: depth only.
//
// - vs_sun: a sun cascade, orthographic. Casters nearer the sun than the light's near plane are flattened onto it (pancaking): by the
//   rasterizer's depth clamp when the device has unclipped depth (UNCLIPPED), else by clamping each vertex's depth here.
// - vs_point / fs_point: a point light's cube face; the depth stored is the distance to the light over its radius (linear, the same
//   in every direction, so a lookup compares one number however the faces meet). The objects the light is mounted in are left out
//   (a light inside an object would otherwise be hidden from everything by its faces; the extension's `ext_excluded`).
// - vs_clear: a triangle over the whole target at the far depth (a cached cascade's region is cleared under a scissor this way before
//   only its casters are drawn again).
// - fs_scroll (with vs_clear): a cached cascade moved by whole texels, from its copy: each texel the depth the copy holds at its
//   place less the shift, the far depth where that lies off the map (the uncovered strip, drawn again after).
// - vs_trans / fs_trans: a cascade's transmittance map (half its side, cleared to white, multiplied): each translucent
//   fragment nearer the sun than the first opaque surface (the static cascade's depth, the farthest of the 2 x 2 texels it covers)
//   lets through (1 - a) + a x its linear paint, so the light the sun sends past it is coloured. The forward pass multiplies the
//   sun's direct light on opaque receivers by it.
//
// Permutation constant (prepended): UNCLIPPED (bool).

struct Caster {
    view_proj: mat4x4<f32>,
    // xyz: the point light; w: its radius.
    light: vec4<f32>,
    // The objects (their extra words) a point light is mounted in: left out of its own cube (0xffffffff: none).
    exclude: vec4<u32>,
    // The extension's block (CasterExt, from its chunk).
    ext: CasterExt,
};

@group(0) @binding(0) var<uniform> caster: Caster;
// The meshes' places (NodePlace; forward.wgsl's `nodes`): a shape's vertex in its mesh's own space (MAT_NODE in its material word)
// names its place by its extra word and is cast from where the place puts it now.
@group(3) @binding(2) var<storage, read> nodes: array<vec4<f32>>;
const MAT_NODE: u32 = 0x80000000u;

fn placed(position: vec3<f32>, material: u32, extra: u32) -> vec3<f32> {
    if (material & MAT_NODE) == 0u {
        return position;
    }
    let p = vec4<f32>(position, 1.0);
    let k = 9u * extra;
    return vec3<f32>(dot(nodes[k], p), dot(nodes[k + 1u], p), dot(nodes[k + 2u], p));
}

// Whether a point light leaves this vertex's surface (its extra word) out of its own cube: the extension's (ext_excluded).
fn excluded(extra: u32) -> bool {
    return ext_excluded(extra);
}

// The static cascades (the transmittance's depth test); the cascade drawn is caster.exclude.x.
@group(1) @binding(0) var csm_static: texture_depth_2d_array;

@vertex
fn vs_sun(
    @location(0) position: vec3<f32>,
    @location(4) material: u32,
    @location(5) extra: u32,
) -> @builtin(position) vec4<f32> {
    var c = caster.view_proj * vec4<f32>(placed(position, material, extra), 1.0);
    if !UNCLIPPED {
        c.z = max(c.z, 0.0);
    }
    return c;
}

struct PointOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
};

@vertex
fn vs_point(
    @location(0) position: vec3<f32>,
    @location(4) material: u32,
    @location(5) extra: u32,
) -> PointOut {
    var out: PointOut;
    let world = placed(position, material, extra);
    out.clip = caster.view_proj * vec4<f32>(world, 1.0);
    // (A shape's mesh place is no object of the light's.)
    if (material & MAT_NODE) == 0u && excluded(extra) {
        // Outside the clip volume (every corner of the object's triangles): not drawn.
        out.clip = vec4<f32>(2.0, 2.0, 2.0, 1.0);
    }
    out.world = world;
    return out;
}

@fragment
fn fs_point(in: PointOut) -> @builtin(frag_depth) f32 {
    return clamp(distance(in.world, caster.light.xyz) / max(caster.light.w, 1e-4), 0.0, 1.0);
}

@vertex
fn vs_clear(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let x = f32((i << 1u) & 2u);
    let y = f32(i & 2u);
    return vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 1.0, 1.0);
}

// The copy of the cascades and the scroll: the shift in texels (x, y), the layer, the map's side.
@group(2) @binding(0) var scroll_from: texture_depth_2d_array;
@group(2) @binding(1) var<uniform> scroll: vec4<i32>;

@fragment
fn fs_scroll(@builtin(position) p: vec4<f32>) -> @builtin(frag_depth) f32 {
    let q = vec2<i32>(p.xy) - scroll.xy;
    if any(q < vec2<i32>(0)) || any(q >= vec2<i32>(scroll.w)) {
        return 1.0;
    }
    return textureLoad(scroll_from, q, scroll.z, 0);
}

struct TransOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs_trans(@location(0) position: vec3<f32>, @location(3) color: vec4<f32>) -> TransOut {
    var out: TransOut;
    var c = caster.view_proj * vec4<f32>(position, 1.0);
    if !UNCLIPPED {
        c.z = max(c.z, 0.0);
    }
    out.clip = c;
    out.color = color;
    return out;
}

fn trans_linear(c: vec3<f32>) -> vec3<f32> {
    let x = max(c, vec3<f32>(0.0));
    return select(pow((x + 0.055) / 1.055, vec3<f32>(2.4)), x / 12.92, x <= vec3<f32>(0.04045));
}

@fragment
fn fs_trans(in: TransOut) -> @location(0) vec4<f32> {
    let layer = i32(caster.exclude.x);
    let dims = vec2<i32>(textureDimensions(csm_static));
    let base = vec2<i32>(in.clip.xy) * 2;
    var far = 0.0;
    for (var k = 0; k < 4; k = k + 1) {
        let q = clamp(base + vec2<i32>(k & 1, k >> 1), vec2<i32>(0), dims - 1);
        far = max(far, textureLoad(csm_static, q, layer, 0));
    }
    let z = clamp(in.clip.z, 0.0, 1.0);
    if z > far + 1e-4 {
        discard;
    }
    let a = clamp(in.color.a, 0.0, 1.0);
    return vec4<f32>(vec3<f32>(1.0 - a) + a * trans_linear(in.color.rgb), 1.0);
}
