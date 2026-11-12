//! The remote-command natives of the network host:
//!
//! | native | args |
//! |---|---|
//! | `commandToClient(client, tag, ...)` | 3..22 |
//! | `commandToServer(tag, ...)` | 2..21; the tag test, then nothing: a server has no `ServerConnection` |
//! | `buildTaggedString(fmt, a1..a9)` | 2..11 |
//!
//! **Always registered.** The command natives are in the engine's console table, so [`register_always`] puts them on every
//! `Vm`; without a network host they find no connection and `commandToServer` has no `ServerConnection`. A game registers its
//! own command natives beside them with the helpers here ([`send_one`], [`host_of`], [`connection_target`], [`client_group`],
//! [`find_net_connection`]). `addTaggedString`,
//! `removeTaggedString` and `getTaggedString` are the VM's.
//!
//! **What a send does.** The command's tag is tested (`[A-Za-z0-9_-]*`) and must be a tag (`Remote Command Error - command must
//! be a tag.`); the NetString events of its new tags and then the command are queued on the connection
//! ([`EventLayer::post_command`]). A target that is not a `GameConnection` the host made (an object from `new
//! GameConnection()`, a connection not yet accepted) is silent. A command the full ordered queue cannot take drops that
//! connection with [`NET_QUEUE_FULL_REASON`].
//!
//! **In the engine's protocol** a send does the same checks against the connection's own table of command tags and hands the
//! command, as a body, to the connection's RPC link ([`command_rpc`](super::command_rpc)); a link whose send queue is full drops
//! the connection with the same reason. A local connection and a connection in Torque's protocol keep the event queue.
//!
//! Credit: the natives follow Torque3D's MIT-licensed `app/net/net.cpp` (`commandToClient`, `commandToServer`,
//! `buildTaggedString`; `Copyright (c) 2012 GarageGames, LLC`), `T3D/gameBase/gameConnection.cpp` (`Copyright (c) 2012
//! GarageGames, LLC`, `Copyright (C) 2015 Faust Logic, Inc.`) and `sim/netStringTable.cpp` (`Copyright (c) 2012 GarageGames,
//! LLC`).

use std::cell::RefCell;
use std::fmt;
use std::rc::{Rc, Weak};

use torque_net::handshake::ConnId;
use torque_net::remote::SendError;
use torque_net::remote::body::BodyError;
use torque_ts_vm::sim::ObjId;
use torque_ts_vm::text::atoi;
use torque_ts_vm::vm::{Native, NativeCall, Vm, VmError};

use super::Engine;
use super::NetGame;
use super::NetShared;
use super::conn::{DeleteMode, delete_connection};
use super::events::EventLayer;
use super::events::{NET_QUEUE_FULL_REASON, queue_full_line};
use super::rpc::ServerRpc;
use torque_engine::rpc::RpcError;

type Shared<G> = Rc<RefCell<NetShared<G>>>;

/// The bytes `buildTaggedString` fills at most (a 512-byte buffer less the NUL).
pub const BUILD_TAGGED_BYTES: usize = 511;

/// Registers a native that is charged the bytes of its arguments first (the commands copy them into the connection's queue).
pub fn register_charged(
    vm: &mut Vm,
    name: &str,
    min: i32,
    max: i32,
    f: impl Fn(&mut Vm, &NativeCall) -> Native + 'static,
) {
    vm.register(name, min, max, "", move |vm, c| {
        let bytes = c
            .argv
            .iter()
            .skip(1)
            .map(Vec::len)
            .fold(0usize, usize::saturating_add);
        if bytes > 0 && !vm.charge_work(u64::try_from(bytes).unwrap_or(u64::MAX)) {
            return Native::Fail(VmError::StepLimit);
        }
        f(vm, c)
    });
}

/// `Sim::findObject` cast to `NetConnection`, and the host's record of it (a command can only be sent on a connection the host
/// made).
pub fn connection_target<G: NetGame>(vm: &Vm, shared: &Shared<G>, text: &[u8]) -> Option<ObjId> {
    let id = find_net_connection(vm, text)?;
    shared.borrow().conns.contains_key(&id).then_some(id)
}

/// The members of `ClientGroup` that are connections the host made, in order.
pub fn client_group<G: NetGame>(vm: &Vm, shared: &Shared<G>) -> Vec<ObjId> {
    let Some(group) = vm.objects().find(b"ClientGroup") else {
        return Vec::new();
    };
    let Some(members) = vm.objects().members(group) else {
        return Vec::new();
    };
    let sh = shared.borrow();
    members
        .as_slice()
        .iter()
        .copied()
        .filter(|id| sh.conns.contains_key(id))
        .collect()
}

/// The kind of a remote command a native sends: the codecs' variant
/// ([`NetCodecs::command_variant`](super::codecs::NetCodecs::command_variant)) and the error line a command that is not a tag
/// prints.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommandKind {
    pub secure: bool,
    pub not_a_tag: &'static str,
}

/// The plain commands' kind (`commandToClient` and the rest).
pub const PLAIN: CommandKind = CommandKind {
    secure: false,
    not_a_tag: "Remote Command Error - command must be a tag.",
};

/// `sendRemoteCommand` to one connection, with the error line of its kind. In the engine's protocol the command goes out as an
/// RPC ([`command_rpc`](super::command_rpc)); the checks, the silence and the full queue's drop are the same.
pub fn send_one<G: NetGame>(
    vm: &mut Vm,
    shared: &Shared<G>,
    layer: &Layer<G>,
    obj: ObjId,
    kind: CommandKind,
    args: &[Vec<u8>],
) -> Result<(), VmError> {
    let conn: Option<ConnId> = shared.borrow().conns.get(&obj).and_then(|r| r.conn);
    let Some(conn) = conn else { return Ok(()) };
    let overflow = |vm: &mut Vm| {
        vm.print(queue_full_line(obj).as_bytes());
        delete_connection(
            vm,
            shared,
            obj,
            Some(NET_QUEUE_FULL_REASON),
            DeleteMode::Scheduled,
        )
    };
    // A connection of an engine-mode session carries its commands as RPCs; a local one (events) and Torque's protocol do not.
    let session = shared
        .borrow()
        .hs
        .connection(conn)
        .is_some_and(|c| c.engine);
    if session
        && let Some(rpc) = vm
            .extension::<ServerRpc>()
            .filter(|rpc| rpc.carries_commands())
    {
        return match layer.encode_command(vm, conn, kind.secure, args) {
            Ok(body) => match rpc.send_command(conn, body) {
                Err(RpcError::Busy) => overflow(vm),
                // A rate that is over, a link that is gone: silent, as a closed queue is.
                Ok(()) | Err(_) => Ok(()),
            },
            Err(BodyError::Send(SendError::NotATag)) => {
                vm.error(kind.not_a_tag.to_string());
                Ok(())
            }
            Err(error @ (BodyError::TooLarge | BodyError::UnknownTag)) => {
                vm.print(
                    format!("engine: a command to client {obj} was not sent: {error}").as_bytes(),
                );
                Ok(())
            }
            // A tag that is not a command name, too many arguments, a closed connection: silent.
            Err(BodyError::Send(_)) => Ok(()),
        };
    }
    match layer.post_command(vm, conn, kind.secure, args) {
        Ok(_) => Ok(()),
        Err(SendError::NotATag) => {
            vm.error(kind.not_a_tag.to_string());
            Ok(())
        }
        Err(SendError::Backpressure) => overflow(vm),
        // A tag that is not a command name, too many arguments, a closed queue: silent.
        Err(_) => Ok(()),
    }
}

pub fn done(result: Result<(), VmError>) -> Native {
    match result {
        Ok(()) => Native::Void,
        Err(e) => Native::Fail(e),
    }
}

/// `buildTaggedString(fmt, a1..a9)`: the text of tag `fmt` with `%1`..`%9` replaced by the arguments, in at most
/// [`BUILD_TAGGED_BYTES`] bytes. It stops at a `%k` whose argument is missing or does not fit; any other `%` is kept as it is.
pub fn build_tagged(fmt_text: &[u8], args: &[&[u8]]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(&b) = fmt_text.get(i) {
        let k = match (b, fmt_text.get(i + 1)) {
            (b'%', Some(&d @ b'1'..=b'9')) => Some(usize::from(d - b'1')),
            _ => None,
        };
        if let Some(k) = k {
            let Some(a) = args.get(k) else { break };
            if BUILD_TAGGED_BYTES - out.len() < a.len() {
                break;
            }
            out.extend_from_slice(a);
            i += 2;
        } else {
            if out.len() >= BUILD_TAGGED_BYTES {
                break;
            }
            out.push(b);
            i += 1;
        }
    }
    out
}

/// The network host as the always-registered natives find it: a weak handle (the host's state holds the server state too), set
/// by [`attach_host`](super::attach_host). Without one every send finds no connection, as on a server nobody has joined.
pub struct CommandHost<G: NetGame> {
    host: Option<(WeakShared<G>, Layer<G>)>,
}

type Layer<G> = EventLayer<<G as NetGame>::Events, <G as NetGame>::Ghosts>;
type WeakShared<G> = Weak<RefCell<NetShared<G>>>;

impl<G: NetGame> Default for CommandHost<G> {
    fn default() -> CommandHost<G> {
        CommandHost { host: None }
    }
}

impl<G: NetGame> Clone for CommandHost<G> {
    fn clone(&self) -> CommandHost<G> {
        CommandHost {
            host: self.host.clone(),
        }
    }
}

impl<G: NetGame> fmt::Debug for CommandHost<G> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = if self.host.is_some() {
            "attached"
        } else {
            "none"
        };
        write!(f, "CommandHost({state})")
    }
}

impl<G: NetGame> CommandHost<G> {
    pub fn new(shared: &Shared<G>, layer: Layer<G>) -> CommandHost<G> {
        CommandHost {
            host: Some((Rc::downgrade(shared), layer)),
        }
    }

    fn get(&self) -> Option<(Shared<G>, Layer<G>)> {
        let (weak, layer) = self.host.as_ref()?;
        Some((weak.upgrade()?, layer.clone()))
    }
}

pub fn host_of<G: NetGame>(engine: &Engine<G>) -> Option<(Shared<G>, Layer<G>)> {
    let host = engine.borrow().commands.clone();
    host.get()
}

/// `Sim::findObject` cast to `NetConnection` (any object of the class, host-made or not).
pub fn find_net_connection(vm: &Vm, text: &[u8]) -> Option<ObjId> {
    let id = vm.objects().find(text)?;
    let class = vm.objects().get(id)?.class();
    let nc = vm.objects().classes.find(b"NetConnection")?;
    vm.objects().classes.is_a(class, nc).then_some(id)
}

/// Registers the command natives on `vm`. **Always**, as the console table has them: with no network host attached (or while
/// a target is no connection the host made) a send finds nothing to send to, and `commandToServer` has no `ServerConnection`.
pub fn register_always<G: NetGame>(vm: &mut Vm, engine: &Engine<G>) {
    // commandToClient(client, tag, ...)
    {
        let engine = Rc::clone(engine);
        register_charged(vm, "commandToClient", 3, 22, move |vm, c| {
            let Some((shared, layer)) = host_of(&engine) else {
                return Native::Void;
            };
            let Some(target) = connection_target(vm, &shared, c.cstr(1)) else {
                return Native::Void;
            };
            done(send_one(vm, &shared, &layer, target, PLAIN, c.rest(2)))
        });
    }
    // commandToServer(tag, ...): a server has no ServerConnection, so there is nothing to send to.
    register_charged(vm, "commandToServer", 2, 21, |_, _| Native::Void);
    register_build_tagged(vm);
}

fn register_build_tagged(vm: &mut Vm) {
    register_charged(vm, "buildTaggedString", 2, 11, |vm, c| {
        let fmt = c.cstr(1);
        let fmt = fmt.strip_prefix(&[1u8][..]).unwrap_or(fmt);
        let id = atoi(fmt) as u32;
        let Some(text) = vm.tags().lookup(id).map(<[u8]>::to_vec) else {
            return Native::Str(Vec::new());
        };
        let args: Vec<&[u8]> = (2..c.argc()).map(|i| c.cstr(i)).collect();
        Native::Str(build_tagged(&text, &args))
    });
}
