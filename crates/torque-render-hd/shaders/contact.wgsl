// Screen-space contact shadows: the small shadows the sun's cascades are too coarse for, where a surface meets what it stands
// on. From each pixel of the depth prepass a short ray (0.6 units, `shadows.point.w` steps, its start jittered by interleaved gradient
// noise) is marched toward the sun in view space; it is blocked where the depth buffer holds a surface in front of the ray by less than
// a thickness that is not the pixel's own plane (the plane is rebuilt from the neighbouring depths, so a flat face never shadows
// itself however grazing the sun is). Written as a visibility in [0, 1], faded toward the ray's end.

@group(0) @binding(0) var<uniform> frame: Frame;
@group(0) @binding(1) var depth: texture_depth_2d;
@group(0) @binding(2) var<uniform> shadows: Shadows;

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

@fragment
fn fs_contact(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let px = vec2<i32>(frag.xy);
    let d = depth_at(px);
    if d <= 0.0 {
        return vec4<f32>(1.0);
    }
    let p = view_point(frag.xy, d);
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
        return vec4<f32>(1.0);
    }
    n = n * inverseSqrt(nl2);
    if dot(n, p) > 0.0 {
        n = -n;
    }
    let light = normalize((frame.view * vec4<f32>(frame.sun_dir.xyz, 0.0)).xyz);
    if dot(n, light) <= 0.0 {
        return vec4<f32>(1.0);
    }
    let steps = max(u32(shadows.point.w), 1u);
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
    return vec4<f32>(vis, vis, vis, 1.0);
}
