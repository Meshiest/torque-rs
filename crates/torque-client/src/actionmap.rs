//! `ActionMap`: TGE 1.4's `actionMap.cc` (`processBind`, `processBindCmd`, `processUnbind`, `findNode`, `getNode`,
//! `processAction`, the break table, `dumpActionMap` and the console methods), with the engine's own rules below.
//!
//! **The model.** A map holds device maps in the order their first binding made them (`keyboard`, `mouse0`, ...), each its nodes in
//! the order they were made; a node is one (modifiers, key code) pair with its flags (`S` scale 2, `R` read as `S`, `D` dead zone 4,
//! `I` inverted 8, a `bindCmd` 0x10), dead zone, scale, function and make/break commands. Key texts are read
//! by [`gui::keys::parse_descriptor_with`] and written by [`gui::keys::key_string_in`] and [`gui::keys::modifier_string`], in the
//! game's key tables ([`ClientGame::key_tables`](crate::game::ClientGame::key_tables)).
//!
//! **The engine's own rules** (beyond TGE 1.4):
//! - `bind` first REMOVES every other plain node of the map, in any device, whose function is the command (any case): a command has
//!   one key, and binding the same key again moves its node to the end. `bindCmd` removes nothing and keeps the node's function.
//! - `getBinding(command)` answers the first such node only: `device TAB modifiers key`.
//! - `getCommand` of a `bindCmd` node is `make TAB break`; `getDeadZone` writes `%f %f` (none: `0 0`); `getScale` 1 without `S`;
//!   `isInverted`, `getScale` and `getDeadZone` of a key the map does not have print `The input event specified by %s %s is not in
//!   this action map!`. `getNumBinds` counts every node.
//! - `save(file, append)`: `// Torque Input Map File` (not when appending), `NAME.delete();`, `new ActionMap(NAME);`, then a line per
//!   node `NAME.bind(device, "modskey"[, FLAGS][, "%f %f"][, %f], function);` or `NAME.bindCmd(device, "modskey", "make",
//!   "break");` (the commands escaped), LF line ends; with no file, the same lines on the console (no header).
//!   **The engine writes no script of its own into the file** (the function and map names are written bare, and a saved
//!   `config.cs` runs at every start-up): a binding whose function is not one identifier ([`is_script_name`]) or whose line would
//!   pass the 1023-byte buffer is left out with a console warning, and a map whose name is not one writes nothing. Every other
//!   binding is written byte for byte as TGE writes it.
//!
//! **Events** ([`process_action`]): a make finds the node with the event's modifiers (each side of a pair as the pair),
//! else the unmodified one; a plain node calls its function with the value (`%g`: a key 1, inverted 0, times the scale), a `bindCmd`
//! evaluates its make command; either way the key enters the break table (device, instance, key), and a break later calls the same
//! function with its value (0) or evaluates the break command, even after the map was popped. A mouse axis calls its node's function
//! with the axis value (inverted, scaled). The canvas's routing (GlobalActionMap first, the canvas, then the other maps from the top) is
//! [`crate::input`].
//!
//! Credit: follows TGE's action maps as Torque3D carries them (MIT: `sim/actionMap.cpp`, Copyright (c) 2012 GarageGames, LLC).

use std::cell::RefCell;
use std::rc::Rc;

use gui::input::InputEvent;
use gui::keys::{self, Descriptor, SI_BREAK, SI_MAKE, SI_MOVE};
use ts_vm::sim::ObjId;
use ts_vm::vm::{Native, NativeCall, Vm, VmError};

use crate::conn::ConnGame;
use crate::game::client_game;
use crate::state::ClientState;

/// The game's key tables.
fn key_tables(vm: &Vm) -> keys::KeyTables {
    client_game(vm).key_tables()
}

/// `createEventDescriptor` in the game's key tables.
fn parse_descriptor(vm: &Vm, text: &[u8]) -> Option<Descriptor> {
    keys::parse_descriptor_with(text, &key_tables(vm))
}

/// The most nodes one map keeps (an engine cap; TGE's vectors have none).
pub const MAX_BINDINGS: usize = 4_096;
/// The most device maps one map keeps.
pub const MAX_DEVICES: usize = 64;
/// The break table's size (an engine cap).
pub const MAX_BREAKS: usize = 1_024;
/// A saved line's buffer (`lineBuffer[1024]`): what is written of one line.
pub const MAX_SAVE_LINE: usize = 1_023;

/// Node flags.
pub const FLAG_RANGED: u32 = 0x01;
pub const FLAG_SCALE: u32 = 0x02;
pub const FLAG_DEAD_ZONE: u32 = 0x04;
pub const FLAG_INVERTED: u32 = 0x08;
pub const FLAG_BIND_CMD: u32 = 0x10;

/// The action code of `anykey`.
pub const ANY_KEY: u16 = 0xfffe;

#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub modifiers: u8,
    pub action: u16,
    pub flags: u32,
    pub dead_zone: (f32, f32),
    pub scale: f32,
    /// `bind`'s function (the string table's spelling), kept by a later `bindCmd`.
    pub function: Option<Rc<[u8]>>,
    pub make: Option<Vec<u8>>,
    pub break_cmd: Option<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DeviceMap {
    pub kind: u16,
    pub inst: u32,
    pub nodes: Vec<Node>,
}

/// A key held down: what its break does.
#[derive(Debug, Clone, PartialEq)]
pub struct BreakEntry {
    pub kind: u16,
    pub inst: u32,
    pub obj_inst: u16,
    pub function: Option<Rc<[u8]>>,
    pub break_cmd: Option<Vec<u8>>,
    pub flags: u32,
    pub dead_zone: (f32, f32),
    pub scale: f32,
}

/// What visiting one node costs (`Vm::charge_work` units), beside the bytes of what is copied.
const NODE_WORK: u64 = 32;

fn total_nodes(maps: &[DeviceMap]) -> usize {
    maps.iter().map(|d| d.nodes.len()).sum()
}

/// `f` over the map's device maps under a short borrow, every node's visit charged first.
fn with_maps<R, G: ConnGame>(
    vm: &Vm,
    client: &Rc<RefCell<ClientState<G>>>,
    id: ObjId,
    f: impl FnOnce(&[DeviceMap]) -> R,
) -> Result<R, VmError> {
    let state = client.borrow();
    let maps = state.binds.get(&id).map_or(&[][..], Vec::as_slice);
    if !vm.charge_work((total_nodes(maps) as u64 + maps.len() as u64).saturating_mul(NODE_WORK)) {
        return Err(VmError::StepLimit);
    }
    Ok(f(maps))
}

/// `findNode`: the device map, then the node whose modifiers (each side of a pair as the pair) and action are the event's;
/// an `anykey` node with the modifiers takes the key codes 0x21..=0x7e.
pub fn find_node(
    maps: &[DeviceMap],
    kind: u16,
    inst: u32,
    modifiers: u8,
    action: u16,
) -> Option<&Node> {
    let dev = maps.iter().find(|d| d.kind == kind && d.inst == inst)?;
    let m = keys::normalise_modifiers(modifiers);
    dev.nodes.iter().find(|n| {
        if n.modifiers != m {
            return false;
        }
        if n.action == ANY_KEY {
            (0x21..0x7f).contains(&action)
        } else {
            n.action == action
        }
    })
}

/// `getNode`: the device map (made at the end) and its node (made at the end with no flags, scale 1).
fn get_node(maps: &mut Vec<DeviceMap>, kind: u16, inst: u32, d: Descriptor) -> Option<&mut Node> {
    let di = match maps.iter().position(|m| m.kind == kind && m.inst == inst) {
        Some(i) => i,
        None => {
            if maps.len() >= MAX_DEVICES {
                return None;
            }
            maps.push(DeviceMap {
                kind,
                inst,
                nodes: Vec::new(),
            });
            maps.len() - 1
        }
    };
    let total = total_nodes(maps);
    let dev = &mut maps[di];
    let ni = match dev
        .nodes
        .iter()
        .position(|n| n.modifiers == d.modifiers && n.action == d.code)
    {
        Some(i) => i,
        None => {
            if total >= MAX_BINDINGS {
                return None;
            }
            dev.nodes.push(Node {
                modifiers: d.modifiers,
                action: d.code,
                flags: 0,
                dead_zone: (0.0, 0.0),
                scale: 1.0,
                function: None,
                make: None,
                break_cmd: None,
            });
            dev.nodes.len() - 1
        }
    };
    Some(&mut dev.nodes[ni])
}

/// `sscanf(text, "%f %f")`: the two floats (a missing one stays 0).
fn two_floats(text: &[u8]) -> (f32, f32) {
    let mut words = text
        .split(|b| b.is_ascii_whitespace())
        .filter(|w| !w.is_empty())
        .map(|w| ts_vm::text::atof(w) as f32);
    (words.next().unwrap_or(0.0), words.next().unwrap_or(0.0))
}

/// `processBind` over `args` (device, key, [flags, [dead zone,] [scale,]] command). True when bound.
fn process_bind<G: ConnGame>(
    vm: &mut Vm,
    client: &Rc<RefCell<ClientState<G>>>,
    id: ObjId,
    args: &[Vec<u8>],
) -> Result<bool, VmError> {
    let n = args.len();
    let text = |i: usize| args.get(i).map_or(&b""[..], Vec::as_slice);
    let Some((kind, inst)) = keys::device_type_and_instance(text(0)) else {
        vm.print(
            format!(
                "processBind: unknown device: {}",
                String::from_utf8_lossy(text(0))
            )
            .as_bytes(),
        );
        return Ok(false);
    };
    let Some(desc) = parse_descriptor(vm, text(1)) else {
        vm.print(
            format!(
                "Could not create a description for binding: {}",
                String::from_utf8_lossy(text(1))
            )
            .as_bytes(),
        );
        return Ok(false);
    };
    let mut flags = 0u32;
    let mut dead_zone = (0.0f32, 0.0f32);
    let mut scale = 1.0f32;
    if n != 3 {
        for &c in text(2) {
            match c {
                b'D' | b'd' => flags |= FLAG_DEAD_ZONE,
                b'I' | b'i' => flags |= FLAG_INVERTED,
                b'R' | b'S' | b'r' | b's' => flags |= FLAG_SCALE,
                _ => {}
            }
        }
        let mut at = 3;
        if flags & FLAG_DEAD_ZONE != 0 {
            dead_zone = two_floats(text(3));
            at = 4;
        }
        if flags & FLAG_SCALE != 0 {
            scale = args.get(at).map_or(0.0, |s| ts_vm::text::atof(s) as f32);
            at += 1;
        }
        if at != n - 1 {
            // The message names the word after the action (its `argv[2]`, the flags), not the key: `... for key: S`.
            vm.print(
                format!(
                    "Improperly specified bind for key: {}",
                    String::from_utf8_lossy(text(2))
                )
                .as_bytes(),
            );
            return Ok(false);
        }
    }
    let command = text(n - 1).to_vec();
    let function = vm.intern_string(&command);
    let mut state = client.borrow_mut();
    let maps = state.binds.entry(id).or_default();
    if !vm.charge_work(
        (total_nodes(maps) as u64)
            .saturating_mul(NODE_WORK)
            .saturating_add(command.len() as u64),
    ) {
        return Err(VmError::StepLimit);
    }
    if !command.is_empty() {
        // Every other plain node of this command goes first.
        for dev in maps.iter_mut() {
            dev.nodes.retain(|nd| {
                nd.flags & FLAG_BIND_CMD != 0
                    || !nd
                        .function
                        .as_deref()
                        .is_some_and(|f| f.eq_ignore_ascii_case(&command))
            });
        }
    }
    let Some(node) = get_node(maps, kind, inst, desc) else {
        return Ok(false);
    };
    node.flags = flags;
    node.dead_zone = dead_zone;
    node.scale = scale;
    node.function = Some(function);
    Ok(true)
}

/// `processBindCmd`.
fn process_bind_cmd<G: ConnGame>(
    vm: &mut Vm,
    client: &Rc<RefCell<ClientState<G>>>,
    id: ObjId,
    c: &NativeCall,
) -> Result<bool, VmError> {
    let Some((kind, inst)) = keys::device_type_and_instance(c.arg(2)) else {
        vm.print(
            format!(
                "processBindCmd: unknown device: {}",
                String::from_utf8_lossy(c.arg(2))
            )
            .as_bytes(),
        );
        return Ok(false);
    };
    let Some(desc) = parse_descriptor(vm, c.arg(3)) else {
        vm.print(
            format!(
                "Could not create a description for binding: {}",
                String::from_utf8_lossy(c.arg(3))
            )
            .as_bytes(),
        );
        return Ok(false);
    };
    let (make, brk) = (c.arg(4).to_vec(), c.arg(5).to_vec());
    let mut state = client.borrow_mut();
    let maps = state.binds.entry(id).or_default();
    if !vm.charge_work(
        (total_nodes(maps) as u64)
            .saturating_mul(NODE_WORK)
            .saturating_add((make.len() + brk.len()) as u64),
    ) {
        return Err(VmError::StepLimit);
    }
    let Some(node) = get_node(maps, kind, inst, desc) else {
        return Ok(false);
    };
    node.flags = FLAG_BIND_CMD;
    node.dead_zone = (0.0, 0.0);
    node.scale = 1.0;
    node.make = Some(make);
    node.break_cmd = Some(brk);
    Ok(true)
}

/// A node found for a getter, or the getter's message.
fn lookup<R, G: ConnGame>(
    vm: &mut Vm,
    client: &Rc<RefCell<ClientState<G>>>,
    id: ObjId,
    c: &NativeCall,
    message: bool,
    f: impl FnOnce(&Node) -> R,
) -> Result<Option<R>, VmError> {
    let found = match (
        keys::device_type_and_instance(c.arg(2)),
        parse_descriptor(vm, c.arg(3)),
    ) {
        (Some((kind, inst)), Some(d)) => with_maps(vm, client, id, |maps| {
            find_node(maps, kind, inst, d.modifiers, d.code).map(f)
        })?,
        _ => None,
    };
    if found.is_none() && message {
        vm.error(format!(
            "The input event specified by {} {} is not in this action map!",
            String::from_utf8_lossy(c.arg(2)),
            String::from_utf8_lossy(c.arg(3))
        ));
    }
    Ok(found)
}

fn text_result(vm: &Vm, found: Result<Vec<u8>, VmError>) -> Native {
    match found {
        Ok(text) if vm.charge_work(text.len() as u64) => Native::Str(text),
        Ok(_) => Native::Fail(VmError::StepLimit),
        Err(e) => Native::Fail(e),
    }
}

/// `%f` as the console prints it (six decimals).
fn f6(v: f32) -> String {
    format!("{:.6}", f64::from(v))
}

/// The key text a node is saved and answered with (`modifiers key`), None when the key has no name.
fn node_key(t: &keys::KeyTables, n: &Node) -> Option<String> {
    keys::key_string_in(t, n.action).map(|k| format!("{}{k}", keys::modifier_string(n.modifiers)))
}

/// TorqueScript's reserved words (`ts_compiler`'s lexer table), any case: never a name `save` writes bare.
const RESERVED: [&[u8]; 16] = [
    b"or",
    b"break",
    b"return",
    b"else",
    b"while",
    b"if",
    b"for",
    b"continue",
    b"function",
    b"new",
    b"datablock",
    b"case",
    b"switch$",
    b"switch",
    b"default",
    b"package",
];

/// The longest name `save` writes bare (an engine cap; the compiler's identifiers stop at 4096 bytes).
pub const MAX_SAVED_NAME: usize = 255;

/// Whether `text` is a name the compiler reads as one identifier (`[A-Za-z_][A-Za-z0-9_]*`, a reserved word excepted): what `save`
/// writes bare. (A `Namespace::name` is refused too: as a bare argument it is no expression the compiler takes, so the whole file
/// would fail to compile; the action maps call global functions only.) Any other text there would put script into a saved
/// `config.cs`, which every start-up runs; the engine refuses to write it.
pub fn is_script_name(text: &[u8]) -> bool {
    if text.is_empty() || text.len() > MAX_SAVED_NAME {
        return false;
    }
    let ident = |p: &[u8]| {
        !p.is_empty()
            && (p[0].is_ascii_alphabetic() || p[0] == b'_')
            && p.iter().all(|b| b.is_ascii_alphanumeric() || *b == b'_')
            && !RESERVED.iter().any(|r| r.eq_ignore_ascii_case(p))
    };
    ident(text)
}

/// Whether a key text can go between `save`'s quotes as it is (the key table's names have no quote, backslash or control byte; this
/// keeps it so).
fn is_quotable(text: &str) -> bool {
    text.bytes()
        .all(|b| (0x20..0x7f).contains(&b) && b != b'"' && b != b'\\')
}

/// Why a node's line was not written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refused {
    /// The bound function is not a script name ([`is_script_name`]).
    Function(Vec<u8>),
    /// The line would pass the 1023-byte line buffer (the engine writes no cut line).
    TooLong(String),
}

/// One node's line of `dumpActionMap` (no line end): None for a key with no name, `Err` for one the engine will not write.
fn save_line(
    t: &keys::KeyTables,
    name: &[u8],
    device: &str,
    n: &Node,
    escape: bool,
) -> Result<Option<Vec<u8>>, Refused> {
    let Some(key) = node_key(t, n).filter(|k| is_quotable(k)) else {
        return Ok(None);
    };
    let mut line = Vec::new();
    let cmd = if n.flags & FLAG_BIND_CMD != 0 {
        "bindCmd"
    } else {
        "bind"
    };
    line.extend_from_slice(name);
    line.extend_from_slice(format!(".{cmd}({device}, \"{key}\"").as_bytes());
    if n.flags & 0xf != 0 {
        let mut f = String::from(", ");
        if n.flags & FLAG_SCALE != 0 {
            f.push('S');
        }
        if n.flags & FLAG_RANGED != 0 {
            f.push('R');
        }
        if n.flags & FLAG_DEAD_ZONE != 0 {
            f.push('D');
        }
        if n.flags & FLAG_INVERTED != 0 {
            f.push('I');
        }
        line.extend_from_slice(f.as_bytes());
    }
    if n.flags & FLAG_DEAD_ZONE != 0 {
        line.extend_from_slice(
            format!(", \"{} {}\"", f6(n.dead_zone.0), f6(n.dead_zone.1)).as_bytes(),
        );
    }
    if n.flags & FLAG_SCALE != 0 {
        line.extend_from_slice(format!(", {}", f6(n.scale)).as_bytes());
    }
    if n.flags & FLAG_BIND_CMD != 0 {
        // Quoted strings: into a file escaped by `expandEscape` (every quote, backslash and control byte); on the console as
        // they are (the console branch appends the commands as they are: `"echo("a b");"`).
        for cmd in [&n.make, &n.break_cmd] {
            line.extend_from_slice(b", \"");
            if let Some(c) = cmd {
                if escape {
                    let Some(escaped) = ts_text::expand_escape_capped(c, MAX_SAVE_LINE) else {
                        return Err(Refused::TooLong(key));
                    };
                    line.extend_from_slice(&escaped);
                } else {
                    line.extend_from_slice(c);
                }
            }
            line.push(b'"');
        }
    } else {
        line.extend_from_slice(b", ");
        match n.function.as_deref() {
            Some(f) if !f.is_empty() => {
                if !is_script_name(f) {
                    return Err(Refused::Function(f.to_vec()));
                }
                line.extend_from_slice(f);
            }
            _ => line.extend_from_slice(b"\"\""),
        }
    }
    line.extend_from_slice(b");");
    if line.len() > MAX_SAVE_LINE {
        return Err(Refused::TooLong(key));
    }
    Ok(Some(line))
}

/// What `save` writes of a map: its name (None when it is no script name: nothing is written), the lines, the nodes refused.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Saved {
    pub name: Option<Vec<u8>>,
    pub lines: Vec<Vec<u8>>,
    pub refused: Vec<Refused>,
}

/// `ActionMap::dumpActionMap`: the lines `save` writes or prints, as TGE writes them for every binding whose function and
/// map are script names; the others are refused ([`Saved`]).
pub fn save_text<G: ConnGame>(
    vm: &Vm,
    client: &Rc<RefCell<ClientState<G>>>,
    id: ObjId,
    escape: bool,
) -> Result<Saved, VmError> {
    let name = vm
        .objects()
        .get(id)
        .and_then(|o| o.name().map(<[u8]>::to_vec))
        .filter(|n| is_script_name(n));
    let Some(name) = name else {
        return Ok(Saved::default());
    };
    let t = key_tables(vm);
    with_maps(vm, client, id, |maps| {
        let mut saved = Saved {
            name: Some(name.clone()),
            ..Saved::default()
        };
        for dev in maps {
            let Some(device) = keys::device_name(dev.kind, dev.inst) else {
                continue;
            };
            for n in &dev.nodes {
                match save_line(&t, &name, &device, n, escape) {
                    Ok(Some(line)) => saved.lines.push(line),
                    Ok(None) => {}
                    Err(r) => saved.refused.push(r),
                }
            }
        }
        saved
    })
}

/// The console's warning for each binding `save` did not write.
fn warn_refused(vm: &mut Vm, saved: &Saved) {
    for r in &saved.refused {
        let why = match r {
            Refused::Function(f) => format!(
                "the function \"{}\" is not a script name",
                String::from_utf8_lossy(&ts_text::expand_escape(&f[..f.len().min(64)]))
            ),
            Refused::TooLong(k) => {
                format!("the line for \"{k}\" is longer than {MAX_SAVE_LINE} bytes")
            }
        };
        vm.warn(format!("ActionMap::save: a binding was not written: {why}"));
    }
}

fn reg<G: ConnGame>(
    vm: &mut Vm,
    client: &Rc<RefCell<ClientState<G>>>,
    name: &str,
    min: i32,
    max: i32,
    usage: &str,
    f: impl Fn(&mut Vm, &Rc<RefCell<ClientState<G>>>, ObjId, &NativeCall) -> Native + 'static,
) {
    let client = Rc::clone(client);
    vm.register_in("ActionMap", name, min, max, usage, move |vm, c| {
        match c.this(vm) {
            Some(id) => f(vm, &client, id, c),
            None => Native::Void,
        }
    });
}

/// `bind`, `bindCmd` and `unbind` are void (`ConsoleMethod(ActionMap, bind, void, ...)`): a script that uses the result gets the
/// "uses result of void function call" error and an empty string.
fn done_void(r: Result<bool, VmError>) -> Native {
    match r {
        Ok(_) => Native::Void,
        Err(e) => Native::Fail(e),
    }
}

pub fn register<G: ConnGame>(vm: &mut Vm, client: &Rc<RefCell<ClientState<G>>>) {
    // `bindCmd` binds script text that runs on the key; `save` writes the binds to a file.
    vm.add_native_effects("ActionMap", "bindCmd", ts_vm::effects::Effects::CODE);
    vm.add_native_effects("ActionMap", "save", ts_vm::effects::Effects::FILES);
    reg(
        vm,
        client,
        "bind",
        5,
        10,
        "actionMap.bind( device, action, [modifier spec, mod...], command )",
        |vm, client, id, c| {
            let args: Vec<Vec<u8>> = c.rest(2).to_vec();
            done_void(process_bind(vm, client, id, &args))
        },
    );
    reg(
        vm,
        client,
        "bindCmd",
        6,
        6,
        "actionMap.bindCmd( device, action, makeCmd, breakCmd )",
        |vm, client, id, c| done_void(process_bind_cmd(vm, client, id, c)),
    );
    reg(
        vm,
        client,
        "unbind",
        4,
        4,
        "actionMap.unbind( device, action )",
        |vm, client, id, c| {
            let (Some((kind, inst)), Some(d)) = (
                keys::device_type_and_instance(c.arg(2)),
                parse_descriptor(vm, c.arg(3)),
            ) else {
                return Native::Void;
            };
            let m = keys::normalise_modifiers(d.modifiers);
            let mut state = client.borrow_mut();
            let maps = state.binds.entry(id).or_default();
            if !vm.charge_work((total_nodes(maps) as u64).saturating_mul(NODE_WORK)) {
                return Native::Fail(VmError::StepLimit);
            }
            if let Some(dev) = maps
                .iter_mut()
                .find(|dm| dm.kind == kind && dm.inst == inst)
            {
                dev.nodes
                    .retain(|n| !(n.modifiers == m && n.action == d.code));
            }
            Native::Void
        },
    );
    reg(
        vm,
        client,
        "getBinding",
        3,
        3,
        "actionMap.getBinding( command )",
        |vm, client, id, c| {
            let command = c.arg(2);
            let t = key_tables(vm);
            let text = with_maps(vm, client, id, |maps| {
                for dev in maps {
                    for n in &dev.nodes {
                        if n.flags & FLAG_BIND_CMD == 0
                            && n.function
                                .as_deref()
                                .is_some_and(|f| f.eq_ignore_ascii_case(command))
                        {
                            return match (keys::device_name(dev.kind, dev.inst), node_key(&t, n)) {
                                (Some(d), Some(k)) => format!("{d}\t{k}").into_bytes(),
                                _ => Vec::new(),
                            };
                        }
                    }
                }
                Vec::new()
            });
            text_result(vm, text)
        },
    );
    reg(
        vm,
        client,
        "getNumBinds",
        2,
        2,
        "actionMap.getNumBinds()",
        |_, client, id, _| {
            Native::Int(client.borrow().binds.get(&id).map_or(0, |m| total_nodes(m)) as i32)
        },
    );
    reg(
        vm,
        client,
        "getCommand",
        4,
        4,
        "actionMap.getCommand( device, action )",
        |vm, client, id, c| {
            let found = lookup(vm, client, id, c, false, |n| {
                if n.flags & FLAG_BIND_CMD != 0 {
                    let mut t = n.make.clone().unwrap_or_default();
                    t.push(b'\t');
                    t.extend_from_slice(n.break_cmd.as_deref().unwrap_or_default());
                    t
                } else {
                    n.function
                        .as_deref()
                        .map(<[u8]>::to_vec)
                        .unwrap_or_default()
                }
            });
            text_result(vm, found.map(Option::unwrap_or_default))
        },
    );
    reg(
        vm,
        client,
        "isInverted",
        4,
        4,
        "actionMap.isInverted( device, action )",
        |vm, client, id, c| match lookup(vm, client, id, c, true, |n| n.flags & FLAG_INVERTED != 0)
        {
            Ok(v) => Native::Bool(v.unwrap_or(false)),
            Err(e) => Native::Fail(e),
        },
    );
    reg(
        vm,
        client,
        "getScale",
        4,
        4,
        "actionMap.getScale( device, action )",
        |vm, client, id, c| match lookup(vm, client, id, c, true, |n| {
            if n.flags & FLAG_SCALE != 0 {
                n.scale
            } else {
                1.0
            }
        }) {
            Ok(v) => Native::Float(f64::from(v.unwrap_or(1.0))),
            Err(e) => Native::Fail(e),
        },
    );
    reg(
        vm,
        client,
        "getDeadZone",
        4,
        4,
        "actionMap.getDeadZone( device, action )",
        |vm, client, id, c| {
            let found = lookup(vm, client, id, c, true, |n| {
                if n.flags & FLAG_DEAD_ZONE != 0 {
                    format!("{} {}", f6(n.dead_zone.0), f6(n.dead_zone.1)).into_bytes()
                } else {
                    b"0 0".to_vec()
                }
            });
            text_result(vm, found.map(Option::unwrap_or_default))
        },
    );
    reg(
        vm,
        client,
        "save",
        2,
        4,
        "actionMap.save( [fileName], [append] )",
        |vm, client, id, c| {
            // A file gets the commands escaped, the console as they are.
            let saved = match save_text(vm, client, id, c.argc() >= 3) {
                Ok(s) => s,
                Err(e) => return Native::Fail(e),
            };
            // A map whose name is no script name writes nothing (its `NAME.delete();` would be script of its own).
            let Some(name) = saved.name.clone() else {
                vm.warn(
                    "ActionMap::save: the map's name is not a script name; nothing was written"
                        .to_string(),
                );
                return Native::Void;
            };
            warn_refused(vm, &saved);
            if c.argc() < 3 {
                for line in &saved.lines {
                    vm.print(line);
                }
                return Native::Void;
            }
            let append = c.argc() >= 4 && c.boolean(3);
            let file = vm
                .expand_script_filename(c.arg(2))
                .unwrap_or_else(|_| c.arg(2).to_vec());
            let mut out = Vec::new();
            if !append {
                out.extend_from_slice(b"// Torque Input Map File\n");
            }
            let n = String::from_utf8_lossy(&name).into_owned();
            let head = format!("{n}.delete();\nnew ActionMap({n});\n");
            out.extend_from_slice(head.as_bytes());
            for line in &saved.lines {
                out.extend_from_slice(line);
                out.push(b'\n');
            }
            match vm.write_engine_file(&file, &out, append) {
                Ok(true) => Native::Void,
                Ok(false) => {
                    vm.error(format!(
                        "Unable to open file '{}' for writing.",
                        String::from_utf8_lossy(&file)
                    ));
                    Native::Void
                }
                Err(e) => Native::Fail(e),
            }
        },
    );
    // `push`: to the top of ActiveActionMapSet (`pushObject`); `pop`: out of it.
    reg(
        vm,
        client,
        "push",
        2,
        2,
        "actionMap.push()",
        |vm, _, id, _| {
            if let Some(set) = vm.objects().find(b"ActiveActionMapSet") {
                vm.objects_mut().add(set, id);
                vm.objects_mut().move_member(set, id, false);
            }
            Native::Void
        },
    );
    reg(
        vm,
        client,
        "pop",
        2,
        2,
        "actionMap.pop()",
        |vm, _, id, _| {
            if let Some(set) = vm.objects().find(b"ActiveActionMapSet") {
                vm.objects_mut().remove(set, id);
            }
            Native::Void
        },
    );
}

/// `%g` of a value as the console prints a float argument.
fn g(v: f32) -> Vec<u8> {
    ts_vm::text::format_g(f64::from(v)).into_bytes()
}

/// A plain or inverted, scaled value (TGE 1.4's make and break rules).
fn shaped(flags: u32, scale: f32, dead_zone: (f32, f32), value: f32) -> f32 {
    let mut v = value;
    if flags & FLAG_RANGED != 0 {
        v = v * 2.0 - 1.0;
        if flags & FLAG_INVERTED != 0 {
            v = -v;
        }
    } else if flags & FLAG_INVERTED != 0 {
        v = 1.0 - v;
    }
    if flags & FLAG_SCALE != 0 {
        v *= scale;
    }
    if flags & FLAG_DEAD_ZONE != 0 {
        if v >= dead_zone.0 && v <= dead_zone.1 {
            v = 0.0;
        } else if v > 0.0 {
            v = (v - dead_zone.0) * (1.0 / (1.0 - dead_zone.0));
        } else {
            v = (v + dead_zone.0) * (1.0 / (1.0 - dead_zone.0));
        }
    }
    v
}

/// `ActionMap::processAction` of the map `id`: whether the map took the event. Scripts run with no borrow held.
pub fn process_action<G: ConnGame>(
    vm: &mut Vm,
    client: &Rc<RefCell<ClientState<G>>>,
    id: ObjId,
    ev: &InputEvent,
) -> Result<bool, VmError> {
    match ev.action {
        SI_MAKE => {
            let node = with_maps(vm, client, id, |maps| {
                find_node(maps, ev.device, ev.device_inst, ev.modifier, ev.obj_inst)
                    .or_else(|| {
                        (ev.modifier != 0)
                            .then(|| find_node(maps, ev.device, ev.device_inst, 0, ev.obj_inst))
                            .flatten()
                    })
                    .cloned()
            })?;
            let Some(node) = node else {
                return Ok(false);
            };
            {
                let mut state = client.borrow_mut();
                let entry = BreakEntry {
                    kind: ev.device,
                    inst: ev.device_inst,
                    obj_inst: ev.obj_inst,
                    function: node.function.clone(),
                    break_cmd: node.break_cmd.clone(),
                    flags: node.flags,
                    dead_zone: node.dead_zone,
                    scale: node.scale,
                };
                if let Some(e) = state.breaks.iter_mut().find(|e| {
                    e.kind == ev.device && e.inst == ev.device_inst && e.obj_inst == ev.obj_inst
                }) {
                    *e = entry;
                } else if state.breaks.len() < MAX_BREAKS {
                    state.breaks.push(entry);
                }
            }
            if node.flags & FLAG_BIND_CMD != 0 {
                if let Some(make) = node.make.as_deref().filter(|m| !m.is_empty()) {
                    vm.eval(make)?;
                }
            } else if let Some(f) = node.function.as_deref().filter(|f| !f.is_empty()) {
                let value = shaped(node.flags, node.scale, node.dead_zone, ev.value);
                vm.invoke(None, f, &[g(value)])?;
            }
            Ok(true)
        }
        SI_BREAK => check_break(vm, client, ev),
        SI_MOVE => {
            let node = with_maps(vm, client, id, |maps| {
                find_node(maps, ev.device, ev.device_inst, ev.modifier, ev.obj_type)
                    .or_else(|| {
                        (ev.modifier != 0)
                            .then(|| find_node(maps, ev.device, ev.device_inst, 0, ev.obj_type))
                            .flatten()
                    })
                    .cloned()
            })?;
            let Some(node) = node else {
                return Ok(false);
            };
            if let Some(f) = node.function.as_deref().filter(|f| !f.is_empty()) {
                let mut v = ev.value;
                if node.flags & FLAG_INVERTED != 0 {
                    v = -v;
                }
                if node.flags & FLAG_SCALE != 0 {
                    v *= node.scale;
                }
                vm.invoke(None, f, &[g(v)])?;
            }
            Ok(true)
        }
        _ => Ok(false),
    }
}

/// The break table: the key's entry, if any, fires (the function with the value, else the break command) and goes.
fn check_break<G: ConnGame>(
    vm: &mut Vm,
    client: &Rc<RefCell<ClientState<G>>>,
    ev: &InputEvent,
) -> Result<bool, VmError> {
    let entry = {
        let mut state = client.borrow_mut();
        let at = state.breaks.iter().position(|e| {
            e.kind == ev.device && e.inst == ev.device_inst && e.obj_inst == ev.obj_inst
        });
        at.map(|i| state.breaks.remove(i))
    };
    let Some(e) = entry else {
        return Ok(false);
    };
    match e.function.as_deref() {
        None => {
            if let Some(cmd) = e.break_cmd.as_deref().filter(|c| !c.is_empty()) {
                vm.eval(cmd)?;
            }
        }
        Some(f) if !f.is_empty() => {
            let value = shaped(e.flags, e.scale, e.dead_zone, ev.value);
            vm.invoke(None, f, &[g(value)])?;
        }
        Some(_) => {}
    }
    Ok(true)
}
