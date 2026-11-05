//! A camera that eases from one mode's eye to another's when the game switches modes (first person to third and back, say),
//! instead of cutting.
//!
//! **A switch** ([`CameraBlend::switch`]) starts a blend from the eye last drawn: switching again mid-blend starts from where
//! the camera is, so it never jumps. **Each frame** the game advances the clock ([`CameraBlend::advance`]), then hands the
//! new mode's eye as it is this frame ([`CameraBlend::eye`]): the blend follows it as it moves. Over [`BLEND_SECONDS`],
//! eased in and out, the position moves along a short arc (the straight line lifted a little in the middle, so it passes over
//! a figure's head rather than through it), the look turns along the great circle between the two directions, and the field of
//! view moves between the two when both have one. At the end the eye is the new mode's exactly.
//!
//! **Walls and ground.** The game's own third-person camera keeps clear of what it would look through; the blend gives every
//! eye it makes to the same rule (the `keep` of [`CameraBlend::eye`]).
//!
//! **The figure** the game hides in first person is hidden only once the blend has arrived there ([`CameraBlend::arrived`]),
//! and shown as soon as a switch leaves it.

use torque_render_scene::source::Eye;

/// How long a switch takes (seconds).
pub const BLEND_SECONDS: f32 = 0.3;
/// How far the path rises in its middle, over the distance it covers.
pub const ARC_LIFT: f32 = 0.15;

/// The blend between two modes' eyes (module docs).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CameraBlend {
    duration: f32,
    /// The eye the blend leaves, and the seconds since the switch; None when no blend runs.
    from: Option<(Eye, f32)>,
    /// The eye last drawn.
    last: Option<Eye>,
}

impl Default for CameraBlend {
    fn default() -> CameraBlend {
        CameraBlend::new(BLEND_SECONDS)
    }
}

impl CameraBlend {
    /// A blend over `duration` seconds (0 cuts).
    pub fn new(duration: f32) -> CameraBlend {
        CameraBlend {
            duration: duration.max(0.0),
            from: None,
            last: None,
        }
    }

    /// The mode changed: the camera leaves the eye last drawn (none yet: it starts at the new mode's).
    pub fn switch(&mut self) {
        self.from = self.last.map(|e| (e, 0.0));
    }

    /// Forgets the blend and the eye last drawn (a new world: the next eye is the mode's own).
    pub fn reset(&mut self) {
        self.from = None;
        self.last = None;
    }

    /// `dt` seconds of the clock.
    pub fn advance(&mut self, dt: f32) {
        if let Some((_, t)) = &mut self.from {
            *t += dt.max(0.0);
            if *t >= self.duration {
                self.from = None;
            }
        }
    }

    /// Whether the camera is at the current mode's eye (no blend runs).
    pub fn arrived(&self) -> bool {
        self.from.is_none()
    }

    /// How far along the blend is, eased: 0 at the switch, 1 arrived.
    pub fn progress(&self) -> f32 {
        match self.from {
            Some((_, t)) if self.duration > 0.0 => ease((t / self.duration).clamp(0.0, 1.0)),
            _ => 1.0,
        }
    }

    /// The eye to draw this frame given the current mode's eye `target` (module docs); `keep` moves a position the blend makes
    /// out of walls and ground as the game's camera keeps its own.
    pub fn eye(&mut self, target: Eye, keep: impl FnOnce([f32; 3]) -> [f32; 3]) -> Eye {
        let eye = match self.from {
            Some((from, _)) => {
                let mut e = mix(&from, &target, self.progress());
                e.position = keep(e.position);
                e
            }
            None => target,
        };
        self.last = Some(eye);
        eye
    }
}

/// Smoothstep: eased in and out.
fn ease(x: f32) -> f32 {
    x * x * (3.0 - 2.0 * x)
}

/// The unit direction an eye looks along (yaw about z from +y, pitch positive looking down).
fn direction(e: &Eye) -> [f32; 3] {
    let (sy, cy) = e.yaw.sin_cos();
    let (sp, cp) = e.pitch.sin_cos();
    [sy * cp, cy * cp, -sp]
}

/// The point `s` of the way along the great circle from unit `a` to unit `b`.
fn slerp(a: [f32; 3], b: [f32; 3], s: f32) -> [f32; 3] {
    let dot = (a[0] * b[0] + a[1] * b[1] + a[2] * b[2]).clamp(-1.0, 1.0);
    let angle = dot.acos();
    if angle < 1e-4 {
        return b;
    }
    // Opposite directions: the turn goes round the vertical.
    let b = if dot < -0.9999 {
        let side = [-a[1], a[0], 0.0];
        let n = (side[0] * side[0] + side[1] * side[1]).sqrt();
        if n < 1e-6 {
            return if s < 0.5 { a } else { b };
        }
        let side = side.map(|x| x / n);
        let half = s * std::f32::consts::PI;
        return [0, 1, 2].map(|i| a[i] * half.cos() + side[i] * half.sin());
    } else {
        b
    };
    let sin = angle.sin();
    let (wa, wb) = (((1.0 - s) * angle).sin() / sin, (s * angle).sin() / sin);
    [0, 1, 2].map(|i| a[i] * wa + b[i] * wb)
}

/// The eye `s` (eased) of the way from `a` to `b`.
fn mix(a: &Eye, b: &Eye, s: f32) -> Eye {
    let d = [0, 1, 2].map(|i| b.position[i] - a.position[i]);
    let length = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
    let lift = ARC_LIFT * length * (s * std::f32::consts::PI).sin();
    let mut position = [0, 1, 2].map(|i| a.position[i] + d[i] * s);
    position[2] += lift;
    let look = slerp(direction(a), direction(b), s);
    let fov = match (a.fov, b.fov) {
        (Some(x), Some(y)) if x != y => {
            Some((f32::from(x) + (f32::from(y) - f32::from(x)) * s).round() as u8)
        }
        _ => b.fov,
    };
    Eye {
        position,
        yaw: look[0].atan2(look[1]),
        pitch: (-look[2]).atan2(look[0].hypot(look[1])),
        fov,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eye(position: [f32; 3], yaw: f32, pitch: f32, fov: Option<u8>) -> Eye {
        Eye {
            position,
            yaw,
            pitch,
            fov,
        }
    }

    fn apart(a: [f32; 3], b: [f32; 3]) -> f32 {
        (0..3).map(|i| (a[i] - b[i]).powi(2)).sum::<f32>().sqrt()
    }

    fn turn(a: &Eye, b: &Eye) -> f32 {
        let (x, y) = (direction(a), direction(b));
        (x[0] * y[0] + x[1] * y[1] + x[2] * y[2])
            .clamp(-1.0, 1.0)
            .acos()
    }

    const FRAME: f32 = 1.0 / 60.0;
    /// Smoothstep's steepest slope, and the arc's.
    const STEEPEST: f32 = 1.5 + ARC_LIFT * std::f32::consts::PI;

    #[test]
    fn a_switch_moves_on_without_a_jump_and_ends_exactly_at_the_new_eye() {
        let third = eye([0.0, -5.5, 3.0], 0.0, 0.3, Some(90));
        let first = eye([0.0, 0.0, 1.7], 0.4, -0.1, Some(70));
        let mut b = CameraBlend::default();
        assert_eq!(b.eye(third, |p| p), third);
        b.switch();
        assert!(!b.arrived());
        let mut last = third;
        let distance = apart(third.position, first.position);
        let angle = turn(&third, &first);
        let mut frames = 0;
        while !b.arrived() {
            b.advance(FRAME);
            let e = b.eye(first, |p| p);
            let step = apart(last.position, e.position);
            assert!(
                step <= STEEPEST * distance * FRAME / BLEND_SECONDS + 1e-4,
                "frame {frames}: a step of {step}"
            );
            assert!(turn(&last, &e) <= 1.5 * angle * FRAME / BLEND_SECONDS + 1e-3);
            let fov = i32::from(e.fov.unwrap());
            assert!((70..=90).contains(&fov));
            last = e;
            frames += 1;
        }
        assert!((17..=19).contains(&frames), "{frames} frames");
        assert_eq!(last, first);
        // Arrived: the eye is the mode's own, frame after frame.
        b.advance(FRAME);
        let moved = eye([1.0, 0.0, 1.7], 0.5, 0.0, Some(70));
        assert_eq!(b.eye(moved, |p| p), moved);
    }

    #[test]
    fn switching_again_mid_blend_starts_from_where_the_camera_is() {
        let third = eye([0.0, -5.5, 3.0], 0.0, 0.3, None);
        let first = eye([0.0, 0.0, 1.7], 0.0, 0.0, None);
        let mut b = CameraBlend::default();
        b.eye(third, |p| p);
        b.switch();
        for _ in 0..8 {
            b.advance(FRAME);
            b.eye(first, |p| p);
        }
        let mid = b.eye(first, |p| p);
        assert!(apart(mid.position, first.position) > 0.5);
        // Back the other way: the next frame is a frame's share from where it was.
        b.switch();
        b.advance(FRAME);
        let next = b.eye(third, |p| p);
        let share = STEEPEST * apart(mid.position, third.position) * FRAME / BLEND_SECONDS;
        assert!(apart(mid.position, next.position) <= share + 1e-4);
        for _ in 0..30 {
            b.advance(FRAME);
            b.eye(third, |p| p);
        }
        assert!(b.arrived());
        assert_eq!(b.eye(third, |p| p), third);
    }

    #[test]
    fn the_blend_follows_a_moving_target_and_keeps_its_eyes_where_the_game_says() {
        let mut b = CameraBlend::default();
        b.eye(eye([0.0, -6.0, 3.0], 0.0, 0.3, None), |p| p);
        b.switch();
        b.advance(BLEND_SECONDS * 0.5);
        let ground = 2.5;
        let e = b.eye(eye([0.0, 10.0, 1.7], 0.0, 0.0, None), |p| {
            [p[0], p[1], p[2].max(ground)]
        });
        assert!(e.position[2] >= ground && e.position[1] > 0.0, "{e:?}");
        // No eye yet: the first switch has nothing to leave.
        let mut fresh = CameraBlend::default();
        fresh.switch();
        assert!(fresh.arrived());
    }

    #[test]
    fn opposite_looks_turn_round_the_vertical() {
        let a = eye([0.0; 3], 0.0, 0.0, None);
        let b = eye([0.0; 3], std::f32::consts::PI, 0.0, None);
        let m = mix(&a, &b, 0.5);
        assert!(
            m.pitch.abs() < 1e-4 && (m.yaw.abs() - std::f32::consts::FRAC_PI_2).abs() < 1e-3,
            "{m:?}"
        );
    }
}
