//! Props for torque-rs: crates, barrels and rubble a server simulates as rigid bodies and every client sees.
//!
//! [`PropsPlugin`] adds
//!
//! - the datablock class `PropData` ([`PropData`]): what a kind of prop collides as, its mass, friction and bounce, when it sleeps,
//!   whether it may be frozen, the shape it is drawn as;
//! - the console class `Prop` ([`server::NetProp`]) and the server's console functions `propSpawn`, `propDelete`, `propFreeze`,
//!   `propUnfreeze`, `propApplyImpulse`, `propSetVelocity`, the getters, and `propCheckPlacement` ([`natives`]): a server API
//!   only. Props have no client entry point: a game's own server commands decide who may spawn what, checking a placement
//!   with [`ServerProps::check_placement`] first;
//! - one persistent Rapier world on the server ([`PropWorld`], on [`torque_physics_rapier`]) stepped after each tick's objects,
//!   its state quantized to the wire's precision every tick ([`quant`]), so a client is sent the server's state exactly;
//! - a collision source on both sides: players, items and projectiles hit props as static boxes, and a player or vehicle that
//!   walks into one pushes it;
//! - the `Prop` ghost class ([`PropGhost`]), its updates ordered and held to a share of each packet, and each client's book of
//!   the props it was sent ([`client::PropBook`]), drawn as shapes.
//!
//! It is not in `DefaultPlugins`: a game adds it. Props are engine-level: any game with an interactive world can use them.
//!
//! ```
//! use torque_engine::EngineBuilder;
//! use torque_plugin_props::PropsPlugin;
//!
//! let engine = EngineBuilder::new().add_plugins(PropsPlugin).build()?;
//! assert!(engine.is_plugin_added::<PropsPlugin>());
//! # Ok::<(), torque_engine::BuildError>(())
//! ```
//!
//! A server script then describes a kind of prop and spawns some:
//!
//! ```text
//! datablock PropData(Crate)
//! {
//!    shapeFile = "art/crate.dts";
//!    size = "1 1 1";       // the collision box
//!    mass = 20;
//!    friction = 0.7;
//! };
//!
//! %crate = propSpawn(Crate, "0 0 4");
//! propApplyImpulse(%crate, "0 40 0");
//! ```
//!
//! # Limits
//!
//! Props per player (`$Pref::Props::MaxPerPlayer`, 64), per server (`$Pref::Props::MaxProps`, 1,024) and awake at once
//! (`$Pref::Props::MaxAwake`, 256), and per remote address of the owners (`$Pref::Props::MaxPerAddress`, 256), whoever asked
//! for the spawn. A spawn past a cap is refused with a console error; past the awake cap the props farthest from every player
//! sleep first. The step is charged to a budget
//! (`$Pref::Props::StepBudget`): a step past it does not run, and the awake cap is lowered until the step fits again.
//!
//! # Network
//!
//! A server and a client must both have the plugin (the handshake refuses a client without it before any ghost). A TGE-protocol
//! peer is never sent a prop. A prop's updates take at most `$Pref::Props::NetShare` (40%) of each packet, the ones that matter most
//! to that client first ([`server::priority`]); asleep and frozen props cost nothing after their last update.
//!
//! # Maintaining
//!
//! **Modules.** [`natives`]: the server's console functions and the placement checks. [`tick`]: the server's tick hook (owners,
//! statics, pushes, the step, the writeback). [`world`]: the physics world and every rule about props, with no script engine.
//! [`quant`]: the wire's precision. [`records`], [`data`], [`client`]: what goes on the net (the ghost and datablock records) and
//! what a client does with it. [`server`]: the state, spawning, the collision source and the update priority.
//!
//! **One tick.** After the process list, the hook drops props whose objects went, tends owners and orphans, looks for the map's
//! statics every 64 ticks, gathers the players' and vehicles' pushes, and calls [`PropWorld::tick`]: a stopped world is made
//! again, pushes become impulses, the world steps (not when nothing moves) under the step budget, each awake prop is quantized
//! and written back, sleepers and the awake cap are applied. The hook then writes the record fields of each dirty prop, marks its
//! ghost groups, deletes the props that went, and calls `onPropSleep` and `onPropWake`.
//!
//! **Invariants**, each with the test that guards it:
//! - The state written back is the wire's, so a client restores the server's bits and native equals wasm32:
//!   `every_awake_state_is_written_back_at_the_wires_precision`, `a_quantized_state_quantizes_back_to_the_same_state`, and the
//!   `props-drop` line of `just trace-hash`.
//! - No script text is built from what a player controls (objects are made with `Vm::new_object`, callbacks called natively):
//!   `no_script_text_is_built_from_what_a_player_controls`.
//! - Caps per owner, per address and per server hold whoever asks: `the_caps_refuse_the_65th_for_a_player_and_the_1025th_for_the_server`,
//!   `the_players_of_one_address_share_its_cap_whatever_their_keys`, `reconnecting_from_one_address_does_not_start_a_new_quota`,
//!   `an_owner_with_no_readable_address_is_capped_under_its_connection`.
//! - A share group's priority stays below a kill's: `a_pending_kill_goes_before_a_share_update_of_any_priority` (torque-net).
//! - A poisoned world is made again: `a_stopped_world_is_made_again_from_the_props_states`,
//!   `a_prop_lost_when_the_physics_world_is_made_again_is_deleted_everywhere`.
//! - This crate has no client entry point: `a_client_sending_the_old_spawn_command_gets_nothing`.
//!
//! **Extension points.** `engine.net().ghost_share` (the props' share of each packet and their [`server::priority`]); the
//! collision sources ([`server::ServerSource`], [`client::ClientSource`]) other movers meet props through; and
//! [`ServerProps::check_placement`] (`propCheckPlacement`), which a game's own rate-limited, permission-checked server command
//! calls before `propSpawn`.
//!
//! **Checks.** `cargo test -p torque-plugin-props`; the fuzzers with `TORQUE_FUZZ_PROPS=100000`; `just trace-hash` (the
//! `props-drop` line must be equal native and wasm32, the other lines unchanged); `cargo bench -p torque-plugin-props --bench
//! step` under the bench lock.

pub mod client;
pub mod data;
pub mod natives;
pub mod quant;
pub mod records;
pub mod server;
pub mod tick;
pub mod world;

use std::rc::Rc;

use torque_collide::source::PluginCollision;
use torque_engine::{
    ClientStage, EngineBuilder, Plugin, PluginId, Replicate, ServerStage, ServerTick,
};

pub use client::{ClientProps, PropBook};
pub use data::{PropData, PropDataRecord, PropParams};
pub use records::PropGhost;
pub use server::{PropPrefs, ServerProps};
pub use world::{PropLimits, PropWorld};

/// The network version of the props plugin: a client and a server must run the same.
pub const NET_VERSION: u32 = 1;

/// The name of the ghost class a prop is.
pub const GHOST_CLASS: &str = "Prop";

/// The name of the props' collision source: their type bit is `$TypeMasks::PropObjectType`.
pub const COLLISION_NAME: &str = "Prop";

/// Adds props to the engine's servers and clients ([crate docs](crate)). Not in `DefaultPlugins`.
#[derive(Clone, Copy, Debug, Default, PluginId)]
#[plugin(id = "torque.props")]
pub struct PropsPlugin;

// ANCHOR: build
impl Plugin for PropsPlugin {
    fn net_version(&self) -> Option<u32> {
        Some(NET_VERSION)
    }

    fn build(&self, engine: &mut EngineBuilder) {
        let server = ServerProps::default();
        let book = ClientProps::default();
        engine.insert_resource(server.clone());
        engine.insert_resource(book.clone());
        engine
            .net()
            .ghost::<PropGhost>(GHOST_CLASS, Replicate::Interpolated)
            .datablock::<PropDataRecord>(<PropData as torque_engine::ScriptClass>::NAME);
        let (share, order) = (server.clone(), server.clone());
        engine.net().ghost_share(
            GHOST_CLASS,
            move |_| {
                share
                    .with(|s| s.net_share)
                    .unwrap_or(server::DEFAULT_NET_SHARE)
            },
            move |vm, obj, scope, skip| server::priority(&order, vm, obj, scope, skip),
        );
        engine
            .net()
            .drawn_shape::<PropGhost>(GHOST_CLASS, client::draw);
        let heard = book.clone();
        engine
            .net()
            .on_ghost::<PropGhost>(GHOST_CLASS, move |vm, event| {
                client::heard(vm, &heard, event)
            });
        engine.push(PluginCollision::server(
            COLLISION_NAME,
            Rc::new(server::ServerSource(server.clone())),
        ));
        engine.push(PluginCollision::client(
            COLLISION_NAME,
            Rc::new(client::ClientSource(book.clone())),
        ));
        // Each server starts its props over; each client its book.
        let fresh = server.clone();
        engine.server(ServerStage::Classes, move |_, _| {
            fresh.reset();
            Ok(())
        });
        let fresh = book;
        engine.client(ClientStage::Classes, move |_, _| {
            fresh.0.borrow_mut().clear();
            Ok(())
        });
        engine
            .server_script()
            .class::<PropData>()
            .class::<server::NetProp>()
            .prefs::<PropPrefs>()
            .natives(natives::declared());
        engine
            .client_script()
            .class::<PropData>()
            .class::<server::NetProp>();
        engine.push(ServerTick::after(tick::hook(server)));
    }
}
// ANCHOR_END: build
