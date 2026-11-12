// A dissolving object's slot and its noise field (our own code; composed after forward.wgsl or shadow.wgsl into the dissolve
// permutations only, so the modules every other draw uses are unchanged). The field is torque_render_scene::hd::dissolve::field:
// three octaves of value noise of the object-space point over the cell size, mixed with the point's height share by the sweep.
//
// A draw of a dissolving object is drawn as instance `slot` of one, its slot its motion slot: `dissolves[slot]` is its object's.

struct DissolveSlot {
    // The world to the object's own space (where the field is: the pattern stays on the object as it moves).
    to_object: mat4x4<f32>,
    // rgb: the edge's light (linear, HDR); w unused.
    edge: vec4<f32>,
    // x: the threshold (drawn at or over it); y: the edge's end (glows under it); z: one over the cell; w: the sweep.
    band: vec4<f32>,
    // x, y: the object's lowest and highest z in its own space.
    span: vec4<f32>,
    pad: vec4<f32>,
};

fn dissolve_lattice(c: vec3<i32>) -> f32 {
    let u = bitcast<vec3<u32>>(c);
    var h = (u.x * 0x8da6b343u) ^ (u.y * 0xd8163841u) ^ (u.z * 0xcb1ab31fu);
    h = h ^ (h >> 16u);
    h = h * 0x7feb352du;
    h = h ^ (h >> 15u);
    h = h * 0x846ca68bu;
    h = h ^ (h >> 16u);
    return f32(h >> 8u) / 16777216.0;
}

fn dissolve_value(p: vec3<f32>) -> f32 {
    let i = floor(p);
    let f = p - i;
    let u = f * f * (3.0 - 2.0 * f);
    let c = vec3<i32>(i);
    let x00 = mix(dissolve_lattice(c), dissolve_lattice(c + vec3<i32>(1, 0, 0)), u.x);
    let x10 = mix(dissolve_lattice(c + vec3<i32>(0, 1, 0)), dissolve_lattice(c + vec3<i32>(1, 1, 0)), u.x);
    let x01 = mix(dissolve_lattice(c + vec3<i32>(0, 0, 1)), dissolve_lattice(c + vec3<i32>(1, 0, 1)), u.x);
    let x11 = mix(dissolve_lattice(c + vec3<i32>(0, 1, 1)), dissolve_lattice(c + vec3<i32>(1, 1, 1)), u.x);
    return mix(mix(x00, x10, u.y), mix(x01, x11, u.y), u.z);
}

fn dissolve_noise(q: vec3<f32>) -> f32 {
    return dissolve_value(q) * 0.5 + dissolve_value(q * 2.03 + vec3<f32>(17.1)) * 0.3
        + dissolve_value(q * 4.01 + vec3<f32>(31.7)) * 0.2;
}

// The field at the world point `world` of the object of `d`.
fn dissolve_field(d: DissolveSlot, world: vec3<f32>) -> f32 {
    let p = (d.to_object * vec4<f32>(world, 1.0)).xyz;
    let n = dissolve_noise(p * d.band.z);
    let span = d.span.y - d.span.x;
    var h = 0.5;
    if span > 1e-6 {
        h = clamp((p.z - d.span.x) / span, 0.0, 1.0);
    }
    let s = clamp(d.band.w, -1.0, 1.0);
    let ramp = select(h, 1.0 - h, s > 0.0);
    return n * (1.0 - abs(s)) + ramp * abs(s);
}
