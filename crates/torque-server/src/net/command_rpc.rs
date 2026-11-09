//! Script commands in the engine's protocol: `commandToServer` and `commandToClient` as two built-in RPCs.
//!
//! The scripts' side is Torque's. `commandToServer('Name', ..)` still calls `serverCmdName(%client, ..)`, `commandToClient`
//! calls `clientCmdName(..)`, a tag argument is still expanded against the tags the sender named, an integer is still an
//! integer. Only the carrier differs: Torque's protocol puts a command and the NetString events of its new tags on the ordered
//! event queue, and the engine's protocol, which has no such queue, sends the same thing as one call of [`COMMAND_RPC`] (to
//! the server) or [`COMMAND_TO_CLIENT_RPC`] (to a client), whose single field is a [`CommandBody`] made by
//! [`encode_command`](torque_net::remote::body::encode_command). Each body names the texts of its tags, so a call that is
//! dropped at a limit leaves no later one with a slot that means nothing.
//!
//! **Limits.** An RPC cannot be rate-limited for each command, so the calls ride two layers. The RPC's own bucket
//! ([`COMMAND_RATE`]) is a net under every command: a call over it is dropped before its body is read and counted by the link
//! (`rpcDrops(client, "torque.command")`). Then the host holds each command to its [`CommandSpec`](torque_engine::command::CommandSpec)
//! ([`cmdlimit`](super::cmdlimit)): the one a plugin declared, or the default limits every other command shares (32 a second,
//! burst 64, [`MAX_COMMAND_ARGS`](torque_engine::command::MAX_COMMAND_ARGS) arguments of at most 255 bytes), counted by
//! `commandDrops(client, name)`. A command is a client's input to the server, so a body that breaks a cap of
//! [`CommandBody::decode`](torque_net::remote::body::CommandBody::decode) drops the client as a malformed packet does.
//!
//! **Where a command runs.** A call's handler only keeps the body ([`CommandInbox`]); the host takes the bodies of a connection
//! right after the link's work ([`ServerRpc::pump`](super::rpc::ServerRpc::pump), the client's pump) and runs them as it runs a
//! packet's commands: the same gates (`NetGame::command_allowed`, a held connection, the plugin command's permission), the same
//! script window and share, the same `serverCmd` lookup. `onRpcCheck` is not asked: it is for the game's RPCs, and a command
//! has the game's own gates.
//!
//! **Before the accept.** A server's calls ride the connection's RPC link, which exists only for a connection the host
//! accepted, and a client's `commandToServer` sends nothing until its connection is accepted, so no command crosses before.

use std::cell::RefCell;
use std::collections::HashMap;

use torque_engine::rpc::{Allow, Delivery, MAX_RPC_BYTES, RpcCx, RpcError};
use torque_engine::{EngineBuilder, NetMessage};
use torque_net::handshake::ConnId;
use torque_net::remote::body::MAX_BODY_BYTES;
use torque_ts_vm::vm::Vm;

/// The RPC a client's commands are sent as.
pub const COMMAND_RPC: &str = "torque.command";
/// The RPC a server's commands are sent as.
pub const COMMAND_TO_CLIENT_RPC: &str = "torque.command.client";
/// The calls a second and the burst of each of the two RPCs' buckets, per connection: far above what any command is declared to
/// take, so the bucket of a command is the one that limits it, and it only bounds what a body costs to read.
pub const COMMAND_RATE: (u32, u32) = (1_024, 1_024);

/// The one field of the two RPCs: a command's body ([`CommandBody`](torque_net::remote::body::CommandBody)).
#[derive(Debug, Clone, PartialEq, Default, NetMessage)]
#[net(crate = torque_engine)]
pub struct CommandMessage {
    /// The body, at most [`MAX_BODY_BYTES`].
    #[net(max_len = 1000)]
    pub body: Vec<u8>,
}

const _: () = assert!(
    MAX_BODY_BYTES == 1_000,
    "the field's max_len is the body's cap"
);

/// The bodies of the commands a link's handlers received, until the host takes them. An extension of the script engine of the
/// side that receives (made by the first handler that needs it).
#[derive(Default)]
pub struct CommandInbox {
    bodies: RefCell<HashMap<Option<ConnId>, Vec<Vec<u8>>>>,
}

impl CommandInbox {
    /// Keeps `body`, received from `from` (`None` on a client).
    pub fn push(&self, from: Option<ConnId>, body: Vec<u8>) {
        self.bodies.borrow_mut().entry(from).or_default().push(body);
    }

    /// Takes the bodies received from `from`, oldest first.
    pub fn take(&self, from: Option<ConnId>) -> Vec<Vec<u8>> {
        self.bodies.borrow_mut().remove(&from).unwrap_or_default()
    }
}

fn keep(cx: &mut RpcCx<'_>, message: CommandMessage) -> Result<(), RpcError> {
    let from = cx.caller();
    let inbox = cx.vm.extension::<CommandInbox>().unwrap_or_else(|| {
        let inbox = std::rc::Rc::new(CommandInbox::default());
        cx.vm.set_extension(std::rc::Rc::clone(&inbox));
        inbox
    });
    inbox.push(from, message.body);
    Ok(())
}

/// The bodies of the commands `vm`'s side received from `from`, oldest first.
pub fn take_bodies(vm: &Vm, from: Option<ConnId>) -> Vec<Vec<u8>> {
    vm.extension::<CommandInbox>()
        .map_or_else(Vec::new, |inbox| inbox.take(from))
}

/// Registers the two RPCs. The engine's protocol does it (a game in Torque's protocol has neither: they would be in its manifest).
pub fn register(engine: &mut EngineBuilder) {
    engine
        .net()
        .rpc::<CommandMessage, ()>(COMMAND_RPC)
        .to_server()
        .delivery(Delivery::Reliable)
        .rate(COMMAND_RATE.0, COMMAND_RATE.1)
        .max_bytes(MAX_RPC_BYTES)
        .outside_connection_rate()
        .allow(Allow::Anyone)
        .server(keep);
    engine
        .net()
        .rpc::<CommandMessage, ()>(COMMAND_TO_CLIENT_RPC)
        .to_client()
        .delivery(Delivery::Reliable)
        .rate(COMMAND_RATE.0, COMMAND_RATE.1)
        .max_bytes(MAX_RPC_BYTES)
        .outside_connection_rate()
        .client(keep);
}
