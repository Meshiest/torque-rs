//! An opt-in constraint on a Player from outside its own rules: a tether to a point (a grapple), which the host gives the step
//! through [`PlayerHost::tether`](super::PlayerHost::tether).
//!
//! It runs inside the Player's own tick, after `updateMove` has made the velocity and before `updatePos` sweeps along it: the
//! velocity is changed so the Player ends the tick no farther from the anchor than the tether's length (a hard tether), or
//! pulled back by a spring past the length (a soft one). The sweep's contacts still stop the Player. Because it is part of the
//! Player's step, a client's prediction replays it exactly when it is given the same tether, and a host that gives none (the
//! default) runs Torque's step unchanged.

use torque_collide::math::F3;
use torque_collide::math::vec3::dot;

use super::PlayerState;

/// The longest tether (units).
pub const MAX_LENGTH: f32 = 200.0;
/// The stiffest spring (force a unit past the length) and the most damping (force a unit a second).
pub const MAX_STIFFNESS: f32 = 1.0e6;
pub const MAX_DAMPING: f32 = 1.0e5;
/// The fastest a tether brings a Player that is past its length back in (units a second): one made longer than the distance it
/// is given reels the Player in, it does not throw it.
pub const MAX_CATCH_UP: f32 = 30.0;

/// A tether on a Player: it may not go farther than `length` from `anchor` (a hard tether, `stiffness` 0), or is pulled back
/// past it by a spring of `stiffness` and `damping` (a soft one). The point held is the Player's position moved by `offset`.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Tether {
    pub anchor: F3,
    pub offset: F3,
    pub length: f32,
    pub stiffness: f32,
    pub damping: f32,
}

impl Tether {
    /// The tether with every value held to its range, or `None` when a value is not finite.
    pub fn held(self) -> Option<Tether> {
        let finite = |v: &[f32]| v.iter().all(|x| x.is_finite());
        if !(finite(&self.anchor)
            && finite(&self.offset)
            && finite(&[self.length, self.stiffness, self.damping]))
        {
            return None;
        }
        Some(Tether {
            length: self.length.clamp(0.0, MAX_LENGTH),
            stiffness: self.stiffness.clamp(0.0, MAX_STIFFNESS),
            damping: self.damping.clamp(0.0, MAX_DAMPING),
            ..self
        })
    }

    /// Whether the tether is a spring past its length rather than a limit.
    pub fn is_soft(&self) -> bool {
        self.stiffness > 0.0
    }
}

/// Applies `tether` to the Player's velocity for a step of `dt` seconds; the change of velocity it made (the host gives the
/// thing at the other end the equal and opposite impulse, `-mass * change`). Nothing changes for a mounted Player, a tether
/// that is not finite, or a Player inside its length that is not moving out past it.
pub fn apply(s: &mut PlayerState, tether: &Tether, dt: f32) -> F3 {
    let Some(t) = tether.held() else {
        return [0.0; 3];
    };
    if s.mount.is_some() || !(dt > 0.0) {
        return [0.0; 3];
    }
    let p = s.position();
    let held: F3 = std::array::from_fn(|k| p[k] + t.offset[k]);
    let d: F3 = std::array::from_fn(|k| held[k] - t.anchor[k]);
    let dist = dot(d, d).sqrt();
    if !(dist > 1.0e-4) {
        return [0.0; 3];
    }
    let n = d.map(|x| x / dist);
    let v = s.velocity;
    let outward = dot(v, n);
    let change = if t.is_soft() {
        let stretch = dist - t.length;
        if !(stretch > 0.0) {
            return [0.0; 3];
        }
        let mass = if s.mass > 1.0e-3 { s.mass } else { 1.0e-3 };
        // The pull is at most what stops the outward speed and takes the stretch back in one step, so a stiff spring on a light
        // Player cannot throw it back past the anchor.
        let pull = (t.stiffness * stretch + t.damping * outward) / mass * dt;
        let most = outward.max(0.0) + (stretch / dt).min(MAX_CATCH_UP);
        pull.clamp(0.0, most)
    } else {
        // The fastest outward speed that ends the step at the length with the sideways speed kept (negative past it: back in).
        let side: F3 = std::array::from_fn(|k| (v[k] - n[k] * outward) * dt);
        let reach = (t.length * t.length - dot(side, side)).max(0.0).sqrt();
        let room = ((reach - dist) / dt).max(-MAX_CATCH_UP);
        if !(outward > room) {
            return [0.0; 3];
        }
        outward - room
    };
    let dv = n.map(|x| -x * change);
    for (v, d) in s.velocity.iter_mut().zip(dv) {
        *v += d;
    }
    dv
}

#[cfg(test)]
mod tests {
    use torque_collide::ObjectId;

    use super::*;
    use crate::player::PlayerData;

    fn player(at: F3, velocity: F3) -> PlayerState {
        let mut s = PlayerState::new(ObjectId(1), &PlayerData::default(), at, 0.0);
        s.velocity = velocity;
        s.mass = 90.0;
        s
    }

    fn after(s: &PlayerState, dt: f32) -> F3 {
        let p = s.position();
        std::array::from_fn(|k| p[k] + s.velocity[k] * dt)
    }

    fn distance(a: F3, b: F3) -> f32 {
        (0..3).map(|k| (a[k] - b[k]).powi(2)).sum::<f32>().sqrt()
    }

    const DT: f32 = 0.032;

    #[test]
    fn a_hard_tether_ends_the_step_at_its_length_and_leaves_a_player_inside_it_alone() {
        let anchor = [0.0, 0.0, 10.0];
        let t = Tether {
            anchor,
            length: 4.0,
            ..Tether::default()
        };
        // At the length, moving straight out at 10 units a second: the outward speed is taken away.
        let mut s = player([0.0, 0.0, 6.0], [3.0, 0.0, -10.0]);
        let change = apply(&mut s, &t, DT);
        assert!(change[2] > 9.9 && change[0] == 0.0, "{change:?}");
        assert!(distance(after(&s, DT), anchor) <= 4.0 + 1.0e-3);
        // Inside it and moving in: nothing.
        let mut s = player([0.0, 0.0, 8.0], [0.0, 1.0, 1.0]);
        assert_eq!(apply(&mut s, &t, DT), [0.0; 3]);
        assert_eq!(s.velocity, [0.0, 1.0, 1.0]);
        // Far past it: reeled in, no faster than the catch-up speed.
        let mut s = player([0.0, 0.0, -40.0], [0.0; 3]);
        apply(&mut s, &t, DT);
        assert!(
            (s.velocity[2] - MAX_CATCH_UP).abs() < 1.0e-3,
            "{:?}",
            s.velocity
        );
    }

    #[test]
    fn a_soft_tether_pulls_only_past_its_length_and_never_past_the_anchor() {
        let anchor = [0.0, 0.0, 10.0];
        let t = Tether {
            anchor,
            length: 4.0,
            stiffness: 900.0,
            damping: 30.0,
            ..Tether::default()
        };
        let mut inside = player([0.0, 0.0, 7.0], [0.0, 0.0, -2.0]);
        assert_eq!(apply(&mut inside, &t, DT), [0.0; 3]);
        // A unit past: pulled back by about (900 * 1 + 30 * 2) / 90 * dt.
        let mut s = player([0.0, 0.0, 5.0], [0.0, 0.0, -2.0]);
        let change = apply(&mut s, &t, DT);
        assert!(
            (change[2] - (900.0 + 60.0) / 90.0 * DT).abs() < 1.0e-4,
            "{change:?}"
        );
        // A spring past any reason on a light player is held to what takes the stretch back in one step.
        let stiff = Tether {
            stiffness: MAX_STIFFNESS,
            ..t
        };
        let mut s = player([0.0, 0.0, 5.0], [0.0; 3]);
        s.mass = 1.0;
        apply(&mut s, &stiff, DT);
        assert!(
            after(&s, DT)[2] <= anchor[2] - 4.0 + 1.0e-3,
            "{:?}",
            s.velocity
        );
    }

    #[test]
    fn a_tether_that_is_not_finite_or_a_mounted_player_changes_nothing() {
        let bad = Tether {
            anchor: [f32::NAN, 0.0, 0.0],
            length: 1.0,
            ..Tether::default()
        };
        let mut s = player([0.0, 0.0, 5.0], [1.0, 2.0, 3.0]);
        assert_eq!(apply(&mut s, &bad, DT), [0.0; 3]);
        assert_eq!(s.velocity, [1.0, 2.0, 3.0]);
        let mut mounted = player([0.0, 0.0, 5.0], [1.0, 2.0, 3.0]);
        mounted.mount = Some(ObjectId(9));
        let t = Tether {
            length: 1.0,
            ..Tether::default()
        };
        assert_eq!(apply(&mut mounted, &t, DT), [0.0; 3]);
    }
}
