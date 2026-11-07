//! A `WaterBlock` as water the Players swim in: on a game's server, and the same water in the client's prediction.
//!
//! - **The block** ([`Water`]): its box (the unit cube from its position, scaled and turned as the block is, its surface the
//!   box's top), its `density`, `viscosity` and `liquidType`, as the water zone a Player's container update finds
//!   ([`torque_sim::player::host::ZoneView`]): the coverage, the buoyancy and the drag it gives, and the swim pose
//!   ([`torque_sim::player::pose::swims`]).
//! - **On the server** ([`install`]): a `WaterBlock` added by a mission enters the container as water (`WaterObjectType`) with
//!   that zone ([`torque_game::scene::ext::MapObject::water`]). It has no collision: Players, vehicles and items pass into it,
//!   and no ray meets it (a camera's cast goes through its surface).
//! - **On the client** ([`Water::from_fields`]): the client is not sent a WaterBlock, so a game reads it from the map the server
//!   runs and gives it to the prediction (its static index's `torque_client::predict::StaticIndex::zones`). The block's
//!   fields are read as the server's script engine reads them (the same parse and the same rotation, the engine's default
//!   field rules: a server with its own `FieldRules::rotation_matrix` would turn a rotated block otherwise), so the two
//!   zones are the same numbers and the client predicts its own Player in the water as the server moves it.
//!
//! Credit: the box and the fields follow `WaterBlock` and `ShapeBase::updateContainer`'s water callback in TGE 1.4 as Torque3D
//! carries them under the MIT license (`Engine/source/environment/waterBlock.cpp`, `T3D/shapeBase.cpp`; Copyright (c) 2012
//! GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.). The code here is this crate's own.

use std::rc::Rc;

use torque_collide::ObjectId;
use torque_collide::convex::{ObjectView, view_of};
use torque_collide::math::{Box3, Matrix, Plane, PlaneTransformer};
use torque_collide::types::WATER;
use torque_game::scene::access::{matrix_of, scale_of, value_of};
use torque_game::scene::container::Container;
use torque_game::scene::{ext, object as scene};
use torque_sim::player::host::{MAX_LIQUID_DENSITY, MAX_LIQUID_VISCOSITY, ZoneView};
use torque_ts_vm::classes::EngineClass;
use torque_ts_vm::fields::{Value, rotation_matrix};
use torque_ts_vm::sim::ObjId;
use torque_ts_vm::text::{atof, scan_floats, to_f32};
use torque_ts_vm::vm::{Vm, VmError};

/// A water block's type bits (`WaterObjectType`).
pub const WATER_TYPE_MASK: u32 = WATER;
/// The farthest from the origin a water block's box may reach (m).
pub const MAX_EXTENT: f32 = 1.0e6;
/// The color a water zone gives a Player (the container update's own).
pub const WATER_COLOR: [f32; 4] = [0.2, 0.6, 0.6, 0.3];

/// A water block: its zone and its container view (module docs).
#[derive(Debug, Clone, PartialEq)]
pub struct Water {
    zone: ZoneView,
    view: ObjectView,
}

impl Water {
    /// The water of a block placed by `m` (its transform) and `scale`, of `density`, `viscosity` and liquid type `liquid_type`.
    /// None when its placement or its box is not finite numbers, or its box reaches past [`MAX_EXTENT`] from the origin, or
    /// its density or viscosity is not within `0..=` [`MAX_LIQUID_DENSITY`] and [`MAX_LIQUID_VISCOSITY`] (a map is not
    /// trusted: such values overflow the buoyancy and the drag).
    pub fn new(
        m: Matrix,
        scale: [f32; 3],
        density: f32,
        viscosity: f32,
        liquid_type: u32,
    ) -> Option<Water> {
        let unit = Box3::new([0.0; 3], [1.0; 3]);
        let view = view_of(m, unit, scale, WATER_TYPE_MASK);
        let w = view.world_box;
        // The placement too: a box that does not come out ordered is put at the origin by the view.
        let finite = w
            .min
            .iter()
            .chain(&w.max)
            .chain(&m)
            .chain(&scale)
            .all(|v| v.is_finite())
            && w.min.iter().chain(&w.max).all(|v| v.abs() <= MAX_EXTENT)
            && (0.0..=MAX_LIQUID_DENSITY).contains(&density)
            && (0.0..=MAX_LIQUID_VISCOSITY).contains(&viscosity);
        if !finite {
            return None;
        }
        // The object box's six planes moved to the world.
        let pt = PlaneTransformer::new(&m, scale);
        let planes = [
            Plane::new(-1.0, 0.0, 0.0, 0.0),
            Plane::new(1.0, 0.0, 0.0, -1.0),
            Plane::new(0.0, -1.0, 0.0, 0.0),
            Plane::new(0.0, 1.0, 0.0, -1.0),
            Plane::new(0.0, 0.0, -1.0, 0.0),
            Plane::new(0.0, 0.0, 1.0, -1.0),
        ]
        .iter()
        .map(|p| pt.transform(p))
        .collect();
        let zone = ZoneView {
            id: ObjectId(0),
            active: true,
            world_box: w,
            planes,
            enabled: true,
            velocity_mod: 1.0,
            gravity_mod: 1.0,
            drag: 0.0,
            applied_force: [0.0; 3],
            water: true,
            viscosity,
            density,
            color: WATER_COLOR,
            liquid_type,
        };
        Some(Water { zone, view })
    }

    /// The water of a block whose fields `field` gives as text (`position`, `rotation`, `scale`, `density`, `viscosity`,
    /// `liquidType`; a field it does not give has the class's default), each read as the server's script engine reads it.
    pub fn from_fields(field: &dyn Fn(&str) -> Option<String>) -> Option<Water> {
        let text = |name: &str, default: &str| field(name).unwrap_or_else(|| default.to_string());
        let mut position = [0.0, 0.0, 0.0, 1.0];
        scan_floats(text("position", "0 0 0").as_bytes(), &mut position);
        let mut aa = [0.0f32; 4];
        scan_floats(text("rotation", "1 0 0 0").as_bytes(), &mut aa);
        let mut scale = [1.0f32; 3];
        scan_floats(text("scale", "1 1 1").as_bytes(), &mut scale);
        let rows = rotation_matrix(aa);
        let mut m: Matrix = std::array::from_fn(|i| rows[i / 4][i % 4]);
        for (r, v) in position.iter().enumerate() {
            m[4 * r + 3] = *v;
        }
        let number = |name: &str, default: &str| to_f32(atof(text(name, default).as_bytes()));
        let kind = text("liquidType", "OceanWater");
        let liquid_type = torque_game::specs::LIQUID_TYPES
            .entries
            .iter()
            .find(|(_, label)| label.eq_ignore_ascii_case(kind.trim()))
            .map_or(0, |(n, _)| *n as u32);
        Water::new(
            m,
            scale,
            number("density", "1"),
            number("viscosity", "15"),
            liquid_type,
        )
    }

    /// The zone a Player's container update finds the water as, as object `id`.
    pub fn zone(&self, id: ObjectId) -> ZoneView {
        ZoneView {
            id,
            ..self.zone.clone()
        }
    }

    /// The water's surface: its box's top.
    pub fn level(&self) -> f32 {
        self.zone.world_box.max[2]
    }

    /// The box the water fills.
    pub fn world_box(&self) -> Box3 {
        self.zone.world_box
    }

    /// The block's container view: its box, [`WATER_TYPE_MASK`].
    pub fn view(&self) -> ObjectView {
        self.view
    }
}

// ---- the server

/// The `WaterBlock` class on a server: when a mission adds one, it enters the container as water (module docs).
struct WaterClass {
    container: Rc<Container>,
}

impl EngineClass for WaterClass {
    fn on_add(&self, vm: &mut Vm, id: ObjId) -> Result<bool, VmError> {
        let float = |vm: &Vm, name: &'static [u8], default: f32| match value_of(vm, id, name) {
            Some(Value::Float(x)) => x,
            _ => default,
        };
        let liquid_type = match value_of(vm, id, b"liquidType") {
            Some(Value::Int(n)) => n as u32,
            _ => 0,
        };
        let water = matrix_of(vm, id).and_then(|m| {
            Water::new(
                m,
                scale_of(vm, id),
                float(vm, b"density", 1.0),
                float(vm, b"viscosity", 15.0),
                liquid_type,
            )
        });
        let Some(water) = water else {
            vm.error("WaterBlock: its box is not finite numbers".to_string());
            return Ok(true);
        };
        let view = water.view();
        let w = view.world_box;
        if let Some(e) = ext::get_mut(vm.objects_mut(), id) {
            e.type_mask = view.type_mask;
            if let Some(s) = e.scene.as_mut() {
                s.object_min = view.obj_box.min;
                s.object_max = view.obj_box.max;
                s.world_min = w.min;
                s.world_max = w.max;
                s.world_center = [0, 1, 2].map(|k| (w.min[k] + w.max[k]) * 0.5);
                s.world_radius = view.world_radius;
            }
            e.map = Some(ext::MapObject {
                view,
                shape: None,
                water: Some(water.zone(ObjectId(id))),
            });
        }
        scene::add_to_scene(vm, &self.container, id);
        Ok(true)
    }
}

/// Gives `WaterBlock` its water on a server whose game classes put their objects in `container`.
pub fn install(vm: &mut Vm, container: &Rc<Container>) {
    vm.set_class_behaviour(
        "WaterBlock",
        Rc::new(WaterClass {
            container: Rc::clone(container),
        }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The valley's lake as the map writes it: its box from the position up the scale, the surface at its top, and its planes
    /// bounding the box.
    #[test]
    fn a_block_s_fields_give_its_box_and_its_water() {
        let fields = |name: &str| match name {
            "position" => Some("-7000 -3500 0".to_string()),
            "rotation" => Some("1 0 0 0".to_string()),
            "scale" => Some("7170 7000 30".to_string()),
            "density" => Some("1.5".to_string()),
            _ => None,
        };
        let w = Water::from_fields(&fields).expect("the water");
        assert_eq!(w.level(), 30.0);
        assert_eq!(
            w.world_box(),
            Box3::new([-7000.0, -3500.0, 0.0], [170.0, 3500.0, 30.0])
        );
        let z = w.zone(ObjectId(7));
        assert_eq!(
            (z.id, z.water, z.density, z.viscosity, z.liquid_type),
            (ObjectId(7), true, 1.5, 15.0, 1)
        );
        let inside = [0.0, 0.0, 10.0];
        let above = [0.0, 0.0, 31.0];
        let side = |p: [f32; 3]| {
            z.planes
                .iter()
                .all(|pl| pl.x * p[0] + pl.y * p[1] + pl.z * p[2] + pl.d <= 0.0)
        };
        assert!(side(inside) && !side(above));
        assert_eq!(w.view().type_mask, WATER_TYPE_MASK);
        // A hostile block leaves no water: a box not finite or too far out, a density or viscosity past its cap or below 0.
        for (name, value) in [
            ("scale", "inf 1 1"),
            ("scale", "1 nan 1"),
            ("position", "2e6 0 0"),
            ("scale", "1 1 3e38"),
            ("rotation", "1 0 0 nan"),
            ("density", "3e38"),
            ("density", "-1"),
            ("density", "nan"),
            ("viscosity", "1e38"),
            ("viscosity", "-1e38"),
        ] {
            let bad = |n: &str| (n == name).then(|| value.to_string());
            assert!(Water::from_fields(&bad).is_none(), "{name} = {value}");
        }
        // At the caps it is water.
        let dense = |n: &str| match n {
            "density" => Some(MAX_LIQUID_DENSITY.to_string()),
            "viscosity" => Some(MAX_LIQUID_VISCOSITY.to_string()),
            _ => None,
        };
        assert!(Water::from_fields(&dense).is_some());
    }
}
