//! A test host: an empty container, a ground plane at z = 0 for the rays, a counting budget.
#![allow(dead_code)]

use torque_collide::math::{Box3, F3};
use torque_collide::{ConvexSource, Exhausted, ObjectId, ObjectView, World};
use torque_vehicle::VehicleState;
use torque_vehicle::host::{RayHit, VehicleHost};

#[derive(Default)]
pub struct Ground {
    pub rays: usize,
    pub charged: u64,
    pub callbacks: Vec<String>,
    pub time: u32,
    pub no_ground: bool,
    /// The WheeledVehicle subclass the host hands the step (none: Torque's).
    pub class: Option<std::rc::Rc<dyn torque_vehicle::wheeled::WheeledClass>>,
}

impl World for Ground {
    fn object(&self, _id: ObjectId) -> Option<ObjectView> {
        None
    }
    fn find_objects(
        &mut self,
        _query: &Box3,
        _mask: u32,
        _exclude: Option<ObjectId>,
        _limit: usize,
        _out: &mut Vec<ObjectId>,
    ) -> Result<(), Exhausted> {
        Ok(())
    }
    fn convex_source(&self, _id: ObjectId) -> ConvexSource {
        ConvexSource::None
    }
    fn hull(&self, _id: ObjectId, _slot: u32) -> Option<std::rc::Rc<torque_collide::mesh::Hull>> {
        None
    }
    fn charge(&mut self, units: u64) -> bool {
        self.charged = self.charged.saturating_add(units);
        true
    }
}

impl VehicleHost for Ground {
    fn world(&mut self) -> &mut dyn World {
        self
    }
    fn sim_time(&self) -> u32 {
        self.time
    }
    fn wheeled_class(&self) -> Option<std::rc::Rc<dyn torque_vehicle::wheeled::WheeledClass>> {
        self.class.clone()
    }
    /// The plane z = 0 from above: the fraction `start.z / (start.z - end.z)`.
    fn cast_ray(&mut self, start: F3, end: F3, _mask: u32) -> Option<RayHit> {
        self.rays += 1;
        if self.no_ground || !(start[2] >= 0.0 && end[2] < 0.0) {
            return None;
        }
        let t = (f64::from(start[2]) / (f64::from(start[2]) - f64::from(end[2]))) as f32;
        let lerp =
            |a: f32, b: f32| (f64::from(a) + (f64::from(b) - f64::from(a)) * f64::from(t)) as f32;
        Some(RayHit {
            t,
            point: [lerp(start[0], end[0]), lerp(start[1], end[1]), 0.0],
            normal: [0.0, 0.0, 1.0],
            material: 0,
            object: None,
        })
    }
    fn on_impact(&mut self, _s: &mut VehicleState, _vec: F3) {
        self.callbacks.push("impact".into());
    }
    fn wheeled_body_contact(
        &mut self,
        _s: &mut VehicleState,
        _wheels: &torque_vehicle::wheeled::Wheeled,
    ) {
        self.callbacks.push("body contact".into());
    }
}

/// A box mesh of half-size `h` around the origin: 8 corners, 12 triangles facing out.
pub fn box_hull(h: F3) -> torque_collide::mesh::Hull {
    let v = |i: u32| -> F3 {
        [
            if i & 1 != 0 { h[0] } else { -h[0] },
            if i & 2 != 0 { h[1] } else { -h[1] },
            if i & 4 != 0 { h[2] } else { -h[2] },
        ]
    };
    let verts: Vec<F3> = (0..8).map(v).collect();
    let tris = vec![
        [0, 2, 1],
        [1, 2, 3],
        [4, 5, 6],
        [5, 7, 6],
        [0, 1, 4],
        [1, 5, 4],
        [2, 6, 3],
        [3, 6, 7],
        [0, 4, 2],
        [2, 4, 6],
        [1, 3, 5],
        [3, 7, 5],
    ];
    let materials = vec![0; tris.len()];
    torque_collide::mesh::Hull::new(
        None,
        vec![torque_collide::mesh::MeshPart {
            node: torque_collide::math::IDENTITY,
            verts,
            tris,
            materials,
        }],
    )
}

/// A floor: one box object (a PhysicalZone-style box convex) of half-size `half` centred at `centre`, and the vehicle's body
/// hull (slot 0 of object `vehicle`).
pub struct Floor {
    pub ground: Ground,
    pub vehicle: ObjectId,
    pub hull: std::rc::Rc<torque_collide::mesh::Hull>,
    pub floor: ObjectView,
}

impl Floor {
    pub fn new(vehicle: ObjectId, body_half: F3, centre: F3, half: F3) -> Floor {
        let mut m = torque_collide::math::IDENTITY;
        m[3] = centre[0];
        m[7] = centre[1];
        m[11] = centre[2];
        let obj_box = Box3::new([-half[0], -half[1], -half[2]], half);
        Floor {
            ground: Ground::default(),
            vehicle,
            hull: std::rc::Rc::new(box_hull(body_half)),
            floor: ObjectView {
                obj_to_world: m,
                world_to_obj: torque_collide::math::affine_inverse(&m),
                scale: [1.0; 3],
                obj_box,
                world_box: torque_collide::math::mul_box(&m, &obj_box),
                world_radius: 1.0,
                type_mask: 1 << 25,
                collision_disabled: false,
            },
        }
    }
}

impl World for Floor {
    fn object(&self, id: ObjectId) -> Option<ObjectView> {
        (id == ObjectId(50)).then_some(self.floor)
    }
    fn find_objects(
        &mut self,
        query: &Box3,
        _mask: u32,
        _exclude: Option<ObjectId>,
        _limit: usize,
        out: &mut Vec<ObjectId>,
    ) -> Result<(), Exhausted> {
        if query.overlaps(&self.floor.world_box) {
            out.push(ObjectId(50));
        }
        Ok(())
    }
    fn convex_source(&self, id: ObjectId) -> ConvexSource {
        if id == ObjectId(50) {
            ConvexSource::Zone
        } else {
            ConvexSource::None
        }
    }
    fn hull(&self, id: ObjectId, slot: u32) -> Option<std::rc::Rc<torque_collide::mesh::Hull>> {
        (id == self.vehicle && slot == 0).then(|| self.hull.clone())
    }
    fn charge(&mut self, units: u64) -> bool {
        self.ground.charged = self.ground.charged.saturating_add(units);
        true
    }
}

impl VehicleHost for Floor {
    fn world(&mut self) -> &mut dyn World {
        self
    }
    fn sim_time(&self) -> u32 {
        self.ground.time
    }
    fn cast_ray(&mut self, start: F3, end: F3, mask: u32) -> Option<RayHit> {
        self.ground.cast_ray(start, end, mask)
    }
    fn on_impact(&mut self, _s: &mut VehicleState, _vec: F3) {
        self.ground.callbacks.push("impact".into());
    }
}
