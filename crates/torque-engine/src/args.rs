//! Reading a console function's arguments that hold more than one value.

use torque_ts_vm::convert::ScriptError;
use torque_ts_vm::sim::ObjId;
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

/// [`vec3`] with every coordinate finite: an infinite or NaN one (a word such as `1e40`) is an error naming `what`
/// ("the start must be finite").
///
/// ```
/// use torque_engine::args::vec3_finite;
///
/// assert_eq!(vec3_finite("1 2 3", "the point").unwrap(), [1.0, 2.0, 3.0]);
/// assert!(vec3_finite("1 1e40 3", "the point").is_err());
/// ```
pub fn vec3_finite(text: &str, what: &str) -> Result<[f32; 3], ScriptError> {
    let v = vec3(text, what)?;
    if v.iter().all(|c| c.is_finite()) {
        Ok(v)
    } else {
        Err(ScriptError::new(format!("{what} must be finite")))
    }
}

/// A vector as `"x y z"` the way Rust prints a float (the shortest text that reads back as the same number, no exponent): the
/// reverse of [`vec3`] for finite values.
///
/// ```
/// use torque_engine::args::vec3_text;
///
/// assert_eq!(vec3_text([1.0, 2.5, -3.0]), "1 2.5 -3");
/// ```
#[must_use]
pub fn vec3_text(v: [f32; 3]) -> String {
    format!("{} {} {}", v[0], v[1], v[2])
}

/// The object id a script gave as an integer: a negative one names no object, so it is 0.
///
/// ```
/// use torque_engine::args::obj_id;
///
/// assert_eq!(obj_id(1347), 1347);
/// assert_eq!(obj_id(-5), 0);
/// ```
#[must_use]
pub fn obj_id(id: i32) -> ObjId {
    ObjId::try_from(id).unwrap_or(0)
}
