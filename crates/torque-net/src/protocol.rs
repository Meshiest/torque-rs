//! The packet layer (`ConnectionProtocol` and the packet-rate part of `NetConnection`): sequence numbers, the ack mask, the send window, the
//! notify queue, rates, the ping timeout and the send gate. A pure state machine: `now` is the caller's sim clock in milliseconds
//! (never read from the OS), bytes go in and out, and nothing here opens a socket or runs a script.
//!
//! **Header** (bit-exact): flag 1, `connectSeq & 1` (1 bit), packet sequence (9), highest ack (9), type (2: 0 data, 1 ping, 2 ack, 3
//! invalid), ack byte count (3, at most 4), then `8 * count` bits of ack mask. A data packet increments the send sequence; ping and
//! ack packets carry the current one. **Data body, before the game payload:** flag, then 10-bit delay and 10-bit size (the sender's
//! current rate), flag, then 10 + 10 bits (its maximum rate).
//!
//! **Receive rules.** The 9-bit sequence is widened against the last one received by its forward distance modulo 512 (`wrapping_sub`,
//! never a wire value as an index); a distance above 31 is dropped. The ack is widened the same way against the highest acked one and
//! is dropped when it is past the last packet sent. A dropped packet changes nothing. The send window is 30 packets. The notify queue
//! is an `Option`-popped queue: a hostile ack can never pop an empty one.
//!
//! **Rates** (`checkMaxRate`): the preferences are clamped (packets a second to the server 8..=32, to the client 1..=32, the packet
//! size 100..=450, [`RateConfig::new`]); a new connection's current rate is 102 ms and 200 bytes, its maximum `1024 / toClientRate`
//! ms and the packet size. The send gate (`checkPacketSend`) waits the rate's delay with up to a second of credit; a game may stretch
//! the delay or force a packet out through a [`SendPacing`].
//!
//! **Addition:** the peer's current rate is clamped to this side's maximum (delay at least the
//! maximum's and one tick, size between [`NET_MIN_PACKET_BYTES`] and the maximum's), where Torque takes the first pair raw.
//!
//! Credit: where Torque3D has the same rules, this module follows its MIT-licensed `sim/netConnection.cpp` (Copyright (c) 2012
//! GarageGames, LLC) `sim/netConnection.h` (Copyright (c) 2012 GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.), and `core/dnet.cpp` and
//! `core/dnet.h` (Copyright (c) 2012 GarageGames, LLC), where `ConnectionProtocol` lives; the code here is this crate's own.

use std::collections::VecDeque;
use std::fmt;

use crate::NetError;
use crate::bits::{BitReader, BitWriter};
use crate::cap::{NET_MAX_DATAGRAM, NET_MIN_PACKET_BYTES, NET_TIMEOUT_MS, NET_TIMEOUT_TRIES};

/// The send window: no data packet is built while this many are unacknowledged.
pub const SEND_WINDOW: u32 = 30;

/// The furthest ahead of the last packet received a sequence number may be (31); anything else is dropped.
pub const ACCEPT_DISTANCE: u32 = 31;

/// A server tick, in milliseconds (the shortest delay a peer may ask the server for).
pub const TICK_MS: u32 = 32;

/// The largest packet size a rate record carries (its 10-bit field): 1,023 bytes.
pub const MAX_PACKET_BYTES: u32 = 0x3ff;

/// The rate a new connection starts with (`NetConnection`'s constructor): 102 ms between packets, 200 bytes a packet.
pub const START_DELAY_MS: u32 = 102;
pub const START_PACKET_BYTES: u32 = 200;

/// The largest size a connection whose packets stay in memory may fill them to ([`Protocol::set_in_memory`]): its own client reads
/// them whole, so the datagram bound does not apply.
pub const MAX_IN_MEMORY_PACKET_BYTES: u32 = 0xffff;

/// What a packet is: the 2-bit type of the header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PacketType {
    Data = 0,
    Ping = 1,
    Ack = 2,
}

impl PacketType {
    fn from_bits(bits: u32) -> Option<PacketType> {
        match bits {
            0 => Some(PacketType::Data),
            1 => Some(PacketType::Ping),
            2 => Some(PacketType::Ack),
            _ => None,
        }
    }
}

/// Which end of the connection this is: it picks the rates and the send gate's delay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Server,
    Client,
}

/// One rate record: `{updateDelay, packetSize, changed}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rate {
    /// Milliseconds between packets.
    pub delay: u32,
    /// Bytes per packet.
    pub size: u32,
    /// Whether the peer has still to be told.
    pub changed: bool,
}

/// The packet rates a process was configured with (`$pref::Net::PacketRateToServer`/`ToClient`, `$pref::Net::PacketSize`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateConfig {
    /// Packets a second a client sends to the server.
    pub to_server_rate: u32,
    /// Packets a second the server sends to a client.
    pub to_client_rate: u32,
    /// The packet size of the maximum rate, in bytes (at most [`MAX_PACKET_BYTES`] reach the wire's rate record).
    pub packet_size: u32,
}

impl RateConfig {
    /// `checkMaxRate`: the preferences clamped (to-server 8..=32, to-client 1..=32, packet size 100..=450).
    pub fn new(pref_to_server: u32, pref_to_client: u32, pref_packet_size: u32) -> RateConfig {
        RateConfig {
            to_server_rate: pref_to_server.clamp(8, 32),
            to_client_rate: pref_to_client.clamp(1, 32),
            packet_size: pref_packet_size.clamp(100, 450),
        }
    }

    /// The standard preferences: 32 packets a second to the server, 10 to the client, 200 bytes.
    pub fn standard() -> RateConfig {
        RateConfig::new(32, 10, START_PACKET_BYTES)
    }
}

impl Default for RateConfig {
    fn default() -> RateConfig {
        RateConfig::standard()
    }
}

/// What a game adds to the send gate (`checkPacketSend`). TGE's gate (no pacing) waits the rate's delay since the last packet, with
/// up to a second of credit, and never forces a packet. A connection keeps its pacing ([`Protocol::set_pacing`]) and asks it at
/// every gate call, so a pacing may keep state between calls.
pub trait SendPacing: fmt::Debug + Send + Sync {
    /// The delay (ms) this gate call waits since the last data packet, from the rate's `delay`, and whether the packet goes whatever
    /// the delay (the send window still holds). `round_trip` is the connection's running round trip (ms); `local`, whether it is
    /// the in-process connection ([`Protocol::set_local`]).
    fn pace(&mut self, delay: u32, round_trip: f32, local: bool) -> (u32, bool);

    /// A copy for another connection (a server hands every new connection a copy of its configured pacing).
    fn clone_box(&self) -> Box<dyn SendPacing>;
}

impl Clone for Box<dyn SendPacing> {
    fn clone(&self) -> Box<dyn SendPacing> {
        self.clone_box()
    }
}

/// A fixed-field packet header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    /// `connectSeq & 1`.
    pub connect_bit: bool,
    /// The sender's sequence number, 9 bits.
    pub seq: u16,
    /// The highest packet the sender has received, 9 bits.
    pub ack: u16,
    pub ptype: PacketType,
    /// How many ack mask bytes follow (0..=4).
    pub ack_bytes: u8,
    /// The ack mask: bit `k` is the packet `ack - k`.
    pub ack_mask: u32,
}

impl Header {
    /// Writes the header: flag 1, then the fields.
    pub fn write(&self, w: &mut BitWriter) {
        w.write_flag(true);
        w.write_flag(self.connect_bit);
        w.write_bits(u32::from(self.seq), 9);
        w.write_bits(u32::from(self.ack), 9);
        w.write_bits(self.ptype as u32, 2);
        let bytes = self.ack_bytes.min(4);
        w.write_bits(u32::from(bytes), 3);
        w.write_bits(self.ack_mask, bytes * 8);
    }

    /// Reads a header. Type 3 and an ack byte count above 4 are [`NetError::Invalid`]; a short stream is [`NetError::Overrun`].
    pub fn read(r: &mut BitReader) -> Result<Header, NetError> {
        r.read_flag()?;
        let connect_bit = r.read_flag()?;
        let seq = r.read_bits(9)? as u16;
        let ack = r.read_bits(9)? as u16;
        let ptype =
            PacketType::from_bits(r.read_bits(2)?).ok_or(NetError::Invalid("packet type"))?;
        let ack_bytes = r.read_bits(3)? as u8;
        if ack_bytes > 4 {
            return Err(NetError::Invalid("ack byte count"));
        }
        let ack_mask = r.read_bits(ack_bytes * 8)?;
        Ok(Header {
            connect_bit,
            seq,
            ack,
            ptype,
            ack_bytes,
            ack_mask,
        })
    }
}

/// The two optional rate pairs at the start of a data packet's body, as they are on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RatePrefix {
    /// The sender's current rate (delay, size), when it changed.
    pub cur: Option<(u32, u32)>,
    /// The sender's maximum rate (delay, size), when it changed.
    pub max: Option<(u32, u32)>,
}

impl RatePrefix {
    pub fn write(&self, w: &mut BitWriter) {
        for pair in [self.cur, self.max] {
            w.write_flag(pair.is_some());
            if let Some((delay, size)) = pair {
                w.write_bits(delay.min(0x3ff), 10);
                w.write_bits(size.min(0x3ff), 10);
            }
        }
    }

    pub fn read(r: &mut BitReader) -> Result<RatePrefix, NetError> {
        let mut pairs = [None, None];
        for slot in &mut pairs {
            if r.read_flag()? {
                let delay = r.read_bits(10)?;
                let size = r.read_bits(10)?;
                *slot = Some((delay, size));
            }
        }
        Ok(RatePrefix {
            cur: pairs[0],
            max: pairs[1],
        })
    }
}

/// Why a packet was dropped without changing anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dropped {
    /// The header ended early, or named type 3 or more than 4 ack bytes.
    Malformed,
    /// The connect-sequence bit is not this connection's.
    ConnectBit,
    /// The sequence number is more than 31 ahead of the last one received (or behind it).
    Window,
    /// The ack names a packet that was never sent.
    AckPastSend,
}

/// The fate of one data packet this side sent, learned from the peer's ack mask.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Notify {
    pub send_seq: u32,
    pub delivered: bool,
}

/// What one received datagram did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Received {
    /// Set when the packet was dropped; nothing else in this value is meaningful then.
    pub dropped: Option<Dropped>,
    pub ptype: Option<PacketType>,
    /// The widened sequence number of the packet.
    pub seq: u32,
    /// The packets of ours this one acked or nacked, in sequence order.
    pub notifies: Vec<Notify>,
    /// A new data packet: its rate prefix was read and the reader is at the payload.
    pub payload: bool,
    /// A ping: the caller sends [`Protocol::ack_packet`] at once.
    pub answer_ping: bool,
}

impl Received {
    fn dropped(why: Dropped) -> Received {
        Received {
            dropped: Some(why),
            ptype: None,
            seq: 0,
            notifies: Vec::new(),
            payload: false,
            answer_ping: false,
        }
    }
}

/// What a timeout check decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Timeout {
    Alive,
    /// Idle for [`NET_TIMEOUT_MS`]: send [`Protocol::ping_packet`].
    SendPing,
    /// [`NET_TIMEOUT_TRIES`] pings went unanswered: the connection is dead (`TimedOut`).
    TimedOut,
}

#[derive(Debug, Clone, Copy)]
struct Note {
    send_time: u32,
    /// The highest packet received at the time this one was sent (what its header acked).
    recv_seq: u32,
    rate_changed: bool,
    max_rate_changed: bool,
}

/// `a` is after `b` in sequence space.
fn after(a: u32, b: u32) -> bool {
    (a.wrapping_sub(b) as i32) > 0
}

/// The state of one connection's packet layer.
#[derive(Debug, Clone)]
pub struct Protocol {
    role: Role,
    connect_sequence: u32,
    last_seq_recvd: u32,
    highest_acked: u32,
    last_send_seq: u32,
    ack_mask: u32,
    last_recv_ack_ack: u32,
    notifies: VecDeque<Note>,
    established: bool,
    cur: Rate,
    max: Rate,
    /// The last current and maximum rates the peer sent, as the wire carried them (the client's load report).
    peer_cur: Option<(u32, u32)>,
    peer_max: Option<(u32, u32)>,
    min_delay: u32,
    to_server_delay: u32,
    local: bool,
    /// The size the sections fill a packet to, in place of the current rate's ([`Protocol::set_packet_bytes`]).
    packet_bytes: Option<u32>,
    /// The packets never leave memory ([`Protocol::set_in_memory`]).
    in_memory: bool,
    /// When the last data packet was built; a new connection starts at 0, so its first packet waits one delay.
    last_send_time: u32,
    credit: i64,
    /// What a game adds to the send gate; `None` is TGE's gate.
    pacing: Option<Box<dyn SendPacing>>,
    round_trip: f32,
    last_ping_time: u32,
    ping_tries_left: u32,
}

impl Protocol {
    /// A new connection. The sequence numbers all start at 0; the rates start as `NetConnection`'s constructor and `checkMaxRate`
    /// leave them (current 102 ms and 200 bytes, unchanged; maximum `{1024 / toClientRate, packet size}`, changed when it differs
    /// from the current one).
    pub fn new(role: Role, connect_sequence: u32, rates: RateConfig, now: u32) -> Protocol {
        let max_delay = 1024 / rates.to_client_rate.max(1);
        Protocol {
            role,
            connect_sequence,
            last_seq_recvd: 0,
            highest_acked: 0,
            last_send_seq: 0,
            ack_mask: 0,
            last_recv_ack_ack: 0,
            notifies: VecDeque::new(),
            established: false,
            cur: Rate {
                delay: START_DELAY_MS,
                size: START_PACKET_BYTES,
                changed: false,
            },
            max: Rate {
                delay: max_delay,
                size: rates.packet_size,
                changed: (max_delay, rates.packet_size) != (START_DELAY_MS, START_PACKET_BYTES),
            },
            peer_cur: None,
            peer_max: None,
            min_delay: match role {
                Role::Server => TICK_MS,
                Role::Client => 1,
            },
            to_server_delay: 1024 / rates.to_server_rate.max(1),
            local: false,
            packet_bytes: None,
            in_memory: false,
            last_send_time: 0,
            credit: 0,
            pacing: None,
            round_trip: 0.0,
            last_ping_time: now,
            ping_tries_left: NET_TIMEOUT_TRIES,
        }
    }

    pub fn role(&self) -> Role {
        self.role
    }

    pub fn connect_sequence(&self) -> u32 {
        self.connect_sequence
    }

    /// Marks the connection as the in-process local one (its [`SendPacing`] is told; TGE's gate treats it as any other).
    pub fn set_local(&mut self, local: bool) {
        self.local = local;
    }

    /// What a game adds to this connection's send gate; `None` is TGE's gate.
    pub fn set_pacing(&mut self, pacing: Option<Box<dyn SendPacing>>) {
        self.pacing = pacing;
    }

    /// The connection's pacing, if it has one.
    pub fn pacing(&self) -> Option<&dyn SendPacing> {
        self.pacing.as_deref()
    }

    pub fn last_seq_recvd(&self) -> u32 {
        self.last_seq_recvd
    }

    pub fn highest_acked(&self) -> u32 {
        self.highest_acked
    }

    pub fn last_send_seq(&self) -> u32 {
        self.last_send_seq
    }

    /// Data packets sent and not yet acked or nacked.
    pub fn unacked(&self) -> usize {
        self.notifies.len()
    }

    /// Whether a first data packet of ours was acked (`setEstablished` in Torque's terms).
    pub fn acked_once(&self) -> bool {
        self.established
    }

    pub fn cur_rate(&self) -> Rate {
        self.cur
    }

    pub fn max_rate(&self) -> Rate {
        self.max
    }

    /// The last current rate the peer sent (delay, size), raw.
    pub fn peer_cur_rate(&self) -> Option<(u32, u32)> {
        self.peer_cur
    }

    /// The last maximum rate the peer sent (delay, size), raw.
    pub fn peer_max_rate(&self) -> Option<(u32, u32)> {
        self.peer_max
    }

    /// The delay (ms) between this side's data packets as a client asks for it (`1024 / toServerRate`).
    pub fn to_server_delay(&self) -> u32 {
        self.to_server_delay
    }

    /// The running round trip, milliseconds.
    pub fn round_trip(&self) -> f32 {
        self.round_trip
    }

    /// Whether the send window is full (30 data packets unacknowledged).
    pub fn window_full(&self) -> bool {
        self.last_send_seq.wrapping_sub(self.highest_acked) >= SEND_WINDOW
    }

    /// `bufferSize * 8` for the current rate: what the event section's loops test against (`SectionParams::packet_bits`). The
    /// packet stream itself takes 12,000 bits whatever the rate, see [`Protocol::begin_data_packet`].
    pub fn packet_limit_bits(&self) -> usize {
        match self.packet_bytes {
            Some(bytes) => self.packet_bytes_cap(bytes) as usize * 8,
            None => self.cur.size.min(MAX_PACKET_BYTES) as usize * 8,
        }
    }

    /// `bytes` within what the connection's stream allows: [`MAX_IN_MEMORY_PACKET_BYTES`] in memory, the datagram bound otherwise.
    fn packet_bytes_cap(&self, bytes: u32) -> u32 {
        if self.in_memory {
            bytes.min(MAX_IN_MEMORY_PACKET_BYTES)
        } else {
            bytes.min(NET_MAX_DATAGRAM as u32)
        }
    }

    /// The connection's packets stay in memory (the server half of the in-process local connection):
    /// [`set_packet_bytes`](Self::set_packet_bytes) may then ask up to [`MAX_IN_MEMORY_PACKET_BYTES`], and the packet stream is that
    /// much longer than the datagram bound, so a record may pass the size by as much as one may at any size.
    pub fn set_in_memory(&mut self, on: bool) {
        self.in_memory = on;
    }

    /// The bits a data packet's stream holds: the datagram bound, and for a connection in memory the size it fills its packets to
    /// on top.
    fn stream_bits(&self) -> usize {
        match self.packet_bytes {
            Some(bytes) if self.in_memory => {
                (NET_MAX_DATAGRAM + self.packet_bytes_cap(bytes) as usize) * 8
            }
            _ => NET_MAX_DATAGRAM * 8,
        }
    }

    /// The sections fill a packet to `bytes` (at most the datagram bound, or [`MAX_IN_MEMORY_PACKET_BYTES`] in memory) whatever the
    /// rate's size; None is the standard rule (the current rate's size). The rate prefix still carries the 10-bit sizes, so the peer
    /// sees nothing new but fuller packets. A profile raises it for the connections of its handshake extension
    /// ([`crate::profile::HandshakeExtension::packet_bytes`]); a standard receiver reads a packet of up to its 1,500-byte buffer.
    pub fn set_packet_bytes(&mut self, bytes: Option<u32>) {
        self.packet_bytes = bytes;
    }

    /// Sets the current rate from the peer's request, bounded by this side's own maximum.
    fn set_cur(&mut self, delay: u32, size: u32) {
        let lowest = self.max.delay.max(self.min_delay);
        self.cur.delay = delay.max(lowest);
        self.cur.size = size.clamp(NET_MIN_PACKET_BYTES.min(self.max.size), self.max.size);
    }

    /// Reads the rate prefix of a data packet and adopts it. The peer's current rate is taken through the clamp; its maximum rate
    /// sets ours to the slower of the two and marks the current rate changed only when that changed it.
    fn read_rate_prefix(&mut self, r: &mut BitReader) -> Result<(), NetError> {
        let prefix = RatePrefix::read(r)?;
        if let Some((delay, size)) = prefix.cur {
            self.peer_cur = Some((delay, size));
            self.set_cur(delay, size);
        }
        if let Some((delay, size)) = prefix.max {
            self.peer_max = Some((delay, size));
            let before = (self.cur.delay, self.cur.size);
            self.set_cur(delay.max(self.max.delay), size.min(self.max.size));
            if before != (self.cur.delay, self.cur.size) {
                self.cur.changed = true;
            }
        }
        Ok(())
    }

    /// Writes the rate prefix and records on the newest notify which pairs went out (a dropped packet re-arms them).
    fn write_rate_prefix(&mut self, w: &mut BitWriter) {
        let prefix = RatePrefix {
            cur: self.cur.changed.then_some((self.cur.delay, self.cur.size)),
            max: self.max.changed.then_some((self.max.delay, self.max.size)),
        };
        prefix.write(w);
        if let Some(note) = self.notifies.back_mut() {
            note.rate_changed = prefix.cur.is_some();
            note.max_rate_changed = prefix.max.is_some();
        }
        if prefix.cur.is_some() {
            self.cur.changed = false;
        }
        if prefix.max.is_some() {
            self.max.changed = false;
        }
    }

    /// Writes a header of the given type. A data packet takes the next send sequence and a notify record.
    pub fn write_header(&mut self, w: &mut BitWriter, ptype: PacketType, now: u32) {
        let ack_bytes = self
            .last_seq_recvd
            .wrapping_sub(self.last_recv_ack_ack)
            .wrapping_add(7)
            >> 3;
        let ack_bytes = ack_bytes.min(4) as u8;
        if ptype == PacketType::Data {
            self.last_send_seq = self.last_send_seq.wrapping_add(1);
            self.last_send_time = now;
            self.notifies.push_back(Note {
                send_time: now,
                recv_seq: self.last_seq_recvd,
                rate_changed: false,
                max_rate_changed: false,
            });
        }
        let mask = if ack_bytes == 4 {
            self.ack_mask
        } else {
            self.ack_mask & ((1u32 << (u32::from(ack_bytes) * 8)) - 1)
        };
        Header {
            connect_bit: self.connect_sequence & 1 != 0,
            seq: (self.last_send_seq & 0x1ff) as u16,
            ack: (self.last_seq_recvd & 0x1ff) as u16,
            ptype,
            ack_bytes,
            ack_mask: mask,
        }
        .write(w);
    }

    /// Starts a data packet: the header and the rate prefix, in a writer limited to 12,000 bits (`NET_MAX_DATAGRAM` bytes), as the packet
    /// stream is whatever the rate's size (the size only feeds `eventWritePacket`'s tests). The caller appends the game payload.
    /// The rate's size is [`Protocol::packet_limit_bits`].
    /// (The writer is cut at the datagram bound, which a packet never exceeds.)
    pub fn begin_data_packet(&mut self, now: u32) -> BitWriter {
        debug_assert!(
            !self.window_full(),
            "a data packet built with the window full"
        );
        let mut w = BitWriter::with_limit(self.stream_bits());
        self.write_header(&mut w, PacketType::Data, now);
        self.write_rate_prefix(&mut w);
        w
    }

    /// A ping packet (header only).
    pub fn ping_packet(&mut self, now: u32) -> Vec<u8> {
        let mut w = BitWriter::new();
        self.write_header(&mut w, PacketType::Ping, now);
        w.into_bytes()
    }

    /// An ack packet (header only).
    pub fn ack_packet(&mut self, now: u32) -> Vec<u8> {
        let mut w = BitWriter::new();
        self.write_header(&mut w, PacketType::Ack, now);
        w.into_bytes()
    }

    /// The send gate (`checkPacketSend`): whether a data packet may be built now. The delay is the rate's (the client's
    /// `1024 / toServerRate`, the server's current rate), as the connection's [`SendPacing`] changes it; `force` skips the delay, not
    /// the window. The caller builds the packet with [`Protocol::begin_data_packet`] when this says yes.
    pub fn may_send(&mut self, now: u32, force: bool) -> bool {
        let mut delay = match self.role {
            Role::Client => self.to_server_delay,
            Role::Server => self.cur.delay,
        };
        let mut force = force;
        if let Some(pacing) = self.pacing.as_mut() {
            let (paced, forced) = pacing.pace(delay, self.round_trip, self.local);
            delay = paced;
            force |= forced;
        }
        if !force {
            let elapsed = i64::from(now.wrapping_sub(self.last_send_time) as i32);
            if elapsed + self.credit < i64::from(delay) {
                return false;
            }
            self.credit = (self.credit + elapsed - i64::from(delay)).min(1000);
        }
        !self.window_full()
    }

    /// Idle-time check (`checkTimeout`): a ping after 4,500 ms of silence, `TimedOut` after 15 unanswered ones. Any valid packet
    /// received resets it.
    pub fn check_timeout(&mut self, now: u32) -> Timeout {
        if now.wrapping_sub(self.last_ping_time) <= NET_TIMEOUT_MS {
            return Timeout::Alive;
        }
        if self.ping_tries_left > 0 {
            self.ping_tries_left -= 1;
            self.last_ping_time = now;
            Timeout::SendPing
        } else {
            Timeout::TimedOut
        }
    }

    fn keep_alive(&mut self, now: u32) {
        self.last_ping_time = now;
        self.ping_tries_left = NET_TIMEOUT_TRIES;
    }

    /// One packet of ours was acked (`delivered`) or lost: pops its record. An empty queue pops nothing.
    fn handle_notify(&mut self, send_seq: u32, delivered: bool, now: u32) -> Option<Notify> {
        let note = self.notifies.pop_front()?;
        if delivered {
            self.established = true;
            let trip = now.wrapping_sub(note.send_time) as f32;
            self.round_trip = (self.round_trip + trip) * 0.5;
            if after(note.recv_seq, self.last_recv_ack_ack) {
                self.last_recv_ack_ack = note.recv_seq;
            }
        } else {
            if note.rate_changed {
                self.cur.changed = true;
            }
            if note.max_rate_changed {
                self.max.changed = true;
            }
        }
        Some(Notify {
            send_seq,
            delivered,
        })
    }

    /// The header checks of `receive` without changing anything: `(sequence distance, widened sequence, widened ack)`.
    fn admit(&self, header: &Header) -> Result<(u32, u32, u32), Dropped> {
        if header.connect_bit != (self.connect_sequence & 1 != 0) {
            return Err(Dropped::ConnectBit);
        }
        let distance = u32::from(header.seq).wrapping_sub(self.last_seq_recvd) & 0x1ff;
        if distance > ACCEPT_DISTANCE {
            return Err(Dropped::Window);
        }
        let seq = self.last_seq_recvd.wrapping_add(distance);
        let ack_distance = u32::from(header.ack).wrapping_sub(self.highest_acked) & 0x1ff;
        let ack = self.highest_acked.wrapping_add(ack_distance);
        if after(ack, self.last_send_seq) {
            return Err(Dropped::AckPastSend);
        }
        Ok((distance, seq, ack))
    }

    /// Whether `receive` would take this datagram's header, decided without changing anything (a caller that rate-limits
    /// connections counts only the datagrams that pass, so packets with a forged header cost the real peer nothing).
    pub fn precheck(&self, bytes: &[u8]) -> Result<(), Dropped> {
        let header = Header::read(&mut BitReader::new(bytes)).map_err(|_| Dropped::Malformed)?;
        self.admit(&header).map(|_| ())
    }

    /// Processes one received packet (`processRawPacket` and the rate prefix of `handlePacket`). The reader is at the first bit of the
    /// datagram; when [`Received::payload`] is set it is left at the first bit of the game payload.
    ///
    /// A header that is short, malformed, from the wrong connect sequence, outside the sequence window or acking a packet never
    /// sent is dropped whole (`Received::dropped`) and changes nothing. An `Err` means a valid header was followed by a rate
    /// prefix that ends early: the caller disconnects with `Packet Error.`.
    pub fn receive(&mut self, r: &mut BitReader, now: u32) -> Result<Received, NetError> {
        let Ok(header) = Header::read(r) else {
            return Ok(Received::dropped(Dropped::Malformed));
        };
        let (distance, seq, ack) = match self.admit(&header) {
            Ok(widened) => widened,
            Err(why) => return Ok(Received::dropped(why)),
        };

        self.ack_mask = self.ack_mask.checked_shl(distance).unwrap_or(0);
        if header.ptype == PacketType::Data {
            self.ack_mask |= 1;
        }
        let mut notifies = Vec::new();
        let mut i = self.highest_acked;
        while i != ack {
            i = i.wrapping_add(1);
            let back = ack.wrapping_sub(i);
            let delivered = header
                .ack_mask
                .checked_shr(back)
                .is_some_and(|bits| bits & 1 != 0);
            if let Some(n) = self.handle_notify(i, delivered, now) {
                notifies.push(n);
            }
        }
        if seq.wrapping_sub(self.last_recv_ack_ack) as i32 > 32 {
            self.last_recv_ack_ack = seq.wrapping_sub(32);
        }
        self.highest_acked = ack;
        self.keep_alive(now);

        let is_new = header.ptype == PacketType::Data && seq != self.last_seq_recvd;
        self.last_seq_recvd = seq;
        let mut payload = false;
        if is_new {
            self.read_rate_prefix(r)?;
            payload = true;
        }
        Ok(Received {
            dropped: None,
            ptype: Some(header.ptype),
            seq,
            notifies,
            payload,
            answer_ping: header.ptype == PacketType::Ping,
        })
    }
}

/// A reader over `bytes` positioned at bit `bit` (for a caller that was told where the payload starts).
pub fn reader_at(bytes: &[u8], bit: usize) -> BitReader<'_> {
    let mut r = BitReader::new(bytes);
    let mut left = bit.min(bytes.len().saturating_mul(8));
    while left > 0 {
        let n = left.min(32) as u8;
        if r.read_bits(n).is_err() {
            break;
        }
        left -= usize::from(n);
    }
    r
}

#[cfg(test)]
mod tests;
