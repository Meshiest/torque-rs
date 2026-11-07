//! Injected parameters: what a recipe or a typed native asks for by its parameter types, fetched for each run.
//!
//! A recipe written as a plain function names what it needs, and the engine hands it over:
//!
//! ```
//! use torque_engine::{EngineBuilder, Plugin, PluginId, Res, ServerStage, VmMut};
//!
//! struct Motd(String);
//!
//! fn set_motd(mut vm: VmMut, motd: Res<Motd>) {
//!     vm.set_global("$Server::Motd", &motd.0);
//! }
//!
//! #[derive(PluginId)]
//! struct MotdPlugin;
//! impl Plugin for MotdPlugin {
//!     fn build(&self, engine: &mut EngineBuilder) {
//!         engine.insert_resource(Motd("Welcome".into()));
//!         engine.recipe(ServerStage::Game, set_motd);
//!     }
//! }
//!
//! let engine = EngineBuilder::new().add_plugins(MotdPlugin).build()?;
//! let mut vm = torque_ts_vm::vm::Vm::new();
//! for &stage in ServerStage::ALL {
//!     engine.replay(stage, &mut vm)?;
//! }
//! assert_eq!(vm.global("$Server::Motd").as_deref(), Some(&b"Welcome"[..]));
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! The parameters ([`Param`]):
//!
//! | Type | What it is |
//! |------|------------|
//! | [`VmMut`] | the script engine, to change; at most one per function (a second one does not compile) |
//! | [`VmRef`] | the script engine, to read (globals, the natives, objects: anything taking `&Vm`); any number, but never beside a `VmMut` |
//! | [`Res<T>`](Res) | the resource of type `T`, to read |
//! | [`ResMut<T>`](ResMut) | the resource of type `T`, to change; checked at run time, so a native holding it that calls into script reaching a native wanting it again gets an error naming the resource, not a panic |
//! | [`Slot<dyn T>`](Slot) | the value of an exclusive slot |
//! | `&Engine`, [`Engine`] | the engine itself (a clone keeps it) |
//! | [`Local<T>`](Local) | state of this recipe's or this native's own, kept between runs |
//! | [`This`] | a method's object (natives only) |
//!
//! A typed native also takes script arguments: any [`ScriptArg`] type (numbers, truth values, text, `Option<T>` for an
//! argument that may be left out, [`Rest`](torque_ts_vm::convert::Rest) for the rest). Those are counted in its argument
//! counts; the injected ones are invisible to scripts. See [`ScriptRegistry`](crate::ScriptRegistry).
//!
//! A signature the engine cannot run does not compile (the check runs when the crate is built, so `cargo check` alone
//! may miss it). Two [`VmMut`]s:
//!
//! ```compile_fail,E0080
//! use torque_engine::{EngineBuilder, ServerStage, VmMut};
//!
//! fn twice(_: VmMut, _: VmMut) {}
//! EngineBuilder::new().recipe(ServerStage::Game, twice);
//! ```
//!
//! An optional script argument before a required one:
//!
//! ```compile_fail,E0080
//! use torque_engine::IntoNative;
//!
//! fn backwards(_: Option<i32>, _: i32) {}
//! let _ = backwards.into_native();
//! ```
//!
//! A script argument in a recipe:
//!
//! ```compile_fail,E0080
//! use torque_engine::{EngineBuilder, ServerStage};
//!
//! fn scripted(_: i32) {}
//! EngineBuilder::new().recipe(ServerStage::Game, scripted);
//! ```
//!
//! A [`VmMut`] beside a [`VmRef`] (one changes what the other reads):
//!
//! ```compile_fail,E0080
//! use torque_engine::{EngineBuilder, ServerStage, VmMut, VmRef};
//!
//! fn both(_: VmMut, _: VmRef) {}
//! EngineBuilder::new().recipe(ServerStage::Game, both);
//! ```
//!
//! A parameter type that is neither injected nor a script argument (the function is no native, so it has no
//! `into_native`):
//!
//! ```compile_fail,E0599
//! use torque_engine::IntoNative;
//!
//! struct Unknown;
//! fn odd(_: Unknown) {}
//! let _ = odd.into_native();
//! ```

use std::any::{Any, type_name};
use std::cell::{Cell, Ref, RefCell, RefMut};
use std::fmt;
use std::ops::{Deref, DerefMut};
use std::rc::Rc;

use torque_ts_vm::convert::{ArgKind, ConvertError, ScriptArg};
use torque_ts_vm::sim::ObjId;
use torque_ts_vm::vm::{NativeCall, Vm};

use crate::Engine;
use crate::recipe::RecipeFn;

/// What a [`Param`] takes from its [`Cx`]: decides the argument counts and the checks on a signature.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ParamKind {
    /// Something the engine hands over, invisible to scripts.
    Injected,
    /// The script engine itself, to change ([`VmMut`]): at most one per function, and no [`VmRef`] beside it.
    Vm,
    /// The script engine, to read ([`VmRef`]): any number per function, but no [`VmMut`] beside them.
    VmRead,
    /// Script arguments (natives only).
    Script(ArgKind),
}

/// The context parameters are fetched from: the script engine, the engine, and for a native the call.
pub struct Cx<'a> {
    vm: Cell<Option<&'a mut Vm>>,
    /// The script engine once a [`VmRef`] has it: shared by every `VmRef` of the run.
    shared: Cell<Option<&'a Vm>>,
    engine: &'a Engine,
    call: Option<&'a NativeCall>,
    /// The script arguments not taken yet.
    args: Cell<&'a [Vec<u8>]>,
    /// The 1-based number of the next script argument, for errors.
    arg_number: Cell<usize>,
}

impl<'a> Cx<'a> {
    /// The context of a recipe run on `vm`.
    pub fn recipe(vm: &'a mut Vm, engine: &'a Engine) -> Cx<'a> {
        Cx {
            vm: Cell::new(Some(vm)),
            shared: Cell::new(None),
            engine,
            call: None,
            args: Cell::new(&[]),
            arg_number: Cell::new(1),
        }
    }

    /// The context of a native call: its script arguments start at `argv[first_arg]` (1 for a function, 2 for a method,
    /// whose `argv[1]` is the object).
    pub fn native(
        vm: &'a mut Vm,
        engine: &'a Engine,
        call: &'a NativeCall,
        first_arg: usize,
    ) -> Cx<'a> {
        Cx {
            vm: Cell::new(Some(vm)),
            shared: Cell::new(None),
            engine,
            call: Some(call),
            args: Cell::new(call.argv.get(first_arg..).unwrap_or(&[])),
            arg_number: Cell::new(1),
        }
    }

    /// The engine.
    pub fn engine(&self) -> &'a Engine {
        self.engine
    }

    /// The native call, when this is one.
    pub fn call(&self) -> Option<&'a NativeCall> {
        self.call
    }

    /// The script engine, once: the parameter that takes it holds it for the run.
    pub fn take_vm(&self) -> Option<&'a mut Vm> {
        self.vm.take()
    }

    /// The script engine to read, as often as asked: the first call takes it (so [`take_vm`](Self::take_vm) finds
    /// nothing after), the later ones share it. None when a parameter took it to change.
    pub fn share_vm(&self) -> Option<&'a Vm> {
        if let Some(vm) = self.shared.get() {
            return Some(vm);
        }
        let vm: &'a Vm = self.vm.take()?;
        self.shared.set(Some(vm));
        Some(vm)
    }

    /// Takes the next script argument (or arguments) as `T`.
    pub fn take_arg<T: ScriptArg>(&self) -> Result<T::Item<'a>, ParamError> {
        let mut args = self.args.get();
        let before = args.len();
        let number = self.arg_number.get();
        let item = T::take(&mut args).map_err(|error| ParamError::Argument { number, error })?;
        self.arg_number.set(number + (before - args.len()));
        self.args.set(args);
        Ok(item)
    }
}

/// A parameter type a recipe or a typed native may take: fetched from the [`Cx`] for each run.
///
/// `Item<'a>` is the parameter's type for one run (`Res<'a, T>` borrows the engine for it). `State` is kept between runs
/// (a recipe's for the engine's life, a native's for its script engine's): [`Local`] keeps its value there.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is neither a script argument nor an injected parameter",
    label = "not a parameter a native or a recipe can take",
    note = "script arguments are numbers, truth values, text, `Option<T>` and `Rest`; injected ones are `VmMut`, `VmRef`, `Res<T>`, `ResMut<T>`, `Slot<T>`, `&Engine`, `Local<T>` and `This`"
)]
pub trait Param {
    /// The parameter's type during one run.
    type Item<'a>;
    /// What the parameter keeps between runs.
    type State: Default + 'static;
    /// What it takes from the context.
    const KIND: ParamKind = ParamKind::Injected;
    /// Fetches the parameter for one run.
    fn fetch<'a>(cx: &'a Cx<'a>, state: &'a Self::State) -> Result<Self::Item<'a>, ParamError>;
}

/// Why a parameter could not be fetched: the run does not happen, and the error is reported (a recipe's stops the
/// replay; a native's is printed on the console and the call returns the empty string).
#[derive(Clone, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum ParamError {
    /// A resource is missing or borrowed.
    Resource(ResourceError),
    /// No plugin set the slot.
    NoSlot {
        /// The slot's type.
        slot: &'static str,
    },
    /// A second parameter wanted the script engine (only one may hold it).
    VmTaken,
    /// A [`Local`]'s state is in use: the function was run again from inside itself.
    LocalInUse {
        /// The state's type.
        state: &'static str,
    },
    /// A parameter that only a native has ([`This`], a script argument) in a recipe, or [`This`] outside a method call.
    NotInCall {
        /// The parameter.
        param: &'static str,
    },
    /// A script argument could not be read.
    Argument {
        /// Its 1-based number among the script arguments.
        number: usize,
        /// Why.
        error: ConvertError,
    },
}

impl fmt::Display for ParamError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParamError::Resource(error) => error.fmt(f),
            ParamError::NoSlot { slot } => write!(f, "no plugin set the slot `{slot}`"),
            ParamError::VmTaken => f.write_str(
                "the script engine is held by another parameter (a function takes one `VmMut`, or any number of `VmRef`)",
            ),
            ParamError::LocalInUse { state } => write!(
                f,
                "the state `Local<{state}>` is in use: the function was called again from inside itself"
            ),
            ParamError::NotInCall { param } => write!(f, "`{param}` is only known in a native's call"),
            ParamError::Argument { number, error } => write!(f, "argument {number}: {error}"),
        }
    }
}

impl std::error::Error for ParamError {}

impl From<ResourceError> for ParamError {
    fn from(error: ResourceError) -> ParamError {
        ParamError::Resource(error)
    }
}

/// Why a resource could not be borrowed.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ResourceError {
    /// No plugin inserted it.
    Missing {
        /// The resource's type.
        resource: &'static str,
    },
    /// It is borrowed (by [`Res`] or [`ResMut`]), so it cannot be borrowed to change.
    Borrowed {
        /// The resource's type.
        resource: &'static str,
    },
    /// It is borrowed to change (by [`ResMut`]), so it cannot be borrowed at all.
    BorrowedMut {
        /// The resource's type.
        resource: &'static str,
    },
}

impl fmt::Display for ResourceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ResourceError::Missing { resource } => {
                write!(
                    f,
                    "the resource `{resource}` is missing: no plugin inserted it"
                )
            }
            ResourceError::Borrowed { resource } => write!(
                f,
                "the resource `{resource}` is already borrowed: a call holding it ran script that wants it as `ResMut`"
            ),
            ResourceError::BorrowedMut { resource } => write!(
                f,
                "the resource `{resource}` is borrowed to change: a call holding it as `ResMut` ran script that wants it again"
            ),
        }
    }
}

impl std::error::Error for ResourceError {}

/// The script engine, to change: a parameter. A function takes at most one (a second one fails to compile).
pub struct VmMut<'a>(&'a mut Vm);

impl<'a> VmMut<'a> {
    /// The parameter over `vm`: how a wrapper that holds the script engine hands it on (the method natives of
    /// [`methods`](crate::methods) that take `&self`).
    pub fn new(vm: &'a mut Vm) -> VmMut<'a> {
        VmMut(vm)
    }

    /// The script engine for the rest of the run.
    pub fn into_inner(self) -> &'a mut Vm {
        self.0
    }
}

impl Deref for VmMut<'_> {
    type Target = Vm;
    fn deref(&self) -> &Vm {
        self.0
    }
}

impl DerefMut for VmMut<'_> {
    fn deref_mut(&mut self) -> &mut Vm {
        self.0
    }
}

impl Param for VmMut<'_> {
    type Item<'a> = VmMut<'a>;
    type State = ();
    const KIND: ParamKind = ParamKind::Vm;
    fn fetch<'a>(cx: &'a Cx<'a>, _: &'a ()) -> Result<VmMut<'a>, ParamError> {
        cx.take_vm().map(VmMut).ok_or(ParamError::VmTaken)
    }
}

/// The script engine, to read: a parameter for a function that only looks (globals, the natives, objects; anything
/// that takes `&Vm`). A function may take any number of them, but none beside a [`VmMut`] (that does not compile).
///
/// ```
/// use torque_engine::{EngineBuilder, ServerStage, VmRef};
/// use torque_ts_vm::vm::Vm;
///
/// // How many natives the script engine has: a look, no change.
/// fn native_count(vm: VmRef) -> i32 {
///     vm.native_signatures().len() as i32
/// }
///
/// let mut builder = EngineBuilder::new();
/// builder.script().function("", "nativeCount", &[], "how many natives there are", native_count);
/// let engine = builder.build()?;
/// let mut vm = Vm::new();
/// for &stage in ServerStage::ALL {
///     engine.replay(stage, &mut vm)?;
/// }
/// let count = vm.native_signatures().len();
/// assert_eq!(vm.eval(b"return nativeCount();")?, count.to_string().as_bytes());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Clone, Copy)]
pub struct VmRef<'a>(&'a Vm);

impl<'a> VmRef<'a> {
    /// The parameter over `vm`: how a wrapper that holds the script engine hands it on.
    pub fn new(vm: &'a Vm) -> VmRef<'a> {
        VmRef(vm)
    }

    /// The script engine for the rest of the run.
    pub fn into_inner(self) -> &'a Vm {
        self.0
    }
}

impl Deref for VmRef<'_> {
    type Target = Vm;
    fn deref(&self) -> &Vm {
        self.0
    }
}

impl Param for VmRef<'_> {
    type Item<'a> = VmRef<'a>;
    type State = ();
    const KIND: ParamKind = ParamKind::VmRead;
    fn fetch<'a>(cx: &'a Cx<'a>, _: &'a ()) -> Result<VmRef<'a>, ParamError> {
        cx.share_vm().map(VmRef).ok_or(ParamError::VmTaken)
    }
}

/// The resource of type `T`, to read: a parameter, and what [`Engine::resource`] gives.
pub struct Res<'a, T: 'static>(Ref<'a, T>);

impl<'a, T> Res<'a, T> {
    pub(crate) fn new(value: Ref<'a, T>) -> Res<'a, T> {
        Res(value)
    }
}

impl<T> Deref for Res<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T: fmt::Debug> fmt::Debug for Res<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        (**self).fmt(f)
    }
}

impl<T: Any> Param for Res<'_, T> {
    type Item<'a> = Res<'a, T>;
    type State = ();
    fn fetch<'a>(cx: &'a Cx<'a>, _: &'a ()) -> Result<Res<'a, T>, ParamError> {
        Ok(cx.engine().try_resource::<T>()?)
    }
}

/// The resource of type `T`, to change: a parameter, and what [`Engine::resource_mut`] gives. The borrow is checked at
/// run time: while one is held, another `Res<T>` or `ResMut<T>` fails with a [`ResourceError`] naming `T`.
pub struct ResMut<'a, T: 'static>(RefMut<'a, T>);

impl<'a, T> ResMut<'a, T> {
    pub(crate) fn new(value: RefMut<'a, T>) -> ResMut<'a, T> {
        ResMut(value)
    }
}

impl<T> Deref for ResMut<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T> DerefMut for ResMut<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.0
    }
}

impl<T: fmt::Debug> fmt::Debug for ResMut<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        (**self).fmt(f)
    }
}

impl<T: Any> Param for ResMut<'_, T> {
    type Item<'a> = ResMut<'a, T>;
    type State = ();
    fn fetch<'a>(cx: &'a Cx<'a>, _: &'a ()) -> Result<ResMut<'a, T>, ParamError> {
        Ok(cx.engine().try_resource_mut::<T>()?)
    }
}

/// The value of the exclusive slot of type `T` (usually a `dyn Trait`): a parameter. A run whose slot no plugin set fails
/// with [`ParamError::NoSlot`].
pub struct Slot<T: ?Sized + 'static>(Rc<T>);

impl<T: ?Sized> Slot<T> {
    /// The shared value, to keep.
    pub fn into_rc(self) -> Rc<T> {
        self.0
    }
}

impl<T: ?Sized> Clone for Slot<T> {
    fn clone(&self) -> Slot<T> {
        Slot(Rc::clone(&self.0))
    }
}

impl<T: ?Sized> Deref for Slot<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T: ?Sized + 'static> Param for Slot<T> {
    type Item<'a> = Slot<T>;
    type State = ();
    fn fetch<'a>(cx: &'a Cx<'a>, _: &'a ()) -> Result<Slot<T>, ParamError> {
        cx.engine().slot::<T>().map(Slot).ok_or(ParamError::NoSlot {
            slot: type_name::<T>(),
        })
    }
}

impl Param for &Engine {
    type Item<'a> = &'a Engine;
    type State = ();
    fn fetch<'a>(cx: &'a Cx<'a>, _: &'a ()) -> Result<&'a Engine, ParamError> {
        Ok(cx.engine())
    }
}

/// A clone of the engine (one `Rc`), for a native or a closure that keeps it.
impl Param for Engine {
    type Item<'a> = Engine;
    type State = ();
    fn fetch<'a>(cx: &'a Cx<'a>, _: &'a ()) -> Result<Engine, ParamError> {
        Ok(cx.engine().clone())
    }
}

/// State of the function's own, kept between its runs, starting at `T::default()`: a parameter.
///
/// A recipe's state lasts as long as the engine (every replay sees it); a native's lasts as long as the script engine it
/// was registered in, so each server or client starts fresh. A function run again from inside itself (a native whose
/// script calls it again) fails with [`ParamError::LocalInUse`].
pub struct Local<'a, T: 'static>(RefMut<'a, T>);

impl<T> Deref for Local<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T> DerefMut for Local<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.0
    }
}

impl<T: Default + 'static> Param for Local<'_, T> {
    type Item<'a> = Local<'a, T>;
    type State = RefCell<T>;
    fn fetch<'a>(_: &'a Cx<'a>, state: &'a RefCell<T>) -> Result<Local<'a, T>, ParamError> {
        state
            .try_borrow_mut()
            .map(Local)
            .map_err(|_| ParamError::LocalInUse {
                state: type_name::<T>(),
            })
    }
}

/// The object a method was called on (TGE's `thisObject`): a parameter of a native. A call with no object fails with
/// [`ParamError::NotInCall`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct This(pub ObjId);

impl Param for This {
    type Item<'a> = This;
    type State = ();
    fn fetch<'a>(cx: &'a Cx<'a>, _: &'a ()) -> Result<This, ParamError> {
        cx.call()
            .and_then(|call| call.this_object)
            .map(This)
            .ok_or(ParamError::NotInCall { param: "This" })
    }
}

impl<T: ScriptArg> Param for T {
    type Item<'a> = T::Item<'a>;
    type State = ();
    const KIND: ParamKind = ParamKind::Script(T::KIND);
    fn fetch<'a>(cx: &'a Cx<'a>, _: &'a ()) -> Result<T::Item<'a>, ParamError> {
        if cx.call().is_none() {
            return Err(ParamError::NotInCall {
                param: type_name::<T>(),
            });
        }
        cx.take_arg::<T>()
    }
}

/// What a signature's parameter kinds add up to.
#[doc(hidden)]
#[derive(Clone, Copy, Debug)]
pub struct Shape {
    /// The [`VmMut`]s.
    pub vms: usize,
    /// The [`VmRef`]s.
    pub vm_reads: usize,
    /// Script arguments: required, optional, and whether a rest follows.
    pub required: usize,
    pub optional: usize,
    pub rest: bool,
    /// A required argument after an optional one or the rest, or anything after the rest.
    pub misordered: bool,
}

/// Adds up the parameter kinds of a signature, in order.
#[doc(hidden)]
pub const fn shape(kinds: &[ParamKind]) -> Shape {
    let mut shape = Shape {
        vms: 0,
        vm_reads: 0,
        required: 0,
        optional: 0,
        rest: false,
        misordered: false,
    };
    let mut i = 0;
    while i < kinds.len() {
        match kinds[i] {
            ParamKind::Injected => {}
            ParamKind::Vm => shape.vms += 1,
            ParamKind::VmRead => shape.vm_reads += 1,
            ParamKind::Script(kind) => {
                if shape.rest {
                    shape.misordered = true;
                }
                match kind {
                    ArgKind::Required => {
                        if shape.optional > 0 {
                            shape.misordered = true;
                        }
                        shape.required += 1;
                    }
                    ArgKind::Optional => shape.optional += 1,
                    ArgKind::Rest => shape.rest = true,
                }
            }
        }
        i += 1;
    }
    shape
}

/// What is wrong with a native's parameter kinds, in order: too many [`VmMut`]s, a `VmMut` beside a [`VmRef`], or the
/// script arguments out of order. None for a signature the engine can run. The macros check a signature with it when
/// the crate is checked; [`IntoNative`](crate::IntoNative) when it is built.
#[doc(hidden)]
pub const fn native_shape_error(kinds: &[ParamKind]) -> Option<&'static str> {
    let shape = shape(kinds);
    if shape.vms > 1 {
        Some("a native takes at most one `VmMut`")
    } else if shape.vms > 0 && shape.vm_reads > 0 {
        Some("a native takes either one `VmMut` or any number of `VmRef`, never both")
    } else if shape.misordered {
        Some("a native's script arguments go required first, then `Option`s, then one `Rest` last")
    } else {
        None
    }
}

/// What a recipe written as a function returns: nothing, or `Result<(), E>` whose error stops the replay.
pub trait RecipeReturn {
    /// The result as [`Engine::replay`] reports it.
    fn into_result(self) -> Result<(), String>;
}

impl RecipeReturn for () {
    fn into_result(self) -> Result<(), String> {
        Ok(())
    }
}

impl<E: fmt::Display> RecipeReturn for Result<(), E> {
    fn into_result(self) -> Result<(), String> {
        self.map_err(|error| error.to_string())
    }
}

/// A function [`EngineBuilder::recipe`](crate::EngineBuilder::recipe) takes: one whose parameters are all [`Param`]s
/// (up to 12, no script arguments, one [`VmMut`] or any number of [`VmRef`]) returning a [`RecipeReturn`], or the closure form
/// `Fn(&mut Vm, &Engine) -> Result<(), String>` that [`server`](crate::EngineBuilder::server) and
/// [`client`](crate::EngineBuilder::client) take.
///
/// `M` tells the forms apart; it is inferred.
pub trait IntoRecipe<M> {
    /// The recipe as the engine stores it.
    #[doc(hidden)]
    fn into_recipe(self) -> RecipeFn;
}

/// The marker of the closure form of [`IntoRecipe`].
#[doc(hidden)]
pub struct RawRecipe;

impl<F> IntoRecipe<RawRecipe> for F
where
    F: Fn(&mut Vm, &Engine) -> Result<(), String> + 'static,
{
    fn into_recipe(self) -> RecipeFn {
        Box::new(self)
    }
}

macro_rules! recipe_fns {
    ($($P:ident),*) => {
        impl<F, R, $($P,)*> IntoRecipe<fn($($P,)*) -> R> for F
        where
            F: Fn($($P),*) -> R + Fn($(<$P as Param>::Item<'_>),*) -> R + 'static,
            R: RecipeReturn + 'static,
            $($P: Param + 'static,)*
        {
            #[allow(non_snake_case, unused_variables)]
            fn into_recipe(self) -> RecipeFn {
                const {
                    let shape = shape(&[$(<$P as Param>::KIND),*]);
                    assert!(shape.vms <= 1, "a recipe takes at most one `VmMut`");
                    assert!(
                        shape.vms == 0 || shape.vm_reads == 0,
                        "a recipe takes either one `VmMut` or any number of `VmRef`, never both"
                    );
                    assert!(
                        shape.required == 0 && shape.optional == 0 && !shape.rest,
                        "a recipe takes no script arguments: only `Param`s such as `VmMut`, `Res<T>` or `Local<T>`"
                    );
                }
                let state: ($(<$P as Param>::State,)*) = Default::default();
                Box::new(move |vm: &mut Vm, engine: &Engine| -> Result<(), String> {
                    let cx = Cx::recipe(vm, engine);
                    let ($($P,)*) = &state;
                    $(let $P = <$P as Param>::fetch(&cx, $P).map_err(|e| e.to_string())?;)*
                    (self)($($P),*).into_result()
                })
            }
        }
    };
}

/// Calls `$m` once per parameter count, 0 through 12.
macro_rules! all_arities {
    ($m:ident) => {
        $m!();
        $m!(T0);
        $m!(T0, T1);
        $m!(T0, T1, T2);
        $m!(T0, T1, T2, T3);
        $m!(T0, T1, T2, T3, T4);
        $m!(T0, T1, T2, T3, T4, T5);
        $m!(T0, T1, T2, T3, T4, T5, T6);
        $m!(T0, T1, T2, T3, T4, T5, T6, T7);
        $m!(T0, T1, T2, T3, T4, T5, T6, T7, T8);
        $m!(T0, T1, T2, T3, T4, T5, T6, T7, T8, T9);
        $m!(T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10);
        $m!(T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11);
    };
}

pub(crate) use all_arities;

all_arities!(recipe_fns);
