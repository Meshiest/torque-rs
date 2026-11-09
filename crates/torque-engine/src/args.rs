//! Reading a console function's arguments that hold more than one value.

use torque_ts_vm::convert::ScriptError;
use torque_ts_vm::text::atof;

/// `"x y z"` as three floats: the first three words, each read as TorqueScript reads a number (a word that is not one is 0).
/// Fewer than three words is an error naming `what` ("the start must be \"x y z\""). The values may be infinite or NaN (a
/// word such as `1e40`); a caller that needs them finite checks.
///
/// ```
/// use torque_engine::args::vec3;
///
/// assert_eq!(vec3("1 2.5 -3", "the point").unwrap(), [1.0, 2.5, -3.0]);
/// assert_eq!(vec3("1 x 3 4", "the point").unwrap(), [1.0, 0.0, 3.0]);
/// assert!(vec3("1 2", "the point").is_err());
/// ```
pub fn vec3(text: &str, what: &str) -> Result<[f32; 3], ScriptError> {
    let mut words = text.split_ascii_whitespace();
    let mut next = || words.next().map(|w| atof(w.as_bytes()) as f32);
    match (next(), next(), next()) {
        (Some(x), Some(y), Some(z)) => Ok([x, y, z]),
        _ => Err(ScriptError::new(format!("{what} must be \"x y z\""))),
    }
}
