//! Another player's figure between the server's updates, as TGE's client moves a Player ghost: run on from the last update along
//! its velocity, and an update that lands somewhere else warped to over a few ticks instead of snapped to.
//!
//! - **Between updates** the figure moves on along the last update's velocity (TGE's client ticks the ghost on with its last move,
//!   `mPredictionCount` ticks at most), for at most [`MAX_EXTRAPOLATION`] past the time the next update is due: a figure whose
//!   updates stop (it stood still) stops too.
//! - **An update** sets the velocity, and the difference between where the figure is drawn and where the update puts it (the
//!   position, the yaw and the head's pitch) is spread over the time the next update is due in, at least a tick and at most
//!   [`MAX_WARP`] (`Player::unpackUpdate`'s warp: `delta.warpOffset` over `warpTicks`, at most `sMaxWarpTicks`). A figure that turns
//!   at a steady rate turns at that rate on the client, with no step at each update. More than [`SNAP_DISTANCE`] away is a
//!   respawn: the figure is put there at once.
//! - **The time between updates** is the average of the last ones (the server sends a client packets at its own rate, not every
//!   tick).
//!
//! Credit: the warp follows `Player::unpackUpdate` and `Player::processTick`'s ghost branch in Torque3D's MIT-licensed
//! `T3D/player.cpp` (Copyright (c) 2012 GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.). The code here is this example's
//! own.

use std::f32::consts::{PI, TAU};

/// A server tick (seconds).
pub const TICK: f32 = 0.032;
/// The longest a warp lasts (seconds; TGE's `sMaxWarpTicks` is 3 ticks).
pub const MAX_WARP: f32 = 0.25;
/// How long past the due time of the next update the figure still runs on along its velocity (seconds).
pub const MAX_EXTRAPOLATION: f32 = 0.1;
/// An update further than this from the figure is a respawn or a teleport (units).
pub const SNAP_DISTANCE: f32 = 4.0;
/// The time between updates assumed before two came (seconds; the engine's server sends ten packets a second).
pub const FIRST_INTERVAL: f32 = 0.1;

/// What an update of a figure's record says.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Update {
    pub position: [f32; 3],
    pub velocity: [f32; 3],
    pub yaw: f32,
    /// The head's pitch (positive looks down).
    pub pitch: f32,
}

/// A figure's motion as the client draws it.
#[derive(Debug, Clone, PartialEq)]
pub struct GhostMotion {
    position: [f32; 3],
    velocity: [f32; 3],
    yaw: f32,
    pitch: f32,
    last: Update,
    /// The warp: what it adds a second, and the seconds it has left.
    warp_position: [f32; 3],
    warp_yaw: f32,
    warp_pitch: f32,
    warp_left: f32,
    /// Seconds the figure still runs on along its velocity.
    run_left: f32,
    /// Seconds since the last update, and the average time between updates.
    since: f32,
    interval: f32,
}

/// `a` turned into `-pi..pi`.
pub fn wrap_angle(a: f32) -> f32 {
    let a = (a + PI).rem_euclid(TAU) - PI;
    if a.is_finite() { a } else { 0.0 }
}

fn distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    (0..3).map(|i| (a[i] - b[i]).powi(2)).sum::<f32>().sqrt()
}

impl GhostMotion {
    /// A figure where its first update puts it.
    pub fn new(u: Update) -> GhostMotion {
        GhostMotion {
            position: u.position,
            velocity: u.velocity,
            yaw: u.yaw,
            pitch: u.pitch,
            last: u,
            warp_position: [0.0; 3],
            warp_yaw: 0.0,
            warp_pitch: 0.0,
            warp_left: 0.0,
            run_left: FIRST_INTERVAL + MAX_EXTRAPOLATION,
            since: 0.0,
            interval: FIRST_INTERVAL,
        }
    }

    /// The record as it stands this frame: a new update when it differs from the last one.
    pub fn receive(&mut self, u: Update) {
        if u == self.last {
            return;
        }
        self.last = u;
        if self.since > 0.0 {
            self.interval = (self.interval * 3.0 + self.since.min(1.0)) * 0.25;
        }
        self.since = 0.0;
        self.velocity = u.velocity;
        self.run_left = self.interval + MAX_EXTRAPOLATION;
        if distance(u.position, self.position) > SNAP_DISTANCE {
            *self = GhostMotion {
                interval: self.interval,
                ..GhostMotion::new(u)
            };
            return;
        }
        let span = self.interval.clamp(TICK, MAX_WARP);
        self.warp_position = [0, 1, 2].map(|i| (u.position[i] - self.position[i]) / span);
        self.warp_yaw = wrap_angle(u.yaw - self.yaw) / span;
        self.warp_pitch = (u.pitch - self.pitch) / span;
        self.warp_left = span;
    }

    /// `dt` seconds on.
    pub fn advance(&mut self, dt: f32) {
        let dt = dt.clamp(0.0, 1.0);
        self.since += dt;
        let run = dt.min(self.run_left);
        self.run_left -= run;
        let warp = dt.min(self.warp_left);
        self.warp_left -= warp;
        for i in 0..3 {
            self.position[i] += self.velocity[i] * run + self.warp_position[i] * warp;
        }
        self.yaw = wrap_angle(self.yaw + self.warp_yaw * warp);
        self.pitch += self.warp_pitch * warp;
    }

    pub fn position(&self) -> [f32; 3] {
        self.position
    }

    pub fn yaw(&self) -> f32 {
        self.yaw
    }

    pub fn pitch(&self) -> f32 {
        self.pitch
    }

    /// The velocity the figure moves at now (none once it stopped running on).
    pub fn velocity(&self) -> [f32; 3] {
        if self.run_left > 0.0 {
            self.velocity
        } else {
            [0.0; 3]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A figure walking along y at `speed` and turning at `turn` radians a second, sent every `every` seconds, drawn every
    /// `frame` seconds: the largest step a frame makes and the largest backward step, in position and yaw.
    fn run(speed: f32, turn: f32, every: f32, frame: f32) -> (f32, f32, f32) {
        let at = |t: f32| Update {
            position: [0.0, speed * t, 0.0],
            velocity: [0.0, speed, 0.0],
            yaw: wrap_angle(turn * t),
            pitch: 0.0,
        };
        let mut m = GhostMotion::new(at(0.0));
        let (mut t, mut next) = (0.0f32, every);
        let (mut max_step, mut max_back, mut max_turn) = (0.0f32, 0.0f32, 0.0f32);
        let mut last = (m.position(), m.yaw());
        while t < 5.0 {
            t += frame;
            if t >= next {
                // The update was sent a tick before it is read.
                m.receive(at(next - TICK));
                next += every;
            }
            m.advance(frame);
            let (p, y) = (m.position(), m.yaw());
            max_step = max_step.max(distance(p, last.0));
            max_back = max_back.max(last.0[1] - p[1]);
            max_turn = max_turn.max(wrap_angle(y - last.1).abs());
            last = (p, y);
        }
        (max_step, max_back, max_turn)
    }

    #[test]
    fn a_steady_figure_moves_and_turns_steadily() {
        for (every, frame) in [(0.1, 0.007), (0.096, 0.016), (0.032, 0.007), (0.1, 0.033)] {
            let (step, back, turn) = run(6.0, 2.0, every, frame);
            assert!(back <= 1e-4, "{every} {frame}: a backward step of {back}");
            assert!(
                step <= 6.0 * frame * 1.6 + 1e-3,
                "{every} {frame}: a step of {step}"
            );
            assert!(
                turn <= 2.0 * frame * 1.6 + 1e-3,
                "{every} {frame}: a turn of {turn}"
            );
        }
    }

    #[test]
    fn a_far_update_is_a_respawn_and_a_stopped_figure_stays() {
        let still = Update {
            position: [0.0; 3],
            velocity: [0.0, 5.0, 0.0],
            yaw: 0.0,
            pitch: 0.0,
        };
        let mut m = GhostMotion::new(still);
        for _ in 0..100 {
            m.advance(0.016);
        }
        assert!(m.position()[1] < 5.0 * (FIRST_INTERVAL + MAX_EXTRAPOLATION) + 1e-3);
        assert_eq!(m.velocity(), [0.0; 3]);
        m.receive(Update {
            position: [10.0, 0.0, 0.0],
            ..still
        });
        assert_eq!(m.position(), [10.0, 0.0, 0.0]);
    }

    #[test]
    fn the_yaw_warps_the_short_way_round() {
        let u = Update {
            position: [0.0; 3],
            velocity: [0.0; 3],
            yaw: 3.0,
            pitch: 0.0,
        };
        let mut m = GhostMotion::new(u);
        m.receive(Update { yaw: -3.0, ..u });
        m.advance(MAX_WARP);
        assert!((wrap_angle(m.yaw() - -3.0)).abs() < 1e-4, "{}", m.yaw());
        m.advance(0.01);
        assert!((wrap_angle(m.yaw() - -3.0)).abs() < 1e-4);
    }
}
