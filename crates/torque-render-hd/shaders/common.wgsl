// The enhanced renderer's shared WGSL: the frame's uniform, the point lights, the material word, colour transfer functions,
// octahedral normals and the sky's SH. Our own code; written from the published techniques named where they are used. Prepended to
// forward.wgsl and clusters.wgsl (with the permutation's constants above it).

// The material word (torque_render_scene::hd::material: a test compares these numbers).
const MAT_GLOW: u32 = 0x400u;
const MAT_UNLIT: u32 = 0x800u;
const MAT_ZERO_NORMAL: u32 = 0x1000u;
const MAT_TEX_ENV_SHIFT: u32 = 13u;
const MAT_KIND_SHIFT: u32 = 16u;
const KIND_PLANE: u32 = 2u;
const KIND_SKY: u32 = 3u;
// A shape, an emitter sprite (coronas, flares), a sky cloud layer.
const KIND_SHAPE: u32 = 1u;
const MAT_EMITTER: u32 = 0x8000u;
const MAT_CLOUD: u32 = 0x40000u;
const MAT_PLAYER: u32 = 0x400000u;
// A shape's first-person image drawn before the eye (material::VIEW_ONLY): it receives no shadow of the client's own Player.
const MAT_VIEW_ONLY: u32 = 0x10000000u;
const MAT_BILLBOARD: u32 = 0x800000u;
// A particle sprite, and a decal drawn with them (it does not fade into its surface).
const MAT_PARTICLE: u32 = 0x1000000u;
const MAT_DECAL: u32 = 0x20000000u;
// With MAT_PARTICLE: a lit sprite (particles.wgsl).
const MAT_LIT: u32 = 0x40000000u;
const MAT_ANCHORED: u32 = 0x2000000u;
const MAT_SKY_EYE: u32 = 0x4000000u;
// A plane's terrain material layer over the layers under it (material::LAYER, LAYER_SHARPNESS and LAYER_DIP: a test compares
// them).
const MAT_LAYER: u32 = 0x40000000u;
const LAYER_SHARPNESS: f32 = 8.0;
const LAYER_DIP: f32 = 1.0;
// A shape's vertex in its mesh's own space (NodePlace): its extra word names its mesh's place in the frame's table.
const MAT_NODE: u32 = 0x80000000u;
// The moving shapes the frame names (MOVING_CASTERS: a test compares them).
const MOVING_CASTERS: u32 = 16u;
// The ground's roughness.
const PLANE_ROUGHNESS: f32 = 0.7;
// The penumbra half width (screen pixels) the sun mask's alpha holds as 1 (contact.wgsl writes it, contact_filter.wgsl reads it).
const PENUMBRA_PIXELS: f32 = 8.0;

// One cluster's light mask: 1,024 bits.
const CLUSTER_WORDS: u32 = 32u;
const MAX_LIGHTS: u32 = 1024u;
const PI: f32 = 3.14159265358979;

struct Frame {
    view_proj: mat4x4<f32>,
    view: mat4x4<f32>,
    inv_proj: mat4x4<f32>,
    // xyz: the eye; w: the effect time.
    eye: vec4<f32>,
    // xyz: towards the sun.
    sun_dir: vec4<f32>,
    // The fixed-function sun: GL light 0's diffuse and ambient (gamma 0..1).
    sun_diffuse: vec4<f32>,
    sun_ambient: vec4<f32>,
    // The sun's linear diffuse strength (the specular's light).
    sun_linear: vec4<f32>,
    // The sky's irradiance shape (L2 SH, RGB).
    sh: array<vec4<f32>, 9>,
    // rgb: the fog's linear colour; w: 1 when fog is on.
    fog_color: vec4<f32>,
    // x: start, y: end; z: 1 for a linear fog (TGE's: no square, no height fog).
    fog: vec4<f32>,
    // Width, height, 1 / width, 1 / height.
    screen: vec4<f32>,
    // Tiles across, tiles down, depth slices, lights.
    clusters: vec4<u32>,
    // Near, far, slices / ln(far / near), unused.
    cluster_z: vec4<f32>,
    // F0, roughness, the SH's luminance weight, its chroma weight.
    surface: vec4<f32>,
    // x: the most lights a cluster shades; y: 1 when the ambient is the sky's irradiance (forward.wgsl's `irradiance`).
    limits: vec4<u32>,
    // Eye to clip space (reversed depth; jittered as view_proj is).
    proj: mat4x4<f32>,
    // The previous frame's world to clip space and this frame's, both without the jitter (the motion vectors), and the latter's
    // inverse (the sky's motion).
    prev_view_proj: mat4x4<f32>,
    unjit_view_proj: mat4x4<f32>,
    inv_unjit_view_proj: mat4x4<f32>,
    // xy: this frame's jitter (internal pixels, y down); z: the frame's index in its history (mod 1024); w: 1 when the TAA's
    // history is valid.
    temporal: vec4<f32>,
    // x: the AO's reach (world units), y: its strength, z: the method (0 off, 1 SSAO, 2 GTAO), w: internal pixels per AO texel.
    ao: vec4<f32>,
    // x: unused (the AO keeps no history); y: 1 in a secondary view (the water's mirrored camera, a capture face: no contact
    // shadows, AO or screen-space reflections); z: 1 when the view is clipped at the water plane, w: the plane's height.
    flags: vec4<f32>,
    // The output's width, height, 1 / width, 1 / height (the TAA's history; `screen` is the internal frame's).
    output: vec4<f32>,
    // The surfaces' materials (the settings' material table). surf0: x the default roughness, y the
    // clear coat's roughness, z F0, w 1 when lit in linear light.
    surf0: vec4<f32>,
    // surf1: x the shapes' roughness, y node-tinted parts', z the silhouette rim, w the refraction (pixels).
    surf1: vec4<f32>,
    // surf2: x the ground's roughness, y 1 for stochastic tiling, z 1 when translucency blends in gamma space (linear_blend off), w the clear coat's
    // share of the sky.
    surf2: vec4<f32>,
    // surf3: x 1 for weighted blended transparency, y 1 for coloured sun shadows, z 1 where no TAA averages a per-pixel pattern
    // (a fade is a cut), w 1 under Low's FXAA-style filter (the shadows' filter turns no pattern).
    surf3: vec4<f32>,
    // x: last frame's effect time (the prepass's previous places of the shape fx's vertices); yzw unused.
    times: vec4<f32>,
    // xyz: the camera's eye less the eye the sky's draws were built around (they follow the eye between syncs); w: how many of
    // `movers` are set.
    sky_shift: vec4<f32>,
    // The water (water::frame_terms): x its height, y 1 when it is drawn as water, z 1 when the eye is under water, w the
    // absorption per unit under water.
    water: vec4<f32>,
    // The tint from under water: rgb its colour (linear), a its alpha.
    under: vec4<f32>,
    // The shapes that moved since the last frame, nearest the eye first: centre and radius, grown to hold both poses.
    movers: array<vec4<f32>, MOVING_CASTERS>,
    // The extension's own block (FrameExt, from the chunk the extension registers), written after the rest.
    ext: FrameExt,
};

// The reflections' uniform (reflect.rs writes it, one a view).
struct Refl {
    // x: 1 when the screen-space reflections are in use (the main view), y: the water's reflection (0 none, 1 the sky cube, 2 SSR over
    // the sky cube, 3 planar), z: 1 when the light capture holds the scene (else the sky cube is the fallback), w: 1 for the debug view's
    // mirror rays (no roughness).
    mode: vec4<f32>,
    // x: the water's height, y: 1 when the eye is above it, z: 1 for clear water (else its texture is its face), w: internal
    // pixels per SSR texel.
    water: vec4<f32>,
    // x: the previous frame's colour mip levels, y: the most Hi-Z steps, z: 1 when last frame's colour and SSR history are valid,
    // w: the effect time (the water's waves).
    ssr: vec4<f32>,
    // x: 1 in a capture face (its alpha holds 2 + the distance of what it drew; the sky leaves 1 or less), y: the share of the sun's
    // and the lights' highlights plain plastic takes (the material table's), z: the cube maps' mip levels, w: 1 when the clouds'
    // cube holds the clouds (clouds.rs).
    capture: vec4<f32>,
    // The light capture's faces (+x, -x, +y, -y, +z, -z): xyz this view's eye less the eye the face was drawn from (zero in every
    // view but the main one).
    capture_shift: array<vec4<f32>, 6>,
    // The extension's own block (ReflExt, from the extension's chunk), written after the rest.
    ext: ReflExt,
};

// A reversed depth as the eye's distance along the view axis (clip w): n f / (n + d (f - n)).
fn linear_depth(d: f32) -> f32 {
    let n = frame.cluster_z.x;
    let f = frame.cluster_z.y;
    return n * f / (n + d * (f - n));
}

// The view-space point at pixel position `px` (continuous, internal pixels) with reversed depth `d`.
fn view_point(px: vec2<f32>, d: f32) -> vec3<f32> {
    let ndc = vec2<f32>(px.x * frame.screen.z * 2.0 - 1.0, 1.0 - px.y * frame.screen.w * 2.0);
    let p = frame.inv_proj * vec4<f32>(ndc, d, 1.0);
    return p.xyz / p.w;
}

// Octahedral encoding (the inverse of oct_decode).
fn oct_encode(n: vec3<f32>) -> vec2<f32> {
    let l1 = abs(n.x) + abs(n.y) + abs(n.z);
    var e = n.xy / max(l1, 1e-20);
    if n.z < 0.0 {
        let s = select(vec2<f32>(-1.0), vec2<f32>(1.0), e >= vec2<f32>(0.0));
        e = (vec2<f32>(1.0) - abs(e.yx)) * s;
    }
    return e;
}

struct Light {
    // xyz: world position; w: radius.
    pos_radius: vec4<f32>,
    // rgb: linear color times brightness; w: unused.
    color: vec4<f32>,
    // A spot's cone: xyz its direction over the soft edge's width in cosines, w the outer half angle's cosine (all zeros: a point
    // light).
    spot: vec4<f32>,
    // Its shadow cubes: x its cube's slot (negative: none) and y how much of the shadow it gives; z, w the cube it is leaving
    // (fading out, or crossfading to x's tier) and that one's share.
    shadow: vec4<f32>,
    // An area light's shape (area.wgsl; w 0: a point or spot light): a rectangle's half sides (a.xyz, b.xyz; a.w 1), lit on the
    // side a x b points to, or a tube's half axis (a.xyz; a.w 2) and its radius (b.w).
    area_a: vec4<f32>,
    area_b: vec4<f32>,
};

// A light's share at the direction `l` toward it: 1 for a point light, a spot's cone with a smooth edge.
fn cone_share(light: Light, l: vec3<f32>) -> f32 {
    let k = length(light.spot.xyz);
    if k == 0.0 {
        return 1.0;
    }
    let x = clamp((dot(-l, light.spot.xyz) / k - light.spot.w) * k, 0.0, 1.0);
    return x * x * (3.0 - 2.0 * x);
}

fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    let x = max(c, vec3<f32>(0.0));
    return select(pow((x + 0.055) / 1.055, vec3<f32>(2.4)), x / 12.92, x <= vec3<f32>(0.04045));
}

fn linear_to_srgb(c: vec3<f32>) -> vec3<f32> {
    let x = max(c, vec3<f32>(0.0));
    return select(1.055 * pow(x, vec3<f32>(1.0 / 2.4)) - 0.055, x * 12.92, x <= vec3<f32>(0.0031308));
}

fn luminance(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
}

// Octahedral normals (Cigolle et al. 2014).
fn oct_decode(e: vec2<f32>) -> vec3<f32> {
    var n = vec3<f32>(e.x, e.y, 1.0 - abs(e.x) - abs(e.y));
    let t = max(-n.z, 0.0);
    n.x = n.x + select(t, -t, n.x >= 0.0);
    n.y = n.y + select(t, -t, n.y >= 0.0);
    return normalize(n);
}

// The nine real SH basis functions (Ramamoorthi and Hanrahan 2001) evaluated against the frame's coefficients.
fn sh_eval(n: vec3<f32>) -> vec3<f32> {
    let x = n.x;
    let y = n.y;
    let z = n.z;
    var c = frame.sh[0].rgb * 0.2820948;
    c = c + frame.sh[1].rgb * (0.4886025 * y);
    c = c + frame.sh[2].rgb * (0.4886025 * z);
    c = c + frame.sh[3].rgb * (0.4886025 * x);
    c = c + frame.sh[4].rgb * (1.0925484 * x * y);
    c = c + frame.sh[5].rgb * (1.0925484 * y * z);
    c = c + frame.sh[6].rgb * (0.3153916 * (3.0 * z * z - 1.0));
    c = c + frame.sh[7].rgb * (1.0925484 * x * z);
    c = c + frame.sh[8].rgb * (0.5462742 * (x * x - y * y));
    return c;
}
