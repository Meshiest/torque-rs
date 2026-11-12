// Area lights (rectangles and tubes) by linearly transformed cosines (Heitz, Dupuy, Hill and Neubelt 2016, "Real-Time
// Polygonal-Light Shading with Linearly Transformed Cosines"): our own code. Prepended after brdf.wgsl (it takes PI from
// common.wgsl). A light's shape is a quad seen from the shading point:
//
// - A rectangle is its own four corners, lit on one side.
// - A tube is the strip between the two lines along its side where the planes through the point touch it (r^2 / D toward the
//   point from the axis, D the point's distance from it, and r sqrt(1 - r^2 / D^2) to each side): every ray from the point to
//   the side it sees crosses that strip, so of a surface of even radiance it gives what the tube's side gives. Seen at a slant
//   the side ends in the arcs of its rims, not in the strip's edges: one more corner at each end stands for its arc, as far
//   out as gives the arc's segment's area. Its ends shine as its side does: the end a point past it sees is a square of the
//   end's area (a second outline), so a wall in line with a tube is not left dark.
//
// The diffuse is the outline's form factor about the normal (Lambert's sum over its edges, clipped to the horizon); the
// specular the same sum over the outline turned by the lobe's fitted inverse matrix (torque_render_scene::hd::ltc), times the
// lobe's albedo split by Schlick's weight. A light's radiance is its color over the area it shows straight in front of it (a
// rectangle's, a tube's length times its diameter), so at a distance it lights as a point light of its color.

// The table's cells a side (torque_render_scene::hd::ltc::LTC_SIZE).
const LTC_SIZE: f32 = 64.0;

// A unit vector at right angles to the unit `n`.
fn area_tangent(n: vec3<f32>) -> vec3<f32> {
    let a = select(vec3<f32>(1.0, 0.0, 0.0), vec3<f32>(0.0, 1.0, 0.0), abs(n.x) > 0.9);
    return normalize(a - n * dot(a, n));
}

// The arc between two directions times its plane's normal's z (one edge's term of Lambert's sum).
fn area_edge(a: vec3<f32>, b: vec3<f32>) -> f32 {
    let na = a * inverseSqrt(max(dot(a, a), 1e-20));
    let nb = b * inverseSqrt(max(dot(b, b), 1e-20));
    let x = clamp(dot(na, nb), -0.9999999, 1.0);
    // theta / sin(theta), its series near 1 (acos loses its digits there).
    let k = select(acos(x) * inverseSqrt(1.0 - x * x), 1.0 + (1.0 - x) / 3.0, x > 0.9999);
    return cross(na, nb).z * k;
}

// The running sum of a quad's edges clipped to the upper half space, and where the quad left it and came back.
struct AreaClip {
    sum: f32,
    leaves: vec3<f32>,
    returns: vec3<f32>,
    crossed: bool,
};

fn area_clip_edge(c: ptr<function, AreaClip>, a: vec3<f32>, b: vec3<f32>) {
    let above_a = a.z >= 0.0;
    let above_b = b.z >= 0.0;
    if above_a && above_b {
        (*c).sum = (*c).sum + area_edge(a, b);
        return;
    }
    if !above_a && !above_b {
        return;
    }
    var x = mix(a, b, a.z / (a.z - b.z));
    x.z = 0.0;
    (*c).crossed = true;
    if above_a {
        (*c).sum = (*c).sum + area_edge(a, x);
        (*c).leaves = x;
    } else {
        (*c).sum = (*c).sum + area_edge(x, b);
        (*c).returns = x;
    }
}

// An area light's outline as the point `p` sees it (corners less p, world axes; four for a rectangle, six for a tube), its
// radiance per unit of color, and whether the point sees its lit side.
struct AreaQuad {
    q0: vec3<f32>,
    q1: vec3<f32>,
    q2: vec3<f32>,
    q3: vec3<f32>,
    q4: vec3<f32>,
    q5: vec3<f32>,
    six: bool,
    radiance: f32,
    lit: bool,
};

// The form factor of the outline (corners in order, in a frame whose z is the cosine's axis): the integral over it of
// max(cos, 0) / pi.
fn area_form_factor(q: AreaQuad) -> f32 {
    var c: AreaClip;
    c.sum = 0.0;
    c.crossed = false;
    area_clip_edge(&c, q.q0, q.q1);
    area_clip_edge(&c, q.q1, q.q2);
    area_clip_edge(&c, q.q2, q.q3);
    if q.six {
        area_clip_edge(&c, q.q3, q.q4);
        area_clip_edge(&c, q.q4, q.q5);
        area_clip_edge(&c, q.q5, q.q0);
    } else {
        area_clip_edge(&c, q.q3, q.q0);
    }
    if c.crossed {
        // The horizon's own edge, from where the outline went under it to where it came back.
        c.sum = c.sum + area_edge(c.leaves, c.returns);
    }
    return abs(c.sum) / (2.0 * PI);
}

// The outline of a rectangle about `center` with half sides `u` and `v`, lit toward u x v.
fn area_rect(center: vec3<f32>, u: vec3<f32>, v: vec3<f32>, p: vec3<f32>) -> AreaQuad {
    var out: AreaQuad;
    let c = center - p;
    out.q0 = c - u - v;
    out.q1 = c + u - v;
    out.q2 = c + u + v;
    out.q3 = c - u + v;
    out.six = false;
    let n = cross(u, v);
    out.radiance = 1.0 / max(length(n), 1e-12);
    out.lit = dot(c, n) < 0.0;
    return out;
}

// The outline of a tube about `center` along the half axis `t` of radius `r`: the strip of its side the point sees, and at each
// end a corner standing for the arc of the end's rim between the strip's corners (the side seen at a slant ends in that arc,
// not in the strip's edge), as far out as makes the triangle the arc's segment's area.
fn area_tube(center: vec3<f32>, t: vec3<f32>, r: f32, p: vec3<f32>) -> AreaQuad {
    var out: AreaQuad;
    let len = max(length(t), 1e-12);
    let axis = t / len;
    let w = p - center;
    let perp = w - axis * dot(w, axis);
    let pl = length(perp);
    let toward = select(perp / max(pl, 1e-20), area_tangent(axis), pl < 1e-10);
    // A point inside the tube (or on its axis): as if just outside it.
    let d = max(pl, r * 1.001);
    // The tangent lines at the half angle a about `toward` (cos a = r / d); the segment's sagitta r - r^2 / d.
    let cos_a = r / d;
    let sin_a = sqrt(max(1.0 - cos_a * cos_a, 0.0));
    let side = cross(axis, toward) * (r * sin_a);
    let c = center - p + toward * (r * cos_a);
    let a = acos(cos_a);
    let k = (a - sin_a * cos_a) / max(sin_a * (1.0 - cos_a), 1e-6);
    let apex = toward * (k * r * (1.0 - cos_a));
    out.q0 = c - t - side;
    out.q1 = c + t - side;
    out.q2 = c + t + apex;
    out.q3 = c + t + side;
    out.q4 = c - t + side;
    out.q5 = c - t + apex;
    out.six = true;
    out.radiance = 1.0 / max(len * r, 1e-12);
    out.lit = true;
    return out;
}

// The end of a tube about `center` along the half axis `t` of radius `r` that the point `p` sees (none between the ends): a
// square of the end's disk's area, its sides across the strip and toward the point.
fn area_tube_end(center: vec3<f32>, t: vec3<f32>, r: f32, p: vec3<f32>) -> AreaQuad {
    var out: AreaQuad;
    let len = max(length(t), 1e-12);
    let axis = t / len;
    let w = p - center;
    let along = dot(w, axis);
    let perp = w - axis * along;
    let pl = length(perp);
    let toward = select(perp / max(pl, 1e-20), area_tangent(axis), pl < 1e-10);
    let half_side = r * 0.886226925;
    let u = cross(axis, toward) * half_side;
    let v = toward * half_side;
    let c = center + t * sign(along) - p;
    out.q0 = c - u - v;
    out.q1 = c + u - v;
    out.q2 = c + u + v;
    out.q3 = c - u + v;
    out.six = false;
    out.radiance = 1.0 / max(len * r, 1e-12);
    out.lit = abs(along) > len;
    return out;
}

// An area light's form factors about the shading frame `m` (x) and the lobe's turned frame `spec_m` (y), and its radiance per unit
// of color (z); all 0 where the point does not see its lit side. `a` and `b` are the light's shape words (common.wgsl's `Light`).
fn area_shares(a: vec4<f32>, b: vec4<f32>, center: vec3<f32>, p: vec3<f32>, m: mat3x3<f32>, spec_m: mat3x3<f32>) -> vec3<f32> {
    var q: AreaQuad;
    if a.w == 1.0 {
        q = area_rect(center, a.xyz, b.xyz, p);
    } else {
        q = area_tube(center, a.xyz, b.w, p);
    }
    if !q.lit {
        return vec3<f32>(0.0);
    }
    var out = vec3<f32>(area_share(q, m), area_share(q, spec_m), q.radiance);
    if a.w != 1.0 {
        let end = area_tube_end(center, a.xyz, b.w, p);
        if end.lit {
            out.x = out.x + area_share(end, m);
            out.y = out.y + area_share(end, spec_m);
        }
    }
    return out;
}

// The outline's corners turned by `m` (into the shading frame, and by the lobe's inverse matrix for the specular).
fn area_turned(q: AreaQuad, m: mat3x3<f32>) -> AreaQuad {
    var out = q;
    out.q0 = m * q.q0;
    out.q1 = m * q.q1;
    out.q2 = m * q.q2;
    out.q3 = m * q.q3;
    if q.six {
        out.q4 = m * q.q4;
        out.q5 = m * q.q5;
    }
    return out;
}

// The shading frame's rows (x toward the eye in the plane of the normal `n`, y across, z the normal) as a matrix that takes a
// world vector into the frame.
fn area_frame(n: vec3<f32>, v: vec3<f32>) -> mat3x3<f32> {
    let t = v - n * dot(n, v);
    // Seen straight on: any tangent (the lobe is round).
    let t1 = select(normalize(t), area_tangent(n), dot(t, t) < 1e-10);
    let t2 = cross(n, t1);
    return transpose(mat3x3<f32>(t1, t2, n));
}

// A table cell's inverse matrix (rows [a 0 b; 0 1 0; c 0 d]) as a matrix.
fn area_minv(abcd: vec4<f32>) -> mat3x3<f32> {
    return mat3x3<f32>(vec3<f32>(abcd.x, 0.0, abcd.z), vec3<f32>(0.0, 1.0, 0.0), vec3<f32>(abcd.y, 0.0, abcd.w));
}

// The quad's form factor in the frame `m` (world to the frame, times the lobe's inverse matrix for the specular).
fn area_share(q: AreaQuad, m: mat3x3<f32>) -> f32 {
    return area_form_factor(area_turned(q, m));
}

// The table's place for roughness `rough` and N.V `nv` (the cells' centers at i / (size - 1)).
fn area_table_uv(rough: f32, nv: f32) -> vec2<f32> {
    let s = LTC_SIZE;
    let k = (s - 1.0) / s;
    return vec2<f32>(clamp(rough, 0.0, 1.0), sqrt(clamp(1.0 - nv, 0.0, 1.0))) * k + 0.5 / s;
}
