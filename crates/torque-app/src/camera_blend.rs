//! A camera that moves from one mode's eye to another's when the game switches modes (first person to third and back, say),
//! instead of cutting.
//!
//! **Relative to the new eye.** The blend keeps the drawn eye as an offset from the new mode's eye (its position, its look's
//! yaw and pitch, its field of view), and only the offset is eased away: the camera moves with the figure the eye is on as
//! that moves, and closes on it on top, so a switch while walking, running or driving never lags behind or stops the camera.
//!
//! **A critically damped spring.** Each part of the offset falls to zero as a critically damped system of the third order
//! (three equal poles at [`CameraBlend::rate`]): from standing its acceleration grows from nothing, it is fastest some 0.08 s
//! in and slows from there all the way, and it never passes the eye. Its position, speed and acceleration are
//! worked out exactly for any frame time, so the motion is the same at 60 frames a second as at 144 or after a hitch.
//! [`SETTLE_SECONDS`] after a switch from standing the camera has covered 95% of the way.
//!
//! **A switch** ([`CameraBlend::switch`]) keeps the camera's motion: the spring starts at the last frame before it, from the
//! camera's offset and velocity less the new eye's. The new eye's own motion is known only a frame later, so the first frame
//! after a switch takes it to move as the old eye did (true while a figure walks, runs or drives: both eyes ride on it), and
//! the frame after works the spring out again from the switch with the motion measured. A switch mid-blend so continues
//! from where the camera is going, with no stop and no kink, and the motion is the same at any frame rate.
//!
//! **Each frame** the game advances the clock ([`CameraBlend::advance`]), then hands the new mode's eye as it is this frame
//! ([`CameraBlend::eye`]). With no blend running the eye is the mode's exactly, so the mouse's look reaches the view with
//! nothing added.
//!
//! **Walls and ground.** The game's own third-person camera keeps clear of what it would look through; the blend gives every
//! eye it makes to the same rule (the `keep` of [`CameraBlend::eye`]).
//!
//! **A teleport** cuts. When what the camera follows moves farther in one frame than anything travels ([`teleported`]: a
//! respawn, a figure put back at the start), or the client's control object changes, the game resets the blend (and its
//! boom): the camera is at the new place at once, as TGE's camera is when a control object warps, never eased across what
//! lies between.
//!
//! **The figure** whose eyes the first person looks from is hidden by the game while the camera is within [`CLOSE`] of them,
//! either way: coming in, the camera's last stretch passes through the back of the head, and going out its first.
//! [`CameraBlend::remaining`] says how far the camera still is from the mode's eye.

use torque_render_scene::source::Eye;

/// The time a switch from standing takes to cover 95% of the way (seconds).
pub const SETTLE_SECONDS: f32 = 0.24;
/// How near a figure's eyes the camera is when the game hides the figure (m): within it, coming in from behind and above,
/// the head would fill the view.
pub const CLOSE: f32 = 1.2;

/// The fastest a camera's subject is taken to travel (m/s), and the least jump that is a teleport whatever the frame's time (m).
pub const TELEPORT_SPEED: f32 = 100.0;
pub const TELEPORT_DISTANCE: f32 = 3.0;

/// Whether a subject at `from` last frame and at `to` this one, `dt` seconds on, was teleported: moved farther than
/// [`TELEPORT_DISTANCE`] and than [`TELEPORT_SPEED`] carries it in that time (module docs).
pub fn teleported(from: [f32; 3], to: [f32; 3], dt: f32) -> bool {
    let d = (0..3)
        .map(|k| (to[k] - from[k]).powi(2))
        .sum::<f32>()
        .sqrt();
    let reach = TELEPORT_SPEED * if dt.is_finite() { dt.max(0.0) } else { 0.0 };
    d > TELEPORT_DISTANCE.max(reach)
}

/// `u` at which a third-order critically damped system from rest has 5% of its way left: `(1 + u + u²/2) e^-u = 0.05`.
const U95: f32 = 6.295_794;
/// The blend is over when the offset is under these (m, rad, degrees) and its position moves slower than this (m/s).
const DONE_DISTANCE: f32 = 1e-3;
const DONE_ANGLE: f32 = 1e-3;
const DONE_FOV: f32 = 0.05;
const DONE_SPEED: f32 = 0.05;

/// The parts of an eye the blend moves: position, yaw, pitch and field of view (degrees; 0 for none).
const PARTS: usize = 6;
const YAW: usize = 3;
const FOV: usize = 5;

type Parts = [f32; PARTS];

/// One part of the offset: its value, rate and acceleration.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct Spring {
    x: f32,
    v: f32,
    a: f32,
}

impl Spring {
    /// `t` seconds of the system with three poles at `-w` (exact).
    fn step(self, w: f32, t: f32) -> Spring {
        let c0 = self.x;
        let c1 = self.v + w * c0;
        let c2 = 0.5 * (self.a + 2.0 * w * c1 - w * w * c0);
        let p = c0 + (c1 + c2 * t) * t;
        let dp = c1 + 2.0 * c2 * t;
        let ddp = 2.0 * c2;
        let e = (-w * t).exp();
        Spring {
            x: p * e,
            v: (dp - w * p) * e,
            a: (ddp - 2.0 * w * dp + w * w * p) * e,
        }
    }
}

/// The eye last drawn and how fast each part of it was changing (per second).
#[derive(Debug, Clone, Copy, PartialEq)]
struct Drawn {
    parts: Parts,
    rate: Parts,
}

/// What the frame after a switch needs to start the spring at the switch.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Measuring {
    before: Drawn,
    dt: f32,
    accel: Parts,
}

/// The blend between two modes' eyes (module docs).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CameraBlend {
    rate: f32,
    /// The clock since the last eye (seconds).
    dt: f32,
    /// The drawn eye less the mode's, part by part; none when no blend runs.
    offset: Option<[Spring; PARTS]>,
    /// A switch the next eye starts from.
    switched: bool,
    /// After a switch's frame, until the new eye's motion is known: the eye drawn before the switch, that frame's time, and
    /// the offset's acceleration carried from a blend that ran.
    measuring: Option<Measuring>,
    last: Option<Drawn>,
    last_target: Option<Parts>,
    /// How fast the mode's eye was moving (per second), as last measured between two frames of one mode.
    target_rate: Parts,
}

impl Default for CameraBlend {
    fn default() -> CameraBlend {
        CameraBlend::new(SETTLE_SECONDS)
    }
}

/// `a - b`, the yaw's difference taken the short way round.
fn difference(a: &Parts, b: &Parts) -> Parts {
    let mut d = [0.0; PARTS];
    for i in 0..PARTS {
        d[i] = a[i] - b[i];
    }
    d[YAW] = wrap(d[YAW]);
    d
}

/// An angle in `-π..π`.
fn wrap(a: f32) -> f32 {
    let tau = std::f32::consts::TAU;
    a - tau * (a / tau).round()
}

fn parts(e: &Eye) -> Parts {
    [
        e.position[0],
        e.position[1],
        e.position[2],
        e.yaw,
        e.pitch,
        e.fov.map_or(0.0, f32::from),
    ]
}

impl CameraBlend {
    /// A blend that covers 95% of the way from standing in `settle` seconds (0 cuts).
    pub fn new(settle: f32) -> CameraBlend {
        let rate = if settle > 0.0 && settle.is_finite() {
            U95 / settle
        } else {
            0.0
        };
        CameraBlend {
            rate,
            dt: 0.0,
            offset: None,
            switched: false,
            measuring: None,
            last: None,
            last_target: None,
            target_rate: [0.0; PARTS],
        }
    }

    /// Makes the blend cover 95% of the way from standing in `settle` seconds from now on (0 cuts); a blend that runs carries on
    /// from where it is.
    pub fn set_settle(&mut self, settle: f32) {
        self.rate = CameraBlend::new(settle).rate;
    }

    /// The spring's rate (its three poles, per second); 0 cuts.
    pub fn rate(&self) -> f32 {
        self.rate
    }

    /// The mode changed: the camera leaves the eye last drawn, carrying on its motion (none drawn yet: it starts at the new
    /// mode's eye).
    pub fn switch(&mut self) {
        self.switched = self.last.is_some() && self.rate > 0.0;
        if self.rate <= 0.0 {
            self.offset = None;
        }
    }

    /// Forgets the blend and the eye last drawn (a new world: the next eye is the mode's own).
    pub fn reset(&mut self) {
        *self = CameraBlend {
            rate: self.rate,
            ..CameraBlend::new(0.0)
        };
    }

    /// `dt` seconds of the clock.
    pub fn advance(&mut self, dt: f32) {
        if dt.is_finite() {
            self.dt += dt.max(0.0);
        }
    }

    /// Whether the camera is at the current mode's eye (no blend runs).
    pub fn arrived(&self) -> bool {
        self.offset.is_none() && !self.switched
    }

    /// How far the camera is from the mode's eye (m) as it was last drawn; 0 once arrived. Right after a switch, before the
    /// next eye, the distance is not known yet: infinite.
    pub fn remaining(&self) -> f32 {
        if self.switched {
            return f32::INFINITY;
        }
        self.offset.map_or(0.0, |o| {
            (o[0].x * o[0].x + o[1].x * o[1].x + o[2].x * o[2].x).sqrt()
        })
    }

    /// The offset `m.dt + more` seconds after the frame before a switch, for the new mode's eye at `goal` at the switch's
    /// frame moving at `moving` (per second).
    fn spring_since_switch(
        &self,
        m: &Measuring,
        goal: &Parts,
        moving: &Parts,
        more: f32,
    ) -> [Spring; PARTS] {
        let mut offset = [Spring::default(); PARTS];
        for i in 0..PARTS {
            let mut x = m.before.parts[i] - (goal[i] - moving[i] * m.dt);
            if i == YAW {
                x = wrap(x);
            }
            offset[i] = Spring {
                x,
                v: m.before.rate[i] - moving[i],
                a: m.accel[i],
            }
            .step(self.rate, m.dt + more);
        }
        offset
    }

    /// The eye to draw this frame given the current mode's eye `target` (module docs); `keep` moves a position the blend makes
    /// out of walls and ground as the game's camera keeps its own.
    pub fn eye(&mut self, target: Eye, keep: impl FnOnce([f32; 3]) -> [f32; 3]) -> Eye {
        let dt = std::mem::take(&mut self.dt);
        let goal = parts(&target);
        // An eye that is not a place (a NaN or an infinite part) is drawn as it is and leaves the blend alone; its time counts
        // toward the next eye's.
        if goal.iter().any(|x| !x.is_finite()) {
            self.dt += dt;
            return target;
        }
        let switching = self.switched && self.last.is_some();
        let previous_goal = self.last_target.replace(goal);
        if let Some(before) = previous_goal
            && dt > 0.0
            && !switching
        {
            self.target_rate = difference(&goal, &before).map(|d| d / dt);
        }
        let fov_blends = target.fov.is_some() && self.last.is_some_and(|d| d.parts[FOV] > 0.0);
        if std::mem::take(&mut self.switched)
            && let Some(last) = self.last
        {
            // The switch's frame: the spring from the frame before, the new eye taken to move as the old one did.
            let m = Measuring {
                before: last,
                dt,
                accel: self.offset.unwrap_or_default().map(|s| s.a),
            };
            let mut offset = self.spring_since_switch(&m, &goal, &self.target_rate, 0.0);
            if !fov_blends {
                offset[FOV] = Spring::default();
            }
            let finite = offset
                .iter()
                .all(|s| s.x.is_finite() && s.v.is_finite() && s.a.is_finite());
            self.offset = finite.then_some(offset);
            self.measuring = finite.then_some(m);
        } else if self.offset.is_some() && dt > 0.0 {
            let offset = match self.measuring.take() {
                // The frame after: the spring from the switch again, with the new eye's motion measured.
                Some(m) => {
                    let back = previous_goal.unwrap_or(goal);
                    self.spring_since_switch(&m, &back, &self.target_rate, dt)
                }
                None => self
                    .offset
                    .unwrap_or_default()
                    .map(|s| s.step(self.rate, dt)),
            };
            let mut offset = offset;
            if !fov_blends {
                offset[FOV] = Spring::default();
            }
            let distance = (offset[0].x.powi(2) + offset[1].x.powi(2) + offset[2].x.powi(2)).sqrt();
            let speed = (offset[0].v.powi(2) + offset[1].v.powi(2) + offset[2].v.powi(2)).sqrt();
            let done = distance < DONE_DISTANCE
                && speed < DONE_SPEED
                && offset[YAW].x.abs() < DONE_ANGLE
                && offset[4].x.abs() < DONE_ANGLE
                && offset[FOV].x.abs() < DONE_FOV;
            // An offset that is not finite (from a frame time or eye the spring cannot follow) ends the blend.
            let finite = offset
                .iter()
                .all(|s| s.x.is_finite() && s.v.is_finite() && s.a.is_finite());
            self.offset = (!done && finite).then_some(offset);
        }
        let eye = match &self.offset {
            Some(offset) => {
                let at = |i: usize| goal[i] + offset[i].x;
                let fov = if fov_blends {
                    Some(at(FOV).round().clamp(1.0, 179.0) as u8)
                } else {
                    target.fov
                };
                Eye {
                    position: keep([at(0), at(1), at(2)]),
                    yaw: wrap(at(YAW)),
                    pitch: at(4),
                    fov,
                }
            }
            None => target,
        };
        let drawn = parts(&eye);
        let rate = match self.last {
            Some(before) if dt > 0.0 => difference(&drawn, &before.parts).map(|d| d / dt),
            Some(before) => before.rate,
            None => [0.0; PARTS],
        };
        self.last = Some(Drawn { parts: drawn, rate });
        eye
    }
}

#[cfg(test)]
#[path = "camera_blend_tests.rs"]
mod tests;
