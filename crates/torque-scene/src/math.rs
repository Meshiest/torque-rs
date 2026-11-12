//! The engine's plain vector, quaternion and matrix arithmetic, the transcendentals, and the finite-value guards: one copy each,
//! for every crate whose simulated or predicted state they compute.
//!
//! Plain means plain `f32` in the order Torque's source writes each routine: a sum is taken left to right, no step is fused
//! (no `mul_add`), and a length is the root of the dot product. Floating-point arithmetic is not associative, so the order is
//! part of each function's result: a server and a predicting client must reach the same bits on every target, and the trace
//! hash checks that they do. Where two routines compute the same value in a different order (TGE's double-precision
//! [`quat::interpolate`] and the `f32` [`quat::slerp_f32`], or [`quat::mul`] and [`quat::mul_wxyz`]), both are kept, each under
//! its own name. Nothing here goes through a replaceable table: the tables of the collision, simulation, vehicle and scene
//! crates point at these bodies by default, and a host that installs its own routines does so through those tables.
//!
//! Credit: where Torque3D has the same rules, these follow its MIT-licensed `math/mMath_C.cpp`, `math/mMatrix.cpp`,
//! `math/mMatrix.h`, `math/mPoint3.h`, `math/mQuat.cpp` and `math/mQuat.h` (each Copyright (c) 2012 GarageGames, LLC); the
//! code here is this crate's own.

pub mod matrix;
pub mod quat;
pub mod transcendental;
pub mod vec3;

/// `v` held to `lo..=hi`, or `None` when it is not finite (a NaN or an infinity). `lo` must not be above `hi`, nor either a NaN.
#[inline]
pub fn finite_clamp(v: f32, lo: f32, hi: f32) -> Option<f32> {
    v.is_finite().then(|| v.clamp(lo, hi))
}

/// `v` with each component held to `-max..=max`, or `None` when any component is not finite. `max` must not be negative, nor a
/// NaN.
#[inline]
pub fn finite_coords(v: [f32; 3], max: f32) -> Option<[f32; 3]> {
    all_finite(&v).then(|| v.map(|x| x.clamp(-max, max)))
}

/// Whether every value is finite.
#[inline]
pub fn all_finite(v: &[f32]) -> bool {
    v.iter().all(|x| x.is_finite())
}

#[cfg(test)]
#[path = "math_tests.rs"]
mod tests;
