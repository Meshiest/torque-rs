//! What hits props: a projectile's ray, and the area impulse of a projectile's explosion. The server turns each into an impulse
//! on the prop at its next step; a predicting client uses the same rules for its own shots ([`crate::predict`]).
//!
//! A hit's impulse runs along the projectile's velocity: the `impulse` of its `ProjectileData` (Torque3D's field, newton
//! seconds) when it has one above 0, else its speed times [`SHOT_MASS`]. An explosion with an `areaImpulse` above 0 and an
//! `areaImpulseRadius` above 0 pushes every prop whose center lies within the radius away from the explosion, falling off to
//! nothing at the radius. Every impulse is held to [`MAX_HIT_IMPULSE`], and a radius to [`MAX_BLAST_RADIUS`].

use crate::world::length;

/// The mass (kilograms) a projectile without an `impulse` hits as: its speed times it is the impulse.
pub const SHOT_MASS: f32 = 0.25;

/// The largest impulse one hit or one explosion gives one prop (newton seconds).
pub const MAX_HIT_IMPULSE: f32 = 5000.0;

/// The largest radius an explosion's area impulse reaches (units).
pub const MAX_BLAST_RADIUS: f32 = 50.0;

/// The most hits the server keeps for one step (more in one tick are left out).
pub const MAX_PENDING_HITS: usize = 256;

/// A projectile's hit on a prop, as the server's collision source is told it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hit {
    /// The prop's object id, the projectile's and the projectile's datablock (what it pushes with is read from the datablock,
    /// which stays when the projectile goes).
    pub prop: u32,
    pub by: u32,
    pub datablock: u32,
    pub point: [f32; 3],
    pub normal: [f32; 3],
    pub velocity: [f32; 3],
}

/// An explosion seen on the server: the projectile's datablock and where it went off.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Blast {
    pub datablock: u32,
    pub at: [f32; 3],
}

/// What a projectile's datablock says it pushes with: its hit's `impulse` (none: by its speed) and its explosion's area impulse
/// and radius.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ShotPush {
    pub impulse: Option<f32>,
    pub area: f32,
    pub radius: f32,
}

impl ShotPush {
    /// The push of fields read as numbers (a value that is not a finite number above 0 reads as none).
    pub fn of(impulse: Option<f32>, area: Option<f32>, radius: Option<f32>) -> ShotPush {
        let positive = |v: Option<f32>| v.filter(|v| v.is_finite() && *v > 0.0);
        ShotPush {
            impulse: positive(impulse).map(|v| v.min(MAX_HIT_IMPULSE)),
            area: positive(area).map_or(0.0, |v| v.min(MAX_HIT_IMPULSE)),
            radius: positive(radius).map_or(0.0, |v| v.min(MAX_BLAST_RADIUS)),
        }
    }

    /// Whether its explosion pushes anything.
    pub fn blasts(&self) -> bool {
        self.area > 0.0 && self.radius > 0.0
    }
}

/// The impulse a hit at `velocity` gives: along it, `push.impulse` or the speed times [`SHOT_MASS`], held to
/// [`MAX_HIT_IMPULSE`]. None for a velocity that is not finite or is next to nothing.
pub fn hit_impulse(velocity: [f32; 3], push: &ShotPush) -> Option<[f32; 3]> {
    let speed = length(velocity);
    if !(speed.is_finite() && speed > 1.0e-3) {
        return None;
    }
    let size = push
        .impulse
        .unwrap_or(speed * SHOT_MASS)
        .min(MAX_HIT_IMPULSE);
    Some(velocity.map(|v| v / speed * size))
}

/// The impulse an explosion at `at` gives a prop whose center is `center`: away from it, `push.area` falling off to nothing at
/// `push.radius`. None past the radius, for an explosion that pushes nothing, or a value that is not finite. A prop whose
/// center is at the explosion is pushed up.
pub fn blast_impulse(at: [f32; 3], center: [f32; 3], push: &ShotPush) -> Option<[f32; 3]> {
    if !push.blasts() {
        return None;
    }
    let to: [f32; 3] = std::array::from_fn(|k| center[k] - at[k]);
    let d = length(to);
    if !(d.is_finite() && d < push.radius) {
        return None;
    }
    let dir = if d > 1.0e-4 {
        to.map(|v| v / d)
    } else {
        [0.0, 0.0, 1.0]
    };
    let size = (push.area * (1.0 - d / push.radius)).min(MAX_HIT_IMPULSE);
    Some(dir.map(|v| v * size))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hit_pushes_along_the_shot_held_to_the_cap() {
        let push = ShotPush::default();
        let j = hit_impulse([0.0, 40.0, 0.0], &push).unwrap();
        assert_eq!(j, [0.0, 40.0 * SHOT_MASS, 0.0]);
        let strong = ShotPush::of(Some(1.0e9), None, None);
        let j = hit_impulse([3.0, 4.0, 0.0], &strong).unwrap();
        assert!((length(j) - MAX_HIT_IMPULSE).abs() < 1.0);
        assert_eq!(hit_impulse([f32::NAN, 0.0, 0.0], &push), None);
        assert_eq!(hit_impulse([0.0; 3], &push), None);
    }

    #[test]
    fn a_blast_falls_off_to_nothing_at_its_radius() {
        let push = ShotPush::of(None, Some(100.0), Some(4.0));
        let near = blast_impulse([0.0; 3], [1.0, 0.0, 0.0], &push).unwrap();
        assert_eq!(near, [75.0, 0.0, 0.0]);
        assert_eq!(blast_impulse([0.0; 3], [4.0, 0.0, 0.0], &push), None);
        assert_eq!(
            blast_impulse([0.0; 3], [0.0; 3], &push).unwrap(),
            [0.0, 0.0, 100.0]
        );
        assert_eq!(ShotPush::of(None, Some(f32::INFINITY), Some(3.0)).area, 0.0);
        assert_eq!(
            ShotPush::of(None, Some(1.0), Some(1.0e6)).radius,
            MAX_BLAST_RADIUS
        );
    }
}
