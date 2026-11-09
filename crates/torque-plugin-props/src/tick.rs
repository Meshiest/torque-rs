//! The server's tick hook, after the process list: deleted props and owners tended, the map's statics looked for, the players'
//! and vehicles' pushes gathered, the world stepped ([`crate::world`]) and what changed written back and called back.

use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use torque_collide::types::STATIC_SHAPE;
use torque_engine::{Engine, TickInfo, VmPrefs};
use torque_ts_vm::geometry;
use torque_ts_vm::vm::Vm;

use crate::natives::control_of;
use crate::quant::conjugate;
use crate::server::{
    CLASS, PropPrefs, ServerProps, address_of, is_class, limits_of, owner_key, player_boxes,
    write_back,
};
use crate::world::{Owner, OwnerKey, PropEvent, PropWorld, Pusher, StaticPiece};

/// Ticks between two looks for the map pieces and static shapes props land on.
pub const STATICS_EVERY: u64 = 64;

/// The server's tick hook ([module docs](self)).
pub fn tick(vm: &mut Vm, props: &ServerProps) -> Result<(), String> {
    let prefs = vm.prefs::<PropPrefs>();
    let (events, expired, rebuilt) = {
        let mut srv = props.0.borrow_mut();
        srv.tick += 1;
        srv.net_share = prefs.net_share;
        let tick = srv.tick;
        let world = srv.world_mut(vm);
        world.limits = limits_of(&prefs);
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
        if tick == 1 || tick.is_multiple_of(STATICS_EVERY) {
            world.sync_statics(statics_of(vm));
        }
        let pushers = pushers_of(vm);
        let players: Vec<[f32; 3]> = player_boxes(vm)
            .iter()
            .map(|(_, a, b)| std::array::from_fn(|k| (a[k] + b[k]) * 0.5))
            .collect();
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
        let events: Vec<(u32, u32, PropEvent)> = report
            .events
            .iter()
            .filter_map(|&(id, e)| world.get(id).map(|p| (id, p.data, e)))
            .collect();
        (events, expired, report.rebuilt.then_some(report.lost))
    };
    if let Some(lost) = rebuilt {
        vm.warn(
            "props: the physics world stopped on a caught panic; it was made again from each prop's last state".to_string(),
        );
        for id in lost {
            vm.warn(format!(
                "props: prop {id} could not be made again in the new physics world; it is deleted"
            ));
            let _ = vm.delete_object(id);
        }
    }
    for id in expired {
        let _ = vm.delete_object(id);
    }
    refresh_controls(vm, props);
    for (id, data, event) in events {
        let name: &[u8] = match event {
            PropEvent::Sleep => b"onPropSleep",
            PropEvent::Wake => b"onPropWake",
        };
        vm.call_method(data, name, &[id.to_string().into_bytes()])
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Owners that left (their props become orphans), came back under the same key (they take their props back, unless two
/// connections there share that key: then nobody does), or have been gone longer than the orphan time (their props are taken
/// out of the world and returned, for their objects to be deleted).
fn tend_owners(vm: &mut Vm, world: &mut PropWorld, tick: u64) -> Vec<u32> {
    let mut owners: Vec<(u32, Owner)> = Vec::new();
    let mut expired = Vec::new();
    let mut keys: Option<BTreeMap<OwnerKey, Option<u32>>> = None;
    for (&id, p) in world.props() {
        let Some(o) = &p.owner else { continue };
        match o.conn {
            Some(c) if vm.objects().contains(c) => {}
            Some(_) => owners.push((
                id,
                Owner {
                    conn: None,
                    left: Some(tick),
                    ..o.clone()
                },
            )),
            None => {
                let keys = keys.get_or_insert_with(|| live_owner_keys(vm));
                if let Some(&Some(c)) = keys.get(&o.key) {
                    owners.push((
                        id,
                        Owner {
                            conn: Some(c),
                            left: None,
                            ..o.clone()
                        },
                    ));
                } else if o
                    .left
                    .is_some_and(|t| tick.saturating_sub(t) > world.limits.orphan_ticks)
                {
                    expired.push(id);
                }
            }
        }
    }
    for (id, mut owner) in owners {
        if let Some(c) = owner.conn {
            owner.address = Some(address_of(vm, c));
        }
        if let Some(mask) = world.set_owner(id, Some(owner)) {
            write_back(vm, world, id, mask);
        }
    }
    for &id in &expired {
        world.remove(id);
    }
    expired
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
        let m = view.obj_to_world;
        let (lo, hi) = (view.obj_box.min, view.obj_box.max);
        let center: [f32; 3] = std::array::from_fn(|k| (lo[k] + hi[k]) * 0.5 * view.scale[k]);
        let half: [f32; 3] = std::array::from_fn(|k| ((hi[k] - lo[k]) * 0.5 * view.scale[k]).abs());
        let position: [f32; 3] = std::array::from_fn(|r| {
            m[r * 4] * center[0]
                + m[r * 4 + 1] * center[1]
                + m[r * 4 + 2] * center[2]
                + m[r * 4 + 3]
        });
        let rows: geometry::Mat4 = std::array::from_fn(|r| std::array::from_fn(|c| m[r * 4 + c]));
        let rotation = conjugate(geometry::quat_from_matrix(&rows));
        if rotation.iter().any(|v| !v.is_finite()) {
            continue;
        }
        out.push((
            id,
            StaticPiece::Box {
                position,
                rotation,
                half,
            },
        ));
    }
    out
}

/// The players and vehicles as pushers: a player pushes at the speed its move asks for (it stops at the prop it walks into), a
/// vehicle at its own.
fn pushers_of(vm: &Vm) -> Vec<Pusher> {
    let mut out = Vec::new();
    for (id, _) in vm.objects().iter() {
        if let Some(s) = torque_game::sim::player::state(vm, id) {
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
    out
}

/// The hook's form for [`torque_engine::ServerTick`].
pub(crate) fn hook(
    props: ServerProps,
) -> impl Fn(&mut Vm, &Engine, TickInfo) -> Result<(), String> {
    move |vm, _, _| tick(vm, &props)
}
