//! A datablock record as the client's script fields: [`fields`] walks a [`TgeDatablockRecord`]'s layout and names each value
//! by the script field its member is registered as ([`TgeDatablockClass::script_field`]), as a client that made the datablock
//! from the event would hold it.
//!
//! **What a value becomes.** A flag is a [`Value::Bool`], an integer, a raw byte or word and a ranged value a [`Value::Int`], a
//! float a [`Value::F32`] (each component of a vector one element: `3 * i + c`), a string a [`Value::Str`], a transform a
//! [`Value::Xform`], a datablock reference a [`Value::Id`]. An optional block that was not sent names nothing (the client keeps
//! the field's default, as `unpackData` leaves the member alone); a sent one's presence flag is replaced by the value of the
//! same name that follows it. A name met again counts as the next element.

use super::{TgeDatablockClass, TgeDatablockRecord};
use crate::Value;
use crate::codec::{Codec, F32, NetWidths, R, Xf};
use crate::datablocks::ScriptField;

/// The fields of `record` as (script field, element, value), in the order the layout meets them.
pub fn fields(record: &TgeDatablockRecord, widths: NetWidths) -> Vec<(String, u32, Value)> {
    // A plugin's record names its own fields.
    if let TgeDatablockRecord::Plugin(_, record) = record {
        return record.record_fields();
    }
    let mut log = FieldLog {
        widths,
        values: Vec::new(),
        last: None,
    };
    // The recorder never fails.
    let _ = record.visit(&mut log);
    let class = record.class();
    log.values
        .into_iter()
        .filter_map(|(member, index, value)| {
            let (name, index) = script_name(class, member, index)?;
            Some((name, index, value))
        })
        .collect()
}

fn script_name(class: TgeDatablockClass, member: &str, index: u32) -> Option<(String, u32)> {
    match class.script_field(member, index as usize) {
        ScriptField::Own => Some((member.to_string(), index)),
        ScriptField::Field(name, i) => Some((name.to_string(), i as u32)),
        ScriptField::Absent => None,
    }
}

/// The recording codec.
struct FieldLog {
    widths: NetWidths,
    values: Vec<(&'static str, u32, Value)>,
    /// The name of the last flag recorded (the value after it with the same name replaces it).
    last: Option<&'static str>,
}

impl FieldLog {
    fn push(&mut self, name: &'static str, value: Value) {
        if self.last.take() == Some(name)
            && let Some(slot) = self.values.last_mut()
        {
            slot.2 = value;
            return;
        }
        let index = self.values.iter().filter(|(n, ..)| *n == name).count() as u32;
        self.values.push((name, index, value));
    }
}

impl Codec for FieldLog {
    fn flag(&mut self, name: &'static str, v: &mut bool) -> R {
        self.push(name, Value::Bool(*v));
        self.last = Some(name);
        Ok(())
    }

    /// An absent block names nothing (the client keeps the field's default); a present one's flag is replaced by the value
    /// of the same name that follows, if one does.
    fn presence(&mut self, name: &'static str, v: &mut bool) -> R {
        if *v {
            self.flag(name, v)?;
        }
        Ok(())
    }

    fn int(&mut self, name: &'static str, v: &mut u32, _bits: u8) -> R {
        self.push(name, Value::Int(i64::from(*v)));
        Ok(())
    }

    fn raw8(&mut self, name: &'static str, v: &mut u8) -> R {
        self.push(name, Value::Int(i64::from(*v)));
        Ok(())
    }

    fn raw32(&mut self, name: &'static str, v: &mut u32) -> R {
        self.push(name, Value::Int(i64::from(*v as i32)));
        Ok(())
    }

    fn float32(&mut self, name: &'static str, v: &mut u32) -> R {
        self.push(name, Value::F32(f32::from_bits(*v)));
        Ok(())
    }

    fn floats<const N: usize>(&mut self, name: &'static str, v: &mut [F32; N]) -> R {
        for f in v.iter() {
            self.push(name, Value::F32(f.get()));
        }
        Ok(())
    }

    fn xform(&mut self, name: &'static str, v: &mut Xf) -> R {
        let p = v.position.map(|f| f.0);
        let q = v.quat.map(|f| f.0);
        self.push(
            name,
            Value::Xform(Some([
                p[0],
                p[1],
                p[2],
                q[0],
                q[1],
                q[2],
                u32::from(v.negative_w),
            ])),
        );
        Ok(())
    }

    fn string(&mut self, name: &'static str, v: &mut Vec<u8>) -> R {
        self.push(name, Value::Str(v.clone()));
        Ok(())
    }

    fn ranged(&mut self, name: &'static str, v: &mut u32, _lo: u32, _hi: u32) -> R {
        self.push(name, Value::Int(i64::from(*v)));
        Ok(())
    }

    fn datablock_ref(&mut self, name: &'static str, id: &mut u32, _lo: u32, _hi: u32) -> R {
        self.push(name, Value::Id(Some(*id)));
        Ok(())
    }

    fn seq<T: Default>(
        &mut self,
        _name: &'static str,
        v: &mut Vec<T>,
        _bits: u8,
        _max: usize,
        _min_bits_each: usize,
        mut each: impl FnMut(&mut Self, &mut T) -> R,
    ) -> R {
        v.iter_mut().try_for_each(|x| each(self, x))
    }

    fn widths(&self) -> NetWidths {
        self.widths
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ObjectView;
    use crate::datablocks::shapes::{StaticShapeData, pack_static_shape_data};

    /// A view of a StaticShapeData: its shape file, its mass, nothing else.
    struct Crate;

    impl ObjectView for Crate {
        fn get(&self, field: &str, _index: usize) -> Value {
            match field {
                "shapeName" => Value::str("kit/cube.dts"),
                "mass" => Value::F32(4.0),
                "explosion" => Value::Id(Some(40)),
                _ => Value::Null,
            }
        }
    }

    #[test]
    fn a_records_members_come_back_as_script_fields() {
        let widths = NetWidths::engine_default();
        let record: StaticShapeData = pack_static_shape_data(&Crate, widths);
        let fields = fields(&TgeDatablockRecord::StaticShapeData(record), widths);
        let get = |name: &str| {
            fields
                .iter()
                .find(|(n, i, _)| n == name && *i == 0)
                .map(|(_, _, v)| v.clone())
        };
        assert_eq!(get("shapeFile"), Some(Value::str("kit/cube.dts")));
        assert_eq!(get("mass"), Some(Value::F32(4.0)));
        assert_eq!(get("explosion"), Some(Value::Id(Some(40))));
        // An absent value names nothing; the shape's own name is not a script field.
        assert_eq!(get("drag"), None);
        assert_eq!(get("shapeName"), None);
    }
}
