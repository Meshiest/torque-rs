//! `MatrixF` arithmetic: a 4x4 matrix row-major, `m[4 * row + column]`, the translation in the last column (elements 3, 7 and
//! 11).

use super::transcendental;
use super::vec3::F3;

/// A transform as `MatrixF` keeps it.
pub type Matrix = [f32; 16];

/// The identity (`MatrixF::identity`).
pub const IDENTITY: Matrix = [
    1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
];

/// `m_matF_x_matF` (`default_matF_x_matF_C`): row `i` of the result is `a[i0]*b[0j] + a[i1]*b[1j] + a[i2]*b[2j] +
/// a[i3]*b[3j]`, summed left to right.
pub fn product(a: &Matrix, b: &Matrix) -> Matrix {
    let mut out = [0.0f32; 16];
    for i in 0..4 {
        for j in 0..4 {
            out[i * 4 + j] = a[i * 4] * b[j]
                + a[i * 4 + 1] * b[4 + j]
                + a[i * 4 + 2] * b[8 + j]
                + a[i * 4 + 3] * b[12 + j];
        }
    }
    out
}

/// `m_matF_x_point3F` (`MatrixF::mulP`).
pub fn mul_p(m: &Matrix, p: F3) -> F3 {
    [
        m[0] * p[0] + m[1] * p[1] + m[2] * p[2] + m[3],
        m[4] * p[0] + m[5] * p[1] + m[6] * p[2] + m[7],
        m[8] * p[0] + m[9] * p[1] + m[10] * p[2] + m[11],
    ]
}

/// `m_matF_x_vectorF` (`MatrixF::mulV`): a direction turned by the 3x3 part.
pub fn mul_v(m: &Matrix, v: F3) -> F3 {
    [
        m[0] * v[0] + m[1] * v[1] + m[2] * v[2],
        m[4] * v[0] + m[5] * v[1] + m[6] * v[2],
        m[8] * v[0] + m[9] * v[1] + m[10] * v[2],
    ]
}

/// `m_matF_x_box3F_C` (Graphics Gems I, 548): the axis-aligned box of the box `min`, `max` moved by `m`. Each axis starts at the
/// translation; for each element `r` of the row, of `r * min[j]` and `r * max[j]` the smaller is added to the minimum and the
/// larger to the maximum (a NaN product counts as not smaller).
pub fn mul_box(m: &Matrix, min: F3, max: F3) -> (F3, F3) {
    let mut low = [m[3], m[7], m[11]];
    let mut high = low;
    for axis in 0..3 {
        for input in 0..3 {
            let a = m[4 * axis + input] * min[input];
            let b = m[4 * axis + input] * max[input];
            if a < b {
                low[axis] += a;
                high[axis] += b;
            } else {
                low[axis] += b;
                high[axis] += a;
            }
        }
    }
    (low, high)
}

/// `m_matF_transpose`: the full 4x4 transpose.
pub fn transpose(m: &Matrix) -> Matrix {
    let mut o = *m;
    for i in 0..4 {
        for j in 0..4 {
            o[i * 4 + j] = m[j * 4 + i];
        }
    }
    o
}

/// `m_matF_determinant_C`: the determinant of the 3x3 part.
pub fn determinant(m: &Matrix) -> f32 {
    m[0] * (m[5] * m[10] - m[6] * m[9])
        + m[4] * (m[2] * m[9] - m[1] * m[10])
        + m[8] * (m[1] * m[6] - m[2] * m[5])
}

/// `m_matF_inverse_C`: the inverse of the affine matrix `m` by cofactors over its [`determinant`] and the translation turned
/// back. Elements 12-15 are left as they were.
pub fn inverse(m: &Matrix) -> Matrix {
    let det = determinant(m);
    let inv_det = 1.0 / det;
    let mut o = *m;
    o[0] = (m[5] * m[10] - m[6] * m[9]) * inv_det;
    o[1] = (m[9] * m[2] - m[10] * m[1]) * inv_det;
    o[2] = (m[1] * m[6] - m[2] * m[5]) * inv_det;
    o[4] = (m[6] * m[8] - m[4] * m[10]) * inv_det;
    o[5] = (m[10] * m[0] - m[8] * m[2]) * inv_det;
    o[6] = (m[2] * m[4] - m[0] * m[6]) * inv_det;
    o[8] = (m[4] * m[9] - m[5] * m[8]) * inv_det;
    o[9] = (m[8] * m[1] - m[9] * m[0]) * inv_det;
    o[10] = (m[0] * m[5] - m[1] * m[4]) * inv_det;
    let t = mul_v(&o, [-m[3], -m[7], -m[11]]);
    o[3] = t[0];
    o[7] = t[1];
    o[11] = t[2];
    o
}

/// `m_matF_affineInverse_C`: the 3x3 part transposed and the translation `-(R^T t)` (the inverse of a transform whose rotation
/// is orthonormal). Elements 12-15 are left as they were.
pub fn affine_inverse(m: &Matrix) -> Matrix {
    let t = *m;
    let mut o = *m;
    o[1] = t[4];
    o[4] = t[1];
    o[2] = t[8];
    o[8] = t[2];
    o[6] = t[9];
    o[9] = t[6];
    o[3] = -(t[0] * t[3] + t[4] * t[7] + t[8] * t[11]);
    o[7] = -(t[1] * t[3] + t[5] * t[7] + t[9] * t[11]);
    o[11] = -(t[2] * t[3] + t[6] * t[7] + t[10] * t[11]);
    o
}

/// `MatrixF::fullInverse()`: the 4x4 inverse by cofactors, each times `1 / det`. False, and the matrix left as it was, when the
/// determinant is below 1e-5 in size (a NaN one is not, and goes through).
pub fn full_inverse(m: &mut Matrix) -> bool {
    let row = |r: usize| [m[4 * r], m[4 * r + 1], m[4 * r + 2], m[4 * r + 3]];
    let ([ax, ay, az, aw], [bx, by, bz, bw]) = (row(0), row(1));
    let ([cx, cy, cz, cw], [dx, dy, dz, dw]) = (row(2), row(3));
    let det = ax * by * cz * dw - ax * by * cw * dz - ax * cy * bz * dw
        + ax * cy * bw * dz
        + ax * dy * bz * cw
        - ax * dy * bw * cz
        - bx * ay * cz * dw
        + bx * ay * cw * dz
        + bx * cy * az * dw
        - bx * cy * aw * dz
        - bx * dy * az * cw
        + bx * dy * aw * cz
        + cx * ay * bz * dw
        - cx * ay * bw * dz
        - cx * by * az * dw
        + cx * by * aw * dz
        + cx * dy * az * bw
        - cx * dy * aw * bz
        - dx * ay * bz * cw
        + dx * ay * bw * cz
        + dx * by * az * cw
        - dx * by * aw * cz
        - dx * cy * az * bw
        + dx * cy * aw * bz;
    if det.abs() < 0.00001 {
        return false;
    }
    let out = [
        by * cz * dw - by * cw * dz - cy * bz * dw + cy * bw * dz + dy * bz * cw - dy * bw * cz,
        -ay * cz * dw + ay * cw * dz + cy * az * dw - cy * aw * dz - dy * az * cw + dy * aw * cz,
        ay * bz * dw - ay * bw * dz - by * az * dw + by * aw * dz + dy * az * bw - dy * aw * bz,
        -ay * bz * cw + ay * bw * cz + by * az * cw - by * aw * cz - cy * az * bw + cy * aw * bz,
        -bx * cz * dw + bx * cw * dz + cx * bz * dw - cx * bw * dz - dx * bz * cw + dx * bw * cz,
        ax * cz * dw - ax * cw * dz - cx * az * dw + cx * aw * dz + dx * az * cw - dx * aw * cz,
        -ax * bz * dw + ax * bw * dz + bx * az * dw - bx * aw * dz - dx * az * bw + dx * aw * bz,
        ax * bz * cw - ax * bw * cz - bx * az * cw + bx * aw * cz + cx * az * bw - cx * aw * bz,
        bx * cy * dw - bx * cw * dy - cx * by * dw + cx * bw * dy + dx * by * cw - dx * bw * cy,
        -ax * cy * dw + ax * cw * dy + cx * ay * dw - cx * aw * dy - dx * ay * cw + dx * aw * cy,
        ax * by * dw - ax * bw * dy - bx * ay * dw + bx * aw * dy + dx * ay * bw - dx * aw * by,
        -ax * by * cw + ax * bw * cy + bx * ay * cw - bx * aw * cy - cx * ay * bw + cx * aw * by,
        -bx * cy * dz + bx * cz * dy + cx * by * dz - cx * bz * dy - dx * by * cz + dx * bz * cy,
        ax * cy * dz - ax * cz * dy - cx * ay * dz + cx * az * dy + dx * ay * cz - dx * az * cy,
        -ax * by * dz + ax * bz * dy + bx * ay * dz - bx * az * dy - dx * ay * bz + dx * az * by,
        ax * by * cz - ax * bz * cy - bx * ay * cz + bx * az * cy + cx * ay * bz - cx * az * by,
    ];
    let inv = 1.0 / det;
    *m = out.map(|e| e * inv);
    true
}

/// `mSinCos`: `mSin` and `mCos`, the double functions of the float angle, each a float.
fn sin_cos(angle: f32) -> (f32, f32) {
    let a = f64::from(angle);
    (transcendental::sin(a) as f32, transcendental::cos(a) as f32)
}

/// `m_matF_set_euler_C` (`MatrixF::set(EulerF)`): an axis counts when its angle is not zero (a NaN counts). No axis: the
/// identity; x, y or z alone: that rotation; any other mix: the general form. No translation.
pub fn set_euler(e: F3) -> Matrix {
    let mut m = IDENTITY;
    let axes = (e[0] != 0.0) as u32 | ((e[1] != 0.0) as u32) << 1 | ((e[2] != 0.0) as u32) << 2;
    match axes {
        0 => {}
        1 => {
            let (sx, cx) = sin_cos(e[0]);
            m[5] = cx;
            m[6] = sx;
            m[9] = -sx;
            m[10] = cx;
        }
        2 => {
            let (sy, cy) = sin_cos(e[1]);
            m[0] = cy;
            m[2] = -sy;
            m[8] = sy;
            m[10] = cy;
        }
        4 => {
            let (sz, cz) = sin_cos(e[2]);
            m[0] = cz;
            m[1] = sz;
            m[4] = -sz;
            m[5] = cz;
        }
        _ => {
            let (sx, cx) = sin_cos(e[0]);
            let (sy, cy) = sin_cos(e[1]);
            let (sz, cz) = sin_cos(e[2]);
            let r1 = cy * cz;
            let r2 = cy * sz;
            let r3 = sy * cz;
            let r4 = sy * sz;
            m[0] = r1 - (r4 * sx);
            m[1] = r2 + (r3 * sx);
            m[2] = -cx * sy;
            m[4] = -cx * sz;
            m[5] = cx * cz;
            m[6] = sx;
            m[8] = r3 + (r2 * sx);
            m[9] = r4 - (r1 * sx);
            m[10] = cx * cy;
        }
    }
    m
}

/// `QuatF::setMatrix` over `m_quatF_set_matF_C`: the quaternion `(x, y, z, w)` as a matrix with no translation; a vector part
/// whose squared length is below `10e-20` gives the identity. The inverse of [`super::quat::from_matrix`].
pub fn from_quat(q: [f32; 4]) -> Matrix {
    let [x, y, z, w] = q;
    if x * x + y * y + z * z < 10e-20 {
        return IDENTITY;
    }
    let xs = x * 2.0;
    let ys = y * 2.0;
    let zs = z * 2.0;
    let wx = w * xs;
    let wy = w * ys;
    let wz = w * zs;
    let xx = x * xs;
    let xy = x * ys;
    let xz = x * zs;
    let yy = y * ys;
    let yz = y * zs;
    let zz = z * zs;
    let mut m = [0.0f32; 16];
    m[0] = 1.0 - (yy + zz);
    m[4] = xy - wz;
    m[8] = xz + wy;
    m[1] = xy + wz;
    m[5] = 1.0 - (xx + zz);
    m[9] = yz - wx;
    m[2] = xz - wy;
    m[6] = yz + wx;
    m[10] = 1.0 - (xx + yy);
    m[15] = 1.0;
    m
}
