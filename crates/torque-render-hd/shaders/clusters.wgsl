// The light clusters (clustered shading, Olsson, Billeter and Assarsson 2012): the view frustum cut into tiles across and
// down the screen and exponential depth slices, each cluster's box in view space tested against every light's sphere, the hits set
// as bits of the cluster's 1,024-bit mask. One workgroup a cluster; its threads take the lights in strides and atomicOr their bits
// into workgroup memory, so the mask does not depend on the order they run in. The box is widened by a thousandth of its far depth
// (the fragments find their cluster by a logarithm, the box is made with a power).

@group(0) @binding(0) var<uniform> frame: Frame;
@group(0) @binding(1) var<storage, read> lights: array<Light>;
@group(0) @binding(2) var<storage, read_write> masks: array<u32>;

var<workgroup> words: array<atomic<u32>, 32>;

// The view-space point at depth d (distance along -z) on the ray through ndc (x, y): the near plane is NDC z = 1 (reversed depth).
fn view_at(ndc: vec2<f32>, d: f32) -> vec3<f32> {
    let p = frame.inv_proj * vec4<f32>(ndc, 1.0, 1.0);
    let q = p.xyz / p.w;
    return q * (d / max(-q.z, 1e-6));
}

@compute @workgroup_size(64)
fn cs_main(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let tiles = frame.clusters.xyz;
    let cluster = wg.x;
    if li < CLUSTER_WORDS {
        atomicStore(&words[li], 0u);
    }
    workgroupBarrier();
    let tx = cluster % tiles.x;
    let ty = (cluster / tiles.x) % tiles.y;
    let tz = cluster / (tiles.x * tiles.y);
    let near = frame.cluster_z.x;
    let far = frame.cluster_z.y;
    let ratio = far / near;
    let d0 = near * pow(ratio, f32(tz) / f32(tiles.z));
    let d1 = near * pow(ratio, f32(tz + 1u) / f32(tiles.z));
    let x0 = -1.0 + 2.0 * f32(tx) / f32(tiles.x);
    let x1 = -1.0 + 2.0 * f32(tx + 1u) / f32(tiles.x);
    let y0 = 1.0 - 2.0 * f32(ty) / f32(tiles.y);
    let y1 = 1.0 - 2.0 * f32(ty + 1u) / f32(tiles.y);
    var lo = vec3<f32>(1e30);
    var hi = vec3<f32>(-1e30);
    for (var k = 0u; k < 8u; k = k + 1u) {
        let x = select(x0, x1, (k & 1u) != 0u);
        let y = select(y0, y1, (k & 2u) != 0u);
        let d = select(d0, d1, (k & 4u) != 0u);
        let p = view_at(vec2<f32>(x, y), d);
        lo = min(lo, p);
        hi = max(hi, p);
    }
    let pad = 1e-3 * d1;
    lo = lo - vec3<f32>(pad);
    hi = hi + vec3<f32>(pad);
    let count = min(frame.clusters.w, MAX_LIGHTS);
    for (var i = li; i < count; i = i + 64u) {
        let light = lights[i];
        let c = (frame.view * vec4<f32>(light.pos_radius.xyz, 1.0)).xyz;
        let q = clamp(c, lo, hi);
        let e = c - q;
        let r = light.pos_radius.w;
        if dot(e, e) <= r * r {
            atomicOr(&words[i / 32u], 1u << (i % 32u));
        }
    }
    workgroupBarrier();
    if li < CLUSTER_WORDS {
        masks[cluster * CLUSTER_WORDS + li] = atomicLoad(&words[li]);
    }
}
