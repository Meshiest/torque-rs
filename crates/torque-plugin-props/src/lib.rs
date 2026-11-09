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
//!   the props it was sent ([`client::PropBook`]), drawn as shapes;
//! - joints ([`joints`]): tethers (hard or soft), winches, welds, hinges and grapples between props, the world and players, in the
//!   same physics world, made with the server's `joint*` functions (no client entry point), the `Joint` ghost class
//!   ([`JointGhost`]) and each client's book of them ([`JointBook`]); a grapple pulls its player inside the Player's own tick
//!   ([`torque_game::player_tether`]), so a client's prediction replays it exactly; with feature `rope`, networked ropes that
//!   follow joints ([`joints::rope`]).
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
//! what a client does with it. [`server`]: the state, spawning, the collision source and the update priority. [`joints`]: the
//! joint kinds and caps (`mod.rs`), their rules in the world (`world.rs`, an `impl PropWorld`), the `Joint` class and the server's
//! tethers (`server.rs`), the console functions (`natives.rs`), the record (`records.rs`), a client's book and tethers
//! (`client.rs`), and the ropes (`rope.rs`, feature `rope`).
//!
//! **One tick.** During the process list each grappled Player's own step asks the server's tether source
//! ([`joints::server::ServerTethers`]) for its tether and reports the pull back. After the process list, the hook drops props and
//! joints whose objects went, tends owners and orphans, looks for the map's statics every 64 ticks, tells the world where the
//! grappled players are, gathers the players' and vehicles' pushes, and calls [`PropWorld::tick`]: a stopped world is made again
//! (its joints too, in id order), pushes become impulses, grapples whose players went are taken out, each grapple's pull goes to
//! the prop at its far end and each winch reels, the props a joint woke are reported, the world steps (not when nothing moves)
//! under the step budget, each awake prop is quantized and written back, sleepers and the awake cap are applied, and each joint's
//! distance, force, limit and break are read. The hook then writes the record fields of each dirty prop and joint, marks their
//! ghost groups, deletes the props that went and the joints (and ropes) that went with them, and calls `onPropSleep`,
//! `onPropWake` and `onBreak`.
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
//! - This crate has no client entry point: `a_client_sending_the_old_spawn_command_gets_nothing`, and for joints
//!   `the_natives_refuse_bad_ends_and_hold_values_to_their_ranges` (its client commands do nothing).
//! - A joint goes with its prop (or a grapple with its player), stays through a freeze, and a world made again takes every joint
//!   again in id order: `a_joint_goes_with_its_prop_and_stays_through_a_freeze_and_a_new_world`,
//!   `a_grapple_tethers_its_player_and_pulls_its_prop_back`,
//!   `a_joint_reaches_every_client_with_its_ends_as_their_own_ghosts_and_goes_with_its_prop`.
//! - Joint caps per prop, per owner, per address and per server hold whoever asks, and an owner's joints follow its props'
//!   orphan rules: `the_caps_refuse_the_17th_on_a_prop_the_65th_for_a_player_one_past_an_address_and_the_1025th_for_the_server`,
//!   `reconnecting_from_one_address_does_not_start_a_new_joint_quota`,
//!   `a_leaving_owners_joints_are_orphans_and_expire_with_its_props`,
//!   `hostile_joint_calls_never_panic_the_server_and_the_caps_hold`.
//! - `jointCheck` reads a player at an end where the server has it and refuses one that is not the asking client's own control
//!   object, or a prop someone else owns, unless the game allows it, and reach-checks a hinge's point:
//!   `a_player_may_only_grapple_itself_and_only_where_the_server_has_it`,
//!   `a_client_may_join_only_its_own_props_or_no_ones_and_a_hinge_its_point_in_reach`.
//! - A joint's script values are held to their ranges (lengths, rates, forces, springs, limits, motors), and its points and
//!   offsets refused when they are not finite or lie past the world's bound, before the physics world sees them (which checks
//!   them again); every value a client decodes is held to its range:
//!   `hostile_joint_values_are_held_and_never_reach_the_physics_world_unchecked`, `ends_that_are_not_two_things_are_refused`,
//!   `hostile_values_are_held_to_their_ranges_on_decode`, `a_hostile_servers_joint_records_never_panic_the_client`.
//! - A player has one grapple, and the server's tether, its pull and a client's prediction find the same one (a client whose
//!   book holds two gives none): `a_player_has_one_grapple_and_an_offset_past_the_world_is_refused`,
//!   `a_clients_book_with_two_grapples_on_one_player_gives_it_no_tether`.
//! - A grapple's tether is the server's bit for bit on the predicting client (its point, length and spring travel as whole
//!   floats, a prop end is read from the quantized state with [`joints::point_on`] on both sides):
//!   `a_grapple_gives_the_clients_prediction_the_servers_tether_exactly` and the `props-joints` line of `just trace-hash` (0
//!   corrections). A Player with no tether runs Torque's step (the host's tether is none by default, and the step then skips
//!   it): every other line of `just trace-hash` unchanged. A tether that is not finite, or on a mounted player, changes
//!   nothing: torque-sim's `a_tether_that_is_not_finite_or_a_mounted_player_changes_nothing`.
//! - A callback (`onBreak`, `onPropSleep`, `onPropWake`) that stops on an error is printed, and the ones after it still run:
//!   `a_callback_that_stops_on_an_error_is_printed_and_the_ones_after_it_still_run`. A physics call that fails where no caller
//!   is there to tell is printed by the tick ([`world::PropWorld::take_problems`]); a freeze the world refuses changes nothing:
//!   `freezing_in_a_stopped_world_changes_nothing_and_the_new_world_keeps_the_joints_whole`.
//! - A joint's break force is compared with the force it holds at the end of a tick (Rapier keeps only the last solver
//!   iteration's impulses), so a knock within one tick does not break it:
//!   `a_joint_pulled_past_its_break_force_breaks_and_holds_nothing`. A weld gives about 2 mm in its first loaded tick, then holds
//!   with no creep: `a_weld_drifts_at_most_a_millimeter_over_600_ticks_under_load`.
//! - A sleeping prop a joint changes wakes (a sleeper otherwise settles back where it lay, and a winch would never lift it):
//!   `a_winch_reaches_its_target_within_two_percent_of_the_planned_time`, `a_hinge_holds_its_limits_within_half_a_degree`.
//! - A rope that shows a joint follows it and pulls on nothing:
//!   `a_joint_rope_at_its_limit_lies_straight_within_a_millimeter_and_a_slack_one_hangs` (feature `rope`); another plugin's
//!   driver under the joints' name is warned about: `another_plugins_driver_under_the_joints_name_is_warned_about`.
//!
//! **Extension points.** `engine.net().ghost_share` (the props' share of each packet and their [`server::priority`]); the
//! collision sources ([`server::ServerSource`], [`client::ClientSource`]) other movers meet props through;
//! [`ServerProps::check_placement`] (`propCheckPlacement`) and `jointCheck`, which a game's own rate-limited, permission-checked
//! server commands call before `propSpawn` and the `joint*` functions; the tether sources ([`torque_game::player_tether`]) a
//! grapple is given to the Player through; and [`JointBook::connected`], the seam a predictor pulls a prop's jointed props in
//! through (joints interpolate until one does).
//!
//! **Checks.** `cargo test -p torque-plugin-props` and `cargo test -p torque-plugin-props --features rope --test joint_rope`;
//! the fuzzers with `TORQUE_FUZZ_PROPS=100000`; `just trace-hash` (the `props-drop` and `props-joints` lines must be equal native
//! and wasm32, the other lines unchanged); `cargo bench -p torque-plugin-props --bench step` under the bench lock.

pub mod client;
pub mod data;
pub mod joints;
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
pub use joints::client::{ClientJoints, JointBook};
pub use joints::records::JointGhost;
pub use records::PropGhost;
pub use server::{PropPrefs, ServerProps};
pub use world::{PropLimits, PropWorld};

/// The network version of the props plugin: a client and a server must run the same.
pub const NET_VERSION: u32 = 2;

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
        let joints = ClientJoints::default();
        engine.insert_resource(server.clone());
        engine.insert_resource(book.clone());
        engine.insert_resource(joints.clone());
        engine
            .net()
            .ghost::<JointGhost>(joints::GHOST_CLASS, Replicate::Interpolated);
        let heard_joints = joints.clone();
        engine
            .net()
            .on_ghost::<JointGhost>(joints::GHOST_CLASS, move |_, event| {
                joints::client::heard(&heard_joints, event)
            });
        // The grapples' tethers on players: the server's step asks the world, a client's prediction its books.
        let tethers = server.clone();
        engine.server(ServerStage::Classes, move |vm, _| {
            torque_game::player_tether::add(vm, joints::server::tethers(&tethers));
            Ok(())
        });
        let client_tethers = joints::client::ClientTethers {
            joints: joints.clone(),
            props: book.clone(),
        };
        engine.client(ClientStage::Classes, move |vm, _| {
            torque_game::player_tether::add(vm, Rc::new(client_tethers.clone()));
            Ok(())
        });
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
        #[cfg(feature = "rope")]
        joints::rope::register(engine, &joints);
        let (fresh, fresh_joints) = (book, joints);
        engine.client(ClientStage::Classes, move |_, _| {
            fresh.0.borrow_mut().clear();
            fresh_joints.0.borrow_mut().clear();
            Ok(())
        });
        engine
            .server_script()
            .class::<PropData>()
            .class::<server::NetProp>()
            .class::<joints::server::NetJoint>()
            .natives(natives::declared())
            .natives(joints::natives::declared())
            // Its prefs are the props' and joints' limits.
            .with_effects(torque_engine::Effects::ENGINE, |r| {
                r.prefs::<PropPrefs>();
            });
        engine
            .client_script()
            .class::<PropData>()
            .class::<server::NetProp>()
            .class::<joints::server::NetJoint>();
        engine.push(ServerTick::after(tick::hook(server)));
    }
}
// ANCHOR_END: build
