//! Messages: the typed requests and replies of a plugin's RPCs (`#[derive(NetMessage)]`).
//!
//! A message is a struct of [`NetField`]s with the same layout machinery as a ghost record ([`Layout`]: one function run by the
//! encoder, the decoder of hostile bytes and the logging encoder), and a few more rules because its bytes come from a peer that
//! chooses them:
//!
//! - a `String` or a `Vec` needs a `max_len` (a message has no default cap);
//! - a float needs `#[net(finite)]`, which makes the decoder refuse a NaN or an infinity;
//! - a [`GhostRef`](super::field::GhostRef) names an object the connection can see, and the server checks that it does
//!   ([`NetMessage::ghost_refs`]);
//! - a type with no [`NetField`] (a raw object id, a map) does not compile.

use std::fmt::Debug;

use super::field::NetField;
use crate::codec::{Codec, Layout, R};
use crate::{ObjectView, Value};

/// A request or a reply of an RPC (`#[derive(NetMessage)]`).
pub trait NetMessage: Layout + Debug + PartialEq + Send + Sync + 'static {
    /// The message's layout as text (each field's name, type, width and cap in order): what the manifest's digest is made of.
    const SCHEMA: &'static str;

    /// The names of the fields in wire order, which a script call fills from its arguments.
    fn field_names() -> &'static [&'static str];

    /// Appends the ghost indexes the message names ([`GhostRef`](super::field::GhostRef)).
    fn ghost_refs(&self, out: &mut Vec<u32>);

    /// Replaces each ghost index the message holds with what `map` makes of it ([`NetField::map_ghosts`]); false when it has no
    /// answer for one.
    fn map_ghosts(&mut self, map: &mut dyn FnMut(u32) -> Option<u32>) -> bool;

    /// Whether the values are what the message's fields allow after a decode (the floats marked `finite` are finite).
    fn validate(&self) -> Result<(), NetError>;

    /// The message from a script call's arguments, one for each field in order (a missing or malformed one reads as the
    /// field's zero value). Fields that are lists (`Vec`) are not filled from a script call.
    fn from_script(args: &[Vec<u8>]) -> Self;

    /// The message as script values, one or more for each field in order (a `Vec` gives one for each element).
    fn script_values(&self) -> Vec<Value>;
}

impl Layout for () {
    fn layout<C: Codec>(&mut self, _c: &mut C) -> R {
        Ok(())
    }
}

impl NetMessage for () {
    const SCHEMA: &'static str = "";

    fn field_names() -> &'static [&'static str] {
        &[]
    }

    fn ghost_refs(&self, _out: &mut Vec<u32>) {}

    fn map_ghosts(&mut self, _map: &mut dyn FnMut(u32) -> Option<u32>) -> bool {
        true
    }

    fn validate(&self) -> Result<(), NetError> {
        Ok(())
    }

    fn from_script(_args: &[Vec<u8>]) -> Self {}

    fn script_values(&self) -> Vec<Value> {
        Vec::new()
    }
}

pub use torque_net::NetError;

/// The error a decoded message gives when a float marked `finite` is not.
pub fn non_finite() -> NetError {
    NetError::Invalid("a float that is not finite")
}

/// A script call's arguments as a view, so a message's fields read from them as a ghost record's read from an object:
/// field `names[i]` is argument `i` as text (the fields parse it as the console does), element 0 of it.
pub struct ScriptArgs<'a> {
    names: &'a [&'static str],
    args: &'a [Vec<u8>],
}

impl<'a> ScriptArgs<'a> {
    /// The view of `args` for fields named `names`.
    pub fn new(names: &'a [&'static str], args: &'a [Vec<u8>]) -> ScriptArgs<'a> {
        ScriptArgs { names, args }
    }
}

impl ObjectView for ScriptArgs<'_> {
    fn get(&self, field: &str, index: usize) -> Value {
        let Some(at) = self.names.iter().position(|n| *n == field) else {
            return Value::Null;
        };
        match (index, self.args.get(at)) {
            (0, Some(arg)) => {
                Value::Str(arg.split(|b| *b == 0).next().unwrap_or_default().to_vec())
            }
            _ => Value::Null,
        }
    }
}

/// The values of a field as script values, appended to `out` (for the derive).
pub fn push_script_values<T: NetField>(out: &mut Vec<Value>, value: &T) {
    value.values(out);
}
