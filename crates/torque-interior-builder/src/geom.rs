//! Points, planes and convex polygons in `f64`.

/// A point or a vector.
pub type V3 = [f64; 3];

/// How far from a plane a point may be and still lie on it (the input's units).
pub const EPS: f64 = 1e-4;

pub fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

pub fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

pub fn scale(a: V3, s: f64) -> V3 {
    [a[0] * s, a[1] * s, a[2] * s]
}

pub fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

pub fn cross(a: V3, b: V3) -> V3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

pub fn length(a: V3) -> f64 {
    dot(a, a).sqrt()
}

/// `a` made unit; None for a zero or not finite vector.
pub fn normalize(a: V3) -> Option<V3> {
    let l = length(a);
    (l > 1e-12 && l.is_finite()).then(|| scale(a, 1.0 / l))
}

/// A plane `n . p + d = 0` with a unit normal. A brush's planes face out of it: its inside is where `n . p + d < 0`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Plane {
    pub n: V3,
    pub d: f64,
}

impl Plane {
    /// The plane through `p` facing along `n` (made unit); None for a zero normal.
    pub fn new(n: V3, p: V3) -> Option<Plane> {
        let n = normalize(n)?;
        Some(Plane { n, d: -dot(n, p) })
    }

    /// The plane through three points facing along `(a - b) x (c - b)` (a `.map` face's order); None for points on a line.
    pub fn from_points(a: V3, b: V3, c: V3) -> Option<Plane> {
        Plane::new(cross(sub(a, b), sub(c, b)), b)
    }

    pub fn dist(&self, p: V3) -> f64 {
        dot(self.n, p) + self.d
    }

    pub fn flipped(self) -> Plane {
        Plane {
            n: scale(self.n, -1.0),
            d: -self.d,
        }
    }

    /// The same plane facing the same way (normals within 1e-6, distances within [`EPS`]).
    pub fn same(&self, o: &Plane) -> bool {
        (0..3).all(|k| (self.n[k] - o.n[k]).abs() < 1e-6) && (self.d - o.d).abs() < EPS
    }

    /// The same plane facing either way.
    pub fn coplanar(&self, o: &Plane) -> bool {
        self.same(o) || self.same(&o.flipped())
    }
}

/// Which side of a plane a polygon is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Front,
    Back,
    On,
    Both,
}

/// The side `poly` is on, its points within [`EPS`] of the plane counted on it.
pub fn side(poly: &[V3], p: &Plane) -> Side {
    let (mut front, mut back) = (false, false);
    for &v in poly {
        let d = p.dist(v);
        front |= d > EPS;
        back |= d < -EPS;
    }
    match (front, back) {
        (true, true) => Side::Both,
        (true, false) => Side::Front,
        (false, true) => Side::Back,
        (false, false) => Side::On,
    }
}

/// `poly` (convex, in order) cut by `p`: the part in front and the part behind, each empty when nothing is on its side. Points
/// within [`EPS`] of the plane go to both.
pub fn split(poly: &[V3], p: &Plane) -> (Vec<V3>, Vec<V3>) {
    let dists: Vec<f64> = poly.iter().map(|&v| p.dist(v)).collect();
    let (mut front, mut back) = (Vec::new(), Vec::new());
    let n = poly.len();
    for i in 0..n {
        let (a, da) = (poly[i], dists[i]);
        let (b, db) = (poly[(i + 1) % n], dists[(i + 1) % n]);
        if da >= -EPS {
            front.push(a);
        }
        if da <= EPS {
            back.push(a);
        }
        if (da > EPS && db < -EPS) || (da < -EPS && db > EPS) {
            let t = da / (da - db);
            let m = add(a, scale(sub(b, a), t));
            front.push(m);
            back.push(m);
        }
    }
    let keep = |v: Vec<V3>| if v.len() >= 3 { v } else { Vec::new() };
    (keep(front), keep(back))
}

/// The part of `poly` behind `p` (on it included).
pub fn clip_back(poly: &[V3], p: &Plane) -> Vec<V3> {
    if poly.iter().all(|&v| p.dist(v) <= EPS) {
        return poly.to_vec();
    }
    split(poly, p).1
}

/// Twice the area vector of a polygon (Newell's normal times twice the area).
pub fn area_vector(poly: &[V3]) -> V3 {
    let mut a = [0.0; 3];
    for i in 1..poly.len().saturating_sub(1) {
        a = add(a, cross(sub(poly[i], poly[0]), sub(poly[i + 1], poly[0])));
    }
    a
}

pub fn area(poly: &[V3]) -> f64 {
    length(area_vector(poly)) * 0.5
}

/// A square of half side `half` on `p` round the point of the plane nearest the origin, wound clockwise seen from the front.
pub fn base_winding(p: &Plane, half: f64) -> Vec<V3> {
    let n = p.n;
    let k = (0..3)
        .min_by(|&a, &b| n[a].abs().total_cmp(&n[b].abs()))
        .unwrap_or(0);
    let mut axis = [0.0; 3];
    axis[k] = 1.0;
    let u = normalize(cross(axis, n)).unwrap_or([1.0, 0.0, 0.0]);
    let v = cross(n, u);
    let c = scale(n, -p.d);
    let at = |a: f64, b: f64| add(c, add(scale(u, a * half), scale(v, b * half)));
    // u, v, n are right-handed: (1,1), (1,-1), (-1,-1), (-1,1) runs clockwise seen from along -n, i.e. from the front.
    vec![at(1.0, 1.0), at(1.0, -1.0), at(-1.0, -1.0), at(-1.0, 1.0)]
}

/// Removes points within [`EPS`] of the one before (and the last of the first).
pub fn dedup(poly: &mut Vec<V3>) {
    poly.dedup_by(|b, a| length(sub(*a, *b)) < EPS);
    while poly.len() > 1 && length(sub(poly[0], poly[poly.len() - 1])) < EPS {
        poly.pop();
    }
}

/// The box of some points.
pub fn bounds(points: impl IntoIterator<Item = V3>) -> (V3, V3) {
    let mut lo = [f64::INFINITY; 3];
    let mut hi = [f64::NEG_INFINITY; 3];
    for p in points {
        for k in 0..3 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    (lo, hi)
}

/// Work counted against a cap: a loop whose length an untrusted input sets charges its steps here before it runs them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Budget {
    pub spent: u64,
    pub cap: u64,
}

impl Budget {
    pub fn new(cap: u64) -> Budget {
        Budget { spent: 0, cap }
    }

    /// Adds `n` steps; false once the total is over the cap.
    pub fn charge(&mut self, n: u64) -> bool {
        self.spent = self.spent.saturating_add(n);
        self.spent <= self.cap
    }
}

/// Whether two boxes meet (grown by `pad`).
pub fn boxes_meet(a: &(V3, V3), b: &(V3, V3), pad: f64) -> bool {
    (0..3).all(|k| a.0[k] <= b.1[k] + pad && b.0[k] <= a.1[k] + pad)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_base_winding_lies_on_its_plane_clockwise_from_the_front() {
        for n in [[0.0, 0.0, 1.0], [1.0, 2.0, -3.0], [0.0, -1.0, 0.0]] {
            let p = Plane::new(n, [1.0, 2.0, 3.0]).unwrap();
            let w = base_winding(&p, 10.0);
            assert!(w.iter().all(|&v| p.dist(v).abs() < 1e-9));
            // Clockwise seen from the front: the area vector points away from the viewer, along -n.
            assert!(dot(area_vector(&w), p.n) < 0.0);
        }
    }

    #[test]
    fn a_split_keeps_both_parts_and_their_area() {
        let sq = vec![
            [0.0, 0.0, 0.0],
            [0.0, 2.0, 0.0],
            [2.0, 2.0, 0.0],
            [2.0, 0.0, 0.0],
        ];
        let p = Plane::new([1.0, 0.0, 0.0], [0.5, 0.0, 0.0]).unwrap();
        let (f, b) = split(&sq, &p);
        assert!((area(&f) - 3.0).abs() < 1e-12 && (area(&b) - 1.0).abs() < 1e-12);
        assert_eq!(side(&f, &p), Side::Front);
        assert_eq!(clip_back(&sq, &p.flipped()).len(), 4);
    }

    #[test]
    fn three_points_give_the_plane_a_map_face_means() {
        // The x = -64 face of a box from -64 to 64 faces -x.
        let p = Plane::from_points(
            [-64.0, -64.0, -16.0],
            [-64.0, -63.0, -16.0],
            [-64.0, -64.0, -15.0],
        )
        .unwrap();
        assert_eq!(p.n, [-1.0, 0.0, 0.0]);
        assert_eq!(p.d, -64.0);
    }
}
