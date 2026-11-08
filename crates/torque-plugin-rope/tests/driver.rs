//! Ends hung at an offset in a ghost's own frame, and a rope steered by a driver: the world's half of it (no engine, no network).

use torque_plugin_rope::{
    Anchor, Curve, DrawnShapes, Drive, DriveEnd, FrameAnchors, RopeEnd, RopeError, RopeId,
    RopeParams, RopeStyle, RopeWorld, Tangent,
};
use torque_render_scene::hd::DynamicObject;
use torque_render_scene::math::Mat;

fn params(segments: usize, length: f32) -> RopeParams {
    RopeParams {
        segments,
        length,
        ..RopeParams::default()
    }
}

/// A shape drawn at the pose `(at, yaw)`: turned by `yaw` about z and moved to `at`.
fn shape(ghost: u32, at: [f32; 3], yaw: f32) -> DynamicObject {
    let (s, c) = yaw.sin_cos();
    let transform = Mat {
        m: [
            c, -s, 0.0, at[0], s, c, 0.0, at[1], 0.0, 0.0, 1.0, at[2], 0.0, 0.0, 0.0, 1.0,
        ],
    };
    DynamicObject {
        ghost,
        transform,
        prev_transform: transform,
        translucent: false,
        centre: at,
        radius: 1.0,
        draws: Vec::new(),
        previous: None,
        stamp: None,
        nodes: Vec::new(),
    }
}

fn near(a: [f32; 3], b: [f32; 3], within: f32) -> bool {
    (0..3).all(|k| (a[k] - b[k]).abs() <= within)
}

/// The farthest any of `points` is from the segment between the first and the last.
fn off_the_segment(points: &[glam::Vec3]) -> f32 {
    let (a, b) = (points[0], points[points.len() - 1]);
    let ab = b - a;
    points
        .iter()
        .map(|&p| {
            let t = if ab.length_squared() > 0.0 {
                ((p - a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0)
            } else {
                0.0
            };
            p.distance(a + ab * t)
        })
        .fold(0.0, f32::max)
}

#[test]
fn the_default_mount_at_turns_the_offset_with_the_shape() {
    let at = [10.0, 20.0, 30.0];
    // A quarter turn about z: the shape's +x is the world's +y.
    let mount = DrawnShapes
        .mount_at(
            &shape(3, at, std::f32::consts::FRAC_PI_2),
            "",
            [2.0, 0.0, 1.0],
        )
        .expect("a mount");
    assert!(near(mount.at, [10.0, 22.0, 31.0], 1.0e-4), "{:?}", mount.at);
    // No offset is the shape's origin, as before.
    let origin = DrawnShapes
        .mount_at(&shape(3, at, 0.7), "", [0.0; 3])
        .unwrap();
    assert_eq!(
        origin.at,
        DrawnShapes.mount(&shape(3, at, 0.7), "").unwrap().at
    );
}

#[test]
fn an_end_hung_at_an_offset_rotates_with_the_ghost_and_is_where_it_is_drawn_every_frame() {
    let offset = [2.0, 0.5, 0.25];
    let pose = |t: f32| ([4.0 + t, 0.5 * (t * 1.3).sin(), 3.0], t * 2.0);
    let mut world = RopeWorld::new();
    let id = world
        .create(params(16, 9.0), [-4.0, 0.0, 3.0], [6.0, 0.0, 3.0])
        .unwrap();
    world.pin_ghost_at(id, RopeEnd::End, 7, "", offset).unwrap();
    assert_eq!(
        world.rope(id).unwrap().anchor(RopeEnd::End),
        Anchor::Ghost {
            ghost: 7,
            node: String::new(),
            offset
        }
    );
    // 144 frames a second, two seconds: the ghost turns almost four rad.
    let mut angles = Vec::new();
    for frame in 0..=288u32 {
        let t = frame as f32 / 144.0;
        let (at, yaw) = pose(t);
        world.step_frame_at(t, |ghost, node, offset| {
            (ghost == 7)
                .then(|| DrawnShapes.mount_at(&shape(7, at, yaw), node, offset))
                .flatten()
        });
        let end = world.rope(id).unwrap().drawn_points()[16].to_array();
        let want = DrawnShapes
            .mount_at(&shape(7, at, yaw), "", offset)
            .unwrap()
            .at;
        assert!(near(end, want, 1.0e-4), "frame {frame}: {end:?} {want:?}");
        // Seen from the ghost, the end never moves: the same length from its origin, turned by its yaw.
        let rel = [end[0] - at[0], end[1] - at[1], end[2] - at[2]];
        let flat = (rel[0] * rel[0] + rel[1] * rel[1]).sqrt();
        assert!(
            (flat - (offset[0] * offset[0] + offset[1] * offset[1]).sqrt()).abs() < 1.0e-3,
            "frame {frame}: {flat}"
        );
        assert!((rel[2] - offset[2]).abs() < 1.0e-4);
        angles.push(rel[1].atan2(rel[0]));
    }
    assert!(
        angles.windows(2).any(|w| (w[1] - w[0]).abs() > 1.0e-3),
        "the end went round"
    );
}

#[test]
fn an_end_pinned_without_an_offset_is_where_it_was() {
    let mut world = RopeWorld::new();
    let id = world
        .create(params(8, 5.0), [0.0, 0.0, 3.0], [4.0, 0.0, 3.0])
        .unwrap();
    world.pin_ghost(id, RopeEnd::End, 7, "").unwrap();
    assert!(matches!(
        world.rope(id).unwrap().anchor(RopeEnd::End),
        Anchor::Ghost {
            offset: [0.0, 0.0, 0.0],
            ..
        }
    ));
    // The two-argument frame step still works and gives the offset no say.
    world.step_frame(0.0, |_, _| None);
    world.step_frame(0.1, |ghost, _| {
        (ghost == 7).then_some(torque_plugin_rope::Mount {
            at: [4.0, 1.0, 3.0],
            dir: None,
        })
    });
    assert!(near(
        world.rope(id).unwrap().drawn_points()[8].to_array(),
        [4.0, 1.0, 3.0],
        1.0e-5
    ));
}

#[test]
fn an_offset_end_with_no_drawn_frame_follows_the_ghosts_position_plus_the_offset() {
    let mut world = RopeWorld::new();
    let id = world
        .create(params(8, 5.0), [0.0, 0.0, 3.0], [4.0, 0.0, 3.0])
        .unwrap();
    world
        .pin_ghost_at(id, RopeEnd::End, 7, "", [0.0, 2.0, 0.0])
        .unwrap();
    world.follow_ghosts(|ghost, _| (ghost == 7).then_some([5.0, 0.0, 1.0]));
    for _ in 0..200 {
        world.advance(0.032);
    }
    let end = world.rope(id).unwrap().points().last().unwrap();
    assert!(near(end, [5.0, 2.0, 1.0], 0.05), "{end:?}");
}

fn two_points(a: [f32; 3], b: [f32; 3]) -> Drive {
    Drive::new(DriveEnd::Point(a), DriveEnd::Point(b))
}

fn rope_of(world: &mut RopeWorld, segments: usize, length: f32) -> RopeId {
    world
        .create(params(segments, length), [0.0, 0.0, 3.0], [4.0, 0.0, 3.0])
        .unwrap()
}

#[test]
fn a_driver_puts_the_ends_exactly_where_it_says_in_the_next_frame() {
    let mut world = RopeWorld::new();
    let id = rope_of(&mut world, 12, 8.0);
    for frame in 0..=240u32 {
        let t = frame as f32 / 120.0;
        let a = [t, 0.0, 2.0];
        let b = [6.0 - t, 1.0 + t.sin(), 3.0 + t.cos()];
        world.drive(id, &two_points(a, b)).unwrap();
        world.step_frame(t, |_, _| None);
        let shown = world.rope(id).unwrap().drawn_points();
        assert!(near(shown[0].to_array(), a, 1.0e-5), "frame {frame}");
        assert!(near(shown[12].to_array(), b, 1.0e-5), "frame {frame}");
    }
}

#[test]
fn a_rope_whose_driver_says_it_is_at_its_limit_lies_on_the_segment_between_its_ends() {
    let mut world = RopeWorld::new();
    let id = rope_of(&mut world, 16, 6.0);
    // The ends are 5 apart and the rope 6 long: without the limit it hangs; with it, it is straight and even.
    let mut hanging = two_points([0.0, 0.0, 3.0], [5.0, 0.0, 3.0]);
    hanging.length = Some(6.0);
    let mut taut = hanging.clone();
    taut.at_limit = true;
    for t in 0..120 {
        world.drive(id, &hanging).unwrap();
        world.step_frame(t as f32 / 60.0, |_, _| None);
    }
    let hung = off_the_segment(world.rope(id).unwrap().drawn_points());
    assert!(hung > 0.05, "a slack rope hangs: {hung}");
    assert!(!world.rope(id).unwrap().taut());
    for t in 120..240 {
        world.drive(id, &taut).unwrap();
        world.step_frame(t as f32 / 60.0, |_, _| None);
    }
    let rope = world.rope(id).unwrap();
    assert!(rope.at_limit() && rope.taut());
    let shown = rope.drawn_points();
    assert!(
        off_the_segment(shown) < 1.0e-3,
        "{}",
        off_the_segment(shown)
    );
    // Evenly spaced along it.
    let steps: Vec<f32> = shown.windows(2).map(|w| w[0].distance(w[1])).collect();
    let (lo, hi) = steps
        .iter()
        .fold((f32::MAX, 0.0f32), |(lo, hi), &s| (lo.min(s), hi.max(s)));
    assert!(hi - lo < 1.0e-3, "{lo} {hi}");
    // Moving the ends while at the limit keeps it straight every frame, at any pose.
    for t in 240..480 {
        let s = t as f32 / 60.0;
        let mut drive = taut.clone();
        drive.start = DriveEnd::Point([0.0, s.sin(), 2.0]);
        drive.end = DriveEnd::Point([4.0 + s.cos(), 3.0, 3.0 + s.sin()]);
        world.drive(id, &drive).unwrap();
        world.step_frame(s, |_, _| None);
        let shown = world.rope(id).unwrap().drawn_points();
        assert!(off_the_segment(shown) < 1.0e-3, "frame {t}");
    }
    // Letting the limit go lets it hang again.
    for t in 480..600 {
        let mut drive = hanging.clone();
        drive.start = DriveEnd::Point([0.0, 0.0, 3.0]);
        drive.end = DriveEnd::Point([5.0, 0.0, 3.0]);
        world.drive(id, &drive).unwrap();
        world.step_frame(t as f32 / 60.0, |_, _| None);
    }
    assert!(!world.rope(id).unwrap().at_limit());
    assert!(off_the_segment(world.rope(id).unwrap().drawn_points()) > 0.05);
}

#[test]
fn a_ghost_end_from_a_driver_follows_the_ghost_every_frame_with_no_more_calls() {
    let mut world = RopeWorld::new();
    let id = rope_of(&mut world, 10, 5.0);
    let mut drive = Drive::new(
        DriveEnd::Point([0.0, 0.0, 3.0]),
        DriveEnd::Ghost {
            ghost: 9,
            node: String::new(),
            offset: [1.0, 0.0, 0.0],
        },
    );
    drive.at_limit = true;
    world.drive(id, &drive).unwrap();
    for frame in 0..=120u32 {
        let t = frame as f32 / 60.0;
        let (at, yaw) = ([3.0, t, 2.0], t * 3.0);
        world.step_frame_at(t, |ghost, node, offset| {
            (ghost == 9)
                .then(|| DrawnShapes.mount_at(&shape(9, at, yaw), node, offset))
                .flatten()
        });
        let want = DrawnShapes
            .mount_at(&shape(9, at, yaw), "", [1.0, 0.0, 0.0])
            .unwrap()
            .at;
        let shown = world.rope(id).unwrap().drawn_points();
        assert!(near(shown[10].to_array(), want, 1.0e-4), "frame {frame}");
        assert!(off_the_segment(shown) < 1.0e-3, "frame {frame}");
    }
}

#[test]
fn a_driver_sets_the_length_at_once_first_and_then_over_the_settle_time() {
    let mut world = RopeWorld::new();
    let id = rope_of(&mut world, 8, 5.0);
    let mut drive = two_points([0.0, 0.0, 3.0], [4.0, 0.0, 3.0]);
    drive.length = Some(7.0);
    drive.settle = 0.5;
    world.drive(id, &drive).unwrap();
    assert!((world.rope(id).unwrap().length() - 7.0).abs() < 1.0e-4);
    assert!(!world.rope(id).unwrap().reeling());
    // A new length reels over the settle time.
    drive.length = Some(3.0);
    world.drive(id, &drive).unwrap();
    assert!(world.rope(id).unwrap().reeling());
    for t in 0..=40 {
        world.step_frame(t as f32 / 60.0, |_, _| None);
    }
    assert!((world.rope(id).unwrap().length() - 3.0).abs() < 1.0e-3);
    // The same length again starts no reel; a drive with no length leaves the rope its own.
    world.drive(id, &drive).unwrap();
    assert!(!world.rope(id).unwrap().reeling());
    drive.length = None;
    world.drive(id, &drive).unwrap();
    world.set_length(id, 4.5, 0.0).unwrap();
    assert!((world.rope(id).unwrap().length() - 4.5).abs() < 1.0e-4);
    // Hostile lengths are held to the rope's range, and a NaN does nothing.
    drive.length = Some(f32::NAN);
    world.drive(id, &drive).unwrap();
    assert!((world.rope(id).unwrap().length() - 4.5).abs() < 1.0e-4);
    drive.length = Some(1.0e9);
    world.drive(id, &drive).unwrap();
    assert!(world.rope(id).unwrap().length() <= torque_plugin_rope::MAX_LENGTH + 1.0e-3);
}

#[test]
fn a_driver_gives_a_rope_a_curve_that_passes_through_both_ends() {
    let mut world = RopeWorld::new();
    let id = rope_of(&mut world, 16, 5.0);
    let mut drive = two_points([0.0, 0.0, 1.0], [6.0, 2.0, 1.0]);
    drive.curve = Some(Curve {
        start: Tangent::Fixed([0.0, 0.0, 1.0]),
        end: Tangent::Fixed([0.0, 0.0, 1.0]),
        bend: 0.5,
    });
    world.drive(id, &drive).unwrap();
    world.step_frame(0.0, |_, _| None);
    world.step_frame(0.016, |_, _| None);
    let rope = world.rope(id).unwrap();
    assert!(rope.driven() && rope.curve().is_some());
    let shown = rope.drawn_points();
    assert!(near(shown[0].to_array(), [0.0, 0.0, 1.0], 1.0e-5));
    assert!(near(shown[16].to_array(), [6.0, 2.0, 1.0], 1.0e-5));
    assert!(shown[8].z > 1.2, "it bows up: {:?}", shown[8]);
    // No curve takes the driven shape away again.
    drive.curve = None;
    world.drive(id, &drive).unwrap();
    assert!(world.rope(id).unwrap().curve().is_none());
}

#[test]
fn driving_what_is_already_so_changes_nothing() {
    let mut world = RopeWorld::new();
    let id = rope_of(&mut world, 8, 5.0);
    let mut drive = Drive::new(
        DriveEnd::Point([0.0, 0.0, 3.0]),
        DriveEnd::Ghost {
            ghost: 2,
            node: "3".to_string(),
            offset: [0.0, 1.0, 0.0],
        },
    );
    drive.length = Some(5.0);
    drive.at_limit = true;
    world.drive(id, &drive).unwrap();
    let revision = world.revision();
    for _ in 0..10 {
        world.drive(id, &drive).unwrap();
    }
    assert_eq!(world.revision(), revision);
    // A rope that is not there is refused.
    assert_eq!(world.drive(99, &drive), Err(RopeError::NoSuchRope(99)));
}

#[test]
fn a_driver_can_let_an_end_go_and_a_line_keeps_its_length() {
    let mut world = RopeWorld::new();
    let id = rope_of(&mut world, 8, 5.0);
    let mut drive = Drive::new(DriveEnd::Point([0.0, 0.0, 3.0]), DriveEnd::Free);
    world.drive(id, &drive).unwrap();
    assert_eq!(world.rope(id).unwrap().anchor(RopeEnd::End), Anchor::Free);
    assert!(!world.rope(id).unwrap().taut());
    let line = world
        .create(
            RopeParams {
                style: RopeStyle::Line,
                ..params(2, 4.0)
            },
            [0.0, 0.0, 0.0],
            [3.0, 0.0, 0.0],
        )
        .unwrap();
    drive.end = DriveEnd::Point([0.0, 4.0, 0.0]);
    drive.length = Some(9.0);
    world.drive(line, &drive).unwrap();
    world.step_frame(0.0, |_, _| None);
    world.step_frame(0.05, |_, _| None);
    let shown = world.rope(line).unwrap().drawn_points();
    assert!(near(shown[1].to_array(), [0.0, 4.0, 0.0], 1.0e-5));
}

#[test]
fn hostile_numbers_from_a_driver_never_make_a_nan() {
    let mut world = RopeWorld::new();
    let id = rope_of(&mut world, 12, 5.0);
    let bad = [f32::NAN, f32::INFINITY, -f32::INFINITY, 1.0e30];
    for (i, &x) in bad.iter().enumerate() {
        let mut drive = Drive::new(
            DriveEnd::Point([x, 0.0, 3.0]),
            DriveEnd::Ghost {
                ghost: 1,
                node: String::new(),
                offset: [0.0, x, 0.0],
            },
        );
        drive.length = Some(x);
        drive.settle = x;
        drive.at_limit = i % 2 == 0;
        drive.curve = Some(Curve {
            start: Tangent::Fixed([x, x, x]),
            end: Tangent::Auto,
            bend: x,
        });
        world.drive(id, &drive).unwrap();
        for k in 0..30 {
            world.step_frame_at((i * 30 + k) as f32 / 60.0, |_, _, offset| {
                Some(torque_plugin_rope::Mount {
                    at: offset,
                    dir: Some([x, 0.0, 0.0]),
                })
            });
        }
        assert!(
            world.rope(id).unwrap().points().all(|p| p
                .iter()
                .all(|c| c.is_finite() && c.abs() <= torque_plugin_rope::sim::MAX_COORD)),
            "round {i}"
        );
    }
}

#[test]
fn letting_go_of_a_driver_ends_the_limit_and_keeps_the_ends_and_the_length() {
    let mut world = RopeWorld::new();
    let id = rope_of(&mut world, 8, 5.0);
    let mut drive = two_points([0.0, 0.0, 3.0], [4.0, 0.0, 3.0]);
    drive.length = Some(6.0);
    drive.at_limit = true;
    world.drive(id, &drive).unwrap();
    world.release_drive(id);
    let rope = world.rope(id).unwrap();
    assert!(!rope.at_limit());
    assert_eq!(rope.anchor(RopeEnd::End), Anchor::Point([4.0, 0.0, 3.0]));
    assert!((rope.length() - 6.0).abs() < 1.0e-4);
    // The next driver's first length is set at once again.
    drive.length = Some(3.0);
    drive.settle = 1.0;
    world.drive(id, &drive).unwrap();
    assert!(!world.rope(id).unwrap().reeling());
}

#[test]
fn a_long_rope_is_not_reeled_again_every_tick_for_a_length_it_has() {
    let mut world = RopeWorld::new();
    let long = torque_plugin_rope::MAX_LENGTH;
    let id = world
        .create(params(8, long), [0.0, 0.0, 3.0], [4.0, 0.0, 3.0])
        .unwrap();
    let mut drive = two_points([0.0, 0.0, 3.0], [4.0, 0.0, 3.0]);
    drive.length = Some(long * 0.9137);
    drive.settle = 0.032;
    world.drive(id, &drive).unwrap();
    let revision = world.revision();
    for _ in 0..20 {
        world.drive(id, &drive).unwrap();
    }
    assert_eq!(world.revision(), revision);
    assert!(!world.rope(id).unwrap().reeling());
}
