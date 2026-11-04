//! The server's objects as the ghost layer reads them: which script objects are ghosted and with which TGE class
//! ([`ghost_class`]), one object's fields as an [`ObjectView`] ([`ScriptView`]), and the mask bits a moved object marks
//! ([`moved_mask`]).
//!
//! The example reads the objects' fields as the scripts set them (`position`, `rotation`, `scale`, `dataBlock`, ...), as text.
//! A game whose objects are torque-game's scene classes reads their transforms instead, and marks its objects dirty as they
//! change through torque-game's `GhostLink`.

use torque::ghosts::shapes::{CAMERA_MOVE_MASK, STATIC_POSITION_MASK, item_mask};
use torque::ghosts::{Link, ObjectView, TgeClass, Value};
use torque::net::ghost::ObjId;
use torque::script::text::atof;
use torque::script::vm::vm::Vm;

/// The classes the example ghosts. Every other object (datablocks, groups, script objects) stays on the server.
pub const GHOSTED: [TgeClass; 3] = [TgeClass::StaticShape, TgeClass::Item, TgeClass::Camera];

/// The TGE class an object is ghosted as, by its console class's name; `None` for an object the example does not ghost.
pub fn ghost_class(vm: &Vm, id: ObjId) -> Option<TgeClass> {
    let objects = vm.objects();
    let object = objects.get(id)?;
    let name = objects.classes.name(object.class());
    GHOSTED
        .into_iter()
        .find(|class| class.name().as_bytes().eq_ignore_ascii_case(name))
}

/// The mask bits a class's `setTransform` marks: what an object whose position, rotation or scale changed sends again.
pub fn moved_mask(class: TgeClass) -> u32 {
    match class {
        TgeClass::Item => item_mask::POSITION,
        TgeClass::Camera => CAMERA_MOVE_MASK,
        _ => STATIC_POSITION_MASK,
    }
}

/// The fields whose change [`moved_mask`] sends.
pub fn placement(vm: &Vm, id: ObjId) -> Vec<Vec<u8>> {
    [&b"position"[..], b"rotation", b"scale"]
        .iter()
        .map(|name| vm.objects().field(id, name, b"").unwrap_or_default())
        .collect()
}

/// One script object as a connection's ghost record reads it.
pub struct ScriptView<'a> {
    pub vm: &'a Vm,
    pub id: ObjId,
    pub class: TgeClass,
}

impl ScriptView<'_> {
    fn text(&self, name: &str) -> Vec<u8> {
        self.vm
            .objects()
            .field(self.id, name.as_bytes(), b"")
            .unwrap_or_default()
    }

    /// Word `index` of a field as a float (0 when the field has fewer words).
    fn word(&self, name: &str, index: usize) -> f32 {
        let text = self.text(name);
        text.split(|b| b.is_ascii_whitespace())
            .filter(|w| !w.is_empty())
            .nth(index)
            .map_or(0.0, |w| atof(w) as f32)
    }

    fn words<const N: usize>(&self, name: &str) -> [f32; N] {
        std::array::from_fn(|i| self.word(name, i))
    }

    fn flag(&self, name: &str) -> bool {
        let text = self.text(name);
        !text.is_empty() && atof(&text) != 0.0
    }

    /// The datablock's object id (the field holds its name or its id).
    fn datablock(&self) -> Option<u32> {
        let text = self.text("dataBlock");
        (!text.is_empty())
            .then(|| self.vm.objects().find(&text))
            .flatten()
    }

    /// The transform as `writeAffineTransform` sends it: the position, the rotation's quaternion x, y, z and `w < 0`.
    fn transform(&self) -> Value {
        let position: [f32; 3] = self.words("position");
        let quat = quat_from_axis_angle(self.words("rotation"));
        Value::Xform(Some([
            position[0].to_bits(),
            position[1].to_bits(),
            position[2].to_bits(),
            quat[0].to_bits(),
            quat[1].to_bits(),
            quat[2].to_bits(),
            u32::from(quat[3] < 0.0),
        ]))
    }
}

impl ObjectView for ScriptView<'_> {
    fn get(&self, field: &str, index: usize) -> Value {
        match field {
            "datablock" => Value::Id(self.datablock()),
            "position" => Value::F32(self.word("position", index)),
            // A Camera's rotation is its pitch and yaw; the example's cameras look straight ahead.
            "rotation" => Value::F32(0.0),
            "scale" => Value::F32(self.word("scale", index)),
            "transform" => self.transform(),
            "static" | "rotate" | "collidable" => Value::Bool(self.flag(field)),
            // No velocity: an Item at rest.
            "at_rest" => Value::Bool(true),
            _ => Value::Null,
        }
    }

    fn link(&self, _field: &str) -> Link {
        Link::Null
    }
}

/// A rotation given as an axis and an angle in degrees (the `rotation` field's text) as the unit quaternion `x, y, z, w`
/// (`QuatF::set(AngAxisF)`). A zero axis is no rotation.
pub fn quat_from_axis_angle(rotation: [f32; 4]) -> [f32; 4] {
    let [x, y, z, degrees] = rotation;
    let length = (x * x + y * y + z * z).sqrt();
    if length == 0.0 {
        return [0.0, 0.0, 0.0, 1.0];
    }
    let half = degrees.to_radians() * 0.5;
    let s = half.sin() / length;
    [x * s, y * s, z * s, half.cos()]
}

#[cfg(test)]
mod tests {
    use super::quat_from_axis_angle;

    #[test]
    fn a_quarter_turn_about_z_is_the_half_angle_quaternion() {
        let q = quat_from_axis_angle([0.0, 0.0, 2.0, 90.0]);
        let h = std::f32::consts::FRAC_1_SQRT_2;
        for (a, b) in q.iter().zip([0.0, 0.0, h, h]) {
            assert!((a - b).abs() < 1e-6, "{q:?}");
        }
        assert_eq!(
            quat_from_axis_angle([0.0, 0.0, 0.0, 30.0]),
            [0.0, 0.0, 0.0, 1.0]
        );
    }
}
