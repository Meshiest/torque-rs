use super::*;
use crate::data::{PropNumbers, PropParams, PropShape};
use crate::world::{PropLimits, PropWorld, SpawnArgs, StaticPiece};
use torque_collide::math::IDENTITY;

const NO_TURN: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

/// The eight corners of the box `lo..hi`.
fn corners(lo: [f32; 3], hi: [f32; 3]) -> Vec<[f32; 3]> {
    (0..8)
        .map(|i| std::array::from_fn(|k| if i & (1 << k) == 0 { lo[k] } else { hi[k] }))
        .collect()
}

/// A mesh part of the box `lo..hi` as an exporter writes a box: each corner once a face, so twenty-four points.
fn box_part(lo: [f32; 3], hi: [f32; 3]) -> MeshPart {
    MeshPart {
        node: IDENTITY,
        verts: corners(lo, hi).into_iter().cycle().take(24).collect(),
        tris: Vec::new(),
        materials: Vec::new(),
    }
}

fn part_of(points: Vec<[f32; 3]>) -> MeshPart {
    MeshPart {
        node: IDENTITY,
        verts: points,
        tris: Vec::new(),
        materials: Vec::new(),
    }
}

const BOUNDS: [f32; 6] = [-1.0, -1.0, -0.25, 1.0, 1.0, 0.25];

fn hulls_of(slots: &[&[MeshPart]]) -> Result<PropHulls, Refusal> {
    PropHulls::of_parts(slots, BOUNDS, [1.0; 3], &HullCaps::default(), &mut |_| true)
}

/// An L two units a side and half a unit thick: two bars, the second beside the first's end.
fn l_shape() -> PropHulls {
    let arms = [
        box_part([-1.0, -1.0, -0.25], [1.0, -0.5, 0.25]),
        box_part([-1.0, -0.5, -0.25], [-0.5, 1.0, 0.25]),
    ];
    hulls_of(&[&arms]).expect("the L's hulls")
}

#[test]
fn a_box_written_with_a_corner_a_face_is_its_eight_points_through_the_nodes_turn_and_the_draw_scale()
 {
    let mut part = box_part([-1.0, -1.0, -1.0], [1.0, 1.0, 1.0]);
    assert_eq!(part.verts.len(), 24);
    part.node[3] = 5.0;
    let hulls = PropHulls::of_parts(
        &[&[part]],
        BOUNDS,
        [2.0, 1.0, 0.5],
        &HullCaps::default(),
        &mut |_| true,
    )
    .expect("hulls");
    assert_eq!(hulls.hulls().len(), 1);
    let points = &hulls.hulls()[0];
    assert_eq!(points.len(), 8);
    // The node moves a point before the scale stretches it: x from 5 - 1 and 5 + 1, times 2.
    let xs: Vec<f32> = points.iter().map(|p| p[0]).collect();
    assert!(xs.iter().all(|&x| x == 8.0 || x == 12.0), "{xs:?}");
    assert!(points.iter().all(|p| p[2].abs() == 0.5));
    assert!(!hulls.is_bounds_only());
}

#[test]
fn negative_zero_and_zero_are_one_point() {
    let part = part_of(vec![
        [0.0, 0.0, 0.0],
        [-0.0, 0.0, -0.0],
        [1.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        [0.0, 0.0, 1.0],
    ]);
    assert_eq!(hulls_of(&[&[part]]).expect("hulls").hulls()[0].len(), 4);
}

#[test]
fn the_caps_refuse_too_many_hulls_too_many_points_and_points_that_are_no_numbers() {
    let caps = HullCaps::default();
    let one = |i: usize| box_part([i as f32; 3], [i as f32 + 0.5; 3]);
    // At the caps: 32 hulls, and a hull of 64 points, are taken.
    let at_cap: Vec<MeshPart> = (0..MAX_HULLS).map(one).collect();
    assert_eq!(hulls_of(&[&at_cap]).expect("32 hulls").hulls().len(), 32);
    let ring = |n: usize| -> Vec<[f32; 3]> {
        (0..n)
            .map(|i| {
                let a = i as f32 * 0.1;
                [a.cos(), a.sin(), (i % 7) as f32 * 0.1]
            })
            .collect()
    };
    assert_eq!(
        hulls_of(&[&[part_of(ring(MAX_HULL_VERTICES))]])
            .expect("64 points")
            .vertex_count(),
        MAX_HULL_VERTICES
    );
    // One past each.
    let too_many: Vec<MeshPart> = (0..=MAX_HULLS).map(one).collect();
    assert_eq!(
        hulls_of(&[&too_many]),
        Err(Refusal::TooManyHulls { found: 33 })
    );
    assert_eq!(
        hulls_of(&[&[part_of(ring(MAX_HULL_VERTICES + 1))]]),
        Err(Refusal::TooManyVertices { found: 65 })
    );
    // The hulls of two collision details count together.
    let half: Vec<MeshPart> = (0..20).map(one).collect();
    assert!(matches!(
        hulls_of(&[&half, &half]),
        Err(Refusal::TooManyHulls { .. })
    ));
    // Points that are not numbers, or lie far out (a hostile file, or a draw scale that stretches the shape).
    for bad in [f32::NAN, f32::INFINITY, -f32::INFINITY, MAX_COORD + 1.0] {
        let part = part_of(vec![
            [0.0; 3],
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, bad],
        ]);
        assert_eq!(hulls_of(&[&[part]]), Err(Refusal::BadPoint), "{bad}");
    }
    let stretched = box_part([-1.0; 3], [1.0; 3]);
    assert_eq!(
        PropHulls::of_parts(&[&[stretched]], BOUNDS, [1000.0; 3], &caps, &mut |_| true),
        Err(Refusal::BadPoint)
    );
    // Lower caps hold as well.
    let tight = HullCaps {
        hulls: 1,
        vertices: 4,
        total: 8,
    };
    let two = [one(0), one(1)];
    assert!(PropHulls::of_parts(&[&two], BOUNDS, [1.0; 3], &tight, &mut |_| true).is_err());
    assert!(PropHulls::of_parts(&[&two[..1]], BOUNDS, [1.0; 3], &tight, &mut |_| true).is_err());
}

#[test]
fn a_shape_with_no_collision_mesh_collides_as_its_bounds() {
    let none: [&[MeshPart]; 0] = [];
    let hulls = PropHulls::of_parts(
        &none,
        BOUNDS,
        [1.0, 2.0, 4.0],
        &HullCaps::default(),
        &mut |_| true,
    )
    .expect("the bounds");
    assert!(hulls.is_bounds_only());
    assert_eq!(hulls.hulls().len(), 1);
    assert_eq!(hulls.vertex_count(), 8);
    assert!(
        (hulls.volume() - 2.0 * 4.0 * 2.0).abs() < 1e-3,
        "{}",
        hulls.volume()
    );
    // Detail slots with no mesh in them are no collision mesh either.
    let empty = [MeshPart::default()];
    let again = hulls_of(&[&empty]).expect("the bounds");
    assert!(again.is_bounds_only());
    // Bounds that are not a box give nothing.
    for bounds in [
        [1.0, 0.0, 0.0, -1.0, 1.0, 1.0],
        [f32::NAN, 0.0, 0.0, 1.0, 1.0, 1.0],
    ] {
        assert_eq!(
            PropHulls::of_parts(&none, bounds, [1.0; 3], &HullCaps::default(), &mut |_| true),
            Err(Refusal::NoBounds)
        );
    }
}

#[test]
fn the_hulls_of_one_shape_are_the_same_whenever_they_are_made() {
    let a = l_shape();
    let b = l_shape();
    assert_eq!(a, b);
    assert!(a.hulls().iter().all(|h| h.len() == 8));
    // The L holds the two bars' volumes (1.0 and 0.25).
    assert!((a.volume() - 1.0 * 0.5 * 2.0 * 0.5 - 0.5 * 1.5 * 0.5).abs() < 1e-3);
}

/// A world with a floor whose top is at z = 0.
fn floor() -> PropWorld {
    let mut w = PropWorld::new(4, PropLimits::default());
    w.sync_statics(vec![(
        1,
        StaticPiece::Box {
            position: [0.0, 0.0, -1.0],
            rotation: NO_TURN,
            half: [500.0, 500.0, 1.0],
        },
    )]);
    w
}

/// A prop of datablock 1 at `at`, turned not at all, that collides as `hulls`.
fn spawn_hulled(
    w: &mut PropWorld,
    id: u32,
    params: PropParams,
    hulls: Option<Rc<PropHulls>>,
    at: [f32; 3],
) -> Result<(), crate::world::SpawnError> {
    w.spawn_with(SpawnArgs {
        id,
        data: 1,
        params,
        hulls,
        pose: (at, NO_TURN),
        owner: None,
        predict: 0,
    })
}

fn far(_: [f32; 3]) -> f32 {
    0.0
}

/// How high a small crate stands after it is dropped into the L's notch (above the L's inner corner), with the L fixed on
/// the floor as `hulls` makes it (none: as a box).
fn crate_height_in_the_notch(hulls: Option<Rc<PropHulls>>) -> f32 {
    let mut w = floor();
    let numbers = PropNumbers {
        size: [2.0, 2.0, 0.5],
        mass: 50.0,
        ..PropNumbers::default()
    };
    let params = match &hulls {
        Some(h) => numbers.held().with_hulls(h.volume()),
        None => numbers.held(),
    };
    spawn_hulled(&mut w, 10, params, hulls, [0.0, 0.0, 0.25]).expect("the L");
    assert!(w.freeze(10).is_some());
    let small = PropNumbers {
        size: [0.4; 3],
        mass: 5.0,
        ..PropNumbers::default()
    }
    .held();
    w.spawn(11, 2, small, ([0.3, 0.3, 1.2], NO_TURN), None, 0)
        .expect("the crate");
    for _ in 0..300 {
        w.tick(&[], &far);
    }
    w.get(11).expect("the crate").q.position()[2]
}

#[test]
fn a_prop_with_an_l_shaped_hull_lets_another_sit_in_its_notch_where_a_box_would_not() {
    let boxed = crate_height_in_the_notch(None);
    let notched = crate_height_in_the_notch(Some(Rc::new(l_shape())));
    // On the box's top (0.5) the crate's middle is 0.2 above it; in the notch it is 0.2 above the floor.
    assert!((boxed - 0.7).abs() < 0.05, "on the box: {boxed}");
    assert!((notched - 0.2).abs() < 0.05, "in the notch: {notched}");
}

#[test]
fn a_crate_dropped_on_the_l_arm_rests_on_it_and_a_hulled_prop_keeps_its_mass() {
    let hulls = Rc::new(l_shape());
    let numbers = PropNumbers {
        size: [2.0, 2.0, 0.5],
        mass: 50.0,
        ..PropNumbers::default()
    };
    let plain = numbers.held();
    let fitted = plain.with_hulls(hulls.volume());
    assert!((fitted.mass() - 50.0).abs() < 0.01, "{}", fitted.mass());
    // The datablock's box and the hulls' volume are different shapes; the box still bounds them.
    assert!(matches!(fitted.shape, PropShape::Hulls { .. }));
    assert_eq!(fitted.shape.half_extents(), plain.shape.half_extents());
    assert!(fitted.density > plain.density);
    // A density, not a mass, stays a density.
    let by_density = PropNumbers {
        mass: 0.0,
        density: 3.0,
        ..numbers
    }
    .held()
    .with_hulls(hulls.volume());
    assert_eq!(by_density.density, 3.0);
    // Not a volume: unchanged.
    for v in [0.0, -1.0, f32::NAN, f32::INFINITY] {
        assert_eq!(plain.with_hulls(v), plain);
    }
    // On the arm.
    let mut w = floor();
    spawn_hulled(&mut w, 10, fitted, Some(hulls), [0.0, 0.0, 0.25]).expect("the L");
    w.freeze(10);
    w.spawn(
        11,
        2,
        PropNumbers {
            size: [0.4; 3],
            ..PropNumbers::default()
        }
        .held(),
        ([0.0, -0.75, 1.5], NO_TURN),
        None,
        0,
    )
    .expect("the crate");
    for _ in 0..300 {
        w.tick(&[], &far);
    }
    let z = w.get(11).expect("the crate").q.position()[2];
    assert!((z - 0.7).abs() < 0.05, "{z}");
}

#[test]
fn a_hulled_prop_asleep_where_it_rests_stays_still() {
    let mut w = floor();
    let hulls = Rc::new(l_shape());
    let params = PropNumbers {
        size: [2.0, 2.0, 0.5],
        mass: 50.0,
        ..PropNumbers::default()
    }
    .held()
    .with_hulls(hulls.volume());
    spawn_hulled(&mut w, 10, params, Some(hulls), [0.0, 0.0, 0.25]).expect("the L");
    for _ in 0..400 {
        w.tick(&[], &far);
    }
    let after = w.get(10).expect("the L");
    assert!(after.rest.is_asleep(), "it settled and slept");
    let at = after.q.position();
    for _ in 0..200 {
        w.tick(&[], &far);
    }
    assert_eq!(w.get(10).expect("the L").q.position(), at);
}

/// A tiny seeded generator, the same sequence every run.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }

    /// A number, now and then a hostile one.
    fn number(&mut self) -> f32 {
        match self.below(150) {
            0 => f32::NAN,
            1 => f32::INFINITY,
            2 => -f32::INFINITY,
            3 => 1.0e30,
            4 => -1.0e-30,
            _ => (self.below(2001) as f32 - 1000.0) / 100.0,
        }
    }
}

#[test]
fn hostile_collision_meshes_are_refused_or_held_and_never_reach_the_world_unchecked() {
    let rounds: u64 = std::env::var("TORQUE_FUZZ_PROPS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(400);
    let mut rng = Rng(0x2545_f491_4f6c_dd1d);
    let (mut taken, mut refused) = (0, 0);
    for _ in 0..rounds {
        let slots: Vec<Vec<MeshPart>> = (0..rng.below(4))
            .map(|_| {
                (0..rng.below(14))
                    .map(|_| {
                        let mut node = IDENTITY;
                        if rng.below(3) == 0 {
                            node[3] = rng.number();
                            node[5] = rng.number();
                        }
                        let count = [0, 1, 4, 8, 8, 8, 8, 60, 70, 500][rng.below(10) as usize];
                        MeshPart {
                            node,
                            verts: (0..count)
                                .map(|_| [rng.number(), rng.number(), rng.number()])
                                .collect(),
                            tris: Vec::new(),
                            materials: Vec::new(),
                        }
                    })
                    .collect()
            })
            .collect();
        let refs: Vec<&[MeshPart]> = slots.iter().map(Vec::as_slice).collect();
        let bounds: [f32; 6] = std::array::from_fn(|_| rng.number());
        let scale: [f32; 3] = std::array::from_fn(|_| rng.number().abs().clamp(0.001, 256.0));
        match PropHulls::of_parts(&refs, bounds, scale, &HullCaps::default(), &mut |_| true) {
            Ok(hulls) => {
                taken += 1;
                assert!(hulls.hulls().len() <= MAX_HULLS);
                assert!(hulls.hulls().iter().all(|h| h.len() <= MAX_HULL_VERTICES));
                assert!(
                    hulls
                        .hulls()
                        .iter()
                        .flatten()
                        .flatten()
                        .all(|c| c.is_finite() && c.abs() <= MAX_COORD)
                );
                assert!(hulls.volume().is_finite());
                // The world takes it or says no; it never panics.
                let params = PropNumbers::default().held().with_hulls(hulls.volume());
                let mut w = floor();
                let _ = spawn_hulled(&mut w, 1, params, Some(Rc::new(hulls)), [0.0, 0.0, 2.0]);
                for _ in 0..5 {
                    w.tick(&[], &far);
                }
            }
            Err(_) => refused += 1,
        }
    }
    assert!(taken > 0 && refused > 0, "{taken} taken, {refused} refused");
}

/// `PropParams` is what the client reads from a datablock record; hulls do not change a datablock that did not ask for them.
#[test]
fn a_datablock_that_asks_for_the_box_has_no_hulls_in_its_numbers() {
    let params: PropParams = PropNumbers::default().held();
    assert!(matches!(params.shape, PropShape::Box(_)));
    assert_eq!(params.collision, crate::data::PropCollision::Box);
}

fn built(hulls: PropHulls) -> Made {
    Ok(Rc::new(hulls))
}

#[test]
fn a_client_predicts_hulls_only_when_the_ones_it_built_are_the_servers() {
    let l = l_shape();
    let digest = l.digest();
    let one = hulls_of(&[&[box_part([-1.0, -1.0, -0.25], [1.0, -0.5, 0.25])]]).expect("one bar");
    assert_eq!(digest.hulls, 2);
    assert_eq!(digest.points, 16);
    assert_eq!(digest.volume, l.volume().to_bits());
    // The server has the datablock's box: the client predicts a box, whatever it built or could not.
    for client in [None, Some(built(l.clone())), Some(Err(Refusal::Unreadable))] {
        assert_eq!(choose(None, client.as_ref()), Choice::Box);
    }
    // The server has the L: the client's own build of the same shape is taken.
    assert_eq!(choose(Some(digest), Some(&built(l.clone()))), Choice::Hulls);
    // Not read yet: it waits.
    assert_eq!(choose(Some(digest), None), Choice::Wait);
    // Another shape under the same path, and every way a client's read ends without the server's hulls (a file that is not
    // there, a read the client's budget or caps refused, a shape past a cap, a hostile point): the prop is left to the server.
    assert_eq!(choose(Some(digest), Some(&built(one))), Choice::Unfit);
    for why in [
        Refusal::Unreadable,
        Refusal::Budget,
        Refusal::NoLoader,
        Refusal::ServerTotal,
        Refusal::BadPoint,
        Refusal::NoBounds,
        Refusal::TooManyHulls { found: 33 },
        Refusal::TooManyVertices { found: 65 },
    ] {
        assert_eq!(
            choose(Some(digest), Some(&Err(why))),
            Choice::Unfit,
            "{why}"
        );
    }
    // Each part of the digest tells shapes apart: the count, the points and the volume.
    let same_count = [
        hulls_of(&[&[
            box_part([-1.0, -1.0, -0.25], [1.0, -0.5, 0.25]),
            box_part([-1.0, -0.5, -0.25], [-0.4, 1.0, 0.25]),
        ]])
        .expect("a wider arm"),
        hulls_of(&[&[
            part_of(corners([-1.0, -1.0, -0.25], [1.0, -0.5, 0.25])),
            part_of({
                let mut p = corners([-1.0, -0.5, -0.25], [-0.5, 1.0, 0.25]);
                p.push([-0.7, 0.0, 0.0]);
                p
            }),
        ]])
        .expect("a ninth point"),
    ];
    for other in same_count {
        assert_ne!(other.digest(), digest);
        assert_eq!(choose(Some(digest), Some(&built(other))), Choice::Unfit);
    }
}

#[test]
fn making_hulls_is_charged_to_the_callers_budget_and_a_refusal_is_a_budget_one() {
    let parts: Vec<MeshPart> = (0..32)
        .map(|i| box_part([i as f32; 3], [i as f32 + 0.5; 3]))
        .collect();
    let few = parts[..4].to_vec();
    let caps = HullCaps::default();
    let cost = |parts: &[MeshPart]| {
        let mut total = 0;
        let made = PropHulls::of_parts(&[parts], BOUNDS, [1.0; 3], &caps, &mut |u| {
            total += u;
            true
        });
        assert!(made.is_ok());
        total
    };
    let (small, large) = (cost(&few), cost(&parts));
    // Every point read (twenty-four a box) and every hull and point of the volume's build: in proportion to the hulls.
    assert!(small > 4 * 24 && large > 7 * small, "{small} and {large}");
    // A budget that falls short at any charge is a refusal for the budget, and is not cached as anything else; exactly enough
    // passes.
    for limit in [0, 1, small / 2, large - 1] {
        let mut spent = 0;
        let made = PropHulls::of_parts(&[&parts], BOUNDS, [1.0; 3], &caps, &mut |u| {
            spent += u;
            spent <= limit
        });
        assert_eq!(made.err(), Some(Refusal::Budget), "limit {limit}");
    }
    let mut spent = 0;
    let made = PropHulls::of_parts(&[&parts], BOUNDS, [1.0; 3], &caps, &mut |u| {
        spent += u;
        spent <= large
    });
    assert!(made.is_ok());
}

/// A shape of the most hulls and points the caps allow: 32 hulls of 64 points each, on small spheres in a row.
fn largest() -> PropHulls {
    let parts: Vec<MeshPart> = (0..MAX_HULLS)
        .map(|i| {
            let points = (0..MAX_HULL_VERTICES)
                .map(|k| {
                    let (a, z) = (k as f32 * 2.399_963, 1.0 - 2.0 * (k as f32 + 0.5) / 64.0);
                    let r = (1.0 - z * z).sqrt() * 0.2;
                    [i as f32 * 0.5 + r * a.cos(), r * a.sin(), z * 0.2]
                })
                .collect();
            part_of(points)
        })
        .collect();
    let made = hulls_of(&[&parts]).expect("the largest shape");
    assert_eq!(made.hulls().len(), MAX_HULLS);
    assert!(made.hulls().iter().all(|h| h.len() == MAX_HULL_VERTICES));
    made
}

#[test]
fn a_script_that_makes_two_hundred_of_the_largest_shapes_in_one_pass_loses_none_and_later_steps_stay_in_budget()
 {
    let hulls = Rc::new(largest());
    let params = PropNumbers {
        size: [16.0, 1.0, 1.0],
        mass: 50.0,
        ..PropNumbers::default()
    }
    .held()
    .with_hulls(hulls.volume());
    let mut w = floor();
    // No tick between the spawns, as a script run in one pass has none.
    for k in 0..200u32 {
        let at = [(k % 20) as f32 * 30.0, (k / 20) as f32 * 6.0, 5.0];
        spawn_hulled(&mut w, 10 + k, params, Some(Rc::clone(&hulls)), at)
            .unwrap_or_else(|e| panic!("spawn {k}: {e}"));
    }
    assert_eq!(w.len(), 200);
    // The work is a good part of the step budget or more, and the steps that follow pay it down a half a budget a step, each
    // running within its budget.
    let budget = w.limits.step_budget;
    let first = w.tick(&[], &far);
    let (bodies, work) = first.built.expect("the report says what was built");
    assert_eq!(bodies, 200);
    assert!(work > budget / 2, "{work} against {budget}");
    let mut worked = Vec::new();
    for _ in 0..12 {
        let report = w.tick(&[], &far);
        assert!(!report.over_budget, "a step ran past its budget");
        assert!(report.work <= budget, "{} against {budget}", report.work);
        worked.push(report.work);
    }
    assert!(first.work <= budget && !first.over_budget);
    // The debt is paid down: the early steps cost more than the late ones.
    assert!(worked[0] > worked[11], "{worked:?}");
    assert_eq!(w.len(), 200);
}

#[test]
fn a_burst_past_the_debt_cap_is_forgiven_down_to_it_and_pays_down_within_eight_steps() {
    let hulls = Rc::new(largest());
    let params = PropNumbers {
        size: [16.0, 1.0, 1.0],
        mass: 50.0,
        ..PropNumbers::default()
    }
    .held()
    .with_hulls(hulls.volume());
    let mut w = floor();
    // A budget of 200,000 units caps the debt at 800,000; 200 of the largest shapes cost about 3.9 million.
    w.limits = PropLimits {
        step_budget: 200_000,
        ..PropLimits::default()
    };
    for k in 0..200u32 {
        let at = [(k % 20) as f32 * 30.0, (k / 20) as f32 * 6.0, 5.0];
        spawn_hulled(&mut w, 10 + k, params, Some(Rc::clone(&hulls)), at)
            .unwrap_or_else(|e| panic!("spawn {k}: {e}"));
    }
    assert_eq!(w.len(), 200, "no spawn is refused");
    let owed = w.owed_work();
    assert!(owed > 4 * 200_000 * 4, "a burst far past the cap: {owed}");
    for step in 0..8 {
        let report = w.tick(&[], &far);
        assert!(!report.over_budget, "step {step} ran past its budget");
        if step == 0 {
            // The excess is forgiven and counted, and what is kept is exactly the cap, less the half budget this step paid.
            assert_eq!(w.forgiven_work, owed - 800_000);
            assert_eq!(w.owed_work(), 800_000 - 100_000);
            assert_eq!(report.built.expect("reported").1, owed);
        }
    }
    assert_eq!(w.owed_work(), 0, "paid down in eight steps");
    assert_eq!(w.len(), 200);
}
