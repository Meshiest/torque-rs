//! The op and event format of client scripts: the bytes between a **realm** (the Luau state a server's client scripts run
//! in, inside a WebAssembly guest, one per server connection) and the engine that hosts it. The guest is
//! `torque_lua_client.wasm` (crates/torque-plugin-lua with its `client-script` feature, built by `just lua-client-web`); the
//! host runs it under Wasmtime natively and as a module in a browser. Both sides read and write the bytes with this crate.
//!
//! The format is frozen at [`FORMAT_VERSION`] 2 (2: a frame's callbacks get at most the steps per frame). A change of any layout, number or rule below is a new version, and a host
//! refuses a guest whose `torque_realm_format()` is not the version it was built for.
//!
//! # 1. The model
//!
//! Nothing the guest does calls into the engine. The guest keeps a mirror of what it made (controls, timers, pending calls)
//! and, while it runs, appends **ops** to one buffer; when the entry call returns, the host reads the whole buffer (one
//! **flush**), checks every op and applies it. The host hands the guest **events** (input on the realm's controls, messages
//! from its server, revocations, the screen size) as one batch per frame. Every op is checked twice: by the guest, so a
//! script gets a clear Lua error, and by the host, which is the enforcement point and never trusts the guest. An op a
//! checked guest would never emit (an ungranted tier, an unknown kind, a value out of range, a forged handle) is a
//! **protocol violation**: the host stops the realm.
//!
//! Time is the host's: every entry call carries `now_us`, the microseconds since the realm was made, from the host's
//! monotonic clock. `realm.time()`, timers and the guest's rate limits use it, so a recorded run replays exactly.
//!
//! # 2. The guest's exports
//!
//! All pointers and lengths are `i32` (wasm32), unsigned in meaning; `now_us` is `i64`, at least 0 and never smaller than
//! the previous call's. Every entry returns a status ([`abi`]): `OK` 0, `MALFORMED` 1 (the arguments or a batch do not
//! follow this format: a host bug or tampering), `REFUSED` 2 (a script file refused: caps, path, compiled code; or an entry
//! file that is not there), `STATE` 3 (no realm yet, or one already made).
//!
//! | Export | Does |
//! |---|---|
//! | `torque_realm_format() -> i32` | [`FORMAT_VERSION`] |
//! | `torque_realm_tiers() -> i32` | the [`Grants`] bits of the tiers this guest implements |
//! | `torque_realm_alloc(len) -> ptr` | a place of `len` bytes for the host to write into (0 when there is no room) |
//! | `torque_realm_free(ptr, len)` | frees a place `torque_realm_alloc(len)` returned |
//! | `torque_realm_new(config, config_len) -> status` | makes the instance's one realm from a [`abi::Config`] record |
//! | `torque_realm_add_script(mod, mod_len, path, path_len, src, src_len) -> status` | keeps one script file of a mod (8.) |
//! | `torque_realm_run(now_us, mod, mod_len, path, path_len) -> status` | runs a kept file in its mod's environment |
//! | `torque_realm_frame(now_us, events, events_len) -> status` | one frame: the events, timers, queued callbacks, `onFrame` |
//! | `torque_realm_ops() -> ptr`, `torque_realm_ops_len() -> i32` | the last entry's flush, valid until the next entry call |
//! | `torque_realm_pending() -> i32` | events still queued for a later frame (the frame budget ran out) |
//!
//! A `MALFORMED` batch is refused whole: none of its events is applied. Script errors and limit stops are not statuses;
//! they arrive as ops (`realm.log`, `realm.limit`) with status `OK`. The guest imports nothing outside WASI
//! (`wasi_snapshot_preview1`, the C and Rust runtimes' set), and the host answers those as for the Lua module: no files,
//! no environment, no arguments, `random_get`, standard error, and the clocks Luau's `os.clock` reads.
//!
//! **What the host keeps to** (the guest relies on it):
//! - Every clock the guest can read (WASI's monotonic and its process and thread CPU-time ids) is one per-instance
//!   clock that starts with the instance and steps by 1 µs: never the process's CPU time or the machine's wall clock.
//! - No export is called from inside an import (the guest is never re-entered), and calls come from one thread.
//! - After any trap (the deadline, the memory cap, a guest fault) the instance is thrown away and nothing of it is
//!   called again; a realm is made anew, if at all, in a fresh instance.
//! - A content path ([`ui::check_content_path`]) is looked up in the list of this server's verified files, never
//!   opened on disk by its text.
//! - The values of one batch decode within [`MAX_BATCH_NODES`] nodes in all (the host reads network data with the same
//!   decoder).
//!
//! # 3. Primitives
//!
//! Little-endian throughout. `u8`, `u16`, `u32`, `u64`, `f64` (IEEE 754 binary64 bits). `bool` is one byte, 0 or 1; any
//! other byte is malformed. `text` is a `u32` length and that many bytes of UTF-8 (invalid UTF-8 is malformed). `bytes` is
//! a `u32` length and that many bytes, anything. A **handle** is a `u32`: 0 is "none"; the guest numbers its controls (roots
//! included) 1, 2, 3, ... in the order it makes them and never reuses a number, so a create naming a number that is not the
//! next one is a violation, and a number that was destroyed never comes back.
//!
//! # 4. Records
//!
//! A flush and an event batch are each a sequence of records, back to back, nothing before or after:
//!
//! | Field | Type | |
//! |---|---|---|
//! | tier | `u8` | [`Tier`]: which tier the op or event belongs to |
//! | kind | `u8` | the op or event within the tier ([`op`], [`event`]) |
//! | len | `u32` | the body's length, at most [`MAX_RECORD`] |
//! | body | `len` bytes | the fields of that kind, in order, exactly filling it |
//!
//! A flush has at most [`MAX_OPS`] records and [`MAX_FLUSH_BYTES`] bytes; an event batch at most [`MAX_EVENTS`] records and
//! [`MAX_BATCH_BYTES`] bytes. A body with bytes left over, or too few, is malformed.
//!
//! # 5. Tiers and grants
//!
//! A tier is a named capability. The realm's environment holds the Lua table of each **granted** tier the guest implements
//! and not one other: an ungranted tier's table is absent (`nil`), so using it is a plain Lua error. Grants are a `u32` of
//! bits, bit `n` for tier `n` ([`Grants`]); bit 0, `realm`, is the base every realm has. Bits for tiers above
//! [`Tier::Predict`] are malformed; a granted tier the guest does not implement is left out (its table absent).
//!
//! | Id | Tier | Lua table | |
//! |---|---|---|---|
//! | 0 | `realm` | `realm` | base: time, timers, `onFrame`, log, `has`, `on("revoked")` |
//! | 1 | `ui` | `ui` | menus and HUD controls |
//! | 2 | `net` | `net` | messages and calls to its own server |
//! | 3 | `state` | `state` | read-only views of replicated state (not in format 1's guest) |
//! | 4 | `draw` | `draw` | HUD drawing and world markers (not yet) |
//! | 5 | `effects` | `effects` | local sounds and effects (not yet) |
//! | 6 | `input` | `input` | the server's own game actions (not yet) |
//! | 7 | `camera` | `camera` | bounded camera control (not yet) |
//! | 8 | `local` | `world` | local-only objects (`local` is a Lua keyword, so the table is `world`) (not yet) |
//! | 9 | `predict` | `predict` | prediction hooks (not yet) |
//!
//! Format 1 defines the ops and events of `realm`, `ui` and `net`. Any record of another tier is a violation in format 1.
//! **Revocation**: the host sends `realm.revoked(tier)` and removes that tier's effects in the same frame; from then on the
//! guest's functions of that tier return `false` and emit nothing, and it drops that tier's queued events. A tier is never
//! granted again within a realm.
//!
//! # 6. Values
//!
//! A **value** (messages, call arguments and results, `change` events) is a tag byte and its payload ([`Value`]):
//!
//! | Tag | Value | Payload |
//! |---|---|---|
//! | 0 | nil | none |
//! | 1 | false | none |
//! | 2 | true | none |
//! | 3 | number | `f64`; every NaN is written as `0x7FF8_0000_0000_0000`, and `-0` as `0` |
//! | 4 | string | `bytes` (Lua strings are bytes) |
//! | 5 | table | `u16` count, then count pairs of a key value and a value |
//!
//! All the values of one batch together decode to at most [`MAX_BATCH_NODES`] values and keys ([`NodeBudget`]); past
//! it the batch is malformed. Keys are strings or numbers (not NaN), each at most once in a table, in any order. A table has at
//! most [`MAX_TABLE_ENTRIES`] pairs and nests at most [`MAX_DEPTH`] tables deep (a table in a table is depth 2). Functions,
//! userdata (handles included), threads, buffers and cycles have no encoding: the guest refuses them with a Lua error.
//!
//! # 7. Ops (guest to host)
//!
//! | Tier | Kind | Op | Body |
//! |---|---|---|---|
//! | realm | 0 | `log` | `u8` level (0 info, 1 warning, 2 error), `text` line |
//! | realm | 1 | `limit` | `u8` which ([`op::realm::LIMIT_STEPS`], [`op::realm::LIMIT_MEMORY`]), `u8` entry ([`op::realm::ENTRY_RUN`] ...) |
//! | ui | 0 | `root` | `u32` handle, `text` mod id |
//! | ui | 1 | `create` | `u32` handle, `u32` parent, `u8` kind ([`ui::Kind`]), props |
//! | ui | 2 | `set` | `u32` handle, props |
//! | ui | 3 | `destroy` | `u32` handle: the control and everything under it |
//! | ui | 4 | `parent` | `u32` handle, `u32` new parent |
//! | ui | 5 | `order` | `u32` handle, `bool` front (true: drawn last among its siblings; false: first) |
//! | ui | 6 | `interactive` | `bool` on: the realm asks for (or gives back) the cursor |
//! | ui | 7 | `focus` | `u32` handle of a textfield |
//! | net | 0 | `send` | value: a free-form message to its server |
//! | net | 1 | `call` | `u32` call id, `text` RPC name, value arguments |
//!
//! - `log`: the guest has already made the line plain: every character [`is_hidden`] names replaced by a space,
//!   at most [`MAX_LOG_LINE`] bytes, at most [`LOG_LINES_PER_SECOND`] lines a second (then one warning, then nothing until
//!   the next second). The host tags it with the server before it shows it; it is never parsed.
//! - `limit`: a callback ran out of steps or memory and was stopped (its log line follows). The host's strike rule counts
//!   these.
//! - `root`: the mod's sub-root, a panel covering the server area, made by a mod's first `ui.root()` (one per mod).
//! - `create`: `handle` is the next number; `parent` is a live handle of this realm (a root or a control) and the new
//!   control is at most [`ui::MAX_DEPTH`] below its root.
//! - `parent`: never a root, never under itself, within the depth cap. Roots are never destroyed or moved.
//! - `focus`: only within [`ui::ACTIVATION_US`] of an activating event (`click`, `press`, `change`, `submit`, `select`) of
//!   this realm's own controls; the host keeps its own clock of that and refuses (does not stop for) a late one.
//! - `call`: ids count up from 1 and are never reused; the host answers each with one `net.reply`. The name follows
//!   [`net::check_rpc_name`]; the host refuses (with a `refused` reply) an RPC its server did not mark for the realm.
//! - `send` and `call`: at most [`net::MAX_SEND_BYTES`] / [`net::MAX_CALL_BYTES`] of value; the guest also keeps
//!   [`net::SENDS_PER_SECOND`] (burst [`net::SEND_BURST`]) and [`net::SEND_BYTES_PER_SECOND`] together, past which
//!   `net.send` and `net.call` return `false` and emit nothing, and at most [`net::MAX_OUTSTANDING`] calls of one name.
//!
//! **Props** (in `create` and `set`): a `u8` count, then count pairs of a `u8` prop id ([`ui::Prop`]) and its payload, ids
//! strictly ascending. A prop the kind does not have, a payload of the wrong type and a value out of its range are
//! violations. Payloads by type ([`ui::PropType`]):
//!
//! | Type | Payload | Lua side |
//! |---|---|---|
//! | bool | `bool` | `true` / `false` |
//! | number | `f64`, finite, in the prop's range | a number |
//! | int | `u32`, in the prop's range | a whole number |
//! | vec2 | `f64` x, `f64` y, finite, each in the prop's range | `{x, y}` or `{x = .., y = ..}` |
//! | color | `u8` r, g, b, a | `{r, g, b, a?}` of 0-1, or `"#rrggbb"` / `"#rrggbbaa"` |
//! | text | `text`, at most the prop's bytes, no control characters but `\n` and `\t` | a string |
//! | rich | as text, and well-formed markup ([`ui::check_rich`]) | a string |
//! | enum | `u8` index into the prop's names | one of the names |
//! | image | `text`: empty (none), or a content path ([`ui::check_content_path`]) with an image extension | a string |
//! | rows | `u16` count, then count `text`s | an array of strings |
//! | sides | four `f64` (top, right, bottom, left), finite, each in the prop's range | a number (all four), or an array of 2 (top and bottom, left and right) or 4 |
//! | tracks | `u8` count (at most [`ui::MAX_TRACKS`]), then count tracks: a `u8` tag (0 auto; 1 length, then `f64`; 2 share, then `f64`), finite, in range | an array of numbers (lengths) and the strings `"auto"` and `"1fr"` (shares) |
//!
//! The kinds, their props, ranges and defaults are [`ui::KINDS`] and [`ui::PROPS`]. The text props of one control together
//! hold at most [`ui::MAX_CONTROL_TEXT`] bytes, not counting a richtext's `text` ([`ui::MAX_RICH_TEXT`]) and a list's
//! `rows` ([`ui::MAX_LIST_TEXT`] in all, [`ui::MAX_ROW_TEXT`] a row), which have their own caps; all controls together
//! [`ui::MAX_TEXT`]; the distinct images of live controls are at most [`ui::MAX_IMAGES`]; live controls (roots not counted)
//! at most [`ui::MAX_CONTROLS`]. Controls have no names, no profiles, no commands, no variables, no accelerators and no
//! classes: those fields do not exist in this format.
//!
//! **Layout containers** are the kinds `flex` and `grid` (kinds 11 and 12, props 53 to 63): the host lays their children out
//! by the rules of CSS's flexbox and grid, using the item props every kind has (`grow`, `shrink`, `basis`, `alignSelf`,
//! `justifySelf`, `margin`, `row`, `column`, `rowSpan`, `columnSpan`, `minSize`, `maxSize`: props 41 to 52). A container has
//! no look props (no `enabled`, `tooltip` or `opacity`) and no events; its looks are its children's. At most
//! [`ui::MAX_CONTAINER_DEPTH`] containers nest, a container has at most [`ui::MAX_CONTAINER_CHILDREN`] children, a grid at
//! most [`ui::MAX_TRACKS`] tracks on an axis, and every number is finite and within its prop's range (gaps, padding and
//! margins at most [`ui::MAX_SPACING`], shares at most [`ui::MAX_SHARE`], lengths at most [`ui::MAX_LENGTH`]); a `create` or
//! `parent` past a cap is a violation. The layout is the engine's to give: a host without a layout class keeps a container as
//! a plain group that places nothing, and every other op works as before. This is an addition to format 2's tables, not a new
//! version: a host that has it applies every flush of a guest that does not.
//!
//! # 8. Script files
//!
//! `add_script` keeps one file: `mod` is a mod id ([`abi::check_mod_id`]); `path` a plain relative path within the mod
//! ([`abi::check_script_path`]) ending in `.lua` or `.luau`; `src` the file's text (compiled Luau, which starts with a byte
//! below 0x20 other than a tab, newline or form feed, is refused). At most [`abi::MAX_SCRIPTS`] files of at most
//! [`abi::MAX_SCRIPT_BYTES`] each and [`abi::MAX_SCRIPTS_BYTES`] in all; a path kept twice is refused. `require(name)`
//! loads `name.lua`, `name.luau` or `name/init.lua` from the requiring mod's own kept files (`.` and `/` separate parts of
//! letters, digits, `_` and `-`), nothing else.
//!
//! # 9. Events (host to guest)
//!
//! | Tier | Kind | Event | Body | Reaches |
//! |---|---|---|---|---|
//! | realm | 0 | `revoked` | `u8` tier | `realm.on("revoked", fn(name))` |
//! | ui | 0 | `click` | `u32` handle | `h:on("click", fn())` |
//! | ui | 1 | `hover` | `u32` handle, `bool` on | `h:on("hover", fn(on))` |
//! | ui | 2 | `press` | `u32` handle | `h:on("press", fn())` |
//! | ui | 3 | `change` | `u32` handle, value (toggle: boolean; textfield: string; slider: number) | `h:on("change", fn(v))` |
//! | ui | 4 | `submit` | `u32` handle, `text` | `h:on("submit", fn(text))` |
//! | ui | 5 | `select` | `u32` handle, `u32` row (1-based; 0 none) | `h:on("select", fn(row or nil))` |
//! | ui | 6 | `scroll` | `u32` handle, `f64` x, `f64` y | `h:on("scroll", fn(x, y))` |
//! | ui | 7 | `screen` | `f64` width, `f64` height, `f64` UI scale | `ui.on("screen", fn(w, h, scale))` |
//! | ui | 8 | `closed` | none | `ui.on("closed", fn())`: Escape dropped the realm's cursor and focus |
//! | net | 0 | `message` | value | `net.on(fn(value))` |
//! | net | 1 | `reply` | `u32` call id, `bool` ok, value (the result, or the error as a string) | the call's `fn(ok, v)` |
//! | net | 2 | `rpc` | `text` RPC name, value | `net.handle(name, fn(value))` |
//! | net | 3 | `overflow` | none | `net.on("overflow", fn())`: messages were dropped |
//!
//! `change`, `select` and `scroll` update the guest's mirror when they arrive (so `h:get` agrees with what the player
//! sees) even when their callbacks wait. An event naming a handle the realm does not have (never made, destroyed, a root
//! for a control event) or carrying the wrong value type for its control is dropped without a callback. Events of an
//! ungranted or revoked tier are dropped.
//!
//! **One frame** (`torque_realm_frame`): the batch is decoded whole; revocations apply at once; the other events join the
//! queue (at most [`MAX_QUEUED`]: past it the oldest `ui` input event goes, else the oldest event, and a dropped net
//! message makes one `overflow` callback); then the queue runs in order, then due timers (by due time, then age), then the
//! `onFrame` callbacks with the frame's seconds since the previous frame. Each callback gets the config's steps per
//! callback, but no more than the config's steps per frame; once the frame's steps are spent no further callback
//! starts, and the rest waits for the next frame (`torque_realm_pending`). A file run (`torque_realm_run`, and its
//! `require`s) is not a frame's: it gets the full steps per callback.
//!
//! # 10. Limits the guest keeps
//!
//! Steps: one per Luau interrupt (every call and loop edge, in every coroutine) and one per [`abi::BYTES_PER_STEP`] bytes a
//! bulk builtin (`string.rep`, `table.concat`, `buffer.fill`, ...) handles. A callback past its steps is stopped: every
//! interrupt errors until it returns, so neither `pcall` nor a coroutine goes on. Memory: Luau's allocator is capped at the
//! config's bytes; an allocation past it is a Lua error, and the realm collects its garbage at once. Timers: at most
//! [`MAX_TIMERS`] live (`after`, `every` and `onFrame` together); a period shorter than a frame fires once a frame. An
//! `on` list (`realm.on`, `ui.on`, `net.on`) holds at most [`MAX_HANDLERS`] functions. Strings longer than a field can hold are refused
//! before they are read. The wall-clock deadline and the linear-memory cap are the host's.
//!
//! # Maintaining
//!
//! **Map.** `abi` the guest's config and the caps of script files; `codec` the records, the reader and the writer; `value` the
//! free-form values; `ui` the control kinds, props (ids, types, ranges, defaults), the text, markup and path rules and the caps;
//! `net` the RPC names. Three places read these bytes: the guest (`torque-plugin-lua`, `client_script`), the host's flush check
//! (`torque-client-script-host`) and the GUI's applier (`torque-gui`, `client_script::apply`); the first and the last each check
//! everything again, because neither trusts the other.
//!
//! **Extending.** A kind or a prop is a row of `ui::KINDS` or `ui::PROPS` (ids only ever appended); a new prop type is an arm in
//! `check_value`, `write_value`, `read_value`, `field_text` and, in the guest, `to_prop`, `prop_to_lua`. Both appliers then need
//! the rules that tie props together (a container's nesting and children, a slider's range). Such an addition keeps
//! [`FORMAT_VERSION`]: a host that has it reads every flush of a guest that does not. Anything that changes a byte or a rule that
//! exists is a new version. The guest's sources are part of its hash, so every change here is followed by the justfile's `lua-client-web`
//! and the new hash in `ACCEPTED_CLIENT_GUESTS`.
//!
//! **Checking.** `cargo test -p torque-client-script-format` (the round trips, the ranges, every refusal and a seeded fuzz of the
//! props reader, 300,000 cases with `TORQUE_CLIENT_SCRIPT_FUZZ` set), then the appliers' and the guest's suites
//! (the justfile's `client-script-check`).

#![warn(missing_docs)]

pub mod abi;
mod codec;
pub mod net;
pub mod ui;
mod value;

pub use codec::{Malformed, Reader, Record, Records, Writer};
pub use value::{NodeBudget, Value, ValueError};

/// The version of this format; a host refuses a guest whose `torque_realm_format()` differs.
pub const FORMAT_VERSION: u32 = 2;

/// The most bytes a record's body has.
pub const MAX_RECORD: usize = 64 << 10;
/// The most ops in one flush.
pub const MAX_OPS: usize = 4096;
/// The most bytes in one flush.
pub const MAX_FLUSH_BYTES: usize = 4 << 20;
/// The most events in one batch the host hands over.
pub const MAX_EVENTS: usize = 1024;
/// The most bytes in one event batch.
pub const MAX_BATCH_BYTES: usize = 4 << 20;
/// The most events the guest keeps queued between frames.
pub const MAX_QUEUED: usize = 256;
/// The most live timers (`realm.after`, `realm.every` and `realm.onFrame` together).
pub const MAX_TIMERS: usize = 64;
/// The most functions one `on` list (`realm.on`, `ui.on`, `net.on`) holds.
pub const MAX_HANDLERS: usize = 16;
/// The most values and keys all the values of one batch (or one value read alone) decode to.
pub const MAX_BATCH_NODES: usize = 1 << 16;
/// The most pairs in one table of a value.
pub const MAX_TABLE_ENTRIES: usize = 256;
/// How deep tables nest in a value.
pub const MAX_DEPTH: usize = 8;
/// The longest log line, in bytes.
pub const MAX_LOG_LINE: usize = 512;
/// Log lines a second, past which they are dropped (with one warning).
pub const LOG_LINES_PER_SECOND: u32 = 20;

/// Whether `c` hides, breaks or reorders the text it is in: control characters (C0, DEL, C1), the line and paragraph
/// separators (U+2028, U+2029), the bidirectional controls (U+061C, U+200E, U+200F, U+202A to U+202E, U+2066 to
/// U+2069), the zero-width characters (U+200B to U+200D, U+2060 to U+2064, U+FEFF) and the tag characters (U+E0000 to
/// U+E007F). Log lines get spaces for them; text props refuse them (but `\n` and `\t`).
pub fn is_hidden(c: char) -> bool {
    matches!(
        c,
        '\u{0}'..='\u{1f}'
            | '\u{7f}'..='\u{9f}'
            | '\u{61c}'
            | '\u{200b}'..='\u{200f}'
            | '\u{2028}'..='\u{202e}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{2069}'
            | '\u{feff}'
            | '\u{e0000}'..='\u{e007f}'
    )
}

/// The names Windows gives devices, which a file name may not have under any extension.
const DEVICE_NAMES: &[&str] = &["con", "prn", "aux", "nul"];

/// Whether `part` is a plain file or folder name: ASCII letters, digits, `_`, `-` and `.`, at least one byte, not only
/// dots, not ending in a dot, and not a Windows device name (`CON`, `PRN`, `AUX`, `NUL`, `COM1` to `COM9`, `LPT1` to
/// `LPT9`, in any case, with any extension).
pub fn is_plain_part(part: &str) -> bool {
    let plain = !part.is_empty()
        && part
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
        && !part.ends_with('.');
    if !plain {
        return false;
    }
    let stem = part
        .split_once('.')
        .map_or(part, |(stem, _)| stem)
        .to_ascii_lowercase();
    let numbered = |prefix: &str| {
        stem.strip_prefix(prefix)
            .is_some_and(|n| matches!(n.as_bytes(), [b'1'..=b'9']))
    };
    !(DEVICE_NAMES.contains(&stem.as_str()) || numbered("com") || numbered("lpt"))
}

/// A tier: a named capability with its own Lua table, ops and events.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u8)]
pub enum Tier {
    /// The base every realm has: time, timers, `onFrame`, log, `has`, `on("revoked")`.
    Realm = 0,
    /// Menus and HUD controls.
    Ui = 1,
    /// Messages and calls to its own server.
    Net = 2,
    /// Read-only views of replicated state.
    State = 3,
    /// HUD drawing and world markers.
    Draw = 4,
    /// Local sounds and effects.
    Effects = 5,
    /// The server's own game actions.
    Input = 6,
    /// Bounded camera control.
    Camera = 7,
    /// Local-only objects (the Lua table `world`).
    Local = 8,
    /// Prediction hooks.
    Predict = 9,
}

impl Tier {
    /// Every tier, by id.
    pub const ALL: [Tier; 10] = [
        Tier::Realm,
        Tier::Ui,
        Tier::Net,
        Tier::State,
        Tier::Draw,
        Tier::Effects,
        Tier::Input,
        Tier::Camera,
        Tier::Local,
        Tier::Predict,
    ];

    /// The tier with id `id`.
    pub fn from_id(id: u8) -> Option<Tier> {
        Tier::ALL.get(usize::from(id)).copied()
    }

    /// The tier named `name` (its grant name, as a server's manifest lists it).
    pub fn from_name(name: &str) -> Option<Tier> {
        Tier::ALL.into_iter().find(|t| t.name() == name)
    }

    /// The tier's id.
    pub fn id(self) -> u8 {
        self as u8
    }

    /// The tier's name.
    pub fn name(self) -> &'static str {
        match self {
            Tier::Realm => "realm",
            Tier::Ui => "ui",
            Tier::Net => "net",
            Tier::State => "state",
            Tier::Draw => "draw",
            Tier::Effects => "effects",
            Tier::Input => "input",
            Tier::Camera => "camera",
            Tier::Local => "local",
            Tier::Predict => "predict",
        }
    }

    /// The name of the tier's Lua table (`world` for `local`, a Lua keyword).
    pub fn table(self) -> &'static str {
        match self {
            Tier::Local => "world",
            other => other.name(),
        }
    }
}

/// A set of tiers: bit `n` for the tier with id `n`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Grants(pub u32);

impl Grants {
    /// The bits a tier can have.
    pub const VALID: u32 = (1 << Tier::ALL.len()) - 1;

    /// The tiers in `tiers` (and the base).
    #[must_use]
    pub fn of(tiers: &[Tier]) -> Grants {
        Grants(tiers.iter().fold(1, |bits, t| bits | 1 << t.id()))
    }

    /// Whether `tier` is in the set.
    pub fn has(self, tier: Tier) -> bool {
        self.0 & (1 << tier.id()) != 0
    }

    /// The set with `tier` added.
    #[must_use]
    pub fn with(self, tier: Tier) -> Grants {
        Grants(self.0 | 1 << tier.id())
    }

    /// The set without `tier`.
    #[must_use]
    pub fn without(self, tier: Tier) -> Grants {
        Grants(self.0 & !(1 << tier.id()))
    }

    /// The tiers in both sets.
    #[must_use]
    pub fn and(self, other: Grants) -> Grants {
        Grants(self.0 & other.0)
    }

    /// Whether every bit names a tier.
    pub fn is_valid(self) -> bool {
        self.0 & !Grants::VALID == 0
    }
}

/// The ops' kinds, by tier.
pub mod op {
    /// The base's ops.
    pub mod realm {
        /// `log`: `u8` level, `text` line.
        pub const LOG: u8 = 0;
        /// `limit`: `u8` which, `u8` entry.
        pub const LIMIT: u8 = 1;

        /// A log line's level: information.
        pub const LEVEL_INFO: u8 = 0;
        /// A log line's level: a warning (dropped lines, a refused request).
        pub const LEVEL_WARNING: u8 = 1;
        /// A log line's level: a script error.
        pub const LEVEL_ERROR: u8 = 2;

        /// `limit`'s which: the callback ran out of steps.
        pub const LIMIT_STEPS: u8 = 0;
        /// `limit`'s which: the callback hit the memory cap.
        pub const LIMIT_MEMORY: u8 = 1;

        /// `limit`'s entry: a file run (`torque_realm_run`, or a `require` within one).
        pub const ENTRY_RUN: u8 = 0;
        /// `limit`'s entry: an event's callback.
        pub const ENTRY_EVENT: u8 = 1;
        /// `limit`'s entry: a timer's callback.
        pub const ENTRY_TIMER: u8 = 2;
        /// `limit`'s entry: an `onFrame` callback.
        pub const ENTRY_FRAME: u8 = 3;
    }

    /// The `ui` tier's ops.
    pub mod ui {
        /// `root`: `u32` handle, `text` mod id.
        pub const ROOT: u8 = 0;
        /// `create`: `u32` handle, `u32` parent, `u8` kind, props.
        pub const CREATE: u8 = 1;
        /// `set`: `u32` handle, props.
        pub const SET: u8 = 2;
        /// `destroy`: `u32` handle.
        pub const DESTROY: u8 = 3;
        /// `parent`: `u32` handle, `u32` parent.
        pub const PARENT: u8 = 4;
        /// `order`: `u32` handle, `bool` front.
        pub const ORDER: u8 = 5;
        /// `interactive`: `bool` on.
        pub const INTERACTIVE: u8 = 6;
        /// `focus`: `u32` handle.
        pub const FOCUS: u8 = 7;
    }

    /// The `net` tier's ops.
    pub mod net {
        /// `send`: value.
        pub const SEND: u8 = 0;
        /// `call`: `u32` call id, `text` name, value.
        pub const CALL: u8 = 1;
    }
}

/// The events' kinds, by tier.
pub mod event {
    /// The base's events.
    pub mod realm {
        /// `revoked`: `u8` tier.
        pub const REVOKED: u8 = 0;
    }

    /// The `ui` tier's events.
    pub mod ui {
        /// `click`: `u32` handle.
        pub const CLICK: u8 = 0;
        /// `hover`: `u32` handle, `bool` on.
        pub const HOVER: u8 = 1;
        /// `press`: `u32` handle.
        pub const PRESS: u8 = 2;
        /// `change`: `u32` handle, value.
        pub const CHANGE: u8 = 3;
        /// `submit`: `u32` handle, `text`.
        pub const SUBMIT: u8 = 4;
        /// `select`: `u32` handle, `u32` row (1-based; 0 none).
        pub const SELECT: u8 = 5;
        /// `scroll`: `u32` handle, `f64` x, `f64` y.
        pub const SCROLL: u8 = 6;
        /// `screen`: `f64` width, `f64` height, `f64` UI scale.
        pub const SCREEN: u8 = 7;
        /// `closed`: no body.
        pub const CLOSED: u8 = 8;
    }

    /// The `net` tier's events.
    pub mod net {
        /// `message`: value.
        pub const MESSAGE: u8 = 0;
        /// `reply`: `u32` call id, `bool` ok, value.
        pub const REPLY: u8 = 1;
        /// `rpc`: `text` name, value.
        pub const RPC: u8 = 2;
        /// `overflow`: no body.
        pub const OVERFLOW: u8 = 3;
    }
}

#[cfg(test)]
mod tests;
