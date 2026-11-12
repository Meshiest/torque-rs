//! `[f32; 3]` arithmetic (`Point3F`): each component on its own, a sum over the components taken `x`, then `y`, then `z`.

/// A point or a vector.
pub type F3 = [f32; 3];

/// `a + b`.
#[inline]
pub fn add(a: F3, b: F3) -> F3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

/// `a - b`.
#[inline]
pub fn sub(a: F3, b: F3) -> F3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

/// `a * k`.
#[inline]
pub fn scale(a: F3, k: f32) -> F3 {
    [a[0] * k, a[1] * k, a[2] * k]
}

/// `a + b * k`, each component rounded after the product and again after the sum (not fused).
#[inline]
pub fn add_scaled(a: F3, b: F3, k: f32) -> F3 {
    [a[0] + b[0] * k, a[1] + b[1] * k, a[2] + b[2] * k]
}

/// `mDot`: `(a.x * b.x + a.y * b.y) + a.z * b.z`.
#[inline]
pub fn dot(a: F3, b: F3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// `mCross`: `a x b`.
#[inline]
pub fn cross(a: F3, b: F3) -> F3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// `Point3F::lenSquared`: [`dot`] of `v` with itself.
#[inline]
pub fn length_squared(v: F3) -> f32 {
    dot(v, v)
}

/// `Point3F::len` in `f32`: the root of [`length_squared`]. TGE's own `len` sums the squares in double precision first; a
/// caller that follows it keeps that form apart.
#[inline]
pub fn length(v: F3) -> f32 {
    length_squared(v).sqrt()
}

/// `m_point3F_normalize_C`: each component times `1 / len`; a zero vector gives `(0, 0, 1)`.
pub fn normalize(p: F3) -> F3 {
    let squared = p[0] * p[0] + p[1] * p[1] + p[2] * p[2];
    if squared != 0.0 {
        let factor = 1.0 / squared.sqrt();
        [p[0] * factor, p[1] * factor, p[2] * factor]
    } else {
        [0.0, 0.0, 1.0]
    }
}

/// `v` over its [`length`], each component divided by it (not multiplied by its inverse, as [`normalize`] does), or `v` as it
/// is when the length is not above 0 (a zero or a NaN length).
pub fn unit_or_same(v: F3) -> F3 {
    let l = length(v);
    if l > 0.0 { v.map(|x| x / l) } else { v }
}

/// `Point3F::isZero`: `x*x <= POINT_EPSILON` for each component, `POINT_EPSILON` being the double `1e-4`.
pub fn is_zero(v: F3) -> bool {
    v.iter().all(|&x| f64::from(x * x) <= 1e-4)
}

/// `m_point3F_interpolate_C`: `from * (1 - t) + to * t` per component.
pub fn interpolate(from: F3, to: F3, factor: f32) -> F3 {
    let inverse = 1.0 - factor;
    [
        from[0] * inverse + to[0] * factor,
        from[1] * inverse + to[1] * factor,
        from[2] * inverse + to[2] * factor,
    ]
}
