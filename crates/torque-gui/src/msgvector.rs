//! MessageVector and GuiMessageVectorCtrl's attachment as TGE 1.4 has them.
//!
//! - **MessageVector** (TGE 1.4's): a list of lines, each a text and an integer tag. `pushBackLine(msg, tag)` (the tag is required) and
//!   `pushFrontLine(msg, tag = 0)` insert at the end and the front; `insertLine(pos, msg, tag = 0)` inserts
//!   before line `pos` and answers 0 when `pos` (unsigned) is past the end; `popBackLine` / `popFrontLine`
//!   delete one line, or print `MessageVector::popBackLine(): underflow` (`...popFrontLine...`, errors) and answer 0 on an empty
//!   vector; `deleteLine(line)` answers 0 for a line past the end; `clear` deletes from the back.
//!   `getNumLines` is the count; `getLineText(line)` / `getLineTag(line)` print
//!   `MessageVector::getLineText(con): out of bounds line` (`getLineTag`) and answer "" / 0 past the end; `getLineTextByTag(tag)`
//!   and `getLineIndexByTag(tag)` find the first line with that tag ("" / -1 when none has it).
//!   `dump(file, header)` (writes a file) is not implemented.
//! - **GuiMessageVectorCtrl** `attach(vector)`: `Could not find MessageVector: <name>` (an error) and 0 for
//!   what is not one; 0 while the control is asleep; else a former attachment is replaced (`GuiMessageVectorCtrl::attach:
//!   overriding attachment`, a warning) and 1. `detach()`: `GuiMessageVectorCtrl: double detach` (a warning) when
//!   nothing is attached. Sleeping detaches. Only the attachment is kept: the control draws no lines yet, and a vector that is
//!   deleted counts as detached.
//!
//! Credit: where Torque3D has the same rules, this module follows its MIT-licensed `gui/utility/messageVector.cpp` (Copyright
//! (c) 2012 GarageGames, LLC), `gui/game/guiMessageVectorCtrl.cpp` (Copyright (c) 2012 GarageGames, LLC); the code here is this
//! crate's own.

use std::collections::VecDeque;

use torque_ts_vm::sim::ObjId;
use torque_ts_vm::vm::{Native, NativeCall, Vm};

use crate::{Gui, arg, atoi, this};

/// The most lines a vector keeps (a safety cap: TGE's grows without a bound; a script that pushes more loses the oldest).
pub const MAX_LINES: usize = 1 << 16;
/// The most bytes a line keeps (a safety cap).
pub const MAX_LINE_BYTES: usize = 1 << 16;
/// The most bytes of text a vector keeps in all (a safety cap: 65,536 lines of 64 KiB were 4 GiB): past it the oldest lines
/// go, as past [`MAX_LINES`].
pub const MAX_VECTOR_BYTES: usize = 16 << 20;

/// A MessageVector's lines (the object's `ext`): a double-ended queue, so dropping the oldest is not a shift of every line,
/// and the bytes of text they hold.
#[derive(Debug, Default)]
struct Lines {
    lines: VecDeque<(Vec<u8>, i32)>,
    bytes: usize,
}

impl Lines {
    fn len(&self) -> usize {
        self.lines.len()
    }

    fn get(&self, at: usize) -> Option<&(Vec<u8>, i32)> {
        self.lines.get(at)
    }

    fn iter(&self) -> impl Iterator<Item = &(Vec<u8>, i32)> {
        self.lines.iter()
    }

    fn clear(&mut self) {
        self.lines.clear();
        self.bytes = 0;
    }

    fn taken(&mut self, line: Option<(Vec<u8>, i32)>) -> Option<(Vec<u8>, i32)> {
        if let Some((text, _)) = &line {
            self.bytes -= text.len();
        }
        line
    }

    fn pop_back(&mut self) -> Option<(Vec<u8>, i32)> {
        let line = self.lines.pop_back();
        self.taken(line)
    }

    fn pop_front(&mut self) -> Option<(Vec<u8>, i32)> {
        let line = self.lines.pop_front();
        self.taken(line)
    }

    fn remove(&mut self, at: usize) -> Option<(Vec<u8>, i32)> {
        let line = self.lines.remove(at);
        self.taken(line)
    }

    /// A line before `at` (at most the end), then the oldest dropped while the vector is past [`MAX_LINES`] or [`MAX_VECTOR_BYTES`].
    fn insert(&mut self, at: usize, text: Vec<u8>, tag: i32) {
        let at = at.min(self.lines.len());
        self.bytes += text.len();
        self.lines.insert(at, (text, tag));
        while self.lines.len() > MAX_LINES || self.bytes > MAX_VECTOR_BYTES {
            if self.pop_front().is_none() {
                break;
            }
        }
    }
}

fn lines<R>(vm: &mut Vm, id: ObjId, f: impl FnOnce(&mut Lines) -> R) -> Option<R> {
    let object = vm.objects_mut().get_mut(id)?;
    let ext = object.ext.get_or_insert_with(|| Box::new(Lines::default()));
    ext.downcast_mut::<Lines>().map(f)
}

fn is_vector(vm: &Vm, id: ObjId) -> bool {
    crate::control::is_a(vm, id, "MessageVector")
}

/// `argv[i]` as an unsigned index (`atoi` read as a `U32`).
fn index(c: &NativeCall, i: usize) -> usize {
    atoi(arg(c, i)) as u32 as usize
}

fn insert(vm: &mut Vm, id: ObjId, at: usize, text: &[u8], tag: i32) {
    let mut text = text.to_vec();
    text.truncate(MAX_LINE_BYTES);
    lines(vm, id, |l| l.insert(at, text, tag));
}

fn method(
    vm: &mut Vm,
    name: &str,
    min: i32,
    max: i32,
    usage: &str,
    f: impl Fn(&mut Vm, ObjId, &NativeCall) -> Native + 'static,
) {
    vm.register_in(
        "MessageVector",
        name,
        min,
        max,
        usage,
        move |vm, c| match this(vm, c) {
            Some(id) if is_vector(vm, id) => f(vm, id, c),
            _ => Native::Void,
        },
    );
}

fn tag_arg(c: &NativeCall, i: usize) -> i32 {
    if c.argv.len() > i { atoi(arg(c, i)) } else { 0 }
}

pub(crate) fn register(vm: &mut Vm, gui: &Gui) {
    method(vm, "clear", 2, 2, "()", |vm, id, _| {
        lines(vm, id, Lines::clear);
        Native::Void
    });
    method(
        vm,
        "pushBackLine",
        4,
        4,
        "(string msg, int tag=0)",
        |vm, id, c| {
            let n = lines(vm, id, |l| l.len()).unwrap_or(0);
            insert(vm, id, n, arg(c, 2), tag_arg(c, 3));
            Native::Void
        },
    );
    method(vm, "popBackLine", 2, 2, "()", |vm, id, _| {
        match lines(vm, id, Lines::pop_back).flatten() {
            Some(_) => Native::Bool(true),
            None => {
                vm.error("MessageVector::popBackLine(): underflow".to_string());
                Native::Bool(false)
            }
        }
    });
    method(
        vm,
        "pushFrontLine",
        3,
        4,
        "(string msg, int tag=0)",
        |vm, id, c| {
            insert(vm, id, 0, arg(c, 2), tag_arg(c, 3));
            Native::Void
        },
    );
    method(vm, "popFrontLine", 2, 2, "()", |vm, id, _| {
        let popped = lines(vm, id, Lines::pop_front).flatten();
        if popped.is_some() {
            Native::Bool(true)
        } else {
            vm.error("MessageVector::popFrontLine(): underflow".to_string());
            Native::Bool(false)
        }
    });
    method(
        vm,
        "insertLine",
        4,
        5,
        "(int insertPos, string msg, int tag=0)",
        |vm, id, c| {
            let at = index(c, 2);
            if at > lines(vm, id, |l| l.len()).unwrap_or(0) {
                return Native::Bool(false);
            }
            insert(vm, id, at, arg(c, 3), tag_arg(c, 4));
            Native::Bool(true)
        },
    );
    method(vm, "deleteLine", 3, 3, "(int deletePos)", |vm, id, c| {
        let at = index(c, 2);
        let gone = lines(vm, id, |l| l.remove(at)).flatten();
        Native::Bool(gone.is_some())
    });
    method(vm, "getNumLines", 2, 2, "()", |vm, id, _| {
        Native::Int(lines(vm, id, |l| l.len() as i32).unwrap_or(0))
    });
    method(vm, "getLineText", 3, 3, "(int line)", |vm, id, c| {
        let at = index(c, 2);
        match lines(vm, id, |l| l.get(at).map(|x| x.0.clone())).flatten() {
            Some(t) => Native::Str(t),
            None => {
                vm.error("MessageVector::getLineText(con): out of bounds line".to_string());
                Native::Str(Vec::new())
            }
        }
    });
    method(vm, "getLineTag", 3, 3, "(int line)", |vm, id, c| {
        let at = index(c, 2);
        match lines(vm, id, |l| l.get(at).map(|x| x.1)).flatten() {
            Some(t) => Native::Int(t),
            None => {
                vm.error("MessageVector::getLineTag(con): out of bounds line".to_string());
                Native::Int(0)
            }
        }
    });
    method(vm, "getLineTextByTag", 3, 3, "(int tag)", |vm, id, c| {
        let tag = atoi(arg(c, 2));
        let text = lines(vm, id, |l| {
            l.iter().find(|x| x.1 == tag).map(|x| x.0.clone())
        })
        .flatten()
        .unwrap_or_default();
        Native::Str(text)
    });
    method(vm, "getLineIndexByTag", 3, 3, "(int tag)", |vm, id, c| {
        let tag = atoi(arg(c, 2));
        let at = lines(vm, id, |l| l.iter().position(|x| x.1 == tag)).flatten();
        Native::Int(at.map_or(-1, |i| i as i32))
    });
    crate::classes::reg(
        vm,
        gui,
        "GuiMessageVectorCtrl",
        "attach",
        3,
        3,
        "(MessageVector item)",
        |vm, gui, id, c| {
            let vector = vm.objects().find(arg(c, 2)).filter(|v| is_vector(vm, *v));
            let Some(vector) = vector else {
                vm.error(format!(
                    "Could not find MessageVector: {}",
                    String::from_utf8_lossy(arg(c, 2))
                ));
                return Native::Bool(false);
            };
            if !gui.is_awake(id) {
                return Native::Bool(false);
            }
            if attached(vm, gui, id).is_some() {
                vm.warn("GuiMessageVectorCtrl::attach: overriding attachment".to_string());
            }
            gui.with_mut(id, |c| c.message_vector = Some(vector));
            Native::Bool(true)
        },
    );
    crate::classes::reg(
        vm,
        gui,
        "GuiMessageVectorCtrl",
        "detach",
        2,
        2,
        "()",
        |vm, gui, id, _| {
            if attached(vm, gui, id).is_none() {
                vm.warn("GuiMessageVectorCtrl: double detach".to_string());
            }
            gui.with_mut(id, |c| c.message_vector = None);
            Native::Void
        },
    );
}

/// The vector a GuiMessageVectorCtrl is attached to, if it still exists.
fn attached(vm: &Vm, gui: &Gui, id: ObjId) -> Option<ObjId> {
    gui.with(id, |c| c.message_vector)
        .flatten()
        .filter(|v| vm.objects().contains(*v))
}

/// A control going to sleep lets go of its vector.
pub(crate) fn on_sleep(gui: &Gui, id: ObjId) {
    gui.with_mut(id, |c| c.message_vector = None);
}

#[cfg(test)]
#[path = "msgvector_tests.rs"]
mod tests;
