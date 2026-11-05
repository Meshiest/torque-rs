//! Mission objects that come with an interior: a `.mis` fragment of its `InteriorInstance` and the objects its source names
//! beside it (water, triggers, physical zones, spawn points), placed with it.
//!
//! In Torque a `WaterBlock`, a `Trigger` and a `PhysicalZone` are objects of the mission, not interior geometry. A volume is
//! written as its class lays out its box: a `WaterBlock`'s object box is the unit cube from the origin (its position the
//! volume's low corner, its scale the size), a `Trigger`'s and a `PhysicalZone`'s polyhedron is the default
//! `0 0 0 1 0 0 0 -1 0 0 0 1` (x from 0 to 1, y from -1 to 0, z from 0 to 1: the position is the corner of low x, high y and low
//! z). Each object is placed as the interior is: moved, turned and scaled with it.
//!
//! Credit: the box layouts follow TGE as Torque3D keeps it: `WaterBlock`'s object box in `environment/waterBlock.cpp`
//! (Copyright (c) 2012 GarageGames, LLC), the `Trigger` polyhedron field in `T3D/trigger.cpp` (Copyright (c) 2012
//! GarageGames, LLC) and the `PhysicalZone` one in `T3D/physicalZone.cpp` (Copyright (c) 2012 GarageGames, LLC; Copyright
//! (C) 2015 Faust Logic, Inc.); the code here is this crate's own.

use crate::geom::{self, V3};

/// The polyhedron `Trigger` and `PhysicalZone` default to.
pub const UNIT_POLYHEDRON: &str = "0 0 0 1 0 0 0 -1 0 0 0 1";

/// What an object is.
#[derive(Debug, Clone, PartialEq)]
pub enum Shape {
    /// A point (its position).
    Point(V3),
    /// A box volume, low and high corners.
    Volume(V3, V3),
}

/// An object of the mission that comes with the interior, in the interior's space.
#[derive(Debug, Clone, PartialEq)]
pub struct MissionObject {
    /// The script class (`WaterBlock`, `Trigger`, `PhysicalZone`, `SpawnSphere`, ...).
    pub class: String,
    pub name: Option<String>,
    pub shape: Shape,
    /// A turn about z in degrees (a point object's facing).
    pub yaw: f64,
    /// Fields written as given (`dataBlock`, `velocityMod`, ...).
    pub fields: Vec<(String, String)>,
}

/// Where the interior stands in the mission: its position, its rotation (an axis and an angle in degrees, the `rotation`
/// field) and its scale.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    pub position: V3,
    pub axis: V3,
    pub angle: f64,
    pub scale: V3,
}

impl Default for Placement {
    fn default() -> Placement {
        Placement {
            position: [0.0; 3],
            axis: [0.0, 0.0, 1.0],
            angle: 0.0,
            scale: [1.0; 3],
        }
    }
}

impl Placement {
    /// `p` of the interior's space in the mission's.
    pub fn place(&self, p: V3) -> V3 {
        let s = [0, 1, 2].map(|k| p[k] * self.scale[k]);
        geom::add(self.position, rotate(s, self.axis, self.angle.to_radians()))
    }
}

/// `v` turned by `angle` radians about `axis` (Rodrigues).
fn rotate(v: V3, axis: V3, angle: f64) -> V3 {
    let Some(k) = geom::normalize(axis) else {
        return v;
    };
    let (s, c) = angle.sin_cos();
    let kv = geom::cross(k, v);
    let kd = geom::dot(k, v);
    [0, 1, 2].map(|i| v[i] * c + kv[i] * s + k[i] * kd * (1.0 - c))
}

/// A number as a mission writes it: six places at most, no trailing zeros.
pub fn num(v: f64) -> String {
    let v = if v.abs() < 5e-7 { 0.0 } else { v };
    let s = format!("{v:.6}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    s.to_string()
}

fn vec3(v: V3) -> String {
    format!("{} {} {}", num(v[0]), num(v[1]), num(v[2]))
}

/// A TorqueScript string literal's inside.
fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {}
            c => out.push(c),
        }
    }
    out
}

/// A field or object name a script can carry (letters, digits and `_`, not starting with a digit).
pub fn is_name(s: &str) -> bool {
    let mut chars = s.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

impl MissionObject {
    /// The object as a mission's `new` statement, placed by `at`.
    pub fn text(&self, at: &Placement) -> String {
        let name = self
            .name
            .as_deref()
            .filter(|n| is_name(n))
            .unwrap_or_default();
        let mut lines = vec![format!("new {}({name}) {{", self.class)];
        let rotation = |extra: f64| {
            if extra == 0.0 {
                format!("{} {}", vec3(at.axis), num(at.angle))
            } else if at.angle == 0.0 {
                format!("0 0 1 {}", num(extra))
            } else {
                // The interior's turn after the object's own about z, as one axis and angle.
                let (qa, qb) = (quat([0.0, 0.0, 1.0], extra), quat(at.axis, at.angle));
                let q = quat_mul(qb, qa);
                let (axis, angle) = axis_angle(q);
                format!("{} {}", vec3(axis), num(angle))
            }
        };
        match self.shape {
            Shape::Point(p) => {
                lines.push(format!("   position = \"{}\";", vec3(at.place(p))));
                lines.push(format!("   rotation = \"{}\";", rotation(self.yaw)));
            }
            Shape::Volume(lo, hi) => {
                let size = [0, 1, 2].map(|k| (hi[k] - lo[k]) * at.scale[k]);
                let corner = if self.class.eq_ignore_ascii_case("WaterBlock") {
                    lo
                } else {
                    [lo[0], hi[1], lo[2]]
                };
                lines.push(format!("   position = \"{}\";", vec3(at.place(corner))));
                lines.push(format!("   rotation = \"{}\";", rotation(0.0)));
                lines.push(format!("   scale = \"{}\";", vec3(size)));
                if !self.class.eq_ignore_ascii_case("WaterBlock")
                    && !self
                        .fields
                        .iter()
                        .any(|(k, _)| k.eq_ignore_ascii_case("polyhedron"))
                {
                    lines.push(format!("   polyhedron = \"{UNIT_POLYHEDRON}\";"));
                }
            }
        }
        for (k, v) in &self.fields {
            if is_name(k) {
                lines.push(format!("   {k} = \"{}\";", escape(v)));
            }
        }
        lines.push("};".to_string());
        lines.join("\n")
    }
}

fn quat(axis: V3, degrees: f64) -> [f64; 4] {
    let k = geom::normalize(axis).unwrap_or([0.0, 0.0, 1.0]);
    let (s, c) = (degrees.to_radians() * 0.5).sin_cos();
    [k[0] * s, k[1] * s, k[2] * s, c]
}

fn quat_mul(a: [f64; 4], b: [f64; 4]) -> [f64; 4] {
    [
        a[3] * b[0] + a[0] * b[3] + a[1] * b[2] - a[2] * b[1],
        a[3] * b[1] - a[0] * b[2] + a[1] * b[3] + a[2] * b[0],
        a[3] * b[2] + a[0] * b[1] - a[1] * b[0] + a[2] * b[3],
        a[3] * b[3] - a[0] * b[0] - a[1] * b[1] - a[2] * b[2],
    ]
}

fn axis_angle(q: [f64; 4]) -> (V3, f64) {
    let w = q[3].clamp(-1.0, 1.0);
    let angle = 2.0 * w.acos();
    match geom::normalize([q[0], q[1], q[2]]) {
        Some(axis) => (axis, angle.to_degrees()),
        None => ([0.0, 0.0, 1.0], 0.0),
    }
}

/// The interior's `InteriorInstance` named `name` (when a script can carry it) over `file`, then `objects`, all placed by
/// `at`: a fragment to paste into a mission's `MissionGroup`.
pub fn fragment(name: &str, file: &str, at: &Placement, objects: &[MissionObject]) -> String {
    let mut out = vec![
        format!(
            "new InteriorInstance({}) {{",
            if is_name(name) { name } else { "" }
        ),
        format!("   position = \"{}\";", vec3(at.position)),
        format!("   rotation = \"{} {}\";", vec3(at.axis), num(at.angle)),
        format!("   scale = \"{}\";", vec3(at.scale)),
        format!("   interiorFile = \"{}\";", escape(file)),
        "};".to_string(),
    ];
    out.extend(objects.iter().map(|o| o.text(at)));
    let mut text = out.join("\n");
    text.push('\n');
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volumes_take_their_class_box_and_move_with_the_interior() {
        let at = Placement {
            position: [10.0, 20.0, 1.0],
            ..Placement::default()
        };
        let water = MissionObject {
            class: "WaterBlock".into(),
            name: Some("Pool".into()),
            shape: Shape::Volume([0.0, 0.0, -2.0], [4.0, 3.0, 0.0]),
            yaw: 0.0,
            fields: vec![("liquidType".into(), "OceanWater".into())],
        };
        let t = water.text(&at);
        assert!(t.starts_with("new WaterBlock(Pool) {"), "{t}");
        assert!(t.contains("position = \"10 20 -1\";"), "{t}");
        assert!(t.contains("scale = \"4 3 2\";"), "{t}");
        assert!(t.contains("liquidType = \"OceanWater\";"), "{t}");
        let trigger = MissionObject {
            class: "Trigger".into(),
            name: None,
            shape: Shape::Volume([0.0, 0.0, 0.0], [1.0, 2.0, 3.0]),
            yaw: 0.0,
            fields: vec![("dataBlock".into(), "Door\"Trigger".into())],
        };
        let t = trigger.text(&at);
        assert!(t.contains("position = \"10 22 1\";"), "{t}");
        assert!(t.contains(UNIT_POLYHEDRON), "{t}");
        assert!(t.contains("dataBlock = \"Door\\\"Trigger\";"), "{t}");
    }

    #[test]
    fn a_turned_interior_turns_its_objects() {
        let at = Placement {
            position: [0.0; 3],
            axis: [0.0, 0.0, 1.0],
            angle: 90.0,
            scale: [2.0; 3],
        };
        let p = at.place([1.0, 0.0, 0.0]);
        assert!((p[0]).abs() < 1e-12 && (p[1] - 2.0).abs() < 1e-12);
        let spawn = MissionObject {
            class: "SpawnSphere".into(),
            name: None,
            shape: Shape::Point([1.0, 0.0, 0.0]),
            yaw: 90.0,
            fields: Vec::new(),
        };
        let t = spawn.text(&at);
        assert!(t.contains("position = \"0 2 0\";"), "{t}");
        assert!(t.contains("rotation = \"0 0 1 180\";"), "{t}");
        assert_eq!(num(0.1 + 0.2), "0.3");
        assert_eq!(num(-0.0000001), "0");
    }
}
