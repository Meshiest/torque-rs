// A picture of the voxel scene from a camera (after gi_common.wgsl and gi_scene.wgsl): each pixel's ray marched from the eye
// through the clipmap, the voxel it meets shown by its albedo, its normal or its lit radiance, its faces turned toward the eye
// a little lighter and each voxel's edges a little darker (the voxels read as blocks).

struct DebugParams {
    inv_view_proj: mat4x4<f32>,
    eye: vec4<f32>,
    // x, y: the picture's size; z: what it shows (0 albedo, 1 normal, 2 radiance).
    size: vec4<u32>,
};

@group(0) @binding(0) var<uniform> vol: GiVolume;
@group(0) @binding(1) var<uniform> dbg: DebugParams;
@group(0) @binding(2) var<storage, read> tiles: array<vec2<u32>>;
@group(0) @binding(3) var<storage, read> occ: array<u32>;
@group(0) @binding(4) var<storage, read> voxels: array<vec4<u32>>;
@group(0) @binding(5) var<storage, read_write> pixels: array<vec4<f32>>;

@compute @workgroup_size(8, 8)
fn cs_debug(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= dbg.size.x || id.y >= dbg.size.y {
        return;
    }
    let ndc = (vec2<f32>(id.xy) + 0.5) / vec2<f32>(dbg.size.xy) * 2.0 - 1.0;
    let q = dbg.inv_view_proj * vec4<f32>(ndc.x, -ndc.y, 0.5, 1.0);
    let o = dbg.eye.xyz;
    let d = normalize(q.xyz / q.w - o);
    let hit = gi_march(o, d, 4096.0, 0u);
    var c = vec3<f32>(0.02, 0.025, 0.04);
    if hit.hit {
        let w = voxels[hit.voxel];
        let n = gi_unoct(unpack2x16snorm(w.y));
        let s = vol.levels[hit.level].size.x;
        // The face the ray entered by, for the shading and the edges.
        let p = o + d * hit.t;
        let f = fract(p / s);
        let e = min(f, 1.0 - f);
        // The middle of the three distances to the voxel's faces: the smallest is the face itself.
        let edge = e.x + e.y + e.z - min(e.x, min(e.y, e.z)) - max(e.x, max(e.y, e.z));
        let lines = select(1.0, 0.75, edge < 0.06);
        if dbg.size.z == 0u {
            c = gi_unpack_albedo(w.x) * (0.4 + 0.6 * max(dot(n, -d), 0.0)) * lines;
        } else if dbg.size.z == 1u {
            c = (n * 0.5 + 0.5) * lines;
        } else {
            c = gi_unpack_rgb9e5(w.w) * lines;
        }
    }
    pixels[id.x + id.y * dbg.size.x] = vec4<f32>(c, 1.0);
}
