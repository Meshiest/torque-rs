// The forward module's dissolving draws (our own code; composed after forward.wgsl and dissolve_common.wgsl): the same vertex
// work as vs_main and vs_depth with the point's world place carried to the fragment, which discards where the object's field is
// under its threshold. The permutation composes forward.wgsl with its vs_main and fs_depth as plain functions (`vs_main_of`,
// `fs_depth_of`: an entry point cannot be called), so their work here is theirs. The prepass and the opaque pass discard the same fragments (the field of the same interpolated world
// point), so the depth, the motion, the normals and the colour agree; the edge's light is added in HDR (the bloom takes it).
//
// The slots are group 1's binding 2 (beside the texture and sampler: the dissolving draws' own group, as the pool's sprites have).

@group(1) @binding(2) var<storage, read> dissolves: array<DissolveSlot>;

struct DissolveOut {
    @builtin(position) @invariant clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) color: vec4<f32>,
    @location(4) @interpolate(flat) material: u32,
    @location(5) @interpolate(flat) extra: u32,
    @location(6) rest: vec3<f32>,
    @location(7) lod: f32,
    @location(8) @interpolate(flat) slot: u32,
};

@vertex
fn vs_dissolve(@builtin(instance_index) slot: u32, v: VertexIn) -> DissolveOut {
    let o = vs_main_of(v);
    var out: DissolveOut;
    out.clip = o.clip;
    out.world = o.world;
    out.normal = o.normal;
    out.uv = o.uv;
    out.color = o.color;
    out.material = o.material;
    out.extra = o.extra;
    out.rest = o.rest;
    out.lod = o.lod;
    out.slot = slot;
    return out;
}

@fragment
fn fs_dissolve(in: DissolveOut) -> @location(0) vec4<f32> {
    var v: VertexOut;
    v.clip = in.clip;
    v.world = in.world;
    v.normal = in.normal;
    v.uv = in.uv;
    v.color = in.color;
    v.material = in.material;
    v.extra = in.extra;
    v.rest = in.rest;
    v.lod = in.lod;
    let c = forward_color(v);
    let d = dissolves[in.slot];
    // The rest place: where vs_depth_dissolve's world point is (the shape fx move a vertex after it).
    let f = dissolve_field(d, in.rest);
    if f < d.band.x {
        discard;
    }
    if shadows.config.w != 0u || f >= d.band.y {
        return c;
    }
    // The edge: brightest at the front, gone at the band's end.
    let t = 1.0 - (f - d.band.x) / max(d.band.y - d.band.x, 1e-6);
    var light = d.edge.rgb * (t * t);
    if frame.fog_color.w > 0.0 {
        light = light * fog_keep(in.world);
    }
    return vec4<f32>(c.rgb + light, c.a);
}

struct DissolveDepthOut {
    @builtin(position) @invariant clip: vec4<f32>,
    @location(0) now: vec4<f32>,
    @location(1) before: vec4<f32>,
    @location(2) normal: vec3<f32>,
    @location(3) world: vec3<f32>,
    @location(4) @interpolate(flat) material: u32,
    @location(5) lod: f32,
    @location(6) @interpolate(flat) id: vec2<f32>,
    @location(7) rest: vec3<f32>,
    @location(8) @interpolate(flat) slot: u32,
};

@vertex
fn vs_depth_dissolve(
    @builtin(instance_index) slot: u32,
    @location(0) position: vec3<f32>,
    @location(1) normal: vec2<f32>,
    @location(4) material: u32,
    @location(5) extra: u32,
    @location(6) previous: vec3<f32>,
) -> DissolveDepthOut {
    let o = depth_out(position, normal, material, extra, previous, motions[slot]);
    var out: DissolveDepthOut;
    out.clip = o.clip;
    out.now = o.now;
    out.before = o.before;
    out.normal = o.normal;
    out.world = o.world;
    out.material = o.material;
    out.lod = o.lod;
    out.id = o.id;
    out.rest = placed(position, material, extra);
    out.slot = slot;
    return out;
}

@fragment
fn fs_depth_dissolve(in: DissolveDepthOut) -> PrepassTargets {
    let d = dissolves[in.slot];
    if dissolve_field(d, in.rest) < d.band.x {
        discard;
    }
    var p: PrepassOut;
    p.clip = in.clip;
    p.now = in.now;
    p.before = in.before;
    p.normal = in.normal;
    p.world = in.world;
    p.material = in.material;
    p.lod = in.lod;
    p.id = in.id;
    return fs_depth_of(p);
}
