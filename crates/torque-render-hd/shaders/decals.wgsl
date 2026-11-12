// The projected decals (decals.rs): each decal an oriented box drawn by its inside faces over the opaque frame, the opaque
// frame's point at each pixel (its depth) tested against the box. Composed after the forward pass's text (its frame, sky and
// reflection groups, its fog and its `shaded`); group 1 is the decal texture's, with the boxes and the prepass's targets.
//
// - The point: the depth's world point at the pixel (the jittered camera the depth was drawn with). It takes the decal where it
//   lies in the box, on a surface that takes decals (the prepass's normal w: 1; 0.5 a moving shape's, 0 nothing drawn), facing
//   within the CPU path's clip cosine of the decal's normal (faded in over FACING_FADE past it). Its texture coordinates are
//   the CPU path's across the box's face; their gradients are the next pixels' rays met on the surface's plane, so a pixel at a
//   silhouette takes no far mip from the depth's jump.
// - A multiplied decal (fs_decal_multiply) is fs_modulate2x's factor over the frame; an alpha-blended one (fs_decal_alpha) the
//   forward pass's `shaded` of the CPU path's material word at the point, its alpha times the box's share.

struct DecalBox {
    // xyz: the center; w: the opacity.
    center: vec4<f32>,
    // xyz: half the face along each of its sides (w unused).
    x: vec4<f32>,
    y: vec4<f32>,
    // The face's normal times the box's half depth, and the CPU path's material word of the decal.
    z: vec3<f32>,
    material: u32,
};

@group(1) @binding(2) var<storage, read> decal_boxes: array<DecalBox>;
@group(1) @binding(3) var decal_depth: texture_depth_2d;
@group(1) @binding(4) var decal_normals: texture_2d<f32>;

// torque_render_scene::decals::CLIP_ANGLE_COS and torque_render_scene::hd::decals::FACING_FADE.
const DECAL_CLIP_COS: f32 = 0.17;
const DECAL_FACING_FADE: f32 = 0.1;
// The longest texture gradient a pixel takes (a whole texture: the smallest mip).
const DECAL_MAX_GRADIENT: f32 = 1.0;

struct DecalOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) @interpolate(flat) index: u32,
};

// Component `c` of a box corner on the face across `axis` at `side`, `cu` and `cw` along the next two axes (by selects: a
// vector's component written at a computed index is not addressable everywhere).
fn box_corner(c: u32, axis: u32, side: f32, cu: f32, cw: f32) -> f32 {
    return select(select(cw, cu, c == (axis + 1u) % 3u), side, c == axis);
}

// Corner `v` of the box's 12 triangles (6 a face: +x, -x, +y, -y, +z, -z), counter-clockwise seen from outside when the axes are
// right-handed. A left-handed box (x cross y against z) is drawn with its z mirrored, so its outside stays counter-clockwise.
@vertex
fn vs_decal(@builtin(vertex_index) v: u32, @builtin(instance_index) i: u32) -> DecalOut {
    let b = decal_boxes[i];
    let face = v / 6u;
    let k = v % 6u;
    let axis = face / 2u;
    let outward = (face & 1u) == 0u;
    // The quad's corners 0 1 2, 0 2 3 of (-,-), (+,-), (+,+), (-,+).
    var q = k;
    if k == 3u {
        q = 0u;
    } else if k > 3u {
        q = k - 2u;
    }
    var cu = select(-1.0, 1.0, q == 1u || q == 2u);
    var cw = select(-1.0, 1.0, q >= 2u);
    if !outward {
        let t = cu;
        cu = cw;
        cw = t;
    }
    let side = select(-1.0, 1.0, outward);
    let s = vec3<f32>(
        box_corner(0u, axis, side, cu, cw),
        box_corner(1u, axis, side, cu, cw),
        box_corner(2u, axis, side, cu, cw),
    );
    var z = b.z;
    if dot(cross(b.x.xyz, b.y.xyz), z) < 0.0 {
        z = -z;
    }
    let p = b.center.xyz + b.x.xyz * s.x + b.y.xyz * s.y + z * s.z;
    var out: DecalOut;
    out.clip = frame.view_proj * vec4<f32>(p, 1.0);
    out.index = i;
    return out;
}

// The world point of internal pixel position `px` (continuous) at reversed depth `d`.
fn decal_world(px: vec2<f32>, d: f32) -> vec3<f32> {
    let v = view_point(px, d);
    // The view's inverse: its rotation's transpose about its translation.
    let r = mat3x3<f32>(frame.view[0].xyz, frame.view[1].xyz, frame.view[2].xyz);
    return transpose(r) * (v - frame.view[3].xyz);
}

struct DecalHit {
    // The opaque frame's point, its depth and its surface's normal.
    p: vec3<f32>,
    d: f32,
    n: vec3<f32>,
    // The point's place in the box (-1 to 1 inside).
    l: vec3<f32>,
    // The share of the decal at the pixel (0 outside the box, on a surface that takes none, or facing away).
    keep: f32,
};

// What the texture is sampled with: the coordinates and their change to the next pixel across and down, and the point's.
struct DecalTexel {
    uv: vec2<f32>,
    duvx: vec2<f32>,
    duvy: vec2<f32>,
    dpx: vec3<f32>,
    dpy: vec3<f32>,
};

// The box's place of world point `p`: -1 to 1 along each axis inside it.
fn decal_local(b: DecalBox, p: vec3<f32>) -> vec3<f32> {
    let r = p - b.center.xyz;
    return vec3<f32>(
        dot(r, b.x.xyz) / dot(b.x.xyz, b.x.xyz),
        dot(r, b.y.xyz) / dot(b.y.xyz, b.y.xyz),
        dot(r, b.z) / dot(b.z, b.z),
    );
}

// The CPU path's texture coordinates of box place `l`.
fn decal_uv(l: vec3<f32>) -> vec2<f32> {
    return vec2<f32>(0.5 + 0.5 * l.x, 0.5 - 0.5 * l.y);
}

// Where the ray of pixel position `px` meets the plane through `p` facing `n` (`p` itself when the ray runs along the plane).
fn decal_on_plane(px: vec2<f32>, p: vec3<f32>, n: vec3<f32>) -> vec3<f32> {
    let e = frame.eye.xyz;
    let dir = decal_world(px, 0.5) - e;
    let along = dot(dir, n);
    if abs(along) < 1e-6 {
        return p;
    }
    return e + dir * (dot(p - e, n) / along);
}

fn decal_gradient(g: vec2<f32>) -> vec2<f32> {
    let l = length(g);
    if l > DECAL_MAX_GRADIENT {
        return g * (DECAL_MAX_GRADIENT / l);
    }
    return g;
}

// The pixel's point and its share of the decal: two loads and the box test, all a pixel outside the decal costs before its
// discard (a box over the whole screen draws every pixel of it).
fn decal_hit(b: DecalBox, frag: vec2<f32>) -> DecalHit {
    var h: DecalHit;
    let px = vec2<i32>(frag);
    h.d = textureLoad(decal_depth, px, 0);
    let nrm = textureLoad(decal_normals, px, 0);
    h.p = decal_world(frag, max(h.d, 1e-7));
    h.n = oct_decode(nrm.xy);
    h.l = decal_local(b, h.p);
    let inside = h.d > 0.0 && nrm.w > 0.75 && max(abs(h.l.x), max(abs(h.l.y), abs(h.l.z))) <= 1.0;
    let facing = dot(h.n, normalize(b.z));
    let t = clamp((facing - DECAL_CLIP_COS) / DECAL_FACING_FADE, 0.0, 1.0);
    h.keep = select(0.0, t * t * (3.0 - 2.0 * t), inside);
    return h;
}

// The texture's coordinates at a hit and their gradients.
fn decal_texel(b: DecalBox, frag: vec2<f32>, h: DecalHit) -> DecalTexel {
    var t: DecalTexel;
    t.uv = decal_uv(h.l);
    let px1 = decal_on_plane(frag + vec2<f32>(1.0, 0.0), h.p, h.n);
    let py1 = decal_on_plane(frag + vec2<f32>(0.0, 1.0), h.p, h.n);
    t.dpx = px1 - h.p;
    t.dpy = py1 - h.p;
    t.duvx = decal_gradient(decal_uv(decal_local(b, px1)) - t.uv);
    t.duvy = decal_gradient(decal_uv(decal_local(b, py1)) - t.uv);
    return t;
}

@fragment
fn fs_decal_multiply(in: DecalOut) -> @location(0) vec4<f32> {
    let b = decal_boxes[in.index];
    let h = decal_hit(b, in.clip.xy);
    if h.keep <= 0.0 {
        discard;
    }
    let x = decal_texel(b, in.clip.xy, h);
    let t = textureSampleGrad(tex, samp, x.uv, x.duvx, x.duvy);
    var keep = clamp(t.a * b.center.w, 0.0, 1.0) * h.keep;
    if frame.fog_color.w > 0.0 {
        keep = keep * fog_keep(h.p);
    }
    if frame.water.z > 0.5 {
        keep = keep * under_water_keep(h.p);
    }
    if keep <= 0.0 {
        discard;
    }
    if shadows.config.w != 0u && frame.flags.y < 0.5 {
        return vec4<f32>(0.5, 0.5, 0.5, 1.0);
    }
    let factor = mix(vec3<f32>(1.0), 2.0 * linear_to_srgb(t.rgb), keep);
    return vec4<f32>(0.5 * pow(max(factor, vec3<f32>(0.0)), vec3<f32>(2.2)), 1.0);
}

@fragment
fn fs_decal_alpha(in: DecalOut) -> @location(0) vec4<f32> {
    let b = decal_boxes[in.index];
    let h = decal_hit(b, in.clip.xy);
    if h.keep <= 0.0 {
        discard;
    }
    let x = decal_texel(b, in.clip.xy, h);
    let t = textureSampleGrad(tex, samp, x.uv, x.duvx, x.duvy);
    if t.a * b.center.w <= 0.0 {
        discard;
    }
    var v: VertexOut;
    v.clip = vec4<f32>(in.clip.xy, h.d, 1.0);
    v.world = h.p;
    v.normal = normalize(b.z);
    v.uv = x.uv;
    v.color = vec4<f32>(1.0, 1.0, 1.0, b.center.w);
    v.material = b.material;
    v.extra = 0u;
    v.rest = h.p;
    v.lod = 0.0;
    v.pool = vec4<f32>(0.0, -1.0, -1.0, 0.0);
    v.uv2 = x.uv;
    var d: Derivs;
    d.dpx = x.dpx;
    d.dpy = x.dpy;
    d.duvx = x.duvx;
    d.duvy = x.duvy;
    let s = shaded(v, d, t, 0.0);
    let a = s.alpha * h.keep;
    if a <= 0.0 {
        discard;
    }
    if s.debug {
        return debug_out;
    }
    return vec4<f32>(s.diffuse + s.spec, a);
}
