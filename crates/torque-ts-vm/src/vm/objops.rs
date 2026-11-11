//! The object opcodes: `new` and `datablock` declarations (CREATE_OBJECT, ADD_OBJECT, END_OBJECT) and
//! field access (SETCUROBJECT, SETCURFIELD, LOADFIELD, SAVEFIELD), as TGE's interpreter loop runs them.
//!
//! Follows Torque3D's MIT-licensed `console/torquescript/compiledEval.cpp` (object creation) (Copyright (c) 2013
//! GarageGames, LLC; Copyright (c) 2015 Faust Logic, Inc.; Copyright (c) 2021 TGEMIT Authors & Contributors) and
//! `console/simObject.cpp` (Copyright (c) 2012 GarageGames, LLC; Copyright (c) 2015 Faust Logic, Inc.); the code here
//! is this crate's own.

use std::rc::Rc;

use super::{PRELOAD_ERROR_BYTES, Vm, VmError, lossy};
use crate::classes::{EngineClass, PreloadError};
use crate::code::CodeBlock;
use crate::sim::{
    ClassId, ObjId, ROOT_GROUP_ID, SCRIPT_GROUP, SCRIPT_OBJECT, SIM_DATA_BLOCK, SimObject,
};

/// Extra call-depth units charged while a class's behaviours (`EngineClass`) run: one more than the re-entry
/// (`REENTRY_COST`) of the script a behaviour runs. The frames between a script that makes or deletes an object
/// and the script its behaviour runs are `add_object` or `delete_now`, the class chain's loop and the
/// behaviour's own frames, which are the host's and as large as it likes; a recursion through them (a script
/// that makes an object whose behaviour runs a script that makes an object...) stops at about 85 levels instead
/// of 128, inside the stack the `Vm` documents (as `files::REFUSAL_BACKTRACE_COST` does for a refused path's
/// BackTrace).
const BEHAVIOUR_COST: usize = 1;

/// What `datablock Class(name)` found in DataBlockGroup (see `find_declared_datablock`).
enum Declared {
    Absent,
    Reused,
    Refused,
}

/// What one interpreter frame is building with `new` (TGE's `currentNewObject` and
/// `failJump`). Both are locals of the interpreter loop: a `new` written inside a field value
/// of another `new` replaces the object being built (the outer object is never registered),
/// while a `new` inside a called function has its own frame.
#[derive(Default)]
pub(crate) struct Creating {
    pub(crate) object: Option<NewObject>,
    pub(crate) fail_jump: usize,
}

pub(crate) enum NewObject {
    /// Made by CREATE_OBJECT, not registered yet.
    Pending(Box<SimObject>),
    /// Registered: by ADD_OBJECT, or an existing datablock being declared again.
    Live(ObjId),
}

/// The object the field opcodes act on (`curObject`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum FieldTarget {
    #[default]
    None,
    Live(ObjId),
    /// The object this frame is building (`SETCUROBJECT_NEW`).
    New,
}

/// What [`Vm::add_new_object`] did with the object the frame was building.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddedObject {
    /// Registered (or an existing datablock declared again) and placed: its id.
    Added(ObjId),
    /// Its registration or its preload failed: the declaration's fail jump (from [`Vm::begin_new_object`]).
    Failed(usize),
}

/// How a declaration names and finds its object beyond TGE's `new` and `datablock` (Torque3D's `CREATE_OBJECT`
/// operands): see [`Vm::begin_new_object_as`]. Follows Torque3D's MIT-licensed `compiledEval.cpp` (Copyright (c) 2013
/// GarageGames, LLC; Copyright (c) 2015 Faust Logic, Inc.; Copyright (c) 2021 TGEMIT Authors & Contributors); the
/// code here is this crate's own.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Declaration {
    /// `datablock Class(name)`.
    pub datablock: bool,
    /// The name is the object's internal name ([`SimObject::internal_name`]), not its object name.
    pub internal_name: bool,
    /// An existing object the declaration fills in instead of making one (Torque3D's `singleton` found by name):
    /// it is not registered again.
    pub reuse: Option<ObjId>,
}

/// The object a frame was building, set aside while a declaration nested in it builds its own (see
/// [`Vm::suspend_new_object`]).
pub struct SuspendedObject(Creating);

impl Vm {
    /// A frame starts with nothing being built. Out of line: `exec`'s frame is multiplied by the call depth.
    #[inline(never)]
    pub(crate) fn push_creating(&mut self) {
        self.creating.push(Creating::default());
    }

    /// A frame ends: whatever it was building and had not registered is dropped.
    #[inline(never)]
    pub(crate) fn pop_creating(&mut self, keep: usize) {
        self.creating.truncate(keep);
    }

    fn creating_mut(&mut self) -> Option<&mut Creating> {
        self.creating.last_mut()
    }

    /// Sets aside what the running frame is building, and its fail jump, for a declaration nested in it (Torque3D
    /// keeps a stack of them per frame); the frame then builds nothing until [`resume_new_object`](Self::resume_new_object).
    pub fn suspend_new_object(&mut self) -> SuspendedObject {
        SuspendedObject(self.creating_mut().map(std::mem::take).unwrap_or_default())
    }

    /// The running frame goes back to building what [`suspend_new_object`](Self::suspend_new_object) set aside.
    pub fn resume_new_object(&mut self, suspended: SuspendedObject) {
        if let Some(creating) = self.creating_mut() {
            *creating = suspended.0;
        }
    }

    /// Whether the running frame is building an object (registered or not).
    pub fn is_making_object(&self) -> bool {
        self.creating.last().is_some_and(|c| c.object.is_some())
    }

    /// OP_CREATE_OBJECT: a declaration starts. `site` is the block and code index of the opcode, `parent` the
    /// parent object's name (None for none), `body` and `fail` where the executor goes on when the object is
    /// being made or cannot be (the 0 pushed before stays the result); `argv` the class, the name and the
    /// constructor arguments. Returns `body` or `fail`.
    #[inline(never)]
    pub fn begin_new_object(
        &mut self,
        site: (&CodeBlock, usize),
        (parent, is_datablock): (Option<Vec<u8>>, bool),
        (body, fail): (usize, usize),
        argv: Vec<Vec<u8>>,
        no_calls: bool,
    ) -> Result<usize, VmError> {
        let declaration = Declaration {
            datablock: is_datablock,
            ..Declaration::default()
        };
        self.begin_new_object_as(site, (parent, declaration), (body, fail), argv, no_calls)
    }

    /// [`begin_new_object`](Self::begin_new_object) for a declaration that may name an internal name or reuse an
    /// existing object ([`Declaration`]).
    #[inline(never)]
    pub fn begin_new_object_as(
        &mut self,
        site: (&CodeBlock, usize),
        (parent, declaration): (Option<Vec<u8>>, Declaration),
        (body, fail): (usize, usize),
        argv: Vec<Vec<u8>>,
        no_calls: bool,
    ) -> Result<usize, VmError> {
        let (block, op_ip) = site;
        let is_datablock = declaration.datablock;
        if let Some(creating) = self.creating_mut() {
            creating.object = None;
            creating.fail_jump = fail;
        }
        if no_calls {
            return Ok(fail);
        }
        if let Some(id) = declaration.reuse.filter(|id| self.objects.contains(*id)) {
            if let Some(creating) = self.creating_mut() {
                creating.object = Some(NewObject::Live(id));
            }
            return Ok(body);
        }
        let class_name = argv.get(1).cloned().unwrap_or_default();
        let name = argv.get(2).cloned().unwrap_or_default();
        if is_datablock {
            match self.find_declared_datablock(&class_name, &name) {
                Declared::Absent => {}
                Declared::Reused => return Ok(body),
                Declared::Refused => return Ok(fail),
            }
        }
        // The messages name the line of the CREATE_OBJECT opcode, at the level the console rules give
        // (errors in TGE 1.4).
        let Some(class) = self.objects.classes.find(&class_name) else {
            self.report_new_refusal(format!(
                "{}: Unable to instantiate non-conobject class {}.",
                block.file_line(op_ip),
                lossy(&class_name)
            ));
            return Ok(fail);
        };
        if !self.is_sim_object_class(class) {
            self.warn_non_sim_object((block, op_ip), &class_name);
            return Ok(fail);
        }
        let mut object = SimObject::new(class, &self.objects.classes);
        // A fresh object copies its class's slots (a class with array fields has about a thousand): charged by the
        // slot, as a `: parent` copy is.
        self.objects.add_work(object.slot_count() as u64);
        // The classes' constructors (`EngineClass::on_construct`): no script runs, only the new object changes.
        for behaviour in self.objects.classes.behaviours(class) {
            behaviour.on_construct(&self.objects, &mut object);
        }
        if is_datablock && !self.take_datablock_id((block, op_ip), &mut object, &class_name) {
            return Ok(fail);
        }
        // The compiler emits an identifier for the empty string when a declaration has no parent, and
        // TGE guards with `if(*objParent)`.
        if let Some(parent) = parent.as_ref().filter(|p| !p.is_empty()) {
            self.copy_parent(&mut object, parent, (block, op_ip), &class_name)?;
        }
        // `assignName` keeps the name through the string table (`StringTable->insert`): a runtime name reads in the
        // spelling the table has (`setName("Hud_Box")` names the object `HUD_Box` when a DSO had that identifier first).
        let name = if name.is_empty() {
            name
        } else {
            {
                let end = name.iter().position(|b| *b == 0).unwrap_or(name.len());
                self.strings.intern(&name[..end]).to_vec()
            }
        };
        if declaration.internal_name {
            object.set_internal_name(&name);
        } else {
            object.set_initial_name(&name);
        }
        // `processArguments`: only classes that take constructor arguments accept any; the others fail
        // the declaration without a message.
        let takes_arguments = self
            .objects
            .classes
            .get(class)
            .is_some_and(|c| c.takes_arguments);
        if argv.len() > 3 {
            if !takes_arguments {
                return Ok(fail);
            }
            let args = &argv[3..];
            for behaviour in self.objects.classes.behaviours(class) {
                if !behaviour.process_arguments(&self.objects, &mut object, args) {
                    return Ok(fail);
                }
            }
        }
        if let Some(creating) = self.creating_mut() {
            creating.object = Some(NewObject::Pending(Box::new(object)));
        }
        Ok(body)
    }

    /// The `: parent` of a declaration (`assignFieldsFrom`): the parent's fields are copied into the new
    /// object. A copy copies every byte of the parent's dynamic fields, so it is charged by them BEFORE it is
    /// made (`SimObject::copy_fields_from`), and one the step budget cannot pay for ends the run with
    /// `StepLimit` having allocated nothing. A parent that does not exist is a warning. Out of line: it is
    /// on the frame path of `create_object`.
    #[inline(never)]
    fn copy_parent(
        &mut self,
        object: &mut SimObject,
        parent: &[u8],
        (block, op_ip): (&CodeBlock, usize),
        class_name: &[u8],
    ) -> Result<(), VmError> {
        let Some(found) = self.objects.find(parent) else {
            self.report_new_refusal(format!(
                "{}: Unable to find parent object {} for {}.",
                block.file_line(op_ip),
                lossy(parent),
                lossy(class_name)
            ));
            return Ok(());
        };
        let Some(source) = self.objects.get(found) else {
            return Ok(());
        };
        let texts = if source.class() == object.class() {
            let objects = &self.objects;
            source.static_texts_of(&objects.classes, &|id| {
                objects.get(id).and_then(|o| o.name().map(<[u8]>::to_vec))
            })
        } else {
            Vec::new()
        };
        let mut charge = |units: u64| self.charge_work(units);
        if object.copy_fields_from(source, &mut charge) {
            object.set_static_texts(&texts, &mut super::statics::VmEnv(self));
            Ok(())
        } else {
            Err(VmError::StepLimit)
        }
    }

    /// Whether `new` can make objects of the class (false for the console classes that are not SimObjects).
    #[inline(never)]
    fn is_sim_object_class(&self, class: ClassId) -> bool {
        self.objects
            .classes
            .get(class)
            .is_some_and(|c| c.sim_object)
    }

    /// At the level the console rules give, as the other two refusals of `new`.
    #[inline(never)]
    fn warn_non_sim_object(&mut self, site: (&CodeBlock, usize), class_name: &[u8]) {
        self.report_new_refusal(format!(
            "{}: Unable to instantiate non-SimObject class {}.",
            site.0.file_line(site.1),
            lossy(class_name)
        ));
    }

    /// The start of `datablock Class(name)`: an existing datablock of that name is declared again.
    /// Looked up in DataBlockGroup only (a newer object that took the global name does not hide it; a retiring one,
    /// [`Objects::is_retiring`](crate::sim::Objects::is_retiring), counts as absent),
    /// compared with the class ignoring case (`dStricmp`). The case is
    /// unobservable from script: identifiers reach the runtime in their first-interned spelling and
    /// `datablock (%expr)(Name)` is a syntax error, so only hand-made bytecode could differ. The same class: the
    /// declaration goes on with the existing object, whose fields the body updates, and its parent,
    /// constructor arguments and id are left alone (object creation is skipped then). Another
    /// class: the error, and the declaration fails. The check precedes the class lookup, so an unknown
    /// class that differs from the existing datablock's is "re-declared with a different class" too.
    #[inline(never)]
    fn find_declared_datablock(&mut self, class_name: &[u8], name: &[u8]) -> Declared {
        if name.is_empty() {
            return Declared::Absent;
        }
        let group = self.objects.datablock_group();
        let Some(existing) = self.objects.find_below(group, name) else {
            return Declared::Absent;
        };
        // A retiring datablock is not declared again: the declaration makes a new one of the name.
        if self.objects.is_retiring(existing) {
            return Declared::Absent;
        }
        let same_class = self.objects.get(existing).is_some_and(|o| {
            self.objects
                .classes
                .name(o.class())
                .eq_ignore_ascii_case(class_name)
        });
        if !same_class {
            self.error(format!(
                "Cannot re-declare data block {} with a different class.",
                lossy(name)
            ));
            return Declared::Refused;
        }
        if let Some(creating) = self.creating_mut() {
            creating.object = Some(NewObject::Live(existing));
        }
        Declared::Reused
    }

    /// `SimDataBlock::assignId` for a new object made by the `datablock` keyword: the id is taken now,
    /// even if the declaration fails later (a refused argument list, a registration the limit refuses),
    /// and only a datablock class takes one. Anything else prints the error and fails the declaration
    /// (false); the object was never built, so no id is spent.
    #[inline(never)]
    fn take_datablock_id(
        &mut self,
        (block, op_ip): (&CodeBlock, usize),
        object: &mut SimObject,
        class_name: &[u8],
    ) -> bool {
        if !self.objects.classes.is_a(object.class(), SIM_DATA_BLOCK) {
            self.report_new_refusal(format!(
                "{}: Unable to instantiate non-datablock class {}.",
                block.file_line(op_ip),
                lossy(class_name)
            ));
            return false;
        }
        object.set_id(self.objects.next_datablock_id());
        true
    }

    /// OP_ADD_OBJECT (`site`: the block and code index of the opcode). Registers the object being built
    /// (unless it already is), then places it: a top-level object (`place_at_root`) goes to the group
    /// `$instantGroup` names, or RootGroup, unless registration already put it in a group; a child goes into
    /// the group `target` gives (the id on the executor's integer stack: asked only then, and after the
    /// object's scripts ran), or into RootGroup and that set. The executor then puts the id where the
    /// declaration's result goes.
    ///
    /// This frame is on the recursion path of a script `onAdd` (this, `register_new`, `on_add`,
    /// `call_method`, `exec`), multiplied by the call depth: everything with a large local lives in a
    /// helper that has returned by the time the script runs.
    #[inline(never)]
    pub fn add_new_object(
        &mut self,
        site: (&CodeBlock, usize),
        place_at_root: bool,
        target: &dyn Fn() -> Result<u32, VmError>,
    ) -> Result<AddedObject, VmError> {
        let (fail, object) = self.take_new_object(site.1)?;
        let id = match object {
            NewObject::Live(id) => id,
            NewObject::Pending(object) => match self.register_new(object, site)? {
                Some(id) => id,
                None => return Ok(AddedObject::Failed(fail)),
            },
        };
        let added = self.after_add(id, place_at_root, site, (fail, target))?;
        if let AddedObject::Added(id) = added {
            self.note_registered(id, site.0.name());
        }
        Ok(added)
    }

    /// The rest of ADD_OBJECT once the object is registered (or re-declared): a datablock's `preload`, then
    /// `finish_add`, and sets the next instruction: `next` after the declaration, `fail` when the preload failed
    /// (the datablock is gone). Out of line, so `add_object`'s frame, which is on the `onAdd` recursion path,
    /// holds none of it.
    #[inline(never)]
    fn after_add(
        &mut self,
        id: ObjId,
        place_at_root: bool,
        site: (&CodeBlock, usize),
        (fail, target): (usize, &dyn Fn() -> Result<u32, VmError>),
    ) -> Result<AddedObject, VmError> {
        if !self.preload(id, site)? {
            return Ok(AddedObject::Failed(fail));
        }
        self.finish_add(id, place_at_root, target)?;
        Ok(AddedObject::Added(id))
    }

    /// A datablock's `preload` after ADD_OBJECT registered (or re-declared) it (as Torque3D's `compiledEval.cpp`):
    /// every behaviour of its class chain, the root's first, until one fails; false
    /// after a failure (see `preload_failed`). A script the preload ran that stops the run ends it here (see
    /// `abandon_preload`). On the recursion path of a behaviour that runs scripts: the failure's work is out of
    /// line.
    #[inline(never)]
    fn preload(&mut self, id: ObjId, site: (&CodeBlock, usize)) -> Result<bool, VmError> {
        let Some(class) = self.objects.get(id).map(SimObject::class) else {
            return Ok(true);
        };
        if !self.objects.classes.is_a(class, SIM_DATA_BLOCK) {
            return Ok(true);
        }
        let behaviours = self.objects.classes.behaviours(class);
        let mut index = 0;
        self.depth = self.depth.saturating_add(BEHAVIOUR_COST);
        let result = 'behaviours: {
            while let Some(behaviour) = behaviours.get(index) {
                match behaviour.preload(self, id) {
                    Ok(()) => index += 1,
                    Err(PreloadError::Vm(error)) => {
                        break 'behaviours Err(self.abandon_preload(id, error));
                    }
                    Err(PreloadError::Failed(why)) => {
                        break 'behaviours self
                            .preload_refused(id, Some(why), site)
                            .map(|()| false);
                    }
                    Err(PreloadError::Stale) => {
                        break 'behaviours self.preload_refused(id, None, site).map(|()| false);
                    }
                }
                // A script the behaviour ran may have deleted the object: nothing is left to preload.
                if !self.objects.contains(id) {
                    break;
                }
            }
            Ok(true)
        };
        self.depth = self.depth.saturating_sub(BEHAVIOUR_COST);
        result
    }

    /// A refused preload: `written` is the text it put in the error buffer (cut to the buffer's size), or none when it returned
    /// false without writing it (the line then shows what the buffer held). Out of line like the rest of the failure's work.
    #[inline(never)]
    fn preload_refused(
        &mut self,
        id: ObjId,
        written: Option<String>,
        site: (&CodeBlock, usize),
    ) -> Result<(), VmError> {
        if let Some(why) = written {
            let mut text = why.into_bytes();
            text.truncate(PRELOAD_ERROR_BYTES);
            self.preload_error = text.into_boxed_slice();
        }
        let why = lossy(&self.preload_error);
        self.preload_failed(id, &why, site)
    }

    /// A refused preload: prints `<file> (<line>): preload failed for <name>: <text>.` (error level; `(null)`
    /// for an unnamed datablock) and deletes the datablock. The deletion posts no `DeleteThisEvent`
    /// (the next `schedule` id is one more) but waits for a method that holds the datablock,
    /// as `deleteAll` does; the datablock's id stays spent and the links it made stay.
    #[inline(never)]
    fn preload_failed(
        &mut self,
        id: ObjId,
        why: &str,
        site: (&CodeBlock, usize),
    ) -> Result<(), VmError> {
        self.report_preload_failure(id, why, site);
        self.delete_refused(id)
    }

    /// The message of `preload_failed`, in a frame of its own that has returned before the deletion runs
    /// behaviours.
    #[inline(never)]
    fn report_preload_failure(
        &mut self,
        id: ObjId,
        why: &str,
        (block, op_ip): (&CodeBlock, usize),
    ) {
        let name = self
            .objects
            .get(id)
            .and_then(SimObject::name)
            .map_or_else(|| "(null)".to_string(), lossy);
        self.error(format!(
            "{}: preload failed for {name}: {why}.",
            block.file_line(op_ip)
        ));
    }

    /// The deletion of `preload_failed`, in a frame of its own.
    #[inline(never)]
    fn delete_refused(&mut self, id: ObjId) -> Result<(), VmError> {
        if !self.objects.contains(id) || self.objects.defer_delete(id) {
            return Ok(());
        }
        self.delete_now(id, None)
    }

    /// A script a datablock's preload ran stopped the run. A datablock that is in no group (made by `new`) must
    /// not stay registered, as after a script `onAdd`'s error (`abandon_registration`): it is taken out again
    /// (behaviours told, no script callbacks). One in DataBlockGroup (made by `datablock`, or declared again)
    /// stays, like the rest of the aborted run's work. Returns the error.
    #[cold]
    #[inline(never)]
    fn abandon_preload(&mut self, id: ObjId, error: VmError) -> VmError {
        if self.objects.get(id).is_some_and(|o| o.group().is_none()) {
            let _ = self.delete_now(id, Some(error.clone()));
        }
        error
    }

    /// `GuiControl::onAdd`'s link: a NAMED object links its name's namespace under its
    /// class's, and when the link holds the name is its namespace from then on (its methods are looked up from the
    /// name). A link the engine refuses (the name already under another parent) prints its error and leaves the object
    /// in its class's namespace; an unnamed object links nothing. For a host's behaviour (`EngineClass::on_add`);
    /// prints nothing else and runs no script.
    #[inline(never)]
    pub fn link_name_namespace(&mut self, id: ObjId) {
        self.link_name(id, false);
    }

    /// `TCPObject::onAdd`'s link: as [`link_name_namespace`](Self::link_name_namespace), but the name is the object's
    /// namespace whether or not the link held (it is set after `classLinkTo` without looking at the result; a refused
    /// link still prints its error).
    #[inline(never)]
    pub fn link_name_namespace_always(&mut self, id: ObjId) {
        self.link_name(id, true);
    }

    fn link_name(&mut self, id: ObjId, always: bool) {
        let Some(object) = self.objects.get(id) else {
            return;
        };
        let Some(name) = object.name().filter(|n| !n.is_empty()).map(<[u8]>::to_vec) else {
            return;
        };
        let class = self.objects.classes.name(object.class()).to_vec();
        self.objects
            .add_work((name.len() as u64).saturating_add(class.len() as u64));
        let linked = match self.namespaces.link_class(&name, &class) {
            Ok(()) => true,
            Err(conflict) => {
                self.error(conflict.message());
                false
            }
        };
        if (linked || always)
            && let Some(object) = self.objects.get_mut(id)
        {
            object.set_namespace(&name);
        }
    }

    /// `GameBaseData::onAdd`'s links (see `classes::GameBaseData`).
    #[inline(never)]
    pub(crate) fn link_datablock_namespaces(&mut self, id: ObjId) {
        let Some(object) = self.objects.get(id) else {
            return;
        };
        let classes = &self.objects.classes;
        let class = classes.name(object.class()).to_vec();
        let name = object.name().filter(|n| !n.is_empty()).map(<[u8]>::to_vec);
        let class_name = object.static_value(classes, b"className");
        // `className` is a shared text too (see `link_script_namespaces`).
        self.objects.add_work(
            (class_name.len() as u64).saturating_add(name.as_ref().map_or(0, Vec::len) as u64),
        );
        // A class that does not list GameBaseData's `className` has none to read or to set.
        let has_class_name = classes.static_field(object.class(), b"className").is_some();
        if let Some(name) = name {
            self.link_game_datablock(id, &name, &class, &class_name);
        }
        if has_class_name && class_name.is_empty() {
            self.write_field_quiet(Some(id), b"className", b"", &class);
        }
    }

    /// The links of a named game datablock: its name under its `className` under the class, or just under the
    /// class. The first the runtime refuses prints its error and ends the work, leaving the object's namespace
    /// as it is (the class's).
    #[inline(never)]
    fn link_game_datablock(&mut self, id: ObjId, name: &[u8], class: &[u8], class_name: &[u8]) {
        let linked = if !class_name.is_empty() && !class_name.eq_ignore_ascii_case(class) {
            self.link_or_report(class_name, class) && self.link_or_report(name, class_name)
        } else {
            self.link_or_report(name, class)
        };
        if linked && let Some(object) = self.objects.get_mut(id) {
            object.set_namespace(name);
        }
    }

    /// `Con::linkNamespaces`: false, after printing the error, when `child` already has another parent.
    #[inline(never)]
    fn link_or_report(&mut self, child: &[u8], parent: &[u8]) -> bool {
        match self.namespaces.link_class(child, parent) {
            Ok(()) => true,
            Err(conflict) => {
                self.error(conflict.message());
                false
            }
        }
    }

    /// Takes the object this frame is building out of `creating`: the place-at-root flag, the fail jump
    /// and the object. Nothing being built is hostile bytecode, or a failed inner `new` in a field value
    /// (TGE crashes on a NULL object here; `VmError::NoNewObject` documents it).
    #[inline(never)]
    fn take_new_object(&mut self, op_ip: usize) -> Result<(usize, NewObject), VmError> {
        let (object, fail) = match self.creating_mut() {
            Some(creating) => (creating.object.take(), creating.fail_jump),
            None => (None, 0),
        };
        match object {
            Some(object) => Ok((fail, object)),
            None => Err(VmError::NoNewObject { ip: op_ip }),
        }
    }

    /// The rest of ADD_OBJECT once the object is registered: it becomes the frame's object, is placed
    /// and its id goes on the integer stack.
    #[inline(never)]
    fn finish_add(
        &mut self,
        id: ObjId,
        place_at_root: bool,
        target: &dyn Fn() -> Result<u32, VmError>,
    ) -> Result<(), VmError> {
        if let Some(creating) = self.creating_mut() {
            creating.object = Some(NewObject::Live(id));
        }
        // An object its own onAdd deleted is not placed (TGE would use freed memory here).
        let placed = self
            .objects
            .get(id)
            .is_none_or(|o| place_at_root && o.group().is_some());
        if !placed {
            self.place(id, place_at_root, target)?;
        }
        Ok(())
    }

    /// Where ADD_OBJECT puts a new object (see `add_object`).
    fn place(
        &mut self,
        id: ObjId,
        place_at_root: bool,
        target: &dyn Fn() -> Result<u32, VmError>,
    ) -> Result<(), VmError> {
        let mut set = None;
        let group = if place_at_root {
            let instant = self
                .globals
                .get(b"$instantGroup")
                .map(crate::vars::Entry::get_string)
                .unwrap_or_default();
            self.objects
                .find(&instant)
                .filter(|g| self.objects.is_group(*g))
        } else {
            let target = target()?;
            if self.objects.is_group(target) {
                Some(target)
            } else {
                set = Some(target).filter(|s| self.objects.is_set(*s));
                None
            }
        };
        self.objects.add(group.unwrap_or(ROOT_GROUP_ID), id);
        if let Some(set) = set {
            self.objects.add(set, id);
        }
        // A nesting group's own `addObject` (the class behaviours'; none by default).
        if !place_at_root && let Some(g) = group {
            self.group_added(g, id)?;
        }
        Ok(())
    }

    /// [`EngineClass::on_add_object`] of every behaviour of `group`'s class chain, the root's first.
    #[inline(never)]
    fn group_added(&mut self, group: ObjId, child: ObjId) -> Result<(), VmError> {
        let Some(class) = self.objects.get(group).map(SimObject::class) else {
            return Ok(());
        };
        let behaviours = self.objects.classes.behaviours(class);
        if behaviours.is_empty() {
            return Ok(());
        }
        self.depth = self.depth.saturating_add(BEHAVIOUR_COST);
        let mut result = Ok(());
        for behaviour in behaviours.iter() {
            if !self.objects.contains(group) || !self.objects.contains(child) {
                break;
            }
            if let Err(e) = behaviour.on_add_object(self, group, child) {
                result = Err(e);
                break;
            }
        }
        self.depth = self.depth.saturating_sub(BEHAVIOUR_COST);
        result
    }

    /// `registerObject` for a new object, then its `onAdd`. On failure the object is taken
    /// out again and `Register object failed for object <name> of class <class>.` is printed; None.
    ///
    /// On the recursion path of a script `onAdd` (see `add_object`): the object enters the tables in
    /// `enter_tables`, which has returned before `on_add` runs.
    #[inline(never)]
    pub(crate) fn register_new(
        &mut self,
        object: Box<SimObject>,
        site: (&CodeBlock, usize),
    ) -> Result<Option<ObjId>, VmError> {
        let Some(id) = self.enter_tables(object, site)? else {
            return Ok(None);
        };
        // `SimObject::onAdd` is the first thing every class's `onAdd` does (`Parent::onAdd`): the allocation policy
        // counts the object or refuses it before the class does any of its work. A refusal prints nothing (no
        // `Register object failed` line).
        if !self.alloc_admit(id) {
            return self.refuse_registration(id, site, false);
        }
        match self.on_add(id) {
            Ok(true) => Ok(Some(id)),
            Ok(false) => self.refuse_registration(id, site, true),
            Err(error) => Err(self.abandon_registration(id, error)),
        }
    }

    /// `Objects::register` for a new object: its id, name and place in the tables. None (with the
    /// warning) when it cannot be registered. The object stays boxed until here so that the frames
    /// of `add_object` and `register_new` (on the recursion path of a script `onAdd`) never hold the
    /// whole `SimObject`.
    #[inline(never)]
    #[allow(clippy::boxed_local)]
    fn enter_tables(
        &mut self,
        object: Box<SimObject>,
        site: (&CodeBlock, usize),
    ) -> Result<Option<ObjId>, VmError> {
        // `SimDataBlock::onAdd` fails once the datablock id counter has passed the limit (the counter is
        // tested, so `new SimDataBlock` fails too). Checked before the object
        // enters the tables, so a failing datablock never shares an id with a live
        // object; it is just not findable while the handler runs.
        if self.objects.classes.is_a(object.class(), SIM_DATA_BLOCK)
            && self.objects.datablocks_exhausted()
        {
            self.datablock_limit_exceeded(&object, site)?;
            return Ok(None);
        }
        Ok(self.register_in_tables(object, site))
    }

    /// The registration proper, in a frame of its own: `enter_tables` is live while a handler of the
    /// datablock limit runs scripts, this is not.
    #[inline(never)]
    #[allow(clippy::boxed_local)]
    fn register_in_tables(
        &mut self,
        object: Box<SimObject>,
        site: (&CodeBlock, usize),
    ) -> Option<ObjId> {
        let shown_name = object.name().map_or_else(|| "(null)".to_string(), lossy);
        let shown_class = lossy(self.objects.classes.name(object.class()));
        let Some(registered) = self.objects.register(*object) else {
            self.warn_register_failed(site, &shown_name, &shown_class);
            return None;
        };
        if registered.wrapped
            && let Some(line) = self.messages().ids_wrapped()
        {
            self.print(line.as_bytes());
        }
        Some(registered.id)
    }

    /// The datablock limit: the error, the console rules' callback (`ConsoleRules::datablock_limit_callback`) and the failed
    /// registration's warning. The handler runs a script, so this frame is on the recursion path of a
    /// handler that declares datablocks itself: it is out of line and does nothing but call helpers
    /// that have returned by the time the handler runs (stack: see the `Vm` docs).
    #[cold]
    #[inline(never)]
    fn datablock_limit_exceeded(
        &mut self,
        object: &SimObject,
        site: (&CodeBlock, usize),
    ) -> Result<(), VmError> {
        self.report_datablock_limit();
        if let Some(callback) = self.console_rules().datablock_limit_callback {
            self.evaluate_call(callback.as_bytes())?;
        }
        self.warn_failed_object(object, site);
        Ok(())
    }

    #[cold]
    #[inline(never)]
    fn report_datablock_limit(&mut self) {
        let max = self.limits().max_datablocks();
        self.error(format!(
            "Error: SimDataBlock::onAdd() - Exceeded maximum number of data blocks ({max})"
        ));
    }

    /// A line `new` prints when it cannot make an object, at the level the console rules give.
    #[cold]
    #[inline(never)]
    fn report_new_refusal(&mut self, text: String) {
        let level = self.console_rules().new_refusals;
        self.log_bytes(level, text.as_bytes());
    }

    /// `Register object failed ...` for an object that never entered the tables.
    #[cold]
    #[inline(never)]
    fn warn_failed_object(&mut self, object: &SimObject, site: (&CodeBlock, usize)) {
        let name = object.name().map_or_else(|| "(null)".to_string(), lossy);
        let class = lossy(self.objects.classes.name(object.class()));
        self.warn_register_failed(site, &name, &class);
    }

    /// What `Con::evaluate("name();")` does: calls the global function `name`, or warns as a missing
    /// function does in evaluated text (`<input> (0)`, then a BackTrace). It goes through `invoke`, so a host
    /// native of that name with the wrong argument count prints the Rust form `(null)::name - ...`; a compiled
    /// script call would print `<input> (0): ::name - ...`.
    #[inline(never)]
    fn evaluate_call(&mut self, name: &[u8]) -> Result<(), VmError> {
        if self.namespaces.lookup(b"", name).is_some() {
            self.invoke(None, name, &[]).map(|_| ())
        } else {
            self.warn_missing_function(name)
        }
    }

    #[cold]
    #[inline(never)]
    fn warn_missing_function(&mut self, name: &[u8]) -> Result<(), VmError> {
        self.warn(format!(
            "<input> (0): Unable to find function {}",
            lossy(name)
        ));
        self.backtrace_after_message()
    }

    /// `onAdd` refused the object: the class `on_remove` hook and every behaviour's `on_remove` run (leaf first),
    /// the object leaves the tables and the warning names it as it is now (printed after
    /// `unregisterObject`, which keeps the name). A behaviour's removal error is returned after all that.
    #[cold]
    #[inline(never)]
    fn refuse_registration(
        &mut self,
        id: ObjId,
        site: (&CodeBlock, usize),
        say: bool,
    ) -> Result<Option<ObjId>, VmError> {
        // A failed `registerObject` calls `unregisterObject`, which runs `onRemove` at every level of
        // the class chain, also at levels whose `onAdd` never ran; for a class below ScriptObject that includes
        // the script `onRemove` (a ScriptObject the allocation policy refuses is the case).
        let removed = self.refused_removal(id);
        self.finish_refusal(id, site, removed, say)
    }

    /// The hook and behaviours of `refuse_registration`, run while the object is still registered.
    #[cold]
    #[inline(never)]
    fn refused_removal(&mut self, id: ObjId) -> Result<(), VmError> {
        // (a ScriptObject or ScriptGroup refused by `SimObject::onAdd`, the allocation policy, never ran its script
        // `onAdd` and still runs its script `onRemove`: `ScriptObject::onRemove` is what `unregisterObject` calls)
        self.on_remove(id, true)
    }

    /// The end of `refuse_registration`: the object leaves the tables and the warning names it as it was.
    #[cold]
    #[inline(never)]
    fn finish_refusal(
        &mut self,
        id: ObjId,
        site: (&CodeBlock, usize),
        removed: Result<(), VmError>,
        say: bool,
    ) -> Result<Option<ObjId>, VmError> {
        let (name, class) = match self.objects.get(id) {
            Some(object) => (
                object.name().map_or_else(|| "(null)".to_string(), lossy),
                lossy(self.objects.classes.name(object.class())),
            ),
            None => ("(null)".to_string(), String::new()),
        };
        self.objects.unregister(id);
        if say {
            self.warn_register_failed(site, &name, &class);
        }
        removed.map(|()| None)
    }

    /// A script `onAdd` stopped with a run-level error (step limit, call depth, a failing native): the
    /// whole run is being aborted, and the object it was adding must not stay registered but in no
    /// group. TGE has no errors here (it hangs or crashes where this stops), so there is
    /// nothing to copy: the object is unregistered, like a refused registration but without the warning
    /// and without the class `on_remove` hook, because the class `on_add` hook (which runs after the
    /// script `onAdd`) never ran. Whatever the aborted `onAdd` did to other objects stays. Returns the
    /// error to propagate.
    #[cold]
    #[inline(never)]
    fn abandon_registration(&mut self, id: ObjId, error: VmError) -> VmError {
        // what the allocation policy took at the head of `on_add` goes back (the count, the reference, the membership)
        self.alloc_released(id);
        self.objects.unregister(id);
        error
    }

    #[cold]
    #[inline(never)]
    fn warn_register_failed(
        &mut self,
        (block, op_ip): (&CodeBlock, usize),
        name: &str,
        class: &str,
    ) {
        self.warn(format!(
            "{}: Register object failed for object {name} of class {class}.",
            block.file_line(op_ip)
        ));
    }

    /// `SimObject::onAdd` and the class overrides, for an object just entered in the tables. False
    /// fails the registration. A ScriptObject or ScriptGroup first links its namespaces and calls its
    /// script `onAdd`; a datablock joins DataBlockGroup; the class hook and the class chain's behaviours (a Rust
    /// class's `onAdd`, see `class_on_add`) stay last.
    pub(crate) fn on_add(&mut self, id: ObjId) -> Result<bool, VmError> {
        let Some(class) = self.objects.get(id).map(SimObject::class) else {
            return Ok(true);
        };
        let classes = &self.objects.classes;
        if classes.is_a(class, SCRIPT_OBJECT) || classes.is_a(class, SCRIPT_GROUP) {
            self.link_script_namespaces(id);
            // `%this.onAdd()`: silent when there is none.
            self.call_method(id, b"onAdd", &[])?;
        }
        self.join_datablock_group(id, class);
        self.class_on_add(id, class)
    }

    /// The class `on_add` hook, then the behaviours of the class chain, the root's first
    /// (`EngineClass::on_add`): false from any of them fails the registration (`refuse_registration` tells every
    /// behaviour); an error from one tells the behaviours that had added the object before it that it left, and
    /// is returned. Out of line: `on_add` is on the `onAdd` recursion path, and a behaviour may run scripts.
    #[inline(never)]
    fn class_on_add(&mut self, id: ObjId, class: ClassId) -> Result<bool, VmError> {
        if !self.call_add_hook(id, class) {
            return Ok(false);
        }
        let behaviours = self.objects.classes.behaviours(class);
        let mut index = 0;
        self.depth = self.depth.saturating_add(BEHAVIOUR_COST);
        let result = 'behaviours: {
            while let Some(behaviour) = behaviours.get(index) {
                match behaviour.on_add(self, id) {
                    Ok(true) => index += 1,
                    Ok(false) => break 'behaviours Ok(false),
                    Err(error) => {
                        break 'behaviours Err(self.release_added(id, &behaviours[..index], error));
                    }
                }
            }
            Ok(true)
        };
        self.depth = self.depth.saturating_sub(BEHAVIOUR_COST);
        result
    }

    /// The class `on_add` hook (true when there is none): in a frame of its own, which has returned before the
    /// behaviours run.
    #[inline(never)]
    fn call_add_hook(&mut self, id: ObjId, class: ClassId) -> bool {
        match self.objects.classes.on_add_hook(class) {
            Some(hook) => hook(self, id),
            None => true,
        }
    }

    /// A behaviour's `on_add` failed with a run-level error: the behaviours before it (the root's first in the
    /// list) that had added the object are told it left, leaf first, so the Rust state they made is released;
    /// their own errors are dropped (the first one is on its way out). Returns `error`.
    #[cold]
    #[inline(never)]
    fn release_added(
        &mut self,
        id: ObjId,
        added: &[Rc<dyn EngineClass>],
        error: VmError,
    ) -> VmError {
        self.depth = self.depth.saturating_add(BEHAVIOUR_COST);
        for behaviour in added.iter().rev() {
            let _ = behaviour.on_remove(self, id);
        }
        self.depth = self.depth.saturating_sub(BEHAVIOUR_COST);
        error
    }

    /// What a script's `new <class>() { <field> = <value>; ... };` does with no name, no parent and no arguments, as a
    /// statement whose result is kept (ADD_OBJECT 1: the new object goes into `$instantGroup`'s group, else RootGroup, unless
    /// its `onAdd` placed it): the object made with its constructors' state, each field written as the body's `SAVEFIELD_STR`
    /// writes it (through its type and validator, to the object not yet registered), then registered, its `onAdd` run (scripts
    /// may run), a datablock preloaded. `site` is the code block and the index of the declaration's ADD_OBJECT opcode, which
    /// the messages of a refused registration name. `Ok(None)` when the class is unknown or not a `SimObject`'s (no message:
    /// a host asks only for its own classes) or the object is refused (the declaration's 0).
    ///
    /// For a host doing a script's work natively, so that the object is the one the
    /// script would make. Charged as the declaration is (the slots copied; the fields' work as `setDataField`'s).
    #[inline(never)]
    pub fn declare_new_object(
        &mut self,
        site: (&CodeBlock, usize),
        class_name: &[u8],
        fields: &[(&[u8], &[u8])],
    ) -> Result<Option<ObjId>, VmError> {
        let Some(class) = self.objects.classes.find(class_name) else {
            return Ok(None);
        };
        if !self.is_sim_object_class(class) {
            return Ok(None);
        }
        let mut object = SimObject::new(class, &self.objects.classes);
        self.objects.add_work(object.slot_count() as u64);
        for behaviour in self.objects.classes.behaviours(class) {
            behaviour.on_construct(&self.objects, &mut object);
        }
        self.creating.push(Creating {
            object: Some(NewObject::Pending(Box::new(object))),
            fail_jump: 0,
        });
        for (name, value) in fields {
            self.write_field(None, name, b"", value);
        }
        let Some(Creating {
            object: Some(NewObject::Pending(object)),
            ..
        }) = self.creating.pop()
        else {
            return Ok(None);
        };
        let Some(id) = self.register_new(object, site)? else {
            return Ok(None);
        };
        if !self.preload(id, site)? {
            return Ok(None);
        }
        // `finish_add` with ADD_OBJECT 1: placed unless its `onAdd` deleted it or put it in a group.
        let placed = self.objects.get(id).is_none_or(|o| o.group().is_some());
        if !placed {
            self.place(id, true, &|| Err(VmError::StackUnderflow))?;
        }
        Ok(Some(id))
    }

    /// [`declare_new_object`](Self::declare_new_object) for a host with no script of its own to name: the object a script's
    /// `new <class>() { <field> = <value>; ... };` makes, with its `onAdd` and its placement, from Rust and with no text
    /// compiled (a field's value is only written through its type, never run). Messages of a refused registration name
    /// evaluated text.
    pub fn new_object(
        &mut self,
        class_name: &[u8],
        fields: &[(&[u8], &[u8])],
    ) -> Result<Option<ObjId>, VmError> {
        let block = CodeBlock::native();
        self.declare_new_object((&block, 0), class_name, fields)
    }

    /// An object the host makes itself with no `onAdd` of its class (a networked copy of an object, say: the host
    /// constructs the class's object and registers it; the class's `onAdd` there is the host's own, which runs no
    /// script and which the classes, all server halves, do not have). An object of `class_name` with a new dynamic id,
    /// the constructors' state and `fields` (`(name, array index, text)`, through each field's type), marked
    /// [`SimObject::engine_made`] so a script reads its fields as "". `None` for an unknown class or when no id is
    /// free. Charged by its slots.
    pub fn add_engine_object(
        &mut self,
        class_name: &[u8],
        fields: &[(&[u8], &[u8], &[u8])],
    ) -> Option<ObjId> {
        let class = self.objects.classes.find(class_name)?;
        let mut object = SimObject::new(class, &self.objects.classes);
        self.objects.add_work(object.slot_count() as u64);
        for behaviour in self.objects.classes.behaviours(class) {
            behaviour.on_construct(&self.objects, &mut object);
        }
        let id = self.objects.register(object)?.id;
        for (name, index, value) in fields {
            self.set_object_member(id, name, index, value);
        }
        if let Some(object) = self.objects.get_mut(id) {
            object.engine_made = true;
        }
        Some(id)
    }

    /// A datablock the host makes outside script (one it was sent, say): an object of `class_name` with the id `id`,
    /// the constructors' state, the fields the unpack wrote (`(name, array index, text)`, each through its field's
    /// type), registered (`onAdd`: it joins DataBlockGroup, the class behaviours see it), then preloaded
    /// (`preload(false)`, every behaviour of its chain). `Ok(None)` when it is not made: no such class or not a
    /// datablock class, an id outside the datablock range or taken, a refused `onAdd` (the object is gone), or a failed
    /// preload (the datablock stays; what follows is the caller's). No script names it: such datablocks have no names.
    /// Charged by its slots as a script declaration is.
    pub fn add_engine_datablock(
        &mut self,
        class_name: &[u8],
        id: ObjId,
        fields: &[(Vec<u8>, Vec<u8>, Vec<u8>)],
    ) -> Result<Option<ObjId>, VmError> {
        let Some(class) = self.objects.classes.find(class_name) else {
            return Ok(None);
        };
        if !self.objects.classes.is_a(class, SIM_DATA_BLOCK)
            || !self.objects.is_datablock_id(id)
            || self.objects.contains(id)
        {
            return Ok(None);
        }
        let mut object = SimObject::new(class, &self.objects.classes);
        self.objects.add_work(object.slot_count() as u64);
        for behaviour in self.objects.classes.behaviours(class) {
            behaviour.on_construct(&self.objects, &mut object);
        }
        object.set_id(id);
        let Some(registered) = self.objects.register(object) else {
            return Ok(None);
        };
        let id = registered.id;
        for (name, index, value) in fields {
            self.set_object_member(id, name, index, value);
        }
        match self.on_add(id) {
            Ok(true) => {}
            Ok(false) => {
                let _ = self.delete_object_quiet(id);
                return Ok(None);
            }
            Err(e) => return Err(e),
        }
        if !self.objects.contains(id) {
            return Ok(None);
        }
        Ok(self.preload_engine_datablock(id)?.then_some(id))
    }

    /// `preload(false)` of a datablock the host made or updated ([`Vm::add_engine_datablock`]): every behaviour of its
    /// class chain, the root's first, until one fails. False when one failed or the object is gone; a run-level error
    /// is returned.
    pub fn preload_engine_datablock(&mut self, id: ObjId) -> Result<bool, VmError> {
        let Some(class) = self.objects.get(id).map(SimObject::class) else {
            return Ok(false);
        };
        let behaviours = self.objects.classes.behaviours(class);
        self.depth = self.depth.saturating_add(BEHAVIOUR_COST);
        let mut loaded = true;
        for behaviour in behaviours.iter() {
            match behaviour.preload(self, id) {
                Ok(()) => {}
                Err(PreloadError::Vm(error)) => {
                    self.depth = self.depth.saturating_sub(BEHAVIOUR_COST);
                    return Err(error);
                }
                Err(_) => {
                    loaded = false;
                    break;
                }
            }
            if !self.objects.contains(id) {
                loaded = false;
                break;
            }
        }
        self.depth = self.depth.saturating_sub(BEHAVIOUR_COST);
        Ok(loaded)
    }

    /// `SimDataBlock::onAdd`: an object with a datablock id (3 up to the limit) joins DataBlockGroup, so
    /// a class hook sees it there and placement leaves it alone. `new SimDataBlock` has a dynamic id
    /// and goes where `new` puts any object. When DataBlockGroup itself has been deleted the
    /// add finds no group and the object is placed like any other (see `Objects::datablock_group`).
    #[inline(never)]
    fn join_datablock_group(&mut self, id: ObjId, class: ClassId) {
        if self.objects.classes.is_a(class, SIM_DATA_BLOCK) && self.objects.is_datablock_id(id) {
            let group = self.objects.datablock_group();
            self.objects.add(group, id);
        }
    }

    /// `ScriptObject::onAdd` / `ScriptGroup::onAdd`: links `superClass` under ScriptObject (or
    /// ScriptGroup), `class` under that, and the object's name under that; the last link that held is
    /// the object's namespace. A refused link prints its error and is skipped.
    #[inline(never)]
    fn link_script_namespaces(&mut self, id: ObjId) {
        let Some(object) = self.objects.get(id) else {
            return;
        };
        // The object's own class: ScriptObject or ScriptGroup, or a Rust class registered below one of
        // them, whose namespace chain then keeps its name. Deliberately unlike TGE, which hard-codes
        // "ScriptObject" / "ScriptGroup" as the base, so for an engine class derived
        // from ScriptObject its own namespace is not in the object's chain; a host that registers such
        // classes should know.
        let classes = &self.objects.classes;
        let base = classes.name(object.class()).to_vec();
        let links = [
            object.static_value(classes, b"superClass"),
            object.static_value(classes, b"class"),
            object.name().unwrap_or_default().to_vec(),
        ];
        // The class names are shared texts: a `: Parent` copy hands them over as handles, and every object then
        // copies and folds them here. Charged by their bytes.
        self.objects.add_work(
            links
                .iter()
                .fold(0u64, |sum, l| sum.saturating_add(l.len() as u64)),
        );
        let mut parent = base;
        for child in links {
            if child.is_empty() {
                continue;
            }
            match self.namespaces.link_class(&child, &parent) {
                Ok(()) => parent = child,
                Err(conflict) => self.error(conflict.message()),
            }
        }
        if let Some(object) = self.objects.get_mut(id) {
            object.set_namespace(&parent);
        }
    }

    /// The field `field[array]` of `target` as text ("" without an object), as `LOADFIELD` reads it. The copy is
    /// charged to the step budget as it is made (`SimObject::data_field_charged`: a text or dynamic value by its
    /// length, a list by the bytes it formats), so reading a big field over and over meets the step limit however the
    /// result is used. Out of line: it runs no script, and the interpreter's frame keeps none of it.
    #[inline(never)]
    pub fn read_field_of(&self, target: FieldTarget, field: &[u8], array: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        self.read_field_into(target, field, array, &mut out);
        out
    }

    /// [`read_field_of`](Self::read_field_of) appending the text to `out`, a buffer the caller keeps between reads.
    #[inline(never)]
    pub fn read_field_into(
        &self,
        target: FieldTarget,
        field: &[u8],
        array: &[u8],
        out: &mut Vec<u8>,
    ) {
        let object = match target {
            FieldTarget::None => None,
            FieldTarget::Live(id) => self.objects.get(id),
            FieldTarget::New => match self.creating.last().and_then(|c| c.object.as_ref()) {
                Some(NewObject::Pending(object)) => Some(&**object),
                Some(NewObject::Live(id)) => self.objects.get(*id),
                None => None,
            },
        };
        let Some(object) = object else {
            return;
        };
        if object.engine_made {
            return;
        }
        let objects = &self.objects;
        let names = |id: ObjId| objects.get(id).and_then(|o| o.name().map(<[u8]>::to_vec));
        let mut charge = |units: u64| self.charge_work(units);
        object.data_field_charged_into(&objects.classes, field, array, &names, &mut charge, out);
    }

    /// The field `field[array]` of `target` set to `value` as `SAVEFIELD_STR` does: through the field's type and
    /// validator; nothing for no object or an engine-made one.
    pub fn write_field_of(
        &mut self,
        target: FieldTarget,
        field: &[u8],
        array: &[u8],
        value: &[u8],
    ) {
        let target = match target {
            FieldTarget::None => return,
            FieldTarget::Live(id) => Some(id),
            FieldTarget::New => match self.creating.last().and_then(|c| c.object.as_ref()) {
                Some(NewObject::Pending(_)) => None,
                Some(NewObject::Live(id)) => Some(*id),
                None => return,
            },
        };
        if target.is_some_and(|id| self.objects.get(id).is_some_and(|o| o.engine_made)) {
            return;
        }
        self.write_field(target, field, array, value);
    }
}

/// One step of a deletion (see [`Vm::delete_object`]).
enum Removal {
    /// Run the object's `onRemove`, then queue a group's members. `Some(group)` says the object is a
    /// member of that group, which is being deleted: it is skipped when it is no longer in it.
    Begin(ObjId, Option<ObjId>),
    /// Take the object out of the tables, once a group's members are gone.
    End(ObjId),
}

/// One set `Vm::delete_all` is working through.
struct DeleteAll {
    set: ObjId,
    /// Members at the front that are passed over because a running method holds them.
    held: usize,
    /// The recursion into the last member has been done for this round.
    recursed: bool,
}

/// The sets `Vm::delete_all` is inside (innermost last) and the same ids as a lookup.
struct AllState {
    stack: Vec<DeleteAll>,
    active: std::collections::HashSet<ObjId>,
}

impl AllState {
    #[inline(never)]
    fn new(root: ObjId) -> AllState {
        AllState {
            stack: vec![DeleteAll {
                set: root,
                held: 0,
                recursed: false,
            }],
            active: std::collections::HashSet::from([root]),
        }
    }
}

/// What `Vm::delete_all` does next.
enum AllStep {
    /// Every set is done.
    Done,
    /// The last member (a set) was entered: its `on_enter` method runs now.
    Entered(ObjId),
    /// Delete this member of the set being emptied.
    Delete(ObjId),
    /// The step budget is spent (see `Vm::ensure_budget`).
    OutOfBudget,
}

impl Vm {
    /// Holds `object` as a running script method does (TGE does it for a method call into a script function):
    /// until the matching release,
    /// `delete` only records the delete. `call_function` uses it on every such call; host code that
    /// calls into an object can too. Out of line: it is
    /// on the call recursion path.
    #[inline(never)]
    pub fn hold(&mut self, object: ObjId) {
        self.objects.lock(object);
    }

    /// Drops a hold taken with [`hold`](Self::hold) (see `release_after`).
    pub fn release(&mut self, object: ObjId) -> Result<(), VmError> {
        self.release_after(object, Ok(()))
    }

    /// The end of a held call: drops the hold and, when it was the
    /// last one and a delete was asked for meanwhile, deletes the object now, before the caller goes on.
    /// The callbacks run where the method's result is, so theirs replaces
    /// it (see `on_remove`). `result` is the outcome of the call: after an error the
    /// object is still deleted but no script callback runs, and that error is returned (TGE has
    /// no such errors; leaving the delete pending would keep the object forever).
    #[inline(never)]
    pub fn release_after(
        &mut self,
        object: ObjId,
        result: Result<(), VmError>,
    ) -> Result<(), VmError> {
        if !self.objects.unlock(object) {
            return result;
        }
        self.post_delete_event(object);
        self.delete_now(object, result.err())
    }

    /// Deletes an object as `deleteObject` does. While a script method of the object is
    /// running (it is held) the delete is only recorded: the object stays registered, named, in its group
    /// and sets, with its fields, and further deletes change nothing; the release that drops the last
    /// hold carries it out. Otherwise it happens now (`delete_now`).
    pub fn delete_object(&mut self, id: ObjId) -> Result<(), VmError> {
        if self.objects.defer_delete(id) {
            return Ok(());
        }
        self.post_delete_event(id);
        self.delete_now(id, None)
    }

    /// Deletes an object the way a C++ object destroyed directly is deleted (its own `delete`, a host's delete events):
    /// the Deleted flag, `unregisterObject` and the destructor at once, without
    /// `SimObject::deleteObject`'s `DeleteThisEvent`, so no sequence number is spent and nothing more is pending afterwards.
    /// A method of the object that is running holds the delete back, as for [`delete_object`](Self::delete_object)
    /// (whose release then posts the event, as `deleteObject` does when the method returns).
    pub fn delete_object_quiet(&mut self, id: ObjId) -> Result<(), VmError> {
        if self.objects.defer_delete(id) {
            return Ok(());
        }
        self.delete_now(id, None)
    }

    /// `deleteObject`'s `DeleteThisEvent`: queued at the next millisecond on RootGroup. It
    /// takes an event id and counts as pending; the object is freed at once, so it does
    /// nothing when it fires. Only `deleteObject` posts one: not a group's cascade, not `deleteAll`. An
    /// object that does not exist, or is already being removed (deleting it again from its own
    /// `onRemove`: the Deleted flag makes that a no-op), posts none. Out of line: on the
    /// recursion path of an `onRemove` that deletes again.
    #[inline(never)]
    fn post_delete_event(&mut self, id: ObjId) {
        if self.objects.get(id).is_some_and(|o| !o.is_removing()) {
            let time = self.events.now().wrapping_add(1);
            // A full queue (a script flooding it) simply posts nothing.
            self.events
                .post(time, ROOT_GROUP_ID, crate::events::EventKind::Delete);
        }
    }

    /// The deletion itself: the object's `onRemove` runs first, then a group's members are deleted by
    /// descending id, depth first, each with its own `onRemove`, and only then is the object taken out
    /// of its group, its sets and the tables (the group's own `onRemove` first, then its members' by
    /// descending id; the group's count stays constant meanwhile, and each member is unlinked from the
    /// group just before its `onRemove`). The group's member list is sorted by id after its own
    /// `onRemove` and before the members'. Members are removed whether or not something
    /// holds them, as `SimGroup::onRemove` unregisters them directly.
    ///
    /// The cascade is a loop, not recursion; callbacks run through `call_method`, so they are charged
    /// against the call depth. Where TGE crashes, hangs or walks freed memory:
    /// - a group that holds itself (TGE recurses until the stack overflows) or a group cycle
    ///   is deleted once;
    /// - a member an earlier member's `onRemove` moved to another group is not deleted, as in TGE, but
    ///   TGE also runs the earlier member's `onRemove` a second time, because it walks the live list by
    ///   pointer and the swap-removal of the moved member puts that member's entry where the walk goes
    ///   next; this runs it once.
    ///
    /// If a callback fails, the rest is still taken out of the tables (without script callbacks, but the
    /// class `on_remove` hook of every object still runs, so Rust state is released) and the first error
    /// is returned. `failure` is an error already on its way out (`release_after`): then no script
    /// callback runs at all.
    pub(crate) fn delete_now(
        &mut self,
        id: ObjId,
        failure: Option<VmError>,
    ) -> Result<(), VmError> {
        // On the recursion path of an `onRemove` that deletes again: the bookkeeping lives in out-of-line
        // helpers that have returned before the script runs, so this frame stays small.
        let mut work = vec![Removal::Begin(id, None)];
        let mut failure = failure;
        while let Some(step) = work.pop() {
            match step {
                Removal::Begin(id, from) => {
                    if !self.begin_step(id, from) {
                        continue;
                    }
                    if let Err(e) = self.on_remove(id, failure.is_none())
                        && failure.is_none()
                    {
                        failure = Some(e);
                    }
                    self.queue_members(id, &mut work);
                }
                Removal::End(id) => self.end_step(id, &mut work),
            }
        }
        failure.map_or(Ok(()), Err)
    }

    /// The start of a removal: false when the object is not to be removed now (it left the group that is
    /// being deleted; it is gone; it is already being removed).
    #[inline(never)]
    fn begin_step(&mut self, id: ObjId, from: Option<ObjId>) -> bool {
        if let Some(group) = from
            && self.objects.get(id).and_then(SimObject::group) != Some(group)
        {
            return false;
        }
        if !self.objects.begin_removal(id) {
            return false;
        }
        if from.is_some() {
            self.objects.unlink_from_group(id);
        }
        true
    }

    /// After an object's `onRemove`: its `End` step, and for a group its members, which come off the stack
    /// by descending id.
    #[inline(never)]
    fn queue_members(&mut self, id: ObjId, work: &mut Vec<Removal>) {
        work.push(Removal::End(id));
        if self.objects.is_group(id) {
            // `SimGroup::onRemove` sorts its list after the group's own script `onRemove`.
            self.objects.sort_members(id);
            // Ascending, so the highest id is taken off the stack first.
            let members = self.objects.live_members_by_id(id);
            work.extend(members.into_iter().map(|m| Removal::Begin(m, Some(id))));
        }
    }

    /// The end of a removal: members that arrived meanwhile are removed first, then the object leaves the
    /// tables.
    #[inline(never)]
    fn end_step(&mut self, id: ObjId, work: &mut Vec<Removal>) {
        let late = if self.objects.is_group(id) {
            self.objects.live_members_by_id(id)
        } else {
            Vec::new()
        };
        if late.is_empty() {
            self.objects.unregister(id);
        } else {
            work.push(Removal::End(id));
            work.extend(late.into_iter().map(|m| Removal::Begin(m, Some(id))));
        }
    }

    /// `onRemove`: the script callback of a ScriptObject or ScriptGroup (silent when there is none, and
    /// skipped when `script` is false), then the class hook and the behaviours (`class_on_remove`), which run
    /// whatever the script did.
    fn on_remove(&mut self, id: ObjId, script: bool) -> Result<(), VmError> {
        let Some(class) = self.objects.get(id).map(SimObject::class) else {
            return Ok(());
        };
        let classes = &self.objects.classes;
        let mut result = Ok(());
        if script && (classes.is_a(class, SCRIPT_OBJECT) || classes.is_a(class, SCRIPT_GROUP)) {
            // `Con::executef(this, "onRemove", scriptThis())`: `scriptThis()` writes the id text where
            // results go, then the script `onRemove` (if any) leaves its own result there. Only a
            // deferred delete shows it, because that slot then holds the result of the method that just
            // returned (an id such as "8313" without a script onRemove). A `delete` or `deleteAll`
            // native sets its own (empty) result afterwards.
            self.executor
                .set_result(crate::text::format_int(id).as_bytes());
            result = self.call_method(id, b"onRemove", &[]).map(|_| ());
        }
        self.class_on_remove(id, class, result)
    }

    /// The class `on_remove` hook, then the behaviours of the class chain, the leaf's first
    /// (`EngineClass::on_remove`); `result` is the script `onRemove`'s, which wins over a behaviour's error
    /// (the first error is the one returned; the object leaves the tables whatever happens). Out of line:
    /// `on_remove` is on the `onRemove` recursion path.
    #[inline(never)]
    fn class_on_remove(
        &mut self,
        id: ObjId,
        class: ClassId,
        mut result: Result<(), VmError>,
    ) -> Result<(), VmError> {
        self.call_remove_hook(id, class);
        let mut behaviours = self.objects.classes.behaviours(class);
        // The list is the root's first: popping takes the leaf's first.
        self.depth = self.depth.saturating_add(BEHAVIOUR_COST);
        while let Some(behaviour) = behaviours.pop() {
            let removed = behaviour.on_remove(self, id);
            if result.is_ok() {
                result = removed;
            }
        }
        self.depth = self.depth.saturating_sub(BEHAVIOUR_COST);
        // `SimObject::onRemove` is the root of every class's, so it comes last: the object gives its count back.
        self.alloc_released(id);
        result
    }

    /// The class `on_remove` hook, in a frame of its own (see `call_add_hook`).
    #[inline(never)]
    fn call_remove_hook(&mut self, id: ObjId, class: ClassId) {
        if let Some(hook) = self.objects.classes.on_remove_hook(class) {
            hook(self, id);
        }
    }

    /// Deletes every member of the set `root` (a game's `deleteAll` natives): the set's `on_enter` method runs first,
    /// when given (a script call, silent when there is none, no hold). Then, while the set has members: if its LAST
    /// member is a set (of any kind), that member is emptied the same way first (so the members of a nested plain set
    /// are deleted, and `on_enter` runs for it too); then the member at index 0 is deleted
    /// without the `delete` method's datablock guard, with no unlinking before its `onRemove`
    /// (unlike the group cascade). The recursion into the last member is an explicit stack, never Rust
    /// recursion; every round is charged.
    ///
    /// Unlike TGE: a member a running method holds is passed over and stays in the set (its delete is
    /// carried out when that method returns), where TGE spins forever on it; a member that
    /// is already being removed (`deleteAll` from its own `onRemove`) is dropped from the list, where
    /// TGE spins forever; a set that holds itself, or an enclosing set, is not entered again, where TGE
    /// recurses until the stack overflows.
    ///
    /// A native does not return to the interpreter while it loops, so the loop checks the step budget
    /// itself in every round (`delete_all_step`, `ensure_budget`): a set that never empties fails with
    /// `StepLimit` instead of hanging.
    ///
    /// This frame is on the recursion path of an `onRemove` that calls `deleteAll` again: the decisions
    /// are made in `delete_all_step`, which has returned before any script runs.
    #[inline(never)]
    pub fn delete_all(&mut self, root: ObjId, on_enter: Option<&[u8]>) -> Result<(), VmError> {
        let mut state = AllState::new(root);
        if let Some(method) = on_enter {
            self.call_on_enter(root, method)?;
        }
        loop {
            match self.delete_all_step(&mut state) {
                AllStep::Done => return Ok(()),
                AllStep::OutOfBudget => return Err(VmError::StepLimit),
                AllStep::Entered(inner) => {
                    if let Some(method) = on_enter {
                        self.call_on_enter(inner, method)?;
                    }
                }
                AllStep::Delete(member) => {
                    // `deleteAll` deletes its members directly: no delete event.
                    if !self.objects.defer_delete(member) {
                        self.delete_now(member, None)?;
                    }
                    self.delete_all_after(&mut state, member);
                }
            }
        }
    }

    /// Fails with `StepLimit` when the work counted so far (not yet charged: the interpreter does that
    /// at its next instruction) already takes the run past its step limit. For loops inside natives.
    #[inline(never)]
    pub fn ensure_budget(&self) -> Result<(), VmError> {
        if let Some(limit) = self.step_limit {
            let pending = self.objects.peek_work() / super::WORK_PER_STEP;
            if self.steps.saturating_add(pending) > limit {
                return Err(VmError::StepLimit);
            }
        }
        Ok(())
    }

    /// `Con::executef(set, method)`: silent when there is no such method.
    #[inline(never)]
    fn call_on_enter(&mut self, set: ObjId, method: &[u8]) -> Result<(), VmError> {
        self.call_method(set, method, &[]).map(|_| ())
    }

    /// What `delete_all` does next: pops finished sets, enters the last member when it is a set not
    /// entered yet, or names the member at the front to delete.
    #[inline(never)]
    fn delete_all_step(&mut self, state: &mut AllState) -> AllStep {
        let AllState { stack, active } = state;
        loop {
            if self.ensure_budget().is_err() {
                return AllStep::OutOfBudget;
            }
            let Some(frame) = stack.last_mut() else {
                return AllStep::Done;
            };
            self.objects.add_work(1);
            let (set, held, recursed) = (frame.set, frame.held, frame.recursed);
            let (count, last) = match self.objects.members(set) {
                Some(members) => (members.len(), members.get(members.len().wrapping_sub(1))),
                None => (0, None),
            };
            if held >= count {
                active.remove(&set);
                stack.pop();
                continue;
            }
            if !recursed {
                frame.recursed = true;
                if let Some(inner) = last.filter(|m| self.objects.is_set(*m) && !active.contains(m))
                {
                    active.insert(inner);
                    stack.push(DeleteAll {
                        set: inner,
                        held: 0,
                        recursed: false,
                    });
                    return AllStep::Entered(inner);
                }
            }
            frame.recursed = false;
            match self.objects.members(set).and_then(|m| m.get(held)) {
                Some(member) => return AllStep::Delete(member),
                None => continue,
            }
        }
    }

    /// After a member's delete: it is gone, or passed over (held), or dropped from the list (being
    /// removed).
    #[inline(never)]
    fn delete_all_after(&mut self, state: &mut AllState, member: ObjId) {
        let Some(frame) = state.stack.last_mut() else {
            return;
        };
        let set = frame.set;
        if !self.objects.is_member(set, member) {
            return;
        }
        if self
            .objects
            .get(member)
            .is_some_and(SimObject::delete_pending)
        {
            frame.held += 1;
        } else {
            self.objects.forget_member(set, member);
        }
    }
}
