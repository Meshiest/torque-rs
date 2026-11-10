//! The `net` tier: messages and calls to the realm's own server, and the values they carry.

use std::collections::HashMap;

use mlua::{Error, Function, Lua, LuaString, MultiValue, Table, Value};

use super::format::net as fnet;
use super::format::{self, Malformed, NodeBudget, Reader, Tier, event, op};
use super::{
    CallbackId, add_handler, function, granted, invoke_kept, keep, name_of, release, state,
};
use crate::realm::table_slots;

/// The most slots a table sent may have (its 256 entries, and room to spare for how Luau sizes tables).
const MAX_SENT_SLOTS: u64 = 4 * format::MAX_TABLE_ENTRIES as u64;

/// One message (or one byte) in the buckets' units.
const UNIT: u64 = 1_000_000;

/// The realm's messages and calls.
pub(crate) struct NetState {
    /// Messages left in the bucket, in millionths.
    messages: u64,
    /// Bytes left in the bucket, in millionths.
    bytes: u64,
    refilled: Option<u64>,
    /// The second a refusal was last logged in.
    noticed: Option<u64>,
    next_call: u32,
    calls: HashMap<u32, (String, CallbackId)>,
    outstanding: HashMap<String, usize>,
    message_handlers: Vec<CallbackId>,
    overflow_handlers: Vec<CallbackId>,
    rpc_handlers: HashMap<String, CallbackId>,
}

impl Default for NetState {
    fn default() -> NetState {
        NetState {
            messages: u64::from(fnet::SEND_BURST) * UNIT,
            bytes: u64::from(fnet::SEND_BYTES_PER_SECOND) * UNIT,
            refilled: None,
            noticed: None,
            next_call: 0,
            calls: HashMap::new(),
            outstanding: HashMap::new(),
            message_handlers: Vec::new(),
            overflow_handlers: Vec::new(),
            rpc_handlers: HashMap::new(),
        }
    }
}

impl NetState {
    /// Takes one message of `len` bytes from the buckets at `now`, or `false` when they are empty.
    fn take(&mut self, now: u64, len: usize) -> bool {
        let elapsed = self.refilled.map_or(0, |at| now.saturating_sub(at));
        self.refilled = Some(now);
        let refill = |left: u64, per_second: u32, cap: u64| {
            left.saturating_add(elapsed.saturating_mul(u64::from(per_second)))
                .min(cap)
        };
        self.messages = refill(
            self.messages,
            fnet::SENDS_PER_SECOND,
            u64::from(fnet::SEND_BURST) * UNIT,
        );
        self.bytes = refill(
            self.bytes,
            fnet::SEND_BYTES_PER_SECOND,
            u64::from(fnet::SEND_BYTES_PER_SECOND) * UNIT,
        );
        let cost = (len as u64).saturating_mul(UNIT);
        if self.messages < UNIT || self.bytes < cost {
            return false;
        }
        self.messages -= UNIT;
        self.bytes -= cost;
        true
    }

    /// Forgets every handler and call, for a revocation: the kept functions to let go.
    pub(crate) fn revoke(&mut self) -> Vec<CallbackId> {
        let mut released: Vec<CallbackId> = self.calls.drain().map(|(_, (_, f))| f).collect();
        released.append(&mut self.message_handlers);
        released.append(&mut self.overflow_handlers);
        released.extend(self.rpc_handlers.drain().map(|(_, f)| f));
        self.outstanding.clear();
        released
    }
}

/// `value` as the format's value, read without metamethods, within `left` bytes of encoding.
fn from_lua(value: &Value, depth: usize, left: &mut usize) -> mlua::Result<format::Value> {
    let mut spend = |n: usize| {
        if *left < n {
            return Err(Error::runtime("net: the value is too large"));
        }
        *left -= n;
        Ok(())
    };
    Ok(match value {
        Value::Nil => {
            spend(1)?;
            format::Value::Nil
        }
        Value::Boolean(b) => {
            spend(1)?;
            format::Value::Bool(*b)
        }
        Value::Integer(i) => {
            spend(9)?;
            format::Value::Number(*i as f64)
        }
        Value::Number(n) => {
            spend(9)?;
            format::Value::Number(*n)
        }
        Value::String(s) => {
            let bytes = s.as_bytes();
            spend(5usize.saturating_add(bytes.len()))?;
            format::Value::String(bytes.to_vec())
        }
        Value::Table(t) => {
            if depth >= format::MAX_DEPTH {
                return Err(Error::runtime("net: tables nest at most 8 deep"));
            }
            // Its slots are counted before it is walked: a sparse table of a huge capacity costs nothing unmetered.
            if table_slots(t) > MAX_SENT_SLOTS {
                return Err(Error::runtime("net: a table has at most 256 entries"));
            }
            spend(3)?;
            let mut pairs = Vec::new();
            for pair in t.pairs::<Value, Value>() {
                let (k, v) = pair?;
                if pairs.len() >= format::MAX_TABLE_ENTRIES {
                    return Err(Error::runtime("net: a table has at most 256 entries"));
                }
                if !matches!(k, Value::String(_) | Value::Number(_) | Value::Integer(_)) {
                    return Err(Error::runtime("net: a table's keys are strings or numbers"));
                }
                pairs.push((
                    from_lua(&k, depth + 1, left)?,
                    from_lua(&v, depth + 1, left)?,
                ));
            }
            format::Value::Table(pairs)
        }
        other => {
            return Err(Error::runtime(format!(
                "net: a {} cannot be sent (only nil, booleans, numbers, strings and tables)",
                other.type_name()
            )));
        }
    })
}

/// `value` encoded, at most `max` bytes.
fn encode(value: &Value, max: usize) -> mlua::Result<Vec<u8>> {
    let mut left = max;
    // The rules a Lua table cannot break the format checks anyway (a key twice, a NaN key).
    from_lua(value, 0, &mut left)?
        .to_bytes()
        .map_err(|e| Error::runtime(format!("net: {e}")))
}

/// A value of the format as Lua's.
pub(crate) fn value_to_lua(lua: &Lua, value: &format::Value) -> mlua::Result<Value> {
    Ok(match value {
        format::Value::Nil => Value::Nil,
        format::Value::Bool(b) => Value::Boolean(*b),
        format::Value::Number(n) => Value::Number(*n),
        format::Value::String(s) => Value::String(lua.create_string(s)?),
        format::Value::Table(pairs) => {
            let t = lua.create_table_with_capacity(0, pairs.len())?;
            for (k, v) in pairs {
                t.raw_set(value_to_lua(lua, k)?, value_to_lua(lua, v)?)?;
            }
            Value::Table(t)
        }
    })
}

/// Whether the tier was revoked (its functions then answer `false`).
fn revoked(lua: &Lua) -> mlua::Result<bool> {
    Ok(!granted(lua, Tier::Net)?)
}

/// Takes a send from the buckets, logging one warning a second when they are empty.
fn admit(lua: &Lua, len: usize) -> mlua::Result<bool> {
    let mut st = state(lua)?;
    let now = st.now;
    if st.net.take(now, len) {
        return Ok(true);
    }
    let second = now / 1_000_000;
    if st.net.noticed != Some(second) {
        st.net.noticed = Some(second);
        st.log(
            op::realm::LEVEL_WARNING,
            "net: messages to the server are over their rate; some were not sent",
        );
    }
    Ok(false)
}

/// The `net` table (one for the realm).
pub(crate) fn table(lua: &Lua) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    t.raw_set("send", function(lua, send)?)?;
    t.raw_set("call", function(lua, call)?)?;
    t.raw_set("on", function(lua, on)?)?;
    t.raw_set("handle", function(lua, handle)?)?;
    t.set_readonly(true);
    Ok(t)
}

/// `net.send(value)`: `false` when over the rate.
fn send(lua: &Lua, args: MultiValue) -> mlua::Result<bool> {
    if revoked(lua)? {
        return Ok(false);
    }
    let (1, Some(value)) = (args.len(), args.front()) else {
        return Err(Error::runtime(
            "net.send takes one value: messages go to this realm's own server only",
        ));
    };
    let bytes = encode(value, fnet::MAX_SEND_BYTES)?;
    if !admit(lua, bytes.len())? {
        return Ok(false);
    }
    state(lua)?.emit(Tier::Net, op::net::SEND, |w| w.raw(&bytes))?;
    Ok(true)
}

/// `net.call(name, args, fn)`: `false` when over the rate; `fn(ok, result)` gets the reply.
fn call(lua: &Lua, (name, args, f): (LuaString, Value, Function)) -> mlua::Result<bool> {
    if revoked(lua)? {
        return Ok(false);
    }
    let name = name_of(&name);
    fnet::check_rpc_name(&name).map_err(|e| Error::runtime(format!("net.call: {e}")))?;
    let bytes = encode(&args, fnet::MAX_CALL_BYTES)?;
    {
        let st = state(lua)?;
        if st.net.outstanding.get(&name).copied().unwrap_or(0) >= fnet::MAX_OUTSTANDING {
            return Err(Error::runtime(
                "net.call: at most 16 calls of one RPC wait for their reply",
            ));
        }
        if st.net.calls.len() >= fnet::MAX_OUTSTANDING_TOTAL {
            return Err(Error::runtime(
                "net.call: at most 64 calls wait for their replies",
            ));
        }
    }
    if !admit(lua, bytes.len())? {
        return Ok(false);
    }
    let callback = keep(lua, f)?;
    let emitted = {
        let mut st = state(lua)?;
        st.net
            .next_call
            .checked_add(1)
            .ok_or_else(|| Error::runtime("net.call: this realm has made all the calls it can"))
            .and_then(|id| {
                st.emit(Tier::Net, op::net::CALL, |w| {
                    w.u32(id);
                    w.text(&name);
                    w.raw(&bytes);
                })?;
                st.net.next_call = id;
                *st.net.outstanding.entry(name.clone()).or_insert(0) += 1;
                st.net.calls.insert(id, (name, callback));
                Ok(())
            })
    };
    if let Err(error) = emitted {
        release(lua, callback);
        return Err(error);
    }
    Ok(true)
}

/// `net.on(fn)` for messages, `net.on("overflow", fn)` for dropped ones.
fn on(lua: &Lua, (first, second): (Value, Option<Function>)) -> mlua::Result<bool> {
    if revoked(lua)? {
        return Ok(false);
    }
    match (first, second) {
        (Value::Function(f), None) => add_handler(lua, f, |st| &mut st.net.message_handlers)?,
        (Value::String(s), Some(f)) if s.as_bytes().as_ref() == b"overflow" => {
            add_handler(lua, f, |st| &mut st.net.overflow_handlers)?
        }
        _ => {
            return Err(Error::runtime(
                "net.on takes a function for messages, or \"overflow\" and a function",
            ));
        }
    }
    Ok(true)
}

/// `net.handle(name, fn)`: the server's RPC `name` (one it marked for the realm) reaches `fn`.
fn handle(lua: &Lua, (name, f): (LuaString, Function)) -> mlua::Result<bool> {
    if revoked(lua)? {
        return Ok(false);
    }
    let name = name_of(&name);
    fnet::check_rpc_name(&name).map_err(|e| Error::runtime(format!("net.handle: {e}")))?;
    let callback = keep(lua, f)?;
    let old = state(lua)?.net.rpc_handlers.insert(name, callback);
    if let Some(old) = old {
        release(lua, old);
    }
    Ok(true)
}

/// A `net` event.
pub(crate) enum NetEvent {
    Message(format::Value),
    Reply {
        id: u32,
        ok: bool,
        value: format::Value,
    },
    Rpc {
        name: String,
        value: format::Value,
    },
    Overflow,
}

/// Reads a `net` event's body.
pub(crate) fn decode(
    kind: u8,
    r: &mut Reader<'_>,
    budget: &mut NodeBudget,
) -> Result<NetEvent, Malformed> {
    Ok(match kind {
        event::net::MESSAGE => NetEvent::Message(super::read_value(r, budget)?),
        event::net::REPLY => NetEvent::Reply {
            id: r.u32()?,
            ok: r.bool()?,
            value: super::read_value(r, budget)?,
        },
        event::net::RPC => {
            let name = r.text()?.to_string();
            fnet::check_rpc_name(&name).map_err(|_| Malformed("an RPC name out of its form"))?;
            NetEvent::Rpc {
                name,
                value: super::read_value(r, budget)?,
            }
        }
        event::net::OVERFLOW => NetEvent::Overflow,
        _ => return Err(Malformed("an unknown net event")),
    })
}

/// Runs the `overflow` callbacks.
pub(crate) fn overflow(lua: &Lua) {
    let handlers = state(lua)
        .map(|st| st.net.overflow_handlers.clone())
        .unwrap_or_default();
    for id in handlers {
        invoke_kept(lua, op::realm::ENTRY_EVENT, id, ());
    }
}

/// Runs a queued `net` event's callbacks.
pub(crate) fn dispatch(lua: &Lua, e: NetEvent) {
    let entry = op::realm::ENTRY_EVENT;
    match e {
        NetEvent::Message(value) => {
            let handlers = state(lua)
                .map(|st| st.net.message_handlers.clone())
                .unwrap_or_default();
            for id in handlers {
                if let Ok(v) = value_to_lua(lua, &value) {
                    invoke_kept(lua, entry, id, v);
                }
            }
        }
        NetEvent::Reply { id, ok, value } => {
            let call = state(lua).ok().and_then(|mut st| {
                let (name, callback) = st.net.calls.remove(&id)?;
                if let Some(n) = st.net.outstanding.get_mut(&name) {
                    *n -= 1;
                    if *n == 0 {
                        st.net.outstanding.remove(&name);
                    }
                }
                Some(callback)
            });
            if let Some(callback) = call {
                if let Ok(v) = value_to_lua(lua, &value) {
                    invoke_kept(lua, entry, callback, (ok, v));
                }
                release(lua, callback);
            }
        }
        NetEvent::Rpc { name, value } => {
            let handler = state(lua)
                .ok()
                .and_then(|st| st.net.rpc_handlers.get(&name).copied());
            if let Some(id) = handler
                && let Ok(v) = value_to_lua(lua, &value)
            {
                invoke_kept(lua, entry, id, v);
            }
        }
        NetEvent::Overflow => overflow(lua),
    }
}
