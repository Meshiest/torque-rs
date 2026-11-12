// The casters of dissolving objects (our own code; composed after shadow.wgsl and dissolve_common.wgsl): vs_sun's and
// vs_point's work with the point's world place carried to a fragment that discards where the object's field is under its
// threshold (the same field as the forward pass's, so no shadow falls from the parts gone). The slots are the casters' group 3
// binding 3; a draw is instance `slot` of one.

@group(3) @binding(3) var<storage, read> dissolves: array<DissolveSlot>;

struct DissolveSunOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) @interpolate(flat) slot: u32,
};

@vertex
fn vs_sun_dissolve(
    @builtin(instance_index) slot: u32,
    @location(0) position: vec3<f32>,
    @location(4) material: u32,
    @location(5) extra: u32,
) -> DissolveSunOut {
    var out: DissolveSunOut;
    let world = placed(position, material, extra);
    var c = caster.view_proj * vec4<f32>(world, 1.0);
    if !UNCLIPPED {
        c.z = max(c.z, 0.0);
    }
    out.clip = c;
    out.world = world;
    out.slot = slot;
    return out;
}

@fragment
fn fs_sun_dissolve(in: DissolveSunOut) {
    let d = dissolves[in.slot];
    if dissolve_field(d, in.world) < d.band.x {
        discard;
    }
}

struct DissolvePointOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) @interpolate(flat) slot: u32,
};

@vertex
fn vs_point_dissolve(
    @builtin(instance_index) slot: u32,
    @location(0) position: vec3<f32>,
    @location(4) material: u32,
    @location(5) extra: u32,
) -> DissolvePointOut {
    var out: DissolvePointOut;
    let world = placed(position, material, extra);
    out.clip = caster.view_proj * vec4<f32>(world, 1.0);
    if (material & MAT_NODE) == 0u && excluded(extra) {
        out.clip = vec4<f32>(2.0, 2.0, 2.0, 1.0);
    }
    out.world = world;
    out.slot = slot;
    return out;
}

@fragment
fn fs_point_dissolve(in: DissolvePointOut) -> @builtin(frag_depth) f32 {
    let d = dissolves[in.slot];
    if dissolve_field(d, in.world) < d.band.x {
        discard;
    }
    return clamp(distance(in.world, caster.light.xyz) / max(caster.light.w, 1e-4), 0.0, 1.0);
}
