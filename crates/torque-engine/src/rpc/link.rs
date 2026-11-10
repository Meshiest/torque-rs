//! One connection's RPC state: framing, the limits, the matching of replies to calls, and the work a host runs after a packet.
//!
//! **Frames.** A batch handed to the carrier is frames back to back, each a 16-bit byte length and that many bytes of body. A
//! body is a bit stream: the kind ([`FrameKind`], 2 bits), the RPC's id (the table's width), the 16-bit call id for a request or
//! a reply, then the message in its layout (an error: a 2-bit [`ErrorCode`], and for a handler's error its text).
//!
//! **Hostile input.** [`RpcLink::receive`] reads bytes from a peer. A frame is checked before anything is decoded: the RPC must
//! exist and be one this side receives, its length must be within the RPC's `max_bytes`, and a reply must answer a call that is
//! waiting (or one that timed out lately: the late reply is ignored); each of these failing is a [`NetError`] (the connection is
//! dropped, as for any malformed packet). A call over its rate is dropped before it is decoded, counted, and answered with
//! [`RpcError::RateLimited`] when it has a reply waiting (once a tick for each RPC). The decode checks every field's cap against
//! the bits left; a trailing byte is an error. Nothing here panics.
//!
//! **What this side sends** is held to the same rates the receiving side applies, so an honest peer never sends what the other
//! drops, and to the values the receiving side accepts (a float that is not finite is never sent).
//!
//! **Time** is counted in server ticks the host reports ([`RpcLink::tick`]); the rate buckets, the reply timeout and the
//! once-a-second drop report all follow it, so a link behaves the same at any frame rate and under test.

use std::any::Any;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;

use super::{
    CarrierFull, DEFAULT_CONN_RATE, DEFAULT_SEND_QUEUE, DEFAULT_TIMEOUT_TICKS, Delivery,
    GhostResolver, Handler, MAX_BATCH_BYTES, MAX_ERROR_TEXT, MAX_OUTSTANDING, NetMessage,
    RpcCarrier, RpcCx, RpcDirection, RpcEntry, RpcError, RpcHooks, RpcRef, RpcTable,
};
use crate::command::{DropCounter, REPORT_TICKS, TokenBucket};
use crate::permissions::PermissionView;
use torque_ghosts::codec::NetWidths;
use torque_net::NetError;
use torque_net::bits::{BitReader, BitWriter};
use torque_net::cap::cut_str;
use torque_net::handshake::ConnId;
use torque_ts_vm::vm::Vm;

/// What a frame is (the first 2 bits of its body).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FrameKind {
    /// A call that waits for a reply.
    Request,
    /// A call with no reply.
    Notify,
    /// The reply to a request.
    Reply,
    /// A failed request's answer.
    Error,
}

impl FrameKind {
    fn bits(self) -> u32 {
        match self {
            FrameKind::Request => 0,
            FrameKind::Notify => 1,
            FrameKind::Reply => 2,
            FrameKind::Error => 3,
        }
    }

    fn from_bits(bits: u32) -> FrameKind {
        match bits & 3 {
            0 => FrameKind::Request,
            1 => FrameKind::Notify,
            2 => FrameKind::Reply,
            _ => FrameKind::Error,
        }
    }
}

/// Why a request failed, as it crosses the wire (2 bits).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ErrorCode {
    /// [`RpcError::Refused`].
    Refused,
    /// [`RpcError::RateLimited`].
    RateLimited,
    /// [`RpcError::Handler`], with its text.
    Handler,
}

impl ErrorCode {
    fn bits(self) -> u32 {
        match self {
            ErrorCode::Refused => 0,
            ErrorCode::RateLimited => 1,
            ErrorCode::Handler => 2,
        }
    }

    fn from_bits(bits: u32) -> Option<ErrorCode> {
        match bits {
            0 => Some(ErrorCode::Refused),
            1 => Some(ErrorCode::RateLimited),
            2 => Some(ErrorCode::Handler),
            _ => None,
        }
    }
}

/// The most bytes of unreliable frames waiting for one packet's room.
const UNRELIABLE_ROOM: usize = 1_024;

/// The most call ids kept for calls that timed out, so their late replies are told from replies to nothing.
const MAX_TOMBSTONES: usize = 4_096;

/// Which end of the connection a link is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Side {
    /// The server: it receives calls of `to_server` RPCs.
    Server,
    /// A client: it receives the notifications of `to_client` RPCs and the replies to its calls.
    Client,
}

/// The closure that waits for a call's reply, with the message decoded (or the error).
pub type Reply = Box<dyn FnOnce(Result<Box<dyn Any>, RpcError>)>;

struct Pending {
    rpc: usize,
    deadline: u64,
    done: Reply,
}

enum Incoming {
    Call {
        rpc: usize,
        call: Option<u16>,
        message: Box<dyn Any>,
    },
    Reply {
        done: Reply,
        result: Result<Box<dyn Any>, RpcError>,
    },
}

/// What a host runs after a packet ([`RpcLink::take_work`]): a call to hand to its handler, or a reply for a waiting caller.
pub enum Work {
    /// A call to run ([`Work::run`]).
    Call(CallWork),
    /// A reply (or an error) for the closure that waits for it.
    Reply {
        /// The caller's closure.
        done: Reply,
        /// What came.
        result: Result<Box<dyn Any>, RpcError>,
    },
}

/// A call received and decoded, ready to run.
pub struct CallWork {
    entry: Rc<RpcEntry>,
    call: Option<u16>,
    message: Box<dyn Any>,
    id: usize,
    id_bits: u8,
    widths: NetWidths,
    side: Side,
}

/// What the host gives a call as it runs it.
pub struct RpcEnv<'a> {
    /// The script engine of this side.
    pub vm: &'a mut Vm,
    /// The tick.
    pub tick: u64,
    /// The peer the call came from (`None` on a client).
    pub conn: Option<ConnId>,
    /// What the peer holds: its permissions.
    pub perms: &'a dyn PermissionView,
    /// The ghosts the server sent the peer.
    pub ghosts: &'a dyn GhostResolver,
    /// The script's say.
    pub hooks: &'a mut dyn RpcHooks,
}

/// What a run call leaves for the link: the frame of its answer, if it has one.
pub struct Done {
    frame: Option<Vec<u8>>,
    refused: Option<usize>,
}

impl Done {
    fn nothing() -> Done {
        Done {
            frame: None,
            refused: None,
        }
    }
}

impl Work {
    /// Runs the work: a call's checks and handler, or the caller's closure. The link is not borrowed while it runs, so a
    /// handler or closure may use whatever holds the link. Give what comes back to [`RpcLink::complete`].
    pub fn run(self, env: &mut RpcEnv<'_>) -> Done {
        match self {
            Work::Call(call) => call.run(env),
            Work::Reply { done, result } => {
                done(result);
                Done::nothing()
            }
        }
    }
}

fn header(
    kind: FrameKind,
    id_bits: u8,
    id: usize,
    call: Option<u16>,
) -> Result<BitWriter, NetError> {
    let id = u32::try_from(id).map_err(|_| NetError::Invalid("an RPC id too large for a frame"))?;
    let mut w = BitWriter::new();
    w.write_bits(kind.bits(), 2);
    w.write_bits(id, id_bits);
    if let Some(call) = call {
        w.write_bits(u32::from(call), 16);
    }
    Ok(w)
}

fn error_frame(id_bits: u8, id: usize, call: u16, error: &RpcError) -> Option<Vec<u8>> {
    let mut w = header(FrameKind::Error, id_bits, id, Some(call)).ok()?;
    match error {
        RpcError::Refused => w.write_bits(ErrorCode::Refused.bits(), 2),
        RpcError::RateLimited => w.write_bits(ErrorCode::RateLimited.bits(), 2),
        other => {
            w.write_bits(ErrorCode::Handler.bits(), 2);
            let text = match other {
                RpcError::Handler(text) => text.as_str(),
                _ => "failed",
            };
            let text = cut_str(text, MAX_ERROR_TEXT);
            w.write_u8(u8::try_from(text.len()).unwrap_or(u8::MAX));
            w.write_bytes(text.as_bytes());
        }
    }
    Some(w.into_bytes())
}

impl CallWork {
    /// The RPC this call is for.
    pub fn entry(&self) -> &RpcEntry {
        &self.entry
    }

    /// Whether every ghost the message names is one the peer's side knows: the server's check of what it sent the caller, and a
    /// client's of what its world has.
    fn ghosts_known(&self, env: &RpcEnv<'_>) -> bool {
        let mut refs = Vec::new();
        (self.entry.request.refs)(self.message.as_ref(), &mut refs);
        refs.iter().all(|r| env.ghosts.knows(env.conn, *r))
    }

    /// Whether the server lets the call run: `Allow`, then the script's `onRpcCheck`.
    fn permitted(&self, env: &mut RpcEnv<'_>) -> bool {
        let cx = RpcCx::new(env.vm, env.conn, env.perms, env.tick, env.ghosts);
        self.entry.allow.permits(&cx) && env.hooks.allow(env.vm, env.conn, &self.entry.name)
    }

    /// The handler's answer.
    fn invoke(mut self, env: &mut RpcEnv<'_>) -> Result<Box<dyn Any>, RpcError> {
        match &self.entry.handler {
            Handler::Native(handler) => {
                if self.entry.realm && self.side == Side::Client {
                    let values = (self.entry.request.values)(self.message.as_ref());
                    env.hooks.realm(env.vm, &self.entry.name, values);
                }
                let mut cx = RpcCx::new(env.vm, env.conn, env.perms, env.tick, env.ghosts);
                handler(&mut cx, self.message)
            }
            Handler::Script(function) => {
                // A script knows objects, not this connection's ghost indexes.
                let ghosts = env.ghosts;
                let conn = env.conn;
                let mapped = (self.entry.request.map_ghosts)(self.message.as_mut(), &mut |index| {
                    ghosts.resolve(conn, index)
                });
                if !mapped {
                    return Err(RpcError::Refused);
                }
                let values = (self.entry.request.values)(self.message.as_ref());
                let args = values.iter().map(value_text).collect();
                env.hooks
                    .script(env.vm, env.conn, function, args)
                    .map(|()| Box::new(()) as Box<dyn Any>)
                    .map_err(RpcError::Handler)
            }
        }
    }

    fn run(self, env: &mut RpcEnv<'_>) -> Done {
        let (id, id_bits, call) = (self.id, self.id_bits, self.call);
        let allowed = self.ghosts_known(env) && (self.side == Side::Client || self.permitted(env));
        if !allowed {
            return Done {
                frame: call.and_then(|call| error_frame(id_bits, id, call, &RpcError::Refused)),
                refused: Some(id),
            };
        }
        let (entry, widths) = (Rc::clone(&self.entry), self.widths);
        let result = self.invoke(env);
        let Some(call) = call else {
            return Done::nothing();
        };
        let frame = match result {
            Ok(resp) => {
                let Ok(mut w) = header(FrameKind::Reply, id_bits, id, Some(call)) else {
                    return Done::nothing();
                };
                match (entry.response.encode)(resp.as_ref(), &mut w, widths) {
                    Ok(()) if w.bytes().len() <= entry.max_bytes => Some(w.into_bytes()),
                    Ok(()) => {
                        let error = RpcError::Handler("the reply is too large".to_string());
                        error_frame(id_bits, id, call, &error)
                    }
                    Err(e) => error_frame(id_bits, id, call, &RpcError::Handler(e.to_string())),
                }
            }
            Err(error) => error_frame(id_bits, id, call, &error),
        };
        Done {
            frame,
            refused: None,
        }
    }
}

/// A script value as the text a script function is called with.
fn value_text(value: &torque_ghosts::Value) -> Vec<u8> {
    use torque_ghosts::Value;
    match value {
        Value::Null | Value::Id(None) | Value::Xform(None) => Vec::new(),
        Value::Bool(b) => vec![if *b { b'1' } else { b'0' }],
        Value::Int(n) => n.to_string().into_bytes(),
        Value::F32(x) => x.to_string().into_bytes(),
        Value::Str(s) => s.clone(),
        Value::Id(Some(id)) => id.to_string().into_bytes(),
        Value::Xform(Some(words)) => {
            let [px, py, pz, qx, qy, qz, neg] = *words;
            let float = |bits: u32| f32::from_bits(bits).to_string();
            let w = if neg == 0 { 1 } else { -1 };
            format!(
                "{} {} {} {} {} {} {}",
                float(px),
                float(py),
                float(pz),
                float(qx),
                float(qy),
                float(qz),
                w
            )
            .into_bytes()
        }
    }
}

/// One connection's RPC state: framing, limits, matching of replies to calls, and the work to run after a packet.
pub struct RpcLink {
    table: RpcTable,
    side: Side,
    widths: NetWidths,
    carrier: Rc<dyn RpcCarrier>,
    peer: Option<ConnId>,
    now: u64,
    id_bits: u8,
    /// What this side accepts: the connection's bucket for all RPCs and one for each.
    conn_bucket: TokenBucket,
    buckets: Vec<Option<TokenBucket>>,
    /// What this side sends, held to the same rates.
    send_conn_bucket: TokenBucket,
    send_buckets: Vec<Option<TokenBucket>>,
    limit_sends: bool,
    next_call: u16,
    pending: HashMap<u16, Pending>,
    /// Calls that timed out: id, the RPC, and the tick the id may be used again at; with the ids in the order they were made.
    tombstones: HashMap<u16, (usize, u64)>,
    tombstone_order: VecDeque<u16>,
    outstanding: Vec<usize>,
    inbox: VecDeque<Incoming>,
    out_reliable: Vec<Vec<u8>>,
    out_unreliable: Vec<Vec<u8>>,
    queued: usize,
    unreliable_queued: usize,
    send_queue: usize,
    timeout: u64,
    drops: Vec<DropCounter>,
    /// The tick a rate-limited call of each RPC was last answered at (one answer a tick).
    rate_answered: Vec<Option<u64>>,
    /// The ticks of the latest drops, for the kick: a sliding window of the last [`REPORT_TICKS`] ticks.
    kick_window: VecDeque<u64>,
    kick_drops: u32,
    dropped_unreliable: u64,
    refusals: Vec<u64>,
    closed: bool,
}

impl RpcLink {
    /// A link for `side` of the connection to `peer` (`None` on a client), over `carrier`, whose buckets start full at tick
    /// `now`. `widths` are the connection's id widths (a ghost reference is as wide as the ghost index).
    pub fn new(
        table: RpcTable,
        side: Side,
        peer: Option<ConnId>,
        widths: NetWidths,
        carrier: Rc<dyn RpcCarrier>,
        now: u64,
    ) -> RpcLink {
        let count = table.len();
        let id_bits = table.id_bits();
        let send_queue = DEFAULT_SEND_QUEUE.min(carrier.capacity());
        RpcLink {
            table,
            side,
            widths,
            carrier,
            peer,
            now,
            id_bits,
            conn_bucket: TokenBucket::new(DEFAULT_CONN_RATE.0, DEFAULT_CONN_RATE.1, now),
            buckets: vec![None; count],
            send_conn_bucket: TokenBucket::new(DEFAULT_CONN_RATE.0, DEFAULT_CONN_RATE.1, now),
            send_buckets: vec![None; count],
            limit_sends: true,
            next_call: 0,
            pending: HashMap::new(),
            tombstones: HashMap::new(),
            tombstone_order: VecDeque::new(),
            outstanding: vec![0; count],
            inbox: VecDeque::new(),
            out_reliable: Vec::new(),
            out_unreliable: Vec::new(),
            queued: 0,
            unreliable_queued: 0,
            send_queue,
            timeout: DEFAULT_TIMEOUT_TICKS,
            drops: vec![DropCounter::default(); count],
            rate_answered: vec![None; count],
            kick_window: VecDeque::new(),
            kick_drops: 0,
            dropped_unreliable: 0,
            refusals: vec![0; count],
            closed: false,
        }
    }

    /// The calls a second the connection may make across all RPCs, and its burst (default 64 and 128), both ways.
    pub fn set_connection_rate(&mut self, per_second: u32, burst: u32) {
        self.conn_bucket = TokenBucket::new(per_second, burst, self.now);
        self.send_conn_bucket = TokenBucket::new(per_second, burst, self.now);
    }

    /// Whether what this side sends is held to the RPCs' rates (the default). A test of the receiving side turns it off to
    /// send what an honest peer would not; only with the `testing` feature, and the receiving side holds its peer to the same
    /// rates anyway.
    #[cfg(any(test, feature = "testing"))]
    pub fn set_send_limits(&mut self, on: bool) {
        self.limit_sends = on;
    }

    /// The cap on the reliable bytes handed over and not acknowledged, plus those waiting (default 64 KiB), held within what the
    /// carrier takes ([`RpcCarrier::capacity`]).
    pub fn set_send_queue(&mut self, bytes: usize) {
        self.send_queue = bytes.min(self.carrier.capacity());
    }

    /// The ticks a call waits for its reply before [`RpcError::Timeout`].
    pub fn set_timeout(&mut self, ticks: u64) {
        self.timeout = ticks;
    }

    /// The drops in a second after which [`RpcLink::over_drop_limit`] says to kick the peer (0: never).
    pub fn set_kick_drops(&mut self, per_second: u32) {
        self.kick_drops = per_second;
        if per_second == 0 {
            self.kick_window.clear();
        }
    }

    /// The id widths of the connection, which a late accept may change.
    pub fn set_widths(&mut self, widths: NetWidths) {
        self.widths = widths;
    }

    /// The id widths messages are written in.
    pub fn widths(&self) -> NetWidths {
        self.widths
    }

    /// The peer.
    pub fn peer(&self) -> Option<ConnId> {
        self.peer
    }

    /// The tick the link is at.
    pub fn now(&self) -> u64 {
        self.now
    }

    /// Moves the clock to `tick` (a tick before the last changes nothing) and fails the calls whose reply is late: their
    /// closures run with [`RpcError::Timeout`] in the next [`take_work`](Self::take_work), and a reply that comes later is
    /// ignored.
    pub fn tick(&mut self, tick: u64) {
        self.now = self.now.max(tick);
        let late: Vec<u16> = self
            .pending
            .iter()
            .filter(|(_, p)| p.deadline <= self.now)
            .map(|(call, _)| *call)
            .collect();
        for call in late {
            self.fail_pending(call, RpcError::Timeout);
        }
        while let Some(&call) = self.tombstone_order.front() {
            match self.tombstones.get(&call) {
                Some(&(_, until)) if until > self.now => break,
                _ => {
                    self.tombstones.remove(&call);
                    self.tombstone_order.pop_front();
                }
            }
        }
        while self
            .kick_window
            .front()
            .is_some_and(|t| self.now.saturating_sub(*t) >= REPORT_TICKS)
        {
            self.kick_window.pop_front();
        }
    }

    fn fail_pending(&mut self, call: u16, error: RpcError) {
        let Some(p) = self.pending.remove(&call) else {
            return;
        };
        if let Some(n) = self.outstanding.get_mut(p.rpc) {
            *n = n.saturating_sub(1);
        }
        if error == RpcError::Timeout {
            // The reply may still be on its way: remember the id, so it is told from a reply to nothing and is not reused.
            let until = self.now.saturating_add(self.timeout.saturating_mul(10));
            self.tombstones.insert(call, (p.rpc, until));
            self.tombstone_order.push_back(call);
            while self.tombstones.len() > MAX_TOMBSTONES {
                if let Some(old) = self.tombstone_order.pop_front() {
                    self.tombstones.remove(&old);
                }
            }
        }
        self.inbox.push_back(Incoming::Reply {
            done: p.done,
            result: Err(error),
        });
    }

    /// The connection ended: every call waiting for a reply fails with [`RpcError::Disconnected`] (in the next
    /// [`take_work`](Self::take_work)), and nothing more is sent.
    pub fn close(&mut self) {
        self.closed = true;
        let calls: Vec<u16> = self.pending.keys().copied().collect();
        for call in calls {
            self.fail_pending(call, RpcError::Disconnected);
        }
        self.out_reliable.clear();
        self.out_unreliable.clear();
        self.queued = 0;
        self.unreliable_queued = 0;
    }

    /// How many calls of RPC `name` this link dropped for going over a rate.
    pub fn drops(&self, name: &str) -> u64 {
        self.table
            .id(name)
            .and_then(|id| self.drops.get(id))
            .map_or(0, DropCounter::total)
    }

    /// How many calls of RPC `name` the receiving side refused (permission or an unknown ghost) on this link.
    pub fn refusals(&self, name: &str) -> u64 {
        self.table
            .id(name)
            .and_then(|id| self.refusals.get(id))
            .copied()
            .unwrap_or(0)
    }

    /// How many unreliable messages were dropped for want of room.
    pub fn unreliable_dropped(&self) -> u64 {
        self.dropped_unreliable
    }

    /// The reliable bytes waiting for hand-over and handed over but not acknowledged.
    pub fn send_queue_bytes(&self) -> usize {
        self.queued.saturating_add(self.carrier.unacked())
    }

    /// Whether the replies waiting to go are far past the send queue: the peer does not read what it is sent, and the host
    /// should drop it.
    pub fn is_overloaded(&self) -> bool {
        self.send_queue_bytes() > self.send_queue.saturating_mul(2)
    }

    /// Whether the peer went over the drop limit in the last second ([`set_kick_drops`](Self::set_kick_drops)).
    pub fn over_drop_limit(&self) -> bool {
        self.kick_drops > 0 && self.kick_window.len() > self.kick_drops as usize
    }

    /// The console lines about dropped calls that are due: one for each RPC with drops since its last line, at most once a
    /// second for each.
    pub fn take_reports(&mut self) -> Vec<String> {
        let who = match self.peer {
            Some(peer) => format!("Client {}", peer.0),
            None => "Server".to_string(),
        };
        let now = self.now;
        let mut lines = Vec::new();
        for (id, counter) in self.drops.iter_mut().enumerate() {
            if let Some(count) = counter.report(now)
                && let Some(entry) = self.table.get(id)
            {
                lines.push(format!(
                    "{who} {}: {count} call(s) dropped (too many calls).",
                    entry.name
                ));
            }
        }
        lines
    }

    /// The typed handle of RPC `name` for message types `Req` and `Resp`, if the table has it with those layouts.
    ///
    /// # Errors
    /// [`RpcError::Unsupported`] when there is no such RPC or its layouts differ.
    pub fn rpc<Req: NetMessage, Resp: NetMessage>(
        &self,
        name: &str,
    ) -> Result<RpcRef<Req, Resp>, RpcError> {
        let entry = self
            .table
            .id(name)
            .and_then(|id| self.table.get(id))
            .ok_or(RpcError::Unsupported)?;
        let want = super::digest_of(
            &super::MessageCodec::of::<Req>(),
            &super::MessageCodec::of::<Resp>(),
            entry.direction,
        );
        if want != entry.digest {
            return Err(RpcError::Unsupported);
        }
        Ok(RpcRef::new(name))
    }

    // ---- sending

    fn sendable(&self, entry: &RpcEntry) -> bool {
        match self.side {
            Side::Server => entry.direction == RpcDirection::ToClient,
            Side::Client => entry.direction == RpcDirection::ToServer,
        }
    }

    /// Spends a token of RPC `id`'s send bucket and the connection's (the connection's only when the RPC's admits).
    fn admit_send(&mut self, id: usize) -> bool {
        if !self.limit_sends {
            return true;
        }
        let now = self.now;
        let Some(entry) = self.table.get(id) else {
            return false;
        };
        let (rate, burst, shared) = (entry.rate, entry.burst, entry.shares_connection_rate);
        let bucket =
            self.send_buckets[id].get_or_insert_with(|| TokenBucket::new(rate, burst, now));
        bucket.take(now) && (!shared || self.send_conn_bucket.take(now))
    }

    /// Frames `message` for RPC `id` and queues it, with `done` waiting for the reply when `entry` has one.
    fn send_erased(
        &mut self,
        id: usize,
        message: &dyn Any,
        done: Option<Reply>,
    ) -> Result<(), RpcError> {
        if self.closed {
            return Err(RpcError::Disconnected);
        }
        let entry = Rc::clone(self.table.get(id).ok_or(RpcError::Unsupported)?);
        if !self.sendable(&entry) {
            return Err(RpcError::Unsupported);
        }
        // What the receiving side would refuse is never sent: a peer that got a message it must refuse would end the connection.
        (entry.request.validate)(message).map_err(|_| RpcError::Invalid)?;
        let call = if entry.reply {
            if self.outstanding.get(id).copied().unwrap_or(0) >= MAX_OUTSTANDING {
                return Err(RpcError::Busy);
            }
            Some(self.free_call_id().ok_or(RpcError::Busy)?)
        } else {
            None
        };
        let kind = if call.is_some() {
            FrameKind::Request
        } else {
            FrameKind::Notify
        };
        let mut w = header(kind, self.id_bits, id, call).map_err(|_| RpcError::Unsupported)?;
        (entry.request.encode)(message, &mut w, self.widths).map_err(|_| RpcError::Unsupported)?;
        if w.is_overflowed() {
            return Err(RpcError::TooLarge);
        }
        let body = w.into_bytes();
        if body.len() > entry.max_bytes {
            return Err(RpcError::TooLarge);
        }
        let cost = body.len() + 2;
        if entry.delivery == Delivery::Unreliable {
            if self.unreliable_queued + cost > UNRELIABLE_ROOM || !self.admit_send(id) {
                self.dropped_unreliable += 1;
                return Ok(());
            }
            self.unreliable_queued += cost;
            self.out_unreliable.push(body);
            return Ok(());
        }
        if self.send_queue_bytes() + cost > self.send_queue {
            return Err(RpcError::Busy);
        }
        if !self.admit_send(id) {
            return Err(RpcError::RateLimited);
        }
        self.queued += cost;
        self.out_reliable.push(body);
        if let Some(call) = call {
            self.outstanding[id] += 1;
            self.pending.insert(
                call,
                Pending {
                    rpc: id,
                    deadline: self.now.saturating_add(self.timeout),
                    done: done.unwrap_or_else(|| Box::new(|_| {})),
                },
            );
        }
        Ok(())
    }

    fn free_call_id(&mut self) -> Option<u16> {
        for _ in 0..=u16::MAX {
            let call = self.next_call;
            self.next_call = self.next_call.wrapping_add(1);
            if !self.pending.contains_key(&call) && !self.tombstones.contains_key(&call) {
                return Some(call);
            }
        }
        None
    }

    /// Calls `rpc` with `request`; `done` runs in a later [`take_work`](Self::take_work) with the reply or the error.
    ///
    /// # Errors
    /// Nothing is sent for: [`RpcError::TooLarge`], [`RpcError::Busy`] (the send queue is full, or the RPC has
    /// [`MAX_OUTSTANDING`] calls waiting), [`RpcError::RateLimited`] (this side's own bucket for the RPC), [`RpcError::Invalid`]
    /// (a float that is not finite), [`RpcError::Unsupported`], [`RpcError::Disconnected`].
    pub fn call<Req: NetMessage, Resp: NetMessage>(
        &mut self,
        rpc: &RpcRef<Req, Resp>,
        request: Req,
        done: impl FnOnce(Result<Resp, RpcError>) + 'static,
    ) -> Result<(), RpcError> {
        let id = self.table.id(rpc.name()).ok_or(RpcError::Unsupported)?;
        let reply: Reply = Box::new(move |result| {
            done(result.and_then(|b| {
                b.downcast::<Resp>()
                    .map(|b| *b)
                    .map_err(|_| RpcError::Unsupported)
            }));
        });
        self.send_erased(id, &request, Some(reply))
    }

    /// Sends the notification `rpc` (no reply, nothing outstanding). On a server this is a `to_client` RPC for one client.
    ///
    /// # Errors
    /// As [`call`](Self::call).
    pub fn notify<Req: NetMessage>(
        &mut self,
        rpc: &RpcRef<Req, ()>,
        request: Req,
    ) -> Result<(), RpcError> {
        self.notify_ref(rpc, &request)
    }

    /// [`notify`](Self::notify) with the message borrowed, for a caller that keeps it when the call fails (a full queue, a rate
    /// that is spent) and tries again.
    ///
    /// # Errors
    /// As [`call`](Self::call).
    pub fn notify_ref<Req: NetMessage>(
        &mut self,
        rpc: &RpcRef<Req, ()>,
        request: &Req,
    ) -> Result<(), RpcError> {
        let id = self.table.id(rpc.name()).ok_or(RpcError::Unsupported)?;
        self.send_erased(id, request, None)
    }

    /// Calls or notifies the RPC `name` with a script's arguments (one for each field of the request); a reply a script call
    /// would get is dropped. A field that names a ghost is an object id in the script: it goes out as the ghost index the object
    /// has on this connection (`ghosts`), and an object the peer has no ghost of is [`RpcError::Invalid`].
    ///
    /// # Errors
    /// As [`call`](Self::call).
    pub fn send_script(
        &mut self,
        name: &str,
        args: &[Vec<u8>],
        ghosts: &dyn GhostResolver,
    ) -> Result<(), RpcError> {
        let id = self.table.id(name).ok_or(RpcError::Unsupported)?;
        let entry = Rc::clone(self.table.get(id).ok_or(RpcError::Unsupported)?);
        let mut message = (entry.request.from_script)(args);
        let peer = self.peer;
        let mapped = (entry.request.map_ghosts)(message.as_mut(), &mut |object| {
            ghosts.index_of(peer, object)
        });
        if !mapped {
            return Err(RpcError::Invalid);
        }
        self.send_erased(id, message.as_ref(), None)
    }

    /// Calls the `to_server` RPC `name` with a script's arguments (one for each field of the request): `done` runs in a later
    /// [`take_work`](Self::take_work) with the reply as script values, or the error; for a notification it runs at once with
    /// no values. Ghost fields go out as with [`send_script`](Self::send_script).
    ///
    /// # Errors
    /// As [`call`](Self::call); nothing is sent and `done` never runs.
    pub fn call_script(
        &mut self,
        name: &str,
        args: &[Vec<u8>],
        ghosts: &dyn GhostResolver,
        done: impl FnOnce(Result<Vec<torque_ghosts::Value>, RpcError>) + 'static,
    ) -> Result<(), RpcError> {
        let id = self.table.id(name).ok_or(RpcError::Unsupported)?;
        let entry = Rc::clone(self.table.get(id).ok_or(RpcError::Unsupported)?);
        let mut message = (entry.request.from_script)(args);
        let peer = self.peer;
        let mapped = (entry.request.map_ghosts)(message.as_mut(), &mut |object| {
            ghosts.index_of(peer, object)
        });
        if !mapped {
            return Err(RpcError::Invalid);
        }
        if !entry.reply {
            self.send_erased(id, message.as_ref(), None)?;
            done(Ok(Vec::new()));
            return Ok(());
        }
        let values = entry.response.values;
        let reply: Reply = Box::new(move |result| {
            done(result.map(|reply| values(reply.as_ref())));
        });
        self.send_erased(id, message.as_ref(), Some(reply))
    }

    /// Hands what is queued to the carrier: the reliable frames in batches of at most [`MAX_BATCH_BYTES`], then the
    /// unreliable ones. Call it once a tick, after the work. A batch the carrier refuses stays queued, with every frame after it
    /// and in order, for the next tick.
    pub fn flush(&mut self) {
        let mut frames: VecDeque<Vec<u8>> = std::mem::take(&mut self.out_reliable).into();
        let mut kept: Vec<Vec<u8>> = Vec::new();
        while !frames.is_empty() {
            let mut batch = Vec::new();
            let mut taken = Vec::new();
            while let Some(body) = frames.pop_front() {
                let Ok(len) = u16::try_from(body.len()) else {
                    continue;
                };
                if !batch.is_empty() && batch.len() + body.len() + 2 > MAX_BATCH_BYTES {
                    frames.push_front(body);
                    break;
                }
                batch.extend_from_slice(&len.to_le_bytes());
                batch.extend_from_slice(&body);
                taken.push(body);
            }
            if self.carrier.send(batch, Delivery::Reliable) == Err(CarrierFull) {
                kept = taken;
                kept.extend(frames.drain(..));
                break;
            }
        }
        self.queued = kept.iter().map(|f| f.len() + 2).sum();
        self.out_reliable = kept;
        let frames = std::mem::take(&mut self.out_unreliable);
        self.unreliable_queued = 0;
        let mut batch = Vec::new();
        for body in frames {
            let Ok(len) = u16::try_from(body.len()) else {
                continue;
            };
            batch.extend_from_slice(&len.to_le_bytes());
            batch.extend_from_slice(&body);
        }
        if !batch.is_empty() && self.carrier.send(batch, Delivery::Unreliable).is_err() {
            self.dropped_unreliable += 1;
        }
    }

    // ---- receiving

    /// Reads a batch from the peer.
    ///
    /// # Errors
    /// An `Err` means the peer sent what no honest peer of this engine does (an RPC it may not call, a message over its size or
    /// its caps, a reply to nothing): the connection should be dropped.
    pub fn receive(&mut self, batch: &[u8]) -> Result<(), NetError> {
        if batch.len() > MAX_BATCH_BYTES {
            return Err(NetError::Invalid("rpc batch over its size"));
        }
        let mut at = 0;
        while at < batch.len() {
            let head = batch.get(at..at + 2).ok_or(NetError::Overrun)?;
            let len = usize::from(u16::from_le_bytes([head[0], head[1]]));
            at += 2;
            let body = batch.get(at..at + len).ok_or(NetError::Overrun)?;
            at += len;
            self.receive_frame(body)?;
        }
        Ok(())
    }

    /// Spends a token of RPC `id`'s bucket and then the connection's: the connection is charged only for a call its RPC admits.
    fn admit(&mut self, id: usize) -> bool {
        let now = self.now;
        let Some(entry) = self.table.get(id) else {
            return false;
        };
        let (rate, burst, shared) = (entry.rate, entry.burst, entry.shares_connection_rate);
        let bucket = self.buckets[id].get_or_insert_with(|| TokenBucket::new(rate, burst, now));
        bucket.take(now) && (!shared || self.conn_bucket.take(now))
    }

    fn note_drop(&mut self, id: usize) {
        self.drops[id].note();
        if self.kick_drops > 0 {
            self.kick_window.push_back(self.now);
            // Only the newest `kick_drops + 1` matter for whether the window holds more than `kick_drops`.
            while self.kick_window.len() > self.kick_drops as usize + 1 {
                self.kick_window.pop_front();
            }
        }
    }

    fn receive_frame(&mut self, body: &[u8]) -> Result<(), NetError> {
        let mut r = BitReader::new(body);
        let kind = FrameKind::from_bits(r.read_bits(2)?);
        let id = r.read_bits(self.id_bits)? as usize;
        let entry = Rc::clone(
            self.table
                .get(id)
                .ok_or(NetError::Invalid("an RPC the table does not have"))?,
        );
        match kind {
            FrameKind::Request | FrameKind::Notify => {
                self.receive_call(&mut r, kind, id, &entry, body.len())
            }
            FrameKind::Reply | FrameKind::Error => {
                self.receive_answer(&mut r, kind, id, &entry, body.len())
            }
        }
    }

    fn receive_call(
        &mut self,
        r: &mut BitReader<'_>,
        kind: FrameKind,
        id: usize,
        entry: &RpcEntry,
        len: usize,
    ) -> Result<(), NetError> {
        let mine = match self.side {
            Side::Server => RpcDirection::ToServer,
            Side::Client => RpcDirection::ToClient,
        };
        if entry.direction != mine {
            return Err(NetError::Invalid("an RPC sent the wrong way"));
        }
        if (kind == FrameKind::Request) != entry.reply {
            return Err(NetError::Invalid("an RPC of the wrong kind"));
        }
        if len > entry.max_bytes {
            return Err(NetError::Invalid("an RPC message over its size"));
        }
        let call = if kind == FrameKind::Request {
            Some(r.read_bits(16)? as u16)
        } else {
            None
        };
        if !self.admit(id) {
            self.note_drop(id);
            if let Some(call) = call
                && self.rate_answered[id] != Some(self.now)
            {
                self.rate_answered[id] = Some(self.now);
                if let Some(frame) = error_frame(self.id_bits, id, call, &RpcError::RateLimited) {
                    self.queued += frame.len() + 2;
                    self.out_reliable.push(frame);
                }
            }
            return Ok(());
        }
        let message = (entry.request.decode)(r, self.widths)?;
        if r.bits_left() >= 8 {
            return Err(NetError::Invalid("bytes after an RPC message"));
        }
        self.inbox.push_back(Incoming::Call {
            rpc: id,
            call,
            message,
        });
        Ok(())
    }

    fn receive_answer(
        &mut self,
        r: &mut BitReader<'_>,
        kind: FrameKind,
        id: usize,
        entry: &RpcEntry,
        len: usize,
    ) -> Result<(), NetError> {
        if self.side == Side::Server {
            return Err(NetError::Invalid("a reply to a call the server never made"));
        }
        // A reply is held to its RPC's own size; an error is a header and at most MAX_ERROR_TEXT bytes of text.
        let cap = if kind == FrameKind::Error {
            MAX_ERROR_TEXT + 16
        } else {
            entry.max_bytes
        };
        if len > cap {
            return Err(NetError::Invalid("an RPC reply over its size"));
        }
        let call = r.read_bits(16)? as u16;
        if let Some(&(rpc, _)) = self.tombstones.get(&call) {
            // The late answer to a call that timed out: nothing waits for it any more.
            if rpc != id {
                return Err(NetError::Invalid("a reply of another RPC"));
            }
            self.tombstones.remove(&call);
            self.tombstone_order.retain(|c| *c != call);
            return Ok(());
        }
        let rpc = self
            .pending
            .get(&call)
            .map(|p| p.rpc)
            .ok_or(NetError::Invalid("a reply to a call nobody made"))?;
        if rpc != id {
            return Err(NetError::Invalid("a reply of another RPC"));
        }
        let result = if kind == FrameKind::Reply {
            Ok((entry.response.decode)(r, self.widths)?)
        } else {
            Err(read_error(r)?)
        };
        if r.bits_left() >= 8 {
            return Err(NetError::Invalid("bytes after an RPC reply"));
        }
        if let Some(p) = self.pending.remove(&call) {
            self.outstanding[id] = self.outstanding[id].saturating_sub(1);
            self.inbox.push_back(Incoming::Reply {
                done: p.done,
                result,
            });
        }
        Ok(())
    }

    // ---- the work

    /// Everything received (and every timeout) as work to run, in the order it arrived. Run each with [`Work::run`] and give
    /// the result to [`complete`](Self::complete); the link is free while they run.
    pub fn take_work(&mut self) -> Vec<Work> {
        let items = std::mem::take(&mut self.inbox);
        items
            .into_iter()
            .filter_map(|item| match item {
                Incoming::Call { rpc, call, message } => {
                    let entry = Rc::clone(self.table.get(rpc)?);
                    Some(Work::Call(CallWork {
                        entry,
                        call,
                        message,
                        id: rpc,
                        id_bits: self.id_bits,
                        widths: self.widths,
                        side: self.side,
                    }))
                }
                Incoming::Reply { done, result } => Some(Work::Reply { done, result }),
            })
            .collect()
    }

    /// Takes in what a run work left: the frame of its answer (counted against the send queue), and a refusal to count.
    pub fn complete(&mut self, done: Done) {
        if let Some(id) = done.refused
            && let Some(n) = self.refusals.get_mut(id)
        {
            *n = n.saturating_add(1);
        }
        if let Some(frame) = done.frame
            && !self.closed
        {
            self.queued += frame.len() + 2;
            self.out_reliable.push(frame);
        }
    }

    /// Runs the work now with `env` (the work and the link together, for a caller that has nothing to run beside it).
    pub fn dispatch(&mut self, env: &mut RpcEnv<'_>) {
        for work in self.take_work() {
            let done = work.run(env);
            self.complete(done);
        }
    }
}

fn read_error(r: &mut BitReader<'_>) -> Result<RpcError, NetError> {
    match ErrorCode::from_bits(r.read_bits(2)?) {
        Some(ErrorCode::Refused) => Ok(RpcError::Refused),
        Some(ErrorCode::RateLimited) => Ok(RpcError::RateLimited),
        Some(ErrorCode::Handler) => {
            let len = usize::from(r.read_u8()?);
            if len > MAX_ERROR_TEXT || r.bits_left() < len * 8 {
                return Err(NetError::Overrun);
            }
            let bytes = r.read_bytes(len)?;
            Ok(RpcError::Handler(
                String::from_utf8_lossy(&bytes).into_owned(),
            ))
        }
        None => Err(NetError::Invalid("an RPC error code")),
    }
}

#[cfg(test)]
impl RpcLink {
    /// The ids of the calls waiting for a reply.
    pub(crate) fn pending_calls(&self) -> Vec<u16> {
        self.pending.keys().copied().collect()
    }

    /// The ids of the calls that timed out and may still be answered.
    pub(crate) fn tombstoned_calls(&self) -> Vec<u16> {
        self.tombstones.keys().copied().collect()
    }

    /// How many ids the tombstones' order holds (it holds each live tombstone once).
    pub(crate) fn tombstone_order_len(&self) -> usize {
        self.tombstone_order.len()
    }

    /// Makes `call` the next id tried.
    pub(crate) fn set_next_call(&mut self, call: u16) {
        self.next_call = call;
    }
}
