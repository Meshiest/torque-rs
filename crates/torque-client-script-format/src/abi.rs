//! The guest's entry points: statuses, the config record, the script files' rules and the guest's own limits.

use crate::codec::{Malformed, Reader, Writer};
use crate::{FORMAT_VERSION, Grants};

/// The entry succeeded (script errors and limit stops are ops, not statuses).
pub const OK: i32 = 0;
/// The arguments or a batch do not follow the format: a host bug or tampering. Nothing of the call was applied.
pub const MALFORMED: i32 = 1;
/// A script file was refused (caps, path, compiled code), or an entry file is not there.
pub const REFUSED: i32 = 2;
/// There is no realm yet, or one was made already.
pub const STATE: i32 = 3;

/// The bytes of a [`Config`] record.
pub const CONFIG_LEN: usize = 24;

/// The most script files a realm keeps.
pub const MAX_SCRIPTS: usize = 64;
/// The most bytes of one script file.
pub const MAX_SCRIPT_BYTES: usize = 256 << 10;
/// The most bytes of all of a realm's script files.
pub const MAX_SCRIPTS_BYTES: usize = 1 << 20;
/// The longest mod id.
pub const MAX_MOD_ID: usize = 64;
/// The longest script path.
pub const MAX_SCRIPT_PATH: usize = 200;
/// The bytes of work a bulk builtin is charged one step for.
pub const BYTES_PER_STEP: u64 = 64;

/// What `torque_realm_new` makes a realm with. Its record: `u32` format version, `u32` grants, `u32` Lua memory cap in
/// bytes, `u32` steps per callback, `u32` steps per frame, `u32` random seed (Luau's `math.random` starts from it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Config {
    /// The tiers granted (the guest leaves out those it does not implement).
    pub grants: Grants,
    /// Luau's allocator cap, in bytes.
    pub memory: u32,
    /// The steps a file run may take; an event's, a timer's or an `onFrame` callback may take as many, but no more than
    /// [`Config::frame_steps`].
    pub callback_steps: u32,
    /// The steps all callbacks of one frame may take before the rest waits.
    pub frame_steps: u32,
    /// `math.random`'s seed.
    pub seed: u32,
}

impl Default for Config {
    /// The defaults: the base only, 32 MiB, 200,000 steps a callback, 2,000,000 a frame.
    fn default() -> Config {
        Config {
            grants: Grants(1),
            memory: 32 << 20,
            callback_steps: 200_000,
            frame_steps: 2_000_000,
            seed: 0,
        }
    }
}

impl Config {
    /// The smallest memory cap a config may give.
    pub const MIN_MEMORY: u32 = 1 << 20;

    /// The record.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Writer::new();
        out.u32(FORMAT_VERSION);
        out.u32(self.grants.0);
        out.u32(self.memory);
        out.u32(self.callback_steps);
        out.u32(self.frame_steps);
        out.u32(self.seed);
        out.into_bytes()
    }

    /// The config a record holds: its version this format's, its grants valid, its memory at least
    /// [`Config::MIN_MEMORY`], its step counts above 0.
    pub fn from_bytes(bytes: &[u8]) -> Result<Config, Malformed> {
        if bytes.len() != CONFIG_LEN {
            return Err(Malformed("a config record of the wrong length"));
        }
        let mut r = Reader::new(bytes);
        if r.u32()? != FORMAT_VERSION {
            return Err(Malformed("a config of another format version"));
        }
        let config = Config {
            grants: Grants(r.u32()?),
            memory: r.u32()?,
            callback_steps: r.u32()?,
            frame_steps: r.u32()?,
            seed: r.u32()?,
        };
        if !config.grants.is_valid() {
            return Err(Malformed("grant bits that name no tier"));
        }
        if config.memory < Config::MIN_MEMORY
            || config.callback_steps == 0
            || config.frame_steps == 0
        {
            return Err(Malformed("a memory cap or a step count too small"));
        }
        Ok(config)
    }
}

/// A mod id: at most [`MAX_MOD_ID`] bytes, [`crate::is_plain_part`].
pub fn check_mod_id(id: &str) -> Result<(), &'static str> {
    if id.len() > MAX_MOD_ID || !crate::is_plain_part(id) {
        return Err("a mod id is letters, digits, `_`, `-` and `.`");
    }
    Ok(())
}

/// A script path within its mod: at most [`MAX_SCRIPT_PATH`] bytes, [`crate::is_plain_part`] parts separated by `/`,
/// ending in `.lua` or `.luau`.
pub fn check_script_path(path: &str) -> Result<(), &'static str> {
    if path.is_empty() || path.len() > MAX_SCRIPT_PATH {
        return Err("a script path is 1 to 200 bytes");
    }
    for part in path.split('/') {
        if !crate::is_plain_part(part) {
            return Err("a script path's parts are letters, digits, `_`, `-` and `.`");
        }
    }
    if !(path.ends_with(".lua") || path.ends_with(".luau")) {
        return Err("a script file ends in .lua or .luau");
    }
    Ok(())
}

/// Whether `source` is compiled Luau, not text: it starts with a byte below 0x20 other than a tab, newline, carriage
/// return or form feed.
pub fn is_compiled(source: &[u8]) -> bool {
    source
        .first()
        .is_some_and(|&b| b < 0x20 && !matches!(b, b'\t' | b'\n' | b'\r' | 0x0c))
}
