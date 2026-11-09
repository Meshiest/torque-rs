//! Plugin RPCs: typed requests a client makes of the server (with a reply, or as a notification) and notifications the server
//! sends a client, declared by a plugin ([`NetRegistry::rpc`]) with their size, rate and who may call them.
//!
//! ```
//! use torque_engine::rpc::{Allow, Delivery, RpcCx, RpcError};
//! use torque_engine::{EngineBuilder, NetMessage, Plugin, PluginId};
//!
//! #[derive(Debug, Clone, PartialEq, Default, NetMessage)]
//! struct Greet {
//!     #[net(max_len = 32)]
//!     name: String,
//! }
//!
//! #[derive(Debug, Clone, PartialEq, Default, NetMessage)]
//! struct Greeting {
//!     #[net(max_len = 64)]
//!     text: String,
//! }
//!
//! #[derive(PluginId)]
//! #[plugin(id = "example.greeter")]
//! struct Greeter;
//!
//! impl Plugin for Greeter {
//!     fn build(&self, engine: &mut EngineBuilder) {
//!         engine
//!             .net()
//!             .rpc::<Greet, Greeting>("greet")
//!             .to_server()
//!             .delivery(Delivery::Reliable)
//!             .rate(4, 8)
//!             .max_bytes(128)
//!             .allow(Allow::Anyone)
//!             .server(|_cx: &mut RpcCx, req: Greet| -> Result<Greeting, RpcError> {
//!                 Ok(Greeting { text: format!("hello, {}", req.name) })
//!             });
//!     }
//! }
//!
//! let engine = EngineBuilder::new().add_plugins(Greeter).build()?;
//! assert_eq!(engine.net_rpcs()?.len(), 1);
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! **Messages** are structs with `#[derive(NetMessage)]`: the same layout machinery as a ghost record, with caps a peer
//! cannot get around (a `String` or `Vec` needs a `max_len`, a float needs `finite`, a raw object id does not compile).
//! **Directions**: a `to_server` RPC is called by a client and handled on the server (with a reply, or a notification when the
//! reply type is `()`); a `to_client` RPC is a notification the server sends and a client handles, and a server never waits for
//! a client's answer, so its reply type must be `()` (a reply-bearing one does not compile).
//!
//! **Where the bytes go.** An [`RpcLink`] is one connection's state: it frames calls, holds them to their rate and size,
//! matches replies to calls, and hands the frames to a carrier ([`RpcCarrier`]) in batches. The TGE-mode carrier is the
//! reserved priority channel [`RPC_CHANNEL`] in each direction (the channels' own caps apply to it); the engine protocol's
//! carrier is another implementation of the same trait. Every RPC is listed by name, direction and layout digest in the
//! manifest, so a peer whose RPCs differ is refused at connect.
//!
//! # Maintaining
//!
//! **Module map.** This file: the types (`Delivery`, `RpcError`, `Allow`, `RpcCx`), the builder (`NetRegistry::rpc`), the
//! `RpcTable`, the carrier seam (`RpcCarrier`) and the reserved channel's logic (`RpcChannel`). `rpc/link.rs`: `RpcLink` (frames,
//! buckets, call matching, the work queue). `rpc_tests.rs`: the tests. The hosts that run links are `torque-server`'s
//! `net/rpc.rs` (`ServerRpc`, `ClientRpc`, the natives, `onRpcCheck`) and, for the client's ghost table and hang-up,
//! `torque-app`'s `client.rs` (`RpcHost`). Permissions are `permissions.rs`.
//!
//! **A call's path.** `RpcLink::call`/`notify`/`send_script` validate the message, check the queue and the send bucket, and
//! queue a frame. `flush` hands batches to the carrier, which may refuse (the frames stay). The peer's carrier gives a batch to
//! `receive`, which checks the frame, rate-limits and decodes it into the inbox. The host calls `take_work` after the packet's
//! ghosts are applied and runs each `Work` with an `RpcEnv`: ghost check, `Allow`, `onRpcCheck`, then the handler (a script
//! handler gets objects, not ghost indexes). `complete` queues the reply, and the next `flush` sends it.
//!
//! **Invariants and their tests.**
//! - A refused or unknown peer input is a `NetError` and the connection is dropped, never a panic (`rpc_protocol_errors`,
//!   `rpc_fuzz_hostile_batches_never_panic`).
//! - `onRpcCheck` fails closed and a call with no caller object is refused (`net::rpc::tests::on_rpc_check_allows_only_a_true_answer`,
//!   `a_call_with_no_caller_object_is_refused_even_with_no_on_rpc_check`, in `torque-server`).
//! - No raw ghost index reaches a handler, and a ghost the peer's side does not have is refused before it runs
//!   (`rpc_a_script_gets_objects_not_ghost_indexes_both_ways`, `rpc_ghost_references_are_checked_before_the_handler`,
//!   `rpc_a_client_checks_the_ghosts_a_notification_names`).
//! - Nothing a peer must refuse is sent: no float that is not finite (`rpc_a_float_that_is_not_finite_is_refused_on_receipt_and_never_sent`).
//! - A reply to a call that timed out is ignored and its id is not reused while it may come (`rpc_a_late_reply_to_a_timed_out_call_is_ignored`,
//!   `rpc_a_tombstoned_call_id_is_not_reused`, `rpc_the_tombstone_order_holds_each_live_id_once`).
//! - The reserved channel holds received batches for each connection apart, and only the connection over its share is dropped
//!   (`rpc_one_connection_cannot_crowd_out_the_others_inbox`).
//! - Two RPCs may not share a script handler, compared without case after punctuation becomes `_`
//!   (`rpc_two_rpcs_that_share_a_script_handler_do_not_register`).
//! - Two RPCs may not ask for the same default permission: the plugin's id is one segment (`.` becomes `_`), and a clash after
//!   that cleaning is refused at registration (`rpc_two_rpcs_that_share_a_default_permission_do_not_register`).
//! - What a side sends is held to the same buckets the receiver applies, and a refused batch stays queued in order
//!   (`rpc_what_a_side_sends_is_held_to_the_same_rates`, `rpc_a_refused_batch_stays_queued_in_order_and_replies_count_against_the_queue`).
//!
//! **Extension points.** A protocol brings its own `RpcCarrier` (`send` may refuse, `capacity` bounds the queue) and gives batches
//! to `RpcLink::receive`; the reserved channel is the TGE-mode carrier. `PermissionProvider` (in `permissions.rs`) is the slot for
//! where grants are kept.
//!
//! **Checks.** `cargo test -p torque-engine rpc` (it holds the permission table and the limit tests), the fuzz as `TORQUE_FUZZ_RPC=1000000 cargo test -p torque-engine --lib rpc_fuzz`,
//! `cargo test -p torque-macros --test ui` (`TRYBUILD=overwrite` rewrites the expected compiler output), `cargo test -p
//! torque-example-minimal-engine --test rpc`, the wire goldens (`cargo test -p torque-net --test tge_golden`, `cargo test -p
//! torque-trace-hash --test wire_golden`) and `just trace-hash` (a game with no RPC must not change a byte), and `just script-reference` (its sample plugin registers one RPC of each kind, so a change to the script natives or to how a message's layout text reads must change the RPC page too).

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::marker::PhantomData;
use std::rc::Rc;
use std::sync::OnceLock;

use crate::permissions::{DeclaredPermission, PermissionView};
use torque_ghosts::codec::{self, NetWidths, ReadCtx};
use torque_ghosts::plugin::NetMessage;
use torque_ghosts::{GhostRef, Value};
use torque_net::NetError;
use torque_net::bits::{BitReader, BitWriter};
use torque_net::chunk::ChannelLimits;
use torque_net::chunk::registry::{ChannelEntry, ChunkChannel};
use torque_net::chunk::{Kind, upstream::MAX_UPSTREAM_MESSAGE};
use torque_net::handshake::{ConnId, siphash24};
use torque_net::huffman::Table;
use torque_ts_vm::sim::ObjId;
use torque_ts_vm::vm::Vm;

use crate::net::NetRegistry;
use crate::{BuildError, Engine, EngineBuilder, PluginId};

mod link;

pub use link::{CallWork, Done, ErrorCode, FrameKind, Reply, RpcEnv, RpcLink, Side, Work};

/// The most bytes one RPC message may encode to (a call's frame, header included): the hard maximum of
/// [`RpcBuilder::max_bytes`].
pub const MAX_RPC_BYTES: usize = 1_024;
/// The most RPCs an engine may register (their ids are indexes in the name-sorted table).
pub const MAX_RPCS: usize = 256;
/// The most calls of one RPC awaiting a reply at once.
pub const MAX_OUTSTANDING: usize = 16;
/// The default cap on the reliable RPC bytes a connection holds that the peer has not acknowledged.
pub const DEFAULT_SEND_QUEUE: usize = 64 << 10;
/// The most bytes of a handler's error text that cross the wire.
pub const MAX_ERROR_TEXT: usize = 255;
/// The longest RPC name.
pub const MAX_RPC_NAME: usize = 40;
/// The reserved channel that carries RPCs in TGE mode (listed in the manifest like any other channel).
pub const RPC_CHANNEL: &str = "torque.rpc";
/// The default calls a second one connection may make across all RPCs, and its burst.
pub const DEFAULT_CONN_RATE: (u32, u32) = (64, 128);
/// Ticks of no reply after which a call fails with [`RpcError::Timeout`] (10 seconds of 32 ms ticks).
pub const DEFAULT_TIMEOUT_TICKS: u64 = 10_000 / 32;

/// How the bytes of an RPC travel.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
#[non_exhaustive]
pub enum Delivery {
    /// Resent until acknowledged, and in order among a connection's reliable RPCs.
    #[default]
    Reliable,
    /// Sent once in the next packet that has room and dropped when lost; for cosmetic one-shots. It has no reply, and
    /// in TGE mode it is sent reliably.
    Unreliable,
}

/// Who calls an RPC and who handles it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum RpcDirection {
    /// A client calls, the server handles.
    ToServer,
    /// The server sends a notification, a client handles it.
    ToClient,
}

/// Why an RPC call failed.
#[derive(Clone, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum RpcError {
    /// The server refused the call: permission, or a ghost it did not send that client.
    Refused,
    /// The call went over a rate limit (the peer's, or on a call that has a reply waiting, the answer says so).
    RateLimited,
    /// The message encodes to more than the RPC's `max_bytes` (local, before sending).
    TooLarge,
    /// The connection's send queue is full or the RPC has all its calls outstanding (local, before sending): try again.
    Busy,
    /// The handler's own text (at most [`MAX_ERROR_TEXT`] bytes: internal details never cross).
    Handler(String),
    /// The connection ended before the reply.
    Disconnected,
    /// No reply came in time.
    Timeout,
    /// The message holds a value the RPC does not allow (a float that is not finite) or names an object the peer cannot see
    /// (local, before sending).
    Invalid,
    /// The RPC does not exist, is not this side's to call, or has other message types (local).
    Unsupported,
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RpcError::Refused => f.write_str("refused"),
            RpcError::RateLimited => f.write_str("too many calls"),
            RpcError::TooLarge => f.write_str("the message is too large"),
            RpcError::Busy => f.write_str("too many calls waiting"),
            RpcError::Handler(text) => f.write_str(text),
            RpcError::Disconnected => f.write_str("disconnected"),
            RpcError::Timeout => f.write_str("no reply"),
            RpcError::Invalid => f.write_str("the message holds a value the call does not allow"),
            RpcError::Unsupported => f.write_str("no such call"),
        }
    }
}

impl std::error::Error for RpcError {}

/// What the server knows of the ghosts of a connection: an RPC may name an object only if it was ghosted to the caller.
pub trait GhostResolver {
    /// Whether ghost `index` is one the server sent `conn` (a call naming another is refused before its handler runs).
    fn knows(&self, conn: Option<ConnId>, index: u32) -> bool;

    /// The object ghost `index` stands for on `conn`.
    fn resolve(&self, conn: Option<ConnId>, index: u32) -> Option<ObjId>;

    /// The ghost index `object` has on `conn`, if it is ghosted there (what a script's object id becomes on the wire).
    fn index_of(&self, conn: Option<ConnId>, object: ObjId) -> Option<u32>;
}

/// A resolver that knows no ghost.
pub struct NoGhosts;

impl GhostResolver for NoGhosts {
    fn knows(&self, _conn: Option<ConnId>, _index: u32) -> bool {
        false
    }

    fn resolve(&self, _conn: Option<ConnId>, _index: u32) -> Option<ObjId> {
        None
    }

    fn index_of(&self, _conn: Option<ConnId>, _object: ObjId) -> Option<u32> {
        None
    }
}

/// What a handler is given beside the message: the script engine of the side it runs on, who called, and the tick.
pub struct RpcCx<'a> {
    /// The script engine of the side the handler runs on.
    pub vm: &'a mut Vm,
    conn: Option<ConnId>,
    perms: &'a dyn PermissionView,
    tick: u64,
    ghosts: &'a dyn GhostResolver,
}

impl<'a> RpcCx<'a> {
    /// A context for a call from `conn` (`None` on a client: the server called) whose permissions are `perms`, at `tick`.
    pub fn new(
        vm: &'a mut Vm,
        conn: Option<ConnId>,
        perms: &'a dyn PermissionView,
        tick: u64,
        ghosts: &'a dyn GhostResolver,
    ) -> RpcCx<'a> {
        RpcCx {
            vm,
            conn,
            perms,
            tick,
            ghosts,
        }
    }

    /// The connection the call came from; `None` on a client.
    pub fn caller(&self) -> Option<ConnId> {
        self.conn
    }

    /// Whether the caller holds `permission` (a direct grant or one of their roles'). A client holds none: its permissions are
    /// the server's to give.
    pub fn has_permission(&self, permission: &str) -> bool {
        self.perms.has_permission(permission)
    }

    /// Who the caller is: the key their permissions and quotas are kept under (an auth identity, else the connection). `None`
    /// on a client.
    pub fn subject(&self) -> Option<crate::permissions::SubjectKey> {
        self.perms.subject()
    }

    /// The caller's permission view, which outlives the borrow of the context (for code that also needs `vm`).
    pub fn permission_view(&self) -> &'a dyn PermissionView {
        self.perms
    }

    /// The tick the call is handled in.
    pub fn tick(&self) -> u64 {
        self.tick
    }

    /// The object a [`GhostRef`] in the message stands for (the call was refused already if the caller was not sent it).
    pub fn resolve(&self, ghost: GhostRef) -> Option<ObjId> {
        self.ghosts.resolve(self.conn, ghost.0)
    }
}

/// Who may call a server RPC; checked on the server before the handler runs, then the server script's `onRpcCheck` may still
/// deny the call.
#[derive(Clone)]
#[non_exhaustive]
pub enum Allow {
    /// Any connected client.
    Anyone,
    /// Callers who hold this permission ([`Allow::permission`]).
    Permission(String),
    /// The closure decides ([`Allow::check`]).
    Check(Rc<dyn for<'a> Fn(&RpcCx<'a>) -> bool>),
}

impl Allow {
    /// An RPC only callers who hold `permission` (a dotted name such as `props.spawn`; a wildcard grant of the caller covers it) may
    /// call. The name is checked when the RPC is registered, and declared for the plugin when it is not already.
    pub fn permission(permission: &str) -> Allow {
        Allow::Permission(permission.to_string())
    }

    /// An RPC the closure decides for each call.
    pub fn check(f: impl for<'a> Fn(&RpcCx<'a>) -> bool + 'static) -> Allow {
        Allow::Check(Rc::new(f))
    }

    /// Whether the call `cx` describes is allowed.
    pub fn permits(&self, cx: &RpcCx<'_>) -> bool {
        match self {
            Allow::Anyone => true,
            Allow::Permission(permission) => cx.has_permission(permission),
            Allow::Check(f) => f(cx),
        }
    }
}

impl std::fmt::Debug for Allow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Allow::Anyone => f.write_str("Anyone"),
            Allow::Permission(permission) => write!(f, "Permission({permission})"),
            Allow::Check(_) => f.write_str("Check(..)"),
        }
    }
}

/// What the host adds when it runs a call: the server script's say in it.
pub trait RpcHooks {
    /// The server script's `onRpcCheck(%client, %name)`: anything but true denies the call.
    fn allow(&mut self, vm: &mut Vm, caller: Option<ConnId>, rpc: &str) -> bool {
        let _ = (vm, caller, rpc);
        true
    }

    /// Calls the script function `function` for an RPC whose handler is a script's, with the message as script values.
    fn script(
        &mut self,
        vm: &mut Vm,
        caller: Option<ConnId>,
        function: &str,
        args: Vec<Vec<u8>>,
    ) -> Result<(), String> {
        let _ = (vm, caller, function, args);
        Err("no script host".to_string())
    }
}

/// The hooks of a host that adds nothing.
pub struct NoHooks;

impl RpcHooks for NoHooks {}

/// The erased handler of an RPC.
type NativeHandler = Rc<dyn Fn(&mut RpcCx<'_>, Box<dyn Any>) -> Result<Box<dyn Any>, RpcError>>;

#[derive(Clone)]
enum Handler {
    Native(NativeHandler),
    /// The game's script function (`rpcServer_<name>` or `rpcClient_<name>`).
    Script(String),
}

/// Reads a message of one type from a frame, checked and validated.
type DecodeFn = fn(&mut BitReader<'_>, NetWidths) -> Result<Box<dyn Any>, NetError>;

/// Replaces the ghost indexes a message holds ([`NetMessage::map_ghosts`]), false when one has no answer.
type MapGhostsFn = fn(&mut dyn Any, &mut dyn FnMut(u32) -> Option<u32>) -> bool;

/// The message codec of one type, erased.
#[derive(Clone, Copy)]
struct MessageCodec {
    decode: DecodeFn,
    encode: fn(&dyn Any, &mut BitWriter, NetWidths) -> Result<(), NetError>,
    validate: fn(&dyn Any) -> Result<(), NetError>,
    map_ghosts: MapGhostsFn,
    refs: fn(&dyn Any, &mut Vec<u32>),
    from_script: fn(&[Vec<u8>]) -> Box<dyn Any>,
    values: fn(&dyn Any) -> Vec<Value>,
    unit: bool,
    fields: fn() -> &'static [&'static str],
    schema: &'static str,
}

fn table() -> &'static Table {
    static TABLE: OnceLock<Table> = OnceLock::new();
    TABLE.get_or_init(Table::default)
}

impl MessageCodec {
    fn of<M: NetMessage>() -> MessageCodec {
        MessageCodec {
            decode: |r, widths| {
                let ctx = ReadCtx::new(widths, false);
                let mut charge = torque_ghosts::free_charge();
                let message: M = codec::decode(r, table(), &mut charge, &ctx)?;
                message.validate()?;
                Ok(Box::new(message))
            },
            encode: |message, w, widths| {
                let message = message
                    .downcast_ref::<M>()
                    .ok_or(NetError::Invalid("a message of another type"))?;
                codec::encode(message, w, table(), widths)
            },
            validate: |message| {
                message
                    .downcast_ref::<M>()
                    .ok_or(NetError::Invalid("a message of another type"))?
                    .validate()
            },
            map_ghosts: |message, map| {
                message
                    .downcast_mut::<M>()
                    .is_some_and(|message| message.map_ghosts(map))
            },
            refs: |message, out| {
                if let Some(message) = message.downcast_ref::<M>() {
                    message.ghost_refs(out);
                }
            },
            from_script: |args| Box::new(M::from_script(args)),
            values: |message| {
                message
                    .downcast_ref::<M>()
                    .map_or_else(Vec::new, M::script_values)
            },
            unit: TypeId::of::<M>() == TypeId::of::<()>(),
            fields: M::field_names,
            schema: M::SCHEMA,
        }
    }
}

/// An RPC a plugin registered: everything both ends need, in the engine's list of them (`engine.list::<RpcEntry>()`).
#[derive(Clone)]
pub struct RpcEntry {
    /// The name, as scripts and the manifest know it.
    pub name: String,
    /// The plugin that registered it.
    pub plugin: PluginId,
    /// Who calls it.
    pub direction: RpcDirection,
    /// How its bytes travel.
    pub delivery: Delivery,
    /// Calls a second one connection may make.
    pub rate: u32,
    /// Calls one connection may make at once.
    pub burst: u32,
    /// The most bytes a message of it encodes to.
    pub max_bytes: usize,
    /// Whether the call has a reply (the reply type is not `()`).
    pub reply: bool,
    /// Who may call it (checked on the server).
    pub allow: Allow,
    digest: u64,
    request: MessageCodec,
    response: MessageCodec,
    handler: Handler,
}

impl RpcEntry {
    /// The digest of the layouts of the request and the reply and the entry's direction, which the manifest carries.
    pub fn digest(&self) -> u64 {
        self.digest
    }

    /// The request's field names in order (what a script call fills).
    pub fn request_fields(&self) -> &'static [&'static str] {
        (self.request.fields)()
    }

    /// The request's layout as text.
    pub fn request_schema(&self) -> &'static str {
        self.request.schema
    }

    /// The reply's layout as text (empty for a notification).
    pub fn response_schema(&self) -> &'static str {
        self.response.schema
    }

    /// The script function of an RPC whose handler is a script's.
    pub fn script_function(&self) -> Option<&str> {
        match &self.handler {
            Handler::Script(function) => Some(function),
            Handler::Native(_) => None,
        }
    }

    /// The manifest entry for this RPC.
    pub fn manifest_entry(&self) -> String {
        torque_net::manifest::rpc_entry(
            &self.name,
            self.direction == RpcDirection::ToServer,
            self.digest,
        )
    }
}

impl std::fmt::Debug for RpcEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RpcEntry")
            .field("name", &self.name)
            .field("direction", &self.direction)
            .field("delivery", &self.delivery)
            .field("rate", &(self.rate, self.burst))
            .field("max_bytes", &self.max_bytes)
            .field("reply", &self.reply)
            .finish_non_exhaustive()
    }
}

const DIGEST_KEY: [u8; 16] = *b"torque-rpc-table";

fn digest_of(request: &MessageCodec, response: &MessageCodec, direction: RpcDirection) -> u64 {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(request.schema.as_bytes());
    bytes.push(0);
    bytes.extend_from_slice(response.schema.as_bytes());
    bytes.push(0);
    bytes.push(u8::from(!response.unit));
    bytes.push(u8::from(direction == RpcDirection::ToServer));
    siphash24(&DIGEST_KEY, &bytes)
}

/// Why an engine's RPCs make no table.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum RpcTableError {
    /// Two RPCs have this name.
    Duplicate(String),
    /// More than [`MAX_RPCS`].
    TooMany,
}

impl std::fmt::Display for RpcTableError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RpcTableError::Duplicate(name) => write!(f, "two RPCs are named {name}"),
            RpcTableError::TooMany => write!(f, "more than {MAX_RPCS} RPCs"),
        }
    }
}

impl std::error::Error for RpcTableError {}

/// The RPCs of an engine in id order: sorted by name, so both ends agree on the ids without sending them.
#[derive(Clone, Default, Debug)]
pub struct RpcTable {
    entries: Rc<Vec<Rc<RpcEntry>>>,
}

impl RpcTable {
    /// The table of `entries`.
    pub fn new(entries: &[RpcEntry]) -> Result<RpcTable, RpcTableError> {
        let mut sorted: Vec<Rc<RpcEntry>> = entries.iter().cloned().map(Rc::new).collect();
        sorted.sort_by(|a, b| a.name.cmp(&b.name));
        if sorted.len() > MAX_RPCS {
            return Err(RpcTableError::TooMany);
        }
        if let Some(pair) = sorted.windows(2).find(|p| p[0].name == p[1].name) {
            return Err(RpcTableError::Duplicate(pair[0].name.clone()));
        }
        Ok(RpcTable {
            entries: Rc::new(sorted),
        })
    }

    /// How many RPCs.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether there are none.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// RPC `id`.
    pub fn get(&self, id: usize) -> Option<&Rc<RpcEntry>> {
        self.entries.get(id)
    }

    /// The id of `name`.
    pub fn id(&self, name: &str) -> Option<usize> {
        self.entries
            .binary_search_by(|e| e.name.as_str().cmp(name))
            .ok()
    }

    /// The entries in id order.
    pub fn iter(&self) -> impl Iterator<Item = &Rc<RpcEntry>> {
        self.entries.iter()
    }

    /// The width of an RPC id on the wire.
    pub fn id_bits(&self) -> u8 {
        let last = u32::try_from(self.entries.len().saturating_sub(1)).unwrap_or(u32::MAX);
        torque_net::bits::bit_length(last).max(1)
    }
}

/// The marker of a builder whose direction is not set yet.
pub struct Unset;
/// The marker of a builder for a client-to-server RPC.
pub struct ToServer;
/// The marker of a builder for a server-to-client RPC.
pub struct ToClient;

/// The reply type of a server-to-client RPC: only `()`, since a server never waits for a client's answer. A server that needs
/// one sends a notification and the client answers with an RPC of its own.
#[diagnostic::on_unimplemented(
    message = "a server-to-client RPC is a notification, so its reply type must be `()` and not `{Self}`",
    label = "a server never waits for a client's reply",
    note = "send a notification and let the client answer with a client-to-server RPC"
)]
pub trait Notification: NetMessage {}

impl Notification for () {}

/// A registered RPC's typed handle: what a client calls through ([`RpcLink::rpc`]) and what a server sends with.
pub struct RpcRef<Req, Resp> {
    name: Rc<str>,
    types: PhantomData<fn(Req) -> Resp>,
}

impl<Req, Resp> RpcRef<Req, Resp> {
    /// The RPC's name.
    pub fn name(&self) -> &str {
        &self.name
    }

    pub(crate) fn new(name: &str) -> RpcRef<Req, Resp> {
        RpcRef {
            name: Rc::from(name),
            types: PhantomData,
        }
    }
}

impl<Req, Resp> Clone for RpcRef<Req, Resp> {
    fn clone(&self) -> RpcRef<Req, Resp> {
        RpcRef {
            name: Rc::clone(&self.name),
            types: PhantomData,
        }
    }
}

impl<Req, Resp> std::fmt::Debug for RpcRef<Req, Resp> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "RpcRef({})", self.name)
    }
}

/// An RPC being declared ([`NetRegistry::rpc`]): choose a direction, the limits, and finish with the handler.
pub struct RpcBuilder<'a, Req, Resp, D> {
    engine: &'a mut EngineBuilder,
    name: String,
    direction: RpcDirection,
    delivery: Delivery,
    rate: u32,
    burst: u32,
    max_bytes: usize,
    allow: Allow,
    default_permission: bool,
    types: PhantomData<fn(Req, Resp, D)>,
}

impl NetRegistry<'_> {
    /// Starts declaring the RPC `name` (letters, digits, `.`, `_` and `-`, at most [`MAX_RPC_NAME`] bytes) whose request is
    /// `Req` and reply `Resp` (`()` for a notification). Choose its direction with `to_server` or `to_client`; the
    /// defaults are reliable delivery, 8 calls a second with a burst of 16, 256 bytes and [`Allow::Anyone`]. An engine
    /// with no RPC adds nothing to the wire, the manifest or the script engine.
    pub fn rpc<Req: NetMessage, Resp: NetMessage>(
        &mut self,
        name: &str,
    ) -> RpcBuilder<'_, Req, Resp, Unset> {
        RpcBuilder {
            engine: self.builder(),
            name: name.to_string(),
            direction: RpcDirection::ToServer,
            delivery: Delivery::Reliable,
            rate: 8,
            burst: 16,
            max_bytes: 256,
            allow: Allow::Anyone,
            default_permission: false,
            types: PhantomData,
        }
    }
}

impl<'a, Req: NetMessage, Resp: NetMessage> RpcBuilder<'a, Req, Resp, Unset> {
    /// A call a client makes and the server handles.
    pub fn to_server(self) -> RpcBuilder<'a, Req, Resp, ToServer> {
        self.direction(RpcDirection::ToServer)
    }
}

impl<'a, Req: NetMessage, Resp: Notification> RpcBuilder<'a, Req, Resp, Unset> {
    /// A notification the server sends and a client handles. Its reply type is `()`.
    pub fn to_client(self) -> RpcBuilder<'a, Req, Resp, ToClient> {
        self.direction(RpcDirection::ToClient)
    }
}

impl<'a, Req, Resp, D> RpcBuilder<'a, Req, Resp, D> {
    fn direction<E>(self, direction: RpcDirection) -> RpcBuilder<'a, Req, Resp, E> {
        RpcBuilder {
            engine: self.engine,
            name: self.name,
            direction,
            delivery: self.delivery,
            rate: self.rate,
            burst: self.burst,
            max_bytes: self.max_bytes,
            allow: self.allow,
            default_permission: self.default_permission,
            types: PhantomData,
        }
    }

    /// How the bytes travel: [`Delivery::Reliable`] (the default) or [`Delivery::Unreliable`] (only for a notification).
    #[must_use]
    pub fn delivery(mut self, delivery: Delivery) -> Self {
        self.delivery = delivery;
        self
    }

    /// The calls a second one connection may make and its burst (a token bucket counted in server ticks, so it is
    /// deterministic). A call over it is dropped and counted; the receiver applies it, a client too to what a server sends.
    #[must_use]
    pub fn rate(mut self, per_second: u32, burst: u32) -> Self {
        self.rate = per_second;
        self.burst = burst;
        self
    }

    /// The most bytes a message of this RPC encodes to, its header included (at most [`MAX_RPC_BYTES`]).
    #[must_use]
    pub fn max_bytes(mut self, bytes: usize) -> Self {
        self.max_bytes = bytes;
        self
    }
}

impl<Req: NetMessage, Resp: NetMessage> RpcBuilder<'_, Req, Resp, ToServer> {
    /// Who may call it ([`Allow::Anyone`] by default).
    #[must_use]
    pub fn allow(mut self, allow: Allow) -> Self {
        self.allow = allow;
        self
    }

    /// Asks for the default permission, `rpc.<plugin>.<name>` (the plugin's id and the RPC's name, lowercased, anything but
    /// letters, digits, `.`, `_` and `-` made `_`, and the plugin's `.` made `_` too so it is one segment): only callers who
    /// hold it may call. Replaces [`allow`](Self::allow). An RPC whose default permission another RPC already has is refused.
    #[must_use]
    pub fn default_permission(mut self) -> Self {
        self.default_permission = true;
        self
    }

    /// Finishes the RPC with a native handler the server runs for each call that passed the checks. The handler gets the
    /// call's context and the decoded message and returns the reply (or a [`RpcError`]: only [`RpcError::Refused`] and
    /// [`RpcError::Handler`] reach the caller).
    pub fn server<F>(self, handler: F) -> RpcRef<Req, Resp>
    where
        F: for<'c> Fn(&mut RpcCx<'c>, Req) -> Result<Resp, RpcError> + 'static,
    {
        self.finish(Handler::Native(native_handler(handler)))
    }
}

impl<Req: NetMessage> RpcBuilder<'_, Req, (), ToServer> {
    /// Finishes the notification with the game's script as the handler: `rpcServer_<name>(%client, ..fields)`, the name's
    /// punctuation turned into `_`.
    pub fn server_script(self) -> RpcRef<Req, ()> {
        let function = script_function("rpcServer", &self.name);
        self.finish(Handler::Script(function))
    }
}

impl<Req: NetMessage, Resp: Notification> RpcBuilder<'_, Req, Resp, ToClient> {
    /// Finishes the RPC with a native handler each client runs for what the server sends. The server is hostile as far as the
    /// client is concerned: the message is checked against the same caps as a client's, and the rate applies on receipt.
    pub fn client<F>(self, handler: F) -> RpcRef<Req, Resp>
    where
        F: for<'c> Fn(&mut RpcCx<'c>, Req) -> Result<(), RpcError> + 'static,
    {
        self.finish(Handler::Native(native_handler(move |cx, req| {
            handler(cx, req).map(|()| Resp::default())
        })))
    }

    /// Finishes the RPC with the game's client script as the handler: `rpcClient_<name>(..fields)`.
    pub fn client_script(self) -> RpcRef<Req, Resp> {
        let function = script_function("rpcClient", &self.name);
        self.finish(Handler::Script(function))
    }
}

fn native_handler<Req: NetMessage, Resp: NetMessage>(
    handler: impl for<'c> Fn(&mut RpcCx<'c>, Req) -> Result<Resp, RpcError> + 'static,
) -> NativeHandler {
    Rc::new(move |cx, message| match message.downcast::<Req>() {
        Ok(req) => handler(cx, *req).map(|resp| Box::new(resp) as Box<dyn Any>),
        Err(_) => Err(RpcError::Unsupported),
    })
}

/// The script function of an RPC: the prefix, `_`, and the name with everything but letters and digits as `_`.
pub fn script_function(prefix: &str, name: &str) -> String {
    let clean: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    format!("{prefix}_{clean}")
}

/// The permission an RPC asks for by default: `rpc.<plugin>.<name>`. The plugin's id is one segment (its `.` becomes `_`), so
/// a plugin's id cannot reach into the RPC name's segments.
fn default_permission_name(plugin: PluginId, name: &str) -> String {
    let clean = |text: &str, keep_dot: bool| -> String {
        text.to_ascii_lowercase()
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || matches!(c, '_' | '-') || (keep_dot && c == '.') {
                    c
                } else {
                    '_'
                }
            })
            .collect()
    };
    format!(
        "rpc.{}.{}",
        clean(plugin.as_str(), false),
        clean(name, true)
    )
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_RPC_NAME
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

impl<Req: NetMessage, Resp: NetMessage, D> RpcBuilder<'_, Req, Resp, D> {
    fn finish(mut self, handler: Handler) -> RpcRef<Req, Resp> {
        let plugin = self.engine.current_plugin();
        let mut bad_permission = None;
        let mut default_clash = None;
        if self.default_permission {
            let name = default_permission_name(plugin, &self.name);
            if let Err(error) = torque_permissions::Permission::parse(&name) {
                bad_permission = Some(format!(
                    "the default permission `{name}` is not a permission ({error}): shorten the plugin id or the RPC name, or name one with `allow`"
                ));
            }
            default_clash = self
                .engine
                .list::<RpcEntry>()
                .iter()
                .find(|e| {
                    matches!(&e.allow, Allow::Permission(p) if *p == name)
                        && default_permission_name(e.plugin, &e.name) == name
                })
                .map(|e| (name.clone(), e.name.clone(), e.plugin));
            self.allow = Allow::Permission(name);
        } else if let Allow::Permission(p) = &self.allow
            && let Err(error) = torque_permissions::Permission::parse(p)
        {
            bad_permission = Some(error.to_string());
        }
        let script_clash = match &handler {
            Handler::Script(function) => self
                .engine
                .list::<RpcEntry>()
                .iter()
                .find(|e| {
                    e.script_function()
                        .is_some_and(|f| f.eq_ignore_ascii_case(function))
                })
                .map(|e| (function.clone(), e.name.clone())),
            Handler::Native(_) => None,
        };
        let handle = RpcRef::new(&self.name);
        let request = MessageCodec::of::<Req>();
        let response = MessageCodec::of::<Resp>();
        let problem = if let Some(error) = bad_permission {
            Some(format!("RPC `{}`: {error}", self.name))
        } else if let Some((permission, other, other_plugin)) = default_clash {
            Some(format!(
                "RPC `{}` and RPC `{other}` of plugin `{other_plugin}` would both ask for the default permission `{permission}` (a plugin id's `.` becomes `_`, names are lowercased and punctuation becomes `_`): rename one, or name a permission with `allow`",
                self.name
            ))
        } else if let Some((function, other)) = script_clash {
            Some(format!(
                "RPC `{}` and RPC `{other}` would both be handled by the script function `{function}` (an RPC name's punctuation becomes `_` in the function, and the function's case is ignored): rename one",
                self.name
            ))
        } else if !valid_name(&self.name) {
            Some(format!(
                "`{}` is not an RPC name (letters, digits, `.`, `_` and `-`, at most {MAX_RPC_NAME} bytes)",
                self.name
            ))
        } else if self.max_bytes == 0 || self.max_bytes > MAX_RPC_BYTES {
            Some(format!(
                "RPC `{}`: max_bytes {} is outside 1..={MAX_RPC_BYTES}",
                self.name, self.max_bytes
            ))
        } else if self.delivery == Delivery::Unreliable && !response.unit {
            Some(format!(
                "RPC `{}` is unreliable, so it has no reply: its reply type must be `()`",
                self.name
            ))
        } else if self.rate == 0 || self.burst == 0 {
            Some(format!(
                "RPC `{}`: the rate and the burst are at least 1",
                self.name
            ))
        } else if let Some(first) = self
            .engine
            .list::<RpcEntry>()
            .iter()
            .find(|e| e.name == self.name)
        {
            Some(format!(
                "RPC `{}` is also registered by plugin `{}`",
                self.name, first.plugin
            ))
        } else if self.engine.list::<RpcEntry>().len() >= MAX_RPCS {
            Some(format!("more than {MAX_RPCS} RPCs"))
        } else {
            None
        };
        if let Some(message) = problem {
            self.engine.fail(BuildError::Rejected { plugin, message });
            return handle;
        }
        if let Allow::Permission(p) = &self.allow
            && !self
                .engine
                .list::<DeclaredPermission>()
                .iter()
                .any(|d| d.name == *p)
        {
            let description = format!("Call the {} RPC", self.name);
            self.engine.push(DeclaredPermission {
                plugin,
                name: p.clone(),
                description,
            });
        }
        let digest = digest_of(&request, &response, self.direction);
        self.engine.push(RpcEntry {
            name: self.name,
            plugin,
            direction: self.direction,
            delivery: self.delivery,
            rate: self.rate,
            burst: self.burst,
            max_bytes: self.max_bytes,
            reply: !response.unit,
            allow: self.allow,
            digest,
            request,
            response,
            handler,
        });
        handle
    }
}

impl Engine {
    /// The RPCs plugins registered, in id order (empty for an engine with none, whose wire, manifest and channels are then
    /// what they were without RPCs).
    pub fn net_rpcs(&self) -> Result<RpcTable, RpcTableError> {
        RpcTable::new(self.list::<RpcEntry>())
    }
}

/// A carrier that could not take a batch now ([`RpcCarrier::send`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct CarrierFull;

/// How a connection's RPC bytes reach the peer: the seam between the RPC layer and a protocol.
///
/// The TGE-mode carrier is the reserved channel ([`RPC_CHANNEL`]); the engine protocol brings its own.
pub trait RpcCarrier {
    /// The reliable bytes handed over that the peer has not acknowledged.
    fn unacked(&self) -> usize;

    /// The most reliable bytes the carrier holds unacknowledged: the link keeps its own send queue within it.
    fn capacity(&self) -> usize {
        DEFAULT_SEND_QUEUE
    }

    /// Hands over one batch of frames. A carrier that cannot send an unreliable batch sends it reliably. A carrier that cannot
    /// take a batch now (its queue or a shared budget is full) refuses it, and the link keeps the frames for the next tick.
    ///
    /// # Errors
    /// [`CarrierFull`] when the batch was not taken.
    fn send(&self, batch: Vec<u8>, delivery: Delivery) -> Result<(), CarrierFull>;
}

/// The logic of the reserved channel in either direction: it keeps the batches that arrive until the host's tick takes them.
///
/// Each connection has its own share of what is held ([`CONN_INBOX_CAP`]), so one client cannot crowd out the others. A
/// connection that goes over its share is reported by [`take`](Self::take) ([`Received::over`]) for the host to drop; nothing a
/// well-behaved connection sent is ever discarded.
#[derive(Default)]
pub struct RpcChannel {
    inbox: Vec<(Option<ConnId>, Vec<u8>)>,
    held: HashMap<Option<ConnId>, usize>,
    over: Vec<Option<ConnId>>,
}

/// The most bytes of received batches the channel holds for one connection between two takes: twice the send queue an honest
/// peer is held to.
pub const CONN_INBOX_CAP: usize = 2 * DEFAULT_SEND_QUEUE;

/// What [`RpcChannel::take`] hands over.
#[derive(Default, Debug)]
pub struct Received {
    /// The batches received since the last take, with the connection each came from (`None` on a client), in order.
    pub batches: Vec<(Option<ConnId>, Vec<u8>)>,
    /// The connections that went over their share: their further batches were refused, and the host drops them.
    pub over: Vec<Option<ConnId>>,
}

impl RpcChannel {
    /// Hands over what was received and starts the next tick's shares.
    pub fn take(&mut self) -> Received {
        self.held.clear();
        Received {
            batches: std::mem::take(&mut self.inbox),
            over: std::mem::take(&mut self.over),
        }
    }

    fn keep(&mut self, from: Option<ConnId>, data: &[u8]) {
        let held = self.held.entry(from).or_default();
        if held.saturating_add(data.len()) > CONN_INBOX_CAP {
            if !self.over.contains(&from) {
                self.over.push(from);
            }
            return;
        }
        *held += data.len();
        self.inbox.push((from, data.to_vec()));
    }
}

impl ChunkChannel for RpcChannel {
    fn receive(&mut self, _kind: Kind, data: &[u8]) {
        self.keep(None, data);
    }

    fn receive_from(&mut self, from: ConnId, _kind: Kind, data: &[u8]) {
        self.keep(Some(from), data);
    }

    fn reset(&mut self) {
        self.take();
    }

    fn closed(&mut self) {
        self.take();
    }

    fn as_any_mut(&mut self) -> Option<&mut dyn Any> {
        Some(self)
    }
}

/// The bytes of a send queue the reserved channel allows (what it may hold unacknowledged).
const CHANNEL_MESSAGE: usize = 16 << 10;

/// The two entries of the reserved channel (to each client and to the server) for an engine whose plugins registered RPCs.
pub fn channel_entries() -> [ChannelEntry; 2] {
    let limits = ChannelLimits::new(1_024)
        .with_max_message(CHANNEL_MESSAGE.min(MAX_UPSTREAM_MESSAGE))
        .with_max_backlog(DEFAULT_SEND_QUEUE)
        .with_priority();
    [
        ChannelEntry::new(RPC_CHANNEL, limits, RpcChannel::default),
        ChannelEntry::to_server(RPC_CHANNEL, limits, RpcChannel::default),
    ]
}

/// The most bytes of frames one batch holds (a batch is one channel message).
pub const MAX_BATCH_BYTES: usize = CHANNEL_MESSAGE;

#[cfg(test)]
#[path = "rpc_tests.rs"]
mod tests;
