//! The messages between a realm and its server, as plugin RPCs.
//!
//! Four built-in RPCs carry them: [`UP`] (a free-form value from a realm to the server), [`DOWN`] (one from the server to the
//! realm), [`STATUS`] (what the player decided) and [`OFFER`] (the server's client scripts and what they ask for). A value is
//! the format's value (`nil`, boolean, number, string, and tables nested at most 8 deep with at most 256 entries each) encoded
//! by the guest and decoded here by the format's decoder, which caps its depth, entries and nodes. An RPC message cannot hold a
//! byte string past 1,024 elements, so a value travels as a length and 32-byte chunks; the length and the chunks must agree
//! exactly, and the padding is zero.
//!
//! **Caps.** Up: [`MAX_UP_BYTES`] a message (the guest's cap), 32 a second with a burst of 64 (the RPC's bucket) and 64 KiB a
//! second (the server's byte bucket, [`ByteBucket`]). Down: [`MAX_DOWN_BYTES`] a message, 64 a second (the client's bucket on
//! receipt), a queue of 256 into the realm (`ScriptInbox`). Each cap is checked where the bytes arrive, as hostile input.

use torque_client_script_format::{Tier, Value};

use crate::abilities::ABILITIES;
use torque_engine::NetMessage;
use torque_net::fair::TokenBucket;

/// A realm's free-form message to its server.
pub const UP: &str = "torque.script.up";
/// The server's free-form message to a realm.
pub const DOWN: &str = "torque.script.down";
/// What the player decided about the server's scripts.
pub const STATUS: &str = "torque.script.status";
/// The server's client scripts and the abilities they ask for.
pub const OFFER: &str = "torque.script.offer";

/// The most bytes of one value a realm sends (the format's cap on a `net.send`).
pub const MAX_UP_BYTES: usize = torque_client_script_format::net::MAX_SEND_BYTES;
/// The most bytes of one value the server sends a realm: what fits an RPC frame beside its header, in whole chunks.
pub const MAX_DOWN_BYTES: usize = 480 * CHUNK;
/// Messages a second a realm may send (the RPC's bucket) and its burst.
pub const UP_RATE: (u32, u32) = (
    torque_client_script_format::net::SENDS_PER_SECOND,
    torque_client_script_format::net::SEND_BURST,
);
/// Messages a second the server may send a realm, and its burst.
pub const DOWN_RATE: (u32, u32) = (64, 128);
/// The bytes a second one connection's messages may total, and the same for its burst.
pub const UP_BYTES_PER_SECOND: u64 = torque_client_script_format::net::SEND_BYTES_PER_SECOND as u64;
/// The most values waiting for a realm to take them.
pub const MAX_INBOX: usize = 256;
/// The most mods one offer lists.
pub const MAX_OFFERED_MODS: usize = 16;

/// The bytes of one chunk.
pub const CHUNK: usize = 32;

/// A realm's value on its way to the server.
#[derive(Debug, Clone, PartialEq, Default, NetMessage)]
pub struct ScriptUp {
    /// The value's encoded length.
    #[net(bits = 13)]
    pub len: u16,
    /// The bytes, [`CHUNK`] to a chunk, the last padded with zeros.
    #[net(max_len = 128)]
    pub chunks: Vec<[u8; 32]>,
}

/// The server's value on its way to a realm.
#[derive(Debug, Clone, PartialEq, Default, NetMessage)]
pub struct ScriptDown {
    /// The value's encoded length.
    #[net(bits = 14)]
    pub len: u16,
    /// The bytes, [`CHUNK`] to a chunk, the last padded with zeros.
    #[net(max_len = 480)]
    pub chunks: Vec<[u8; 32]>,
}

const STATE_DECLINED: u8 = 0;
const STATE_UNAVAILABLE: u8 = 1;
const STATE_ALLOWED: u8 = 2;

/// What the player decided, for the server, as it travels (2 bits: declined, unavailable, allowed); [`Status`] is the typed
/// form ([`ScriptStatus::of`], [`ScriptStatus::status`]). Nothing more about why.
#[derive(Debug, Clone, PartialEq, Default, NetMessage)]
pub struct ScriptStatus {
    /// The answer, as [`ScriptStatus::status`] reads it.
    #[net(bits = 2)]
    pub state: u8,
    /// For an allowed answer, the abilities granted, as names joined by commas.
    #[net(max_len = 120)]
    pub tiers: String,
}

/// The server's client scripts: for each mod, its id, the content hash the client verifies it against, its entry file and
/// the abilities it asks for (names joined by commas). The four lists are as long as one another.
#[derive(Debug, Clone, PartialEq, Default, NetMessage)]
pub struct ScriptOffer {
    /// The mods' ids.
    #[net(max_len = 16)]
    pub ids: Vec<String>,
    /// The mods' content hashes.
    #[net(max_len = 16)]
    pub hashes: Vec<[u8; 32]>,
    /// The entry file of each mod.
    #[net(max_len = 16)]
    pub entries: Vec<String>,
    /// The abilities each mod asks for.
    #[net(max_len = 16)]
    pub tiers: Vec<String>,
}

/// Why a value was not sent or taken.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    /// The value is over the size cap for its direction.
    TooLarge,
    /// The bytes are not a value of the format: a length and chunks that disagree, nonzero padding, bytes the decoder refuses.
    Malformed,
}

/// `bytes` as a length and chunks.
fn pack(bytes: &[u8]) -> (u16, Vec<[u8; CHUNK]>) {
    let chunks = bytes
        .chunks(CHUNK)
        .map(|part| {
            let mut chunk = [0u8; CHUNK];
            chunk[..part.len()].copy_from_slice(part);
            chunk
        })
        .collect();
    (u16::try_from(bytes.len()).unwrap_or(u16::MAX), chunks)
}

/// The bytes a length and chunks hold: exactly `len` of them, the padding zero, within `cap`.
fn unpack(len: u16, chunks: &[[u8; CHUNK]], cap: usize) -> Result<Vec<u8>, Refused> {
    let len = usize::from(len);
    if len > cap {
        return Err(Refused::TooLarge);
    }
    if chunks.len() != len.div_ceil(CHUNK) {
        return Err(Refused::Malformed);
    }
    let bytes: Vec<u8> = chunks.iter().flatten().copied().collect();
    let (data, padding) = bytes.split_at(len);
    if padding.iter().any(|b| *b != 0) {
        return Err(Refused::Malformed);
    }
    Ok(data.to_vec())
}

fn encode(value: &Value, cap: usize) -> Result<Vec<u8>, Refused> {
    let bytes = value.to_bytes().map_err(|_| Refused::TooLarge)?;
    if bytes.len() > cap {
        return Err(Refused::TooLarge);
    }
    Ok(bytes)
}

fn decode(bytes: &[u8]) -> Result<Value, Refused> {
    Value::from_bytes(bytes).map_err(|_| Refused::Malformed)
}

impl ScriptUp {
    /// The message that carries `value`.
    ///
    /// # Errors
    ///
    /// [`Refused::TooLarge`] when the value does not encode within [`MAX_UP_BYTES`].
    pub fn of(value: &Value) -> Result<ScriptUp, Refused> {
        let (len, chunks) = pack(&encode(value, MAX_UP_BYTES)?);
        Ok(ScriptUp { len, chunks })
    }

    /// The value the message carries, from hostile bytes.
    ///
    /// # Errors
    ///
    /// [`Refused`] for a length past [`MAX_UP_BYTES`], chunks that do not match it, or bytes that are no value.
    pub fn value(&self) -> Result<Value, Refused> {
        decode(&unpack(self.len, &self.chunks, MAX_UP_BYTES)?)
    }
}

impl ScriptDown {
    /// The message that carries `value`.
    ///
    /// # Errors
    ///
    /// [`Refused::TooLarge`] when the value does not encode within [`MAX_DOWN_BYTES`].
    pub fn of(value: &Value) -> Result<ScriptDown, Refused> {
        let (len, chunks) = pack(&encode(value, MAX_DOWN_BYTES)?);
        Ok(ScriptDown { len, chunks })
    }

    /// The value the message carries, from hostile bytes.
    ///
    /// # Errors
    ///
    /// As [`ScriptUp::value`], with [`MAX_DOWN_BYTES`].
    pub fn value(&self) -> Result<Value, Refused> {
        decode(&unpack(self.len, &self.chunks, MAX_DOWN_BYTES)?)
    }
}

/// How the player answered, as the server learns it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    /// The player did not allow the scripts (not now, never, a setting, no ability left).
    Declined,
    /// This client cannot run server scripts (no guest it accepts, no runner, the feature off).
    Unavailable,
    /// A realm runs with these abilities.
    Allowed(Vec<Tier>),
}

impl Status {
    /// The word a script is told: `declined`, `unavailable` or `allowed`.
    pub fn word(&self) -> &'static str {
        match self {
            Status::Declined => "declined",
            Status::Unavailable => "unavailable",
            Status::Allowed(_) => "allowed",
        }
    }
}

impl ScriptStatus {
    /// The message that says `status`.
    pub fn of(status: &Status) -> ScriptStatus {
        match status {
            Status::Declined => ScriptStatus {
                state: STATE_DECLINED,
                tiers: String::new(),
            },
            Status::Unavailable => ScriptStatus {
                state: STATE_UNAVAILABLE,
                tiers: String::new(),
            },
            Status::Allowed(tiers) => ScriptStatus {
                state: STATE_ALLOWED,
                tiers: tiers.iter().map(|t| t.name()).collect::<Vec<_>>().join(","),
            },
        }
    }

    /// What the message says, from hostile bytes: `None` for a state no client sends. Of an allowed answer's abilities only the
    /// names of abilities are kept (each once); anything else in the text is left out.
    pub fn status(&self) -> Option<Status> {
        match self.state {
            STATE_DECLINED => Some(Status::Declined),
            STATE_UNAVAILABLE => Some(Status::Unavailable),
            STATE_ALLOWED => {
                let mut tiers: Vec<Tier> = Vec::new();
                for tier in self
                    .tiers
                    .split(',')
                    .filter_map(Tier::from_name)
                    .filter(|t| ABILITIES.contains(t))
                {
                    if !tiers.contains(&tier) {
                        tiers.push(tier);
                    }
                }
                Some(Status::Allowed(tiers))
            }
            _ => None,
        }
    }
}

/// One mod of an offer, checked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OfferedMod {
    /// The mod's id.
    pub id: String,
    /// The content hash its files must have.
    pub hash: [u8; 32],
    /// The entry file within the mod.
    pub entry: String,
    /// The abilities asked for, by name.
    pub tiers: Vec<String>,
}

impl ScriptOffer {
    /// The offer of `mods`, at most [`MAX_OFFERED_MODS`] of them, each name cut to what the message holds.
    pub fn of(mods: &[OfferedMod]) -> ScriptOffer {
        let mut offer = ScriptOffer::default();
        for m in mods.iter().take(MAX_OFFERED_MODS) {
            offer.ids.push(m.id.clone());
            offer.hashes.push(m.hash);
            offer.entries.push(m.entry.clone());
            offer.tiers.push(m.tiers.join(","));
        }
        offer
    }

    /// The mods the offer lists, from hostile bytes: lists of one length, each id a mod id, each entry a plain `.lua` or `.luau`
    /// path, each ability a plain name, an id once. An offer with any part wrong is refused whole.
    ///
    /// # Errors
    ///
    /// [`Refused::Malformed`] for such an offer.
    pub fn mods(&self) -> Result<Vec<OfferedMod>, Refused> {
        let count = self.ids.len();
        if count > MAX_OFFERED_MODS
            || self.hashes.len() != count
            || self.entries.len() != count
            || self.tiers.len() != count
        {
            return Err(Refused::Malformed);
        }
        let mut mods: Vec<OfferedMod> = Vec::with_capacity(count);
        for n in 0..count {
            let id = &self.ids[n];
            let entry = &self.entries[n];
            let lower = entry.to_ascii_lowercase();
            let plain_entry = entry.len() <= 128
                && (lower.ends_with(".lua") || lower.ends_with(".luau"))
                && torque_client_script_format::abi::check_script_path(entry).is_ok();
            let tiers: Vec<String> = self.tiers[n]
                .split(',')
                .filter(|t| !t.is_empty())
                .map(str::to_string)
                .collect();
            let plain_tiers = tiers.len() <= torque_mods::MAX_CLIENT_TIERS
                && tiers.iter().all(|t| {
                    t.len() <= 32
                        && t.bytes()
                            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
                });
            if torque_mods::check_id(id).is_err()
                || !plain_entry
                || !plain_tiers
                || mods.iter().any(|m| m.id.eq_ignore_ascii_case(id))
            {
                return Err(Refused::Malformed);
            }
            mods.push(OfferedMod {
                id: id.clone(),
                hash: self.hashes[n],
                entry: entry.clone(),
                tiers,
            });
        }
        Ok(mods)
    }
}

/// A byte allowance that refills with the server's ticks: the cap on how many bytes a second one connection's messages total
/// ([`UP_BYTES_PER_SECOND`], a second's worth in the burst), a [`TokenBucket`] on a clock of ticks.
#[derive(Clone, Copy, Debug)]
pub struct ByteBucket(TokenBucket<u64>);

/// The server's tick, in milliseconds: the engine's.
const TICK_MS: u64 = torque_engine::TickInfo::MS;

impl ByteBucket {
    /// A full bucket at `tick`.
    pub fn new(tick: u64) -> ByteBucket {
        ByteBucket(TokenBucket::starting_at(
            UP_BYTES_PER_SECOND,
            UP_BYTES_PER_SECOND,
            tick.saturating_mul(TICK_MS),
        ))
    }

    /// The tick of the last spend or refill.
    pub fn last_tick(&self) -> u64 {
        self.0.last().map_or(0, |ms| ms / TICK_MS)
    }

    /// Spends `bytes` at `tick`: whether the allowance had them.
    pub fn take(&mut self, tick: u64, bytes: usize) -> bool {
        u64::try_from(bytes).is_ok_and(|bytes| self.0.take(tick.saturating_mul(TICK_MS), bytes))
    }
}

/// `value` as the text a TorqueScript handler gets: a string as it is, except that a control character (a NUL, which would cut a
/// TorqueScript string short, a newline, an escape) becomes `\u00XX` as it does inside a table's JSON; a number or boolean as
/// the console writes it, nil as the empty string and a table as JSON text (keys in order, strings escaped). A handler never gets
/// a control character, so it cannot be cut short by one or sent a second line.
pub fn value_text(value: &Value) -> String {
    match value {
        Value::Nil => String::new(),
        Value::Bool(b) => if *b { "1" } else { "0" }.to_string(),
        Value::Number(n) => number_text(*n),
        Value::String(s) => escape_controls(&String::from_utf8_lossy(s)),
        Value::Table(_) => {
            let mut out = String::new();
            json(value, &mut out);
            out
        }
    }
}

fn number_text(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e15 {
        format!("{n:.0}")
    } else {
        n.to_string()
    }
}

fn json(value: &Value, out: &mut String) {
    match value {
        Value::Nil => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) if n.is_finite() => out.push_str(&number_text(*n)),
        Value::Number(_) => out.push_str("null"),
        Value::String(s) => json_string(&String::from_utf8_lossy(s), out),
        Value::Table(pairs) => {
            out.push('{');
            for (n, (key, item)) in pairs.iter().enumerate() {
                if n > 0 {
                    out.push(',');
                }
                json_string(&value_text(key), out);
                out.push(':');
                json(item, out);
            }
            out.push('}');
        }
    }
}

/// `text` with each control character as `\u00XX`.
fn escape_controls(text: &str) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if c.is_control() {
            let _ = write!(out, "\\u{:04x}", u32::from(c));
        } else {
            out.push(c);
        }
    }
    out
}

fn json_string(text: &str, out: &mut String) {
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if c.is_control() => {
                use std::fmt::Write as _;
                let _ = write!(out, "\\u{:04x}", u32::from(c));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_value_survives_the_chunks() {
        let value = Value::Table(vec![
            (Value::String(b"hp".to_vec()), Value::Number(42.0)),
            (
                Value::String(b"name".to_vec()),
                Value::String(b"x".repeat(100)),
            ),
        ]);
        let up = ScriptUp::of(&value).expect("fits");
        assert_eq!(up.value(), Ok(value.clone()));
        assert_eq!(ScriptDown::of(&value).expect("fits").value(), Ok(value));
    }

    #[test]
    fn text_for_a_script_never_carries_a_control_character() {
        let value = Value::Table(vec![(
            Value::String(b"a\"b".to_vec()),
            Value::String(b"line\nbreak\x00".to_vec()),
        )]);
        let text = value_text(&value);
        assert_eq!(text, "{\"a\\\"b\":\"line\\u000abreak\\u0000\"}");
        assert!(!text.chars().any(char::is_control));
        assert_eq!(
            value_text(&Value::String(b"a\0b\nc\x1b[0m\x7fd".to_vec())),
            "a\\u0000b\\u000ac\\u001b[0m\\u007fd",
            "a top-level string has no control character either"
        );
        assert_eq!(
            value_text(&Value::String(b"plain \"text\" \\".to_vec())),
            "plain \"text\" \\"
        );
        assert_eq!(value_text(&Value::Number(3.0)), "3");
        assert_eq!(value_text(&Value::Number(0.5)), "0.5");
        assert_eq!(value_text(&Value::Bool(true)), "1");
    }

    #[test]
    fn the_byte_bucket_spends_and_refills_with_ticks() {
        let mut bucket = ByteBucket::new(0);
        assert!(bucket.take(0, 64 << 10));
        assert!(!bucket.take(0, 1));
        // One second of ticks (31 of 32 ms) gives back about a second's worth.
        assert!(bucket.take(32, 60_000));
        assert!(!bucket.take(32, 60_000));
    }
}
