//! Values between Lua and TorqueScript: everything in TorqueScript is text, so a Lua value becomes the text a script would
//! write for it, and text comes back as a Lua string.

use mlua::{Error, Lua, MultiValue, Result, Value};

use crate::api::Object;

/// The text TorqueScript sees for `value`: a string as it is, a number as a script writes it (`3`, `0.5`), a truth value
/// as `1` or `0`, nil as the empty text, an object as its id. Tables and functions have none.
pub(crate) fn to_text(value: &Value) -> Result<Vec<u8>> {
    Ok(match value {
        Value::Nil => Vec::new(),
        Value::Boolean(b) => {
            if *b {
                b"1".to_vec()
            } else {
                b"0".to_vec()
            }
        }
        Value::Integer(i) => i.to_string().into_bytes(),
        Value::Number(n) => number_text(*n).into_bytes(),
        Value::String(s) => s.as_bytes().to_vec(),
        Value::UserData(data) => match data.borrow::<Object>() {
            Ok(object) => object.0.to_string().into_bytes(),
            Err(_) => {
                return Err(Error::runtime(
                    "this value cannot be passed to TorqueScript",
                ));
            }
        },
        other => {
            return Err(Error::runtime(format!(
                "a {} cannot be passed to TorqueScript (pass text, numbers, booleans or objects)",
                other.type_name()
            )));
        }
    })
}

/// A number as TorqueScript writes one: whole numbers without a point.
fn number_text(n: f64) -> String {
    if n.is_finite() && n.fract() == 0.0 && n.abs() < 9.007_199_254_740_992e15 {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}

/// The texts of `values` (the arguments of a call), at most `max` of them.
pub(crate) fn texts(values: &MultiValue, max: usize) -> Result<Vec<Vec<u8>>> {
    if values.len() > max {
        return Err(Error::runtime(format!(
            "too many arguments ({}; TorqueScript takes at most {max})",
            values.len()
        )));
    }
    values.iter().map(to_text).collect()
}

/// `text` as a Lua string.
pub(crate) fn from_text(lua: &Lua, text: &[u8]) -> Result<Value> {
    lua.create_string(text).map(Value::String)
}
