//! The scalar math functions, `atoi`/`atof` and the random number generator. The solvers follow Torque3D's MIT-licensed
//! `math/mSolver.cpp` and the generator its `MRandomLCG` (`math/mRandom.cpp`), both Copyright (c) 2012 GarageGames,
//! LLC.
//!
//! The arithmetic: every argument is `atof`'s double rounded to a float; a C library function (`sin`, `pow`, ...)
//! runs on that float widened to a double; the result is rounded to a float again, and the interpreter prints that
//! float with `%g`. So `mSin(s)` is `to_f32(sin(to_f64(to_f32(atof(s)))))`, never an `f32` library function. NaNs are
//! as IEEE arithmetic and the host's library give them.
//!
//! **The host's math library.** `sin`, `cos`, `tan`, `asin`, `acos`, `log`, `pow` and `atan2` come from the
//! host's C library (Rust's `f64` methods). Their results can differ in the last bit of a double between
//! libraries, and a script sees a result only as a float printed with six significant digits (`%g`), so a
//! difference shows only when a double lies within one double-ulp of a rounding boundary of both the float and
//! the six-digit text.
//!
//! `getRandom(a, b)` over the whole `i32` range takes the draw as it is; `mSolveQuadratic`, `mSolveCubic` and
//! `mSolveQuartic` print 0 for roots they did not find.

use super::{Native, NativeCall, Vm};
use crate::text::{atof, atoi, format_fixed, format_g, to_f32, to_f64, until_nul};

fn arg(c: &NativeCall, i: usize) -> &[u8] {
    until_nul(c.argv.get(i).map_or(&b""[..], Vec::as_slice))
}

/// `dAtof`: the C runtime's `atof`, rounded to a float.
fn float_arg(c: &NativeCall, i: usize) -> f32 {
    to_f32(atof(arg(c, i)))
}

/// A float native's result: the float, widened exactly for the interpreter.
fn float(x: f32) -> Native {
    Native::Float(to_f64(x))
}

/// `f` of the argument widened to a double, rounded to a float.
fn unary(x: f32, f: fn(f64) -> f64) -> f32 {
    to_f32(f(to_f64(x)))
}

/// [`unary`] with two arguments.
fn binary(x: f32, y: f32, f: fn(f64, f64) -> f64) -> f32 {
    to_f32(f(to_f64(x), to_f64(y)))
}

/// `mAtan(rise, run)`: [`binary`] over `atan2`.
fn atan2(rise: f32, run: f32) -> f32 {
    binary(rise, run, f64::atan2)
}

/// A float's conversion to an integer: truncation toward zero, saturating at the `i32` range (NaN is 0).
pub fn ftoi(x: f64) -> i32 {
    x as i32
}

/// `mSolveQuadratic` (Torque3D's `mSolveQuadratic_c`, `math/mSolver.cpp`): the number of roots and the roots, smaller
/// first; 0 in the slots of roots not found.
fn solve_quadratic(a: f32, b: f32, c: f32) -> (i32, [f32; 2]) {
    let zero = |v: f64| -1e-8 < v && v < 1e-8;
    let (a, b, c) = (to_f64(a), to_f64(b), to_f64(c));
    if zero(a) {
        return if zero(b) {
            (0, [0.0; 2])
        } else {
            (1, [to_f32(-c / b), 0.0])
        };
    }
    let desc = to_f32(b * b - (a * 4.0) * c);
    if zero(to_f64(desc)) {
        return (1, [to_f32(b / (a + a)), 0.0]);
    }
    if desc > 0.0 {
        let sq = to_f64(to_f32(to_f64(desc).sqrt()));
        let den = to_f64(to_f32(a + a));
        let x0 = to_f32((-b + sq) / den);
        let x1 = to_f32((-b - sq) / den);
        return (2, if x1 < x0 { [x1, x0] } else { [x0, x1] });
    }
    (0, [0.0; 2])
}

/// `mIsZero` of the solvers: strictly between -1e-8 and 1e-8 (`EQN_EPSILON`).
fn near_zero(v: f64) -> bool {
    -1e-8 < v && v < 1e-8
}

/// `1.f / 3.f` widened to a double.
fn third() -> f64 {
    to_f64(1.0f32 / 3.0)
}

/// The cube root of the solvers (`mCbrt`): `pow(|v|, 1/3)` rounded to a float, negated for a negative `v`.
fn cube_root(v: f32) -> f32 {
    if v < 0.0 {
        -to_f32((-to_f64(v)).powf(third()))
    } else {
        to_f32(to_f64(v).powf(third()))
    }
}

/// The roots kept in order, smallest first, as the solvers sort them (each slot against every later one).
fn sort_roots(x: &mut [f32]) {
    for j in 0..x.len() {
        for k in j + 1..x.len() {
            if x[k] < x[j] {
                x.swap(j, k);
            }
        }
    }
}

/// `mSolveCubic` (Torque3D's `mSolveCubic_c`): the number of roots and the roots, smaller first, by Cardano's formula
/// (every named value a float; `A / 3` kept as a double until the roots are shifted back). An `a` of zero is the
/// quadratic's answer, unshifted. Slots past the roots are 0.
fn solve_cubic(a: f32, b: f32, c: f32, d: f32) -> (usize, [f32; 3]) {
    let mut x = [0.0f32; 3];
    let af = to_f64(a);
    if near_zero(af) {
        let (n, r) = solve_quadratic(b, c, d);
        let n = n as usize;
        x[..n].copy_from_slice(&r[..n]);
        return (n, x);
    }
    let k = third();
    let big_a = to_f32(to_f64(b) / af);
    let big_b = to_f32(to_f64(c) / af);
    let sq_a = to_f32(to_f64(big_a) * to_f64(big_a));
    let p = to_f32((to_f64(big_b) - to_f64(sq_a) * k) * k);
    let sub = k * to_f64(big_a);
    let cube_a = to_f32(to_f64(big_a) * to_f64(sq_a));
    let lead = to_f64(cube_a) * to_f64(2.0f32 / 27.0) - to_f64(big_b) * sub;
    let big_c = to_f32(to_f64(d) / af);
    let q = to_f32((lead + to_f64(big_c)) * 0.5);
    let (pd, qd) = (to_f64(p), to_f64(q));
    let cube_p = to_f32(pd * pd * pd);
    let disc = to_f32(to_f64(to_f32(qd * qd)) + to_f64(cube_p));
    let n = if near_zero(to_f64(disc)) {
        if near_zero(qd) {
            x[0] = 0.0;
            1
        } else {
            let u = cube_root(-q);
            x[1] = -u;
            x[0] = to_f32(to_f64(u) + to_f64(u));
            2
        }
    } else if disc < 0.0 {
        let s = to_f32((-pd).sqrt());
        let t = to_f32(to_f64(s) + to_f64(s));
        let root = to_f32((-to_f64(cube_p)).sqrt());
        let ratio = to_f32(-qd / to_f64(root));
        let phi = to_f64(to_f32(to_f64(to_f32(to_f64(ratio).acos())) * k));
        let neg_t = -to_f64(t);
        let third_pi = 1.047_197_551_196_597_6;
        x[1] = to_f32((phi + third_pi).cos() * neg_t);
        x[2] = to_f32((phi - third_pi).cos() * neg_t);
        x[0] = to_f32(to_f64(to_f32(phi.cos())) * to_f64(t));
        3
    } else {
        let s = to_f32(to_f64(disc).sqrt());
        let u = cube_root(to_f32(to_f64(s) + qd));
        let v = cube_root(to_f32(to_f64(s) - qd));
        x[0] = to_f32(to_f64(v) - to_f64(u));
        1
    };
    let sub = to_f32(sub);
    for r in &mut x[..n] {
        *r = to_f32(to_f64(*r) - to_f64(sub));
    }
    sort_roots(&mut x[..n]);
    (n, x)
}

/// `mSolveQuartic` (Torque3D's `mSolveQuartic_c`, Ferrari's method): the number of roots and the roots,
/// smaller first. The resolvent cubic's smallest root `z` splits it into two quadratics, `x^2 + v x + (z - u)` and `x^2 - v x +
/// (z + u)`; a negative `z^2 - r` or `2z - p` answers 0 roots, the cubic's roots left in the
/// slots. An `a` of zero is the cubic's answer.
fn solve_quartic(a: f32, b: f32, c: f32, d: f32, e: f32) -> (usize, [f32; 4]) {
    let mut x = [0.0f32; 4];
    let af = to_f64(a);
    if near_zero(af) {
        let (n, r) = solve_cubic(b, c, d, e);
        x[..3].copy_from_slice(&r);
        return (n, x);
    }
    let big_a = to_f32(to_f64(b) / af);
    let big_b = to_f32(to_f64(c) / af);
    let big_c = to_f32(to_f64(d) / af);
    let big_d = to_f32(to_f64(e) / af);
    let (ad, bd, cd) = (to_f64(big_a), to_f64(big_b), to_f64(big_c));
    let sq_a = to_f32(ad * ad);
    let sd = to_f64(sq_a);
    let p = to_f32(bd - 0.375 * sd);
    let q = to_f32((sd * ad * 0.125 - ad * 0.5 * bd) + cd);
    let sub = to_f32(ad * 0.25);
    let r =
        to_f32(((sd * 0.0625 * bd - sd * sd * 0.011_718_75) - cd * to_f64(sub)) + to_f64(big_d));
    let (pd, qd, rd) = (to_f64(p), to_f64(q), to_f64(r));
    let n = if near_zero(rd) {
        let (n, roots) = solve_cubic(1.0, 0.0, p, q);
        x[..3].copy_from_slice(&roots);
        x[n] = 0.0;
        n + 1
    } else {
        let (_, roots) = solve_cubic(
            1.0,
            to_f32(pd * -0.5),
            -r,
            to_f32(0.5 * rd * pd - qd * qd * 0.125),
        );
        x[..3].copy_from_slice(&roots);
        let z = to_f64(x[0]);
        let u = to_f32(z * z - rd);
        let v = to_f32((z + z) - pd);
        let side = |w: f32| {
            let wd = to_f64(w);
            if near_zero(wd) {
                Some(0.0f32)
            } else if w <= 0.0 {
                None
            } else {
                Some(to_f32(wd.sqrt()))
            }
        };
        let (Some(u), Some(v)) = (side(u), side(v)) else {
            return (0, x);
        };
        let (n1, r1) = solve_quadratic(1.0, v, to_f32(z - to_f64(u)));
        let n1 = n1 as usize;
        x[..n1].copy_from_slice(&r1[..n1]);
        let (n2, r2) = solve_quadratic(1.0, -v, to_f32(to_f64(u) + z));
        let n2 = n2 as usize;
        x[n1..n1 + n2].copy_from_slice(&r2[..n2]);
        n1 + n2
    };
    for root in &mut x[..n] {
        *root = to_f32(to_f64(*root) - to_f64(sub));
    }
    sort_roots(&mut x[..n]);
    (n, x)
}

/// `mFloatLength(v, n)`: `%.nf` with `n` clamped to 9 as an unsigned number (so a negative
/// `n` is 9 too), of the float `v`.
fn float_length(v: f32, decimals: i32) -> Vec<u8> {
    let n = (decimals as u32).min(9) as usize;
    format_fixed(to_f64(v), n).into_bytes()
}

/// A one-argument float native: name, usage, the double function.
type Unary = (&'static str, &'static str, fn(f64) -> f64);

/// The generator's state when a `Vm` starts: Torque3D's global seed (`gRandGenSeed`, `math/mRandom.cpp`).
pub const INITIAL_RANDOM_SEED: i32 = 1_376_312_589;

impl Vm {
    /// One draw of the generator (`MRandomLCG::randI`, the Park-Miller "minimal
    /// standard": state * 16807 mod 2^31 - 1, with Schrage's method above 127773). A state of 0 stays 0;
    /// a negative state (from `setRandomSeed(-1)`) stays negative and is returned as a large unsigned value.
    pub fn random_next(&mut self) -> u32 {
        let s = self.random_seed;
        let next = if s <= 127_773 {
            s.wrapping_mul(16807) % 0x7fff_ffff
        } else {
            let t = (16807i32.wrapping_mul(s % 127_773))
                .wrapping_sub(2836i32.wrapping_mul(s / 127_773));
            if t > 0 {
                t
            } else {
                t.wrapping_add(0x7fff_ffff)
            }
        };
        self.random_seed = next;
        next as u32
    }

    /// The generator's state (`getRandomSeed`).
    pub fn random_seed(&self) -> i32 {
        self.random_seed
    }

    /// Sets the generator's state (`setRandomSeed`); the value is used as given.
    pub fn set_random_seed(&mut self, seed: i32) {
        self.random_seed = seed;
    }

    /// `getRandom([a [, b]])`. No argument: a float in (0, 1], `F32(r) * 2^-31`. One: an
    /// integer from 0 to `a`, or 0 without a draw when `a` is negative. Two: an integer between them, in
    /// either order. Integers come back as floats, so values above 2^24 lose their low bits.
    fn get_random(&mut self, c: &NativeCall) -> f32 {
        match c.argv.len() {
            2 => {
                let a = atoi(arg(c, 1));
                if a < 0 {
                    return 0.0;
                }
                let r = self.random_next() % (a as u32).wrapping_add(1);
                r as i32 as f32
            }
            n if n >= 3 => {
                let (a, b) = (atoi(arg(c, 1)), atoi(arg(c, 2)));
                let (lo, hi) = (a.min(b), a.max(b));
                let r = self.random_next();
                // hi - lo + 1 wraps to 0 only for the whole i32 range, where a division by zero would
                // follow; this takes the draw as it is.
                let span = (hi.wrapping_sub(lo) as u32).wrapping_add(1);
                let r = if span == 0 { r } else { r % span };
                r.wrapping_add(lo as u32) as i32 as f32
            }
            _ => self.random_next() as f32 * (1.0 / 2_147_483_648.0),
        }
    }
}

pub(super) fn register(vm: &mut Vm) {
    vm.register("mFloor", 2, 2, "mFloor(x) - x rounded down", |_, c| {
        Native::Int(ftoi(to_f64(float_arg(c, 1)).floor()))
    });
    vm.register("mCeil", 2, 2, "mCeil(x) - x rounded up", |_, c| {
        Native::Int(ftoi(to_f64(float_arg(c, 1)).ceil()))
    });
    // The low bound wins when the bounds are crossed (min with the high bound first).
    vm.register(
        "mClamp",
        4,
        4,
        "mClamp(value, low, high) - the value kept between low and high",
        |_, c| {
            let (v, lo, hi) = (atoi(arg(c, 1)), atoi(arg(c, 2)), atoi(arg(c, 3)));
            let m = if v > hi { hi } else { v };
            Native::Int(if m > lo { m } else { lo })
        },
    );
    vm.register(
        "getMin",
        3,
        3,
        "getMin(a, b) - the smaller number",
        |_, c| {
            let (a, b) = (float_arg(c, 1), float_arg(c, 2));
            float(if a > b { b } else { a })
        },
    );
    vm.register(
        "getMax",
        3,
        3,
        "getMax(a, b) - the larger number",
        |_, c| {
            let (a, b) = (float_arg(c, 1), float_arg(c, 2));
            float(if a > b { a } else { b })
        },
    );
    vm.register("mAbs", 2, 2, "mAbs(x) - the absolute value", |_, c| {
        float(f32::from_bits(float_arg(c, 1).to_bits() & 0x7fff_ffff))
    });
    let unaries: [Unary; 8] = [
        ("mSqrt", "mSqrt(x) - the square root", f64::sqrt),
        ("mLog", "mLog(x) - the natural logarithm", f64::ln),
        ("mSin", "mSin(radians) - the sine", f64::sin),
        ("mCos", "mCos(radians) - the cosine", f64::cos),
        ("mTan", "mTan(radians) - the tangent", f64::tan),
        ("mAsin", "mAsin(x) - the arc sine, in radians", f64::asin),
        ("mAcos", "mAcos(x) - the arc cosine, in radians", f64::acos),
        (
            "mRadToDeg",
            "mRadToDeg(radians) - the angle in degrees",
            |x| x * 180.0 / std::f64::consts::PI,
        ),
    ];
    for (name, usage, f) in unaries {
        vm.register(name, 2, 2, usage, move |_, c| {
            float(unary(float_arg(c, 1), f))
        });
    }
    vm.register(
        "mDegToRad",
        2,
        2,
        "mDegToRad(degrees) - the angle in radians",
        |_, c| float(unary(float_arg(c, 1), |x| x * std::f64::consts::PI / 180.0)),
    );
    vm.register(
        "mPow",
        3,
        3,
        "mPow(base, exponent) - base raised to the exponent",
        |_, c| float(binary(float_arg(c, 1), float_arg(c, 2), f64::powf)),
    );
    vm.register(
        "mAtan",
        3,
        3,
        "mAtan(y, x) - the angle of the point (x, y), in radians",
        |_, c| float(atan2(float_arg(c, 1), float_arg(c, 2))),
    );
    vm.register(
        "mFloatLength",
        3,
        3,
        "mFloatLength(x, decimals) - x printed with that many decimals",
        |_, c| Native::Str(float_length(float_arg(c, 1), atoi(arg(c, 2)))),
    );
    // The usage text has a blank line before "@returns" (the manifest shows the two newlines as spaces).
    vm.register(
        "mSolveQuadratic",
        4,
        4,
        "mSolveQuadratic(a, b, c) - the real roots of a*x^2 + b*x + c: their count, then the roots",
        |_, c| {
            let (n, x) = solve_quadratic(float_arg(c, 1), float_arg(c, 2), float_arg(c, 3));
            let text = format!("{n} {} {}", format_g(to_f64(x[0])), format_g(to_f64(x[1])));
            Native::Str(text.into_bytes())
        },
    );
    vm.register(
        "mSolveCubic",
        5,
        5,
        "mSolveCubic(a, b, c, d) - the real roots of a*x^3 + b*x^2 + c*x + d: their count, then the roots",
        |_, c| {
            let (n, x) = solve_cubic(float_arg(c, 1), float_arg(c, 2), float_arg(c, 3), float_arg(c, 4));
            let roots: Vec<String> = x.iter().map(|r| format_g(to_f64(*r))).collect();
            Native::Str(format!("{n} {}", roots.join(" ")).into_bytes())
        },
    );
    vm.register(
        "mSolveQuartic",
        6,
        6,
        "mSolveQuartic(a, b, c, d, e) - the real roots of a*x^4 + b*x^3 + c*x^2 + d*x + e: their count, then the roots",
        |_, c| {
            let (n, x) = solve_quartic(
                float_arg(c, 1),
                float_arg(c, 2),
                float_arg(c, 3),
                float_arg(c, 4),
                float_arg(c, 5),
            );
            let roots: Vec<String> = x.iter().map(|r| format_g(to_f64(*r))).collect();
            Native::Str(format!("{n} {}", roots.join(" ")).into_bytes())
        },
    );
    vm.register(
        "atoi",
        2,
        2,
        "atoi(text) - the integer the text starts with",
        |_, c| Native::Int(atoi(arg(c, 1))),
    );
    vm.register(
        "atof",
        2,
        2,
        "atof(text) - the number the text starts with",
        |_, c| float(float_arg(c, 1)),
    );
    vm.register(
        "getRandom",
        1,
        3,
        "getRandom([a [, b]]) - a random number: in (0, 1], from 0 to a, or from a to b",
        |vm, c| float(vm.get_random(c)),
    );
    vm.register(
        "getRandomSeed",
        1,
        1,
        "getRandomSeed() - the random generator's state",
        |vm, _| Native::Int(vm.random_seed),
    );
    // Without an argument the seed is the tick count (`getRealTime`).
    vm.register(
        "setRandomSeed",
        1,
        2,
        "setRandomSeed([seed]) - sets the random generator's state (the real-time counter without a seed)",
        |vm, c| {
            vm.random_seed = if c.argv.len() >= 2 {
                atoi(arg(c, 1))
            } else {
                vm.clock.real_time_ms() as i32
            };
            Native::Void
        },
    );
}
