//! The datagram decoder: a list of the datagrams a client and a server exchanged, run through the packet layer and the
//! event layer, up to the **boundary**, the first event of a class the caller names (the first class this crate has no decoder
//! for, typically; `None` decodes everything). Nothing here reads a file: the caller passes the datagrams, the Huffman table, the
//! derived event ids and a [`DecodeProfile`] (the handshake profile, the control section's format and the reader of its packet
//! data, the boundary).
//!
//! What it checks, per direction:
//! - every datagram before the boundary decodes: header, rate prefix, the game-packet head the direction has (server to client: the
//!   move ack, the damage flash, the control and camera sections, the first-person and field-of-view pairs; client to server: the
//!   control checksum, the moves, the first-person and field-of-view pairs), the events, and the one-bit ghost part a server
//!   packet ends with;
//! - **fewer than 8 bits are left** after all of that (the unused bits of a datagram's last byte are stale and never read);
//! - the events **re-encode to the same bits** (header, rate prefix and the event section; a self-consistency check only);
//! - every class id on the wire is below the derived count and has the direction the datagram came from;
//! - the NetString events and the remote commands replay through this crate's [`SendStrings`] LRU: the slot every tag argument names
//!   is the slot it would have chosen, and a NetString event is on the wire exactly where it would post one.
//! - an ack or ping packet has fewer than 8 bits after its header.
//!
//! **What it cannot see.** The last 7 bits of a datagram are the padding of its final byte (stale) and are never looked at, so a
//! field of up to 7 bits that nothing reads could hide there. The re-encode covers the header, the rate prefix and the event section;
//! it does not cover the game head (move ack, flash, control, camera, first person, field of view; checksum, moves) or the final ghost flag, and it
//! is a self-consistency check only: a codec wrong the same way in both directions passes. What discriminates is the LRU replay
//! (the slots the sender chose against the ones this table would); the decoded NetString events are in the report
//! ([`Report::netstrings`]) for a host to compare with what the receiver printed. Nothing after the boundary is decoded.

use std::collections::{BTreeMap, VecDeque};

use crate::NetError;
use crate::bits::{BitReader, BitWriter};
use crate::client::{Decoded, decode_datagram};
use crate::control::{ClientBlock, ControlFormat, ControlReader, ServerHead};
use crate::events::{
    EventCtx, EventIds, EventReceiver, EventUnpacker, Incoming, NetEvent, NetEventUnpacker,
    WireItem, free_charge, write_wire_items,
};
use crate::huffman::Table;
use crate::netstring::{LocalTags, RemoteStrings, SendStrings};
use crate::profile::HandshakeProfile;
use crate::protocol::{PacketType, RatePrefix, Role, reader_at};
use crate::remote::RemoteArg;

/// Which way a datagram went.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Dir {
    ToServer,
    ToClient,
}

impl Dir {
    pub fn index(self) -> usize {
        match self {
            Dir::ToServer => 0,
            Dir::ToClient => 1,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Dir::ToServer => "client to server",
            Dir::ToClient => "server to client",
        }
    }

    /// The role of the side that reads this direction.
    fn reader(self) -> Role {
        match self {
            Dir::ToServer => Role::Server,
            Dir::ToClient => Role::Client,
        }
    }
}

/// One datagram of the list.
#[derive(Debug, Clone, Copy)]
pub struct Datagram<'a> {
    pub dir: Dir,
    pub ms: u32,
    pub bytes: &'a [u8],
}

/// The text the unpacker returns for the boundary class, so the decoder can tell it from a malformed event.
const BOUNDARY: &str = "boundary event";

/// A bit count above which a datagram is not fully explained.
const LEFTOVER_LIMIT: usize = 8;

/// What was decoded in one direction.
#[derive(Debug, Clone, Default)]
pub struct DirReport {
    /// Datagrams before the boundary (game and handshake).
    pub datagrams: usize,
    pub data_packets: usize,
    pub ack_packets: usize,
    pub ping_packets: usize,
    pub handshake_packets: usize,
    /// Events read off the wire, by class id (the boundary event itself excluded).
    pub events: BTreeMap<u32, usize>,
    /// The most bits left after everything a data packet holds; `leftover_hist[n]` counts packets with `n` bits left (8 = 8 or more).
    pub leftover_max: usize,
    pub leftover_hist: [usize; 9],
    /// The most bits left after the header of an ack or ping packet.
    pub non_data_leftover_max: usize,
    pub errors: Vec<String>,
    /// Data packets whose header, rate prefix and event section were re-encoded, and how many came out different.
    pub reencoded: usize,
    pub reencode_mismatches: usize,
    pub duplicates_dropped: usize,
    pub too_far_dropped: usize,
    pub max_waiting: usize,
    /// Server packets whose ghost flag was set (ghosting began before the boundary).
    pub ghost_flags_set: usize,
    /// Remote-command string arguments that begin with byte 1 (read as tags).
    pub plain_byte1_args: usize,
}

/// Where the first event of the boundary class is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Boundary {
    /// The index of the datagram in the list.
    pub record: usize,
    pub ms: u32,
    pub dir: Dir,
    /// Events of that datagram read before the boundary event.
    pub events_before: usize,
}

/// The LRU replay.
#[derive(Debug, Clone, Default)]
pub struct Replay {
    /// Remote commands replayed, tag arguments checked, NetString events seen.
    pub commands: usize,
    pub tag_args: usize,
    pub netstrings: usize,
    /// Slots where this LRU chose another slot than the wire's, events it would not have posted (or not at that place),
    /// tag arguments naming an unmapped slot.
    pub slot_mismatches: Vec<String>,
    pub event_mismatches: Vec<String>,
    pub unmapped: Vec<String>,
    /// NetString events no command used.
    pub unused_netstrings: usize,
}

/// A whole list decoded.
#[derive(Debug, Clone, Default)]
pub struct Report {
    pub records: usize,
    pub boundary: Option<Boundary>,
    pub dirs: [DirReport; 2],
    /// The NetString events of each direction, in the order they ran: `(slot, text)`.
    pub netstrings: [Vec<(u8, Vec<u8>)>; 2],
    /// The replay of each direction.
    pub replay: [Replay; 2],
    /// `ConnectionMessage`s seen: direction, message, sequence, count.
    pub connection_messages: Vec<(Dir, u8, u32, u32)>,
    /// The longest server datagram sent before the client's first data packet (the first server packets are capped at 200 bytes).
    pub max_server_bytes_before_client: usize,
}

impl Report {
    pub fn errors(&self) -> usize {
        self.dirs.iter().map(|d| d.errors.len()).sum()
    }

    /// Everything that must be zero for the list to pass.
    pub fn failures(&self) -> usize {
        self.errors()
            + self
                .dirs
                .iter()
                .map(|d| d.reencode_mismatches + d.leftover_hist[LEFTOVER_LIMIT])
                .sum::<usize>()
            + self
                .replay
                .iter()
                .map(|r| r.slot_mismatches.len() + r.event_mismatches.len() + r.unmapped.len())
                .sum::<usize>()
    }
}

// ---- the heads of a game packet

/// The server-to-client head before the events, up to the field of view: [`ServerHead::read_in`] (it sets the reader's
/// compression point, as a control section does).
fn skip_s2c_head(
    r: &mut BitReader,
    ghost_bits: u8,
    format: ControlFormat,
    reader: &mut dyn ControlReader,
) -> Result<(), NetError> {
    ServerHead::read_in(
        r,
        ghost_bits,
        format,
        &mut free_charge(),
        &mut |index, r, charge| reader.read_control(index, r, ghost_bits, charge),
    )
    .map(|_| ())
}

/// The client-to-server head: [`ClientBlock::read_with`] (camera flag, checksum, move block, first person, fov).
fn skip_c2s_head(r: &mut BitReader, format: ControlFormat) -> Result<(), NetError> {
    ClientBlock::read_with(r, format, &mut free_charge()).map(|_| ())
}

/// What the game of the datagrams sets: its handshake, its control section (the format, and the reader of its control objects'
/// packet data, [`crate::control::CameraReader`] for Torque's) and the boundary class (`None`: the whole list).
pub struct DecodeProfile<'a> {
    pub handshake: &'a HandshakeProfile,
    pub control: ControlFormat,
    pub reader: &'a mut dyn ControlReader,
    pub boundary: Option<u32>,
    /// The width of a datablock id in the audio events ([`crate::profile::DEFAULT_DATABLOCK_BITS`] for TGE's).
    pub datablock_bits: u8,
}

// ---- the unpacker with a boundary

struct Bounded {
    inner: NetEventUnpacker,
    boundary_class: Option<u32>,
}

impl EventUnpacker for Bounded {
    type Event = NetEvent;

    fn unpack(
        &mut self,
        class: u32,
        r: &mut BitReader,
        ctx: &EventCtx<'_>,
    ) -> Result<NetEvent, NetError> {
        if Some(class) == self.boundary_class {
            return Err(NetError::Invalid(BOUNDARY));
        }
        self.inner.unpack(class, r, ctx).map_err(|e| match e {
            NetError::Invalid("event class has no decoder") => {
                NetError::Invalid("event class without a decoder before the boundary")
            }
            other => other,
        })
    }
}

/// An operation of the replay: what ran, in the order it ran.
enum Op {
    NetString(u8, Vec<u8>),
    Command(Vec<RemoteArg>),
}

/// Compares two bit ranges.
fn bits_equal(a: &[u8], a_start: usize, b: &[u8], len: usize) -> bool {
    let bit = |bytes: &[u8], i: usize| {
        bytes
            .get(i >> 3)
            .is_some_and(|byte| (byte >> (i & 7)) & 1 != 0)
    };
    (0..len).all(|i| bit(a, a_start + i) == bit(b, i))
}

/// Decodes a list of datagrams up to the first event of the profile's boundary class, as the profile's game writes it.
pub fn decode_stream(
    datagrams: &[Datagram<'_>],
    table: &Table,
    ids: &EventIds,
    ghost_bits: u8,
    profile: &mut DecodeProfile<'_>,
) -> Report {
    let boundary = profile.boundary;
    let ctx = EventCtx {
        table,
        ids,
        ghost_bits,
        datablock_bits: profile.datablock_bits,
    };
    let mut report = Report {
        records: datagrams.len(),
        ..Report::default()
    };
    let mut receivers: [EventReceiver<NetEvent>; 2] = [EventReceiver::new(), EventReceiver::new()];
    let mut ops: [Vec<Op>; 2] = [Vec::new(), Vec::new()];
    let mut client_spoke = false;

    for (index, dg) in datagrams.iter().enumerate() {
        let d = dg.dir.index();
        let dir_report = &mut report.dirs[d];
        dir_report.datagrams += 1;
        let decoded = match decode_datagram(table, profile.handshake, dg.bytes) {
            Ok(decoded) => decoded,
            Err(e) => {
                dir_report.errors.push(format!("record {index}: {e}"));
                continue;
            }
        };
        let (header, rate, payload_bit) = match decoded {
            Decoded::Game {
                header,
                rate,
                payload_bit,
            } => (header, rate, payload_bit),
            _ => {
                dir_report.handshake_packets += 1;
                continue;
            }
        };
        match header.ptype {
            PacketType::Ack | PacketType::Ping => {
                if header.ptype == PacketType::Ack {
                    dir_report.ack_packets += 1;
                } else {
                    dir_report.ping_packets += 1;
                }
                let left = (dg.bytes.len() * 8).saturating_sub(payload_bit);
                dir_report.non_data_leftover_max = dir_report.non_data_leftover_max.max(left);
                if left >= LEFTOVER_LIMIT {
                    dir_report.errors.push(format!(
                        "record {index}: {left} bits after the header of an ack or ping packet"
                    ));
                }
                continue;
            }
            PacketType::Data => dir_report.data_packets += 1,
        }
        if dg.dir == Dir::ToServer {
            client_spoke = true;
        } else if !client_spoke {
            report.max_server_bytes_before_client =
                report.max_server_bytes_before_client.max(dg.bytes.len());
        }

        let mut r = reader_at(dg.bytes, payload_bit);
        r.enable_string_buffer();
        let head = match dg.dir {
            Dir::ToClient => {
                skip_s2c_head(&mut r, ghost_bits, profile.control, &mut *profile.reader)
            }
            Dir::ToServer => skip_c2s_head(&mut r, profile.control),
        };
        if let Err(e) = head {
            dir_report.errors.push(format!("record {index}: head: {e}"));
            continue;
        }
        let events_start = r.position();
        let mut unpacker = Bounded {
            inner: NetEventUnpacker,
            boundary_class: boundary,
        };
        let mut out: Vec<Incoming<NetEvent>> = Vec::new();
        let mut log: Vec<WireItem<NetEvent>> = Vec::new();
        let result = receivers[d].read_section(
            &mut r,
            &ctx,
            dg.dir.reader(),
            &mut unpacker,
            &mut out,
            Some(&mut log),
        );
        dir_report.max_waiting = dir_report.max_waiting.max(receivers[d].waiting());
        let stats = match result {
            Ok(stats) => stats,
            Err(NetError::Invalid(BOUNDARY)) => {
                receivers[d].run_ready(&mut out);
                for item in &log {
                    *dir_report.events.entry(item.class).or_insert(0) += 1;
                }
                report.boundary = Some(Boundary {
                    record: index,
                    ms: dg.ms,
                    dir: dg.dir,
                    events_before: log.len(),
                });
                dir_report.datagrams -= 1;
                dir_report.data_packets -= 1;
                collect_ops(&out, &mut ops[d], &mut report.connection_messages, dg.dir);
                break;
            }
            Err(e) => {
                dir_report
                    .errors
                    .push(format!("record {index}: events: {e}"));
                continue;
            }
        };
        let events_end = r.position();
        dir_report.duplicates_dropped += stats.duplicates;
        dir_report.too_far_dropped += stats.too_far;
        for item in &log {
            *dir_report.events.entry(item.class).or_insert(0) += 1;
        }

        // The ghost part of a server packet: one flag, 0 before ghosting starts.
        if dg.dir == Dir::ToClient && r.bits_left() >= 1 {
            match r.read_flag() {
                Ok(true) => dir_report.ghost_flags_set += 1,
                Ok(false) => {}
                Err(e) => dir_report
                    .errors
                    .push(format!("record {index}: ghost flag: {e}")),
            }
        }
        let left = r.bits_left();
        dir_report.leftover_max = dir_report.leftover_max.max(left);
        if let Some(slot) = dir_report.leftover_hist.get_mut(left.min(LEFTOVER_LIMIT)) {
            *slot += 1;
        }

        // Re-encode header, rate prefix and the event section.
        let mut w = BitWriter::new();
        header.write(&mut w);
        if let Some(prefix) = rate {
            RatePrefix::write(&prefix, &mut w);
        }
        let head_bits = w.bits();
        let head_ok = head_bits == payload_bit && bits_equal(dg.bytes, 0, w.bytes(), head_bits);
        let mut sec = BitWriter::new();
        sec.enable_string_buffer();
        write_wire_items(&mut sec, &ctx, &log);
        let section_bits = events_end - events_start;
        let section_ok = sec.bits() == section_bits
            && bits_equal(dg.bytes, events_start, sec.bytes(), section_bits);
        dir_report.reencoded += 1;
        if !(head_ok && section_ok) {
            dir_report.reencode_mismatches += 1;
        }

        collect_ops(&out, &mut ops[d], &mut report.connection_messages, dg.dir);
        for inc in &out {
            if let NetEvent::Remote(c) = &inc.event {
                dir_report.plain_byte1_args += c
                    .args
                    .iter()
                    .filter(|a| matches!(a, RemoteArg::Text(t) if t.first() == Some(&1)))
                    .count();
            }
        }
    }

    for dir in [Dir::ToServer, Dir::ToClient] {
        let d = dir.index();
        report.replay[d] = replay(&ops[d], &mut report.netstrings[d]);
    }
    report
}

fn collect_ops(
    out: &[Incoming<NetEvent>],
    ops: &mut Vec<Op>,
    messages: &mut Vec<(Dir, u8, u32, u32)>,
    dir: Dir,
) {
    for inc in out {
        match &inc.event {
            NetEvent::NetString(e) => ops.push(Op::NetString(e.slot, e.text.clone())),
            NetEvent::Remote(c) => ops.push(Op::Command(c.args.clone())),
            NetEvent::ConnectionMessage(m) => messages.push((dir, m.message, m.seq, m.count)),
        }
    }
}

/// Replays one direction's NetString events and commands through a fresh sender LRU.
fn replay(ops: &[Op], strings_out: &mut Vec<(u8, Vec<u8>)>) -> Replay {
    let mut rep = Replay::default();
    let mut tags = LocalTags::new();
    let mut remote = RemoteStrings::new();
    let mut send = SendStrings::new();
    let mut pending: VecDeque<(u8, Vec<u8>)> = VecDeque::new();
    for (n, op) in ops.iter().enumerate() {
        match op {
            Op::NetString(slot, text) => {
                rep.netstrings += 1;
                strings_out.push((*slot, text.clone()));
                let id = tags.add(text);
                remote.set(*slot, id, text);
                pending.push_back((*slot, text.clone()));
            }
            Op::Command(args) => {
                rep.commands += 1;
                for arg in args {
                    let RemoteArg::Tag(slot) = arg else { continue };
                    rep.tag_args += 1;
                    let Some(mapped) = remote.get(i64::from(*slot)) else {
                        rep.unmapped
                            .push(format!("op {n}: tag argument names unmapped slot {slot}"));
                        continue;
                    };
                    let text = mapped.text.clone();
                    let id = tags.add(&text);
                    let checked = send.check_string(id, &text);
                    if checked.slot != *slot {
                        rep.slot_mismatches.push(format!(
                            "op {n}: the wire names slot {slot}, the LRU would use {}",
                            checked.slot
                        ));
                    }
                    if let Some(event) = checked.event {
                        match pending.pop_front() {
                            Some((s, t)) if s == event.slot && t == event.text => {}
                            Some((s, _)) => rep.event_mismatches.push(format!(
                                "op {n}: the LRU posts slot {} but the wire's next NetString is slot {s}",
                                event.slot
                            )),
                            None => rep.event_mismatches.push(format!(
                                "op {n}: the LRU posts slot {} but the wire has no NetString left",
                                event.slot
                            )),
                        }
                    }
                }
            }
        }
    }
    rep.unused_netstrings = pending.len();
    rep
}

#[cfg(test)]
mod tests;
