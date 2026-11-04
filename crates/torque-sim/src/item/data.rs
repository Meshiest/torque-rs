//! `ItemData` as the Item's step reads it (the constructor and `ShapeBaseData`'s; no field has a range
//! validator). A datablock's script fields are copied in by the host.
//!
//! Credit: where Torque3D has the same rules, this module follows its MIT-licensed `T3D/item.cpp` (Copyright (c) 2012
//! GarageGames, LLC); the code here is this crate's own.

/// The fields `Item::processTick` and its helpers read.
#[derive(Debug, Clone, PartialEq)]
pub struct ItemData {
    /// `ShapeBaseData`: the mass `applyImpulse` divides by (default 1.0).
    pub mass: f32,
    /// The drag `onNewDataBlock` copies into the Item and `updateContainer` rescales (ItemData's default 0.5).
    pub drag: f32,
    /// The density the buoyancy divides the liquid's by (ItemData's default 2.0).
    pub density: f32,
    /// `friction` (default 0.0).
    pub friction: f32,
    /// `elasticity` (default 0.0).
    pub elasticity: f32,
    /// `sticky` (default false): a sticky item never wakes from rest by itself.
    pub sticky: bool,
    /// `gravityMod` (default 1.0): the gravity is `20.0 * gravityMod`, not 9.81.
    pub gravity_mod: f32,
    /// `maxVelocity` (default -1.0: no clamp).
    pub max_velocity: f32,
    /// `dynamicType` (default 0): its bits that `item_dynamic_allowed` allows are copied into the Item's type mask by `onAdd`.
    pub dynamic_type: u32,
    /// The shape loaded (on the object): `Item::buildConvex` adds nothing without one. Set by the
    /// host's `preload`.
    pub has_shape: bool,
}

impl Default for ItemData {
    fn default() -> ItemData {
        ItemData {
            mass: 1.0,
            drag: 0.5,
            density: 2.0,
            friction: 0.0,
            elasticity: 0.0,
            sticky: false,
            gravity_mod: 1.0,
            max_velocity: -1.0,
            dynamic_type: 0,
            has_shape: true,
        }
    }
}

impl ItemData {
    /// `preload`: the shape resolved (or not). Nothing numeric is derived from it that the step reads.
    pub fn preload(&mut self, has_shape: bool) {
        self.has_shape = has_shape;
    }
}
