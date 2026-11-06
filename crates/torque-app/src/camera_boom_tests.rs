use super::*;

type Boxes = Vec<([f32; 3], [f32; 3])>;

/// The share of the way from `from` to `to` at which the segment enters one of `boxes` (a start inside one does not count).
fn cast(boxes: &Boxes, from: [f32; 3], to: [f32; 3]) -> Option<f32> {
    let d = [0, 1, 2].map(|i| to[i] - from[i]);
    let mut best: Option<f32> = None;
    for (lo, hi) in boxes {
        let (mut near, mut far) = (0.0f32, 1.0f32);
        let mut miss = false;
        for i in 0..3 {
            if d[i].abs() < 1e-9 {
                if from[i] < lo[i] || from[i] > hi[i] {
                    miss = true;
                }
                continue;
            }
            let a = (lo[i] - from[i]) / d[i];
            let b = (hi[i] - from[i]) / d[i];
            near = near.max(a.min(b));
            far = far.min(a.max(b));
        }
        if !miss && near <= far && near > 0.0 && best.is_none_or(|b| near < b) {
            best = Some(near);
        }
    }
    best
}

fn inside(boxes: &Boxes, p: [f32; 3]) -> bool {
    boxes
        .iter()
        .any(|(lo, hi)| (0..3).all(|i| p[i] > lo[i] && p[i] < hi[i]))
}

const PIVOT: [f32; 3] = [0.0, 0.0, 1.7];
const BACK: [f32; 3] = [0.0, -1.0, 0.0];
const FULL: f32 = 6.0;
const RATES: [f32; 2] = [60.0, 144.0];

/// A post 0.2 m thick across the line 3 m behind the pivot.
fn post() -> Boxes {
    vec![([-1.0, -3.1, 0.0], [1.0, -2.9, 3.0])]
}

/// Frames at `fps` for `seconds` of a boom behind the pivot among `boxes`: (time, length).
fn lengths(boom: &mut CameraBoom, boxes: &Boxes, fps: f32, seconds: f32) -> Vec<(f32, f32)> {
    let dt = 1.0 / fps;
    let n = (seconds * fps).round() as usize;
    (1..=n)
        .map(|k| {
            let p = boom.follow(PIVOT, BACK, FULL, dt, |a, b| cast(boxes, a, b));
            assert!(!inside(boxes, p), "the camera in a box at {p:?}");
            (k as f32 * dt, -p[1])
        })
        .collect()
}

#[test]
fn a_post_across_the_line_pulls_the_camera_in_fast_and_its_going_lets_it_out_slowly() {
    let in_room = 2.9 - CLEARANCE;
    for fps in RATES {
        let mut boom = CameraBoom::default();
        let open = lengths(&mut boom, &Vec::new(), fps, 0.2);
        assert!((open.last().unwrap().1 - FULL).abs() < 1e-5);
        // The post comes: no jump to it, 95% of the way in after the pull-in time.
        let pulled = lengths(&mut boom, &post(), fps, 0.5);
        let first = pulled[0].1;
        assert!(
            first > in_room + 0.5,
            "{fps} fps: {first} on the first frame"
        );
        let at = |frames: &[(f32, f32)], t: f32| frames.iter().find(|f| f.0 >= t - 1e-4).unwrap().1;
        let way = FULL - in_room;
        let left = at(&pulled, PULL_IN_SECONDS) - in_room;
        assert!(
            left <= 0.05 * way + 0.02 && left >= 0.0,
            "{fps} fps: {left} left at the pull-in time"
        );
        assert!(at(&pulled, 0.4) - in_room < 1e-3);
        // Gone: out again, 95% of the way by the ease-out time, never past the full length.
        let eased = lengths(&mut boom, &Vec::new(), fps, 1.2);
        let left = FULL - at(&eased, EASE_OUT_SECONDS);
        assert!(
            left <= 0.05 * way + 0.02 && left >= 0.0,
            "{fps} fps: {left} left at the ease-out time"
        );
        assert!(eased.iter().all(|f| f.1 <= FULL + 1e-5));
        assert!(at(&eased, 0.1) < FULL - 0.5 * way, "out too fast");
    }
}

#[test]
fn the_lengths_are_the_same_at_any_frame_rate() {
    let run = |fps: f32| {
        let mut boom = CameraBoom::default();
        lengths(&mut boom, &Vec::new(), fps, 0.1);
        let mut out = lengths(&mut boom, &post(), fps, 0.5);
        out.extend(lengths(&mut boom, &Vec::new(), fps, 1.0));
        out
    };
    let (a, b) = (run(60.0), run(150.0));
    // Every second frame at 60 falls on every fifth at 150. The last bit past the post is one step, on each rate's own frame:
    // one frame may differ there.
    let mut apart = Vec::new();
    for i in (1..a.len()).step_by(2) {
        let g = b[(i + 1) * 5 / 2 - 1];
        assert!((a[i].0 - g.0).abs() < 1e-4);
        if (a[i].1 - g.1).abs() > 1e-3 {
            apart.push((a[i].0, a[i].1, g.1));
        }
    }
    assert!(apart.len() <= 1, "{apart:?}");
}

#[test]
fn backing_into_a_wall_pushes_the_camera_in_with_it_and_never_through() {
    // A wall 0.4 m thick 5 m behind the start; the pivot backs toward it at 3 m/s.
    let wall: Boxes = vec![([-5.0, -5.4, 0.0], [5.0, -5.0, 4.0])];
    for fps in RATES {
        let mut boom = CameraBoom::new(0.4, CLEARANCE);
        let dt = 1.0 / fps;
        let mut pivot = PIVOT;
        for _ in 0..(1.2 * fps) as usize {
            pivot[1] -= 3.0 * dt;
            let p = boom.follow(pivot, BACK, FULL, dt, |a, b| cast(&wall, a, b));
            assert!(p[1] >= -5.0 + 0.1, "{fps} fps: into the wall at {p:?}");
        }
    }
}

#[test]
fn a_wall_the_boom_grazes_keeps_the_near_plane_off_it() {
    // A wall along the boom, its face 0.1 m to the side of the line from 2 m back on: the middle line never meets it.
    let wall: Boxes = vec![([0.1, -20.0, 0.0], [0.5, -2.0, 4.0])];
    assert!(cast(&wall, PIVOT, [0.0, -FULL, 1.7]).is_none());
    let mut boom = CameraBoom::default();
    let p = boom.follow(PIVOT, BACK, FULL, 1.0 / 60.0, |a, b| cast(&wall, a, b));
    assert!(-p[1] < FULL - 1.0, "{p:?}");
    // With nothing in the way the camera is at the boom's end.
    let mut boom = CameraBoom::default();
    let p = boom.follow(PIVOT, BACK, FULL, 1.0 / 60.0, |_, _| None);
    assert_eq!(p, [0.0, -FULL, 1.7]);
}

#[test]
fn a_frame_time_that_is_not_a_time_cuts_to_the_room_and_the_boom_goes_on() {
    for bad in [f32::INFINITY, f32::NAN, f32::NEG_INFINITY] {
        let mut boom = CameraBoom::default();
        lengths(&mut boom, &Vec::new(), 60.0, 0.1);
        let p = boom.follow(PIVOT, BACK, FULL, bad, |a, b| cast(&post(), a, b));
        assert!(p.iter().all(|x| x.is_finite()), "{bad}: {p:?}");
        assert!((-p[1] - (2.9 - CLEARANCE)).abs() < 1e-4, "{bad}: {p:?}");
        // Later frames are lengths again: out slowly once the post is gone.
        let out = lengths(&mut boom, &Vec::new(), 60.0, 1.5);
        assert!(out.iter().all(|f| f.1.is_finite()));
        assert!(
            (out.last().unwrap().1 - FULL).abs() < 0.01,
            "{bad}: {:?}",
            out.last()
        );
    }
}
