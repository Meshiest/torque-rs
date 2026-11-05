//! The `PlayerData` fields the Player's step reads, with the constructor's defaults (`ShapeBaseData`'s and `PlayerData`'s) and the
//! products of `PlayerData::preload`. The host fills the scripted and wire
//! fields; [`PlayerData::preload`] then computes the rest as Torque does.
//!
//! Credit: where Torque3D has the same rules, this module follows its MIT-licensed `T3D/player.cpp` and `T3D/shapeBase.cpp`
//! (each Copyright (c) 2012 GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.); the code here is this crate's own.

use torque_collide::math::float_to_int;

/// One entry of `PlayerData::actionList`: the fields of an action the step reads.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ActionAnimation {
    /// `death`: a death action (`updateDeathOffsets` moves the root only for these).
    pub death: bool,
}

/// The action numbers the step reads of the Player class's action table (`PlayerData::ActionAnimationList`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActionNumbers {
    /// The table's fixed actions (`NumTableActionAnims`); the shape's other sequences follow them.
    pub table: u32,
    /// The jump actions: moving on a run surface stops them.
    pub jump: u32,
    pub stand_jump: u32,
    /// Landing: a move stops it, as it stops every action past the table.
    pub land: u32,
    /// No action (`NullAnimation`).
    pub none: u32,
}

/// TGE's numbers: eight fixed actions, jump 5, standjump 6, land 7, no action 511.
pub const TGE_ACTION_NUMBERS: ActionNumbers = ActionNumbers {
    table: 8,
    jump: 5,
    stand_jump: 6,
    land: 7,
    none: 511,
};

/// The datablock as the step sees it: `ShapeBaseData`'s fields first, then `PlayerData`'s.
#[derive(Debug, Clone, PartialEq)]
pub struct PlayerData {
    /// `ShapeBaseData::mass` (1.0 in the `ShapeBaseData` constructor, 9.0 in `PlayerData`'s); `onNewDataBlock` copies it
    /// to the object.
    pub mass: f32,
    /// `ShapeBaseData::drag`.
    pub drag: f32,
    /// `density`.
    pub density: f32,
    /// `maxEnergy`.
    pub max_energy: f32,
    /// `maxDamage`.
    pub max_damage: f32,
    /// `repairRate`: the most of the repair reserve a tick repairs.
    pub repair_rate: f32,
    /// `isInvincible`: the object takes no damage (the damage and repair step is skipped).
    pub is_invincible: bool,
    /// The energy is the mount's (`getEnergyLevel` and the recharge gate).
    pub inherit_energy: bool,
    /// The look-angle limits.
    pub min_look_angle: f32,
    pub max_look_angle: f32,
    pub max_freelook_angle: f32,
    /// Run force, run energy drain and minimum run energy.
    pub run_force: f32,
    pub run_energy_drain: f32,
    pub min_run_energy: f32,
    /// Forward, back and side speeds, then the same three underwater: the speeds `onNewDataBlock` copies into the Player's
    /// table.
    pub max_forward_speed: f32,
    pub max_backward_speed: f32,
    pub max_side_speed: f32,
    pub max_underwater_forward_speed: f32,
    pub max_underwater_backward_speed: f32,
    pub max_underwater_side_speed: f32,
    /// Step height, run surface angle and the horizontal and upward speed limits and resistances.
    pub max_step_height: f32,
    pub run_surface_angle: f32,
    pub horiz_max_speed: f32,
    pub horiz_resist_speed: f32,
    pub horiz_resist_factor: f32,
    pub up_max_speed: f32,
    pub up_resist_speed: f32,
    pub up_resist_factor: f32,
    /// `recoverDelay` and `recoverRunForceScale` (neither has a console or wire field).
    pub recover_delay: i32,
    pub recover_run_force_scale: f32,
    /// Jump force, energy drain, minimum energy, speed limits and surface angle.
    pub jump_force: f32,
    pub jump_energy_drain: f32,
    pub min_jump_energy: f32,
    pub min_jump_speed: f32,
    pub max_jump_speed: f32,
    pub jump_surface_angle: f32,
    /// `jumpDelay`.
    pub jump_delay: i32,
    /// The impact speed limits.
    pub min_impact_speed: f32,
    pub ground_impact_min_speed: f32,
    /// `boundingBox`.
    pub bounding_box: [f32; 3],
    /// `pickupRadius` (preload may raise it).
    pub pickup_radius: f32,
    /// The action count (preload, shape-derived; 0 without a shape).
    pub action_count: u32,
    /// `actionList`: the actions' fields the step reads, by action number.
    pub action_list: Vec<ActionAnimation>,
    /// The class's action numbers (TGE's by default).
    pub actions: ActionNumbers,
    /// `pickupDelta`: what `findContact` widens its item query by (set by preload from the pickup radius).
    pub pickup_delta: i32,
    /// `runSurfaceCos` and `jumpSurfaceCos` (preload).
    pub run_surface_cos: f32,
    pub jump_surface_cos: f32,
    /// The poses the datablock allows and their boxes and speeds (the engine's own after Torque3D's; none by default).
    pub poses: super::pose::PoseData,
}

impl Default for PlayerData {
    /// The constructors' values, before `preload`: `ShapeBaseData` (drag 0, density 1, maxDamage 1, repairRate 0.0033,
    /// `isInvincible` and the inherit-energy flag clear), then `PlayerData` (maxEnergy 60 and the rest).
    fn default() -> Self {
        PlayerData {
            mass: 9.0,
            drag: 0.0,
            density: 1.0,
            max_energy: 60.0,
            max_damage: 1.0,
            repair_rate: 0.0033,
            is_invincible: false,
            inherit_energy: false,
            min_look_angle: -1.4,
            max_look_angle: 1.4,
            max_freelook_angle: 3.0,
            run_force: 432.0,
            run_energy_drain: 0.0,
            min_run_energy: 0.0,
            max_forward_speed: 10.0,
            max_backward_speed: 10.0,
            max_side_speed: 10.0,
            max_underwater_forward_speed: 10.0,
            max_underwater_backward_speed: 10.0,
            max_underwater_side_speed: 10.0,
            max_step_height: 1.0,
            run_surface_angle: 70.0,
            horiz_max_speed: 80.0,
            horiz_resist_speed: 38.0,
            horiz_resist_factor: 1.0,
            up_max_speed: 80.0,
            up_resist_speed: 38.0,
            up_resist_factor: 1.0,
            recover_delay: 30,
            recover_run_force_scale: 1.0,
            jump_force: 75.0,
            jump_energy_drain: 0.0,
            min_jump_energy: 0.0,
            min_jump_speed: 20.0,
            max_jump_speed: 40.0,
            jump_surface_angle: 78.0,
            jump_delay: 30,
            min_impact_speed: 25.0,
            ground_impact_min_speed: 10.0,
            bounding_box: [1.0, 1.0, 2.3],
            pickup_radius: 0.0,
            action_count: 0,
            action_list: Vec::new(),
            actions: TGE_ACTION_NUMBERS,
            pickup_delta: 0,
            run_surface_cos: 0.0,
            jump_surface_cos: 0.0,
            poses: super::pose::PoseData::default(),
        }
    }
}

impl PlayerData {
    /// The numeric products of `PlayerData::preload`, in its order. The pickup radius is converted to a delta of the box: the
    /// larger horizontal side of `boundingBox`.
    pub fn preload(&mut self) {
        let m = self.larger_side();
        self.preload_with(m);
    }

    /// The larger horizontal side of `boundingBox` (a NaN picks y).
    pub fn larger_side(&self) -> f32 {
        if self.bounding_box[0] > self.bounding_box[1] {
            self.bounding_box[0]
        } else {
            self.bounding_box[1]
        }
    }

    /// `preload` with the side `h` the pickup radius is measured from (a subclass whose boxes are in other units passes its
    /// own; through the [`library`](crate::library) table): the radius is raised to `h` or cut to `2 h`, and the pickup delta is
    /// `(S32)(radius - h)`.
    pub fn preload_with(&mut self, h: f32) {
        (crate::library::get().player_preload)(self, h);
    }

    /// `preload_with` in plain arithmetic: [`PLAIN`](crate::library::PLAIN)'s entry.
    pub fn plain_preload_with(&mut self, h: f32) {
        let cos_of = |deg: f32| ((deg * std::f32::consts::PI) / 180.0).cos();
        self.run_surface_cos = cos_of(self.run_surface_angle);
        self.jump_surface_cos = cos_of(self.jump_surface_angle);
        if self.min_jump_energy < self.jump_energy_drain {
            self.min_jump_energy = self.jump_energy_drain;
        }
        if self.recover_delay > 0x7f {
            self.recover_delay = 0x7f;
        }
        if self.jump_delay > 0x7f {
            self.jump_delay = 0x7f;
        }
        // The pickup radius and delta from the side.
        let pr = self.pickup_radius;
        if pr < h {
            self.pickup_radius = h;
            self.pickup_delta = 0;
        } else if pr <= 2.0 * h {
            self.pickup_delta = float_to_int(f64::from(pr - h));
        } else {
            self.pickup_radius = 2.0 * h;
            self.pickup_delta = float_to_int(f64::from(self.pickup_radius - h));
        }
        if self.max_jump_speed <= self.min_jump_speed {
            self.max_jump_speed = self.min_jump_speed + 0.1;
        }
    }

    /// The action's death flag (false past the table, as the action count test refuses it).
    pub fn action_is_death(&self, anim: u32) -> bool {
        anim < self.action_count && self.action_list.get(anim as usize).is_some_and(|a| a.death)
    }
}
