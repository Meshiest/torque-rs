//! The built-in camera modes: the first person, the third person on a boom and the orbit.

use torque_render_scene::source::Eye;

use super::{CameraCx, CameraMode, CameraWorld};
use crate::camera_behind::{boom_line, looking};
use crate::camera_boom::CameraBoom;
use crate::ghosts::OwnView;
use crate::orbit::{Orbit, ZOOM_NOTCH};

/// The shape of the camera behind a figure: how far back, how high above its feet the boom starts, how far up the view the
/// camera rides off the boom's line (the figure a little below the middle of the view), and how near it comes to the boom's
/// pivot and to what is in the way (all m).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rig {
    pub distance: f32,
    pub target_height: f32,
    pub lift: f32,
    pub nearest: f32,
    pub clearance: f32,
}

impl Default for Rig {
    fn default() -> Rig {
        Rig {
            distance: 6.5,
            target_height: 1.7,
            lift: 0.6,
            nearest: 0.4,
            clearance: 0.3,
        }
    }
}

impl Rig {
    /// Where the boom starts: above the figure's feet, no higher than its eye (crouched or prone the camera rides lower with
    /// the eye, eased as the eye is).
    pub fn pivot(&self, f: &OwnView) -> [f32; 3] {
        let eye = (f.eye[2] - f.position[2]).max(0.0);
        [
            f.position[0],
            f.position[1],
            f.position[2] + self.target_height.min(eye),
        ]
    }

    /// The boom from [`Rig::pivot`] back to the camera with nothing in the way: its way (unit) and length, back against the
    /// figure's look ([`boom_line`]).
    pub fn boom(&self, f: &OwnView) -> ([f32; 3], f32) {
        boom_line(f.yaw, f.pitch, self.distance, self.lift)
    }

    /// `at` raised to stay twice the clearance above the ground or water under it.
    pub fn above_ground(&self, world: &dyn CameraWorld, at: &mut [f32; 3]) {
        if let Some(ground) = world.ground(at[0], at[1]) {
            at[2] = at[2].max(ground + self.clearance * 2.0);
        }
    }

    /// The point `distance` from `target` along unit `back`, brought in front of the first thing between (no nearer than
    /// `nearest`, or the distance when that is nearer).
    pub fn kept_clear(
        &self,
        world: &dyn CameraWorld,
        target: [f32; 3],
        back: [f32; 3],
        distance: f32,
        nearest: f32,
    ) -> [f32; 3] {
        let reach = match world.clear_hit(target, back, distance) {
            Some(t) => (t - self.clearance).max(nearest.min(distance)),
            None => distance,
        };
        [0, 1, 2].map(|i| target[i] + back[i] * reach)
    }

    /// `point` of the camera, kept in front of what lies between it and the figure's pivot (none: no figure, `point` as it is).
    pub fn keep(
        &self,
        world: &dyn CameraWorld,
        control: Option<&OwnView>,
        point: [f32; 3],
    ) -> [f32; 3] {
        let Some(f) = control else { return point };
        let from = self.pivot(f);
        let d = [0, 1, 2].map(|i| point[i] - from[i]);
        let length = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        if length < 1e-4 {
            return point;
        }
        self.kept_clear(world, from, d.map(|x| x / length), length, 0.0)
    }
}

/// The first person: the eye at the figure's eyes, looking where the figure looks.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FirstPerson {
    rig: Rig,
}

impl FirstPerson {
    /// The first person, its ease kept out of walls and ground as the third person of `rig` is.
    pub fn new(rig: Rig) -> FirstPerson {
        FirstPerson { rig }
    }
}

impl CameraMode for FirstPerson {
    fn eye(&self, cx: &CameraCx<'_>) -> Option<Eye> {
        cx.control.map(|f| looking(f.eye, f.yaw, f.pitch))
    }

    fn keep(&self, cx: &CameraCx<'_>, point: [f32; 3]) -> [f32; 3] {
        self.rig.keep(cx.world, cx.control, point)
    }

    fn keeps_look(&self, _: &CameraCx<'_>) -> bool {
        true
    }
}

/// The third person: the camera on a boom back from above the figure's feet, pulled in fast in front of what comes between
/// and let out slowly when it goes, looking where the figure looks. The boom follows every frame, in use or not.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ThirdPerson {
    rig: Rig,
    boom: CameraBoom,
    /// The eye the frame placed on the boom.
    behind: Option<Eye>,
}

impl ThirdPerson {
    /// The third person shaped by `rig`.
    pub fn new(rig: Rig) -> ThirdPerson {
        ThirdPerson {
            rig,
            boom: CameraBoom::new(rig.nearest, rig.clearance),
            behind: None,
        }
    }

    /// The boom's length the last frame used.
    pub fn boom_length(&self) -> Option<f32> {
        self.boom.length()
    }
}

impl CameraMode for ThirdPerson {
    fn eye(&self, cx: &CameraCx<'_>) -> Option<Eye> {
        let f = cx.control?;
        if let Some(e) = self.behind {
            return Some(e);
        }
        // Before the frame placed it: where the boom's room puts it.
        let pivot = self.rig.pivot(f);
        let (back, length) = self.rig.boom(f);
        let mut at = self
            .rig
            .kept_clear(cx.world, pivot, back, length, self.rig.nearest);
        self.rig.above_ground(cx.world, &mut at);
        Some(looking(at, f.yaw, f.pitch))
    }

    fn update(&mut self, cx: &CameraCx<'_>, _active: bool) {
        let Some(f) = cx.control else {
            self.reset();
            return;
        };
        let (back, length) = self.rig.boom(f);
        let world = cx.world;
        let mut at = self
            .boom
            .follow(self.rig.pivot(f), back, length, cx.dt, |a, b| {
                world.clear_share(a, b)
            });
        self.rig.above_ground(cx.world, &mut at);
        self.behind = Some(looking(at, f.yaw, f.pitch));
    }

    fn keep(&self, cx: &CameraCx<'_>, point: [f32; 3]) -> [f32; 3] {
        self.rig.keep(cx.world, cx.control, point)
    }

    fn keeps_look(&self, _: &CameraCx<'_>) -> bool {
        true
    }

    fn reset(&mut self) {
        self.boom.reset();
        self.behind = None;
    }
}

/// The orbit: the camera circles a point above the figure, turned by the frame's look and moved in and out by its wheel
/// ([`Orbit`]). It starts behind the figure each time it is switched to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OrbitMode {
    /// The circle's state.
    pub orbit: Orbit,
    /// How far above the figure's feet the camera looks at (m).
    pub height: f32,
}

impl Default for OrbitMode {
    fn default() -> OrbitMode {
        OrbitMode {
            orbit: Orbit::default(),
            height: 1.5,
        }
    }
}

impl CameraMode for OrbitMode {
    fn eye(&self, cx: &CameraCx<'_>) -> Option<Eye> {
        let f = cx.control?;
        let target = [f.position[0], f.position[1], f.position[2] + self.height];
        Some(self.orbit.eye(target))
    }

    fn update(&mut self, cx: &CameraCx<'_>, active: bool) {
        if active {
            let zoom = ZOOM_NOTCH.powf(cx.input.zoom);
            let zoom = if zoom.is_finite() { zoom } else { 1.0 };
            let [yaw, pitch] = cx.input.look.map(|d| if d.is_finite() { d } else { 0.0 });
            self.orbit.turn(yaw, pitch, zoom);
        }
    }

    fn enter(&mut self, cx: &CameraCx<'_>) {
        if let Some(f) = cx.control {
            self.orbit.yaw = f.yaw.rem_euclid(std::f32::consts::TAU);
        }
    }
}
