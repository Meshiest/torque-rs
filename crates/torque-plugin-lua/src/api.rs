//! The `ts` table a script sees, and objects: what a Lua script can do with its realm's script engine.

use mlua::{
    AnyUserData, Error, Function, Lua, MetaMethod, MultiValue, Table, UserData, UserDataMethods,
    Value,
};
use torque_plugin_sdk::abi::TORQUE_MAX_ARGS;
use torque_plugin_sdk::{Definition, Side};

use crate::realm::{self, charge_bytes, charged_function, engine, with_call};
use crate::value::{from_text, texts, to_text};

/// An object of the script engine, by its id: `obj.field`, `obj.field = value`, `obj:method(...)`, `obj.id`.
pub(crate) struct Object(pub(crate) u32);

impl UserData for Object {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_meta_method(MetaMethod::Index, |lua, this, key: mlua::LuaString| {
            let key = key.to_str()?.to_string();
            if key == "id" {
                // A number (Luau has no integer type; mlua's `Integer` is 32 bits on wasm32).
                return Ok(Value::Number(f64::from(this.0)));
            }
            let id = this.0;
            if with_call(lua, |call| call.has_method(id, &key))? {
                return bound_method(lua, id, key).map(Value::Function);
            }
            match with_call(lua, |call| call.field(id, key.as_bytes(), b""))? {
                Some(value) => from_text(lua, &value),
                None => Ok(Value::Nil),
            }
        });
        methods.add_meta_method(
            MetaMethod::NewIndex,
            |lua, this, (key, value): (mlua::LuaString, Value)| {
                let key = key.as_bytes().to_vec();
                if key == b"id" {
                    return Err(Error::runtime("an object's id cannot be set"));
                }
                let value = to_text(&value)?;
                let id = this.0;
                if !with_call(lua, |call| call.set_field(id, &key, b"", &value))? {
                    return Err(Error::runtime(format!(
                        "the field `{}` of {id} was not set (no such object, or the engine refused it)",
                        String::from_utf8_lossy(&key)
                    )));
                }
                Ok(())
            },
        );
        methods.add_meta_method(MetaMethod::ToString, |_, this, ()| Ok(this.0.to_string()));
        methods.add_meta_function(MetaMethod::Eq, |_, (a, b): (AnyUserData, AnyUserData)| {
            Ok(match (a.borrow::<Object>(), b.borrow::<Object>()) {
                (Ok(a), Ok(b)) => a.0 == b.0,
                _ => false,
            })
        });
    }
}

/// `obj:name(...)`: the method `name` of the object `id`.
fn bound_method(lua: &Lua, id: u32, name: String) -> mlua::Result<Function> {
    charged_function(lua, charge_bytes, move |lua, args: MultiValue| {
        let mut args = args.into_iter();
        match args.next() {
            Some(Value::UserData(data)) if data.borrow::<Object>().is_ok_and(|o| o.0 == id) => {}
            _ => {
                return Err(Error::runtime(format!(
                    "call the method as obj:{name}(...), with a colon"
                )));
            }
        }
        call_method(lua, id, &name, &args.collect())
    })
}

/// Runs the method `name` of `id` with `args`. The engine's call policy refuses what writes files or runs code from text
/// (whatever names it), and says why on the console.
fn call_method(lua: &Lua, id: u32, name: &str, args: &MultiValue) -> mlua::Result<Value> {
    let args = texts(args, TORQUE_MAX_ARGS - 1)?;
    let refs: Vec<&[u8]> = args.iter().map(Vec::as_slice).collect();
    let result = with_call(lua, |call| call.call_method(id, name, &refs))?;
    let result = engine(lua, name, result)?;
    from_text(lua, &result)
}

/// A function or method name as a script writes it: `name`, `Namespace::name`.
fn split_name(full: &str) -> (&str, &str) {
    full.rsplit_once("::").unwrap_or(("", full))
}

/// The object a value names: an object, its id, or its name.
fn object_of(lua: &Lua, value: &Value) -> mlua::Result<Option<u32>> {
    if let Value::UserData(data) = value
        && let Ok(object) = data.borrow::<Object>()
    {
        return Ok(Some(object.0));
    }
    let text = to_text(value)?;
    if text.is_empty() {
        return Ok(None);
    }
    with_call(lua, |call| call.find_object(&text))
}

/// A global's name with its `$`.
fn global_name(name: &str) -> String {
    if name.starts_with('$') {
        name.to_string()
    } else {
        format!("${name}")
    }
}

/// The `ts` table of one mod's (or folder's) global table: its definitions go into `package`.
pub(crate) fn ts_table(lua: &Lua, package: &str) -> mlua::Result<Table> {
    let ts = lua.create_table()?;
    ts.raw_set(
        "realm",
        match realm::side(lua) {
            Side::Server => "server",
            Side::Client => "client",
            Side::Both => "tool",
        },
    )?;
    ts.raw_set(
        "call",
        charged_function(
            lua,
            charge_bytes,
            |lua, (name, args): (String, MultiValue)| {
                let (namespace, function) = split_name(&name);
                let args = texts(&args, TORQUE_MAX_ARGS)?;
                let refs: Vec<&[u8]> = args.iter().map(Vec::as_slice).collect();
                let result = with_call(lua, |call| call.call_function(namespace, function, &refs))?;
                let result = engine(lua, &name, result)?;
                from_text(lua, &result)
            },
        )?,
    )?;
    ts.raw_set(
        "method",
        charged_function(
            lua,
            charge_bytes,
            |lua, (object, name, args): (Value, String, MultiValue)| match object_of(lua, &object)?
            {
                Some(id) => call_method(lua, id, &name, &args),
                None => Ok(Value::String(lua.create_string("")?)),
            },
        )?,
    )?;
    ts.raw_set(
        "object",
        charged_function(lua, charge_bytes, |lua, name: Value| {
            match object_of(lua, &name)? {
                Some(id) => lua.create_userdata(Object(id)).map(Value::UserData),
                None => Ok(Value::Nil),
            }
        })?,
    )?;
    ts.raw_set(
        "exists",
        charged_function(lua, charge_bytes, |lua, object: Value| {
            let id = match &object {
                Value::UserData(data) => data.borrow::<Object>().ok().map(|o| o.0),
                _ => None,
            };
            Ok(match id {
                Some(id) => {
                    with_call(lua, |call| call.find_object(id.to_string().as_bytes()))?.is_some()
                }
                None => object_of(lua, &object)?.is_some(),
            })
        })?,
    )?;
    ts.raw_set(
        "field",
        charged_function(
            lua,
            charge_bytes,
            |lua, (object, name, index): (Value, String, Option<Value>)| {
                let Some(id) = object_of(lua, &object)? else {
                    return Ok(Value::Nil);
                };
                let index = index.as_ref().map(to_text).transpose()?.unwrap_or_default();
                match with_call(lua, |call| call.field(id, name.as_bytes(), &index))? {
                    Some(value) => from_text(lua, &value),
                    None => Ok(Value::Nil),
                }
            },
        )?,
    )?;
    ts.raw_set(
        "setField",
        charged_function(
            lua,
            charge_bytes,
            |lua, (object, name, value, index): (Value, String, Value, Option<Value>)| {
                let Some(id) = object_of(lua, &object)? else {
                    return Ok(false);
                };
                let index = index.as_ref().map(to_text).transpose()?.unwrap_or_default();
                let value = to_text(&value)?;
                with_call(lua, |call| {
                    call.set_field(id, name.as_bytes(), &index, &value)
                })
            },
        )?,
    )?;
    ts.raw_set(
        "global",
        charged_function(lua, charge_bytes, |lua, name: String| {
            let value = with_call(lua, |call| call.global(&global_name(&name)))?;
            from_text(lua, &value)
        })?,
    )?;
    ts.raw_set(
        "setGlobal",
        charged_function(lua, charge_bytes, |lua, (name, value): (String, Value)| {
            let value = to_text(&value)?;
            if !with_call(lua, |call| call.set_global(&global_name(&name), &value))? {
                return Err(Error::runtime(format!("`{name}` is not a global's name")));
            }
            Ok(())
        })?,
    )?;
    let package = package.to_string();
    ts.raw_set(
        "define",
        charged_function(
            lua,
            charge_bytes,
            move |lua, (name, function): (String, Function)| {
                let (namespace, short) = split_name(&name);
                // One id for each package's definition: two mods defining one name each keep their own.
                let id = realm::defined_id(lua, &format!("{package}/{name}"));
                realm::keep_defined(lua, id, function)?;
                let usage = format!("{name}(...) - defined by a Lua script");
                let definition = Definition {
                    namespace,
                    name: short,
                    package: &package,
                    usage: &usage,
                    id,
                };
                let defined = with_call(lua, |call| call.define_function(&definition))?;
                engine(lua, &format!("ts.define(\"{name}\")"), defined)
            },
        )?,
    )?;
    ts.raw_set(
        "warn",
        charged_function(lua, charge_bytes, |lua, values: MultiValue| {
            let line = values
                .iter()
                .map(Value::to_string)
                .collect::<mlua::Result<Vec<_>>>()?
                .join("\t");
            with_call(lua, |call| call.warn(&line))
        })?,
    )?;
    ts.set_readonly(true);
    Ok(ts)
}
