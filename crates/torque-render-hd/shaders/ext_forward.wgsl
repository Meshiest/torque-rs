// The engine's own extension, forward part: the hooks forward.wgsl and water.wgsl call, each leaving the surface as the engine
// shades it. An extension's forward chunk defines the same functions (and may add its own bindings, frame group 0 bindings 14 and
// 15 and reflections' group 3 binding 5, and entry points).

// A surface's paint (the vertex colour) before its texture.
fn ext_paint(in: VertexOut, paint_in: vec4<f32>, ng: vec3<f32>, v: vec3<f32>) -> vec4<f32> {
    return paint_in;
}

// Whether a plane keeps its own texture (tiled stochastically where the settings say).
fn ext_plane_tiles(material: u32) -> bool {
    return true;
}

// What a fragment's detail makes of its texture, normals, occlusion and sun, how far it lies in a hollow and whether the hollow
// was traced.
struct ExtDetail {
    t: vec4<f32>,
    n: vec3<f32>,
    ng: vec3<f32>,
    occlusion: f32,
    sun: f32,
    hollow: f32,
    traced: bool,
};

fn ext_detail(in: VertexOut, d: Derivs, fr: UvFrame, t_in: vec4<f32>, n_in: vec3<f32>, ng_in: vec3<f32>, lit: bool) -> ExtDetail {
    var x: ExtDetail;
    x.t = t_in;
    x.n = n_in;
    x.ng = ng_in;
    x.occlusion = 1.0;
    x.sun = 1.0;
    x.hollow = 0.0;
    x.traced = false;
    return x;
}

// The GL_DECAL mix of a surface's texture over its paint (gamma).
fn ext_decal(material: u32, paint: vec3<f32>, mixed_in: vec3<f32>, traced: bool) -> vec3<f32> {
    return mixed_in;
}

// An extension surface's (kind 0) material, set in `s`.
fn ext_surface(s: ptr<function, Surf>, material: u32, base: vec4<f32>, hollow: f32, translucent: bool) {
}

// A vertex where it is drawn at time `t`.
fn ext_position(position: vec3<f32>, material: u32, t: f32) -> vec4<f32> {
    return vec4<f32>(position, 1.0);
}

// A pixel's share past the fog (`keep`, the rest's) for a fragment of `kind`.
fn ext_fog_keep(kind: u32, p: vec3<f32>, keep: f32) -> f32 {
    return keep;
}

// Whether a ground plane's highlights fade toward its far edge.
fn ext_plane_glint_fades() -> bool {
    return false;
}

// The share of the ambient a face turned straight down takes from the ground's bounce.
fn ext_ground_bounce() -> f32 {
    return 1.0;
}

// What a surface reflects in the screen-space reflections (the prepass's normal target's z): 0 nothing, else its class.
fn reflect_class(material: u32) -> f32 {
    return 0.0;
}

// Whether the extension draws a face for a water of this material (ext_water_face).
fn ext_water_faced(material: u32) -> bool {
    return false;
}

// The extension's face under a water's waves: its normal, its occlusion and its share of the sun.
struct ExtFace {
    n: vec3<f32>,
    occlusion: f32,
    sun: f32,
};

fn ext_water_face(up: vec3<f32>, p: vec3<f32>, uv: vec2<f32>, sigma_s: vec3<f32>, sigma_t: vec3<f32>, duvx: vec2<f32>, duvy: vec2<f32>) -> ExtFace {
    return ExtFace(up, 1.0, 1.0);
}
