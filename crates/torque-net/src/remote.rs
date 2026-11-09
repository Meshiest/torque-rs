//! `RemoteCommandEvent`: the wire form, the sender, and the receiver's tag expansion (`pack`, `unpack`, `process`,
//! `sendRemoteCommand`, `expandString`). A game with a command class of its own registers it as one
//! ([`crate::events::EventRegistry::register_command`]): its commands are [`RemoteCommandEvent`]s whose
//! [`RemoteCommandEvent::variant`] names the class, sent with [`send_command_with`].
//!
//! **Wire.** `int(5)` argc, then per argument a 2-bit kind: 0 empty, 1 a string (255 bytes, the packet's prefix-delta buffer), 2 a
//! tag (`int(5)` connection slot), 3 an integer (a sign flag, then flag -> 7 bits, else flag -> 15 bits, else 31 bits). The
//! sender classifies a string argument exactly as Torque does ([`RemoteArg::classify`]): empty -> 0; first
//! byte 1 -> a tag; `-` or a digit whose `%d` of `atoi` is the same text -> an integer; anything else -> a string. `-2147483648`
//! is an integer too and, as in Torque, arrives as `0` (its negation is itself, compares as negative and goes out in 7 bits as 0).
//!
//! **Sender** ([`send_command`]): first the command tag's text must be `[A-Za-z0-9_-]*` (applied to every `commandTo*`;
//! silent when it fails), then the first argument must be a tag; trailing empty arguments are dropped; at most 20 arguments; every tag
//! argument goes through the connection's [`SendStrings`] in argument order (an id with no tag is clamped to the table size and still
//! takes a slot, with an empty text, as `validateSendString` does), so the NetString events of a command's new tags are posted first, in argument
//! order, then the command. A queue without room for the whole batch refuses it before the LRU is touched.
//!
//! **Receiver** ([`resolve`]): arguments are walked from the last to the first; a tag argument becomes `\x01<id> <text>` with
//! `%1..%9` in the text replaced by the later arguments (a later tag argument reduced to its text) and the whole expansion capped at
//! [`NET_EXPAND_BYTES`] - 1 bytes; an unmapped handle expands to `<NULL>`; a command that is not a tag is silent. A plain string
//! argument that begins with byte 1 is read as a tag, as Torque does, but its index is checked: outside 0..31 it is unmapped.
//!
//! **As an RPC body** ([`body`]): the same command and the texts of its tags in one byte string, for the engine protocol, which
//! has no ordered event queue to put NetString events on.
//!
//! **Hostile input.** A count the bits left cannot hold is an error, not an allocation (argc is the 5-bit field, so at most
//! [`NET_MAX_ARGC`] = 31 arguments are read, as in Torque).
//!
//! Credit: where Torque3D has the same rules, this module follows its MIT-licensed `app/net/net.cpp` (Copyright (c) 2012
//! GarageGames, LLC); the code here is this crate's own.

pub mod body;

use crate::NetError;
use crate::bits::{BitReader, BitWriter};
use crate::cap::{
    self, NET_ARG_BYTES, NET_COMMAND_NAME_BYTES, NET_EXPAND_BYTES, NET_MAX_ARGC, NET_MAX_ARGC_SEND,
};
use crate::events::{EventCtx, EventQueue, NetEvent, WireEvent};
use crate::netstring::{Mapped, NetStringEvent, RemoteStrings, SendStrings};
use crate::strcoder::{read_string, write_string};

/// C's `atoi` as `strtol` does it on a 32-bit `long`: leading white space, an optional sign, digits; the value saturates.
pub fn atoi(s: &[u8]) -> i32 {
    let mut i = 0;
    while s.get(i).is_some_and(|b| matches!(b, b' ' | 9..=13)) {
        i += 1;
    }
    let negative = match s.get(i) {
        Some(b'-') => {
            i += 1;
            true
        }
        Some(b'+') => {
            i += 1;
            false
        }
        _ => false,
    };
    let mut value: i64 = 0;
    while let Some(d) = s.get(i).filter(|b| b.is_ascii_digit()) {
        value = (value * 10 + i64::from(d - b'0')).min(1 << 33);
        i += 1;
    }
    let value = if negative { -value } else { value };
    value.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}

/// One argument as it is on the wire.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteArg {
    /// Kind 0: null or empty.
    Empty,
    /// Kind 1: a string of at most 255 bytes.
    Text(Vec<u8>),
    /// Kind 2: a connection slot, 0..31.
    Tag(u8),
    /// Kind 3: an integer whose `%d` is the argument's text.
    Int(i32),
}

impl RemoteArg {
    /// How the pack routine sees an argument string. A tag's number is the connection slot the constructor rewrote it to.
    pub fn classify(s: &[u8]) -> RemoteArg {
        let s = s.split(|&b| b == 0).next().unwrap_or(&[]);
        let s = s.get(..s.len().min(NET_ARG_BYTES)).unwrap_or(&[]);
        match s.first() {
            None => RemoteArg::Empty,
            Some(1) => RemoteArg::Tag((atoi(s.get(1..).unwrap_or(&[])) & 31) as u8),
            Some(&b) if b == b'-' || b.is_ascii_digit() => {
                let v = atoi(s);
                if v.to_string().as_bytes() == s {
                    RemoteArg::Int(v)
                } else {
                    RemoteArg::Text(s.to_vec())
                }
            }
            Some(_) => RemoteArg::Text(s.to_vec()),
        }
    }

    pub fn pack(&self, w: &mut BitWriter, ctx: &EventCtx<'_>) {
        match self {
            RemoteArg::Empty => w.write_bits(0, 2),
            RemoteArg::Text(s) => {
                w.write_bits(1, 2);
                write_string(w, ctx.table, s, NET_ARG_BYTES);
            }
            RemoteArg::Tag(slot) => {
                w.write_bits(2, 2);
                w.write_bits(u32::from(*slot) & 31, 5);
            }
            RemoteArg::Int(v) => {
                w.write_bits(3, 2);
                w.write_flag(*v < 0);
                // Torque negates and then compares as a signed int: `-2147483648` stays negative, so it takes the 7-bit form as 0.
                let m = if *v < 0 { v.wrapping_neg() } else { *v };
                let bits = m as u32;
                if m < 0x80 {
                    w.write_flag(true);
                    w.write_bits(bits, 7);
                } else {
                    w.write_flag(false);
                    if m < 0x8000 {
                        w.write_flag(true);
                        w.write_bits(bits, 15);
                    } else {
                        w.write_flag(false);
                        w.write_bits(bits, 31);
                    }
                }
            }
        }
    }

    /// `unpackString`.
    pub fn unpack(r: &mut BitReader, ctx: &EventCtx<'_>) -> Result<RemoteArg, NetError> {
        Ok(match r.read_bits(2)? {
            0 => RemoteArg::Empty,
            1 => RemoteArg::Text(read_string(r, ctx.table)?),
            2 => RemoteArg::Tag(r.read_bits(5)? as u8),
            _ => {
                let negative = r.read_flag()?;
                let magnitude = if r.read_flag()? {
                    r.read_bits(7)?
                } else if r.read_flag()? {
                    r.read_bits(15)?
                } else {
                    r.read_bits(31)?
                };
                let v = i64::from(magnitude);
                RemoteArg::Int((if negative { -v } else { v }) as i32)
            }
        })
    }

    /// The string the receiver's script side sees before expansion: a tag is `\x01<slot>`, an integer its `%d`.
    pub fn to_script(&self) -> Vec<u8> {
        match self {
            RemoteArg::Empty => Vec::new(),
            RemoteArg::Text(s) => s.clone(),
            RemoteArg::Tag(slot) => {
                let mut v = vec![1u8];
                v.extend_from_slice(slot.to_string().as_bytes());
                v
            }
            RemoteArg::Int(i) => i.to_string().into_bytes(),
        }
    }
}

/// A remote command on the wire.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteCommandEvent {
    /// The command's class: 0 is `RemoteCommandEvent`, `n` a game's `n`-th command class
    /// ([`crate::events::EventIds::command_class`]). Not on the wire: the class id carries it.
    pub variant: u8,
    pub args: Vec<RemoteArg>,
}

impl RemoteCommandEvent {
    pub fn pack(&self, w: &mut BitWriter, ctx: &EventCtx<'_>) {
        w.write_bits(self.args.len().min(31) as u32, 5);
        for arg in self.args.iter().take(31) {
            arg.pack(w, ctx);
        }
    }

    /// `argc = readInt(5)`: the whole field is accepted ([`NET_MAX_ARGC`], 31); a count the stream could not hold (2 bits an
    /// argument at least) is a protocol error.
    pub fn unpack(r: &mut BitReader, ctx: &EventCtx<'_>) -> Result<RemoteCommandEvent, NetError> {
        let wanted = u64::from(r.read_bits(5)?);
        let mut args: Vec<RemoteArg> = cap::vec_for(
            "remote command arguments",
            wanted,
            NET_MAX_ARGC,
            r.bits_left(),
            2,
        )?;
        for _ in 0..wanted {
            args.push(RemoteArg::unpack(r, ctx)?);
        }
        Ok(RemoteCommandEvent { variant: 0, args })
    }

    /// The arguments as strings (before expansion).
    pub fn script_args(&self) -> Vec<Vec<u8>> {
        self.args.iter().map(RemoteArg::to_script).collect()
    }
}

/// A command tag's text must be non-null and every byte `A-Za-z0-9_-` (an empty text passes).
pub fn valid_command_text(text: &[u8]) -> bool {
    text.iter()
        .all(|&b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// The host's global tag table, as the sender needs it.
pub trait TagLookup {
    /// The text of a tag id, or `None` for an invalid id.
    fn tag_text(&self, id: u32) -> Option<Vec<u8>>;
    /// The table's size: `validateSendString` clamps a tag id to `0..=size` and looks it up whether or not it exists.
    fn table_size(&self) -> u32;
}

/// Why a command was not posted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendError {
    /// `Remote Command Error - command must be a tag.` (the host prints it; nothing is sent).
    NotATag,
    /// The command tag's text is missing or not `[A-Za-z0-9_-]*`: silent, nothing is sent.
    BadCommandName,
    /// More than [`NET_MAX_ARGC_SEND`] arguments after trailing empties are dropped.
    TooManyArgs,
    /// The ordered queue has no room for the whole batch; nothing was changed. Retry later (backpressure).
    Backpressure,
    /// The connection no longer sends events.
    Closed,
}

/// What a posted command changed in the string table, for the host's tag references.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Sent {
    /// NetString events posted ahead of the command.
    pub netstrings: usize,
    /// Tags the table now holds a reference on / let go of.
    pub acquired: Vec<u32>,
    pub released: Vec<u32>,
}

/// `sendRemoteCommand` over the connection's string table and ordered queue. `args` are the script's strings: a tag is `\x01<id>`.
pub fn send_command(
    queue: &mut EventQueue<NetEvent>,
    strings: &mut SendStrings,
    tags: &dyn TagLookup,
    args: &[Vec<u8>],
) -> Result<Sent, SendError> {
    send_command_with(
        queue,
        strings,
        tags,
        args,
        NetEvent::NetString,
        NetEvent::Remote,
    )
}

/// [`send_command`] into a queue of the host's own event type: `string` wraps each NetString event and `command` the command
/// (a game's own command class wraps it in its event there).
pub fn send_command_with<E: WireEvent>(
    queue: &mut EventQueue<E>,
    strings: &mut SendStrings,
    tags: &dyn TagLookup,
    args: &[Vec<u8>],
    string: impl Fn(NetStringEvent) -> E,
    command: impl FnOnce(RemoteCommandEvent) -> E,
) -> Result<Sent, SendError> {
    // The native commands test the command tag's text first and are silent when it fails; `sendRemoteCommand` then wants a tag.
    let Some(first) = args.first() else {
        return Err(SendError::NotATag);
    };
    let number = first.strip_prefix(&[1u8][..]).unwrap_or(first);
    let command_text = tags.tag_text(atoi(number).max(0) as u32);
    if !command_text.as_deref().is_some_and(valid_command_text) {
        return Err(SendError::BadCommandName);
    }
    if first.first() != Some(&1) {
        return Err(SendError::NotATag);
    }
    let mut len = args.len();
    while len > 1 && args.get(len - 1).is_some_and(|a| a.is_empty()) {
        len -= 1;
    }
    let args = args.get(..len).unwrap_or(args);
    if args.len() > NET_MAX_ARGC_SEND {
        return Err(SendError::TooManyArgs);
    }
    if queue.is_closed() {
        return Err(SendError::Closed);
    }
    if queue.room_ordered() < args.len() + 1 {
        return Err(SendError::Backpressure);
    }
    let mut sent = Sent::default();
    let mut batch: Vec<E> = Vec::with_capacity(args.len() + 1);
    let mut wire: Vec<RemoteArg> = Vec::with_capacity(args.len());
    for arg in args {
        if arg.first() == Some(&1) {
            // `validateSendString`: the id is clamped to 0..=size and goes through `checkString` whether or not it names a tag (an
            // invalid one is a slot with an empty text).
            let id = atoi(arg.get(1..).unwrap_or(&[]))
                .clamp(0, tags.table_size().min(i32::MAX as u32) as i32)
                as u32;
            let text = tags.tag_text(id);
            let checked = strings.check_string(id, text.as_deref().unwrap_or(&[]));
            // A tag that names nothing takes no reference, and its entry owns none to give back.
            let exists = text.is_some();
            if !exists {
                strings.set_owned(checked.slot, id, false);
            }
            if let Some(event) = checked.event {
                batch.push(string(event));
                sent.netstrings += 1;
            }
            // References are the host's to keep only for tags that exist; the table gives back only the ones it owns.
            sent.acquired.extend(checked.acquired.filter(|_| exists));
            sent.released.extend(checked.released);
            wire.push(RemoteArg::Tag(checked.slot));
        } else {
            wire.push(RemoteArg::classify(arg));
        }
    }
    batch.push(command(RemoteCommandEvent {
        variant: 0,
        args: wire,
    }));
    queue
        .post_batch(batch)
        .map_err(|_| SendError::Backpressure)?;
    Ok(sent)
}

/// A received command after expansion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    /// The command name: the text after the first space of the expanded first argument (`<NULL>` for an unmapped tag).
    pub name: Vec<u8>,
    /// The expanded arguments after the command: a tag argument is `\x01<id> <text>`.
    pub args: Vec<Vec<u8>>,
}

fn push_capped(out: &mut Vec<u8>, bytes: &[u8]) {
    let room = NET_EXPAND_BYTES.saturating_sub(1).saturating_sub(out.len());
    out.extend_from_slice(bytes.get(..bytes.len().min(room)).unwrap_or(&[]));
}

/// The text of an expanded tag argument: what follows its first space.
fn after_first_space(arg: &[u8]) -> &[u8] {
    arg.iter()
        .position(|&b| b == b' ')
        .and_then(|i| arg.get(i + 1..))
        .unwrap_or(&[])
}

/// `NetStringTable::expandString`: `\x01<id> ` and the handle's text with `%1..%9` replaced by the `following` arguments, in at
/// most [`NET_EXPAND_BYTES`] - 1 bytes. `%` + any other byte drops the percent and keeps the byte; `%` at the end ends the text; a
/// missing handle prints `<NULL>`.
pub fn expand(handle: Option<&Mapped>, following: &[Vec<u8>]) -> Vec<u8> {
    let mut out = vec![1u8];
    push_capped(&mut out, handle.map_or(0, |m| m.id).to_string().as_bytes());
    push_capped(&mut out, b" ");
    let Some(handle) = handle else {
        push_capped(&mut out, b"<NULL>");
        return out;
    };
    let text = &handle.text;
    let mut i = 0;
    while let Some(&b) = text.get(i) {
        if out.len() >= NET_EXPAND_BYTES - 1 {
            break;
        }
        if b != b'%' {
            push_capped(&mut out, &[b]);
            i += 1;
            continue;
        }
        match text.get(i + 1).copied() {
            Some(d @ b'1'..=b'9') => {
                if let Some(arg) = following.get(usize::from(d - b'1')) {
                    if arg.first() == Some(&1) {
                        push_capped(&mut out, after_first_space(arg));
                    } else {
                        push_capped(&mut out, arg);
                    }
                }
                i += 2;
            }
            None | Some(0) => break,
            Some(other) => {
                push_capped(&mut out, &[other]);
                i += 2;
            }
        }
    }
    out
}

/// `process` of both event classes up to the lookup: expands every tag argument from the last to the first against the connection's
/// remote handles. `None` when the command is not a tag (nothing happens, silently) or there are no arguments.
pub fn resolve(args: &[Vec<u8>], strings: &RemoteStrings) -> Option<Resolved> {
    if args.is_empty() || args.len() > NET_MAX_ARGC {
        return None;
    }
    let mut cur: Vec<Vec<u8>> = args.to_vec();
    for i in (0..cur.len()).rev() {
        let Some(arg) = cur.get(i) else { continue };
        if arg.first() == Some(&1) {
            let slot = i64::from(atoi(arg.get(1..).unwrap_or(&[])));
            let expanded = expand(strings.get(slot), cur.get(i + 1..).unwrap_or(&[]));
            if let Some(cell) = cur.get_mut(i) {
                *cell = expanded;
            }
        } else if i == 0 {
            return None;
        }
    }
    let name = after_first_space(cur.first()?).to_vec();
    Some(Resolved {
        name,
        args: cur.get(1..).unwrap_or(&[]).to_vec(),
    })
}

/// The function name a command is looked up under: `prefix` and `name`, at most [`NET_COMMAND_NAME_BYTES`] bytes (the name is appended into
/// the space the shared buffer has left).
pub fn function_name(prefix: &[u8], name: &[u8]) -> Vec<u8> {
    let mut out = prefix.to_vec();
    out.truncate(NET_COMMAND_NAME_BYTES);
    let room = NET_COMMAND_NAME_BYTES - out.len();
    out.extend_from_slice(name.get(..name.len().min(room)).unwrap_or(&[]));
    out
}

/// The prefix of the function a received command calls: `serverCmd` on a server connection (a client's command), `clientCmd` on a
/// client connection.
pub fn function_prefix(on_server: bool) -> &'static [u8] {
    if on_server {
        b"serverCmd"
    } else {
        b"clientCmd"
    }
}

#[cfg(test)]
mod tests;
