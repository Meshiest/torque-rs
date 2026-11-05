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

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn length(a: [f32; 3]) -> f32 {
    (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt()
}

/// The milliseconds of each frame at `fps` (the way a window's clock hands them out: 17, 17, 16 at 60).
fn frame_times(fps: f32, seconds: f32) -> Vec<f32> {
    let step = 1000.0 / f64::from(fps);
    let mut out = Vec::new();
    let mut done = 0u64;
    let mut k = 1;
    while (done as f32) < seconds * 1000.0 {
        let to = (step * f64::from(k)).round() as u64;
        out.push((to - done) as f32 / 1000.0);
        done = to;
        k += 1;
    }
    out
}

/// The eye of a figure walking along +y at `speed` from the origin, `t` seconds on: at its eyes (first person) or 3.5 m behind
/// and 1.2 m over them, looking down a little.
fn walking(speed: f32, t: f32, behind: bool) -> Eye {
    let y = speed * t;
    if behind {
        eye([0.6, y - 3.5, 2.9], 0.0, 0.25, None)
    } else {
        eye([0.0, y, 1.7], 0.0, 0.0, None)
    }
}

/// A run: the camera on the first person's eye, a switch to behind at `switch` (ms), and again at each of `again`; the eyes
/// drawn with their times. Frames at `fps`, the figure walking at `speed`.
fn run(fps: f32, seconds: f32, speed: f32, switch: u32, again: &[u32]) -> Vec<(f32, Eye, Eye)> {
    let mut b = CameraBlend::default();
    let mut behind = false;
    let mut ms = 0u32;
    let mut out = Vec::new();
    let mut switches: Vec<u32> = std::iter::once(switch)
        .chain(again.iter().copied())
        .collect();
    for dt in frame_times(fps, seconds) {
        if switches.first().is_some_and(|&s| ms >= s) {
            switches.remove(0);
            behind = !behind;
            b.switch();
        }
        ms += (dt * 1000.0).round() as u32;
        b.advance(dt);
        let t = ms as f32 / 1000.0;
        let target = walking(speed, t, behind);
        out.push((t, b.eye(target, |p| p), target));
    }
    out
}

/// Each frame's velocity (from the frame before).
fn velocities(frames: &[(f32, Eye, Eye)]) -> Vec<[f32; 3]> {
    let mut v = vec![[0.0; 3]];
    for w in frames.windows(2) {
        let dt = w[1].0 - w[0].0;
        v.push(sub(w[1].1.position, w[0].1.position).map(|x| x / dt));
    }
    v
}

/// The largest change of velocity from a frame to the next, after the first `from` seconds.
fn largest_kick(frames: &[(f32, Eye, Eye)], from: f32) -> f32 {
    let v = velocities(frames);
    (2..frames.len())
        .filter(|&i| frames[i].0 > from)
        .map(|i| length(sub(v[i], v[i - 1])))
        .fold(0.0, f32::max)
}

/// The steepest acceleration of the spring from rest over a way of `x0`: `0.2306 ω² x0` (at `u = 2 - √2`).
fn steepest(x0: f32) -> f32 {
    let w = CameraBlend::default().rate();
    0.2306 * w * w * x0
}

const RATES: [f32; 2] = [60.0, 144.0];

#[test]
fn from_standing_the_camera_settles_in_time_never_passes_the_eye_and_ends_exactly_on_it() {
    let third = eye([0.0, -5.5, 3.0], 0.0, 0.3, Some(90));
    let first = eye([0.0, 0.0, 1.7], 0.4, -0.1, Some(70));
    let whole = apart(third.position, first.position);
    for fps in RATES {
        let mut b = CameraBlend::default();
        assert_eq!(b.eye(third, |p| p), third);
        b.switch();
        assert!(!b.arrived() && b.remaining().is_infinite());
        let mut last = whole;
        let mut t = 0.0;
        let mut at95 = None;
        let mut frames = 0;
        for dt in frame_times(fps, 2.0) {
            t += dt;
            b.advance(dt);
            let e = b.eye(first, |p| p);
            let left = apart(e.position, first.position);
            assert!(
                left <= last + 1e-5,
                "{fps} fps: away from the eye at {t}: {left} after {last}"
            );
            let fov = i32::from(e.fov.unwrap());
            assert!((70..=90).contains(&fov));
            if at95.is_none() && left <= 0.05 * whole {
                at95 = Some(t);
            }
            last = left;
            frames += 1;
            if b.arrived() {
                assert_eq!(e, first);
                break;
            }
        }
        let at95 = at95.unwrap();
        // The switch's own frame carries the camera on (it stood): the way is covered from the frame after.
        assert!(
            (at95 - SETTLE_SECONDS).abs() <= 2.5 / fps,
            "{fps} fps: 95% at {at95}"
        );
        assert!(
            b.arrived() && t < 0.7,
            "{fps} fps: arrived at {t} after {frames} frames"
        );
        // Arrived: the eye is the mode's own, frame after frame.
        b.advance(1.0 / fps);
        let moved = eye([1.0, 0.0, 1.7], 0.5, 0.0, Some(70));
        assert_eq!(b.eye(moved, |p| p), moved);
    }
}

#[test]
fn a_switch_while_walking_keeps_the_cameras_velocity_and_does_not_lag() {
    let whole = apart(
        walking(4.0, 0.0, true).position,
        walking(4.0, 0.0, false).position,
    );
    let mut kicks = Vec::new();
    for fps in RATES {
        let frames = run(fps, 1.5, 4.0, 500, &[]);
        let v = velocities(&frames);
        let k = frames.iter().position(|f| f.0 >= 0.5).unwrap() + 1;
        // Up to the switch the camera walks with the figure.
        assert!(
            apart(v[k - 1], [0.0, 4.0, 0.0]) < 0.05,
            "{fps} fps: {:?}",
            v[k - 1]
        );
        // No frame's velocity changes by more than the spring's steepest acceleration allows.
        let kick = largest_kick(&frames, 0.1);
        let dt = 1.0 / fps;
        assert!(
            kick <= steepest(whole) * dt * 1.25 + 0.05,
            "{fps} fps: a kick of {kick} m/s"
        );
        kicks.push(kick);
        // Relative to the figure the way is covered as from standing: the camera does not lag it.
        let settle = frames
            .iter()
            .find(|f| f.0 >= 0.5 + SETTLE_SECONDS + 2.0 / fps)
            .unwrap();
        assert!(
            apart(settle.1.position, settle.2.position) <= 0.05 * whole,
            "{fps} fps: {} behind",
            apart(settle.1.position, settle.2.position)
        );
        assert!(frames.last().unwrap().1 == frames.last().unwrap().2);
    }
    // The kicks shrink with the frame time: the velocity is continuous, not stepped.
    assert!(kicks[1] < kicks[0] * 0.6, "{kicks:?}");
}

#[test]
fn switching_again_mid_blend_continues_from_the_cameras_velocity_without_a_kink() {
    let whole = apart(
        walking(0.0, 0.0, true).position,
        walking(0.0, 0.0, false).position,
    );
    let mut kicks = Vec::new();
    for fps in RATES {
        // Out behind the figure, and back to its eyes at the out's fastest (some 0.08 s in).
        let frames = run(fps, 1.6, 0.0, 300, &[380]);
        let v = velocities(&frames);
        // The switch's frame: the first after the one at or past 0.38 s.
        let k = frames.iter().position(|f| f.0 >= 0.38).unwrap() + 1;
        assert!(length(v[k - 1]) > 10.0, "{fps} fps: {:?}", v[k - 1]);
        // The spring turns the camera's speed round: its push is the steepest from rest plus the carried speed's damping, at the
        // switch as after it.
        let kick = largest_kick(&frames, 0.0);
        let carried = length(v[k - 1]);
        let w = CameraBlend::default().rate();
        let dt = 1.0 / fps;
        let bound = (steepest(whole) + 3.0 * w * carried) * dt + 0.05;
        assert!(
            apart(v[k], v[k - 1]) <= bound,
            "{fps} fps: the switch's frame {:?} after {:?}",
            v[k],
            v[k - 1]
        );
        assert!(kick <= bound, "{fps} fps: a kick of {kick}");
        kicks.push(kick);
        // Back at the eyes without passing them: the offset never turns past the eye along the way it came.
        let home = frames.last().unwrap();
        assert_eq!(home.1, home.2);
        let from = sub(frames[k - 1].1.position, home.2.position);
        for f in &frames[k..] {
            let along = sub(f.1.position, f.2.position);
            let dot: f32 = (0..3).map(|i| along[i] * from[i]).sum();
            assert!(dot >= -1e-4, "{fps} fps: past the eye at {}", f.0);
        }
    }
    assert!(kicks[1] < kicks[0] * 0.6, "{kicks:?}");
}

#[test]
fn the_motion_is_the_same_at_any_frame_rate() {
    // A figure walking, a switch out and one back mid-blend (at moments both rates have a frame at): sampled at the same
    // moments, 60 and 144 frames a second agree.
    let a = run(60.0, 1.4, 4.0, 250, &[500]);
    let b = run(144.0, 1.4, 4.0, 250, &[500]);
    let at = |frames: &[(f32, Eye, Eye)], t: f32| -> [f32; 3] {
        let i = frames.iter().position(|f| f.0 >= t).unwrap();
        let (t0, e0) = (frames[i - 1].0, frames[i - 1].1.position);
        let (t1, e1) = (frames[i].0, frames[i].1.position);
        let s = (t - t0) / (t1 - t0);
        [0, 1, 2].map(|k| e0[k] + (e1[k] - e0[k]) * s)
    };
    // From the frame after each switch's frame at 60 on (the switch's own frame carries the camera on).
    let mut worst: f32 = 0.0;
    for k in 0..100 {
        let t = 0.29 + k as f32 * 0.01;
        if (0.5..0.54).contains(&t) {
            continue;
        }
        worst = worst.max(apart(at(&a, t), at(&b, t)));
    }
    // Sampled between frames the 60 fps path is a straight line between its frames: a few cm off a curve at 25 m/s.
    assert!(worst < 0.06, "{worst} m apart");
    // A hitch: one long frame covers what many short ones do.
    let mut long = CameraBlend::default();
    let mut short = CameraBlend::default();
    let from = eye([0.0, -5.0, 3.0], 0.0, 0.3, None);
    let to = eye([0.0, 0.0, 1.7], 0.0, 0.0, None);
    for b in [&mut long, &mut short] {
        b.eye(from, |p| p);
        b.switch();
        b.advance(1.0 / 60.0);
        b.eye(to, |p| p);
    }
    long.advance(0.1);
    let one = long.eye(to, |p| p);
    for _ in 0..10 {
        short.advance(0.01);
        short.eye(to, |p| p);
    }
    let many = short.eye(to, |p| p);
    assert!(apart(one.position, many.position) < 1e-4);
}

#[test]
fn the_mouses_look_reaches_the_view_at_once_during_a_blend_and_after() {
    let mut b = CameraBlend::default();
    let behind = eye([0.0, -5.0, 3.0], 0.0, 0.3, None);
    b.eye(behind, |p| p);
    b.switch();
    let mut yaw = 0.0;
    let mut last = 0.0;
    for k in 0..40 {
        // The mouse turns the view a tenth of a radian on frame 10.
        if k == 10 {
            yaw += 0.1;
        }
        b.advance(1.0 / 60.0);
        let e = b.eye(eye([0.0, 0.0, 1.7], yaw, 0.0, None), |p| p);
        if k == 10 {
            assert!(
                (wrap(e.yaw - last) - 0.1).abs() < 0.02,
                "turned {}",
                e.yaw - last
            );
        }
        last = e.yaw;
    }
    // No blend: the eye is the mode's, frame by frame.
    let mut b = CameraBlend::default();
    for k in 0..10 {
        b.advance(1.0 / 144.0);
        let e = eye([0.0, k as f32, 1.7], k as f32 * 0.3, 0.1, Some(80));
        assert_eq!(b.eye(e, |p| p), e);
    }
}

#[test]
fn the_way_left_is_known_from_the_switchs_frame_on_and_none_once_arrived() {
    let mut b = CameraBlend::default();
    b.eye(walking(0.0, 0.0, true), |p| p);
    b.switch();
    assert!(b.remaining().is_infinite());
    let mut last = f32::INFINITY;
    for _ in 0..60 {
        b.advance(1.0 / 60.0);
        let e = b.eye(walking(0.0, 0.0, false), |p| p);
        let left = b.remaining();
        assert!((left - apart(e.position, walking(0.0, 0.0, false).position)).abs() < 1e-4);
        assert!(left <= last);
        last = left;
    }
    assert!(b.arrived() && b.remaining() == 0.0);
    b.switch();
    assert!(!b.arrived() && b.remaining().is_infinite());
    // No eye yet: the first switch has nothing to leave.
    let mut fresh = CameraBlend::default();
    fresh.switch();
    assert!(fresh.arrived());
    // A blend of no time cuts.
    let mut cut = CameraBlend::new(0.0);
    cut.eye(walking(0.0, 0.0, true), |p| p);
    cut.switch();
    cut.advance(0.016);
    let e = walking(0.0, 0.0, false);
    assert_eq!(cut.eye(e, |p| p), e);
}

#[test]
fn the_look_turns_the_short_way_round_and_eyes_are_kept_where_the_game_says() {
    let mut b = CameraBlend::default();
    b.eye(eye([0.0; 3], 3.0, 0.0, None), |p| p);
    b.switch();
    for _ in 0..3 {
        b.advance(1.0 / 60.0);
        let e = b.eye(eye([0.0; 3], -3.0, 0.0, None), |p| p);
        // From 3 to -3 the short way is through π, not through 0.
        assert!(e.yaw.abs() > 2.9, "{}", e.yaw);
    }
    let ground = 2.5;
    let mut b = CameraBlend::default();
    b.eye(eye([0.0, -6.0, 3.0], 0.0, 0.3, None), |p| p);
    b.switch();
    for _ in 0..8 {
        b.advance(1.0 / 60.0);
        let e = b.eye(eye([0.0, 10.0, 1.7], 0.0, 0.0, None), |p| {
            [p[0], p[1], p[2].max(ground)]
        });
        assert!(e.position[2] >= ground, "{e:?}");
    }
}
