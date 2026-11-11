// The sun's visibility mask: at each pixel of the depth prepass, the sun's cascades looked up as the forward pass does (the point
// from the depth, the face from the prepass's normal; the taps turned per pixel) times the screen-space contact shadows: the small
// shadows the sun's cascades are too coarse for, where a surface meets what it stands on. From the pixel a short ray (0.6 units,
// `shadows.point.w` steps, its start jittered by interleaved gradient noise, the same every frame) is marched toward the sun in view
// space; it is blocked where the depth buffer holds a surface in front of the ray by less than a thickness that is not the pixel's
// own plane (the plane is rebuilt from the neighboring depths, so a flat face never shadows itself however grazing the sun is).
//
// Writes r: the cascades' visibility times the contact shadows' (contact_filter.wgsl smooths it in space; the forward pass shades
// it on the static opaque surfaces), g: the contact shadows' alone (the shapes, which look the cascades up themselves), b: the
// cascades' plain visibility at the pixel's unjittered place (on its plane: the same every frame for a still shadow, where r, drawn
// on the jittered pixel, moves a hard edge by up to a pixel; the anti-aliasing starts a pixel's history again where it changed),
// all in [0, 1]; a: the cascades' filter radius at the pixel in screen pixels over PENUMBRA_PIXELS (the penumbra's half width:
// how far the mask's filter may spread).

@group(0) @binding(0) var<uniform> frame: Frame;
@group(0) @binding(1) var depth: texture_depth_2d;
@group(0) @binding(2) var<uniform> shadows: Shadows;
@group(0) @binding(3) var normals: texture_2d<f32>;
@group(0) @binding(4) var csm: texture_depth_2d_array;
@group(0) @binding(5) var shadow_gather: sampler;

const CONTACT_LENGTH: f32 = 0.6;
const CONTACT_THICKNESS: f32 = 0.1;

@vertex
fn vs_full(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let x = f32((i << 1u) & 2u);
    let y = f32(i & 2u);
    return vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
}

fn size() -> vec2<i32> {
    return vec2<i32>(textureDimensions(depth));
}

// The view-space point at pixel `px` with reversed depth `d`.
fn view_point(px: vec2<f32>, d: f32) -> vec3<f32> {
    let ndc = vec2<f32>(px.x * frame.screen.z * 2.0 - 1.0, 1.0 - px.y * frame.screen.w * 2.0);
    let p = frame.inv_proj * vec4<f32>(ndc, d, 1.0);
    return p.xyz / p.w;
}

fn depth_at(px: vec2<i32>) -> f32 {
    let s = size();
    return textureLoad(depth, clamp(px, vec2<i32>(0), s - 1), 0);
}

fn point_at(px: vec2<i32>) -> vec3<f32> {
    return view_point(vec2<f32>(px) + 0.5, depth_at(px));
}

// The contact shadows' visibility at pixel `px` (view-space point `p`): the march.
fn contact_at(frag: vec2<f32>, px: vec2<i32>, p: vec3<f32>) -> f32 {
    // The pixel's plane from the nearer neighbour on each axis (an edge's far side is not mistaken for the plane).
    let l = point_at(px - vec2<i32>(1, 0));
    let r = point_at(px + vec2<i32>(1, 0));
    let u = point_at(px - vec2<i32>(0, 1));
    let b = point_at(px + vec2<i32>(0, 1));
    let dx = select(p - l, r - p, abs(r.z - p.z) < abs(p.z - l.z));
    let dy = select(p - u, b - p, abs(b.z - p.z) < abs(p.z - u.z));
    var n = cross(dx, dy);
    let nl2 = dot(n, n);
    if nl2 < 1e-20 {
        return 1.0;
    }
    n = n * inverseSqrt(nl2);
    if dot(n, p) > 0.0 {
        n = -n;
    }
    let light = normalize((frame.view * vec4<f32>(frame.sun_dir.xyz, 0.0)).xyz);
    if dot(n, light) <= 0.0 {
        return 1.0;
    }
    let steps = max(u32(shadows.point.w), 1u);
    // The same every frame: the march's result has no noise from frame to frame for a history to average or lag behind.
    let jitter = ign(frag.xy);
    let dist = length(p);
    let start = p + n * (0.002 + 0.0005 * dist);
    let plane_eps = 0.004 + 0.002 * dist;
    var vis = 1.0;
    for (var i = 0u; i < steps; i = i + 1u) {
        let t = (f32(i) + jitter) / f32(steps) * CONTACT_LENGTH;
        let q = start + light * t;
        if q.z >= -frame.cluster_z.x {
            break;
        }
        let clip = frame.proj * vec4<f32>(q, 1.0);
        let ndc = clip.xy / clip.w;
        if any(abs(ndc) > vec2<f32>(1.0)) {
            break;
        }
        let spx = vec2<i32>(vec2<f32>((ndc.x + 1.0) * 0.5, (1.0 - ndc.y) * 0.5) * frame.screen.xy);
        let s = point_at(spx);
        // In front of the ray (nearer the eye: a larger view z) by less than the thickness, and off the pixel's own plane.
        let ahead = s.z - q.z;
        let bias = 0.002 + 0.002 * (-q.z);
        if ahead > bias && ahead < CONTACT_THICKNESS && abs(dot(s - p, n)) > plane_eps {
            let f = t / CONTACT_LENGTH;
            vis = f * f;
            break;
        }
    }
    return vis;
}

// The world point on the plane through `world` (normal `n`) seen at the middle of the unjittered frame's pixel `frag` (the
// jittered pixel's own point where the plane lies all but along the view ray).
fn unjittered(frag: vec2<f32>, world: vec3<f32>, n: vec3<f32>) -> vec3<f32> {
    let ndc = vec2<f32>(frag.x * frame.screen.z * 2.0 - 1.0, 1.0 - frag.y * frame.screen.w * 2.0);
    let far = frame.inv_unjit_view_proj * vec4<f32>(ndc, 1e-6, 1.0);
    let eye = frame.eye.xyz;
    let dir = normalize(far.xyz / far.w - eye);
    let across = dot(dir, n);
    if abs(across) < 1e-4 {
        return world;
    }
    let q = eye + dir * (dot(world - eye, n) / across);
    return select(world, q, distance(q, world) < 0.05 * distance(world, eye));
}

@fragment
fn fs_contact(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let px = vec2<i32>(frag.xy);
    let d = depth_at(px);
    if d <= 0.0 {
        return vec4<f32>(1.0);
    }
    let p = view_point(frag.xy, d);
    var contact = 1.0;
    if shadows.tint.w > 0.0 {
        contact = contact_at(frag.xy, px, p);
    }
    // The cascades at the pixel's world point, through its face's normal (the prepass's), as the forward pass looks them up.
    let n = oct_decode(textureLoad(normals, px, 0).xy);
    let rot = mat3x3<f32>(frame.view[0].xyz, frame.view[1].xyz, frame.view[2].xyz);
    let world = transpose(rot) * (p - frame.view[3].xyz);
    let nl = dot(n, frame.sun_dir.xyz);
    // The taps turned by the pixel's place in a 4 x 4 tile (the 16 turns evenly spaced), so the filter averages the same turns at
    // every pixel of a flat patch (its 5 x 5 tent all 16, its default 3 x 3 box nine of them); not turned under Low's filter (no
    // averaging).
    var turn = (f32(bayer(vec2<u32>(px))) + 0.5) / 16.0 * 6.2831853;
    if frame.surf3.w > 0.5 {
        turn = 0.0;
    }
    var cascade = 1.0;
    var still = 1.0;
    sun_radius = 0.0;
    if nl > 0.0 {
        cascade = sun_shadow_turned(world, n, nl, turn, frame.eye.xyz);
    }
    // The filter's radius in screen pixels: over a pixel's size in world units at the point's depth.
    let pixel = 2.0 * max(-p.z, 1e-3) / (frame.proj[1].y * frame.screen.y);
    let penumbra = sun_radius / pixel;
    if nl > 0.0 {
        still = sun_shadow_plain(unjittered(frag.xy, world, n), n, frame.eye.xyz);
    }
    return vec4<f32>(min(cascade, contact), contact, still, clamp(penumbra / PENUMBRA_PIXELS, 0.0, 1.0));
}
