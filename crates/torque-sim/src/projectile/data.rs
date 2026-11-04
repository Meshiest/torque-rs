//! `ProjectileData` as `Projectile::processTick` reads it (the constructor's defaults and the validators of
//! `initPersistFields`). A datablock's script fields go through the validators below, in the host's field setter.
//!
//! Credit: where Torque3D has the same rules, this module follows its MIT-licensed `T3D/projectile.cpp` (Copyright (c) 2012
//! GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.); the code here is this crate's own.

/// The fields the step reads. `lifetime`, `arming_delay` and `fade_delay` are in TICKS (the scaled validator stores the
/// script's milliseconds divided by the tick length).
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectileData {
    /// `isBallistic`, default false: gravity acts and a hit before the arming delay bounces.
    pub is_ballistic: bool,
    /// `velInheritFactor` [0, 1], default 1.0 (read by the script that fires the projectile).
    pub vel_inherit_factor: f32,
    /// `muzzleVelocity` [0, 10000], default 50.0 (read by the script that fires the projectile).
    pub muzzle_velocity: f32,
    /// `bounceElasticity` [0, 0.999], default 0.999.
    pub bounce_elasticity: f32,
    /// `bounceFriction` [0, 1], default 0.3.
    pub bounce_friction: f32,
    /// `gravityMod` [0, 1], default 1.0.
    pub gravity_mod: f32,
    /// `lifetime` in ticks ([0, 4095]), default 625 ticks (20 s).
    pub lifetime: u32,
    /// `armingDelay` in ticks, default 0.
    pub arming_delay: u32,
    /// `fadeDelay` in ticks, default 625 (client only).
    pub fade_delay: u32,
}

impl Default for ProjectileData {
    fn default() -> ProjectileData {
        ProjectileData {
            is_ballistic: false,
            vel_inherit_factor: 1.0,
            muzzle_velocity: 50.0,
            bounce_elasticity: 0.999,
            bounce_friction: 0.3,
            gravity_mod: 1.0,
            lifetime: 20000 / 32,
            arming_delay: 0,
            fade_delay: 20000 / 32,
        }
    }
}

/// The validators `initPersistFields` attaches (`addNamedFieldV`).
pub mod validate {
    /// `lightRadius`.
    pub const LIGHT_RADIUS: (f32, f32) = (1.0, 20.0);
    /// `velInheritFactor`.
    pub const VEL_INHERIT: (f32, f32) = (0.0, 1.0);
    /// `muzzleVelocity`.
    pub const MUZZLE_VELOCITY: (f32, f32) = (0.0, 10000.0);
    /// `bounceElasticity`.
    pub const BOUNCE_ELASTICITY: (f32, f32) = (0.0, 0.999);
    /// `bounceFriction`.
    pub const BOUNCE_FRICTION: (f32, f32) = (0.0, 1.0);
    /// `gravityMod`.
    pub const GRAVITY_MOD: (f32, f32) = (0.0, 1.0);
    /// The scaled integer validators of `lifetime`, `armingDelay` and `fadeDelay`: the divisor (the tick in ms), the minimum
    /// and the maximum (`Projectile::MaxLivingTicks`).
    pub const TICKS_DIVISOR: i32 = 32;
    pub const TICKS_MIN: i32 = 0;
    pub const TICKS_MAX: i32 = 4095;

    /// The float range validator: a value below the minimum or above the maximum is clamped and an error printed (the
    /// bool is false); a NaN passes both tests unchanged.
    pub fn range(v: f32, (min, max): (f32, f32)) -> (f32, bool) {
        if min > v {
            (min, false)
        } else if v > max {
            (max, false)
        } else {
            (v, true)
        }
    }

    /// The scaled validator: the script's value (ms) divided by the tick as a C `int` division (toward zero), then clamped to
    /// `[0, 4095]` with an error printed when it was outside (the bool is false).
    pub fn scaled_ticks(ms: i32) -> (u32, bool) {
        // Dividing by 32 cannot overflow; i32::MIN / 32 is exact.
        let q = ms / TICKS_DIVISOR;
        if q < TICKS_MIN {
            (TICKS_MIN as u32, false)
        } else if q > TICKS_MAX {
            (TICKS_MAX as u32, false)
        } else {
            (q as u32, true)
        }
    }
}

impl ProjectileData {
    /// `lifetime = ms` through the scaled validator; false when it was out of range (the host prints the error).
    pub fn set_lifetime_ms(&mut self, ms: i32) -> bool {
        let (v, ok) = validate::scaled_ticks(ms);
        self.lifetime = v;
        ok
    }

    /// `armingDelay = ms` through the scaled validator.
    pub fn set_arming_delay_ms(&mut self, ms: i32) -> bool {
        let (v, ok) = validate::scaled_ticks(ms);
        self.arming_delay = v;
        ok
    }

    /// `fadeDelay = ms` through the scaled validator.
    pub fn set_fade_delay_ms(&mut self, ms: i32) -> bool {
        let (v, ok) = validate::scaled_ticks(ms);
        self.fade_delay = v;
        ok
    }
}
