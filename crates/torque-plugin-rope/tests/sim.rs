//! The rope simulation on its own: a rope sags and settles, stays stable over ten thousand steps whatever moves its ends, keeps
//! to its caps, and takes hostile numbers without a NaN.

use torque_plugin_rope::sim::{MAX_COORD, SUBSTEP_SECONDS};
use torque_plugin_rope::{
    Anchor, MAX_ROPES, MAX_SEGMENTS_PER_ROPE, MAX_TOTAL_SEGMENTS, Mount, RopeData, RopeEnd,
    RopeError, RopeId, RopeParams, RopeStyle, RopeWorld,
};

const TICK: f32 = 0.032;

fn params(segments: usize, length: f32) -> RopeParams {
    RopeParams {
        segments,
        length,
        ..RopeParams::default()
    }
}

fn finite(world: &RopeWorld) -> bool {
    world.ropes().all(|(_, rope)| {
        rope.points()
            .all(|p| p.iter().all(|c| c.is_finite() && c.abs() <= MAX_COORD))
    })
}

#[test]
fn a_rope_between_two_posts_sags_below_them_and_keeps_its_length() {
    let mut world = RopeWorld::new();
    let id = world
        .create(params(16, 10.0), [-4.0, 0.0, 3.0], [4.0, 0.0, 3.0])
        .unwrap();
    for _ in 0..500 {
        world.advance(TICK);
    }
    let rope = world.rope(id).unwrap();
    let points: Vec<[f32; 3]> = rope.points().collect();
    assert_eq!(points.len(), 17);
    assert_eq!(points[0], [-4.0, 0.0, 3.0]);
    assert_eq!(points[16], [4.0, 0.0, 3.0]);
    let lowest = points.iter().map(|p| p[2]).fold(f32::MAX, f32::min);
    assert!(
        lowest < 1.0,
        "the middle hangs well below the posts: {lowest}"
    );
    let arc: f32 = points
        .windows(2)
        .map(|w| {
            let d = [w[1][0] - w[0][0], w[1][1] - w[0][1], w[1][2] - w[0][2]];
            (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
        })
        .sum();
    assert!((arc - 10.0).abs() < 0.3, "the arc is its length: {arc}");
    assert!(rope.speed() < 0.05, "it settled: {}", rope.speed());
    assert!(rope.strain() < 0.05, "{}", rope.strain());
    assert_eq!(rope.anchor(RopeEnd::Start), Anchor::Point([-4.0, 0.0, 3.0]));
}

#[test]
fn a_rope_with_no_length_is_a_tenth_longer_than_its_ends_are_apart() {
    let mut world = RopeWorld::new();
    let id = world
        .create(params(10, 0.0), [0.0, 0.0, 5.0], [10.0, 0.0, 5.0])
        .unwrap();
    assert!((world.rope(id).unwrap().length() - 11.0).abs() < 1e-4);
}

#[test]
fn a_released_end_falls_and_swings_from_the_other() {
    let mut world = RopeWorld::new();
    let id = world
        .create(params(12, 6.0), [0.0, 0.0, 8.0], [5.0, 0.0, 8.0])
        .unwrap();
    world.release(id, RopeEnd::End).unwrap();
    for _ in 0..700 {
        world.advance(TICK);
    }
    let rope = world.rope(id).unwrap();
    let end = rope.points().last().unwrap();
    assert!(
        end[2] < 3.0,
        "the free end hangs under the pinned one: {end:?}"
    );
    assert!(end[0].abs() < 1.5, "straight below it: {end:?}");
    assert_eq!(rope.anchor(RopeEnd::End), Anchor::Free);
}

#[test]
fn a_floor_holds_the_rope_up() {
    let mut world = RopeWorld::new();
    world.set_floor(Some(0.0));
    let id = world
        .create(params(16, 12.0), [-2.0, 0.0, 4.0], [2.0, 0.0, 4.0])
        .unwrap();
    for _ in 0..300 {
        world.advance(TICK);
    }
    let radius = RopeParams::default().thickness * 0.5;
    let lowest = world
        .rope(id)
        .unwrap()
        .points()
        .map(|p| p[2])
        .fold(f32::MAX, f32::min);
    assert!(lowest >= radius - 1e-4, "{lowest}");
    assert!(lowest < radius + 0.1, "it lies on the floor: {lowest}");
}

/// Two ends close together with a long rope between them: a cord hangs in a tight fold, a stiff rope will not fold so tightly and
/// its lowest point is higher.
#[test]
fn a_stiff_rope_will_not_fold_as_tightly_as_a_cord() {
    let lowest = |stiffness: f32| {
        let mut world = RopeWorld::new();
        let id = world
            .create(
                RopeParams {
                    stiffness,
                    ..params(24, 6.0)
                },
                [0.0, 0.0, 5.0],
                [0.5, 0.0, 5.0],
            )
            .unwrap();
        for _ in 0..600 {
            world.advance(TICK);
        }
        world
            .rope(id)
            .unwrap()
            .points()
            .map(|p| p[2])
            .fold(f32::MAX, f32::min)
    };
    let (cord, stiff) = (lowest(0.0), lowest(0.5));
    assert!(
        (cord - 2.0).abs() < 0.2,
        "a cord hangs 3 units down: {cord}"
    );
    assert!(stiff > cord + 0.5, "{stiff} {cord}");
}

/// Ten thousand ticks of ropes whose ends are dragged about by a deterministic wobble, with pushes, a floor and ends let go and
/// taken up again (an end jumping across the world is followed at a bounded speed): no coordinate leaves the finite range and no
/// piece stretches past a few times its length; then, with the ends left alone, the ropes settle to pieces of their resting length.
#[test]
fn a_rope_stays_stable_over_ten_thousand_steps() {
    let mut world = RopeWorld::new();
    world.set_floor(Some(0.0));
    let ids: Vec<_> = [(8, 6.0), (24, 12.0), (64, 30.0)]
        .iter()
        .map(|&(n, length)| {
            world
                .create(params(n, length), [0.0, 0.0, 6.0], [5.0, 1.0, 6.0])
                .unwrap()
        })
        .collect();
    let mut worst: f32 = 0.0;
    for step in 0..10_000u32 {
        let t = step as f32 * TICK;
        let wobble = [
            8.0 * (t * 0.7).sin(),
            5.0 * (t * 1.3).cos(),
            4.0 + 3.0 * (t * 0.5).sin(),
        ];
        for (k, &id) in ids.iter().enumerate() {
            let k = k as f32;
            world
                .pin_point(id, RopeEnd::End, [wobble[0] + k, wobble[1], wobble[2]])
                .unwrap();
            if step % 997 == 0 {
                world.impulse(id, [30.0, -20.0, 40.0]).unwrap();
            }
            if step % 1500 == 700 {
                world.release(id, RopeEnd::Start).unwrap();
            }
            if step % 1500 == 900 {
                world
                    .pin_point(id, RopeEnd::Start, [0.0, 0.0, 6.0])
                    .unwrap();
            }
        }
        world.advance(TICK);
        assert!(finite(&world), "step {step}");
        for &id in &ids {
            worst = worst.max(world.rope(id).unwrap().strain());
        }
    }
    assert!(
        worst < 3.0,
        "no piece stretched past three times its length: {worst}"
    );
    // Left alone, they settle.
    for &id in &ids {
        world.pin_point(id, RopeEnd::End, [4.0, 0.0, 6.0]).unwrap();
        world
            .pin_point(id, RopeEnd::Start, [0.0, 0.0, 6.0])
            .unwrap();
    }
    for _ in 0..1500 {
        world.advance(TICK);
    }
    for &id in &ids {
        let rope = world.rope(id).unwrap();
        assert!(rope.strain() < 0.1, "rope {id}: {}", rope.strain());
        assert!(rope.speed() < 0.5, "rope {id}: {}", rope.speed());
    }
}

#[test]
fn a_settled_rope_has_pieces_of_their_resting_length() {
    let mut world = RopeWorld::new();
    let id = world
        .create(params(24, 12.0), [0.0, 0.0, 6.0], [8.0, 0.0, 6.0])
        .unwrap();
    for _ in 0..400 {
        world.advance(TICK);
    }
    assert!(world.rope(id).unwrap().strain() < 0.05);
}

#[test]
fn ends_farther_apart_than_the_rope_is_long_hold_it_taut() {
    let mut world = RopeWorld::new();
    let id = world
        .create(params(10, 4.0), [0.0, 0.0, 3.0], [10.0, 0.0, 3.0])
        .unwrap();
    for _ in 0..200 {
        world.advance(TICK);
    }
    assert!(finite(&world));
    let rope = world.rope(id).unwrap();
    let mid = rope.points().nth(5).unwrap();
    assert!(
        (mid[2] - 3.0).abs() < 0.3,
        "a stretched rope is a line: {mid:?}"
    );
}

#[test]
fn the_caps_hold() {
    let mut world = RopeWorld::new();
    for _ in 0..MAX_ROPES {
        world
            .create(params(2, 1.0), [0.0; 3], [1.0, 0.0, 0.0])
            .unwrap();
    }
    assert_eq!(
        world.create(params(2, 1.0), [0.0; 3], [1.0, 0.0, 0.0]),
        Err(RopeError::TooManyRopes)
    );
    assert_eq!(world.len(), MAX_ROPES);

    let mut world = RopeWorld::new();
    let per = MAX_SEGMENTS_PER_ROPE;
    for _ in 0..MAX_TOTAL_SEGMENTS / per {
        world
            .create(params(per, 10.0), [0.0; 3], [1.0, 0.0, 0.0])
            .unwrap();
    }
    assert_eq!(
        world.create(params(2, 1.0), [0.0; 3], [1.0, 0.0, 0.0]),
        Err(RopeError::TooManySegments)
    );
    assert_eq!(world.total_segments(), MAX_TOTAL_SEGMENTS);
    // A rope asking for more pieces than one may have is cut to the most.
    let mut world = RopeWorld::new();
    let id = world
        .create(params(5_000, 10.0), [0.0; 3], [1.0, 0.0, 0.0])
        .unwrap();
    assert_eq!(world.rope(id).unwrap().segments(), MAX_SEGMENTS_PER_ROPE);
    // Taking one away makes room.
    assert!(world.remove(id));
    assert!(!world.remove(id));
    assert!(world.is_empty());
    assert_eq!(world.impulse(id, [0.0; 3]), Err(RopeError::NoSuchRope(id)));
}

#[test]
fn hostile_numbers_never_make_a_nan() {
    let nan = f32::NAN;
    let inf = f32::INFINITY;
    // A datablock of nothing but bad numbers.
    let data = RopeData {
        style: "line".to_string(),
        emissive: true,
        segments: i32::MIN,
        length: nan,
        thickness: -inf,
        stiffness: nan,
        damping: inf,
        gravity: f32::MAX,
        color: [0; 4],
        cast_shadow: 1,
        receive_shadow: 0,
        texture: torque_engine::Filename("nan.png".to_string()),
        texture_length: nan,
        texture_scroll: inf,
        texture_filter: "x".repeat(10_000),
        sim: "none".to_string(),
        emissive_intensity: nan,
        additive: true,
        end_thickness: -inf,
        color_end: "nan 0 0".to_string(),
    };
    let params = RopeParams::new(&data);
    assert!((2..=MAX_SEGMENTS_PER_ROPE).contains(&params.segments));
    for v in [
        params.length,
        params.thickness,
        params.stiffness,
        params.damping,
        params.gravity,
    ] {
        assert!(v.is_finite(), "{params:?}");
    }
    let mut world = RopeWorld::new();
    let a = world
        .create(params, [nan, inf, -inf], [1e38, -1e38, 0.0])
        .unwrap();
    let b = world
        .create(
            RopeParams::new(&RopeData {
                segments: i32::MAX,
                length: f32::MAX,
                thickness: f32::MAX,
                stiffness: 7.0,
                damping: -3.0,
                gravity: -9.0,
                ..data.clone()
            }),
            [0.0; 3],
            [0.0; 3],
        )
        .unwrap();
    world.set_floor(Some(nan));
    for step in 0..2_000 {
        match step % 5 {
            0 => world.advance(nan),
            1 => world.advance(inf),
            2 => world.advance(-1.0),
            3 => world.advance(1e30),
            _ => world.advance(TICK),
        }
        if step % 50 == 0 {
            world.pin_point(a, RopeEnd::End, [nan, inf, 1e30]).unwrap();
            world.impulse(b, [inf, nan, 1e30]).unwrap();
            world.pin_object(a, RopeEnd::Start, 7, "").unwrap();
            world.resolve(|_, _| Some([nan, inf, -1e30]));
        }
        assert!(finite(&world), "step {step}");
    }
    let mesh = world.mesh();
    assert!(
        mesh.vertices
            .iter()
            .all(|v| v.position.iter().chain(&v.normal).all(|c| c.is_finite()))
    );
}

#[test]
fn a_long_stall_runs_a_bounded_number_of_substeps() {
    let mut world = RopeWorld::new();
    world
        .create(params(8, 5.0), [0.0; 3], [3.0, 0.0, 0.0])
        .unwrap();
    world.advance(60.0);
    assert!(world.substeps() <= 16, "{}", world.substeps());
    // Whole substeps only: a tick is 3.84 of them, the remainder carried.
    let before = world.substeps();
    for _ in 0..25 {
        world.advance(TICK);
    }
    let ran = world.substeps() - before;
    assert!(
        ran.abs_diff((25.0 * TICK / SUBSTEP_SECONDS) as u64) <= 1,
        "{ran}"
    );
}

#[test]
fn the_tubes_are_closed_and_the_casters_come_first() {
    let mut world = RopeWorld::new();
    let a = world
        .create(
            RopeParams {
                cast_shadow: false,
                ..params(6, 4.0)
            },
            [0.0; 3],
            [3.0, 0.0, 0.0],
        )
        .unwrap();
    let b = world
        .create(params(6, 4.0), [0.0, 5.0, 0.0], [3.0, 5.0, 0.0])
        .unwrap();
    assert!(a < b);
    let mesh = world.mesh();
    assert_eq!(mesh.indices.len() % 3, 0);
    assert!(
        mesh.indices
            .iter()
            .all(|&i| (i as usize) < mesh.vertices.len())
    );
    // Rings for 7 points and 4 for the caps, 8 sides, two triangles a side between neighboring rings, two ropes.
    assert_eq!(mesh.vertices.len(), 2 * 11 * 9);
    assert_eq!(mesh.indices.len(), 2 * 10 * 8 * 6);
    assert_eq!(mesh.shadow_indices, mesh.indices.len() / 2);
    // One run of indices for each rope (neither has a texture), the casting one first.
    let runs: Vec<(bool, u32, u32)> = mesh
        .draws
        .iter()
        .map(|d| (d.casting, d.first, d.count))
        .collect();
    let half = mesh.indices.len() as u32 / 2;
    assert_eq!(runs, [(true, 0, half), (false, half, half)]);
    // The casting rope (the second) is first: its vertices are the ones near y = 5.
    let first = mesh.indices[0] as usize;
    assert!((mesh.vertices[first].position[1] - 5.0).abs() < 0.1);
    for v in &mesh.vertices {
        let n = v.normal;
        assert!(((n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt() - 1.0).abs() < 1e-3);
    }
    // The same revision gives the same mesh; a change makes a new one.
    assert!(std::sync::Arc::ptr_eq(&mesh, &world.mesh()));
    world.advance(TICK);
    assert!(!std::sync::Arc::ptr_eq(&mesh, &world.mesh()));
}

fn styled(style: RopeStyle, segments: usize) -> RopeParams {
    RopeParams {
        style,
        ..params(segments, 9.0)
    }
}

#[test]
fn a_line_is_straight_between_its_ends_and_not_simulated() {
    let mut world = RopeWorld::new();
    let id = world
        .create(
            styled(RopeStyle::Line, 20),
            [-4.0, 0.0, 3.0],
            [4.0, 2.0, 3.0],
        )
        .unwrap();
    // One piece whatever the datablock says, and it counts as one segment.
    assert_eq!(world.rope(id).unwrap().segments(), 1);
    assert_eq!(world.total_segments(), 1);
    let before = world.substeps();
    for _ in 0..200 {
        world.advance(TICK);
    }
    assert!(world.substeps() > before);
    let rope = world.rope(id).unwrap();
    let points: Vec<[f32; 3]> = rope.points().collect();
    assert_eq!(points, vec![[-4.0, 0.0, 3.0], [4.0, 2.0, 3.0]], "no sag");
    assert_eq!(rope.speed(), 0.0);
    // A push does nothing to it; an end moved to a point glides there and the line stays straight.
    world.impulse(id, [0.0, 0.0, 50.0]).unwrap();
    world.pin_point(id, RopeEnd::End, [4.0, 6.0, 5.0]).unwrap();
    for _ in 0..300 {
        world.advance(TICK);
    }
    let points: Vec<[f32; 3]> = world.rope(id).unwrap().points().collect();
    assert_eq!(points[0], [-4.0, 0.0, 3.0]);
    for (got, want) in points[1].iter().zip([4.0, 6.0, 5.0]) {
        assert!((got - want).abs() < 1.0e-3, "{points:?}");
    }
    // A released end stays where it is: a line does not fall.
    world.release(id, RopeEnd::Start).unwrap();
    for _ in 0..100 {
        world.advance(TICK);
    }
    let points: Vec<[f32; 3]> = world.rope(id).unwrap().points().collect();
    assert_eq!(points[0], [-4.0, 0.0, 3.0]);
}

#[test]
fn lines_still_count_against_the_rope_cap() {
    let mut world = RopeWorld::new();
    for _ in 0..MAX_ROPES {
        world
            .create(styled(RopeStyle::Line, 2), [0.0; 3], [1.0, 0.0, 0.0])
            .unwrap();
    }
    assert_eq!(world.total_segments(), MAX_ROPES);
    assert_eq!(
        world.create(styled(RopeStyle::Line, 2), [0.0; 3], [1.0, 0.0, 0.0]),
        Err(RopeError::TooManyRopes)
    );
}

#[test]
fn a_ribbon_is_simulated_like_a_tube_and_each_style_has_its_own_geometry() {
    let mut world = RopeWorld::new();
    let tube = world
        .create(
            styled(RopeStyle::Tube, 10),
            [-4.0, 0.0, 3.0],
            [4.0, 0.0, 3.0],
        )
        .unwrap();
    let ribbon = world
        .create(
            styled(RopeStyle::Ribbon, 10),
            [-4.0, 4.0, 3.0],
            [4.0, 4.0, 3.0],
        )
        .unwrap();
    let line = world
        .create(
            styled(RopeStyle::Line, 10),
            [-4.0, 8.0, 3.0],
            [4.0, 8.0, 3.0],
        )
        .unwrap();
    for _ in 0..400 {
        world.advance(TICK);
    }
    let lowest = |id| {
        world
            .rope(id)
            .unwrap()
            .points()
            .map(|p| p[2])
            .fold(f32::MAX, f32::min)
    };
    assert!((lowest(tube) - lowest(ribbon)).abs() < 1.0e-3, "same sag");
    assert!(lowest(ribbon) < 1.5);
    assert_eq!(lowest(line), 3.0);
    // The tube mesh holds the tube only (a tube of eight sides, closed, is 8 * (11 + 4) vertices); the strips hold the other two.
    let mesh = world.mesh();
    assert_eq!(mesh.vertices.len(), 9 * (11 + 4));
    assert_eq!(mesh.shadow_indices, mesh.indices.len(), "the tube casts");
    let strips = world.strips();
    assert_eq!(strips.strips.len(), 2);
    assert_eq!(strips.strips[1].samples.len(), 2, "a line is its two ends");
    assert_eq!(strips.revision, world.revision());
    // A ribbon's curve goes through the rope's points.
    let points: Vec<[f32; 3]> = world
        .rope(ribbon)
        .unwrap()
        .drawn_points()
        .iter()
        .map(|p| p.to_array())
        .collect();
    for (k, p) in points.iter().enumerate() {
        let s = strips.strips[0].samples[k * torque_plugin_rope::strip::SUBDIVISIONS];
        assert!(s.distance((*p).into()) < 1.0e-5);
    }
}

/// Where the anchor is drawn at `t` seconds: a smooth swing, as a person walking about a rope's length from a post.
fn anchor_at(t: f32) -> [f32; 3] {
    [
        4.0 + 0.8 * (t * 1.1).sin(),
        0.6 * (t * 1.5).cos(),
        3.0 + 0.4 * (t * 1.9).sin(),
    ]
}

/// A rope between a point and a ghost, run frame by frame at `fps` for `seconds` on an injected clock, the ghost drawn at
/// [`anchor_at`] each frame: the world, the rope, and what each frame showed (its end and its middle point).
/// What a frame showed: the rope's end and its middle point.
type Shown = ([f32; 3], [f32; 3]);

fn run_frames(fps: u32, seconds: f32) -> (RopeWorld, RopeId, Vec<Shown>) {
    let mut world = RopeWorld::new();
    let id = world
        .create(params(16, 9.0), [-4.0, 0.0, 3.0], anchor_at(0.0))
        .unwrap();
    world.pin_ghost(id, RopeEnd::End, 7, "").unwrap();
    let mut shown = Vec::new();
    let frames = (seconds * fps as f32).round() as u32;
    for frame in 0..=frames {
        let t = frame as f32 / fps as f32;
        world.step_frame(t, |ghost, _| {
            (ghost == 7).then(|| Mount {
                at: anchor_at(t),
                dir: None,
            })
        });
        let points = world.rope(id).unwrap().drawn_points();
        shown.push((points[16].to_array(), points[8].to_array()));
    }
    (world, id, shown)
}

#[test]
fn an_end_that_follows_a_drawn_ghost_is_where_it_is_drawn_every_frame() {
    // 240 frames a second: the game's tick (32 ms) is eight frames, and the end must not wait for one.
    let (world, id, shown) = run_frames(240, 2.0);
    assert!(matches!(
        world.rope(id).unwrap().anchor(RopeEnd::End),
        Anchor::Ghost { ghost: 7, .. }
    ));
    for (frame, (end, _)) in shown.iter().enumerate().skip(1) {
        let want = anchor_at(frame as f32 / 240.0);
        for k in 0..3 {
            assert!(
                (end[k] - want[k]).abs() < 1.0e-5,
                "frame {frame}: {end:?} {want:?}"
            );
        }
    }
    // The middle moves every frame (no staircase at the tick's rate), by about the same amount as its neighbors.
    let moves: Vec<f32> = shown
        .windows(2)
        .skip(60)
        .map(|w| {
            let (a, b) = (w[0].1, w[1].1);
            ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
        })
        .collect();
    assert!(moves.iter().all(|&d| d > 1.0e-6), "it moved every frame");
    let most = moves.iter().fold(0.0f32, |m, &d| m.max(d));
    assert!(most < 0.05, "no jumps: {most}");
    // Each frame's move is about the last one's (the swing is smooth): no stair of one big step in eight.
    for pair in moves.windows(2).filter(|p| p[0] > 1.0e-3) {
        let ratio = pair[1] / pair[0];
        assert!((0.5..=2.0).contains(&ratio), "{} then {}", pair[0], pair[1]);
    }
}

#[test]
fn the_same_motion_at_60_and_240_frames_a_second_ends_alike() {
    let (a, ida, _) = run_frames(60, 2.0);
    let (b, idb, _) = run_frames(240, 2.0);
    let (pa, pb) = (
        a.rope(ida).unwrap().drawn_points(),
        b.rope(idb).unwrap().drawn_points(),
    );
    assert_eq!(pa.len(), pb.len());
    for (i, (x, y)) in pa.iter().zip(pb).enumerate() {
        assert!(x.distance(*y) < 0.05, "point {i}: {x:?} {y:?}");
    }
    assert!(a.rope(ida).unwrap().strain() < 0.2 && b.rope(idb).unwrap().strain() < 0.2);
}

#[test]
fn a_hitch_is_capped_and_the_remainder_dropped() {
    let mut world = RopeWorld::new();
    world
        .create(params(8, 5.0), [0.0; 3], [3.0, 0.0, 0.0])
        .unwrap();
    world.step_frame(0.0, |_, _| None);
    // A whole second between two frames runs the cap's substeps, no more.
    world.step_frame(1.0, |_, _| None);
    assert_eq!(
        world.substeps(),
        torque_plugin_rope::sim::MAX_SUBSTEPS as u64
    );
    // The frame after it is a normal one.
    world.step_frame(1.0 + 1.0 / 60.0, |_, _| None);
    let ran = world.substeps() - torque_plugin_rope::sim::MAX_SUBSTEPS as u64;
    assert!((3..=5).contains(&ran), "{ran}");
    // A second view of the same frame, and a clock that went back, run nothing.
    let before = world.substeps();
    world.step_frame(1.0 + 1.0 / 60.0, |_, _| None);
    world.step_frame(0.5, |_, _| None);
    assert_eq!(world.substeps(), before);
}

#[test]
fn the_tick_runs_the_ropes_only_when_no_frame_is_drawn() {
    let mut world = RopeWorld::new();
    world
        .create(params(8, 5.0), [0.0; 3], [3.0, 0.0, 0.0])
        .unwrap();
    // A drawing client: frames come between ticks, and the ticks run nothing.
    for frame in 0..40 {
        world.step_frame(frame as f32 / 60.0, |_, _| None);
        world.tick(TICK);
    }
    let drawn = world.substeps();
    assert!(drawn > 0 && drawn <= 40 * 5, "{drawn}");
    // A client that does not draw: after a few ticks without a frame the ticks run them.
    for _ in 0..30 {
        world.tick(TICK);
    }
    assert!(world.substeps() > drawn + 100, "{}", world.substeps());
}

#[test]
fn a_tube_runs_the_texture_by_arc_length_through_its_joints_and_around() {
    let mut world = RopeWorld::new();
    let id = world
        .create(
            RopeParams {
                texture_length: 0.5,
                texture_scroll: -1.5,
                texture: 4,
                ..params(6, 5.0)
            },
            [0.0; 3],
            [3.0, 1.0, 0.0],
        )
        .unwrap();
    for _ in 0..50 {
        world.advance(TICK);
    }
    let mesh = world.mesh();
    let points: Vec<[f32; 3]> = world
        .rope(id)
        .unwrap()
        .drawn_points()
        .iter()
        .map(|p| p.to_array())
        .collect();
    let stride = torque_plugin_rope::SIDES + 1;
    let ring = |r: usize| &mesh.vertices[r * stride..(r + 1) * stride];
    // Rings 2 to 8 are the seven points; each takes u from the one before by the piece's length over the repeat.
    assert!(ring(2).iter().all(|v| v.uv[0].abs() < 1.0e-6));
    for i in 1..points.len() {
        let d = ((points[i][0] - points[i - 1][0]).powi(2)
            + (points[i][1] - points[i - 1][1]).powi(2)
            + (points[i][2] - points[i - 1][2]).powi(2))
        .sqrt();
        let du = ring(2 + i)[0].uv[0] - ring(1 + i)[0].uv[0];
        assert!((du - d / 0.5).abs() < 1.0e-4, "piece {i}: {du} {}", d / 0.5);
    }
    // Around each ring v runs from 0 to 1, the first corner is there twice (the seam), and every corner has the rope's scroll.
    for r in 0..mesh.vertices.len() / stride {
        let corners = ring(r);
        assert_eq!(corners[0].uv[1], 0.0);
        assert_eq!(corners[stride - 1].uv[1], 1.0);
        assert_eq!(corners[0].position, corners[stride - 1].position);
        assert!(
            corners
                .windows(2)
                .all(|w| w[0].uv[1] < w[1].uv[1] && w[0].uv[0] == w[1].uv[0])
        );
        assert!(corners.iter().all(|v| v.scroll == -1.5));
    }
    assert!(mesh.draws.iter().all(|d| d.texture == 4));
}

#[test]
fn a_curve_passes_through_both_ends_leaves_along_the_tangents_and_is_straight_without_a_bend() {
    use glam::Vec3;
    use torque_plugin_rope::Curve;
    let (a, b) = (Vec3::new(1.0, 2.0, 3.0), Vec3::new(9.0, -4.0, 5.0));
    let (da, db) = (Vec3::new(0.0, 1.0, 0.0), Vec3::new(0.0, 0.0, 1.0));
    assert!(Curve::at(a, b, da, db, 0.6, 0.0).distance(a) < 1.0e-5);
    assert!(Curve::at(a, b, da, db, 0.6, 1.0).distance(b) < 1.0e-5);
    // It leaves a along da and b along db (the first and last steps of the curve point that way).
    let first = (Curve::at(a, b, da, db, 0.6, 0.001) - a).normalize();
    let last = (Curve::at(a, b, da, db, 0.6, 0.999) - b).normalize();
    assert!(first.dot(da) > 0.999, "{first:?}");
    assert!(last.dot(db) > 0.999, "{last:?}");
    // With no bend the handles have no length: a straight line, whatever the tangents say, and the samples are evenly spaced.
    let straight = Curve::sample(a, b, da, db, 0.0, 9);
    for (k, p) in straight.iter().enumerate() {
        let want = a.lerp(b, k as f32 / 8.0);
        assert!(p.distance(want) < 1.0e-3, "{k}: {p:?} {want:?}");
    }
    // With a bend they are still evenly spaced along the curve.
    let bent = Curve::sample(a, b, da, db, 0.6, 12);
    assert!(bent[0].distance(a) < 1.0e-6 && bent[11].distance(b) < 1.0e-6);
    let steps: Vec<f32> = bent.windows(2).map(|w| w[0].distance(w[1])).collect();
    let (least, most) = steps
        .iter()
        .fold((f32::MAX, 0.0f32), |(l, m), &d| (l.min(d), m.max(d)));
    assert!(most < least * 1.1, "{least} {most}");
    // A bend is a bulge toward the tangents: the middle is off the chord.
    let off = Curve::at(a, b, da, db, 0.6, 0.5).distance(a.lerp(b, 0.5));
    assert!(off > 1.0, "{off}");
}

/// The muzzle at `t` seconds (it walks and the aim turns): its place and the unit direction it points.
fn muzzle_at(t: f32) -> ([f32; 3], [f32; 3]) {
    let yaw = 0.2 + 1.6 * t;
    (
        [1.0 + 0.8 * t, 0.5 * (t * 2.0).sin(), 1.4],
        [yaw.sin(), yaw.cos(), 0.0],
    )
}

#[test]
fn a_beam_starts_at_the_drawn_muzzle_every_frame_while_the_aim_turns() {
    use torque_plugin_rope::{Curve, Tangent};
    let mut world = RopeWorld::new();
    let id = world
        .create(
            RopeParams {
                driven: true,
                segments: 16,
                ..params(16, 6.0)
            },
            [0.0; 3],
            [6.0, 3.0, 1.0],
        )
        .unwrap();
    world.pin_ghost(id, RopeEnd::Start, 5, "muzzle").unwrap();
    world
        .set_curve(
            id,
            Some(Curve {
                start: Tangent::Mount,
                end: Tangent::Auto,
                bend: 0.5,
            }),
        )
        .unwrap();
    let mut middles = Vec::new();
    for frame in 0..=120 {
        let t = frame as f32 / 120.0;
        world.step_frame(t, |ghost, _| {
            let (at, dir) = muzzle_at(t);
            (ghost == 5).then_some(Mount { at, dir: Some(dir) })
        });
        let points = world.rope(id).unwrap().drawn_points();
        let (at, dir) = muzzle_at(t);
        // The start is the muzzle, to the bit, and the beam leaves along the aim.
        assert!(points[0].distance(at.into()) < 1.0e-5, "frame {frame}");
        let leaving = (points[1] - points[0]).normalize();
        assert!(
            leaving.dot(dir.into()) > 0.95,
            "frame {frame}: {leaving:?} against {dir:?}"
        );
        assert!((points[16] - glam::Vec3::new(6.0, 3.0, 1.0)).length() < 1.0e-5);
        middles.push(points[8]);
    }
    // It bends differently as the aim turns, every frame.
    assert!(middles.windows(2).all(|w| w[0].distance(w[1]) > 1.0e-5));
    // Not simulated: no substep was run for it.
    assert_eq!(world.rope(id).unwrap().speed(), 0.0);
    // Clearing the curve leaves it a straight driven rope (the datablock's sim says none).
    world.set_curve(id, None).unwrap();
    world.step_frame(2.0, |_, _| None);
    let straight = world.rope(id).unwrap().drawn_points();
    assert!(straight[8].distance(straight[0].lerp(straight[16], 0.5)) < 1.0e-4);
}

#[test]
fn a_falling_free_end_drawn_moves_down_by_steady_steps_at_any_frame_rate() {
    for fps in [60u32, 240, 960] {
        let mut world = RopeWorld::new();
        let id = world
            .create(params(12, 8.0), [0.0, 0.0, 10.0], [3.0, 0.0, 10.0])
            .unwrap();
        world.release(id, RopeEnd::End).unwrap();
        let mut heights = Vec::new();
        for frame in 0..=(fps / 3) {
            world.step_frame(frame as f32 / fps as f32, |_, _| None);
            heights.push(world.rope(id).unwrap().drawn_points()[12].z);
        }
        // Never back up, and each frame's fall about the same as the one before (gravity makes them grow a little).
        let falls: Vec<f32> = heights.windows(2).map(|w| w[0] - w[1]).collect();
        let settled = &falls[(fps as usize / 20).max(3)..];
        assert!(
            settled.iter().all(|&d| d > 0.0),
            "{fps} fps went back up: {settled:?}"
        );
        for pair in settled.windows(2) {
            let ratio = pair[1] / pair[0];
            assert!(
                (0.85..=1.15).contains(&ratio),
                "{fps} fps: {} then {}",
                pair[0],
                pair[1]
            );
        }
    }
}

#[test]
fn a_world_warns_about_only_so_many_files() {
    let mut world = RopeWorld::new();
    let first = (0..1000)
        .filter(|i| world.first_warning(&format!("art/{i}.png")))
        .count();
    assert_eq!(first, torque_plugin_rope::sim::MAX_WARNED);
    assert!(
        !world.first_warning("art/new.png"),
        "past the cap nothing is recorded"
    );
}

/// How far the points of a rope are from the straight line between its ends.
fn off_the_line(world: &RopeWorld, id: RopeId, drawn: bool) -> f32 {
    let rope = world.rope(id).unwrap();
    let p: Vec<[f32; 3]> = if drawn {
        rope.drawn_points().iter().map(|v| v.to_array()).collect()
    } else {
        rope.points().collect()
    };
    let (a, b) = (p[0], p[p.len() - 1]);
    let d: Vec<f32> = (0..3).map(|k| b[k] - a[k]).collect();
    let len = d.iter().map(|x| x * x).sum::<f32>().sqrt();
    p.iter()
        .map(|q| {
            let r: Vec<f32> = (0..3).map(|k| q[k] - a[k]).collect();
            let along = r.iter().zip(&d).map(|(x, y)| x * y).sum::<f32>() / len;
            (0..3)
                .map(|k| (r[k] - d[k] / len * along).powi(2))
                .sum::<f32>()
                .sqrt()
        })
        .fold(0.0, f32::max)
}

#[test]
fn a_rope_with_its_ends_farther_apart_than_it_is_long_is_drawn_straight_and_even() {
    let mut world = RopeWorld::new();
    let id = world
        .create(params(10, 4.0), [0.0, 0.0, 3.0], [10.0, 0.0, 3.0])
        .unwrap();
    for _ in 0..40 {
        world.advance(TICK);
        assert!(world.rope(id).unwrap().taut());
        // Straight, with the pieces all the same length, whatever the simulation was doing.
        assert!(off_the_line(&world, id, true) < 1e-3);
        assert!(off_the_line(&world, id, false) < 1e-3);
    }
    let points: Vec<[f32; 3]> = world
        .rope(id)
        .unwrap()
        .drawn_points()
        .iter()
        .map(|v| v.to_array())
        .collect();
    let pieces: Vec<f32> = points.windows(2).map(|w| w[1][0] - w[0][0]).collect();
    assert!(pieces.iter().all(|p| (p - 1.0).abs() < 1e-3), "{pieces:?}");
    // It pulls nothing and the ends have not moved.
    assert_eq!(points[0], [0.0, 0.0, 3.0]);
    assert_eq!(points[10], [10.0, 0.0, 3.0]);
    // Slack again: the rope hangs, and is no longer taut.
    world.pin_point(id, RopeEnd::End, [3.0, 0.0, 3.0]).unwrap();
    for _ in 0..200 {
        world.advance(TICK);
    }
    let rope = world.rope(id).unwrap();
    assert!(!rope.taut());
    assert!(off_the_line(&world, id, true) > 0.3);
}

#[test]
fn a_rope_reels_to_its_new_length_over_the_time_it_was_given() {
    let mut world = RopeWorld::new();
    let id = world
        .create(params(10, 6.0), [0.0, 0.0, 3.0], [4.0, 0.0, 3.0])
        .unwrap();
    assert_eq!(world.rope(id).unwrap().length(), 6.0);
    world.set_length(id, 10.0, 2.0).unwrap();
    assert!(world.rope(id).unwrap().reeling());
    let mut seen = Vec::new();
    for tick in 1..=80 {
        world.advance(TICK);
        if tick % 20 == 0 {
            seen.push(world.rope(id).unwrap().length());
        }
    }
    // Evenly: 0.64 s, 1.28 s, 1.92 s and 2.56 s in.
    assert!((seen[0] - (6.0 + 4.0 * 0.32)).abs() < 0.1, "{seen:?}");
    assert!((seen[1] - (6.0 + 4.0 * 0.64)).abs() < 0.1, "{seen:?}");
    assert!((seen[2] - (6.0 + 4.0 * 0.96)).abs() < 0.1, "{seen:?}");
    assert_eq!(seen[3], 10.0);
    assert!(!world.rope(id).unwrap().reeling());
    // Reeling in, and at once; a bad length changes nothing, a short one is held to the least.
    world.set_length(id, 5.0, 0.0).unwrap();
    assert_eq!(world.rope(id).unwrap().length(), 5.0);
    world.set_length(id, f32::NAN, 1.0).unwrap();
    assert_eq!(world.rope(id).unwrap().length(), 5.0);
    world.set_length(id, 0.0, f32::NAN).unwrap();
    assert_eq!(
        world.rope(id).unwrap().length(),
        torque_plugin_rope::sim::MIN_LENGTH
    );
    assert!(world.set_length(99, 1.0, 1.0).is_err());
}

#[test]
fn a_reel_that_pulls_the_ends_taut_straightens_the_rope() {
    let mut world = RopeWorld::new();
    let id = world
        .create(params(10, 8.0), [0.0, 0.0, 3.0], [6.0, 0.0, 3.0])
        .unwrap();
    for _ in 0..100 {
        world.advance(TICK);
    }
    assert!(!world.rope(id).unwrap().taut());
    world.set_length(id, 3.0, 1.0).unwrap();
    let mut was_taut = false;
    for _ in 0..60 {
        world.advance(TICK);
        was_taut |= world.rope(id).unwrap().taut();
    }
    assert!(was_taut);
    assert!(off_the_line(&world, id, true) < 1e-3);
}

#[test]
fn a_temporary_rope_fades_over_its_last_seconds_and_is_taken_away() {
    let mut world = RopeWorld::new();
    let id = world
        .create(params(8, 5.0), [0.0, 0.0, 3.0], [4.0, 0.0, 3.0])
        .unwrap();
    let lasting = world
        .create(params(8, 5.0), [0.0, 1.0, 3.0], [4.0, 1.0, 3.0])
        .unwrap();
    world.set_lifetime(id, 2.0, 1.0).unwrap();
    assert_eq!(world.rope(id).unwrap().fade(), 1.0);
    let mut fades = Vec::new();
    // 1 s in: still whole. 1.5 s: half. Past 2 s: gone, and the other rope is not.
    for tick in 1..=70 {
        world.advance(TICK);
        if tick == 31 || tick == 47 {
            fades.push(world.rope(id).unwrap().fade());
        }
    }
    assert_eq!(fades[0], 1.0);
    assert!((fades[1] - 0.5).abs() < 0.1, "{fades:?}");
    assert!(world.rope(id).is_none());
    assert!(world.rope(lasting).is_some());
    // A lifetime of 0 makes a rope last again; a fade longer than the life is the life.
    let again = world
        .create(params(8, 5.0), [0.0, 2.0, 3.0], [4.0, 2.0, 3.0])
        .unwrap();
    world.set_lifetime(again, 1.0, 9.0).unwrap();
    world.advance(TICK);
    assert!(world.rope(again).unwrap().fade() < 1.0);
    world.set_lifetime(again, 0.0, 0.0).unwrap();
    assert_eq!(world.rope(again).unwrap().life_left(), None);
    assert_eq!(world.rope(again).unwrap().fade(), 1.0);
}

#[test]
fn a_fading_tube_thins_and_a_fading_ribbon_grows_faint() {
    let mut world = RopeWorld::new();
    let tube = world
        .create(
            RopeParams {
                style: RopeStyle::Tube,
                color: [200, 100, 50, 255],
                ..params(8, 5.0)
            },
            [0.0, 0.0, 3.0],
            [4.0, 0.0, 3.0],
        )
        .unwrap();
    let ribbon = world
        .create(
            RopeParams {
                style: RopeStyle::Ribbon,
                color: [200, 100, 50, 200],
                ..params(8, 5.0)
            },
            [0.0, 1.0, 3.0],
            [4.0, 1.0, 3.0],
        )
        .unwrap();
    let width = |world: &mut RopeWorld| {
        let mesh = world.mesh();
        let (mut low, mut high) = (f32::MAX, f32::MIN);
        for v in &mesh.vertices {
            low = low.min(v.position[1]);
            high = high.max(v.position[1]);
        }
        high - low
    };
    let alpha = |world: &mut RopeWorld| {
        world
            .strips()
            .strips
            .iter()
            .map(|s| s.color[3])
            .max()
            .unwrap_or(0)
    };
    world.advance(TICK);
    let (w0, a0) = (width(&mut world), alpha(&mut world));
    world.set_lifetime(tube, 1.0, 1.0).unwrap();
    world.set_lifetime(ribbon, 1.0, 1.0).unwrap();
    for _ in 0..16 {
        world.advance(TICK);
    }
    let (w1, a1) = (width(&mut world), alpha(&mut world));
    assert!(w1 < w0 * 0.8, "the tube thinned: {w0} then {w1}");
    assert!(
        a1 < a0 / 2 + 20 && a1 > 0,
        "the ribbon faded: {a0} then {a1}"
    );
}
