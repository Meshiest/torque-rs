//! Transcendentals, as Torque's `mSin`, `mCos`, `mAcos`, `mAtan` and `mPow` call the C library's double functions, and the
//! float ones its float code calls.
//!
//! They are `libm`'s, never the platform's: glibc, the Windows C runtime and wasm32's C library round some results to different
//! last bits, and the simulation a client predicts must give the same bits on every target (the trace hash checks it). Simulated
//! and predicted code calls these, never `f32::sin`, `f32::cos`, `f32::atan2`, `f32::exp` or `f32::powf`.

pub fn sin(x: f64) -> f64 {
    libm::sin(x)
}
pub fn cos(x: f64) -> f64 {
    libm::cos(x)
}
pub fn acos(x: f64) -> f64 {
    libm::acos(x)
}
pub fn atan2(y: f64, x: f64) -> f64 {
    libm::atan2(y, x)
}
pub fn pow(x: f64, y: f64) -> f64 {
    libm::pow(x, y)
}
/// The float sine and cosine of `x` (`f32::sin_cos`).
pub fn sin_cos_f32(x: f32) -> (f32, f32) {
    (libm::sinf(x), libm::cosf(x))
}
pub fn sin_f32(x: f32) -> f32 {
    libm::sinf(x)
}
pub fn cos_f32(x: f32) -> f32 {
    libm::cosf(x)
}
pub fn atan2_f32(y: f32, x: f32) -> f32 {
    libm::atan2f(y, x)
}
pub fn acos_f32(x: f32) -> f32 {
    libm::acosf(x)
}
pub fn exp_f32(x: f32) -> f32 {
    libm::expf(x)
}
