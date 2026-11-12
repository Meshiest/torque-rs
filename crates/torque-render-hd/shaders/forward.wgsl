// The enhanced renderer's forward pass: every surface into the HDR target, lit per pixel.
//
// - Sun and ambient, the light response (`frame.surf0.w`, the linear_light setting): the linear response (the default) is
//   physically based: Lambert in linear light, albedo x (ambient + N.L sun + the lights), times what the specular lobe leaves
//   (brdf.wgsl: 1 - its directional albedo), and the lobe reflects the environment (a reflective surface) or the ambient as a
//   uniform one (the rest), so under a uniform sky a white surface gives back the sky. The gamma-space response (linear_light off)
//   takes the fixed-function light factor k = ambient + max(N.L, 0) diffuse (gamma space, as GL light 0), applies it to the albedo
//   in gamma space and takes the result to linear, so a sunlit and a shadowed face land on the fixed-function colors; its diffuse
//   is not split with the specular. The ambient is the sky's irradiance (irradiance.wgsl's L2 SH: the sky cube's, at High and up
//   with the light capture's bounce; `frame.limits.y`), else (the fixed-function ambient) the scene's ambient shaped by the sky's
//   SH (its variation only, weighted: the level stays the ambient's).
// - Specular (brdf.wgsl): GGX with Smith's height-correlated visibility and Schlick's Fresnel, F0 0.04 and the material table's
//   roughness, from the sun and every point light, with the multiple-scattering compensation.
// - Point lights: Lambert plus the specular, an inverse-square falloff with a 2-unit core, windowed to the fxLight's radius
//   ((1 - (d/r)^4)^2, as Karis 2013), a spot's cut to its cone with a smooth edge, found through the cluster masks (forward+,
//   Olsson et al. 2012) or, in the brute-force permutation, all of them in order (the gate compares the two).
// - Area lights (area.wgsl): rectangles and tubes by linearly transformed cosines, past the point lights in the buffer and the
//   cluster masks (from `frame.limits.z`), shaded in a loop of their own after the point lights' loop.
// - Fog: from the Sky's fog distance to the visible distance (1 - ((end - d) / (end - start))^2), the horizon ban's colour.
// - Unlit and glow surfaces are their colour.
// - Textures: GL_DECAL mixes in gamma space, as the fixed-function pipeline does; MODULATE and REPLACE in linear.
//
// - Sun shadows: cascaded shadow maps. The cascade is the first whose square holds the point (pushed off its face by a normal
//   offset of a texel or two, more on faces turned from the sun). Each tap gathers the 2 x 2 texels around it and compares each with
//   the receiver's own plane extended to that texel's centre (receiver-plane depth bias, Isidoro 2006, from the face's normal: a flat
//   face never shadows itself, so the constant bias is a fifth of a thousandth of a unit), blended bilinearly; the taps lie on a Vogel
//   disk turned per pixel by interleaved gradient noise (Jimenez 2014). PCSS (Fernando 2005) at High and up: a blocker search over the
//   widest penumbra the sun's angular size allows, then the filter's radius from the blockers' mean depth. Faded out toward the
//   shadow distance. Times the contact shadows (the prepass's screen-space march) on opaque surfaces.
// - The shadow colour (the gamma-space response): a shadow takes away the sun's direct light except the Sun's shadow colour's
//   share of it (k = ambient + N.L diffuse (v + (1 - v) shadow)), so a face turned from the sun keeps its unshadowed colour and a
//   shadowed face facing it is tinted by the shadow colour.
// - Point shadows: each shadowed light's cube (its slot in the light's colour.w, -1 for none) holds the distance to the light over
//   its radius; a lookup compares the receiver's (normal-offset) distance on a small Vogel disk across the cube direction.
// - The debug views (`shadows.config.w`): 1 the sun's visibility (r: in use, g: the cascades', b: the contact shadows'), 2 the first
//   light's shadow visibility (r) and N.L (g): what the shadow gates measure; 3 the ambient occlusion before the settings'
//   share of it (the AO gate's).
//
// - Translucency: weighted blended order-independent transparency (McGuire and Bavoil 2013) behind an exact front layer.
//   A depth pass finds each pixel's nearest translucent fragment; fs_wboit accumulates every other one (premultiplied colour times a
//   depth weight, Rgba16Float, and the revealage's product, R16Float) and wboit.wgsl composites that over the frame; then the front
//   layer is blended exactly over the composite (FRONT_LAYER, depth equal to the front's): its colour is chosen so that the
//   hardware's linear blend over the composite's copy gives the wanted mix, which also lets it refract (the copy read a few pixels
//   off along the view-space normal). Without linear_blend it mixes in gamma space (over a light background a translucent colour
//   stays far more saturated than in linear light); with it in linear light as glass, its reflections not scaled by the alpha.
//
// - Materials: the settings' table (`frame.surf*`) sets each surface's roughness (shapes, node-tinted parts, glass, the
//   ground) and the silhouette rim of the environment; an extension's surfaces (its material bits) are resolved by its hooks
//   (`ext_paint`, `ext_detail`, `ext_decal`, `ext_surface`, `ext_position`: the chunk composed before this file) with its block of
//   the uniform (`frame.ext`). The vertices are moved by the same function in vs_main and vs_depth; the sun's shadows are looked up
//   at the rest position, as the shadow maps hold it. Coloured sun shadows (High and up): the sun's direct light through the
//   transmittance map of the cascade (the translucent surfaces between the sun and the first opaque surface), on opaque receivers.
//
// - The prepass writes, besides the depth, each pixel's motion (this frame's position less the previous frame's, in uv, both
//   without the jitter; a shape's previous position through its motion matrix, group 2) with the previous frame's eye distance (the
//   TAA's and the AO's disocclusion test), and its normal (octahedral) for the AO.
// - Ambient occlusion: the AO pass's result, upsampled to the pixel from the four nearest AO texels weighted bilinearly and by
//   how close their depth is to the pixel's (a texel of another surface gets no weight; none near: the texel nearest in depth's,
//   so a sliver of a surface between the texels keeps its occlusion instead of a light rim along an outline), takes away the
//   settings' share of the ambient (and of a reflective surface's sky reflection); the sun's light only by the scene's share of it
//   (`shadows.colour.w`, 0 by default: a darkening in the sun where surfaces meet).
// - Specular anti-aliasing (Kaplanyan and Hill 2016's normal-variance filtering): GGX's alpha squared is widened by twice the
//   screen-space variance of the normal (its derivatives across the pixel, kernel variance 1/4), at most 0.18, so a normal that turns
//   within a pixel (curved shapes, a metal, a face's rounded edge) gets the blurrier highlight its pixel covers instead of sparkling.
//
// - Reflections (reflect.wgsl): a surface of reflective class 1 is a metal (no diffuse, F0 the paint as shown, the class's
//   roughness from `refl.ext`; with the gamma-space response it also mirrors a key light over the eye), class 2 a half-metallic
//   paint; the extension's `reflect_class` names a surface's class. Both reflect the screen-space reflections (the main view) over
//   the environment (the light capture or the sky cube), weighted by the lobe's directional albedo (brdf.wgsl), the specular
//   occlusion of the AO and the horizon of the face's own normal. The prepass writes their class beside
//   the normal for the SSR pass. Debug view 4: the reflection's radiance on reflecting surfaces (black elsewhere), the reflection
//   gates' picture.
// - Secondary views (CLIP; the water's mirrored camera, the light capture's faces): with `frame.flags.z` everything but the sky
//   below the water plane is cut away.
// - particles.wgsl (appended): the lit particles' shading (fogged here as surfaces are).
// - skyfog.wgsl (appended): the emitters' HDR gains, the lit cloud layers (linear light), and the fog toward the sky drawn behind
//   the pixel (with the height fog setting, times a height fog).
//
// Permutation constants (prepended): CLUSTERED (bool), FRONT_LAYER (bool), CLIP (bool).

@group(0) @binding(0) var<uniform> frame: Frame;
@group(0) @binding(1) var<storage, read> lights: array<Light>;
@group(0) @binding(2) var<storage, read> masks: array<u32>;
@group(0) @binding(3) var background: texture_2d<f32>;
@group(0) @binding(4) var<uniform> shadows: Shadows;
@group(0) @binding(5) var csm: texture_depth_2d_array;
@group(0) @binding(6) var shadow_cmp: sampler_comparison;
@group(0) @binding(7) var shadow_gather: sampler;
// The point lights' cubes in three tiers of resolution (binding 8 the finest; 17 and 18 the half and quarter sizes).
@group(0) @binding(8) var point_maps: texture_depth_cube_array;
@group(0) @binding(9) var contact: texture_2d<f32>;
@group(0) @binding(10) var ao_tex: texture_2d<f32>;
// The nearest translucent fragment's depth (the front layer), the cascades' transmittance and its sampler.
@group(0) @binding(11) var trans_front: texture_depth_2d;
@group(0) @binding(12) var transmittance: texture_2d_array<f32>;
@group(0) @binding(13) var trans_samp: sampler;
// The meshes' places (NodePlace: nine vec4 each, the rows of its placement now, of its normals' turn and of its placement at the
// last frame) that a shape's vertex in its mesh's own space (MAT_NODE) names by its extra word.
@group(0) @binding(16) var<storage, read> nodes: array<vec4<f32>>;
@group(0) @binding(17) var point_maps_half: texture_depth_cube_array;
@group(0) @binding(18) var point_maps_quarter: texture_depth_cube_array;
// A plane lit by the scene's lightmap at its extra word's place instead of the sun, sky and lights (material::LIGHTMAP), and a
// shape or plane cut out where its texture's alpha is under one half (material::CUTOUT). Here rather than in common.wgsl: the
// other modules that take common.wgsl stay as they were.
const MAT_LIGHTMAP: u32 = 0x08000000u;
const MAT_CUTOUT: u32 = 0x20000000u;
// The scene's lightmap (MAT_LIGHTMAP planes), sampled with trans_samp (linear, clamped).
@group(0) @binding(19) var lightmap_tex: texture_2d<f32>;
// The diffuse ambient's L2 SH (irradiance.wgsl's: 0-8 the main view's, 9-17 the sky's alone, a secondary view's).
struct Irradiance {
    sh: array<vec4<f32>, 18>,
};
@group(0) @binding(20) var<uniform> irradiance: Irradiance;
// The area lights' fitted table (torque_render_scene::hd::ltc, area.wgsl): layer 0 the lobe's inverse matrix (a, b, c, d),
// layer 1 its albedo and Schlick part; x the roughness, y sqrt(1 - N.V). Sampled with trans_samp (linear, clamped).
@group(0) @binding(21) var ltc_table: texture_2d_array<f32>;
@group(1) @binding(0) var tex: texture_2d<f32>;
@group(1) @binding(1) var samp: sampler;
// The prepass's motion slots: the matrix taking a shape's current world point to where it was (the identity for everything else, and
// for a shape whose vertices bring their own previous positions), and its object id in id.x (0 for everything static). A shape's
// draw is drawn as instance `slot` of one (its instance index is its slot); slot 0 is the identity, the static cells' and the
// extension's own draws'.
// Each slot is 256 bytes (the reference way binds one slot at its dynamic offset and draws instance 0).
struct Motion {
    m: mat4x4<f32>,
    @size(192) id: vec4<f32>,
};
@group(2) @binding(0) var<storage, read> motions: array<Motion>;

struct VertexIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec2<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) color: vec4<f32>,
    @location(4) material: u32,
    @location(5) extra: u32,
};

struct VertexOut {
    @builtin(position) @invariant clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) color: vec4<f32>,
    @location(4) @interpolate(flat) material: u32,
    @location(5) @interpolate(flat) extra: u32,
    // The vertex before its shape fx moved it (where the shadow maps hold it); a MAT_LIGHTMAP plane's place in the lightmap
    // instead (its extra word's two 16-bit fractions in x and y: such a plane takes no shadow, so its rest is never read).
    @location(6) rest: vec3<f32>,
    // How far a draw that fades with distance has faded out (0 whole, 1 gone; 0 for everything else; an extension's).
    @location(7) lod: f32,
};

// Whether a material word is a plane's lit by the lightmap.
fn lightmapped(material: u32) -> bool {
    return ((material >> MAT_KIND_SHIFT) & 3u) == KIND_PLANE && (material & MAT_LIGHTMAP) != 0u;
}

// Rows `row` to `row + 2` of place `i` times `p`.
fn node_rows(i: u32, row: u32, p: vec4<f32>) -> vec3<f32> {
    let k = 9u * i + row;
    return vec3<f32>(dot(nodes[k], p), dot(nodes[k + 1u], p), dot(nodes[k + 2u], p));
}

// A vertex in the world (a MAT_NODE one where its mesh's place puts it now; the same expression in vs_main and vs_depth).
fn placed(position: vec3<f32>, material: u32, extra: u32) -> vec3<f32> {
    if (material & MAT_NODE) == 0u {
        return position;
    }
    return node_rows(extra, 0u, vec4<f32>(position, 1.0));
}

// The same at the last frame's place (the prepass's motion of each limb).
fn placed_before(position: vec3<f32>, material: u32, extra: u32) -> vec3<f32> {
    if (material & MAT_NODE) == 0u {
        return position;
    }
    return node_rows(extra, 6u, vec4<f32>(position, 1.0));
}

// A vertex's normal in the world (a MAT_NODE one turned by its mesh's place, unit again).
fn placed_normal(n: vec3<f32>, material: u32, extra: u32) -> vec3<f32> {
    if (material & MAT_NODE) == 0u {
        return n;
    }
    let m = node_rows(extra, 3u, vec4<f32>(n, 0.0));
    let l = length(m);
    return select(n, m / l, l > 0.0);
}

// A vertex where its shape fx has it at the effect time (the same expression in vs_main and vs_depth: both invariant).
fn fx_position(position: vec3<f32>, material: u32) -> vec4<f32> {
    return fx_position_at(position, material, frame.eye.w);
}

// The same at time `t` (the prepass's previous place: last frame's effect time, so the waves' own motion is the motion).
fn fx_position_at(position: vec3<f32>, material: u32, t: f32) -> vec4<f32> {
    return ext_position(position, material, t);
}

// A billboard (material::BILLBOARD: the sun's flares) at the frame's eye, facing the frame's camera, dimmed by the camera's facing
// (clamp(dot(towards the sun, forward) * 1.5, 0, 1); a linked flare's size too). An anchored one (a light's corona) at its own
// place, facing the frame's camera, undimmed.
fn billboard_position(v: VertexIn) -> vec4<f32> {
    let right = vec3<f32>(frame.view[0][0], frame.view[1][0], frame.view[2][0]);
    let up = vec3<f32>(frame.view[0][1], frame.view[1][1], frame.view[2][1]);
    if (v.material & MAT_ANCHORED) != 0u {
        let ha = bitcast<f32>(v.extra);
        let xa = v.uv.x * 2.0 - 1.0;
        let za = 1.0 - v.uv.y * 2.0;
        return vec4<f32>(v.position + right * (xa * ha) + up * (za * ha), 1.0);
    }
    let rel = v.position;
    var h = bitcast<f32>(v.extra);
    if h < 0.0 {
        h = -h * billboard_facing(rel);
    }
    let x = v.uv.x * 2.0 - 1.0;
    let z = 1.0 - v.uv.y * 2.0;
    return vec4<f32>(frame.eye.xyz + rel + right * (x * h) + up * (z * h), 1.0);
}

fn billboard_facing(rel: vec3<f32>) -> f32 {
    let fwd = -vec3<f32>(frame.view[0][2], frame.view[1][2], frame.view[2][2]);
    return clamp(dot(normalize(rel), fwd) * 1.5, 0.0, 1.0);
}

@vertex
fn vs_main(v: VertexIn) -> VertexOut {
    var out: VertexOut;
    let at = placed(v.position, v.material, v.extra);
    var p = fx_position(at, v.material);
    var color = v.color;
    // The sky's draws stay around the eye (they were built around a sync's).
    if (v.material & MAT_SKY_EYE) != 0u {
        p = vec4<f32>(p.xyz + frame.sky_shift.xyz, 1.0);
    }
    if (v.material & MAT_BILLBOARD) != 0u {
        p = billboard_position(v);
        if (v.material & MAT_ANCHORED) == 0u {
            color = vec4<f32>(color.rgb * billboard_facing(v.position), color.a);
        }
    }
    out.clip = frame.view_proj * p;
    // An emitter (a corona, a flare, an additive particle) is drawn without the TAA's jitter: with the TAA it is added after it (the
    // late sprites), so a jittered one shimmered a still frame.
    if (v.material & MAT_EMITTER) != 0u {
        out.clip = frame.unjit_view_proj * p;
    }
    out.world = p.xyz;
    out.rest = at;
    out.lod = 0.0;
    out.normal = placed_normal(oct_decode(v.normal), v.material, v.extra);
    out.uv = v.uv;
    out.color = color;
    out.material = v.material;
    out.extra = select(v.extra, 0u, (v.material & MAT_NODE) != 0u);
    if lightmapped(v.material) {
        out.rest = vec3<f32>(vec2<f32>(f32(v.extra & 0xffffu), f32(v.extra >> 16u)) / 65535.0, 0.0);
    }
    return out;
}

// The depth prepass: the same position as vs_main (both invariant: the forward pass's depth test meets the prepass's depth exactly),
// with the unjittered clip positions now and a frame ago, and the normal.
struct PrepassOut {
    @builtin(position) @invariant clip: vec4<f32>,
    @location(0) now: vec4<f32>,
    @location(1) before: vec4<f32>,
    @location(2) normal: vec3<f32>,
    @location(3) world: vec3<f32>,
    @location(4) @interpolate(flat) material: u32,
    @location(5) lod: f32,
    @location(6) @interpolate(flat) id: f32,
};

// The vertex's previous position (location 6) is a second stream: a shape's own last-frame positions (its animation and pose: a
// walking Player's limbs), or the vertex buffer itself again for everything else (moved by the motion matrix alone).
// A MAT_NODE vertex's (extra: its place) previous position is where its mesh's place was at the last frame; its object's motion
// matrix moves only the vertices in the world.
fn depth_out(position: vec3<f32>, normal: vec2<f32>, material: u32, extra: u32, previous: vec3<f32>, motion: Motion) -> PrepassOut {
    var out: PrepassOut;
    let p = fx_position(placed(position, material, extra), material);
    out.clip = frame.view_proj * p;
    out.now = frame.unjit_view_proj * p;
    if (material & MAT_NODE) != 0u {
        out.before = frame.prev_view_proj * fx_position_at(placed_before(previous, material, extra), material, frame.times.x);
    } else {
        out.before = frame.prev_view_proj * (motion.m * fx_position_at(previous, material, frame.times.x));
    }
    out.normal = placed_normal(oct_decode(normal), material, extra);
    out.world = p.xyz;
    out.material = material;
    out.lod = 0.0;
    out.id = motion.id.x;
    return out;
}

// A shape's draw: its motion slot is its instance index.
@vertex
fn vs_depth(
    @builtin(instance_index) slot: u32,
    @location(0) position: vec3<f32>,
    @location(1) normal: vec2<f32>,
    @location(4) material: u32,
    @location(5) extra: u32,
    @location(6) previous: vec3<f32>,
) -> PrepassOut {
    return depth_out(position, normal, material, extra, previous, motions[slot]);
}

// The static cells' draws (still: slot 0; a GPU-culled draw's first instance is its cluster).
@vertex
fn vs_depth_still(
    @location(0) position: vec3<f32>,
    @location(1) normal: vec2<f32>,
    @location(4) material: u32,
    @location(6) previous: vec3<f32>,
) -> PrepassOut {
    return depth_out(position, normal, material, 0u, previous, motions[0]);
}

// Whether a fading draw's pixel drops (interleaved gradient noise of the pixel, turned each frame of the TAA's jitter cycle).
fn lod_drops(frag: vec2<f32>, lod: f32) -> bool {
    if lod <= 0.0 {
        return false;
    }
    // Without the TAA to average the dither, a cut halfway.
    if frame.surf3.z > 0.5 {
        return lod >= 0.5;
    }
    let k = f32(u32(frame.temporal.z) % 8u);
    return ign(floor(frag) + vec2<f32>(k * 5.588238, k * 3.137)) < lod;
}

struct PrepassTargets {
    // xy: the motion in uv (now less before); z: the previous frame's eye distance; w: 1 plus the object's id (1 for everything
    // static; 0 where nothing was drawn: the sky).
    @location(0) motion: vec4<f32>,
    // The world normal, octahedral.
    @location(1) normal: vec4<f32>,
};

@fragment
fn fs_depth(in: PrepassOut) -> PrepassTargets {
    if lod_drops(in.clip.xy, in.lod) {
        discard;
    }
    var out: PrepassTargets;
    let a = in.now.xy / in.now.w;
    let b = in.before.xy / max(in.before.w, 1e-6);
    out.motion = vec4<f32>((a - b) * vec2<f32>(0.5, -0.5), in.before.w, 1.0 + in.id);
    let v = normalize(frame.eye.xyz - in.world);
    var n = normalize(in.normal);
    if (in.material & MAT_ZERO_NORMAL) != 0u || dot(n, n) < 0.5 {
        n = v;
    }
    out.normal = vec4<f32>(oct_encode(n), reflect_class(in.material), 1.0);
    return out;
}

// The ambient occlusion at a pixel of reversed depth `d` (1: none): the AO texels upsampled by depth.
fn ambient_occlusion(frag: vec2<f32>, d: f32) -> f32 {
    if frame.ao.z < 0.5 {
        return 1.0;
    }
    let lin = linear_depth(d);
    let tol = 0.03 * lin + 0.01;
    let dims = vec2<i32>(textureDimensions(ao_tex));
    let s = max(frame.ao.w, 1.0);
    // AO texel t holds internal pixel s t (its top left of each s x s block).
    let u = floor(frag) / s;
    let base = vec2<i32>(floor(u));
    let f = u - floor(u);
    var sum = 0.0;
    var weight = 0.0;
    // The texel nearest the pixel in depth, and how far: the occlusion when no texel lies on the pixel's surface.
    var nearest = 1.0;
    var nearest_gap = 1e30;
    for (var k = 0u; k < 4u; k = k + 1u) {
        let o = vec2<i32>(i32(k & 1u), i32(k >> 1u));
        let q = clamp(base + o, vec2<i32>(0), dims - 1);
        let t = textureLoad(ao_tex, q, 0);
        let bw = select(1.0 - f.x, f.x, o.x == 1) * select(1.0 - f.y, f.y, o.y == 1);
        let gap = abs(t.g - lin);
        let dw = clamp(1.0 - gap / tol, 0.0, 1.0);
        let w = (bw + 1e-3) * dw;
        sum = sum + t.r * w;
        weight = weight + w;
        if t.g > 0.0 && gap < nearest_gap {
            nearest = t.r;
            nearest_gap = gap;
        }
    }
    if weight < 1e-4 {
        return clamp(nearest, 0.0, 1.0);
    }
    return clamp(sum / weight, 0.0, 1.0);
}

// What the debug views show (set by the shading).
var<private> debug_out: vec4<f32> = vec4<f32>(1.0, 1.0, 1.0, 1.0);

// What of the sun's direct light the translucent surfaces between the sun and the point let through (the transmittance map of the
// cascade the last sun_shadow used; white where coloured shadows are off).
fn sun_transmittance() -> vec3<f32> {
    if shadows.colour.x < 0.5 || sun_layer < 0 {
        return vec3<f32>(1.0);
    }
    return textureSampleLevel(transmittance, trans_samp, sun_uv, sun_layer, 0.0).rgb;
}

// The sun's visibility at `p` (normal `n`, N.L `nl` > 0) through the cascades.
fn sun_shadow(p: vec3<f32>, n: vec3<f32>, nl: f32, frag: vec2<f32>) -> f32 {
    return sun_shadow_at(p, n, nl, frag, frame.eye.xyz, frame.surf3.w > 0.5);
}

// A cube slot's face side in texels (its tier's).
fn point_face_size(slot: i32) -> f32 {
    if slot < i32(shadows.tiers.x) {
        return shadows.point.y;
    }
    return select(shadows.tiers.w, shadows.tiers.z, slot < i32(shadows.tiers.y));
}

// One comparison in cube slot `slot` (its tier's array) along `d` at `depth`.
fn point_tap(slot: i32, d: vec3<f32>, depth: f32) -> f32 {
    let first = i32(shadows.tiers.x);
    let second = i32(shadows.tiers.y);
    if slot < first {
        return textureSampleCompareLevel(point_maps, shadow_cmp, d, slot, depth);
    }
    if slot < second {
        return textureSampleCompareLevel(point_maps_half, shadow_cmp, d, slot - first, depth);
    }
    return textureSampleCompareLevel(point_maps_quarter, shadow_cmp, d, slot - second, depth);
}

// A light's shadow visibility through its cubes (`Light::shadow`): each cube's darkness by its share, so a cube fading in or out
// gives part of its shadow and two crossfading cubes blend.
fn light_shadow(light: Light, p: vec3<f32>, n: vec3<f32>, frag: vec2<f32>) -> f32 {
    let s = light.shadow;
    var dark = 0.0;
    if s.x >= 0.0 && s.y > 0.0 {
        dark = dark + s.y * (1.0 - point_shadow(i32(s.x), light, p, n, frag));
    }
    if s.z >= 0.0 && s.w > 0.0 {
        dark = dark + s.w * (1.0 - point_shadow(i32(s.z), light, p, n, frag));
    }
    return clamp(1.0 - dark, 0.0, 1.0);
}

// A point light's shadow visibility at `p` (normal `n`) from its cube in slot `slot`.
fn point_shadow(slot: i32, light: Light, p: vec3<f32>, n: vec3<f32>, frag: vec2<f32>) -> f32 {
    let radius = max(light.pos_radius.w, 1e-4);
    let to = p - light.pos_radius.xyz;
    let dist = length(to);
    if dist < 1e-4 {
        return 1.0;
    }
    // A texel's angle at a face's centre, and its size at the receiver.
    let texel = 2.0 / max(point_face_size(slot), 1.0);
    let w_texel = dist * texel;
    let nl = clamp(-dot(n, to) / dist, 0.0, 1.0);
    let q = to + n * (NORMAL_OFFSET_TEXELS * w_texel);
    let qd = length(q);
    let w = q / qd;
    // Each tap compares the distance at which its direction meets the receiver's plane (exact for a flat face), less a bias of the
    // plane's change over one texel.
    let plane = dot(n, q);
    let tan_t = min(sqrt(max(1.0 - nl * nl, 0.0)) / max(nl, 0.05), 8.0);
    let bias = 0.003 + 0.75 * w_texel * tan_t;
    let helper = select(vec3<f32>(0.0, 0.0, 1.0), vec3<f32>(1.0, 0.0, 0.0), abs(w.z) > 0.9);
    let t1 = normalize(cross(w, helper));
    let t2 = cross(w, t1);
    let taps = max(u32(shadows.point.x), 1u);
    let spread = 1.5 * texel;
    let turn = select(ign(frag + vec2<f32>(17.0, 5.0)) * 6.2831853, 0.0, frame.surf3.w > 0.5);
    var lit = 0.0;
    for (var i = 0u; i < taps; i = i + 1u) {
        let o = vogel(i, taps, turn) * spread;
        let d = normalize(w + t1 * o.x + t2 * o.y);
        let dn = dot(n, d);
        let t = select(qd, clamp(plane / dn, 0.5 * qd, 2.0 * qd), dn < -1e-3);
        lit = lit + point_tap(slot, d, (t - bias) / radius);
    }
    return lit / f32(taps);
}

// The ambient (gamma) for a normal: the scene's ambient times the sky's shape, its luminance and chroma weighted; a hemisphere
// below the horizontal: a face turned down takes the ground's bounce (the extension's share of the ambient straight down), not
// the sky's light.
fn ambient(n: vec3<f32>) -> vec3<f32> {
    let s = max(sh_eval(n), vec3<f32>(0.0));
    let lum = max(luminance(s), 1e-4);
    let sky = clamp(1.0 + frame.surface.z * (lum - 1.0), 0.5, 1.5);
    let down = clamp(-n.z, 0.0, 1.0);
    let level = mix(sky, min(sky, 1.0) * ext_ground_bounce(), down);
    let tint = mix(vec3<f32>(1.0), s / lum, frame.surface.w * (1.0 - down));
    return frame.sun_ambient.rgb * level * tint;
}

// The sky's irradiance over pi at a normal (linear: the ambient's radiance), irradiance.wgsl's (the main view's with the light
// capture's bounce; a secondary view's the sky it sees, the last frame's: those views are drawn before the projection, so the
// first frame's are zero); a face turned down takes the extension's share of a sky drawn below the horizon.
fn sky_irradiance(n: vec3<f32>) -> vec3<f32> {
    let o = select(0u, 9u, frame.flags.y > 0.5);
    var c = irradiance.sh[o].rgb * 0.2820948;
    c = c + irradiance.sh[o + 1u].rgb * (0.4886025 * n.y);
    c = c + irradiance.sh[o + 2u].rgb * (0.4886025 * n.z);
    c = c + irradiance.sh[o + 3u].rgb * (0.4886025 * n.x);
    c = c + irradiance.sh[o + 4u].rgb * (1.0925484 * n.x * n.y);
    c = c + irradiance.sh[o + 5u].rgb * (1.0925484 * n.y * n.z);
    c = c + irradiance.sh[o + 6u].rgb * (0.3153916 * (3.0 * n.z * n.z - 1.0));
    c = c + irradiance.sh[o + 7u].rgb * (1.0925484 * n.x * n.z);
    c = c + irradiance.sh[o + 8u].rgb * (0.5462742 * (n.x * n.x - n.y * n.y));
    // Only where the SH's lower half is the sky as drawn (its first coefficient's w): a ground or the capture's own is the
    // ground's light already.
    let share = select(1.0, ext_ground_bounce(), irradiance.sh[o].w > 0.5);
    let down = clamp(-n.z, 0.0, 1.0);
    return max(c, vec3<f32>(0.0)) * mix(1.0, share, down);
}

// The ambient in linear light: the sky's irradiance where the frame says so (`frame.limits.y`), else the scene's ambient shaped
// by the sky's SH.
fn ambient_linear(n: vec3<f32>) -> vec3<f32> {
    if frame.limits.y != 0u {
        return sky_irradiance(n);
    }
    return srgb_to_linear(ambient(n));
}

// The same in gamma space (the gamma-space response's light factor).
fn ambient_gamma(n: vec3<f32>) -> vec3<f32> {
    if frame.limits.y != 0u {
        return linear_to_srgb(sky_irradiance(n));
    }
    return ambient(n);
}

// A point light's falloff at distance d: inverse square with a 2-unit core, windowed to the radius (exactly 0 beyond it).
fn falloff(d: f32, radius: f32) -> f32 {
    let x = d / max(radius, 1e-4);
    let w = clamp(1.0 - x * x * x * x, 0.0, 1.0);
    return w * w / (1.0 + 0.25 * d * d);
}

fn point_light(i: u32, p: vec3<f32>, n: vec3<f32>, v: vec3<f32>, albedo: vec3<f32>, f0: vec3<f32>, rough: f32, frag: vec2<f32>) -> Lit {
    let light = lights[i];
    let to = light.pos_radius.xyz - p;
    let d = length(to);
    let l = select(n, to / d, d > 1e-6);
    let nl = max(dot(n, l), 0.0);
    let cone = cone_share(light, l);
    var e = light.color.rgb * (nl * cone * falloff(d, light.pos_radius.w));
    let shadowed = (light.shadow.x >= 0.0 && light.shadow.y > 0.0) || (light.shadow.z >= 0.0 && light.shadow.w > 0.0);
    if shadowed && shadows.point.z > 0.0 && nl > 0.0 && cone > 0.0 && d < light.pos_radius.w {
        let vis = light_shadow(light, p, n, frag);
        e = e * vis;
        if i == 0u && shadows.config.w == 2u {
            debug_out = vec4<f32>(vis, nl, 0.0, 1.0);
        }
    }
    var out: Lit;
    out.diffuse = albedo * e;
    out.spec = specular(n, v, l, f0, rough) * e;
    return out;
}

// An area light (area.wgsl): the diffuse from its outline's form factor, the specular from the outline turned by the lobe's fitted
// inverse matrix, windowed to the light's radius about its center as a point light's; a shadowed one through its cube at the
// center (a point light's shadow).
fn area_light(light: Light, p: vec3<f32>, n: vec3<f32>, v: vec3<f32>, albedo: vec3<f32>, f0: vec3<f32>, rough: f32, frag: vec2<f32>) -> Lit {
    var out: Lit;
    out.diffuse = vec3<f32>(0.0);
    out.spec = vec3<f32>(0.0);
    let center = light.pos_radius.xyz;
    let d = distance(center, p);
    let x = d / max(light.pos_radius.w, 1e-4);
    let w = clamp(1.0 - x * x * x * x, 0.0, 1.0);
    if w <= 0.0 {
        return out;
    }
    let nv = max(dot(n, v), 1e-4);
    let uv = area_table_uv(rough, nv);
    let minv = textureSampleLevel(ltc_table, trans_samp, uv, 0, 0.0);
    let moments = textureSampleLevel(ltc_table, trans_samp, uv, 1, 0.0).xy;
    let frame_m = area_frame(n, v);
    let shares = area_shares(light.area_a, light.area_b, center, p, frame_m, area_minv(minv) * frame_m);
    if shares.z <= 0.0 {
        return out;
    }
    // The irradiance pi L F (L the radiance, the color over the area shown) in the point lights' units.
    var e = light.color.rgb * (PI * shares.z * w * w);
    let shadowed = (light.shadow.x >= 0.0 && light.shadow.y > 0.0) || (light.shadow.z >= 0.0 && light.shadow.w > 0.0);
    if shadowed && shadows.point.z > 0.0 && d < light.pos_radius.w {
        e = e * light_shadow(light, p, n, frag);
    }
    out.diffuse = albedo * (e * shares.x);
    // The specular in `specular`'s pi convention (brdf.wgsl: pi x the BRDF times the irradiance): pi L x the lobe's integral
    // over the outline, with F0 and F90 as `specular` takes them.
    let f90 = max(vec3<f32>(1.0 - rough), f0);
    out.spec = e * (shares.y * (f0 * (moments.x - moments.y) + f90 * moments.y));
    return out;
}

fn cluster_of(frag: vec2<f32>, p: vec3<f32>) -> u32 {
    let tiles = frame.clusters.xyz;
    let tx = min(u32(frag.x * frame.screen.z * f32(tiles.x)), tiles.x - 1u);
    let ty = min(u32(frag.y * frame.screen.w * f32(tiles.y)), tiles.y - 1u);
    let depth = max(-(frame.view * vec4<f32>(p, 1.0)).z, frame.cluster_z.x);
    let s = log(depth / frame.cluster_z.x) * frame.cluster_z.z;
    let tz = min(u32(max(s, 0.0)), tiles.z - 1u);
    return (tz * tiles.y + ty) * tiles.x + tx;
}

// A surface as the shading takes it: its linear albedo and alpha, the shading normal (bent by the extension's detail) and the
// face's own (the shadow lookups' receiver plane), the table's roughness and F0, whether it is a metal, its clear coat, the
// detail's occlusion and its share of the sun, the silhouette rim, and whether it shades as glass (its reflection not scaled by the
// alpha).
struct Surf {
    albedo: vec3<f32>,
    alpha: f32,
    n: vec3<f32>,
    ng: vec3<f32>,
    rough: f32,
    f0: vec3<f32>,
    // How much of a metal it is (1 a whole metal; the extension's partly metallic paints between): no diffuse there, and that
    // share of the key.
    metal: f32,
    reflects: bool,
    clearcoat: f32,
    occlusion: f32,
    sun: f32,
    rim: f32,
    linear: bool,
    // How far into a hollow (0 on its face; the extension's detail): no specular, the lights occluded as the sky.
    hollow: f32,
    // The share of the sun's and the lights' highlights it takes (the material table's, else 1).
    spec_level: f32,
};

// The shading's two parts: the diffuse (with the ambient) and everything specular (the sun's and the lights' highlights, the
// reflections, the clear coat, the rim).
struct Lit {
    diffuse: vec3<f32>,
    spec: vec3<f32>,
};

// A metal's key light: its strength over the scene's ambient, its lobe's power and how far over the eye it sits.
const METAL_KEY: f32 = 3.0;
const METAL_KEY_POWER: f32 = 4.0;
const METAL_KEY_LIFT: f32 = 0.5;

// Schlick's Fresnel of a dielectric of reflectance f0 at N.V `nv`.
fn coat_fresnel(f0: f32, nv: f32) -> f32 {
    return fresnel(vec3<f32>(f0), vec3<f32>(1.0), nv).x;
}

// `occlusion`: the ambient occlusion (1 none); `spec_kernel`: the normal's screen-space variance term for GGX's alpha squared; `rest`
// the point before its shape fx (the sun's cascades hold the rest pose).
fn shade_surf(frag: vec2<f32>, p: vec3<f32>, rest: vec3<f32>, s: Surf, material: u32, opaque: bool, occlusion: f32, spec_kernel: f32) -> Lit {
    var out: Lit;
    let v = normalize(frame.eye.xyz - p);
    let n = s.n;
    let nv = max(dot(n, v), 1e-4);
    var rough = s.rough;
    let f0 = s.f0;
    // Specular AA: alpha^2 widened by the normal's variance over the pixel.
    let a2 = rough * rough * rough * rough;
    rough = sqrt(sqrt(clamp(a2 + spec_kernel, 0.0, 1.0)));
    // The settings' share of the occlusion, in linear light (k is a gamma-space factor of the gamma albedo), and the detail's.
    let occl = mix(1.0, occlusion, frame.ao.y) * s.occlusion;
    let occl_gamma = pow(occl, 1.0 / 2.2);
    // The specular lobe's directional albedo with its multiple scattering, and its occlusion. The linear response's diffuse takes
    // what the lobe leaves of the light: a reflective surface's lobe reflects the environment, the others' the ambient (their
    // share of the highlights, as of the sun's).
    let lobe = spec_albedo(f0, rough, nv);
    let spec_occl = spec_occlusion(nv, occl, rough);
    let spec_share = select(s.spec_level, 1.0, s.reflects);
    let kd = select(vec3<f32>(1.0), 1.0 - lobe.e * spec_share, s.linear);
    // The diffuse albedo takes that share and the highlights' scale holds the compensation, so nothing more stays live through
    // the lights' loop (two more vectors live there make the valley's opaque pass a third slower).
    let diffuse = s.albedo * (1.0 - s.metal) * kd;
    let spec_scale = lobe.comp * s.spec_level;
    // The sun (through its shadows: looked up with the face's own normal; the shadow colour's share of it gets through).
    let sl_face = dot(s.ng, frame.sun_dir.xyz);
    let sl = dot(n, frame.sun_dir.xyz);
    var vis = 1.0;
    var tr = vec3<f32>(1.0);
    if sl_face > 0.0 {
        let kind = (material >> MAT_KIND_SHIFT) & 3u;
        sun_no_self = kind == KIND_SHAPE && (material & MAT_VIEW_ONLY) != 0u;
        // The contact pass's mask holds this pixel (an opaque surface of the main view): r the cascades times the contact shadows,
        // filtered in space, g the contact shadows alone.
        let masked = opaque && shadows.tint.w > 0.0 && frame.flags.y < 0.5;
        var cascade = 1.0;
        var near = 1.0;
        if masked && kind != KIND_SHAPE {
            // A static surface takes the mask's visibility (a shape's point and normal are its own: the rest pose, no shadow of
            // the client's own Player on a first-person image); the cascade is still picked for the transmittance's lookup, and
            // the debug view shows the cascades' own value.
            vis = textureLoad(contact, vec2<i32>(frag), 0).r;
            if shadows.colour.x > 0.5 {
                _ = sun_cascade(rest, s.ng, frame.eye.xyz);
            }
            if shadows.config.w == 1u {
                cascade = sun_shadow(rest, s.ng, sl_face, frag);
                near = textureLoad(contact, vec2<i32>(frag), 0).g;
            }
        } else {
            cascade = sun_shadow(rest, s.ng, sl_face, frag);
            if masked {
                near = textureLoad(contact, vec2<i32>(frag), 0).g;
            }
            vis = min(cascade, near);
        }
        if shadows.config.w == 1u {
            debug_out = vec4<f32>(vis, cascade, near, 1.0);
        }
        if opaque {
            tr = sun_transmittance();
        }
    }
    vis = vis * s.sun;
    // The scene's share of the ambient occlusion on the sun's light (none by default).
    if shadows.colour.w > 0.0 {
        vis = vis * mix(1.0, mix(1.0, occlusion, frame.ao.y), shadows.colour.w);
    }
    // The volumetric clouds' shadows on the ground (Ultra; cloudshadow.wgsl); under a low sun, wherever clouds are drawn, the
    // sun as the eye sees it through them (its light and its glint on every surface dim as a cloud covers it).
    vis = vis * cloud_sun(rest, sky.cloud_use.x);
    let sun_on = select(0.0, 1.0, sl_face > 0.0);
    if s.linear {
        // The linear response: albedo x (the ambient + N.L the sun) with the shadow color's share as light (the sun's linear
        // strength is what a face toward it has over the ambient in the gamma-space response), less the lobe's share; the lobe
        // reflects the ambient where it reflects no environment.
        let tint = srgb_to_linear(shadows.tint.rgb);
        let through = vis * tr + (1.0 - vis) * tint;
        let amb = ambient_linear(n);
        let sun = frame.sun_linear.rgb * (max(sl, 0.0) * sun_on);
        out.diffuse = diffuse * (amb * occl + sun * through);
        if !s.reflects {
            out.spec = amb * lobe.e * (spec_share * spec_occl);
        }
    } else {
        let through = vis * tr + (1.0 - vis) * shadows.tint.rgb;
        let k = ambient_gamma(n) * occl_gamma + max(sl, 0.0) * sun_on * frame.sun_diffuse.rgb * through;
        out.diffuse = srgb_to_linear(linear_to_srgb(diffuse) * k);
    }
    if sl > 0.0 && sl_face > 0.0 {
        out.spec = out.spec + specular(n, v, frame.sun_dir.xyz, f0, rough) * frame.sun_linear.rgb * (sl * vis) * tr * spec_scale;
    }
    if s.reflects {
        // The reflection (screen space over the environment) by the lobe's directional albedo, its occlusion and the horizon.
        let r = reflect(-v, n);
        let env = reflection(frag, r, rough);
        out.spec = out.spec + env * lobe.e * (spec_occl * horizon_fade(r, s.ng));
        if s.metal > 0.0 && !s.linear {
            // The key (the gamma-space response's): a soft light behind and over the eye that the metal mirrors where it faces
            // the viewer, of the scene's ambient light. In a plain room the camera-local light capture is the walls' even gray,
            // and an even mirror reads as plastic; this gives a metal a bright face and darker turns, indoors and out.
            let key_dir = normalize(v + vec3<f32>(0.0, 0.0, METAL_KEY_LIFT));
            let key = pow(max(dot(r, key_dir), 0.0), METAL_KEY_POWER) * METAL_KEY * s.metal;
            out.spec = out.spec + key * srgb_to_linear(frame.sun_ambient.rgb) * env_brdf(f0, rough, nv) * occl;
        }
        if shadows.config.w == 4u {
            debug_out = vec4<f32>(env, 1.0);
        }
    }
    if s.clearcoat > 0.0 {
        // The clear coat: a smooth dielectric layer over the surface (its own GGX lobe on the face's normal and its share of the
        // environment), the light under it reduced by what it reflects.
        let ng = s.ng;
        let nvc = max(dot(ng, v), 1e-4);
        // Its lobe widened by the normal's variance over the pixel as the base's is (a sharp highlight on small facets shimmered
        // while the camera moved).
        let cr = sqrt(sqrt(clamp(pow(frame.surf0.y, 4.0) + spec_kernel, 0.0, 1.0)));
        let fc = s.clearcoat * coat_fresnel(0.04, nvc);
        out.diffuse = out.diffuse * (1.0 - fc);
        out.spec = out.spec * (1.0 - fc);
        var coat = env_radiance(reflect(-v, ng), cr) * env_brdf(vec3<f32>(0.04), cr, nvc) * occl * frame.surf2.w;
        if sl_face > 0.0 {
            coat = coat + specular(ng, v, frame.sun_dir.xyz, vec3<f32>(0.04), cr) * frame.sun_linear.rgb * (sl_face * vis) * tr;
        }
        out.spec = out.spec + s.clearcoat * coat;
    }
    if s.rim > 0.0 {
        // The silhouette rim: the environment's light toward the silhouette.
        let rim = pow(1.0 - nv, 3.0) * s.rim;
        out.spec = out.spec + rim * env_radiance(n, 0.7) * occl;
    }
    // The point and spot lights (the area lights follow them in the buffer).
    let count = min(frame.limits.z, MAX_LIGHTS);
    // A hollow underside's share of a light, as of the sky.
    let hollow_light = mix(1.0, s.occlusion, s.hollow);
    if CLUSTERED {
        let base = cluster_of(frag, p) * CLUSTER_WORDS;
        var shaded = 0u;
        for (var w = 0u; w < CLUSTER_WORDS; w = w + 1u) {
            var bits = masks[base + w];
            loop {
                if bits == 0u || shaded >= frame.limits.x {
                    break;
                }
                let b = firstTrailingBit(bits);
                bits = bits & (bits - 1u);
                let i = w * 32u + b;
                if i < count {
                    let l = point_light(i, p, n, v, diffuse, f0, rough, frag);
                    out.diffuse = out.diffuse + l.diffuse * hollow_light;
                    out.spec = out.spec + l.spec * spec_scale;
                    shaded = shaded + 1u;
                }
            }
        }
    } else {
        for (var i = 0u; i < count; i = i + 1u) {
            let l = point_light(i, p, n, v, diffuse, f0, rough, frag);
            out.diffuse = out.diffuse + l.diffuse * hollow_light;
            out.spec = out.spec + l.spec * spec_scale;
        }
    }
    // The area lights, past the point lights in the buffer (from `frame.limits.z` to the end): a loop of their own.
    let total = min(frame.clusters.w, MAX_LIGHTS);
    if count < total {
        if CLUSTERED {
            let base = cluster_of(frag, p) * CLUSTER_WORDS;
            var shaded = 0u;
            for (var w = count / 32u; w < CLUSTER_WORDS; w = w + 1u) {
                var bits = masks[base + w];
                if w == count / 32u {
                    // The point lights' bits in the first word.
                    bits = bits & ~((1u << (count % 32u)) - 1u);
                }
                loop {
                    if bits == 0u || shaded >= frame.limits.x {
                        break;
                    }
                    let b = firstTrailingBit(bits);
                    bits = bits & (bits - 1u);
                    let i = w * 32u + b;
                    if i < total {
                        let l = area_light(lights[i], p, n, v, diffuse, f0, rough, frag);
                        out.diffuse = out.diffuse + l.diffuse * hollow_light;
                        out.spec = out.spec + l.spec * spec_scale;
                        shaded = shaded + 1u;
                    }
                }
            }
        } else {
            for (var i = count; i < total; i = i + 1u) {
                let l = area_light(lights[i], p, n, v, diffuse, f0, rough, frag);
                out.diffuse = out.diffuse + l.diffuse * hollow_light;
                out.spec = out.spec + l.spec * spec_scale;
            }
        }
    }
    // Nothing shines in a hollow.
    out.spec = out.spec * (1.0 - s.hollow);
    return out;
}

// The shading of a surface of the gamma-space response with the material word's roughness (the water's calls).
fn shade(frag: vec2<f32>, p: vec3<f32>, n_in: vec3<f32>, albedo: vec3<f32>, material: u32, opaque: bool, occlusion: f32, spec_kernel: f32) -> vec3<f32> {
    var s: Surf;
    let v = normalize(frame.eye.xyz - p);
    var n = normalize(n_in);
    if (material & MAT_ZERO_NORMAL) != 0u {
        n = v;
    }
    s.albedo = albedo;
    s.alpha = 1.0;
    s.n = n;
    s.ng = n;
    s.rough = select(frame.surf0.x, frame.surf2.x, ((material >> MAT_KIND_SHIFT) & 3u) == KIND_PLANE);
    s.f0 = vec3<f32>(frame.surf0.z);
    s.metal = 0.0;
    s.reflects = false;
    s.clearcoat = 0.0;
    s.occlusion = 1.0;
    s.sun = 1.0;
    s.rim = 0.0;
    s.linear = false;
    s.hollow = 0.0;
    s.spec_level = 1.0;
    let l = shade_surf(frag, p, p, s, material, opaque, occlusion, spec_kernel);
    return l.diffuse + l.spec;
}

// What a fragment resolves to before it is lit: the surface, the colour unlit surfaces show, and whether it is lit.
struct Resolved {
    s: Surf,
    unlit: vec3<f32>,
    lit: bool,
    alpha: f32,
};

// The derivatives a fragment's material needs (taken in uniform control flow, before any branch).
struct Derivs {
    dpx: vec3<f32>,
    dpy: vec3<f32>,
    duvx: vec2<f32>,
    duvy: vec2<f32>,
};

// The material of a fragment: its paint (the extension's colour effects: ext_paint), the texture's share (the fixed-function
// texture environments; the extension's decal mix), the extension's detail, the table's roughness, F0 and rim (kind 0, the
// extension's surfaces: ext_surface).
fn resolve(in: VertexOut, t_in: vec4<f32>, d: Derivs) -> Resolved {
    var r: Resolved;
    let material = in.material;
    let kind = (material >> MAT_KIND_SHIFT) & 3u;
    let env = (material >> MAT_TEX_ENV_SHIFT) & 3u;
    let v = normalize(frame.eye.xyz - in.world);
    var ng = normalize(in.normal);
    if (material & MAT_ZERO_NORMAL) != 0u || dot(in.normal, in.normal) < 1e-8 {
        ng = v;
    }
    var n = ng;
    var paint = in.color;
    // The extension's paint (its colour effects).
    paint = ext_paint(in, paint, ng, v);
    var t = t_in;
    let uv = in.uv;
    let fr = uv_frame(d.dpx, d.dpy, d.duvx, d.duvy);
    var occlusion = 1.0;
    var sun = 1.0;
    let lit = (material & (MAT_UNLIT | MAT_GLOW)) == 0u;
    if lit && env != 0u {
        // A plane's texture tiled stochastically, where the extension leaves the plane its texture (the terrain's layers too,
        // and varied by a large-scale tint, where the scene asks for its ground varied: surf2.y 2).
        let varied = frame.surf2.y > 1.5;
        if kind == KIND_PLANE && ext_plane_tiles(material) && (((material & MAT_LAYER) == 0u && frame.surf2.y > 0.5) || varied) {
            t = stochastic_sample(uv, d.duvx, d.duvy);
        }
        if kind == KIND_PLANE && varied {
            t = vec4<f32>(t.rgb * ground_tint(in.world.xy), t.a);
        }
    }
    // The extension's detail (its normals, occlusion, sun and hollow).
    let x = ext_detail(in, d, fr, t, n, ng, lit);
    t = x.t;
    n = x.n;
    ng = x.ng;
    occlusion = x.occlusion;
    sun = x.sun;
    let hollow = x.hollow;
    let traced = x.traced;
    var base = vec4<f32>(srgb_to_linear(paint.rgb), paint.a);
    if env == 1u {
        // GL_DECAL in gamma space (a mix in linear would wash a texture's baked shading over dark paints), then to linear; the
        // extension's share of it over the paint.
        var mixed = mix(paint.rgb, linear_to_srgb(t.rgb), t.a);
        mixed = ext_decal(material, paint.rgb, mixed, traced);
        base = vec4<f32>(srgb_to_linear(mixed), base.a);
    } else if env == 2u {
        base = base * t;
    } else if env == 3u {
        base = t;
    }
    if kind == KIND_PLANE && (material & MAT_LAYER) != 0u {
        // A terrain layer: its share at the samples sharpened about one half, lowered by the texture's dips.
        base.a = clamp((in.color.a - 0.5) * LAYER_SHARPNESS + 0.5 - (1.0 - t.a) * LAYER_DIP, 0.0, 1.0);
    }
    r.alpha = base.a;
    r.unlit = base.rgb;
    r.lit = lit;
    var s: Surf;
    s.albedo = base.rgb;
    s.alpha = base.a;
    s.n = n;
    s.ng = ng;
    s.f0 = vec3<f32>(frame.surf0.z);
    s.rough = frame.surf0.x;
    s.metal = 0.0;
    s.reflects = false;
    s.clearcoat = 0.0;
    s.occlusion = occlusion;
    s.sun = sun;
    s.rim = 0.0;
    s.linear = frame.surf0.w > 0.5;
    s.hollow = hollow;
    s.spec_level = 1.0;
    let translucent = base.a < 0.999;
    if kind == KIND_PLANE {
        s.rough = frame.surf2.x;
        s.spec_level = refl.capture.y;
    } else if kind == KIND_SHAPE {
        // Node-tinted parts (the vertex colour, or a decal over it): the table's roughness for them.
        let coloured = any(in.color.rgb < vec3<f32>(0.995)) || env == 1u;
        s.rough = select(frame.surf1.x, frame.surf1.y, coloured);
        if (material & MAT_PLAYER) != 0u {
            s.rim = frame.surf1.z;
        }
    } else if kind == 0u {
        ext_surface(&s, material, base, hollow, translucent);
    }
    r.s = s;
    return r;
}

// A fragment shaded (lit or unlit) and fogged: its colour's diffuse and specular parts (unlit: all diffuse) and its alpha.
struct Shaded {
    diffuse: vec3<f32>,
    spec: vec3<f32>,
    alpha: f32,
    // The bent normal (the front layer's refraction).
    n: vec3<f32>,
    debug: bool,
};

fn shaded(in: VertexOut, d: Derivs, t: vec4<f32>, spec_kernel: f32) -> Shaded {
    var out: Shaded;
    let material = in.material;
    let kind = (material >> MAT_KIND_SHIFT) & 3u;
    if shadows.config.w == 4u || shadows.config.w == 5u || shadows.config.w == 6u {
        debug_out = vec4<f32>(0.0);
    }
    let r = resolve(in, t, d);
    out.alpha = r.alpha;
    out.n = r.s.n;
    if !r.lit {
        // The emitters in HDR (skyfog.wgsl), and the lit cloud layers (linear light).
        out.diffuse = r.unlit * emissive_gain(material);
        if (material & MAT_CLOUD) != 0u && linear_light() {
            let cloud = lit_cloud(in.world, vec4<f32>(r.unlit, r.alpha));
            out.diffuse = cloud.rgb;
            out.alpha = cloud.a;
        }
        if lit_particle(material) {
            out.diffuse = particle_lit(in, r.unlit);
        }
    } else {
        let occlusion = ambient_occlusion(in.clip.xy, in.clip.z);
        // A terrain layer is shaded as the opaque ground under it is.
        let layer = kind == KIND_PLANE && (material & MAT_LAYER) != 0u;
        if lightmapped(material) {
            // Baked light: the surface's color times the lightmap's linear light (no sun, sky, lights or highlights).
            out.diffuse = r.s.albedo * textureSampleLevel(lightmap_tex, trans_samp, in.rest.xy, 0.0).rgb;
        } else {
            let l = shade_surf(in.clip.xy, in.world, in.rest, r.s, material, r.alpha >= 0.999 || layer, occlusion, spec_kernel);
            out.diffuse = l.diffuse;
            out.spec = l.spec;
        }
        if shadows.config.w == 3u {
            debug_out = vec4<f32>(occlusion, occlusion, occlusion, 1.0);
        }
    }
    out.debug = shadows.config.w != 0u && frame.flags.y < 0.5;
    if (kind != KIND_SKY || lit_particle(material)) && frame.fog_color.w > 0.0 {
        var keep = fog_keep(in.world);
        keep = ext_fog_keep(kind, in.world, keep);
        out.diffuse = mix(fog_color_toward(in.clip.xy, in.world, keep), out.diffuse, keep);
        out.spec = out.spec * keep;
        if kind == KIND_PLANE && ext_plane_glint_fades() {
            out.spec = out.spec * plane_glint_fade(in.world);
        }
    }
    if kind != KIND_SKY {
        let over = clouds_over_share(in.world);
        if over > 0.0 {
            let c = clouds_at(in.clip.xy);
            let t = mix(1.0, clamp(c.a, 0.0, 1.0), over);
            out.diffuse = out.diffuse * t + max(c.rgb, vec3<f32>(0.0)) * over;
            out.spec = out.spec * t;
        }
    }
    if kind != KIND_SKY && frame.water.z > 0.5 {
        let keep = under_water_keep(in.world);
        out.diffuse = mix(frame.under.rgb, out.diffuse, keep);
        out.spec = out.spec * keep;
    }
    return out;
}

// A ground plane (where the extension says: ext_plane_glint_fades): its highlights fade out over the last PLANE_GLINT_FADE radians of the view down to the ground's far edge
// (the visible distance). Everything past a few hundred units lies in a few rows under that edge, so a low sun's glint, faded by the
// fog alone, stayed bright up to the edge and was cut there by the sky.
const PLANE_GLINT_FADE: f32 = 0.03;
fn plane_glint_fade(p: vec3<f32>) -> f32 {
    let h = max(frame.eye.z - p.z, 0.0);
    let d = max(distance(p.xy, frame.eye.xy), 1e-3);
    let below_edge = h / d - h / max(frame.fog.y, 1e-3);
    return smoothstep(0.0, PLANE_GLINT_FADE, below_edge);
}

// The share of a surface's light the water leaves on the way to an eye under it (water::frame_terms): absorbed toward the
// underwater tint's colour along the part of the sight line below the surface.
fn under_water_keep(p: vec3<f32>) -> f32 {
    let e = frame.eye.xyz;
    let h = frame.water.x;
    let d = distance(p, e);
    var inside = d;
    if p.z > h && e.z < h {
        inside = d * (h - e.z) / max(p.z - e.z, 1e-6);
    }
    return exp(-frame.water.w * inside);
}

// The derivatives of a fragment (uniform control flow).
fn derivs(in: VertexOut) -> Derivs {
    var d: Derivs;
    d.dpx = dpdx(in.world);
    d.dpy = dpdy(in.world);
    d.duvx = dpdx(in.uv);
    d.duvy = dpdy(in.uv);
    return d;
}

// The front layer's (and a translucent fragment's) colour premultiplied by its coverage in the blending space: gamma, the
// gamma-space colour times the alpha; linear, the diffuse times the alpha plus the glass's reflections whole.
fn premultiplied(s: Shaded) -> vec3<f32> {
    if frame.surf2.z > 0.5 {
        return linear_to_srgb(s.diffuse + s.spec) * s.alpha;
    }
    return s.diffuse * s.alpha + s.spec;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    return forward_color(in);
}

// An additive particle (the late pass's sprites) fades into the surface behind it as the blended ones do.
@fragment
fn fs_particle(in: VertexOut) -> @location(0) vec4<f32> {
    let c = forward_color(in);
    let fade = soft_particle(in);
    return vec4<f32>(c.rgb * fade, c.a * fade);
}

// A multiplied decal (Blend::Modulate2x, DST_COLOR and SRC_COLOR: twice this times the frame). Unlit: the surface under it is
// already lit, shadowed and fogged. Its texture times its color in gamma space, as the fixed-function blend sees it, mixed
// toward the neutral 0.5 by its coverage and by the fog and water in front of it (they keep their color); that gamma factor
// on the frame's linear light is about its 2.2 power.
@fragment
fn fs_modulate2x(in: VertexOut) -> @location(0) vec4<f32> {
    let t = textureSample(tex, samp, in.uv);
    let kind = (in.material >> MAT_KIND_SHIFT) & 3u;
    if CLIP && frame.flags.z > 0.5 && kind != KIND_SKY && in.world.z < frame.flags.w {
        discard;
    }
    if shadows.config.w != 0u && frame.flags.y < 0.5 {
        return vec4<f32>(0.5, 0.5, 0.5, 1.0);
    }
    var keep = clamp(t.a * in.color.a, 0.0, 1.0);
    if frame.fog_color.w > 0.0 {
        keep = keep * fog_keep(in.world);
    }
    if frame.water.z > 0.5 {
        keep = keep * under_water_keep(in.world);
    }
    let factor = mix(vec3<f32>(1.0), 2.0 * linear_to_srgb(t.rgb) * in.color.rgb, keep);
    return vec4<f32>(0.5 * pow(max(factor, vec3<f32>(0.0)), vec3<f32>(2.2)), 1.0);
}

fn forward_color(in: VertexOut) -> vec4<f32> {
    let material = in.material;
    // The normal's change across the pixel (in uniform control flow, before any branch): the specular AA's kernel (variance 1/4,
    // twice it, at most 0.18).
    let dndx = dpdx(in.normal);
    let dndy = dpdy(in.normal);
    let spec_kernel = min(2.0 * 0.25 * (dot(dndx, dndx) + dot(dndy, dndy)), 0.18);
    let d = derivs(in);
    let t = textureSample(tex, samp, in.uv);
    let kind = (material >> MAT_KIND_SHIFT) & 3u;
    if CLIP && frame.flags.z > 0.5 && kind != KIND_SKY && in.world.z < frame.flags.w {
        discard;
    }
    if lod_drops(in.clip.xy, in.lod) {
        discard;
    }
    if (kind == KIND_PLANE || kind == KIND_SHAPE) && (material & MAT_CUTOUT) != 0u && t.a * in.color.a < 0.5 {
        discard;
    }
    let s = shaded(in, d, t, spec_kernel);
    if s.debug {
        return debug_out;
    }
    let a = s.alpha;
    if FRONT_LAYER && a > 1e-3 && a < 0.999 {
        // The front translucent layer over the composite's copy: the colour that the hardware's blend turns into the wanted mix
        // over the copy read along the refraction's offset.
        let px = vec2<i32>(in.clip.xy);
        let dims = vec2<i32>(textureDimensions(background));
        let nv = (frame.view * vec4<f32>(s.n, 0.0)).xy;
        let off = vec2<i32>(round(nv * vec2<f32>(1.0, -1.0) * frame.surf1.w * (1.0 - a)));
        let bg = textureLoad(background, px, 0).rgb;
        let bgr = textureLoad(background, clamp(px + off, vec2<i32>(0), dims - 1), 0).rgb;
        var wanted: vec3<f32>;
        if frame.surf2.z > 0.5 {
            wanted = srgb_to_linear(premultiplied(s) + linear_to_srgb(bgr) * (1.0 - a));
        } else {
            wanted = premultiplied(s) + bgr * (1.0 - a);
        }
        return vec4<f32>((wanted - bg * (1.0 - a)) / a, a);
    }
    if CLIP && refl.capture.x > 0.5 && kind == KIND_PLANE && (material & MAT_LAYER) != 0u {
        // A light capture's alpha is its distance: a terrain layer is left out of it.
        discard;
    }
    if CLIP && refl.capture.x > 0.5 && kind != KIND_SKY {
        // A light capture's face: the distance of what it shows (its lookups from another eye, reflect.wgsl's capture_dir).
        return vec4<f32>(s.diffuse + s.spec, 2.0 + distance(in.world, frame.eye.xyz));
    }
    return vec4<f32>(s.diffuse + s.spec, a);
}

// The weighted blended transparency's targets: the premultiplied colour and coverage times the weight (added), and the revealage
// (multiplied by 1 - alpha).
struct WboitOut {
    @location(0) accum: vec4<f32>,
    @location(1) reveal: vec4<f32>,
};

// How much of a particle's sprite shows at this pixel: it fades out over SOFT_PARTICLE units in front of the surface behind it
// (the front layer's depth: the opaque frame, or the nearest translucent surface) instead of a hard cut along every surface it
// meets; seen behind a translucent front layer it shows whole. Our own form of the soft-particle depth fade. A decal lies on
// its surface and shows whole.
const SOFT_PARTICLE: f32 = 0.4;
fn soft_particle(in: VertexOut) -> f32 {
    if (in.material & MAT_DECAL) != 0u {
        return 1.0;
    }
    let front = textureLoad(trans_front, vec2<i32>(in.clip.xy), 0);
    if in.clip.z < front {
        return 1.0;
    }
    return clamp((linear_depth(front) - linear_depth(in.clip.z)) / SOFT_PARTICLE, 0.0, 1.0);
}

// A translucent fragment behind the front layer into the weighted blend (McGuire and Bavoil 2013's weight, eq. 7's shape scaled to
// the world's distances and to the Rgba16Float range).
@fragment
fn fs_wboit(in: VertexOut) -> WboitOut {
    let dndx = dpdx(in.normal);
    let dndy = dpdy(in.normal);
    let spec_kernel = min(2.0 * 0.25 * (dot(dndx, dndx) + dot(dndy, dndy)), 0.18);
    let d = derivs(in);
    let t = textureSample(tex, samp, in.uv);
    let s = shaded(in, d, t, spec_kernel);
    // The front layer is blended exactly after the composite: not here. A particle is never the front layer (it is weighted
    // wherever it is) and fades into what is behind it.
    let particle = (in.material & MAT_PARTICLE) != 0u;
    let front = textureLoad(trans_front, vec2<i32>(in.clip.xy), 0);
    var fade = 1.0;
    if particle {
        fade = soft_particle(in);
    }
    if (!particle && in.clip.z >= front) || s.alpha * fade <= 1e-3 {
        discard;
    }
    let a = clamp(s.alpha * fade, 0.0, 1.0);
    let z = linear_depth(in.clip.z);
    let w = a * clamp(80.0 / (1.0 + pow(z / 8.0, 3.0)), 0.01, 100.0);
    var out: WboitOut;
    out.accum = vec4<f32>(premultiplied(s) * fade * w, a * w);
    out.reveal = vec4<f32>(a, 0.0, 0.0, 0.0);
    return out;
}
