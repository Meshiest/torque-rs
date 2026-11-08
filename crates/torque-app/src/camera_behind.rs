//! The camera behind a figure that looks where the figure looks: the first person's view moved back, never turned.
//!
//! **The view.** In either mode the eye looks along the figure's own look (its yaw and its head's pitch, [`looking`]), as
//! TGE's third-person camera keeps the eye's rotation and only moves the camera back along it. A switch between the two (and
//! the ease between them) so moves the camera and never turns the view.
//!
//! **The boom** ([`boom_line`]) runs from a point on the figure (the pivot) back against the look, the camera [`lift`]ed off
//! that line along the view's up so the figure sits a little below the middle of the view. Its pitch is kept within
//! [`BOOM_PITCH`]: looking far down or up turns the view past the boom, which stops instead of going overhead or under the
//! ground.
//!
//! [`lift`]: boom_line

use torque_render_scene::source::Eye;

/// The least and greatest pitch of the boom (radians, positive down, as the look's): it rises behind a figure looking down
/// and sinks behind one looking up, as far as these.
pub const BOOM_PITCH: (f32, f32) = (-0.6, 1.2);

/// The way a look along `yaw` and `pitch` (radians, positive pitch down) points (unit).
pub fn look(yaw: f32, pitch: f32) -> [f32; 3] {
    let (sy, cy) = yaw.sin_cos();
    let (sp, cp) = pitch.sin_cos();
    [sy * cp, cy * cp, -sp]
}

/// The view's up for a look along `yaw` and `pitch` (unit, square to [`look`]).
fn up(yaw: f32, pitch: f32) -> [f32; 3] {
    let (sy, cy) = yaw.sin_cos();
    let (sp, cp) = pitch.sin_cos();
    [sy * sp, cy * sp, cp]
}

/// An eye at `position` looking along `yaw` and `pitch`, with the view's own field of view.
pub fn looking(position: [f32; 3], yaw: f32, pitch: f32) -> Eye {
    let tau = std::f32::consts::TAU;
    Eye {
        position,
        yaw: yaw - tau * (yaw / tau).round(),
        pitch,
        fov: None,
    }
}

/// The boom of the camera behind a figure looking along `yaw` and `pitch` (module docs): the unit way from the pivot to the
/// camera with nothing in the way and its length, `distance` back against the look (its pitch kept in [`BOOM_PITCH`]) and
/// `lift` up the view.
pub fn boom_line(yaw: f32, pitch: f32, distance: f32, lift: f32) -> ([f32; 3], f32) {
    let pitch = pitch.clamp(BOOM_PITCH.0, BOOM_PITCH.1);
    let (ahead, up) = (look(yaw, pitch), up(yaw, pitch));
    let end = [0, 1, 2].map(|i| -ahead[i] * distance + up[i] * lift);
    let length = (end[0] * end[0] + end[1] * end[1] + end[2] * end[2]).sqrt();
    if length < 1e-6 {
        return ([-ahead[0], -ahead[1], -ahead[2]], 0.0);
    }
    (end.map(|x| x / length), length)
}

/// The eye a blend between the first person and the camera behind drew (`eased`), turned back to the mode's own look
/// (`target`'s): the ease moves the camera only.
pub fn moved_only(eased: Eye, target: Eye) -> Eye {
    Eye {
        position: eased.position,
        ..target
    }
}

/// The way an eye looks (unit).
pub fn forward(eye: &Eye) -> [f32; 3] {
    look(eye.yaw, eye.pitch)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
        a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
    }

    #[test]
    fn the_boom_runs_back_against_the_look_lifted_up_the_view_and_stops_at_its_pitch_limits() {
        for yaw in [-2.5f32, 0.0, 0.7, 3.0] {
            for pitch in [-1.4f32, -0.3, 0.0, 0.5, 1.4] {
                let (dir, length) = boom_line(yaw, pitch, 6.0, 0.5);
                assert!((dot(dir, dir) - 1.0).abs() < 1e-5);
                assert!((length - (36.0f32 + 0.25).sqrt()).abs() < 1e-4);
                let kept = pitch.clamp(BOOM_PITCH.0, BOOM_PITCH.1);
                let end = dir.map(|x| x * length);
                // Back along the (kept) look by the distance, up the view by the lift.
                assert!((dot(end, look(yaw, kept)) + 6.0).abs() < 1e-4);
                assert!((dot(end, up(yaw, kept)) - 0.5).abs() < 1e-4);
            }
        }
    }

    #[test]
    fn an_eye_looks_where_look_points_and_a_blend_keeps_the_look() {
        let eye = looking([1.0, 2.0, 3.0], 7.0, 0.4);
        let f = forward(&eye);
        let want = look(7.0, 0.4);
        assert!((0..3).all(|i| (f[i] - want[i]).abs() < 1e-5));
        let eased = looking([4.0, 5.0, 6.0], 1.0, -0.2);
        let kept = moved_only(eased, eye);
        assert_eq!(
            (kept.position, kept.yaw, kept.pitch),
            (eased.position, eye.yaw, eye.pitch)
        );
    }
}
