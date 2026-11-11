// The sun's visibility mask's spatial filter, after the contact pass at the frame's size: each pixel's visibility becomes the
// average of the pixels about it, weighted by a tent (FILTER_KIND picks its shape) and by how near each one's point lies to the
// pixel's plane (its point and normal from the prepass) and how alike their normals are: a cross-bilateral filter (Tomasi and
// Manduchi 1998; Petschnigg et al. 2004) that keeps a shadow off other surfaces and across creases and, on a flat patch,
// averages the cascades' taps' pattern (turned per pixel over a 4 x 4 tile, fixed from frame to frame: no history averages it;
// the 5 x 5 tent averages all 16 turns, the default 3 x 3 box nine) into a smooth penumbra. The forward pass shades the static
// opaque surfaces with it, and the anti-aliasing reads this frame's and the last's.
//
// The AO pass's layout: binding 3 (the motion) is not read. Writes r: the filtered visibility, g as the contact pass wrote it, b:
// how much of the tent the filter kept (1 on a flat patch, less where neighbors lay off the plane or across a crease: there the
// mask may step, and the anti-aliasing's change test leaves such pixels alone), a: the contact pass's unjittered visibility (its
// b) filtered alike (what that test compares).

@group(0) @binding(0) var<uniform> frame: Frame;
@group(0) @binding(1) var depth: texture_depth_2d;
@group(0) @binding(2) var normals: texture_2d<f32>;
// This frame's mask (the contact pass's output).
@group(0) @binding(4) var mask_in: texture_2d<f32>;

const RADIUS: i32 = 2;
// The plane's tolerance (a share of the distance, units), the normals' cosine below which a neighbor has no weight and the one
// above which it has all.
const PLANE_TOL: vec2<f32> = vec2<f32>(0.01, 0.004);
const NORMAL_COS: vec2<f32> = vec2<f32>(0.8, 0.95);
// The tent's shape: 0 the 5 x 5 tent (1/2, 1, 1, 1, 1/2 along each axis: every turn of the 4 x 4 tile alike), 1 (the default,
// chosen from pictures) the 3 x 3 box (nine of the sixteen turns), 2 a tent whose radius is PENUMBRA_SHARE of the penumbra's
// half width at the pixel (the contact pass's a, PENUMBRA_PIXELS as 1; a sharp shadow is left as it is, a wide penumbra
// smoothed), 3 the 5 x 5 tent weighted by the mask's own difference too (a Gaussian of RANGE_SIGMA: the taps' pattern is
// smoothed, a step across the edge kept).
const FILTER_KIND: u32 = 1u;
const PENUMBRA_SHARE: f32 = 0.75;
const RANGE_SIGMA: f32 = 0.15;

// The tent's weight at offset (x, y) for a penumbra half width of `penumbra` pixels (kind 3's weight of the mask's difference is
// applied once the tap is read).
fn tent_at(x: i32, y: i32, penumbra: f32) -> f32 {
    let ax = abs(x);
    let ay = abs(y);
    if FILTER_KIND == 1u {
        return select(0.0, 1.0, ax <= 1 && ay <= 1);
    }
    if FILTER_KIND == 2u {
        let r = clamp(penumbra * PENUMBRA_SHARE, 0.0, f32(RADIUS)) + 0.5;
        return max(1.0 - f32(ax) / r, 0.0) * max(1.0 - f32(ay) / r, 0.0);
    }
    return select(1.0, 0.5, ax == RADIUS) * select(1.0, 0.5, ay == RADIUS);
}

@vertex
fn vs_full(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let x = f32((i << 1u) & 2u);
    let y = f32(i & 2u);
    return vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
}

// The view-space point at pixel position `px` with reversed depth `d`.
fn view_point(px: vec2<f32>, d: f32) -> vec3<f32> {
    let ndc = vec2<f32>(px.x * frame.screen.z * 2.0 - 1.0, 1.0 - px.y * frame.screen.w * 2.0);
    let p = frame.inv_proj * vec4<f32>(ndc, d, 1.0);
    return p.xyz / p.w;
}

fn view_normal(px: vec2<i32>) -> vec3<f32> {
    return normalize((frame.view * vec4<f32>(oct_decode(textureLoad(normals, px, 0).xy), 0.0)).xyz);
}

@fragment
fn fs_contact_filter(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let o = vec2<i32>(frag.xy);
    let c = textureLoad(mask_in, o, 0);
    let d = textureLoad(depth, o, 0);
    if d <= 0.0 {
        return vec4<f32>(c.r, c.g, 0.0, c.b);
    }
    let p = view_point(vec2<f32>(o) + 0.5, d);
    let n = view_normal(o);
    let tol = PLANE_TOL.x * -p.z + PLANE_TOL.y;
    let dims = vec2<i32>(textureDimensions(mask_in));
    let penumbra = c.a * PENUMBRA_PIXELS;
    var sum = vec2<f32>(0.0);
    var weight = 0.0;
    var tents = 0.0;
    // The box reaches one pixel: its taps alone are read.
    let reach = select(RADIUS, 1, FILTER_KIND == 1u);
    for (var y = -reach; y <= reach; y = y + 1) {
        for (var x = -reach; x <= reach; x = x + 1) {
            let t = o + vec2<i32>(x, y);
            if any(t < vec2<i32>(0)) || any(t >= dims) {
                continue;
            }
            var tent = tent_at(x, y, penumbra);
            if tent <= 0.0 {
                continue;
            }
            let v = textureLoad(mask_in, t, 0);
            if FILTER_KIND == 3u {
                let delta = v.r - c.r;
                tent = tent * exp(-(delta * delta) / (RANGE_SIGMA * RANGE_SIGMA));
            }
            tents = tents + tent;
            let td = textureLoad(depth, t, 0);
            if td <= 0.0 {
                continue;
            }
            let q = view_point(vec2<f32>(t) + 0.5, td);
            let pw = clamp(1.0 - abs(dot(q - p, n)) / tol, 0.0, 1.0);
            let nw = smoothstep(NORMAL_COS.x, NORMAL_COS.y, dot(n, view_normal(t)));
            let w = tent * pw * nw;
            sum = sum + v.rb * w;
            weight = weight + w;
        }
    }
    if weight < 1e-4 {
        return vec4<f32>(c.r, c.g, 0.0, c.b);
    }
    let v = sum / weight;
    return vec4<f32>(v.x, c.g, weight / max(tents, 1e-4), v.y);
}
