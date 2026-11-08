//! `require`: a module of the requiring script's own folder (a mod's), read through the engine's checks, run once in
//! that folder's global table and kept.

use mlua::chunk::ChunkMode;
use mlua::{Error, Function, Lua, Value};
use torque_plugin_sdk::CallError;

use crate::realm::{self, engine, with_call};

/// The longest module name `require` takes.
const MAX_NAME: usize = 200;

/// Whether `name` is a module name: parts of ASCII letters, digits, `_` and `-`, separated by `.` or `/`.
fn is_module_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_NAME
        && name.split(['.', '/']).all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        })
}

/// The `require` of the scripts whose folder is `root`.
pub(crate) fn require_function(lua: &Lua, root: &str) -> mlua::Result<Function> {
    let root = root.to_string();
    lua.create_function(move |lua, name: String| {
        if !is_module_name(&name) {
            return Err(Error::runtime(format!(
                "require: `{name}` is not a module name (parts of letters, digits, `_` and `-`, separated by `.`): \
                 a module is a file of this mod's own folder"
            )));
        }
        let relative = name.replace('.', "/");
        let key = relative.to_ascii_lowercase();
        let modules = realm::modules_of(lua, &root)?;
        match modules.raw_get::<Value>(key.as_str())? {
            Value::Nil => {}
            Value::LightUserData(_) => {
                return Err(Error::runtime(format!(
                    "require: `{name}` requires itself (through the modules it requires)"
                )));
            }
            loaded => return Ok(loaded),
        }
        let prefix = if root.is_empty() {
            String::new()
        } else {
            format!("{root}/")
        };
        let mut found = None;
        for candidate in [
            format!("{prefix}{relative}.lua"),
            format!("{prefix}{relative}.luau"),
            format!("{prefix}{relative}/init.lua"),
        ] {
            match with_call(lua, |call| call.read_script(&candidate))? {
                Ok(source) => {
                    found = Some((candidate, source));
                    break;
                }
                Err(CallError::Limit) => return engine(lua, "require", Err(CallError::Limit)),
                Err(CallError::Refused(_)) => {}
            }
        }
        let Some((path, source)) = found else {
            let place = if root.is_empty() { "the game's top folder" } else { &root };
            return Err(Error::runtime(format!(
                "require: there is no module `{name}` in {place}"
            )));
        };
        if source.first().is_some_and(|&b| b < 0x20 && !b.is_ascii_whitespace()) {
            return Err(Error::runtime(format!(
                "require: {path} is compiled Lua, which is never loaded"
            )));
        }
        let env = realm::env_for(lua, &root, "")?;
        // Marked while it runs, so a cycle is an error rather than a loop.
        modules.raw_set(key.as_str(), Value::LightUserData(mlua::LightUserData(std::ptr::null_mut())))?;
        let ran = lua
            .load(&source)
            .set_name(format!("@{path}"))
            .set_mode(ChunkMode::Text)
            .set_environment(env)
            .call::<Value>(());
        let value = match ran {
            Ok(Value::Nil) => Value::Boolean(true),
            Ok(value) => value,
            Err(error) => {
                modules.raw_set(key.as_str(), Value::Nil)?;
                return Err(error);
            }
        };
        modules.raw_set(key.as_str(), value.clone())?;
        Ok(value)
    })
}
