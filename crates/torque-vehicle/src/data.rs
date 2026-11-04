//! The datablock fields the server step reads (`VehicleData` and its `ShapeBaseData` part; `WheeledVehicleData`
//! `WheeledVehicleTire`, `WheeledVehicleSpring`, `FlyingVehicleData`), with the constructors'
//! defaults. The host fills them from its datablock objects.
//!
//! **Hostile values.** Every field is script-chosen: mass 0, NaN tolerances, a negative `integration`, a huge wheel radius. The
//! step does plain float arithmetic (NaN and infinities propagate through it); the only clamp is the sub-step count
//! ([`crate::MAX_SUBSTEPS`]).
//!
//! Credit: where Torque3D has the same rules, this module follows its MIT-licensed `T3D/vehicles/vehicle.cpp`,
//! `T3D/vehicles/wheeledVehicle.cpp` and `T3D/vehicles/flyingVehicle.cpp` (each Copyright (c) 2012 GarageGames, LLC); the code
//! here is this crate's own.

use torque_collide::math::F3;

/// The `ShapeBaseData` fields the base `processTick` reads for a vehicle (as `torque_sim::player::PlayerData`'s).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShapeBaseData {
    /// Mass, drag and density.
    pub mass: f32,
    pub drag: f32,
    pub density: f32,
    /// Max energy and max damage.
    pub max_energy: f32,
    pub max_damage: f32,
    /// `repairRate` and `isInvincible` (as sim's).
    pub repair_rate: f32,
    pub is_invincible: bool,
    /// Energy comes from the mount.
    pub inherit_energy: bool,
}

impl Default for ShapeBaseData {
    fn default() -> Self {
        ShapeBaseData {
            mass: 1.0,
            drag: 0.7,
            density: 4.0,
            max_energy: 0.0,
            max_damage: 1.0,
            repair_rate: 0.0,
            is_invincible: false,
            inherit_energy: false,
        }
    }
}

/// `VehicleData`: the base fields the server step reads.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VehicleData {
    pub shape: ShapeBaseData,
    /// bodyRestitution (1.0), bodyFriction (0).
    pub body_restitution: f32,
    pub body_friction: f32,
    /// minImpactSpeed (25): the impact callback's threshold.
    pub min_impact_speed: f32,
    /// maxSteeringAngle (0.785).
    pub max_steering_angle: f32,
    /// minDrag, maxDrag (Flying's forces).
    pub min_drag: f32,
    pub max_drag: f32,
    /// integration (1): the sub-steps per tick, unclamped by the datablock.
    pub integration: i32,
    /// collisionTol (0.1), contactTol (0.1).
    pub collision_tol: f32,
    pub contact_tol: f32,
    /// massCenter, massBox (Wheeled's inertia).
    pub mass_center: F3,
    pub mass_box: F3,
    /// jetForce (500), jetEnergyDrain (0.8), minJetEnergy (1).
    pub jet_force: f32,
    pub jet_energy_drain: f32,
    pub min_jet_energy: f32,
}

impl Default for VehicleData {
    /// The constructor's values.
    fn default() -> Self {
        VehicleData {
            shape: ShapeBaseData::default(),
            body_restitution: 1.0,
            body_friction: 0.0,
            min_impact_speed: 25.0,
            max_steering_angle: 0.785,
            min_drag: 0.0,
            max_drag: 0.0,
            integration: 1,
            collision_tol: 0.1,
            contact_tol: 0.1,
            mass_center: [0.0; 3],
            mass_box: [0.0; 3],
            jet_force: 500.0,
            jet_energy_drain: 0.8,
            min_jet_energy: 1.0,
        }
    }
}

/// One wheel record of `WheeledVehicleData` (built by `preload`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WheelRecord {
    /// The opposite wheel's index (-1 none).
    pub opposite: i32,
    /// The hub's position in the shape (x mirrored from the opposite wheel's).
    pub pos: F3,
    /// The hub node, the spring sequence (-1 none) and the spring's travel.
    pub hub_node: i32,
    pub spring_seq: i32,
    pub spring_len: f32,
}

/// `WheeledVehicleData` (the fields after the base).
#[derive(Debug, Clone, PartialEq)]
pub struct WheeledVehicleData {
    pub base: VehicleData,
    /// maxWheelSpeed (40), engineTorque (1), engineBrake (1), brakeTorque (1).
    pub max_wheel_speed: f32,
    pub engine_torque: f32,
    pub engine_brake: f32,
    pub brake_torque: f32,
    /// The wheel records (at most [`crate::wheeled::WHEELS`]).
    pub wheels: Vec<WheelRecord>,
    /// The "steering" and "brakelight" sequences (-1 none).
    pub steering_seq: i32,
    pub brakelight_seq: i32,
}

impl Default for WheeledVehicleData {
    /// The create function's values (no wheels until `preload`).
    fn default() -> Self {
        WheeledVehicleData {
            base: VehicleData::default(),
            max_wheel_speed: 40.0,
            engine_torque: 1.0,
            engine_brake: 1.0,
            brake_torque: 1.0,
            wheels: Vec::new(),
            steering_seq: -1,
            brakelight_seq: -1,
        }
    }
}

/// `WheeledVehicleTire`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TireData {
    /// Mass (not set by the constructor: 0 here).
    pub mass: f32,
    /// kineticFriction (0.5), staticFriction (1), restitution (1).
    pub kinetic_friction: f32,
    pub static_friction: f32,
    pub restitution: f32,
    /// lateralForce (10), lateralDamping (1), lateralRelaxation (1).
    pub lateral_force: f32,
    pub lateral_damping: f32,
    pub lateral_relaxation: f32,
    /// longitudinalForce (10), longitudinalDamping (1), longitudinalRelaxation (1).
    pub longitudinal_force: f32,
    pub longitudinal_damping: f32,
    pub longitudinal_relaxation: f32,
    /// Radius (0.6; `preload` sets it from the tire shape's bounds: [`TireData::radius_from_bounds`]).
    pub radius: f32,
}

impl Default for TireData {
    fn default() -> Self {
        TireData {
            mass: 0.0,
            kinetic_friction: 0.5,
            static_friction: 1.0,
            restitution: 1.0,
            lateral_force: 10.0,
            lateral_damping: 1.0,
            lateral_relaxation: 1.0,
            longitudinal_force: 10.0,
            longitudinal_damping: 1.0,
            longitudinal_relaxation: 1.0,
            radius: 0.6,
        }
    }
}

impl TireData {
    /// `WheeledVehicleTire::preload` with a loaded shape: `(max.x - min.x) * 0.5` of its bounds.
    pub fn radius_from_bounds(min_x: f32, max_x: f32) -> f32 {
        (max_x - min_x) * 0.5
    }
}

/// `WheeledVehicleSpring`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpringData {
    /// length (1), force (10), damping (1), antiSwayForce (1).
    pub length: f32,
    pub force: f32,
    pub damping: f32,
    pub anti_sway_force: f32,
}

impl Default for SpringData {
    fn default() -> Self {
        SpringData {
            length: 1.0,
            force: 10.0,
            damping: 1.0,
            anti_sway_force: 1.0,
        }
    }
}

/// `FlyingVehicleData` (fields from its constructor and `initPersistFields`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FlyingVehicleData {
    pub base: VehicleData,
    /// minTrailSpeed (1).
    pub min_trail_speed: f32,
    /// maneuveringForce, horizontalSurfaceForce, verticalSurfaceForce (0).
    pub maneuvering_force: f32,
    pub horizontal_surface_force: f32,
    pub vertical_surface_force: f32,
    /// autoInputDamping (1), steeringForce (1), steeringRollForce (1), rollForce (1).
    pub auto_input_damping: f32,
    pub steering_force: f32,
    pub steering_roll_force: f32,
    pub roll_force: f32,
    /// autoAngularForce, rotationalDrag, maxAutoSpeed, autoLinearForce (0).
    pub auto_angular_force: f32,
    pub rotational_drag: f32,
    pub max_auto_speed: f32,
    pub auto_linear_force: f32,
    /// hoverHeight (2), createHoverHeight (2), vertThrustMultiple (1).
    pub hover_height: f32,
    pub create_hover_height: f32,
    pub vert_thrust_multiple: f32,
    /// maxSpeed (100; `preload` sets `maneuveringForce / minDrag`).
    pub max_speed: f32,
}

impl Default for FlyingVehicleData {
    fn default() -> Self {
        FlyingVehicleData {
            // The FlyingVehicleData constructor writes pi into maxSteeringAngle.
            base: VehicleData {
                max_steering_angle: std::f32::consts::PI,
                ..VehicleData::default()
            },
            min_trail_speed: 1.0,
            maneuvering_force: 0.0,
            horizontal_surface_force: 0.0,
            vertical_surface_force: 0.0,
            auto_input_damping: 1.0,
            steering_force: 1.0,
            steering_roll_force: 1.0,
            roll_force: 1.0,
            auto_angular_force: 0.0,
            rotational_drag: 0.0,
            max_auto_speed: 0.0,
            auto_linear_force: 0.0,
            hover_height: 2.0,
            create_hover_height: 2.0,
            vert_thrust_multiple: 1.0,
            max_speed: 100.0,
        }
    }
}

impl FlyingVehicleData {
    /// `FlyingVehicleData::preload`'s one number: `maxSpeed = maneuveringForce / minDrag`.
    pub fn preload(&mut self) {
        self.max_speed = self.maneuvering_force / self.base.min_drag;
    }
}

/// The temporary shape instance `WheeledVehicleData::preload` builds from the datablock's shape (the host's asset model).
pub trait PreloadShape {
    /// A node by name (the shape's name table, then its node array).
    fn find_node(&self, name: &str) -> Option<i32>;
    /// A sequence by name.
    fn find_sequence(&self, name: &str) -> Option<i32>;
    /// `setSequence(thread, seq, pos)` then `animate` on the instance: its node transforms change and stay.
    fn animate(&mut self, seq: i32, pos: f32);
    /// The node's translation (elements 3, 7, 11 of its transform) in the instance's current node transforms.
    fn node_translation(&self, node: i32) -> F3;
}

impl WheeledVehicleData {
    /// `WheeledVehicleData::preload`, the wheel records and the two sequences: for `hub0`..`hub7` in order (a missing
    /// hub is skipped, no record used), the hub node; with a `spring<i>` sequence the hub's translation at sequence position 0, then
    /// the travel `z(0) - z(1)` (an exact 0 drops the sequence; a NaN keeps it); without one the translation in the instance's
    /// current pose (the last animation stays). Then the first earlier record within 0.5 in y (`|y' - y| < 0.5`) becomes the
    /// opposite: this record takes its position with x negated, and both point at each other (an earlier pairing of that record
    /// is overwritten). Collision planes are not extracted: the server step does not need them.
    pub fn preload(&mut self, shape: &mut dyn PreloadShape) {
        self.wheels.clear();
        for i in 0..crate::wheeled::WHEELS {
            let Some(node) = shape.find_node(&format!("hub{i}")) else {
                continue;
            };
            let mut rec = WheelRecord {
                opposite: -1,
                pos: [0.0; 3],
                hub_node: node,
                spring_seq: -1,
                spring_len: 0.0,
            };
            match shape.find_sequence(&format!("spring{i}")) {
                Some(seq) => {
                    rec.spring_seq = seq;
                    shape.animate(seq, 0.0);
                    rec.pos = shape.node_translation(node);
                    shape.animate(seq, 1.0);
                    let z1 = shape.node_translation(node)[2];
                    rec.spring_len = rec.pos[2] - z1;
                    if rec.spring_len == 0.0 {
                        rec.spring_seq = -1;
                    }
                }
                None => {
                    rec.pos = shape.node_translation(node);
                }
            }
            let me = self.wheels.len();
            for (j, other) in self.wheels.iter_mut().enumerate() {
                let dy = (other.pos[1] - rec.pos[1]).abs();
                if 0.5 > dy {
                    rec.pos = [-other.pos[0], other.pos[1], other.pos[2]];
                    rec.opposite = j as i32;
                    other.opposite = me as i32;
                    break;
                }
            }
            self.wheels.push(rec);
        }
        self.steering_seq = shape.find_sequence("steering").unwrap_or(-1);
        self.brakelight_seq = shape.find_sequence("brakelight").unwrap_or(-1);
    }
}
