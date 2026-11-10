//! Values: what messages, call arguments and results, and `change` events carry (section 6).

use std::fmt;

use crate::codec::{Malformed, Reader, Writer};
use crate::{MAX_BATCH_NODES, MAX_DEPTH, MAX_TABLE_ENTRIES};

/// How many more values and keys may be decoded: [`MAX_BATCH_NODES`] for a batch, shared by all its values, so a batch's
/// decoded size is bounded whatever its bytes.
#[derive(Debug)]
pub struct NodeBudget(usize);

impl Default for NodeBudget {
    fn default() -> NodeBudget {
        NodeBudget(MAX_BATCH_NODES)
    }
}

impl NodeBudget {
    /// The nodes still to spend.
    #[must_use]
    pub fn left(&self) -> usize {
        self.0
    }

    fn spend(&mut self) -> Result<(), Malformed> {
        self.0 = self
            .0
            .checked_sub(1)
            .ok_or(Malformed("more values in a batch than MAX_BATCH_NODES"))?;
        Ok(())
    }
}

/// A value.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    /// `nil`.
    Nil,
    /// A boolean.
    Bool(bool),
    /// A number.
    Number(f64),
    /// A string (bytes).
    String(Vec<u8>),
    /// A table: its pairs, keys strings or numbers, each once.
    Table(Vec<(Value, Value)>),
}

/// The tags.
const NIL: u8 = 0;
const FALSE: u8 = 1;
const TRUE: u8 = 2;
const NUMBER: u8 = 3;
const STRING: u8 = 4;
const TABLE: u8 = 5;

/// The one NaN the format writes.
const CANONICAL_NAN: u64 = 0x7FF8_0000_0000_0000;
/// `-0`, which the format writes as `0`.
const NEGATIVE_ZERO: u64 = 0x8000_0000_0000_0000;

/// Why a value has no encoding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ValueError {
    /// Tables nest deeper than [`MAX_DEPTH`].
    TooDeep,
    /// A table has more than [`MAX_TABLE_ENTRIES`] pairs.
    TooManyEntries,
    /// A key that is not a string or a number, or is NaN.
    BadKey,
    /// A key twice in one table.
    DuplicateKey,
}

impl fmt::Display for ValueError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            ValueError::TooDeep => "tables nest more than 8 deep",
            ValueError::TooManyEntries => "a table has more than 256 entries",
            ValueError::BadKey => "a key is neither a string nor a number (or is NaN)",
            ValueError::DuplicateKey => "a key is in a table twice",
        })
    }
}

impl std::error::Error for ValueError {}

/// A key's identity, for the once-each rule: numbers by their bits with `-0` as `0`.
#[derive(PartialEq, Eq, PartialOrd, Ord)]
enum KeyId<'a> {
    Number(u64),
    String(&'a [u8]),
}

fn key_id(key: &Value) -> Result<KeyId<'_>, ValueError> {
    match key {
        Value::Number(n) if n.is_nan() => Err(ValueError::BadKey),
        Value::Number(n) => Ok(KeyId::Number(if *n == 0.0 { 0 } else { n.to_bits() })),
        Value::String(s) => Ok(KeyId::String(s)),
        _ => Err(ValueError::BadKey),
    }
}

/// Checks a table's pairs: the count and every key.
fn check_pairs(pairs: &[(Value, Value)]) -> Result<(), ValueError> {
    if pairs.len() > MAX_TABLE_ENTRIES {
        return Err(ValueError::TooManyEntries);
    }
    let mut ids = pairs
        .iter()
        .map(|(k, _)| key_id(k))
        .collect::<Result<Vec<_>, _>>()?;
    ids.sort_unstable();
    if ids.windows(2).any(|w| w[0] == w[1]) {
        return Err(ValueError::DuplicateKey);
    }
    Ok(())
}

impl Value {
    /// Writes the value, or `Err` (and nothing written) when it breaks a rule of values (depth, entries, keys).
    pub fn encode(&self, out: &mut Writer) -> Result<(), ValueError> {
        self.check(0)?;
        self.write(out);
        Ok(())
    }

    /// The value's bytes, alone.
    pub fn to_bytes(&self) -> Result<Vec<u8>, ValueError> {
        let mut out = Writer::new();
        self.encode(&mut out)?;
        Ok(out.into_bytes())
    }

    fn check(&self, depth: usize) -> Result<(), ValueError> {
        if let Value::Table(pairs) = self {
            if depth >= MAX_DEPTH {
                return Err(ValueError::TooDeep);
            }
            check_pairs(pairs)?;
            for (k, v) in pairs {
                k.check(depth + 1)?;
                v.check(depth + 1)?;
            }
        }
        Ok(())
    }

    fn write(&self, out: &mut Writer) {
        match self {
            Value::Nil => out.u8(NIL),
            Value::Bool(false) => out.u8(FALSE),
            Value::Bool(true) => out.u8(TRUE),
            Value::Number(n) => {
                out.u8(NUMBER);
                if n.is_nan() {
                    out.u64(CANONICAL_NAN);
                } else if *n == 0.0 {
                    out.f64(0.0);
                } else {
                    out.f64(*n);
                }
            }
            Value::String(s) => {
                out.u8(STRING);
                out.bytes_field(s);
            }
            Value::Table(pairs) => {
                out.u8(TABLE);
                out.u16(pairs.len() as u16);
                for (k, v) in pairs {
                    k.write(out);
                    v.write(out);
                }
            }
        }
    }

    /// Reads one value, checking every rule (tags, NaN form, keys, counts, depth), each value and key it holds taken
    /// from `budget` (one budget for a whole batch).
    pub fn decode(reader: &mut Reader<'_>, budget: &mut NodeBudget) -> Result<Value, Malformed> {
        Value::read(reader, 0, budget)
    }

    /// The value that is the whole of `bytes`, within a budget of its own.
    pub fn from_bytes(bytes: &[u8]) -> Result<Value, Malformed> {
        let mut reader = Reader::new(bytes);
        let value = Value::decode(&mut reader, &mut NodeBudget::default())?;
        reader.finish()?;
        Ok(value)
    }

    fn read(
        reader: &mut Reader<'_>,
        depth: usize,
        budget: &mut NodeBudget,
    ) -> Result<Value, Malformed> {
        budget.spend()?;
        Ok(match reader.u8()? {
            NIL => Value::Nil,
            FALSE => Value::Bool(false),
            TRUE => Value::Bool(true),
            NUMBER => {
                let bits = reader.u64()?;
                let n = f64::from_bits(bits);
                if (n.is_nan() && bits != CANONICAL_NAN) || bits == NEGATIVE_ZERO {
                    return Err(Malformed("a NaN or zero not in its one form"));
                }
                Value::Number(n)
            }
            STRING => Value::String(reader.bytes_field()?.to_vec()),
            TABLE => {
                if depth >= MAX_DEPTH {
                    return Err(Malformed("tables nested past MAX_DEPTH"));
                }
                let count = usize::from(reader.u16()?);
                if count > MAX_TABLE_ENTRIES {
                    return Err(Malformed("a table with more than MAX_TABLE_ENTRIES pairs"));
                }
                // Each pair takes at least two bytes, so the count is bounded by what is left.
                let mut pairs = Vec::with_capacity(count.min(reader.rest().len() / 2));
                for _ in 0..count {
                    let key = Value::read(reader, depth + 1, budget)?;
                    let value = Value::read(reader, depth + 1, budget)?;
                    pairs.push((key, value));
                }
                check_pairs(&pairs).map_err(|e| match e {
                    ValueError::DuplicateKey => Malformed("a key twice in a table"),
                    _ => Malformed("a key that is neither a string nor a number"),
                })?;
                Value::Table(pairs)
            }
            _ => return Err(Malformed("an unknown value tag")),
        })
    }
}
