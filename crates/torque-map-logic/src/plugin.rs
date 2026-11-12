//! The plugin: the mover ghost class and the server's script functions over a map's logic.
//!
//! [`MapLogicPlugin`] (`torque.map-logic`) adds the ghost class `MapMover` ([`MoverGhost`], interpolated) and, on every server
//! script engine, the console functions below. A host loads a map's logic and puts it on the server's script engine with
//! [`install`]; the functions do nothing (and return 0 or "") until it has.
//!
//! | Function | Does |
//! |---|---|
//! | `mapLogicTick([ticks])` | runs ticks (1 when left out, at most 1000 a call) with the player boxes set below |
//! | `mapLogicFire(target, input [, parameter [, delay]])` | posts an input as `ent_fire` does; 1 when the queue took it |
//! | `mapLogicUse(name)` | uses the entities named `name` (a button's press) as the first player |
//! | `mapLogicPlayer(name, min, max)` | adds the player `name` (once) and sets its box (`"x y z"` each, the map's units) |
//! | `mapLogicFraction(name)` | where the first mover named `name` is, 0 closed to 1 open (-1 when there is none) |
//! | `mapLogicCount(what)` | `entities`, `events`, `inputs`, `queued`, `missing`, `dropped`, `deferred`, `unsupported` |
//!
//! After each `mapLogicTick` the outputs that fired are handed to the script one by one as
//! `MapLogic::onOutput(%entityName, %output, %activatorName)` when the script defines it (a game's hook on the map's logic). A
//! `mapLogicTick` called from such a callback is refused (an error line, 0 ticks); the other functions work there.
//!
//! There is no server tick a plugin can join yet, so the game calls `mapLogicTick` once per server tick (or the host calls
//! [`MapLogic::tick`] itself).

use std::cell::RefCell;
use std::rc::Rc;

use torque_engine::{EngineBuilder, Plugin, PluginId, Replicate, ServerStage};
use torque_ts_vm::vm::{Native, NativeCall, Vm};

use crate::bus::{EntityId, MapLogic, Toucher, ToucherKind};
use crate::mover::MoverGhost;

/// The most ticks one `mapLogicTick` runs.
pub const MAX_TICKS_PER_CALL: u64 = 1000;

/// A map's logic as the server's script engine holds it, with the players' boxes.
#[derive(Debug, Default)]
pub struct SharedLogic {
    pub logic: Option<MapLogic>,
    pub players: Vec<(EntityId, Toucher)>,
    /// Set while `mapLogicTick` runs (its callbacks included): a nested call is refused.
    pub ticking: bool,
}

/// Puts `logic` on `vm` for the console functions (replacing any before it); the shared handle is returned for the host.
pub fn install(vm: &mut Vm, logic: MapLogic) -> Rc<RefCell<SharedLogic>> {
    let shared = Rc::new(RefCell::new(SharedLogic {
        logic: Some(logic),
        players: Vec::new(),
        ticking: false,
    }));
    vm.set_extension(Rc::new(Handle(shared.clone())));
    shared
}

/// The handle the console functions find on the script engine.
struct Handle(Rc<RefCell<SharedLogic>>);

fn shared(vm: &Vm) -> Option<Rc<RefCell<SharedLogic>>> {
    vm.extension::<Handle>().map(|h| h.0.clone())
}

/// Argument `i` as text (invalid UTF-8 replaced).
fn arg(c: &NativeCall, i: usize) -> String {
    String::from_utf8_lossy(c.arg(i)).into_owned()
}

/// The map's logic plugin ([module docs](self)).
#[derive(Clone, Copy, Debug, Default, PluginId)]
#[plugin(id = "torque.map-logic")]
pub struct MapLogicPlugin;

impl Plugin for MapLogicPlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        engine
            .net()
            .ghost::<MoverGhost>("MapMover", Replicate::Interpolated);
        engine.server(ServerStage::Game, |vm, _| {
            register(vm);
            Ok(())
        });
    }
}

/// Registers the console functions on `vm` (the plugin does this on every server script engine).
pub fn register(vm: &mut Vm) {
    vm.register(
        "mapLogicTick",
        1,
        2,
        "([ticks]) runs the map's logic for ticks (1 when left out)",
        |vm, c| {
            let Some(s) = shared(vm) else {
                return Native::Int(0);
            };
            let n = if c.argc() > 1 {
                crate::value::atoi(&arg(c, 1)).clamp(0, MAX_TICKS_PER_CALL as i64) as u64
            } else {
                1
            };
            {
                let mut g = s.borrow_mut();
                if g.ticking {
                    drop(g);
                    vm.error(
                        "mapLogicTick: refused inside a running mapLogicTick (an onOutput callback)"
                            .into(),
                    );
                    return Native::Int(0);
                }
                g.ticking = true;
            }
            let fired = {
                let mut g = s.borrow_mut();
                let touchers: Vec<Toucher> = g.players.iter().map(|(_, t)| *t).collect();
                let Some(ml) = g.logic.as_mut() else {
                    g.ticking = false;
                    return Native::Int(0);
                };
                ml.observe_outputs(true);
                for _ in 0..n {
                    ml.tick(&touchers);
                }
                let name = |ml: &MapLogic, id: Option<EntityId>| {
                    id.and_then(|i| ml.entity(i))
                        .map(|e| e.name.clone().into_bytes())
                        .unwrap_or_default()
                };
                ml.take_fired()
                    .into_iter()
                    .map(|f| {
                        vec![
                            name(ml, Some(f.entity)),
                            f.output.into_bytes(),
                            name(ml, f.activator),
                        ]
                    })
                    .collect::<Vec<_>>()
            };
            for args in fired {
                if vm.invoke(Some(b"MapLogic"), b"onOutput", &args).is_err() {
                    break;
                }
            }
            s.borrow_mut().ticking = false;
            Native::Int(n as i32)
        },
    );
    vm.register(
        "mapLogicFire",
        3,
        5,
        "(target, input [, parameter [, delay]]) posts an input to the map's entities",
        |vm, c| {
            let Some(s) = shared(vm) else {
                return Native::Bool(false);
            };
            let mut g = s.borrow_mut();
            let Some(ml) = g.logic.as_mut() else {
                return Native::Bool(false);
            };
            let delay = crate::value::atof32(&arg(c, 4));
            let activator = ml.first_player();
            Native::Bool(ml.fire_input(&arg(c, 1), &arg(c, 2), &arg(c, 3), delay, activator))
        },
    );
    vm.register(
        "mapLogicUse",
        2,
        2,
        "(name) uses the entities of that name as the first player",
        |vm, c| {
            let Some(s) = shared(vm) else {
                return Native::Int(0);
            };
            let mut g = s.borrow_mut();
            let Some(ml) = g.logic.as_mut() else {
                return Native::Int(0);
            };
            let ids = ml.find(&arg(c, 1));
            let player = ml.first_player();
            for &id in &ids {
                ml.use_entity(id, player);
            }
            Native::Int(ids.len() as i32)
        },
    );
    vm.register(
        "mapLogicPlayer",
        4,
        4,
        "(name, min, max) adds a player once and sets its box",
        |vm, c| {
            let Some(s) = shared(vm) else {
                return Native::Int(-1);
            };
            let mut g = s.borrow_mut();
            let name = arg(c, 1);
            let min = crate::value::vec3(&arg(c, 2)).unwrap_or([0.0; 3]);
            let max = crate::value::vec3(&arg(c, 3)).unwrap_or([0.0; 3]);
            let SharedLogic { logic, players, .. } = &mut *g;
            let Some(ml) = logic.as_mut() else {
                return Native::Int(-1);
            };
            let found = players.iter().position(|(id, _)| {
                ml.entity(*id)
                    .is_some_and(|e| e.name.eq_ignore_ascii_case(&name))
            });
            let i = match found {
                Some(i) => i,
                None => {
                    if players.len() >= 256 {
                        return Native::Int(-1);
                    }
                    let id = ml.add_player(&name);
                    players.push((
                        id,
                        Toucher {
                            id,
                            kind: ToucherKind::Player,
                            min,
                            max,
                        },
                    ));
                    players.len() - 1
                }
            };
            players[i].1.min = min;
            players[i].1.max = max;
            Native::Int(players[i].0.0 as i32)
        },
    );
    vm.register(
        "mapLogicFraction",
        2,
        2,
        "(name) where the first mover of that name is, 0 to 1",
        |vm, c| {
            let Some(s) = shared(vm) else {
                return Native::Float(-1.0);
            };
            let g = s.borrow();
            let Some(ml) = g.logic.as_ref() else {
                return Native::Float(-1.0);
            };
            let f = ml
                .find(&arg(c, 1))
                .into_iter()
                .find_map(|id| ml.kind(id).and_then(|k| k.as_mover()).map(|m| m.frac));
            Native::Float(f.map_or(-1.0, f64::from))
        },
    );
    vm.register(
        "mapLogicCount",
        2,
        2,
        "(what) a count of the map's logic",
        |vm, c| {
            let Some(s) = shared(vm) else {
                return Native::Int(0);
            };
            let g = s.borrow();
            let Some(ml) = g.logic.as_ref() else {
                return Native::Int(0);
            };
            let st = ml.stats;
            let v: u64 = match arg(c, 1).to_ascii_lowercase().as_str() {
                "entities" => ml.entities().filter(|(_, e)| e.alive).count() as u64,
                "events" => st.events,
                "inputs" => st.inputs,
                "queued" => ml.queued() as u64,
                "missing" => st.missing_targets,
                "dropped" => st.dropped_depth + st.dropped_queue,
                "deferred" => st.deferred,
                "unsupported" => ml.unsupported_classes().values().sum::<usize>() as u64,
                _ => 0,
            };
            Native::Int(v.min(i32::MAX as u64) as i32)
        },
    );
}
