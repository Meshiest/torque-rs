//! A remote command as a byte body, for a carrier that is not TGE's event queue (the engine protocol sends it as an RPC).
//!
//! The body holds what [`send_command`](super::send_command) puts on the ordered queue, in one piece: the tags' texts first,
//! then the command. A tag's text always travels with the command that uses it, so a command depends on no earlier one (a
//! carrier may drop a call at a rate limit without leaving later commands with slots that mean nothing). The slots are the
//! sender's [`SendStrings`], so a receiver's 32 handles are kept as TGE keeps them and a script sees the same tag ids.
//!
//! ```text
//! variant   u8
//! strings   u8 count (at most 20), then per string: slot u8 (0..31), length u8, bytes
//! arguments u8 count (at most 20), then per argument a kind u8:
//!             0 empty | 1 text: length u8, bytes | 2 tag: slot u8 | 3 integer: i32, little endian
//! ```
//!
//! **Hostile input.** [`CommandBody::decode`] checks every count and length against the bytes left and the caps above, takes
//! no allocation larger than the body, and refuses trailing bytes. The strings are exactly the tags the arguments name, each
//! slot once, so a body costs the receiver no more tag-table work than it has tag arguments. A body is at most
//! [`MAX_BODY_BYTES`].

use super::{RemoteArg, SendError, TagLookup, send_command_with};
use crate::NetError;
use crate::cap::{NET_ARG_BYTES, NET_MAX_ARGC_SEND, NET_REMOTE_SLOTS};
use crate::events::{EventQueue, NetEvent};
use crate::netstring::SendStrings;

/// The most bytes a body has: what fits an RPC message with its header.
pub const MAX_BODY_BYTES: usize = 1_000;

const KIND_EMPTY: u8 = 0;
const KIND_TEXT: u8 = 1;
const KIND_TAG: u8 = 2;
const KIND_INT: u8 = 3;

/// A remote command with the texts of its tags.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandBody {
    /// The command's class ([`RemoteCommandEvent::variant`](super::RemoteCommandEvent::variant)).
    pub variant: u8,
    /// The tags the arguments name: a slot and its text, once each, in the order the arguments first use them.
    pub strings: Vec<(u8, Vec<u8>)>,
    /// The arguments, a tag as its slot.
    pub args: Vec<RemoteArg>,
}

/// Why a command could not be made into a body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyError {
    /// The command could not be sent at all ([`send_command`](super::send_command)'s reasons).
    Send(SendError),
    /// The body would be over [`MAX_BODY_BYTES`].
    TooLarge,
    /// An argument names a tag the sender's tag table does not have: the command is not sent rather than sent with a text that
    /// is not the script's.
    UnknownTag,
}

impl std::fmt::Display for BodyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BodyError::Send(_) => f.write_str("the command cannot be sent"),
            BodyError::TooLarge => write!(f, "its arguments are over {MAX_BODY_BYTES} bytes"),
            BodyError::UnknownTag => f.write_str("it names a tag that does not exist"),
        }
    }
}

impl CommandBody {
    /// The body's bytes.
    ///
    /// # Errors
    /// [`BodyError::TooLarge`] when they would be over [`MAX_BODY_BYTES`].
    pub fn encode(&self) -> Result<Vec<u8>, BodyError> {
        let mut out = vec![
            self.variant,
            self.strings.len().min(NET_MAX_ARGC_SEND) as u8,
        ];
        for (slot, text) in self.strings.iter().take(NET_MAX_ARGC_SEND) {
            out.push(*slot);
            push_text(&mut out, text);
        }
        out.push(self.args.len().min(NET_MAX_ARGC_SEND) as u8);
        for arg in self.args.iter().take(NET_MAX_ARGC_SEND) {
            match arg {
                RemoteArg::Empty => out.push(KIND_EMPTY),
                RemoteArg::Text(text) => {
                    out.push(KIND_TEXT);
                    push_text(&mut out, text);
                }
                RemoteArg::Tag(slot) => out.extend_from_slice(&[KIND_TAG, *slot]),
                RemoteArg::Int(value) => {
                    out.push(KIND_INT);
                    // TGE's integer form cannot say -2147483648: it arrives as 0.
                    let value = if *value == i32::MIN { 0 } else { *value };
                    out.extend_from_slice(&value.to_le_bytes());
                }
            }
        }
        if out.len() > MAX_BODY_BYTES {
            return Err(BodyError::TooLarge);
        }
        Ok(out)
    }

    /// Reads a body from a peer.
    ///
    /// # Errors
    /// A [`NetError`] for anything a sender of this engine does not make: a count or a length over its cap, a slot past the
    /// table, a slot given twice, a string no argument names or a tag argument with no string, an unknown kind, a body that ends
    /// early or has bytes left.
    pub fn decode(bytes: &[u8]) -> Result<CommandBody, NetError> {
        if bytes.len() > MAX_BODY_BYTES {
            return Err(NetError::Invalid("a command body over its size"));
        }
        let mut r = Bytes(bytes);
        let variant = r.byte()?;
        let strings_count = usize::from(r.byte()?);
        if strings_count > NET_MAX_ARGC_SEND {
            return Err(NetError::Invalid("too many strings in a command body"));
        }
        let mut strings = Vec::with_capacity(strings_count);
        for _ in 0..strings_count {
            let slot = r.slot()?;
            strings.push((slot, r.text()?.to_vec()));
        }
        let argc = usize::from(r.byte()?);
        if argc > NET_MAX_ARGC_SEND {
            return Err(NetError::Invalid("too many arguments in a command body"));
        }
        let mut args = Vec::with_capacity(argc);
        for _ in 0..argc {
            args.push(match r.byte()? {
                KIND_EMPTY => RemoteArg::Empty,
                KIND_TEXT => RemoteArg::Text(r.text()?.to_vec()),
                KIND_TAG => RemoteArg::Tag(r.slot()?),
                KIND_INT => {
                    let raw = r.take(4)?;
                    RemoteArg::Int(i32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]))
                }
                _ => return Err(NetError::Invalid("an argument kind of a command body")),
            });
        }
        if !r.0.is_empty() {
            return Err(NetError::Invalid("bytes after a command body"));
        }
        check_strings(&strings, &args)?;
        Ok(CommandBody {
            variant,
            strings,
            args,
        })
    }

    /// The arguments as the script side sees them before expansion (a tag is `\x01<slot>`).
    pub fn script_args(&self) -> Vec<Vec<u8>> {
        self.args.iter().map(RemoteArg::to_script).collect()
    }
}

/// The strings are the tags the arguments name: each slot once, and every tag argument has its text.
fn check_strings(strings: &[(u8, Vec<u8>)], args: &[RemoteArg]) -> Result<(), NetError> {
    let mut given = 0u32;
    for (slot, _) in strings {
        let bit = 1u32 << slot;
        if given & bit != 0 {
            return Err(NetError::Invalid(
                "a string slot given twice in a command body",
            ));
        }
        given |= bit;
    }
    let mut named = 0u32;
    for arg in args {
        if let RemoteArg::Tag(slot) = arg {
            named |= 1u32 << slot;
        }
    }
    if given != named {
        return Err(NetError::Invalid(
            "the strings of a command body are not its tags",
        ));
    }
    Ok(())
}

fn push_text(out: &mut Vec<u8>, text: &[u8]) {
    let text = text.get(..text.len().min(NET_ARG_BYTES)).unwrap_or(&[]);
    out.push(text.len() as u8);
    out.extend_from_slice(text);
}

/// A cursor over the bytes of a body.
struct Bytes<'a>(&'a [u8]);

impl<'a> Bytes<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], NetError> {
        let (head, rest) = self.0.split_at_checked(n).ok_or(NetError::Overrun)?;
        self.0 = rest;
        Ok(head)
    }

    fn byte(&mut self) -> Result<u8, NetError> {
        Ok(self.take(1)?[0])
    }

    fn slot(&mut self) -> Result<u8, NetError> {
        let slot = self.byte()?;
        if usize::from(slot) >= NET_REMOTE_SLOTS {
            return Err(NetError::Invalid("a string slot of a command body"));
        }
        Ok(slot)
    }

    fn text(&mut self) -> Result<&'a [u8], NetError> {
        let len = usize::from(self.byte()?);
        self.take(len)
    }
}

/// [`send_command`](super::send_command)'s checks and its use of the sender's `strings`, with the result as a body: the
/// command named by `args` (a tag is `\x01<id>`) for a command class `variant`. `strings` is left as the command changed it
/// whether or not it fits a body, so a caller works on a copy and keeps it only on success.
///
/// # Errors
/// [`BodyError::Send`] for a command [`send_command`](super::send_command) refuses, [`BodyError::TooLarge`] for one whose body
/// is over [`MAX_BODY_BYTES`].
pub fn encode_command(
    strings: &mut SendStrings,
    tags: &dyn TagLookup,
    variant: u8,
    args: &[Vec<u8>],
) -> Result<Vec<u8>, BodyError> {
    let mut scratch: EventQueue<NetEvent> = EventQueue::new();
    send_command_with(
        &mut scratch,
        strings,
        tags,
        args,
        NetEvent::NetString,
        NetEvent::Remote,
    )
    .map_err(BodyError::Send)?;
    let wire = scratch
        .release_all()
        .into_iter()
        .find_map(|sent| match sent.event {
            NetEvent::Remote(command) => Some(command.args),
            _ => None,
        })
        .unwrap_or_default();
    let mut texts: Vec<(u8, Vec<u8>)> = Vec::new();
    for arg in &wire {
        let RemoteArg::Tag(slot) = arg else { continue };
        if texts.iter().any(|(seen, _)| seen == slot) {
            continue;
        }
        let text = strings
            .tag_of(*slot)
            .and_then(|id| tags.tag_text(id))
            .ok_or(BodyError::UnknownTag)?;
        texts.push((*slot, text));
    }
    CommandBody {
        variant,
        strings: texts,
        args: wire,
    }
    .encode()
}

#[cfg(test)]
#[path = "body_tests.rs"]
mod tests;
