//! A third-person camera's boom: the line from a point on the figure (the pivot) back to where the camera would be, and how
//! much of it the camera uses when something is in the way.
//!
//! **What is in the way** is looked for along five lines from the pivot ([`CameraBoom::follow`]): to the boom's end and to
//! four points [`SIDE_RADIUS`] around it, square to the boom. The side lines find a wall the boom grazes before the camera's
//! near plane would cut into it, where the middle one alone would let the camera slide along it. The room is the nearest
//! hit along the boom less the game's clearance.
//!
//! **In fast, out slowly.** The length the camera uses follows the room as a critically damped spring: quickly when the room
//! shrinks ([`PULL_IN_SECONDS`]), slowly when it grows again ([`EASE_OUT_SECONDS`]), worked out exactly for any frame time. So
//! a post, a chair or a door's edge that crosses the line between the figure and the camera moves the camera in over a few
//! frames instead of in one, and the camera stays in when the line clears for a moment.
//!
//! **Never through a wall.** The camera may lag the room only while it stays where it can be: the move from where it was
//! drawn to where the spring puts it must not pass through anything, and the back of what is between it and the pivot must
//! stay [`BEHIND_MARGIN`] in front of it; else the camera is put at the room at once. A figure backing into a wall so pushes
//! the camera in with it, frame by frame, and a camera pulled in behind a post comes the last bit past it in one step.

/// How far round the boom's end the side lines reach (m): beyond the near plane's corners (0.1 m out, 90° wide).
pub const SIDE_RADIUS: f32 = 0.15;
/// The time the camera takes to come 95% of the way in when the room shrinks, and out when it grows (seconds).
pub const PULL_IN_SECONDS: f32 = 0.09;
pub const EASE_OUT_SECONDS: f32 = 0.6;
/// How far the camera keeps in front of what is in the way by default (m).
pub const CLEARANCE: f32 = 0.3;
/// The least room the camera keeps from the back of something between it and the pivot (m), beyond the near plane.
pub const BEHIND_MARGIN: f32 = 0.15;

/// `u` at which a second-order critically damped system from rest has 5% of its way left: `(1 + u) e^-u = 0.05`.
const U95: f32 = 4.743_865;

/// A boom's length, frame after frame (module docs).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CameraBoom {
    /// The least length the camera keeps (or the full one when that is shorter), and how far it keeps in front of what is in
    /// the way (m).
    nearest: f32,
    clearance: f32,
    /// The length the camera uses and how fast it changes (m/s); none before the first frame.
    length: Option<(f32, f32)>,
    /// Where the camera was last put.
    point: Option<[f32; 3]>,
}

fn add(a: [f32; 3], b: [f32; 3], s: f32) -> [f32; 3] {
    [a[0] + b[0] * s, a[1] + b[1] * s, a[2] + b[2] * s]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn unit(a: [f32; 3]) -> Option<[f32; 3]> {
    let n = (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt();
    (n > 1e-6).then(|| a.map(|x| x / n))
}

impl Default for CameraBoom {
    /// No least length, [`CLEARANCE`] in front of what is in the way.
    fn default() -> CameraBoom {
        CameraBoom::new(0.0, CLEARANCE)
    }
}

impl CameraBoom {
    /// A boom that keeps at least `nearest` long and `clearance` in front of what is in the way (m).
    pub fn new(nearest: f32, clearance: f32) -> CameraBoom {
        CameraBoom {
            nearest: nearest.max(0.0),
            clearance: clearance.max(0.0),
            length: None,
            point: None,
        }
    }

    /// Forgets the length (a new world, or a camera put somewhere else: the next frame starts at the room).
    pub fn reset(&mut self) {
        *self = CameraBoom::new(self.nearest, self.clearance);
    }

    /// The length the camera used last frame, if any.
    pub fn length(&self) -> Option<f32> {
        self.length.map(|(l, _)| l)
    }

    /// The camera's point this frame, `dt` seconds after the last: on the boom from `pivot` along unit `dir`, at most `full`
    /// long and at least the boom's least length, its clearance in front of what is in the way. `cast(from, to)` is the game's
    /// line of sight: the share of the way from `from` to `to` at which the line first meets something the camera keeps out
    /// of, if it does.
    pub fn follow(
        &mut self,
        pivot: [f32; 3],
        dir: [f32; 3],
        full: f32,
        dt: f32,
        mut cast: impl FnMut([f32; 3], [f32; 3]) -> Option<f32>,
    ) -> [f32; 3] {
        let full = full.max(0.0);
        let least = self.nearest.clamp(0.0, full);
        let clearance = self.clearance;
        let end = add(pivot, dir, full);
        let side = unit(cross(dir, [0.0, 0.0, 1.0])).unwrap_or([1.0, 0.0, 0.0]);
        let up = cross(side, dir);
        let mut hit = full;
        for (axis, s) in [
            (side, 0.0),
            (side, SIDE_RADIUS),
            (side, -SIDE_RADIUS),
            (up, SIDE_RADIUS),
            (up, -SIDE_RADIUS),
        ] {
            if let Some(f) = cast(pivot, add(end, axis, s)) {
                hit = hit.min(f.clamp(0.0, 1.0) * full);
            }
        }
        let room = if hit < full {
            (hit - clearance).max(least)
        } else {
            full
        };
        let (mut length, mut speed) = match self.length {
            Some((l, v)) if dt > 0.0 && dt.is_finite() => {
                let seconds = if room < l {
                    PULL_IN_SECONDS
                } else {
                    EASE_OUT_SECONDS
                };
                let w = U95 / seconds;
                let x = l - room;
                let c = v + w * x;
                let e = (-w * dt).exp();
                (room + (x + c * dt) * e, (v - w * c * dt) * e)
            }
            // An endless frame (or a length that is not finite) is a cut: the camera at the room.
            Some((l, v)) if dt.is_finite() && l.is_finite() => (l, v),
            _ => (room, 0.0),
        };
        if length > full {
            (length, speed) = (full, 0.0);
        }
        if length > room + 1e-4 {
            // Beyond the room, behind what came in the way: only while the camera got there without passing through anything
            // and keeps its near plane off the back of it.
            let there = add(pivot, dir, length);
            let crossed = self.point.is_none_or(|from| cast(from, there).is_some());
            let close_behind = cast(there, pivot).is_some_and(|f| f * length < BEHIND_MARGIN);
            if crossed || close_behind {
                (length, speed) = (room, 0.0);
            }
        }
        let point = add(pivot, dir, length);
        self.length = Some((length, speed));
        self.point = Some(point);
        point
    }
}

#[cfg(test)]
#[path = "camera_boom_tests.rs"]
mod tests;
