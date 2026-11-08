//! Console classes declared in Rust: a struct whose fields are the class's console fields.
//!
//! A plugin declares one with `#[derive(EngineClass)]` and registers it with
//! [`ScriptRegistry::class`](crate::ScriptRegistry::class). Every script engine then has the class, below its parent,
//! with typed static fields: a script writes them by the console's rules for the field's type (`dAtoi`, `dAtof`,
//! `./` in file names, the range clamp), `obj.dump()` lists their docs and ranges, and Rust reads an object back as the
//! struct ([`ScriptClass::read`]). The console's own classes are not declared this way; they keep their class tables.
//!
//! ```
//! use torque_engine::{EngineBuilder, EngineClass, Filename, Plugin, PluginId, ScriptClass, ServerStage};
//! use torque_ts_vm::vm::Vm;
//!
//! #[derive(EngineClass)]
//! #[class(name = "RopeData", parent = "SimDataBlock")]
//! struct RopeData {
//!     /// How many pieces the rope is cut into.
//!     #[field(range = 2..=64, default = 8)]
//!     segments: i32,
//!     length: f32,
//!     material: Filename,
//! }
//!
//! #[derive(PluginId)]
//! struct RopePlugin;
//! impl Plugin for RopePlugin {
//!     fn build(&self, engine: &mut EngineBuilder) {
//!         engine.script().class::<RopeData>();
//!     }
//! }
//!
//! let engine = EngineBuilder::new().add_plugins(RopePlugin).build()?;
//! let mut vm = Vm::new();
//! for &stage in ServerStage::ALL {
//!     engine.replay(stage, &mut vm)?;
//! }
//! vm.eval(b"datablock RopeData(Cable) { length = 2.5; segments = 100; material = \"art/rope\"; };")?;
//! let id = vm.objects().find(b"Cable").expect("the datablock");
//! let rope = RopeData::read(&vm, id)?;
//! assert_eq!((rope.segments, rope.length), (64, 2.5), "100 is clamped to the range");
//! assert_eq!(rope.material.as_str(), "art/rope");
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

use std::fmt;
use std::marker::PhantomData;
use std::rc::Rc;

use torque_ts_vm::classes::{EngineClass as ClassBehavior, PreloadError, Validator};
use torque_ts_vm::convert::Filename;
use torque_ts_vm::fields::{FieldType, Value};
use torque_ts_vm::sim::StaticField;

pub use torque_ts_vm::fields::Value as FieldValue;
pub use torque_ts_vm::sim::ObjId;
pub use torque_ts_vm::vm::Vm;

use crate::Engine;
use crate::assets::AssetType;
use crate::script::{EntryKind, ScriptRegistry};

/// A Rust type that can be a console field: which console field type it is, and how a stored value reads back as it.
/// The types are `i32`, `u8`, `u16`, `f32`, `bool`, `String`, [`Filename`], and the point and color types
/// (`[f32; 3]` is a `Point3F`, `[f32; 2]` a `Point2F`, `[i32; 2]` a `Point2I`, `[f32; 4]` a `ColorF`, `[u8; 4]` a
/// `ColorI`).
#[diagnostic::on_unimplemented(
    message = "`{Self}` cannot be a console field: use `i32`, `u8`, `u16`, `f32`, `bool`, `String`, `Filename`, or a point or color array",
    label = "not a field type"
)]
pub trait ClassField: Sized {
    /// The console field type: how a script's text is parsed into the field and printed back.
    const TYPE: FieldType;
    /// The type a `default = ..` literal has: the type itself for a number or a truth value, text for the rest.
    type Literal;
    /// The type's value in a stored field element; `None` if it holds another kind.
    fn from_value(value: &Value) -> Option<Self>;
    /// The value kept inside `min` and `max` when a read finds it outside. The console's range check clamps what a script
    /// writes but lets a NaN through (it is neither below nor above); a typed read does not give one back (a float
    /// range is the one case: the bound a NaN meets is what it becomes).
    fn bounded(self, min: Option<f64>, max: Option<f64>) -> Self {
        let _ = (min, max);
        self
    }
}

impl ClassField for f32 {
    const TYPE: FieldType = FieldType::F32;
    type Literal = f32;
    fn from_value(value: &Value) -> Option<f32> {
        match value {
            Value::Float(v) => Some(*v),
            _ => None,
        }
    }
    fn bounded(self, min: Option<f64>, max: Option<f64>) -> f32 {
        if !self.is_nan() {
            return self;
        }
        match (min, max) {
            (Some(low), _) => low as f32,
            (None, Some(high)) => high as f32,
            (None, None) => self,
        }
    }
}

macro_rules! class_field {
    ($($ty:ty, $lit:ty => $field:expr, $pat:pat => $value:expr;)*) => {
        $(
            impl ClassField for $ty {
                const TYPE: FieldType = $field;
                type Literal = $lit;
                fn from_value(value: &Value) -> Option<$ty> {
                    match value {
                        $pat => Some($value),
                        _ => None,
                    }
                }
            }
        )*
    };
}

class_field! {
    i32, i32 => FieldType::S32, Value::Int(v) => *v;
    u8, u8 => FieldType::U8, Value::Int(v) => *v as u8;
    u16, u16 => FieldType::U16, Value::Int(v) => *v as u16;
    bool, bool => FieldType::Bool, Value::Bool(v) => *v;
    String, &'static str => FieldType::String, Value::Text(v) => String::from_utf8_lossy(v).into_owned();
    Filename, &'static str => FieldType::Filename, Value::Text(v) => Filename(String::from_utf8_lossy(v).into_owned());
    [f32; 3], &'static str => FieldType::Point3F, Value::Floats3(v) => *v;
    [f32; 2], &'static str => FieldType::Point2F, Value::Floats2(v) => *v;
    [i32; 2], &'static str => FieldType::Point2I, Value::Ints2(v) => *v;
    [f32; 4], &'static str => FieldType::ColorF, Value::ColorF(v) => *v;
    [u8; 4], &'static str => FieldType::ColorI, Value::ColorI(v) => *v;
}

/// A field type that can name an asset (`#[field(asset = T)]`): [`Filename`] alone.
#[diagnostic::on_unimplemented(
    message = "`{Self}` cannot name an asset: `#[field(asset = ..)]` goes on a `Filename` field",
    label = "not a `Filename`"
)]
pub trait AssetField {}

impl AssetField for Filename {}

/// One console field of a class, as `#[derive(EngineClass)]` declares it: a constant.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct FieldDecl {
    /// The console field's name, as scripts write it (`maxLength`).
    pub name: &'static str,
    /// The Rust field's name.
    pub field: &'static str,
    /// The console field type.
    pub ty: FieldType,
    /// The doc comment, whole.
    pub doc: &'static str,
    /// What a fresh object reads, as the text a script writes; `None` for the type's zero.
    pub default: Option<&'static str>,
    /// The lowest value a write keeps, when the field has a range.
    pub min: Option<f64>,
    /// The highest value a write keeps, when the field has a range.
    pub max: Option<f64>,
    /// The asset type a [`Filename`] field names (`#[field(asset = T)]`): each datablock's preload loads the file into
    /// its script engine's [`Assets`](crate::assets::Assets).
    pub asset: Option<AssetType>,
}

impl FieldDecl {
    /// The range's validator, for the types that have one: a float's, or a whole number's (`i32`, `u8`, `u16`).
    pub fn validator(&self) -> Option<Validator> {
        let (min, max) = (self.min?, self.max?);
        match self.ty {
            FieldType::F32 => Some(Validator::FloatRange {
                min: min as f32,
                max: max as f32,
            }),
            FieldType::S32 | FieldType::U8 | FieldType::U16 => Some(Validator::ScaledIntRange {
                factor: 1,
                min: min.ceil() as i32,
                max: max.floor() as i32,
            }),
            _ => None,
        }
    }

    /// The range as a reference shows it (`2 to 64`), when the field has one the console keeps.
    pub fn range_text(&self) -> Option<String> {
        self.validator()?;
        Some(format!("{} to {}", self.min?, self.max?))
    }

    /// The static field the script engine registers.
    fn static_field(&self) -> StaticField {
        StaticField {
            name: self.name.as_bytes().to_vec(),
            ty: self.ty,
            count: 1,
            offset: None,
            validator: self.validator(),
            doc: (!self.doc.is_empty()).then_some(self.doc),
        }
    }
}

/// One field's value for [`ScriptClass::from_values`]: what `get` gives for it, as a `T`; its Rust name when there is
/// none or it holds another kind. What `#[derive(EngineClass)]` calls for each field.
pub fn read_field<T: ClassField>(
    decl: &FieldDecl,
    get: &mut dyn FnMut(&FieldDecl) -> Option<Value>,
) -> Result<T, &'static str> {
    get(decl)
        .and_then(|value| T::from_value(&value))
        .map(|value| value.bounded(decl.min, decl.max))
        .ok_or(decl.field)
}

/// Why an object could not be read as a class's struct.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ClassReadError {
    /// There is no object with the id.
    Gone(ObjId),
    /// The object is not of the class (or of a class below it).
    NotAnInstance {
        /// The object.
        id: ObjId,
        /// The class asked for.
        class: &'static str,
    },
    /// A field is missing from the object's class or holds another kind of value than the struct's field.
    Field {
        /// The class asked for.
        class: &'static str,
        /// The Rust field.
        field: &'static str,
    },
}

impl fmt::Display for ClassReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ClassReadError::Gone(id) => write!(f, "object {id} is gone"),
            ClassReadError::NotAnInstance { id, class } => {
                write!(f, "object {id} is not a {class}")
            }
            ClassReadError::Field { class, field } => {
                write!(
                    f,
                    "the {class} object's field `{field}` is missing or of another type"
                )
            }
        }
    }
}

impl std::error::Error for ClassReadError {}

/// A console class declared as a struct: implemented by `#[derive(EngineClass)]`, registered with
/// [`ScriptRegistry::class`](crate::ScriptRegistry::class).
pub trait ScriptClass: Sized + 'static {
    /// The console class's name.
    const NAME: &'static str;
    /// The class it goes below.
    const PARENT: &'static str;
    /// The class's own fields, in order.
    const FIELDS: &'static [FieldDecl];
    /// Whether [`preload`](Self::preload) is set (`#[class(preload = ..)]`).
    const HAS_PRELOAD: bool = false;

    /// Builds the struct from what `get` gives for each field; `Err` names the field that could not be read.
    fn from_values(get: &mut dyn FnMut(&FieldDecl) -> Option<Value>) -> Result<Self, &'static str>;

    /// Runs for each datablock of the class (and of a class below it) once it is registered, with its fields read: `Err`
    /// prints the preload failure with the text and deletes the datablock. Only datablocks are preloaded, so a class that
    /// is not below `SimDataBlock` never runs it. This is the server side; there is no client preload.
    fn preload(&self, vm: &mut Vm, id: ObjId) -> Result<(), String> {
        let _ = (vm, id);
        Ok(())
    }

    /// The struct of the fields the object `id` holds now.
    fn read(vm: &Vm, id: ObjId) -> Result<Self, ClassReadError> {
        read_fields(vm, id, Self::NAME, &mut |get| Self::from_values(get))
    }
}

/// What gives a field's stored value for its declaration.
type FieldGetter<'a> = dyn FnMut(&FieldDecl) -> Option<Value> + 'a;

/// Makes the console class's behavior for an engine (its preload).
type MakeBehavior = fn(&Engine) -> Rc<dyn ClassBehavior>;

/// What `id`'s fields give `build` (each the value of the class field a declaration names), once `id` is checked to be a
/// `class`.
fn read_fields<R>(
    vm: &Vm,
    id: ObjId,
    class_name: &'static str,
    build: &mut dyn FnMut(&mut FieldGetter<'_>) -> Result<R, &'static str>,
) -> Result<R, ClassReadError> {
    let objects = vm.objects();
    let object = objects.get(id).ok_or(ClassReadError::Gone(id))?;
    let class = object.class();
    let is_instance = objects
        .classes
        .find(class_name.as_bytes())
        .is_some_and(|me| objects.classes.is_a(class, me));
    if !is_instance {
        return Err(ClassReadError::NotAnInstance {
            id,
            class: class_name,
        });
    }
    build(&mut |decl| {
        let (_, slots) = objects.classes.static_field(class, decl.name.as_bytes())?;
        object.slot(*slots.first()?).cloned()
    })
    .map_err(|field| ClassReadError::Field {
        class: class_name,
        field,
    })
}

/// What a class registered by [`ScriptRegistry::class`](crate::ScriptRegistry::class) declares: also what a reference
/// page lists ([`Engine::classes`]).
#[derive(Clone, Copy)]
pub struct ClassInfo {
    /// The class's name.
    pub name: &'static str,
    /// The class it goes below.
    pub parent: &'static str,
    /// Its own fields.
    pub fields: &'static [FieldDecl],
    preload: Option<MakeBehavior>,
}

impl ClassInfo {
    /// The declaration of `T`.
    pub fn of<T: ScriptClass>() -> ClassInfo {
        ClassInfo {
            name: T::NAME,
            parent: T::PARENT,
            fields: T::FIELDS,
            preload: (T::HAS_PRELOAD || T::FIELDS.iter().any(|f| f.asset.is_some()))
                .then_some(behavior::<T>),
        }
    }

    /// Registers the class in `vm`: below its parent, with its typed fields and their defaults and, when it has one, its
    /// preload. A name another class has, or a parent that is not there, is a console error and registers nothing.
    pub(crate) fn install(&self, vm: &mut Vm, engine: &Engine) {
        let fields = self.fields.iter().map(FieldDecl::static_field).collect();
        let Some(class) = vm.register_class_typed(self.name, self.parent, fields, true) else {
            vm.error(format!(
                "class {}: not registered: the name is taken or its parent {} does not exist",
                self.name, self.parent
            ));
            return;
        };
        let defaults: Vec<(&str, &str)> = self
            .fields
            .iter()
            .filter_map(|f| Some((f.name, f.default?)))
            .collect();
        if !defaults.is_empty() {
            vm.apply_defaults(class, &defaults, true);
        }
        if let Some(make) = self.preload {
            vm.set_class_behaviour(self.name, make(engine));
        }
    }
}

impl fmt::Debug for ClassInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClassInfo")
            .field("name", &self.name)
            .field("parent", &self.parent)
            .field("fields", &self.fields.len())
            .finish_non_exhaustive()
    }
}

/// The behavior of a class with a preload: reads the object as `T`, loads the assets its fields name, and runs
/// `T::preload`.
struct Preload<T> {
    /// The engine whose asset loaders the script engine's cache gets, when it has none yet.
    engine: Engine,
    class: PhantomData<fn() -> T>,
}

impl<T: ScriptClass> ClassBehavior for Preload<T> {
    fn preload(&self, vm: &mut Vm, id: ObjId) -> Result<(), PreloadError> {
        let value = T::read(vm, id).map_err(|error| PreloadError::Failed(error.to_string()))?;
        load_assets(&self.engine, T::NAME, T::FIELDS, vm, id).map_err(PreloadError::Failed)?;
        value.preload(vm, id).map_err(PreloadError::Failed)
    }
}

/// Loads the file each asset field of `id` names (`field: <the error>` for the first that does not load). An empty name
/// names nothing.
fn load_assets(
    engine: &Engine,
    class: &'static str,
    fields: &'static [FieldDecl],
    vm: &mut Vm,
    id: ObjId,
) -> Result<(), String> {
    let mut named: Vec<(&FieldDecl, AssetType, String)> = Vec::new();
    for decl in fields {
        let Some(asset) = decl.asset else {
            continue;
        };
        let path = read_fields(vm, id, class, &mut |get| match get(decl) {
            Some(Value::Text(text)) => Ok(String::from_utf8_lossy(&text).into_owned()),
            _ => Err(decl.field),
        })
        .map_err(|error| error.to_string())?;
        if !path.is_empty() {
            named.push((decl, asset, path));
        }
    }
    if named.is_empty() {
        return Ok(());
    }
    let assets = engine.assets(vm);
    for (decl, asset, path) in named {
        asset
            .check(&assets, vm.file_host(), &path)
            .map_err(|error| format!("{}: {error}", decl.name))?;
    }
    Ok(())
}

fn behavior<T: ScriptClass>(engine: &Engine) -> Rc<dyn ClassBehavior> {
    Rc::new(Preload::<T> {
        engine: engine.clone(),
        class: PhantomData,
    })
}

impl ScriptRegistry<'_> {
    /// Declares the console class `T`: registered in every script engine of the side below its parent, with its fields,
    /// before the scripts run (the parent must exist by then). A class name another declaration took fails the build
    /// ([`BuildError::DuplicateScriptName`](crate::BuildError::DuplicateScriptName)). See [`ScriptClass`].
    pub fn class<T: ScriptClass>(&mut self) -> &mut Self {
        let info = ClassInfo::of::<T>();
        let key = format!("class {}", info.name).to_ascii_lowercase();
        self.declare(key, "class", info.name.to_string(), || {
            EntryKind::Class(info)
        })
    }
}

impl Engine {
    /// Every class a plugin declared, in the order they were declared: what a reference page lists.
    pub fn classes(&self) -> impl Iterator<Item = ClassInfo> + '_ {
        self.script_entries().filter_map(|entry| match entry {
            EntryKind::Class(info) => Some(*info),
            _ => None,
        })
    }
}

#[cfg(test)]
#[path = "class_tests.rs"]
mod tests;
