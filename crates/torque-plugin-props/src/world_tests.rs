use super::*;
use crate::data::PropNumbers;

fn crate_params() -> PropParams {
    PropNumbers {
        mass: 20.0,
        ..PropNumbers::default()
    }
    .held()
}

/// A world with a floor whose top is at z = 0.
fn world(limits: PropLimits) -> PropWorld {
    let mut w = PropWorld::new(4, limits);
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

fn owner(key: u64) -> Option<Owner> {
    Some(Owner {
        key: OwnerKey::Connection(key as u32),
        address: Some(Rc::from(format!("10.0.0.{key}").as_str())),
        conn: Some(key as u32),
        left: None,
    })
}

fn far(_: [f32; 3]) -> f32 {
    0.0
}

fn at(x: f32, y: f32, z: f32) -> ([f32; 3], [f32; 4]) {
    ([x, y, z], [0.0, 0.0, 0.0, 1.0])
}

#[test]
fn the_caps_refuse_the_65th_for_a_player_and_the_1025th_for_the_server() {
    let mut w = world(PropLimits::default());
    for k in 0..64 {
        w.spawn(
            100 + k,
            9,
            crate_params(),
            at(k as f32 * 2.0, 0.0, 0.5),
            owner(5),
            0,
        )
        .unwrap();
    }
    assert_eq!(
        w.spawn(200, 9, crate_params(), at(0.0, 9.0, 0.5), owner(5), 0),
        Err(SpawnError::PerPlayer)
    );
    // Another player still may.
    w.spawn(200, 9, crate_params(), at(0.0, 9.0, 0.5), owner(6), 0)
        .unwrap();
    // A datablock's own per-player cap.
    let mut few = crate_params();
    few.max_per_player = 1;
    w.spawn(201, 10, few, at(0.0, 12.0, 0.5), owner(6), 0)
        .unwrap();
    assert_eq!(
        w.spawn(202, 10, few, at(0.0, 15.0, 0.5), owner(6), 0),
        Err(SpawnError::PerPlayer)
    );
    // The server's cap, with no owner.
    let mut id = 1_000;
    while w.len() < MAX_PROPS as usize {
        let n = w.len() as f32;
        w.spawn(
            id,
            9,
            crate_params(),
            at(n % 40.0 * 2.0, 20.0 + (n / 40.0).floor() * 2.0, 0.5),
            None,
            0,
        )
        .unwrap();
        id += 1;
    }
    assert_eq!(
        w.spawn(id, 9, crate_params(), at(0.0, -9.0, 0.5), None, 0),
        Err(SpawnError::PerServer)
    );
    // A slot comes back when a prop goes.
    assert!(w.remove(100));
    w.spawn(id, 9, crate_params(), at(0.0, -9.0, 0.5), None, 0)
        .unwrap();
    assert_eq!(w.get(id).unwrap().slot, 0);
    assert_eq!(
        w.spawn(id + 1, 9, crate_params(), at(f32::NAN, 0.0, 0.5), None, 0),
        Err(SpawnError::Bad)
    );
}

#[test]
fn past_the_awake_cap_the_farthest_sleep_first() {
    let limits = PropLimits {
        awake: 256,
        ..PropLimits::default()
    };
    let mut w = world(limits);
    // 257 props in a row along x, dropping; every one awake at first.
    for k in 0..257u32 {
        w.spawn(
            100 + k,
            9,
            crate_params(),
            at(k as f32 * 2.0, 0.0, 3.0),
            None,
            0,
        )
        .unwrap();
    }
    let near = |p: [f32; 3]| p[0] * p[0] + p[1] * p[1];
    let report = w.tick(&[], &near);
    assert_eq!(report.awake, 256);
    assert_eq!(report.capped, 1);
    // The farthest from the player at the origin is the one put to sleep.
    assert!(w.get(356).unwrap().rest.is_asleep());
    assert!(report.events.contains(&(356, PropEvent::Sleep)));
    assert!(!w.get(100).unwrap().rest.is_asleep());
}

#[test]
fn a_prop_falls_lands_and_sleeps_and_a_sleeper_sends_nothing() {
    let mut w = world(PropLimits::default());
    w.spawn(100, 9, crate_params(), at(0.0, 0.0, 3.0), None, 0)
        .unwrap();
    let mut slept = None;
    let mut quiet_after = 0;
    for t in 0..400 {
        let report = w.tick(&[], &far);
        if report.events.contains(&(100, PropEvent::Sleep)) {
            slept = Some(t);
        }
        if slept.is_some_and(|s| t > s) {
            assert!(report.dirty.is_empty(), "tick {t}: {report:?}");
            quiet_after += 1;
        }
    }
    assert!(slept.is_some_and(|t| t < 200), "{slept:?}");
    assert!(quiet_after > 100);
    let p = w.get(100).unwrap();
    assert!(p.rest.is_asleep());
    // On the floor: its bottom at 0, within a centimeter.
    assert!(
        (p.q.position()[2] - 0.5).abs() < 0.01,
        "{:?}",
        p.q.position()
    );
    assert!(!p.q.moving());
}

#[test]
fn every_awake_state_is_written_back_at_the_wires_precision() {
    let mut w = world(PropLimits::default());
    w.spawn(100, 9, crate_params(), at(0.0, 0.0, 3.0), None, 0)
        .unwrap();
    assert!(w.set_velocity(100, [3.0, 1.0, 0.0], [0.5, 0.0, 2.0]));
    for _ in 0..20 {
        w.tick(&[], &far);
        let p = w.get(100).unwrap();
        let held = w.physics().body(BodyId(100)).unwrap();
        let q = p.q.state();
        assert_eq!(held.position, q.position);
        assert_eq!(held.linvel, q.linvel);
        assert_eq!(held.angvel, q.angvel);
        // The world normalizes the rotation it is given; the client does the same with the same bits.
        let r = torque_physics_rapier::guard::rotation(q.rotation).unwrap();
        assert_eq!(held.rotation, r);
    }
}

#[test]
fn a_prop_put_to_sleep_where_it_rests_holds_still_until_a_push_wakes_it() {
    let mut w = world(PropLimits::default());
    // A crate on the floor and a second one on top of it, both put to sleep as they are spawned.
    w.spawn(100, 9, crate_params(), at(0.0, 0.0, 0.5), None, 0)
        .unwrap();
    w.spawn(101, 9, crate_params(), at(0.0, 0.0, 1.5), None, 0)
        .unwrap();
    assert_eq!(w.sleep(100), Some(MASK_STATE | MASK_FLAGS));
    assert_eq!(w.sleep(101), Some(MASK_STATE | MASK_FLAGS));
    assert_eq!(w.sleep(100), None, "asleep already");
    let start: Vec<_> = [100, 101].map(|id| w.get(id).unwrap().q).to_vec();
    for _ in 0..200 {
        let r = w.tick(&[], &far);
        assert!(r.dirty.is_empty() && r.events.is_empty(), "{r:?}");
    }
    assert_eq!([100, 101].map(|id| w.get(id).unwrap().q).to_vec(), start);
    // A player walking into the lower one wakes it and carries it along.
    let pusher = Pusher {
        id: 7,
        min: [-1.55, -0.5, 0.0],
        max: [-0.5, 0.5, 2.0],
        velocity: [4.0, 0.0, 0.0],
        mass: 90.0,
    };
    let r = w.tick(&[pusher], &far);
    assert!(r.events.contains(&(100, PropEvent::Wake)), "{r:?}");
    for _ in 0..10 {
        w.tick(&[], &far);
    }
    assert!(w.get(100).unwrap().q.position()[0] > 0.1);
    // A frozen prop is not put to sleep.
    w.spawn(102, 9, crate_params(), at(9.0, 0.0, 0.5), None, 0)
        .unwrap();
    w.freeze(102).unwrap();
    assert_eq!(w.sleep(102), None);
    assert_eq!(w.sleep(999), None);
}

#[test]
fn a_frozen_prop_holds_still_and_takes_no_push() {
    let mut w = world(PropLimits::default());
    w.spawn(100, 9, crate_params(), at(0.0, 0.0, 3.0), None, 0)
        .unwrap();
    assert_eq!(w.freeze(100), Some(MASK_STATE | MASK_FLAGS));
    assert_eq!(w.freeze(100), None);
    assert!(!w.impulse(100, [0.0, 0.0, 500.0]));
    for _ in 0..30 {
        let r = w.tick(&[], &far);
        assert!(r.dirty.is_empty());
    }
    assert_eq!(w.get(100).unwrap().q.position(), [0.0, 0.0, 3.0]);
    assert!(w.unfreeze(100).is_some());
    for _ in 0..30 {
        w.tick(&[], &far);
    }
    assert!(w.get(100).unwrap().q.position()[2] < 2.0);
    // A datablock that cannot be frozen.
    let mut no = crate_params();
    no.can_freeze = false;
    w.spawn(101, 9, no, at(5.0, 0.0, 0.5), None, 0).unwrap();
    assert_eq!(w.freeze(101), None);
}

#[test]
fn a_player_walking_into_a_prop_pushes_it_along() {
    let mut w = world(PropLimits::default());
    w.spawn(100, 9, crate_params(), at(0.0, 2.0, 0.5), None, 0)
        .unwrap();
    for _ in 0..40 {
        w.tick(&[], &far);
    }
    let start = w.get(100).unwrap().q.position();
    // A player of 90 kg against the prop's near face, walking north at 6 units a second.
    for _ in 0..30 {
        let face = w.get(100).unwrap().q.position()[1] - 0.5;
        let pusher = Pusher {
            id: 7,
            min: [-0.5, face - 1.02, 0.0],
            max: [0.5, face - 0.02, 2.0],
            velocity: [0.0, 6.0, 0.0],
            mass: 90.0,
        };
        w.tick(&[pusher], &far);
        assert!(w.get(100).unwrap().touched.contains(&7));
    }
    let end = w.get(100).unwrap().q.position();
    assert!(end[1] - start[1] > 2.0, "{start:?} -> {end:?}");
    // Walking away pushes nothing.
    let away = Pusher {
        id: 7,
        min: [-0.5, end[1] - 1.5, 0.0],
        max: [0.5, end[1] - 0.45, 2.0],
        velocity: [0.0, -6.0, 0.0],
        mass: 90.0,
    };
    let before = w.get(100).unwrap().q;
    w.tick(&[away], &far);
    let after = w.get(100).unwrap().q;
    assert!(after.state().linvel[1] <= before.state().linvel[1] + 1e-3);
}

#[test]
fn a_step_past_the_budget_does_not_run_and_lowers_the_awake_cap() {
    let limits = PropLimits {
        step_budget: 1_000,
        ..PropLimits::default()
    };
    let mut w = world(limits);
    for k in 0..64u32 {
        w.spawn(
            100 + k,
            9,
            crate_params(),
            at((k % 8) as f32 * 2.0, (k / 8) as f32 * 2.0, 3.0),
            None,
            0,
        )
        .unwrap();
    }
    let report = w.tick(&[], &far);
    assert!(report.over_budget);
    assert_eq!(report.awake, 48);
    assert_eq!(w.get(100).unwrap().q.position()[2], 3.0);
    // With room again the cut eases back.
    w.limits.step_budget = 100_000_000;
    for _ in 0..20 {
        w.tick(&[], &far);
    }
    assert!(w.over_budget >= 1);
    assert!(w.cut.is_none());
}

#[test]
fn two_worlds_given_the_same_calls_hold_the_same_bits() {
    let run = || {
        let mut w = world(PropLimits::default());
        for k in 0..32u32 {
            let p = (
                [
                    (k % 4) as f32 * 0.9,
                    (k / 4 % 4) as f32 * 0.9,
                    1.0 + (k / 16) as f32 * 1.1,
                ],
                [0.0, 0.0, 0.1, 1.0],
            );
            w.spawn(100 + k, 9, crate_params(), p, None, 0).unwrap();
        }
        let mut words = Vec::new();
        for t in 0..120u32 {
            let pusher = Pusher {
                id: 7,
                min: [-4.0 + t as f32 * 0.05, -0.5, 0.0],
                max: [-3.0 + t as f32 * 0.05, 0.5, 2.0],
                velocity: [1.6, 0.0, 0.0],
                mass: 90.0,
            };
            w.tick(&[pusher], &far);
            words.extend(w.state_words());
        }
        words
    };
    assert_eq!(run(), run());
}

#[test]
fn statics_come_and_go_with_the_map() {
    let mut w = world(PropLimits::default());
    assert_eq!(w.statics(), 1);
    w.sync_statics(Vec::new());
    assert_eq!(w.statics(), 0);
    w.spawn(100, 9, crate_params(), at(0.0, 0.0, 1.0), None, 0)
        .unwrap();
    for _ in 0..30 {
        w.tick(&[], &far);
    }
    // No floor: it falls.
    assert!(w.get(100).unwrap().q.position()[2] < -1.0);
}

#[test]
fn the_players_of_one_address_share_its_cap_whatever_their_keys() {
    let mut w = world(PropLimits {
        per_address: 10,
        per_player: 4,
        ..PropLimits::default()
    });
    let mut id = 100;
    // Five keys from one address, each asking for four: ten are taken, the rest refused, orphans counted.
    let mut made = 0;
    for k in 0..5u32 {
        let o = Owner {
            key: OwnerKey::Named(Rc::from(format!("player-{k}").as_bytes())),
            address: Some(Rc::from("192.168.1.9")),
            conn: (k % 2 == 0).then_some(k),
            left: (k % 2 == 1).then_some(3),
        };
        for _ in 0..4 {
            match w.spawn(
                id,
                9,
                crate_params(),
                at(id as f32 * 2.0, 0.0, 0.5),
                Some(o.clone()),
                0,
            ) {
                Ok(()) => made += 1,
                Err(e) => assert!(
                    matches!(e, SpawnError::PerAddress | SpawnError::PerPlayer),
                    "{e:?}"
                ),
            }
            id += 1;
        }
    }
    assert_eq!(made, 10);
    assert_eq!(w.owned_from("192.168.1.9"), 10);
    // Another address is not held back.
    assert!(
        w.spawn(id, 9, crate_params(), at(0.0, 30.0, 0.5), owner(3), 0)
            .is_ok()
    );
}

#[test]
fn an_owner_key_is_its_whole_text_up_to_its_cap() {
    let long = vec![b'k'; 300];
    let OwnerKey::Named(k) = OwnerKey::new(&long, 7) else {
        panic!("a named key");
    };
    assert_eq!(k.len(), MAX_OWNER_KEY);
    assert_eq!(OwnerKey::new(b"", 7), OwnerKey::Connection(7));
    // Two texts a 64-bit hash could confuse are told apart: the text is compared whole.
    assert_ne!(OwnerKey::new(b"alice", 1), OwnerKey::new(b"alicf", 1));
}

#[test]
fn the_statics_kept_are_the_lowest_ids_and_stay_the_same() {
    let mut w = PropWorld::new(4, PropLimits::default());
    let pieces = |n: u32| -> Vec<(u32, StaticPiece)> {
        (0..n)
            .rev()
            .map(|k| {
                (
                    1_000 + k,
                    StaticPiece::Box {
                        position: [k as f32 * 3.0, 0.0, -1.0],
                        rotation: [0.0, 0.0, 0.0, 1.0],
                        half: [1.0; 3],
                    },
                )
            })
            .collect()
    };
    w.sync_statics(pieces(600));
    let kept: Vec<u32> = w.static_ids().collect();
    assert_eq!(kept.len(), MAX_STATICS);
    assert_eq!(
        kept,
        (1_000..1_000 + MAX_STATICS as u32).collect::<Vec<_>>()
    );
    w.sync_statics(pieces(600));
    assert_eq!(w.static_ids().collect::<Vec<_>>(), kept);
    // One of the kept goes: the next lowest takes its place.
    let mut fewer = pieces(600);
    fewer.retain(|(id, _)| *id != 1_000);
    w.sync_statics(fewer);
    let now: Vec<u32> = w.static_ids().collect();
    assert_eq!(now.len(), MAX_STATICS);
    assert_eq!(now.first(), Some(&1_001));
    assert_eq!(now.last(), Some(&(1_000 + MAX_STATICS as u32)));
}

#[test]
fn a_stopped_world_is_made_again_from_the_props_states() {
    let mut w = world(PropLimits::default());
    for k in 0..6u32 {
        w.spawn(
            100 + k,
            9,
            crate_params(),
            at(k as f32 * 2.0, 0.0, 2.0),
            None,
            0,
        )
        .unwrap();
    }
    w.freeze(105).unwrap();
    for _ in 0..10 {
        w.tick(&[], &far);
    }
    let frozen = w.get(105).unwrap().q;
    w.physics_mut().poison();
    let report = w.tick(&[], &far);
    assert!(report.rebuilt);
    assert_eq!(w.rebuilds, 1);
    assert!(!w.physics().is_poisoned());
    assert_eq!(w.len(), 6);
    assert_eq!(w.statics(), 1);
    // The frozen prop is where it was, the rest go on falling from where they were.
    assert_eq!(w.get(105).unwrap().q, frozen);
    for _ in 0..200 {
        assert!(!w.tick(&[], &far).rebuilt);
    }
    assert!(
        w.props()
            .values()
            .all(|p| (p.q.position()[2] - 0.5).abs() < 0.02 || p.rest.is_frozen())
    );
}

#[test]
fn a_prop_the_rebuilt_world_does_not_take_is_taken_out_and_reported() {
    let mut w = world(PropLimits::default());
    for k in 0..4u32 {
        w.spawn(
            100 + k,
            9,
            crate_params(),
            at(k as f32 * 2.0, 0.0, 0.5),
            None,
            0,
        )
        .unwrap();
    }
    // Room for the floor and two props in the world made again.
    w.config_mut().limits.max_bodies = 3;
    w.physics_mut().poison();
    let report = w.tick(&[], &far);
    assert!(report.rebuilt);
    assert_eq!(report.lost, vec![102, 103]);
    assert_eq!(w.props().keys().copied().collect::<Vec<_>>(), [100, 101]);
    // The new world is full: a spawn is refused by it, not by the props' own caps.
    assert_eq!(
        w.spawn(200, 9, crate_params(), at(9.0, 9.0, 0.5), None, 0),
        Err(SpawnError::World)
    );
}

/// The collision source's revision: new at a spawn, at every tick a prop moves, at a removal and a restore; the same over
/// ticks where every prop sleeps, so the shape set is shared only while it is right.
#[test]
fn the_revision_moves_with_every_change_of_a_props_slot_or_pose_and_only_then() {
    let mut w = world(PropLimits::default());
    let empty = w.revision();
    w.spawn(10, 9, crate_params(), at(0.0, 0.0, 2.0), None, 0)
        .expect("a crate");
    let spawned = w.revision();
    assert_ne!(spawned, empty);
    // Falling: every tick moves it, and each gives a revision never seen before.
    let mut seen = vec![empty, spawned];
    for _ in 0..5 {
        w.tick(&[], &far);
        assert!(!seen.contains(&w.revision()), "a moving tick");
        seen.push(w.revision());
    }
    // Asleep on the floor: ticks change nothing.
    for _ in 0..600 {
        w.tick(&[], &far);
    }
    assert!(w.get(10).unwrap().rest.is_asleep());
    let resting = w.revision();
    for _ in 0..10 {
        w.tick(&[], &far);
    }
    assert_eq!(w.revision(), resting, "nothing moved");
    // A restore to another state and a removal each move it.
    let mut q = w.get(10).unwrap().q;
    q.pos[0] += 100;
    assert!(w.restore(10, q, Rest::Awake));
    assert_ne!(w.revision(), resting);
    let restored = w.revision();
    assert!(w.remove(10));
    assert_ne!(w.revision(), restored);
    // A world made again never answers an old revision.
    assert!(!seen.contains(&PropWorld::new(4, PropLimits::default()).revision()));
}

/// A player's box of `id` standing with its feet at `at`.
fn standing(id: u32, at: [f32; 3]) -> Pusher {
    Pusher {
        id,
        min: [at[0] - 0.5, at[1] - 0.5, at[2]],
        max: [at[0] + 0.5, at[1] + 0.5, at[2] + 2.0],
        velocity: [0.0; 3],
        mass: 90.0,
    }
}

#[test]
fn a_box_the_physics_world_refuses_is_counted_noted_and_tried_again() {
    let mut w = world(PropLimits::default());
    // A world made again with room for the floor alone: no box fits.
    w.config_mut().limits.max_bodies = 1;
    w.physics_mut().poison();
    let (a, b) = (standing(7, [0.0; 3]), standing(8, [3.0, 0.0, 0.0]));
    w.set_bystanders(vec![b]);
    w.tick(&[a], &far);
    assert_eq!(w.box_refused, 2);
    let problems = w.take_problems();
    assert_eq!(problems.len(), 2, "{problems:?}");
    assert!(problems[0].contains("pusher 7's box"), "{problems:?}");
    // Each tick the refused boxes are tried again, and counted again.
    w.tick(&[a], &far);
    assert_eq!(w.box_refused, 4);
    assert_eq!(w.take_problems().len(), 2);
    // Room again: both boxes are made, and nothing more is counted.
    w.config_mut().limits.max_bodies = 8;
    w.physics_mut().poison();
    w.tick(&[a], &far);
    assert_eq!(w.box_refused, 4);
    assert!(w.physics().body(BodyId(7)).is_some() && w.physics().body(BodyId(8)).is_some());
    assert!(w.take_problems().is_empty());
}

#[test]
fn a_bystander_blocks_a_prop_and_pushes_nothing_and_goes_when_set_again() {
    let mut w = world(PropLimits::default());
    w.spawn(100, 9, crate_params(), at(0.0, 0.0, 0.5), None, 0)
        .expect("a crate");
    // A box beside it that stands still (no velocity, as a record gives): the crate it overlaps is shoved out, none pushed.
    w.set_bystanders(vec![standing(8, [0.6, 0.0, 0.0])]);
    for _ in 0..30 {
        w.tick(&[], &far);
    }
    let crate_at = w.get(100).expect("the crate").q.position();
    assert!(crate_at[0] < 0.0, "pushed out of the box: {crate_at:?}");
    assert!(w.physics().body(BodyId(8)).is_some());
    // Set again without it: the box goes.
    w.set_bystanders(Vec::new());
    w.tick(&[], &far);
    assert!(w.physics().body(BodyId(8)).is_none());
}

#[test]
fn a_hostile_box_is_left_out_and_counted_and_a_stopped_world_is_counted_apart() {
    let mut w = world(PropLimits::default());
    let wide = |id: u32, half: f32| Pusher {
        min: [-half; 3],
        max: [half; 3],
        ..standing(id, [0.0; 3])
    };
    let nan = Pusher {
        min: [f32::NAN; 3],
        ..standing(9, [0.0; 3])
    };
    // A box of a hundred thousand units, one that is not a number, and the widest a body may be.
    w.set_bystanders(vec![wide(8, 1.0e5), nan, wide(10, MAX_PUSHER_HALF)]);
    w.tick(&[standing(7, [20.0, 0.0, 0.0])], &far);
    assert_eq!((w.box_rejected, w.box_refused, w.box_stopped), (2, 0, 0));
    assert!(w.physics().body(BodyId(7)).is_some() && w.physics().body(BodyId(10)).is_some());
    assert!(w.physics().body(BodyId(8)).is_none() && w.physics().body(BodyId(9)).is_none());
    // A world that stopped under the call is no refusal of the world's caps.
    w.physics_mut().poison();
    w.place_pushers(&[standing(11, [40.0, 0.0, 0.0])]);
    assert_eq!((w.box_rejected, w.box_refused, w.box_stopped), (2, 0, 1));
}

#[test]
fn bystanders_are_made_in_ascending_id_order_among_the_pushers() {
    let mut w = world(PropLimits::default());
    let at = |id: u32| standing(id, [id as f32 * 3.0, 0.0, 0.0]);
    w.set_bystanders(vec![at(9), at(3), at(5), at(3)]);
    w.tick(&[at(4)], &far);
    let ids: Vec<u32> = w.bodies.iter().map(|b| b.0).collect();
    assert_eq!(ids, [3, 4, 5, 9]);
}

#[test]
fn a_bystander_whose_box_turns_hostile_loses_its_body() {
    let mut w = world(PropLimits::default());
    w.set_bystanders(vec![standing(8, [5.0, 0.0, 0.0])]);
    w.tick(&[], &far);
    assert!(w.physics().body(BodyId(8)).is_some());
    // The next record says a box of a hundred thousand units, then one that is not a number: no body stays at the old place.
    for hostile in [
        Pusher {
            min: [-1.0e5; 3],
            max: [1.0e5; 3],
            ..standing(8, [5.0, 0.0, 0.0])
        },
        Pusher {
            min: [f32::NAN; 3],
            ..standing(8, [5.0, 0.0, 0.0])
        },
    ] {
        w.set_bystanders(vec![standing(8, [5.0, 0.0, 0.0])]);
        w.tick(&[], &far);
        assert!(w.physics().body(BodyId(8)).is_some());
        let before = w.box_rejected;
        w.set_bystanders(vec![hostile]);
        w.tick(&[], &far);
        assert!(w.physics().body(BodyId(8)).is_none(), "{hostile:?}");
        assert_eq!(w.box_rejected, before + 1);
    }
}
