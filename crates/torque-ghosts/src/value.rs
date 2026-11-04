//! A field's value as a host's view hands it over or as a record holds it: the vocabulary of the object and datablock views.

use torque_net::bits::Affine;

/// A field's value, as the host hands it over (a view) or as it travels (a record).
///
/// In a record every number is the wire value: `Int` is the integer that goes out (already masked, or the raw 32 bits of an `S32`),
/// `F32` a raw float, `Id` the datablock id after its wrap to the datablock-id width, `Xform` the 193 bits of a transform as six float words and the
/// `w < 0` flag. `F32` compares by bits, so a NaN equals itself.
#[derive(Debug, Clone)]
pub enum Value {
    /// No value (a null pointer or string, an unset field). Reads as 0, false, "" or no id.
    Null,
    Bool(bool),
    Int(i64),
    F32(f32),
    Str(Vec<u8>),
    /// A datablock pointer already resolved to the object's id (3 or more) by the host at pack time; `None` is null.
    Id(Option<u32>),
    /// A transform: `None` is the identity matrix, else the position, the quaternion's x, y, z (as float bits) and the flag `w < 0`.
    Xform(Option<[u32; 7]>),
}

impl PartialEq for Value {
    fn eq(&self, other: &Value) -> bool {
        match (self, other) {
            (Value::Null, Value::Null) => true,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::Int(a), Value::Int(b)) => a == b,
            (Value::F32(a), Value::F32(b)) => a.to_bits() == b.to_bits(),
            (Value::Str(a), Value::Str(b)) => a == b,
            (Value::Id(a), Value::Id(b)) => a == b,
            (Value::Xform(a), Value::Xform(b)) => a == b,
            _ => false,
        }
    }
}

impl Eq for Value {}

impl Value {
    /// A string value.
    pub fn str(s: impl AsRef<[u8]>) -> Value {
        Value::Str(s.as_ref().to_vec())
    }

    /// A transform value: `None` is the identity, else the fields `writeAffineTransform` sends of the position and unit
    /// quaternion.
    pub fn xform(t: Option<&Affine>) -> Value {
        Value::Xform(t.map(|a| {
            [
                a.position[0].to_bits(),
                a.position[1].to_bits(),
                a.position[2].to_bits(),
                a.quaternion[0].to_bits(),
                a.quaternion[1].to_bits(),
                a.quaternion[2].to_bits(),
                u32::from(a.quaternion[3] < 0.0),
            ]
        }))
    }
}
