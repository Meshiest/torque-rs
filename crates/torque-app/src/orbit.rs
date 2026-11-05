//! A third-person camera that circles a point at a distance, turned by yaw (about z, 0 looking along +y) and pitch (positive
//! looks down), and [`Zoom`]: a camera distance the mouse wheel moves in eased steps.

use torque_render_scene::hd::kit::look_at;
use torque_render_scene::source::Eye;

/// The steepest the camera looks up or down (radians).
pub const MAX_PITCH: f32 = 1.4;
/// The nearest and farthest the camera keeps from its point.
pub const MIN_DISTANCE: f32 = 1.5;
pub const MAX_DISTANCE: f32 = 30.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Orbit {
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
}

impl Default for Orbit {
    /// Behind the point, a little above it, far enough to see a person whole.
    fn default() -> Orbit {
        Orbit {
            yaw: 0.0,
            pitch: 0.12,
            distance: 8.0,
        }
    }
}

impl Orbit {
    /// Turns the camera (radians) and moves it in or out, within its limits.
    pub fn turn(&mut self, yaw: f32, pitch: f32, zoom: f32) {
        self.yaw = (self.yaw + yaw).rem_euclid(std::f32::consts::TAU);
        self.pitch = (self.pitch + pitch).clamp(-MAX_PITCH, MAX_PITCH);
        self.distance = (self.distance * zoom).clamp(MIN_DISTANCE, MAX_DISTANCE);
    }

    /// The way the camera looks, flattened onto the ground (unit).
    pub fn forward(&self) -> [f32; 2] {
        [self.yaw.sin(), self.yaw.cos()]
    }

    /// To the camera's right on the ground (unit).
    pub fn right(&self) -> [f32; 2] {
        [self.yaw.cos(), -self.yaw.sin()]
    }

    /// Where the camera is when it looks at `target`.
    pub fn position(&self, target: [f32; 3]) -> [f32; 3] {
        let [fx, fy] = self.forward();
        let (s, c) = self.pitch.sin_cos();
        [
            target[0] - fx * c * self.distance,
            target[1] - fy * c * self.distance,
            target[2] + s * self.distance,
        ]
    }

    /// The eye looking at `target` (its field of view left to the frame).
    pub fn eye(&self, target: [f32; 3]) -> Eye {
        let mut eye = look_at(self.position(target), target);
        eye.fov = None;
        eye
    }
}

/// How much one wheel notch away from the user scales a [`Zoom`]'s distance (towards the user: its inverse).
pub const ZOOM_NOTCH: f32 = 0.85;
/// How quickly a [`Zoom`] eases to its target (the share of the way left that goes in a second is `1 - e^-rate`).
pub const ZOOM_RATE: f32 = 12.0;

/// A camera distance the mouse wheel moves: each notch scales the distance it aims at by [`ZOOM_NOTCH`] (away from the user in,
/// towards out), kept within its limits, and the distance drawn eases there ([`Zoom::advance`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Zoom {
    pub min: f32,
    pub max: f32,
    target: f32,
    distance: f32,
}

impl Zoom {
    /// A zoom at `distance`, within `min` and `max`.
    pub fn new(distance: f32, min: f32, max: f32) -> Zoom {
        let d = distance.clamp(min, max);
        Zoom {
            min,
            max,
            target: d,
            distance: d,
        }
    }

    /// The wheel turned `notches` (positive away from the user: in).
    pub fn notches(&mut self, notches: f32) {
        if notches.is_finite() {
            self.target = (self.target * ZOOM_NOTCH.powf(notches)).clamp(self.min, self.max);
        }
    }

    /// `dt` seconds of easing towards the distance aimed at.
    pub fn advance(&mut self, dt: f32) {
        let k = 1.0 - (-ZOOM_RATE * dt.max(0.0)).exp();
        self.distance += (self.target - self.distance) * k;
        if (self.target - self.distance).abs() < 1e-3 {
            self.distance = self.target;
        }
    }

    /// The distance drawn now.
    pub fn distance(&self) -> f32 {
        self.distance
    }

    /// The distance the zoom eases to.
    pub fn target(&self) -> f32 {
        self.target
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wheel_zooms_in_eased_steps_within_the_limits() {
        let mut z = Zoom::new(8.0, 3.0, 20.0);
        z.notches(1.0);
        assert!((z.target() - 8.0 * ZOOM_NOTCH).abs() < 1e-5);
        assert_eq!(z.distance(), 8.0);
        z.advance(0.05);
        assert!(z.distance() < 8.0 && z.distance() > z.target());
        z.advance(2.0);
        assert_eq!(z.distance(), z.target());
        z.notches(100.0);
        z.advance(2.0);
        assert_eq!(z.distance(), 3.0);
        z.notches(-100.0);
        z.advance(2.0);
        assert_eq!(z.distance(), 20.0);
    }

    #[test]
    fn the_eye_looks_at_its_point_from_behind_and_above() {
        let o = Orbit {
            yaw: 0.0,
            pitch: 0.5,
            distance: 4.0,
        };
        let p = o.position([1.0, 2.0, 1.0]);
        assert!(p[1] < 2.0 && p[2] > 1.0, "{p:?}");
        let d = ((p[0] - 1.0).powi(2) + (p[1] - 2.0).powi(2) + (p[2] - 1.0).powi(2)).sqrt();
        assert!((d - 4.0).abs() < 1e-5);
        let eye = o.eye([1.0, 2.0, 1.0]);
        assert!(
            (eye.pitch - 0.5).abs() < 1e-5 && eye.yaw.abs() < 1e-5,
            "{eye:?}"
        );
    }

    #[test]
    fn turning_keeps_to_the_limits() {
        let mut o = Orbit::default();
        o.turn(0.0, 10.0, 100.0);
        assert_eq!((o.pitch, o.distance), (MAX_PITCH, MAX_DISTANCE));
        o.turn(-1.0, -10.0, 0.0);
        assert_eq!((o.pitch, o.distance), (-MAX_PITCH, MIN_DISTANCE));
        assert!((0.0..std::f32::consts::TAU).contains(&o.yaw));
    }
}
