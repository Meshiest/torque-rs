//! Console classes as a game registers them: every class's parent, its typed static fields (with element counts,
//! the storage offsets that make aliases, enum tables, range validators and the values a fresh object reads) and
//! its methods (argument counts, usage strings, return kinds). A class table is data ([`ClassSpec`]):
//! [`install_specs`] is how a `Vm` takes one, and [`EngineClass`] is the seam a Rust behaviour
//! (`Vm::set_class_behaviour`) plugs into.
//!
//! Follows Torque3D's MIT-licensed `console/consoleObject.cpp` (`AbstractClassRep`) (Copyright (c) 2012 GarageGames,
//! LLC; Copyright (c) 2015 Faust Logic, Inc.); the code here is this crate's own.

use std::rc::Rc;

use crate::fields::FieldType;
use crate::sim::{ObjId, Objects, SimObject, StaticField};
use crate::vm::{Native, Vm, VmError};

pub mod file_object;

/// One console class.
#[derive(Debug)]
pub struct ClassSpec {
    pub name: &'static str,
    /// None only for `SimObject` and for the console classes that are not SimObjects.
    pub parent: Option<&'static str>,
    /// False for the console classes `new` cannot make (network events and the like): `new` prints
    /// `Unable to instantiate non-SimObject class X.`
    pub sim_object: bool,
    /// Whether the class's fields include its parent's (false: the parent's fields are dynamic fields on its
    /// objects).
    pub inherits_fields: bool,
    /// The fields this class adds (all of its fields when it does not inherit its parent's), in registration order
    /// (inherited ones belong to its ancestors).
    pub fields: &'static [FieldSpec],
    pub methods: &'static [MethodSpec],
    /// What a fresh object of this class reads, as its `dump()` prints it: `name` or `name[i]`
    /// and the text; fields not listed read their type's zero. None when one cannot be made without game
    /// data (the nearest ancestor's defaults apply then).
    pub defaults: Option<&'static [(&'static str, &'static str)]>,
}

/// One static field.
#[derive(Debug)]
pub struct FieldSpec {
    pub name: &'static str,
    pub ty: FieldType,
    /// The number of elements (1 for a plain field).
    pub count: u16,
    /// The byte offset of element 0 in the C++ object: fields of one class chain with the same offset
    /// share their storage (`position` and `rotation` one matrix, `textureName` is `animTexName[0]`). None
    /// for a field with a storage of its own.
    pub offset: Option<u32>,
    pub validator: Option<Validator>,
}

/// A field's range validator (`addFieldV`): runs after every write, also one whose index was out
/// of range.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Validator {
    /// Clamps a float: `Must be between %g and %g`.
    FloatRange { min: f32, max: f32 },
    /// Divides an int by `factor` (milliseconds to ticks), then clamps: `Scaled value must be between %d and %d`.
    ScaledIntRange { factor: i32, min: i32, max: i32 },
}

/// What a method returns: decides the neutral value of a stub and how a result reaches the script.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Returns {
    Void,
    Bool,
    Int,
    Float,
    String,
}

/// One console method of a class.
#[derive(Debug)]
pub struct MethodSpec {
    pub name: &'static str,
    /// Counting the method name and `%this`, as `Vm::register_in` counts; 0 means no limit.
    pub min: i32,
    pub max: i32,
    pub usage: &'static str,
    pub returns: Returns,
}

/// What a Rust class does for its objects: the seam a host's behaviours plug into (a server's,
/// the scene, network and GUI layers). Registered by class name with
/// `Vm::set_class_behaviour`. An object runs the behaviours of its whole class chain, as C++ overrides
/// call their parent's first: `on_add` and `preload` root first (so GameBaseData's namespace links are made before
/// a subclass's behaviour looks at the object), `on_remove` leaf first. Every method has a default that does what a
/// class with no behaviour does.
///
/// The VM clones the behaviours out of the class registry (`Rc`) before it calls them and holds no borrow of
/// anything while they run, so a behaviour may run scripts (`Vm::invoke`, `Vm::call_method`: they charge the
/// re-entry's depth unit), change the object, or register another behaviour. Their errors (`StepLimit`,
/// `CallDepth`, ...) are returned and end the run, as a script's own would. The methods of an engine class are
/// natives (`Vm::register_in`), not part of this trait.
pub trait EngineClass {
    /// After the object entered the tables (TGE's `onAdd`; a ScriptObject's script `onAdd` and a
    /// datablock's DataBlockGroup join came first): `Ok(false)` fails the registration (`Register object failed
    /// ...`: the object is removed again and every behaviour of its chain is told, leaf first); an error aborts
    /// the run, and the behaviours of the chain that had already added the object are told it left.
    fn on_add(&self, vm: &mut Vm, id: ObjId) -> Result<bool, VmError> {
        let _ = (vm, id);
        Ok(true)
    }

    /// Before the object leaves the tables (after its script `onRemove` and the class's `on_remove` hook). An error
    /// is returned by the delete, after the object is gone.
    fn on_remove(&self, vm: &mut Vm, id: ObjId) -> Result<(), VmError> {
        let _ = (vm, id);
        Ok(())
    }

    /// `SimDataBlock::preload(server = true)`, run for a datablock after `new` or `datablock` registered (or
    /// re-declared) it: `Err(PreloadError::Failed(text))` prints `<file> (<line>): preload failed for <name>:
    /// <text>.` and deletes the datablock. This is the server side: there is no client preload.
    fn preload(&self, vm: &mut Vm, id: ObjId) -> Result<(), PreloadError> {
        let _ = (vm, id);
        Ok(())
    }

    /// The C++ constructor beyond the class's initial values, for the object a declaration (`new Class(...)`) makes: run
    /// before its `: parent` copy and its fields, root class first (a `GuiControlProfile` constructor may copy
    /// `GuiDefaultProfile`'s fields when that profile exists). It runs no script: the objects are read only and only
    /// `object`, not yet registered, changes. Its cost is the copy of a bounded number of slots, charged with the
    /// object's own slots.
    fn on_construct(&self, objects: &Objects, object: &mut SimObject) {
        let _ = (objects, object);
    }

    /// A declaration nested in this object (`new Group() { new Child(); }`) put the child in it: the group's own
    /// `addObject` beyond the set's (a scroll control recomputes its sizes). Called for the GROUP's class chain, root
    /// first, after the child joined it; the child's own `on_add` ran before. Runs with no borrow held, like the other
    /// methods.
    fn on_add_object(&self, vm: &mut Vm, group: ObjId, child: ObjId) -> Result<(), VmError> {
        let _ = (vm, group, child);
        Ok(())
    }
}

/// Why a [`EngineClass::preload`] did not succeed.
#[derive(Debug)]
pub enum PreloadError {
    /// The text of TGE's `errorBuffer`: the datablock is refused and deleted.
    Failed(String),
    /// The preload returned false without writing the error buffer: the datablock is refused and deleted, and the line shows
    /// what the last failed preload left in the buffer (TGE's `errorBuffer` is a static array that is never cleared; empty
    /// before any failure), see [`crate::vm::Vm::preload_error_buffer`].
    Stale,
    /// A script the preload ran stopped the run.
    Vm(VmError),
}

impl From<VmError> for PreloadError {
    fn from(error: VmError) -> PreloadError {
        PreloadError::Vm(error)
    }
}

/// `GameBaseData::onAdd` (as Torque3D's MIT-licensed `T3D/gameBase/gameBase.cpp`, Copyright (c) 2012 GarageGames, LLC;
/// Copyright (c) 2015 Faust Logic, Inc.): a NAMED datablock links its name's namespace
/// under its `className`'s (when that is set and is not its class's own name, ignoring case) and that under its
/// class's; its methods are then looked up from its name. Without a usable `className` the name goes straight
/// under the class's. An unnamed one links nothing; a later `className` write relinks nothing. The first link the
/// runtime refuses (a namespace already under another parent) prints its error and ends the work: the object stays
/// in its class's namespace. A `className` that is empty reads the class's name afterwards. `className` is the
/// static field of GameBaseData: a class that does not list it never reads or gets one.
///
/// [`install_specs`] gives it to the class GameBaseData. A host that replaces that behaviour can call this one from its
/// own (it is a plain value), or put its behaviours on the subclasses, which run after it.
pub struct GameBaseData;

impl EngineClass for GameBaseData {
    fn on_add(&self, vm: &mut Vm, id: ObjId) -> Result<bool, VmError> {
        vm.link_datablock_namespaces(id);
        Ok(true)
    }
}

/// The neutral value a stub of a method returns (what an empty implementation would give).
pub fn neutral(returns: Returns) -> Native {
    match returns {
        Returns::Void => Native::Void,
        Returns::Bool => Native::Bool(false),
        Returns::Int => Native::Int(0),
        Returns::Float => Native::Float(0.0),
        Returns::String => Native::Str(Vec::new()),
    }
}

/// A host's own declarations on one class of a table: fields the class's spec does not have (added after its own; one
/// named like a field of the spec, ignoring case, replaces that field's declaration in place) and defaults applied after
/// the spec's (a field listed in both reads the host's).
#[derive(Debug, Clone, Copy)]
pub struct ClassExtra {
    pub class: &'static str,
    pub fields: &'static [FieldSpec],
    pub defaults: &'static [(&'static str, &'static str)],
}

/// Registers one class of a table (see [`install_specs`]); true when it was new.
pub fn install_spec(vm: &mut Vm, spec: &ClassSpec) -> bool {
    install_spec_with(vm, spec, None)
}

/// [`install_spec`] with a host's own declarations on the class (`extra`, when its `class` names the spec; else it is
/// ignored). A class that was already registered keeps its fields; the defaults still apply.
pub fn install_spec_with(vm: &mut Vm, spec: &ClassSpec, extra: Option<&ClassExtra>) -> bool {
    let extra = extra.filter(|e| e.class.eq_ignore_ascii_case(spec.name));
    let static_field = |f: &FieldSpec| StaticField {
        name: f.name.as_bytes().to_vec(),
        ty: f.ty,
        count: f.count,
        offset: f.offset,
        validator: f.validator,
    };
    let mut added = false;
    let class = match vm.objects().classes.find(spec.name.as_bytes()) {
        Some(class) => class,
        None => {
            let own = extra.map_or(&[][..], |e| e.fields);
            let replaced = |name: &str| own.iter().find(|f| f.name.eq_ignore_ascii_case(name));
            let fields = spec
                .fields
                .iter()
                .map(|f| static_field(replaced(f.name).unwrap_or(f)))
                .chain(
                    own.iter()
                        .filter(|f| {
                            !spec
                                .fields
                                .iter()
                                .any(|s| s.name.eq_ignore_ascii_case(f.name))
                        })
                        .map(static_field),
                )
                .collect();
            // A class with no parent in the table sits under SimObject.
            let Some(class) = vm.register_class_typed(
                spec.name,
                spec.parent.unwrap_or("SimObject"),
                fields,
                spec.inherits_fields,
            ) else {
                return false;
            };
            added = true;
            class
        }
    };
    if !spec.sim_object
        && let Some(def) = vm.objects_mut().classes.get_mut(class)
    {
        def.sim_object = false;
    }
    match (spec.defaults, extra.map(|e| e.defaults)) {
        (Some(defaults), None) => vm.apply_defaults(class, defaults),
        (defaults, Some(more)) if defaults.is_some() || !more.is_empty() => {
            let all: Vec<(&str, &str)> = defaults
                .unwrap_or_default()
                .iter()
                .chain(more)
                .copied()
                .collect();
            vm.apply_defaults(class, &all);
        }
        _ => {}
    }
    // The one behaviour a table implies (the links of a named game datablock): given once, so a host's
    // replacement survives a second install.
    if spec.name == "GameBaseData"
        && vm
            .objects()
            .classes
            .get(class)
            .is_some_and(|def| def.behaviour.is_none())
    {
        vm.set_class_behaviour(spec.name, Rc::new(GameBaseData));
    }
    for method in spec.methods {
        if vm.has_own_function(spec.name, method.name) {
            continue;
        }
        let returns = method.returns;
        let key = format!("{}::{}", spec.name, method.name);
        let mark = key.clone();
        vm.register_in(
            spec.name,
            method.name,
            method.min,
            method.max,
            method.usage,
            move |vm, _| {
                vm.count_stub(&key);
                neutral(returns)
            },
        );
        vm.mark_stub(&mark);
    }
    added
}

/// Registers every class of `specs` on `vm` (replacing nothing a host
/// registered, stubbing the methods that have no function yet), then the natives of the classes implemented
/// here (`FileObject`), which replace the stubs. Returns how many classes it added.
pub fn install_specs(vm: &mut Vm, specs: &[ClassSpec]) -> usize {
    let mut added = 0;
    for spec in specs {
        added += usize::from(install_spec(vm, spec));
    }
    // After the table: the natives replace the stubs it just put where they go.
    file_object::register(vm);
    added
}
