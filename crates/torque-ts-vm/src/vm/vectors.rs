//! The vector and matrix functions. Vectors are three floats read with `sscanf("%f %f %f")` into zeroed slots
//! ([`scan_floats`]); results print with `%g`. Matrices are "transforms", `"px py pz ax ay az angle"` (angle in
//! radians, except `MatrixCreate`'s input in degrees). The quaternion and matrix maths follow Torque3D's
//! MIT-licensed `QuatF`, `AngAxisF` and `MatrixF` (`math/mQuat.cpp`, `math/mAngAxis.cpp`, `math/mMatrix.cpp`,
//! `math/mMath_C.cpp`, each Copyright (c) 2012 GarageGames, LLC).
//!
//! Rounding: named `F32` values are stored as floats; the terms of one expression are computed in double precision and
//! rounded once, so `fl(fadd(fmul(a, b), fmul(c, d)))` below means: compute in doubles, round once. The 4x4 product
//! is computed in floats. NaNs are as IEEE arithmetic gives them.
//!
//! Every constant index in this file is into a fixed-size array and every loop runs over a fixed range, so no
//! script text decides what is indexed; the work is a fixed number of operations per call (the argument
//! bytes were charged when the call collected them). Where the text has fewer numbers than a function reads, the
//! missing ones are 0.

use super::{Native, NativeCall, Vm};
use crate::text::{atof, format_g, scan_floats, to_f32, to_f64, until_nul};

fn arg(c: &NativeCall, i: usize) -> &[u8] {
    until_nul(c.argv.get(i).map_or(&b""[..], Vec::as_slice))
}

pub type V3 = [f32; 3];
/// A 4x4 matrix, row by row.
pub type Mat4 = [[f32; 4]; 4];

fn fadd(a: f64, b: f64) -> f64 {
    a + b
}

fn fsub(a: f64, b: f64) -> f64 {
    a - b
}

pub(crate) fn fmul(a: f64, b: f64) -> f64 {
    a * b
}

pub(crate) fn fdiv(a: f64, b: f64) -> f64 {
    a / b
}

/// The float stored from a double.
pub(crate) fn fl(x: f64) -> f32 {
    to_f32(x)
}

pub(crate) fn d(x: f32) -> f64 {
    to_f64(x)
}

/// A product of two floats, stored as a float.
fn mul(a: f32, b: f32) -> f32 {
    fl(fmul(d(a), d(b)))
}

/// `VectorAdd`'s text (the native and a host's fast path share it).
pub(crate) fn add_text(a: &[u8], b: &[u8]) -> Vec<u8> {
    print(&each(floats(a), floats(b), fadd))
}

/// `n` floats from `s` into zeroed slots.
fn floats<const N: usize>(s: &[u8]) -> [f32; N] {
    let mut v = [0.0; N];
    scan_floats(s, &mut v);
    v
}

fn print(values: &[f32]) -> Vec<u8> {
    let parts: Vec<String> = values.iter().map(|x| format_g(d(*x))).collect();
    parts.join(" ").into_bytes()
}

/// `(x*x + y*y) + z*z`, stored as a float.
fn length_squared([x, y, z]: V3) -> f32 {
    let (x, y, z) = (d(x), d(y), d(z));
    fl(fadd(fadd(fmul(x, x), fmul(y, y)), fmul(z, z)))
}

/// `mSqrt(F32)`: the double square root stored as a float.
fn sqrt(x: f32) -> f32 {
    fl(d(x).sqrt())
}

/// `m_point3F_normalize`: the zero vector becomes (0, 0, 1).
pub(crate) fn normalize(v: V3) -> V3 {
    let sq = length_squared(v);
    if sq == 0.0 {
        return [0.0, 0.0, 1.0];
    }
    let factor = fl(fdiv(1.0, d(sqrt(sq))));
    v.map(|x| mul(x, factor))
}

/// `sin` and `cos` of a float angle, each stored as a float.
fn sin_cos(x: f32) -> (f32, f32) {
    let x = d(x);
    (fl(x.sin()), fl(x.cos()))
}

/// `QuatF::set(AngAxisF)`: `[x, y, z, w]`.
pub fn quat_from_axis_angle(axis: V3, angle: f32) -> [f32; 4] {
    let half = fl(fmul(d(angle), 0.5));
    let (s, c) = sin_cos(half);
    let [x, y, z] = axis.map(|a| mul(a, s));
    [x, y, z, c]
}

/// `QuatF::set(EulerF)`: half angles negated, yaw * pitch * roll.
fn quat_from_euler(e: V3) -> [f32; 4] {
    let half = e.map(|x| fl(fmul(-d(x), 0.5)));
    let [(sx, cx), (sy, cy), (sz, cz)] = half.map(sin_cos);
    let (cycz, sysz, sycz, cysz) = (mul(cy, cz), mul(sy, sz), mul(sy, cz), mul(cy, sz));
    let (cycz, sysz, sycz, cysz, cx, sx) = (d(cycz), d(sysz), d(sycz), d(cysz), d(cx), d(sx));
    [
        fl(fadd(fmul(cycz, sx), fmul(sysz, cx))),
        fl(fsub(fmul(sycz, cx), fmul(cysz, sx))),
        fl(fsub(fmul(cysz, cx), fmul(sycz, sx))),
        fl(fadd(fmul(cycz, cx), fmul(sysz, sx))),
    ]
}

/// `AngAxisF::set(QuatF)`: the axis is not normalised; `(1, 0, 0)` for no rotation.
pub fn axis_angle_from_quat([x, y, z, w]: [f32; 4]) -> (V3, f32) {
    let angle = fl(d(w).acos()) * 2.0;
    let s = sqrt(fl(fsub(1.0, fmul(d(w), d(w)))));
    let axis = if s != 0.0 {
        [x, y, z].map(|v| fl(fdiv(d(v), d(s))))
    } else {
        [1.0, 0.0, 0.0]
    };
    (axis, angle)
}

/// `QuatF::setMatrix` (`m_quatF_set_matF`): row-major, no translation.
pub fn matrix_from_quat([x, y, z, w]: [f32; 4]) -> Mat4 {
    let mut m = [
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ];
    // The near-identity test, in doubles against the float 1e-19.
    let length = fadd(fadd(fmul(d(x), d(x)), fmul(d(y), d(y))), fmul(d(z), d(z)));
    if length < f64::from(1e-19f32) {
        return m;
    }
    let (xs, ys, zs) = (mul(x, 2.0), mul(y, 2.0), mul(z, 2.0));
    let (wx, wy, wz) = (mul(w, xs), mul(w, ys), mul(w, zs));
    let (xx, xy, xz) = (mul(x, xs), mul(x, ys), mul(x, zs));
    let (yy, yz, zz) = (mul(y, ys), mul(y, zs), mul(z, zs));
    let one_minus = |a: f32, b: f32| fl(fsub(1.0, fadd(d(a), d(b))));
    let add = |a: f32, b: f32| fl(fadd(d(a), d(b)));
    let sub = |a: f32, b: f32| fl(fsub(d(a), d(b)));
    m[0][0] = one_minus(yy, zz);
    m[0][1] = add(xy, wz);
    m[0][2] = sub(xz, wy);
    m[1][0] = sub(xy, wz);
    m[1][1] = one_minus(xx, zz);
    m[1][2] = add(yz, wx);
    m[2][0] = add(xz, wy);
    m[2][1] = sub(yz, wx);
    m[2][2] = one_minus(xx, yy);
    m
}

/// `QuatF::set(MatrixF)`.
pub fn quat_from_matrix(m: &Mat4) -> [f32; 4] {
    let trace = fl(fadd(fadd(d(m[0][0]), d(m[1][1])), d(m[2][2])));
    if trace > 0.0 {
        let s = sqrt(fl(fadd(d(trace), 1.0)));
        let w = fl(fmul(d(s), 0.5));
        let s = fl(fdiv(0.5, d(s)));
        let part = |a: f32, b: f32| fl(fmul(fsub(d(a), d(b)), d(s)));
        return [
            part(m[1][2], m[2][1]),
            part(m[2][0], m[0][2]),
            part(m[0][1], m[1][0]),
            w,
        ];
    }
    // The largest diagonal entry decides the axis: i, then j and k are the next two (mod 3).
    let mut i = 0;
    if m[1][1] > m[0][0] {
        i = 1;
    }
    if m[2][2] > m[i][i] {
        i = 2;
    }
    let j = (i + 1) % 3;
    let k = (j + 1) % 3;
    let (ii, jj, kk) = (m[i][i], m[j][j], m[k][k]);
    let s = sqrt(fl(fadd(fsub(d(ii), fadd(d(jj), d(kk))), 1.0)));
    let mut q = [0.0; 4];
    q[i] = fl(fmul(d(s), 0.5));
    let s = fl(fdiv(0.5, d(s)));
    let sum = |a: f32, b: f32| fl(fmul(fadd(d(a), d(b)), d(s)));
    q[j] = sum(m[i][j], m[j][i]);
    q[k] = sum(m[i][k], m[k][i]);
    q[3] = fl(fmul(fsub(d(m[j][k]), d(m[k][j])), d(s)));
    q
}

/// A transform string's matrix: rotation from the axis and angle, translation in the last column.
fn transform(s: &[u8]) -> Mat4 {
    let [px, py, pz, ax, ay, az, angle] = floats::<7>(s);
    let mut m = matrix_from_quat(quat_from_axis_angle([ax, ay, az], angle));
    m[0][3] = px;
    m[1][3] = py;
    m[2][3] = pz;
    m
}

/// The matrix product (`MatrixF::mul`), in floats.
fn multiply(a: &Mat4, b: &Mat4) -> Mat4 {
    let mut r = [[0.0; 4]; 4];
    for (out_row, a_row) in r.iter_mut().zip(a) {
        for (j, out) in out_row.iter_mut().enumerate() {
            let mut sum = 0.0;
            for (k, (&a_ik, b_row)) in a_row.iter().zip(b).enumerate() {
                let b_kj = b_row.get(j).copied().unwrap_or(0.0);
                let term = a_ik * b_kj;
                sum = if k == 0 { term } else { sum + term };
            }
            *out = sum;
        }
    }
    r
}

/// `m * v` (`m_matF_x_vectorF`), plus the translation for a point.
fn mul_vector(m: &Mat4, v: V3, point: bool) -> V3 {
    let mut r = [0.0; 3];
    for (out, row) in r.iter_mut().zip(m) {
        let [a0, a1, a2, a3] = *row;
        let mut sum = fadd(
            fadd(fmul(d(a0), d(v[0])), fmul(d(a1), d(v[1]))),
            fmul(d(a2), d(v[2])),
        );
        if point {
            sum = fadd(sum, d(a3));
        }
        *out = fl(sum);
    }
    r
}

/// Each component of two vectors through `op` in doubles, stored once.
fn each(a: V3, b: V3, op: fn(f64, f64) -> f64) -> V3 {
    let mut r = [0.0; 3];
    for ((out, &x), &y) in r.iter_mut().zip(&a).zip(&b) {
        *out = fl(op(d(x), d(y)));
    }
    r
}

fn vector_len(v: V3) -> f32 {
    sqrt(length_squared(v))
}

pub(super) fn register(vm: &mut Vm) {
    vm.register("VectorAdd", 3, 3, "VectorAdd(a, b) - a + b", |_, c| {
        Native::Str(add_text(arg(c, 1), arg(c, 2)))
    });
    vm.register("VectorSub", 3, 3, "VectorSub(a, b) - a - b", |_, c| {
        Native::Str(print(&each(floats(arg(c, 1)), floats(arg(c, 2)), fsub)))
    });
    // The scale is `atof`'s double rounded to a float (not `sscanf`'s direct float).
    vm.register(
        "VectorScale",
        3,
        3,
        "VectorScale(v, s) - v times s",
        |_, c| {
            let s = to_f32(atof(arg(c, 2)));
            Native::Str(print(&each(floats(arg(c, 1)), [s; 3], fmul)))
        },
    );
    vm.register(
        "VectorNormalize",
        2,
        2,
        "VectorNormalize(v) - v scaled to length 1",
        |_, c| {
            let v: V3 = floats(arg(c, 1));
            // A vector whose float length is 0 (the zero vector, or one too small to square) is
            // returned as it is; NaN counts as not 0.
            let v = if vector_len(v) != 0.0 {
                normalize(v)
            } else {
                v
            };
            Native::Str(print(&v))
        },
    );
    vm.register(
        "VectorDot",
        3,
        3,
        "VectorDot(a, b) - the dot product of a and b",
        |_, c| {
            let (a, b): (V3, V3) = (floats(arg(c, 1)), floats(arg(c, 2)));
            let dot = fadd(
                fadd(fmul(d(a[0]), d(b[0])), fmul(d(a[1]), d(b[1]))),
                fmul(d(a[2]), d(b[2])),
            );
            Native::Float(d(fl(dot)))
        },
    );
    vm.register(
        "VectorCross",
        3,
        3,
        "VectorCross(a, b) - the cross product of a and b",
        |_, c| {
            let (a, b): (V3, V3) = (floats(arg(c, 1)), floats(arg(c, 2)));
            let cross = |i: usize, j: usize| {
                let (ai, aj, bi, bj) = (a[i], a[j], b[i], b[j]);
                fl(fsub(fmul(d(ai), d(bj)), fmul(d(aj), d(bi))))
            };
            Native::Str(print(&[cross(1, 2), cross(2, 0), cross(0, 1)]))
        },
    );
    vm.register(
        "VectorLen",
        2,
        2,
        "VectorLen(v) - the length of v",
        |_, c| Native::Float(d(vector_len(floats(arg(c, 1))))),
    );
    // The difference is argv[2] - argv[1], each component a float.
    vm.register(
        "VectorDist",
        3,
        3,
        "VectorDist(a, b) - the distance from a to b",
        |_, c| {
            let diff = each(floats(arg(c, 2)), floats(arg(c, 1)), fsub);
            Native::Float(d(vector_len(diff)))
        },
    );
    vm.register(
        "VectorOrthoBasis",
        2,
        2,
        "VectorOrthoBasis(axisAngle) - the three axes of the rotation, one after another",
        |_, c| {
            let [x, y, z, angle] = floats::<4>(arg(c, 1));
            let m = matrix_from_quat(quat_from_axis_angle([x, y, z], angle));
            // The transpose, row by row (the matrix is stored by columns there).
            let mut out = [0.0; 9];
            for (n, slot) in out.iter_mut().enumerate() {
                *slot = m[n % 3][n / 3];
            }
            Native::Str(print(&out))
        },
    );
    vm.register(
        "MatrixCreate",
        3,
        3,
        "MatrixCreate(position, axisAngle) - a transform from a position and an axis-angle rotation",
        |_, c| {
            let [px, py, pz] = floats::<3>(arg(c, 1));
            let [ax, ay, az, degrees] = floats::<4>(arg(c, 2));
            let angle = fl(fdiv(fmul(d(degrees), std::f64::consts::PI), 180.0));
            Native::Str(print(&[px, py, pz, ax, ay, az, angle]))
        },
    );
    // Straight from the Euler angles to a quaternion (not through a matrix); the angle is in radians.
    vm.register(
        "MatrixCreateFromEuler",
        2,
        2,
        "MatrixCreateFromEuler(euler) - a transform from Euler angles",
        |_, c| {
            let ([x, y, z], angle) = axis_angle_from_quat(quat_from_euler(floats(arg(c, 1))));
            let mut text = b"0 0 0 ".to_vec();
            text.extend_from_slice(&print(&[x, y, z, angle]));
            Native::Str(text)
        },
    );
    vm.register(
        "MatrixMultiply",
        3,
        3,
        "MatrixMultiply(left, right) - the product of two transforms",
        |_, c| {
            let m = multiply(&transform(arg(c, 1)), &transform(arg(c, 2)));
            let (axis, angle) = axis_angle_from_quat(quat_from_matrix(&m));
            let [x, y, z] = normalize(axis);
            Native::Str(print(&[m[0][3], m[1][3], m[2][3], x, y, z, angle]))
        },
    );
    vm.register(
        "MatrixMulVector",
        3,
        3,
        "MatrixMulVector(transform, vector) - the vector rotated by the transform",
        |_, c| {
            let r = mul_vector(&transform(arg(c, 1)), floats(arg(c, 2)), false);
            Native::Str(print(&r))
        },
    );
    vm.register(
        "MatrixMulPoint",
        3,
        3,
        "MatrixMulPoint(transform, point) - the point transformed",
        |_, c| {
            let r = mul_vector(&transform(arg(c, 1)), floats(arg(c, 2)), true);
            Native::Str(print(&r))
        },
    );
    // The sums are not rounded before the halving (no overflow for huge boxes).
    vm.register(
        "getBoxCenter",
        2,
        2,
        "getBoxCenter(box) - the middle of a box given by its two corners",
        |_, c| {
            let [x0, y0, z0, x1, y1, z1] = floats::<6>(arg(c, 1));
            let mid = |lo: f32, hi: f32| fl(fmul(fadd(d(lo), d(hi)), 0.5));
            Native::Str(print(&[mid(x0, x1), mid(y0, y1), mid(z0, z1)]))
        },
    );
}
