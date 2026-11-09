//! Props for torque-rs: crates, barrels and rubble a server simulates as rigid bodies and every client sees.
//!
//! [`PropsPlugin`] adds
//!
//! - the datablock class `PropData` ([`PropData`]): what a kind of prop collides as, its mass, friction and bounce, when it sleeps,
//!   whether it may be frozen, the shape it is drawn as;
//! - the console class `Prop` ([`server::NetProp`]) and the server's console functions `propSpawn`, `propDelete`, `propFreeze`,
//!   `propUnfreeze`, `propApplyImpulse`, `propSetVelocity`, the getters, and the rate-limited command `serverCmdPropSpawn`
//!   ([`server`]);
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
//! (`$Pref::Props::MaxAwake`, 256). A spawn past the first two is refused with a console error (and, for a client's command, a
//! `clientCmdPropRefused`); past the awake cap the props farthest from every player sleep first. The step is charged to a budget
//! (`$Pref::Props::StepBudget`): a step past it does not run, and the awake cap is lowered until the step fits again.
//!
//! # Network
//!
//! A server and a client must both have the plugin (the handshake refuses a client without it before any ghost). A TGE-protocol
//! peer is never sent a prop. A prop's updates take at most `$Pref::Props::NetShare` (40%) of each packet, the ones that matter most
//! to that client first ([`server::priority`]); asleep and frozen props cost nothing after their last update.

#![expect(
    clippy::neg_cmp_op_on_partial_ord,
    reason = "a negated comparison is how a NaN takes a fixed side: `!(a < b)` is \"not below, or unordered\""
)]

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
        engine.server_command("PropSpawn", natives::spawn_spec(), natives::cmd_prop_spawn);
        engine.push(ServerTick::after(tick::hook(server)));
    }
}
// ANCHOR_END: build
