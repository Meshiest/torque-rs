//! The lines of sight a frame's flares are drawn by ([`LineOfSight`]): the sun flare's and each light's corona, from the eye to
//! the flare. TGE's `fxSunLight::TestLOS` and `fxLight::TestLOS` cast a ray through the client's container (the control object
//! in first person and the light's attached object left out) and fade the flare in while it is clear and out while it is not;
//! a frame here takes the fade as settled: drawn when the line is clear, not when it is blocked.
//!
//! Credit: follows TGE 1.4's `game/fx/fxSunLight.cc` and `game/fx/fxLight.cc`, Copyright (C) GarageGames.com, Inc.; the code
//! here is this crate's own.

/// What a line of sight is cast for (a world may keep different blockers for each).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Sight {
    /// The sun's flare.
    Sun,
    /// A light's corona.
    Corona,
}

/// What blocks a frame's lines of sight.
pub trait LineOfSight {
    /// Whether something lies between `from` and `to`: `Some(true)` blocked, `Some(false)` clear, `None` not tested (a world
    /// may bound the work a frame does). `through`: an object the line passes through (the one a light sits in).
    fn blocked(
        &mut self,
        from: [f32; 3],
        to: [f32; 3],
        sight: Sight,
        through: Option<u32>,
    ) -> Option<bool>;
}

/// Nothing in the way: every line of sight is clear.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Open;

impl LineOfSight for Open {
    fn blocked(&mut self, _: [f32; 3], _: [f32; 3], _: Sight, _: Option<u32>) -> Option<bool> {
        Some(false)
    }
}
