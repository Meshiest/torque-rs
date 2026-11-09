//! The plugin: registers the four built-in RPCs, the server's script functions and per-tick offer, and the client's inbox.

use std::rc::Rc;

use torque_client_script_format::Value;
use torque_engine::rpc::{Allow, Delivery, MAX_RPC_BYTES, RealmInbox, RpcCx, RpcError};
use torque_engine::{ClientStage, EngineBuilder, Plugin, PluginId, ServerStage, ServerTick, VmMut};
use torque_net::handshake::ConnId;
use torque_server::net::rpc::ClientRpc;
use torque_ts_vm::vm::Vm;

use crate::inbox::ScriptInbox;
use crate::messages::{
    DOWN, DOWN_RATE, OFFER, STATUS, ScriptDown, ScriptOffer, ScriptStatus, ScriptUp, UP, UP_RATE,
};
use crate::server::{self, MessageHandler, ScriptServer};

/// The bytes an up message's frame may take: its 4 KiB of value and the frame's header.
const UP_FRAME: usize = 4_200;

/// The connection the client is on now (0 without RPCs).
fn epoch_of(vm: &Vm) -> u64 {
    vm.extension::<ClientRpc>().map_or(0, |rpc| rpc.epoch())
}

/// Server scripts for a game (`torque.client-scripts`): the built-in RPCs [`UP`](crate::messages::UP), [`DOWN`](crate::messages::DOWN), [`STATUS`](crate::messages::STATUS) and [`OFFER`](crate::messages::OFFER), the
/// server's `clientScriptSend`, `clientScriptOffer`, `clientScriptOnMessage` and `clientScriptOnStatus`, and each client's
/// inbox. Add it to a game's server and client engines; a client then drives [`Realms`](crate::Realms). It is in no default plugin
/// set, and without a build that grants a tier nothing here lets a server's Lua run.
///
/// ```
/// use torque_engine::EngineBuilder;
/// use torque_plugin_client_script::ClientScriptsPlugin;
///
/// let engine = EngineBuilder::new()
///     .add_plugins(ClientScriptsPlugin::new().on_message(|_vm, _conn, value| println!("{value:?}")))
///     .build()?;
/// assert_eq!(engine.net_rpcs()?.len(), 4);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(PluginId, Default)]
#[plugin(id = "torque.client-scripts")]
pub struct ClientScriptsPlugin {
    handlers: Vec<MessageHandler>,
}

impl ClientScriptsPlugin {
    /// The plugin with no Rust handler of players' messages.
    pub fn new() -> ClientScriptsPlugin {
        ClientScriptsPlugin::default()
    }

    /// Hands every message a player's realm sends to `handler`, with the connection it came from. The value is the player's
    /// own, decoded and within its caps but otherwise untrusted: check it as any client's input.
    #[must_use]
    pub fn on_message(
        mut self,
        handler: impl Fn(&mut Vm, ConnId, &Value) + 'static,
    ) -> ClientScriptsPlugin {
        self.handlers.push(Rc::new(handler));
        self
    }
}

impl Plugin for ClientScriptsPlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        let state = Rc::new(ScriptServer::default());
        for handler in &self.handlers {
            state.add_handler(Rc::clone(handler));
        }
        let up = Rc::clone(&state);
        engine
            .net()
            .rpc::<ScriptUp, ()>(UP)
            .to_server()
            .delivery(Delivery::Reliable)
            .rate(UP_RATE.0, UP_RATE.1)
            .max_bytes(UP_FRAME)
            .allow(Allow::Anyone)
            .server(move |cx: &mut RpcCx, message: ScriptUp| {
                let conn = cx.caller().ok_or(RpcError::Refused)?;
                let tick = cx.tick();
                up.receive(cx.vm, conn, tick, &message);
                Ok(())
            });
        let status = Rc::clone(&state);
        engine
            .net()
            .rpc::<ScriptStatus, ()>(STATUS)
            .to_server()
            .rate(4, 8)
            .max_bytes(256)
            .allow(Allow::Anyone)
            .server(move |cx: &mut RpcCx, message: ScriptStatus| {
                let conn = cx.caller().ok_or(RpcError::Refused)?;
                status.status(cx.vm, conn, &message);
                Ok(())
            });
        engine
            .net()
            .rpc::<ScriptDown, ()>(DOWN)
            .to_client()
            .rate(DOWN_RATE.0, DOWN_RATE.1)
            .max_bytes(MAX_RPC_BYTES)
            .client(|cx: &mut RpcCx, message: ScriptDown| {
                if let Some(inbox) = cx.vm.extension::<ScriptInbox>() {
                    match message.value() {
                        Ok(value) => inbox.push(epoch_of(cx.vm), value),
                        Err(_) => inbox.refuse(),
                    }
                }
                Ok(())
            });
        engine
            .net()
            .rpc::<ScriptOffer, ()>(OFFER)
            .to_client()
            .rate(4, 8)
            .max_bytes(8 * 1024)
            .client(|cx: &mut RpcCx, message: ScriptOffer| {
                if let Some(inbox) = cx.vm.extension::<ScriptInbox>() {
                    match message.mods() {
                        Ok(mods) => inbox.offer(epoch_of(cx.vm), mods),
                        Err(_) => inbox.refuse(),
                    }
                }
                Ok(())
            });
        engine.server(ServerStage::Classes, move |vm, _| {
            vm.set_extension(Rc::clone(&state));
            server::register_natives(vm);
            Ok(())
        });
        engine.client(ClientStage::Classes, |vm, _| {
            vm.set_extension(Rc::new(ScriptInbox::default()));
            vm.set_extension(Rc::new(RealmInbox::default()));
            Ok(())
        });
        engine.push(ServerTick::after(|mut vm: VmMut| {
            server::offer_new_clients(&mut vm);
        }));
    }
}
