//! Joints in the props' world, stepped alone (no script engine): what each kind holds and how well, the caps, and what a joint does
//! when its prop goes, freezes, or the world is made again.

use torque_plugin_props::data::{PropNumbers, PropParams};
use torque_plugin_props::joints::{
    End, Hinge, Joint, JointError, JointEvent, JointLimits, Kind, Motor, Reach, Spring, Winch,
};
use torque_plugin_props::world::{Owner, OwnerKey, PropLimits, PropWorld, StaticPiece};

const TICKS: u32 = 600;
const GRAVITY: f32 = 20.0;

fn crate_params(mass: f32) -> PropParams {
    PropNumbers {
        mass,
        ..PropNumbers::default()
    }
    .held()
}

/// A world with a floor whose top is at z = 0.
fn world() -> PropWorld {
    let mut w = PropWorld::new(4, PropLimits::default());
    w.sync_statics(vec![(
        1,
        StaticPiece::Box {
            position: [0.0, 0.0, -1.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            half: [500.0, 500.0, 1.0],
        },
    )]);
    w
}

fn spawn(w: &mut PropWorld, id: u32, at: [f32; 3], mass: f32) {
    w.spawn(
        id,
        9,
        crate_params(mass),
        (at, [0.0, 0.0, 0.0, 1.0]),
        None,
        0,
    )
    .expect("the prop spawns");
}

fn near(_: [f32; 3]) -> f32 {
    0.0
}

fn tick(w: &mut PropWorld) -> torque_plugin_props::world::TickReport {
    w.tick(&[], &near)
}

fn middle(id: u32) -> End {
    End::Prop {
        id,
        local: [0.0; 3],
    }
}

fn tether(length: f32) -> Kind {
    Kind::Tether {
        reach: Reach::new(length),
        winch: None,
    }
}

fn distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    (0..3).map(|k| (a[k] - b[k]).powi(2)).sum::<f32>().sqrt()
}

fn position(w: &PropWorld, id: u32) -> [f32; 3] {
    w.get(id).expect("the prop").q.position()
}

#[test]
fn a_hard_tether_holds_a_ten_unit_a_second_pull_within_a_centimeter() {
    let mut w = world();
    let anchor = [0.0, 0.0, 6.0];
    spawn(&mut w, 100, [3.0, 0.0, 6.0], 20.0);
    w.add_joint(
        500,
        Joint::new(tether(3.0), middle(100), End::Point(anchor), None),
    )
    .unwrap();
    let mut worst: f32 = 0.0;
    for _ in 0..TICKS {
        // Pulled straight away from the anchor at 10 units a second, every tick.
        let p = position(&w, 100);
        let d = distance(p, anchor).max(1.0e-3);
        let out: [f32; 3] = std::array::from_fn(|k| (p[k] - anchor[k]) / d * 10.0);
        assert!(w.set_velocity(100, out, [0.0; 3]));
        tick(&mut w);
        worst = worst.max(distance(position(&w, 100), anchor));
    }
    assert!(worst <= 3.0 + 0.01, "the farthest it got: {worst}");
    assert!(w.joint(500).unwrap().status.at_limit);
}

#[test]
fn a_soft_tether_settles_within_five_percent_of_its_rest_stretch() {
    let mut w = world();
    let anchor = [0.0, 0.0, 10.0];
    let (mass, k) = (20.0, 2_000.0);
    spawn(&mut w, 100, [0.0, 0.0, 8.0], mass);
    let reach = Reach {
        spring: Some(Spring {
            stiffness: k,
            damping: 2.0 * (k * mass).sqrt(),
        }),
        ..Reach::new(2.0)
    };
    let kind = Kind::Tether { reach, winch: None };
    w.add_joint(500, Joint::new(kind, middle(100), End::Point(anchor), None))
        .unwrap();
    for _ in 0..TICKS {
        tick(&mut w);
    }
    let rest = mass * GRAVITY / k;
    let stretch = distance(position(&w, 100), anchor) - 2.0;
    assert!(
        (stretch - rest).abs() <= rest * 0.05,
        "stretch {stretch}, rest stretch {rest}"
    );
}

#[test]
fn a_winch_reaches_its_target_within_two_percent_of_the_planned_time() {
    let mut w = world();
    let anchor = [0.0, 0.0, 10.0];
    spawn(&mut w, 100, [0.0, 0.0, 4.0], 20.0);
    w.add_joint(
        500,
        Joint::new(tether(6.0), middle(100), End::Point(anchor), None),
    )
    .unwrap();
    for _ in 0..64 {
        tick(&mut w);
    }
    let (from, to, rate) = (6.0, 2.0, 2.0);
    w.set_winch(
        500,
        Winch {
            target: to,
            rate,
            max_force: 0.0,
        },
    )
    .unwrap();
    let planned = (from - to) / rate / torque_physics_rapier::TICK;
    let mut reached = None;
    for t in 1..=TICKS {
        tick(&mut w);
        let length = w.joint(500).unwrap().kind.reach().unwrap().max;
        if reached.is_none() && length <= to && distance(position(&w, 100), anchor) <= to + 0.01 {
            reached = Some(t as f32);
        }
    }
    let reached = reached.expect("the winch reached its target");
    assert!(
        (reached - planned).abs() <= planned * 0.02,
        "reached at {reached} ticks, planned {planned}"
    );
}

#[test]
fn a_winch_does_not_reel_in_past_its_force() {
    let mut w = world();
    let anchor = [0.0, 0.0, 10.0];
    spawn(&mut w, 100, [0.0, 0.0, 4.0], 200.0);
    w.add_joint(
        500,
        Joint::new(tether(6.0), middle(100), End::Point(anchor), None),
    )
    .unwrap();
    for _ in 0..32 {
        tick(&mut w);
    }
    // The load hangs with 4,000 force units; the winch may pull with 1,000.
    w.set_winch(
        500,
        Winch {
            target: 2.0,
            rate: 2.0,
            max_force: 1_000.0,
        },
    )
    .unwrap();
    for _ in 0..200 {
        tick(&mut w);
    }
    let length = w.joint(500).unwrap().kind.reach().unwrap().max;
    assert!(length > 5.5, "the winch reeled to {length}");
}

/// Where `b` is in `a`'s frame (its position turned back by `a`'s rotation).
fn relative(w: &PropWorld, a: u32, b: u32) -> [f32; 3] {
    let (pa, pb) = (position(w, a), position(w, b));
    let q = w.get(a).unwrap().q.rotation();
    let d: [f32; 3] = std::array::from_fn(|k| pb[k] - pa[k]);
    // The inverse turn of a unit quaternion: v + 2w(-u x v) + 2(-u x (-u x v)).
    let u = [-q[0], -q[1], -q[2]];
    let cross = |a: [f32; 3], b: [f32; 3]| {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    };
    let t = cross(u, d).map(|x| 2.0 * x);
    let c = cross(u, t);
    std::array::from_fn(|k| d[k] + q[3] * t[k] + c[k])
}

#[test]
fn a_weld_drifts_at_most_a_millimeter_over_600_ticks_under_load() {
    let mut w = world();
    // A crate welded to the world at its middle, a heavier one welded beside it: a cantilever under gravity. Neither sleeps, so
    // the welds hold the load every tick.
    let awake = |mass| {
        PropNumbers {
            mass,
            sleep_linear: 0.0,
            sleep_angular: 0.0,
            ..PropNumbers::default()
        }
        .held()
    };
    let rest = [0.0, 0.0, 0.0, 1.0];
    w.spawn(100, 9, awake(20.0), ([0.0, 0.0, 5.0], rest), None, 0)
        .unwrap();
    w.spawn(101, 9, awake(60.0), ([1.0, 0.0, 5.0], rest), None, 0)
        .unwrap();
    let ground = w.weld_frame(Some(100), None).unwrap();
    w.add_joint(
        500,
        Joint::new(Kind::Weld { frame: ground }, middle(100), End::World, None),
    )
    .unwrap();
    let frame = w.weld_frame(Some(100), Some(101)).unwrap();
    w.add_joint(
        501,
        Joint::new(Kind::Weld { frame }, middle(100), middle(101), None),
    )
    .unwrap();
    let welded = relative(&w, 100, 101);
    // The first ticks take the load up (the joint's give, a few wire steps); from then on it must not creep.
    for _ in 0..16 {
        tick(&mut w);
    }
    let loaded = relative(&w, 100, 101);
    assert!(
        distance(loaded, welded) <= 0.003,
        "the load took up {loaded:?}"
    );
    let mut worst: f32 = 0.0;
    for _ in 0..TICKS {
        tick(&mut w);
        worst = worst.max(distance(relative(&w, 100, 101), loaded));
    }
    assert!(worst <= 0.001, "the weld drifted {worst}");
    assert!(w.get(101).unwrap().rest.is_awake());
}

/// A prop's turn about the world's x axis (it is hinged about it).
fn angle_about_x(w: &PropWorld, id: u32) -> f32 {
    let q = w.get(id).unwrap().q.rotation();
    2.0 * q[0].atan2(q[3])
}

#[test]
fn a_hinge_holds_its_limits_within_half_a_degree() {
    let mut w = world();
    spawn(&mut w, 100, [0.0, 0.0, 3.0], 20.0);
    let hinge = Hinge {
        axis_a: [1.0, 0.0, 0.0],
        axis_b: [1.0, 0.0, 0.0],
        limits: Some([-0.5, 0.5]),
        motor: Some(Motor {
            speed: 3.0,
            max_torque: 50_000.0,
        }),
    };
    // Hinged at its top edge's middle, to the world.
    let point = [0.0, 0.0, 3.5];
    let kind = Kind::Hinge(hinge);
    w.add_joint(
        500,
        Joint::new(
            kind,
            End::Prop {
                id: 100,
                local: [0.0, 0.0, 0.5],
            },
            End::Point(point),
            None,
        ),
    )
    .unwrap();
    let (lo, hi) = (-0.5f32, 0.5f32);
    let slack = 0.5f32.to_radians();
    let (mut least, mut most) = (f32::INFINITY, f32::NEG_INFINITY);
    for t in 0..TICKS {
        if t == TICKS / 2 {
            w.set_hinge(500, |h| {
                h.motor = Some(Motor {
                    speed: -3.0,
                    max_torque: 50_000.0,
                })
            })
            .unwrap();
        }
        tick(&mut w);
        let a = angle_about_x(&w, 100);
        least = least.min(a);
        most = most.max(a);
    }
    assert!(
        most <= hi + slack && least >= lo - slack,
        "{least} to {most}"
    );
    // The motor drove it to both limits.
    assert!(
        most >= hi - slack && least <= lo + slack,
        "{least} to {most}"
    );
}

#[test]
fn a_joint_pulled_past_its_break_force_breaks_and_holds_nothing() {
    let mut w = world();
    let anchor = [0.0, 0.0, 10.0];
    spawn(&mut w, 100, [0.0, 0.0, 6.0], 20.0);
    let mut joint = Joint::new(tether(4.0), middle(100), End::Point(anchor), None);
    joint.break_force = 2_000.0;
    w.add_joint(500, joint).unwrap();
    // 400 force units of weight: it holds.
    for _ in 0..32 {
        assert!(tick(&mut w).joint_events.is_empty());
    }
    assert!(!w.joint(500).unwrap().status.broken);
    // 3,000 more hung under it: it breaks.
    spawn(&mut w, 101, [0.0, 0.0, 4.0], 150.0);
    w.add_joint(501, Joint::new(tether(2.0), middle(101), middle(100), None))
        .unwrap();
    let mut broke = false;
    for _ in 0..32 {
        broke |= tick(&mut w)
            .joint_events
            .contains(&(500, JointEvent::Break));
    }
    assert!(broke && w.joint(500).unwrap().status.broken);
    assert_eq!(
        w.physics().joint_count(),
        1,
        "only the unbroken one is a physics joint"
    );
    for _ in 0..64 {
        tick(&mut w);
    }
    assert!(position(&w, 100)[2] < 2.0, "it fell onto the one under it");
}

#[test]
fn a_joint_goes_with_its_prop_and_stays_through_a_freeze_and_a_new_world() {
    let mut w = world();
    spawn(&mut w, 100, [0.0, 0.0, 6.0], 20.0);
    spawn(&mut w, 101, [2.0, 0.0, 6.0], 20.0);
    w.add_joint(500, Joint::new(tether(2.0), middle(100), middle(101), None))
        .unwrap();
    w.add_joint(
        501,
        Joint::new(tether(4.0), middle(100), End::Point([0.0, 0.0, 9.0]), None),
    )
    .unwrap();
    assert_eq!(w.physics().joint_count(), 2);
    w.freeze(101).unwrap();
    assert_eq!(
        w.physics().joint_count(),
        2,
        "a frozen prop keeps its joints"
    );
    w.physics_mut().poison();
    let r = tick(&mut w);
    assert!(r.rebuilt);
    assert_eq!(
        w.physics().joint_count(),
        2,
        "a new world takes the joints again"
    );
    assert!(w.remove(100));
    assert!(w.joints().is_empty());
    let mut gone = w.take_gone_joints();
    gone.sort_unstable();
    assert_eq!(gone, [(500, 0), (501, 0)]);
}

#[test]
fn the_caps_refuse_the_17th_on_a_prop_the_65th_for_a_player_one_past_an_address_and_the_1025th_for_the_server()
 {
    let mut w = world();
    let owner = |conn: u32, address: &str| Owner {
        key: OwnerKey::Connection(conn),
        address: Some(address.into()),
        conn: Some(conn),
        left: None,
    };
    let key = owner(7, "10.0.0.7");
    let mut id = 10_000;
    let mut join = |w: &mut PropWorld, prop: u32, owner: Option<Owner>| {
        id += 1;
        w.add_joint(
            id,
            Joint::new(
                tether(5.0),
                middle(prop),
                End::Point([0.0, 0.0, 9.0]),
                owner,
            ),
        )
    };
    for p in 0..72 {
        spawn(&mut w, 100 + p, [p as f32 * 2.0, 0.0, 0.5], 20.0);
    }
    for _ in 0..16 {
        join(&mut w, 100, None).unwrap();
    }
    assert_eq!(join(&mut w, 100, None), Err(JointError::PerProp));
    for k in 0..64 {
        join(&mut w, 101 + k / 16, Some(key.clone())).unwrap();
    }
    assert_eq!(
        join(&mut w, 110, Some(key.clone())),
        Err(JointError::PerPlayer)
    );
    join(&mut w, 110, Some(owner(8, "10.0.0.8"))).unwrap();
    // The players of one address share its cap whatever their keys.
    w.joint_limits.per_address = 66;
    join(&mut w, 110, Some(owner(9, "10.0.0.7"))).unwrap();
    join(&mut w, 110, Some(owner(10, "10.0.0.7"))).unwrap();
    assert_eq!(
        join(&mut w, 110, Some(owner(11, "10.0.0.7"))),
        Err(JointError::PerAddress)
    );
    w.joint_limits.per_address = JointLimits::default().per_address;
    let mut p = 110;
    while w.joints().len() < JointLimits::default().per_server as usize {
        if w.joints_on(p) >= 16 {
            p += 1;
        }
        join(&mut w, p, None).unwrap();
    }
    assert_eq!(join(&mut w, 171, None), Err(JointError::PerServer));
}

#[test]
fn ends_that_are_not_two_things_are_refused() {
    let mut w = world();
    spawn(&mut w, 100, [0.0, 0.0, 6.0], 20.0);
    let points = Joint::new(
        tether(2.0),
        End::Point([0.0; 3]),
        End::Point([1.0; 3]),
        None,
    );
    assert_eq!(w.add_joint(500, points), Err(JointError::Ends));
    let same = Joint::new(tether(2.0), middle(100), middle(100), None);
    assert_eq!(w.add_joint(500, same), Err(JointError::Ends));
    let missing = Joint::new(tether(2.0), middle(100), middle(999), None);
    assert_eq!(w.add_joint(500, missing), Err(JointError::Ends));
    let player = Joint::new(
        tether(2.0),
        End::Player {
            id: 7,
            offset: [0.0; 3],
        },
        middle(100),
        None,
    );
    assert_eq!(
        w.add_joint(500, player),
        Err(JointError::Ends),
        "a player ends only a grapple"
    );
    let nan = Joint::new(
        tether(2.0),
        middle(100),
        End::Point([f32::NAN, 0.0, 0.0]),
        None,
    );
    assert_eq!(w.add_joint(500, nan), Err(JointError::Ends));
    // The world is a weld's second end, and only that: a tether holds to a point of it, a weld to no point.
    let world_tether = Joint::new(tether(2.0), middle(100), End::World, None);
    assert_eq!(w.add_joint(500, world_tether), Err(JointError::Ends));
    let frame = w.weld_frame(Some(100), None).unwrap();
    let weld = |b: End| Joint::new(Kind::Weld { frame }, middle(100), b, None);
    assert_eq!(
        w.add_joint(500, weld(End::Point([0.0, 0.0, 9.0]))),
        Err(JointError::Ends)
    );
    assert_eq!(
        w.add_joint(
            500,
            Joint::new(Kind::Weld { frame }, End::World, middle(100), None)
        ),
        Err(JointError::Ends)
    );
    // A point held in a prop's frame past the world's bound: refused, as the wire would change it.
    let far = Joint::new(
        tether(2.0),
        End::Prop {
            id: 100,
            local: [2.0e5, 0.0, 0.0],
        },
        End::Point([0.0; 3]),
        None,
    );
    assert_eq!(w.add_joint(500, far), Err(JointError::Ends));
    assert_eq!(w.physics().joint_count(), 0);
    w.add_joint(500, weld(End::World)).unwrap();
    assert_eq!(w.physics().joint_count(), 1);
}

#[test]
fn a_player_has_one_grapple_and_an_offset_past_the_world_is_refused() {
    let mut w = world();
    spawn(&mut w, 100, [0.0, 0.0, 0.5], 20.0);
    let grapple = |offset: [f32; 3]| {
        Joint::new(
            Kind::Grapple {
                reach: Reach::new(5.0),
            },
            End::Player { id: 7, offset },
            End::Point([0.0, 0.0, 9.0]),
            None,
        )
    };
    assert_eq!(
        w.add_joint(500, grapple([0.0, 0.0, 1.0e6])),
        Err(JointError::Ends),
        "the wire holds an offset to the world's bound: the server refuses past it"
    );
    w.add_joint(500, grapple([0.0, 0.0, 1.0])).unwrap();
    assert_eq!(
        w.add_joint(501, grapple([0.0; 3])),
        Err(JointError::Grappled)
    );
    let to_prop = Joint::new(
        Kind::Grapple {
            reach: Reach::new(5.0),
        },
        End::Player {
            id: 7,
            offset: [0.0; 3],
        },
        middle(100),
        None,
    );
    assert_eq!(w.add_joint(501, to_prop), Err(JointError::Grappled));
    assert_eq!(w.grapple_on(7), Some(500));
    // A broken grapple still counts (it stays until deleted): the tether, the pull and a client all find that one.
    w.set_players([(7, [0.0, 8.0, 0.0])].into());
    assert!(w.set_break_force(500, 0.001));
    w.pulled(7, [0.0, -2.0, 0.0], 90.0);
    tick(&mut w);
    assert!(w.joint(500).unwrap().status.broken);
    assert_eq!(
        w.add_joint(501, grapple([0.0; 3])),
        Err(JointError::Grappled)
    );
    assert!(w.tether_of(7).is_none(), "a broken grapple pulls nothing");
    assert!(w.remove_joint(500).is_some());
    w.add_joint(501, grapple([0.0; 3])).unwrap();
    assert!(w.tether_of(7).is_some());
}

#[test]
fn freezing_in_a_stopped_world_changes_nothing_and_the_new_world_keeps_the_joints_whole() {
    let mut w = world();
    spawn(&mut w, 100, [0.0, 0.0, 6.0], 20.0);
    spawn(&mut w, 101, [2.0, 0.0, 6.0], 20.0);
    w.add_joint(500, Joint::new(tether(2.0), middle(100), middle(101), None))
        .unwrap();
    w.physics_mut().poison();
    // The new body is refused and the old one cannot go back: the freeze is refused, and nothing is broken by it.
    assert_eq!(w.freeze(101), None);
    assert!(!w.get(101).unwrap().rest.is_frozen());
    assert!(
        w.take_problems().is_empty(),
        "a stopped world is reported by its rebuild"
    );
    let r = tick(&mut w);
    assert!(r.rebuilt && r.lost.is_empty());
    assert_eq!(w.physics().joint_count(), 1);
    assert!(!w.joint(500).unwrap().status.broken);
    assert!(w.freeze(101).is_some());
    assert_eq!(
        w.physics().joint_count(),
        1,
        "a frozen prop keeps its joints"
    );
}

#[test]
fn a_grapple_tethers_its_player_and_pulls_its_prop_back() {
    let mut w = world();
    spawn(&mut w, 100, [0.0, 0.0, 0.5], 20.0);
    let kind = Kind::Grapple {
        reach: Reach::new(5.0),
    };
    let player = End::Player {
        id: 7,
        offset: [0.0, 0.0, 1.0],
    };
    w.add_joint(500, Joint::new(kind, player, middle(100), None))
        .unwrap();
    // No physics joint: the player is not a body.
    assert_eq!(w.physics().joint_count(), 0);
    w.set_players([(7, [0.0, 8.0, 0.0])].into());
    let t = w.tether_of(7).expect("the grapple's tether");
    assert_eq!(t.length, 5.0);
    assert_eq!(t.offset, [0.0, 0.0, 1.0]);
    assert!(distance(t.anchor, [0.0, 0.0, 0.5]) < 0.01);
    assert!(w.tether_of(8).is_none());
    // The player's step pulled it back toward the crate by 2 units a second: the crate gets the opposite impulse.
    w.pulled(7, [0.0, -2.0, 0.0], 90.0);
    tick(&mut w);
    let v = w.get(100).unwrap().q.state().linvel;
    assert!(v[1] > 1.0, "the crate was pulled toward the player: {v:?}");
    // A player that goes takes its grapple.
    w.set_players(Default::default());
    tick(&mut w);
    assert!(w.joint(500).is_none());
    assert_eq!(w.take_gone_joints(), [(500, 0)]);
}

#[test]
fn hostile_joint_values_are_held_and_never_reach_the_physics_world_unchecked() {
    let mut w = world();
    spawn(&mut w, 100, [0.0, 0.0, 6.0], 20.0);
    let reach = Reach {
        min: f32::NAN,
        max: f32::INFINITY,
        spring: Some(Spring {
            stiffness: f32::MAX,
            damping: -1.0,
        }),
    }
    .held();
    assert!(reach.max <= torque_plugin_props::joints::MAX_LENGTH && reach.min == 0.0);
    let kind = Kind::Tether {
        reach,
        winch: Some(
            Winch {
                target: f32::NAN,
                rate: 1.0e9,
                max_force: f32::NEG_INFINITY,
            }
            .held(),
        ),
    };
    let mut joint = Joint::new(kind, middle(100), End::Point([0.0, 0.0, 9.0]), None);
    joint.break_force = f32::NAN;
    w.add_joint(500, joint).unwrap();
    w.set_hinge(500, |_| {}).ok_or(()).unwrap_err();
    for _ in 0..64 {
        tick(&mut w);
    }
    let s = w.joint(500).unwrap().status;
    assert!(s.distance.is_finite() && s.force.is_finite());
    assert!(position(&w, 100).iter().all(|v| v.is_finite()));
}

#[test]
fn a_grapples_pull_moves_its_prop_by_the_pairs_reduced_mass_and_counts_the_players_momentum() {
    use torque_plugin_props::joints::{FarEnd, grapple_momentum, grapple_pull};
    // A 90 kg Player whose tether took 2 units a second from it, on a 20 kg prop: the prop takes the reduced mass's momentum.
    let change = [0.0, -2.0, 0.0];
    let reduced = 90.0 * 20.0 / 110.0;
    let j = grapple_pull(change, 90.0, FarEnd::Prop(20.0)).unwrap();
    assert!((j[1] - 2.0 * reduced).abs() < 1e-3, "{j:?}");
    assert_eq!(
        grapple_pull(change, 90.0, FarEnd::Fixed),
        Some([0.0, 180.0, 0.0])
    );
    assert_eq!(grapple_momentum(change, 90.0), Some([0.0, 180.0, 0.0]));
    assert_eq!(
        grapple_pull([f32::NAN, 0.0, 0.0], 90.0, FarEnd::Fixed),
        None
    );
    let mut w = world();
    spawn(&mut w, 100, [0.0, 0.0, 0.5], 20.0);
    let grapple = Joint::new(
        Kind::Grapple {
            reach: Reach::new(5.0),
        },
        End::Player {
            id: 7,
            offset: [0.0; 3],
        },
        middle(100),
        None,
    );
    w.add_joint(500, grapple).unwrap();
    w.set_players([(7, [0.0, -5.0, 0.5])].into());
    w.pulled(7, change, 90.0);
    w.tick(&[], &|_| 0.0);
    // The break force counts the Player's momentum (180 over a tick), and the prop moves at the reduced mass's speed.
    let force = w.joint(500).unwrap().status.force;
    assert!((force - 180.0 / 0.032).abs() < 1.0, "{force}");
    let v = w.get(100).unwrap().q.state().linvel;
    assert!(v[1] > 1.0 && v[1] < 2.0 * reduced / 20.0 + 0.05, "{v:?}");
    // A frozen prop is a fixed end: the pull counts the same and moves nothing.
    assert!(w.freeze(100).is_some());
    let before = w.get(100).unwrap().q;
    w.set_players([(7, [0.0, -5.0, 0.5])].into());
    w.pulled(7, change, 90.0);
    w.tick(&[], &|_| 0.0);
    let force = w.joint(500).unwrap().status.force;
    assert!((force - 180.0 / 0.032).abs() < 1.0, "{force}");
    assert_eq!(w.get(100).unwrap().q, before);
}
