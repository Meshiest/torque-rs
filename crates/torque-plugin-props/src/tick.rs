//! The server's tick hook, after the process list: deleted props and owners tended, the map's statics looked for, the players'
//! and vehicles' pushes gathered, the world stepped ([`crate::world`]) and what changed written back and called back.

use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use torque_collide::types::STATIC_SHAPE;
use torque_engine::{Engine, TickInfo, VmPrefs};
use torque_ts_vm::vm::Vm;

use crate::hits::{self, Blast, Hit};
use crate::joints::server as joint_server;
use crate::natives::control_of;
use crate::server::{
    CLASS, PropPrefs, ServerProps, address_of, delete_logged, is_class, joint_limits_of, limits_of,
    owner_key, shot_push, write_back, write_record,
};
use crate::world::{Owner, OwnerKey, PlayerAt, PropEvent, PropWorld, Pusher, StaticPiece};

/// Ticks between two looks for the map pieces and static shapes props land on.
pub const STATICS_EVERY: u64 = 64;

/// The server's tick hook ([module docs](self)).
pub fn tick(vm: &mut Vm, props: &ServerProps) -> Result<(), String> {
    // A player at a grapple's end may hang on a parent that moved since the last resolve: its anchor is where the hierarchy
    // puts it.
    torque_game::scene::links::resolve(vm);
    let prefs = vm.prefs::<PropPrefs>();
    let (events, expired, rebuilt, broke, gone_joints, problems, hit) = {
        let mut srv = props.0.borrow_mut();
        srv.tick += 1;
        srv.net_share = prefs.net_share;
        let tick = srv.tick;
        let (world, links) = srv.world_and_links(vm);
        world.limits = limits_of(&prefs);
        world.joint_limits = joint_limits_of(&prefs);
        // Joints whose objects a script deleted, and where the players at grapples' ends are.
        let dropped: Vec<u32> = world
            .joints()
            .keys()
            .filter(|&&id| !is_class(vm, id, joint_server::CLASS))
            .copied()
            .collect();
        let mut gone_joints: Vec<(u32, u32)> = dropped
            .into_iter()
            .filter_map(|id| world.remove_joint(id).map(|j| (id, j.rope)))
            .collect();
        world.set_players(grappled_players(vm, world));
        // Props whose objects a script deleted.
        let gone: Vec<u32> = world
            .props()
            .keys()
            .filter(|&&id| !is_class(vm, id, CLASS))
            .copied()
            .collect();
        for id in gone {
            world.remove(id);
        }
        let expired = tend_owners(vm, world, tick);
        // The hierarchy's places for props are made before the step: parts join their roots, driven props are given their targets.
        crate::hierarchy::reconcile(vm, links, world);
        if tick == 1 || tick.is_multiple_of(STATICS_EVERY) {
            world.sync_statics(statics_of(vm));
        }
        let state = &mut *srv;
        let hit = projectile_pushes(vm, &mut state.hits, &mut state.blasts, &mut state.world);
        let world = state.world_mut(vm);
        let (pushers, players) = pushers_of(vm);
        let near = |p: [f32; 3]| {
            players
                .iter()
                .map(|q| {
                    let d: [f32; 3] = std::array::from_fn(|k| p[k] - q[k]);
                    d[0] * d[0] + d[1] * d[1] + d[2] * d[2]
                })
                .fold(f32::INFINITY, f32::min)
        };
        let report = world.tick(&pushers, &near);
        for &(id, mask) in &report.dirty {
            write_back(vm, world, id, mask);
        }
        for &id in &report.moved {
            write_record(vm, world, id);
        }
        for &(id, mask) in &report.joints {
            joint_server::write_back(vm, world, id, mask);
        }
        let broke: Vec<u32> = report.joint_events.iter().map(|&(id, _)| id).collect();
        gone_joints.extend(world.take_gone_joints());
        let events: Vec<(u32, u32, PropEvent)> = report
            .events
            .iter()
            .filter_map(|&(id, e)| world.get(id).map(|p| (id, p.data, e)))
            .collect();
        // A prop the world would not put back after a change is deleted, as one that expired.
        let mut problems = world.take_problems();
        problems.extend(report.dropped.iter().map(|id| {
            format!("props: prop {id} could not be put back in the physics world; it is deleted")
        }));
        let mut expired = expired;
        expired.extend(&report.dropped);
        (
            events,
            expired,
            report.rebuilt.then_some(report.lost),
            broke,
            gone_joints,
            problems,
            hit,
        )
    };
    for problem in problems {
        vm.warn(problem);
    }
    if let Some(lost) = rebuilt {
        vm.warn(
            "props: the physics world stopped on a caught panic; it was made again from each prop's last state".to_string(),
        );
        for id in lost {
            vm.warn(format!(
                "props: prop {id} could not be made again in the new physics world; it is deleted"
            ));
            delete_logged(vm, id);
        }
    }
    for id in expired {
        delete_logged(vm, id);
    }
    // The props that went took their joints with them: their objects and ropes go too.
    joint_server::delete_objects(vm, &gone_joints);
    // A callback that stops on an error is printed, and the ones after it still run.
    for id in broke {
        if is_class(vm, id, joint_server::CLASS)
            && let Err(e) = vm.call_method(id, b"onBreak", &[])
        {
            vm.warn(format!("props: joint {id}'s onBreak: {e}"));
        }
    }
    refresh_controls(vm, props);
    // A prop a projectile hit is told so (after the projectile's own `onCollision`, which ran in its tick).
    for h in hit {
        let args = [
            h.prop.to_string().into_bytes(),
            h.by.to_string().into_bytes(),
            torque_game::sim::hosted::text3_fixed(h.point),
            torque_game::sim::hosted::text3_fixed(h.normal),
        ];
        if is_class(vm, h.prop, CLASS)
            && let Err(e) = vm.call_method(h.data, b"onCollision", &args)
        {
            vm.warn(format!("props: prop {}'s onCollision: {e}", h.prop));
        }
    }
    for (id, data, event) in events {
        let name = match event {
            PropEvent::Sleep => "onPropSleep",
            PropEvent::Wake => "onPropWake",
        };
        if let Err(e) = vm.call_method(data, name.as_bytes(), &[id.to_string().into_bytes()]) {
            vm.warn(format!("props: prop {id}'s {name}: {e}"));
        }
    }
    Ok(())
}

/// Owners that left (their props and joints become orphans), came back under the same key (they take them back, unless two
/// connections there share that key: then nobody does), or have been gone longer than the orphan time (their props are taken
/// out of the world and returned, for their objects to be deleted; their joints go after the tick, with the ones that went with
/// their props).
fn tend_owners(vm: &mut Vm, world: &mut PropWorld, tick: u64) -> Vec<u32> {
    let mut keys: Option<BTreeMap<OwnerKey, Option<u32>>> = None;
    let orphan = world.limits.orphan_ticks;
    let props: Vec<(u32, Owner)> = world
        .props()
        .iter()
        .filter_map(|(&id, p)| Some((id, p.owner.clone()?)))
        .collect();
    let joints: Vec<(u32, Owner)> = world
        .joints()
        .iter()
        .filter_map(|(&id, j)| Some((id, j.owner.clone()?)))
        .collect();
    let mut expired = Vec::new();
    for (id, o) in props {
        match tended(vm, &o, tick, orphan, &mut keys) {
            Tended::Kept => {}
            Tended::Changed(owner) => {
                if let Some(mask) = world.set_owner(id, Some(owner)) {
                    write_back(vm, world, id, mask);
                }
            }
            Tended::Expired => expired.push(id),
        }
    }
    // A joint's owner is tended by the same rule; an expired one goes, its object deleted after the tick with the joints that
    // went with their props.
    for (id, o) in joints {
        match tended(vm, &o, tick, orphan, &mut keys) {
            Tended::Kept => {}
            Tended::Changed(owner) => {
                world.set_joint_owner(id, Some(owner));
            }
            Tended::Expired => world.expire_joint(id),
        }
    }
    for &id in &expired {
        world.remove(id);
    }
    expired
}

/// What tending did to an owner.
enum Tended {
    Kept,
    Changed(Owner),
    Expired,
}

/// An owner tended ([`tend_owners`]): its connection went (an orphan from `tick`), it came back under its key (with the
/// connection's address now), or it has been gone longer than `orphan` ticks.
fn tended(
    vm: &mut Vm,
    o: &Owner,
    tick: u64,
    orphan: u64,
    keys: &mut Option<BTreeMap<OwnerKey, Option<u32>>>,
) -> Tended {
    match o.conn {
        Some(c) if vm.objects().contains(c) => Tended::Kept,
        Some(_) => Tended::Changed(Owner {
            conn: None,
            left: Some(tick),
            ..o.clone()
        }),
        None => {
            let keys = keys.get_or_insert_with(|| live_owner_keys(vm));
            if let Some(&Some(c)) = keys.get(&o.key) {
                Tended::Changed(Owner {
                    conn: Some(c),
                    left: None,
                    address: Some(address_of(vm, c)),
                    ..o.clone()
                })
            } else if o.left.is_some_and(|t| tick.saturating_sub(t) > orphan) {
                Tended::Expired
            } else {
                Tended::Kept
            }
        }
    }
}

/// The owner key of every connection there is now, with the connection; none for a key two connections share (the props
/// of that key go to neither).
fn live_owner_keys(vm: &Vm) -> BTreeMap<OwnerKey, Option<u32>> {
    let conns: Vec<u32> = vm
        .objects()
        .iter()
        .filter(|(_, o)| vm.objects().classes.name(o.class()) == b"GameConnection")
        .map(|(id, _)| id)
        .collect();
    let mut keys: BTreeMap<OwnerKey, Option<u32>> = BTreeMap::new();
    for c in conns {
        keys.entry(owner_key(vm, c))
            .and_modify(|held| *held = None)
            .or_insert(Some(c));
    }
    keys
}

/// Where each player at a grapple's first end is now.
fn grappled_players(vm: &Vm, world: &PropWorld) -> BTreeMap<u32, PlayerAt> {
    world
        .joints()
        .values()
        .filter_map(|j| match j.a {
            crate::joints::End::Player { id, .. } => Some(id),
            _ => None,
        })
        .filter_map(|id| Some((id, player_at(vm, id)?)))
        .collect()
}

/// Where Player `id` is for the joints at it: its position and the way its transform faces (a client's prediction reads its
/// predicted Player from its box's floor, the same point to a float's last bits, and the same transform).
fn player_at(vm: &Vm, id: u32) -> Option<PlayerAt> {
    let s = torque_game::sim::player::state(vm, id)?;
    let m = &s.obj_to_world;
    Some(PlayerAt::facing(
        torque_game::sim::player::position(vm, id)?,
        [m[1], m[5]],
    ))
}

/// The object each owner's connection controls now, for the priority of the next packets.
fn refresh_controls(vm: &mut Vm, props: &ServerProps) {
    let owners: BTreeSet<u32> = props
        .with(|s| {
            s.world.as_ref().map_or_else(Default::default, |w| {
                w.props()
                    .values()
                    .filter_map(|p| p.owner.as_ref()?.conn)
                    .collect()
            })
        })
        .unwrap_or_default();
    let controls = owners
        .into_iter()
        .filter_map(|c| Some((c, control_of(vm, c)?)))
        .collect();
    props.0.borrow_mut().controls = controls;
}

/// The map pieces and static shapes now: terrains and interiors by their collision, static shapes by their box.
fn statics_of(vm: &Vm) -> Vec<(u32, StaticPiece)> {
    let mut out = Vec::new();
    for (id, _) in vm.objects().iter() {
        if let Some(map) = torque_game::scene::ext::map_of(vm, id) {
            if let Some(shape) = &map.shape {
                out.push((id, StaticPiece::Map(Rc::clone(shape))));
            }
            continue;
        }
        let Some(view) = torque_game::scene::container_world::object_view(vm, id) else {
            continue;
        };
        if view.type_mask & (STATIC_SHAPE | torque_collide::types::STATIC_TS) == 0
            || view.collision_disabled
        {
            continue;
        }
        if let Some(piece) = StaticPiece::of_view(&view) {
            out.push((id, piece));
        }
    }
    out
}

/// A hit that pushed a prop, for its datablock's `onCollision`.
struct Pushed {
    prop: u32,
    data: u32,
    by: u32,
    point: [f32; 3],
    normal: [f32; 3],
}

/// The projectiles' pushes before the step: each hit kept since the last one ([`crate::hits::hit_impulse`], at the hit point, by
/// what its projectile's datablock says), then each explosion since the last step on every prop in its reach (in id order).
/// Gives the hits that pushed a prop. Both lists are emptied: an explosion pushes the props there are when it went off.
fn projectile_pushes(
    vm: &Vm,
    hits: &mut Vec<Hit>,
    blasts: &mut Vec<Blast>,
    world: &mut Option<PropWorld>,
) -> Vec<Pushed> {
    let (hits, blasts) = (std::mem::take(hits), std::mem::take(blasts));
    let Some(world) = world.as_mut().filter(|w| !w.is_empty()) else {
        return Vec::new();
    };
    let mut pushed = Vec::new();
    for h in hits {
        let Some(data) = world.get(h.prop).map(|p| p.data) else {
            continue;
        };
        let Some(j) = hits::hit_impulse(h.velocity, &shot_push(vm, h.datablock)) else {
            continue;
        };
        if world.impulse_at(h.prop, j, h.point) {
            pushed.push(Pushed {
                prop: h.prop,
                data,
                by: h.by,
                point: h.point,
                normal: h.normal,
            });
        }
    }
    for blast in blasts {
        let push = shot_push(vm, blast.datablock);
        if !push.blasts() {
            continue;
        }
        let near: Vec<(u32, [f32; 3])> = world
            .props()
            .iter()
            .filter_map(|(&p, prop)| {
                Some((p, hits::blast_impulse(blast.at, prop.q.position(), &push)?))
            })
            .collect();
        for (p, j) in near {
            world.impulse(p, j);
        }
    }
    pushed
}

/// The players and vehicles as pushers: a player pushes at the speed its move asks for (it stops at the prop it walks into), a
/// vehicle at its own. With them, the center of each player's box (what the awake cap's distances are to), from the same walk
/// of the objects.
fn pushers_of(vm: &Vm) -> (Vec<Pusher>, Vec<[f32; 3]>) {
    let mut out = Vec::new();
    let mut players = Vec::new();
    for (id, _) in vm.objects().iter() {
        if let Some(s) = torque_game::sim::player::state(vm, id) {
            let (a, b) = (s.world_box.min, s.world_box.max);
            players.push(std::array::from_fn(|k| (a[k] + b[k]) * 0.5));
            let m = s.obj_to_world;
            let speed =
                torque_game::sim::player::info(vm, id).map_or(6.0, |i| i.data.max_forward_speed);
            let (x, y) = (s.last_move.x, s.last_move.y);
            let wish = [0, 1, 2].map(|r| (m[r * 4] * x + m[r * 4 + 1] * y) * speed);
            out.push(Pusher {
                id,
                min: s.world_box.min,
                max: s.world_box.max,
                velocity: [wish[0], wish[1], 0.0],
                mass: s.mass,
            });
        } else if let Some(s) = torque_game::sim::vehicle::state(vm, id) {
            out.push(Pusher {
                id,
                min: s.world_box.min,
                max: s.world_box.max,
                velocity: s.rigid.lin_vel,
                mass: s.mass,
            });
        }
    }
    (out, players)
}

/// The hook's form for [`torque_engine::ServerTick`].
pub(crate) fn hook(
    props: ServerProps,
) -> impl Fn(&mut Vm, &Engine, TickInfo) -> Result<(), String> {
    move |vm, _, _| tick(vm, &props)
}
