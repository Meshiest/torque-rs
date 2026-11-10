//! A realm's `net.call` of the server's own RPCs: which RPCs it may name, its table of arguments as the RPC's script
//! arguments, and the reply and the errors as the realm sees them.
//!
//! **Which RPCs.** Only a `to_server` RPC registered with [`RpcBuilder::realm`](torque_engine::rpc::RpcBuilder::realm), and never
//! one of this crate's own. The realm's table must name only fields the request has (a name that is none is refused locally,
//! as a mismatch of the layout), each value a nil, boolean, number, string, or a list of up to 16 numbers (a vector), each at
//! most 255 bytes of text (a number or a vector's numbers written out, joined by spaces) without a NUL. The message is then built from
//! those arguments exactly as a console call builds it, and the link validates it again (size, `finite` floats, ghost fields: a
//! realm has no object to name, so a call that needs one is refused). The server decodes and checks it as it checks any client's.

use torque_client_script_format::Value;
use torque_engine::rpc::{RpcDirection, RpcEntry, RpcError, RpcTable};

use crate::messages::{DOWN, OFFER, STATUS, UP};

/// The most bytes of one text argument.
pub const MAX_ARG_BYTES: usize = 255;
/// The most numbers of a vector argument.
pub const MAX_VECTOR: usize = 16;

/// The RPC `name` a realm may call, from `table`: a `to_server` RPC marked for the realm that is not one of this crate's.
pub fn callable<'a>(table: &'a RpcTable, name: &str) -> Option<&'a RpcEntry> {
    if [UP, DOWN, STATUS, OFFER].contains(&name) {
        return None;
    }
    let entry = table.get(table.id(name)?)?;
    (entry.realm && entry.direction == RpcDirection::ToServer).then_some(entry)
}

/// The script arguments of `args` for `entry`: one for each field of its request, in order. `None` when the realm's table does
/// not fit the request's layout.
pub fn script_args(entry: &RpcEntry, args: &Value) -> Option<Vec<Vec<u8>>> {
    let fields = entry.request_fields();
    let pairs: &[(Value, Value)] = match args {
        Value::Nil => &[],
        Value::Table(pairs) => pairs,
        _ => return None,
    };
    let mut out: Vec<Vec<u8>> = vec![Vec::new(); fields.len()];
    for (key, value) in pairs {
        let Value::String(name) = key else {
            return None;
        };
        let at = fields
            .iter()
            .position(|f| f.as_bytes() == name.as_slice())?;
        out[at] = argument(value)?;
    }
    Some(out)
}

/// One argument's text, at most [`MAX_ARG_BYTES`] whatever the value (a 300-digit number is as long as a string).
fn argument(value: &Value) -> Option<Vec<u8>> {
    argument_text(value).filter(|text| text.len() <= MAX_ARG_BYTES)
}

fn argument_text(value: &Value) -> Option<Vec<u8>> {
    match value {
        Value::Nil => Some(Vec::new()),
        Value::Bool(b) => Some(vec![if *b { b'1' } else { b'0' }]),
        Value::Number(n) if n.is_finite() => Some(number(*n).into_bytes()),
        Value::Number(_) => None,
        Value::String(s) => (s.len() <= MAX_ARG_BYTES && !s.contains(&0)).then(|| s.clone()),
        Value::Table(pairs) => {
            if pairs.len() > MAX_VECTOR {
                return None;
            }
            let mut items: Vec<(f64, f64)> = Vec::with_capacity(pairs.len());
            for (key, item) in pairs {
                match (key, item) {
                    (Value::Number(k), Value::Number(n)) if n.is_finite() => items.push((*k, *n)),
                    _ => return None,
                }
            }
            items.sort_by(|a, b| a.0.total_cmp(&b.0));
            let words: Vec<String> = items.into_iter().map(|(_, n)| number(n)).collect();
            Some(words.join(" ").into_bytes())
        }
    }
}

fn number(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e15 {
        format!("{n:.0}")
    } else {
        n.to_string()
    }
}

/// A reply's script values as the realm's value: nothing is nil, one value itself, several a list.
pub fn reply_value(values: &[torque_ghosts::Value]) -> Value {
    let one = |v: &torque_ghosts::Value| match v {
        torque_ghosts::Value::Null
        | torque_ghosts::Value::Id(None)
        | torque_ghosts::Value::Xform(None) => Value::Nil,
        torque_ghosts::Value::Bool(b) => Value::Bool(*b),
        torque_ghosts::Value::Int(n) => Value::Number(*n as f64),
        torque_ghosts::Value::F32(f) if f.is_finite() => Value::Number(f64::from(*f)),
        torque_ghosts::Value::F32(_) => Value::Nil,
        torque_ghosts::Value::Str(s) => {
            Value::String(s.iter().copied().take(MAX_ARG_BYTES).collect())
        }
        torque_ghosts::Value::Id(Some(id)) => Value::Number(f64::from(*id)),
        torque_ghosts::Value::Xform(Some(_)) => Value::Nil,
    };
    match values {
        [] => Value::Nil,
        [only] => one(only),
        many => Value::Table(
            many.iter()
                .enumerate()
                .map(|(n, v)| (Value::Number((n + 1) as f64), one(v)))
                .collect(),
        ),
    }
}

/// The short text a realm gets for a failed call: `refused`, `rate limited`, `too large`, `timeout`, `disconnected`, or the
/// handler's own text, plain and at most 255 bytes.
pub fn error_text(error: &RpcError) -> String {
    match error {
        RpcError::Refused | RpcError::Invalid | RpcError::Unsupported => "refused".to_string(),
        RpcError::RateLimited | RpcError::Busy => "rate limited".to_string(),
        RpcError::TooLarge => "too large".to_string(),
        RpcError::Timeout => "timeout".to_string(),
        RpcError::Disconnected => "disconnected".to_string(),
        RpcError::Handler(text) => text
            .chars()
            .map(|c| {
                if c.is_control() || torque_client_script_format::is_hidden(c) {
                    ' '
                } else {
                    c
                }
            })
            .take(MAX_ARG_BYTES)
            .collect(),
        _ => "refused".to_string(),
    }
}
