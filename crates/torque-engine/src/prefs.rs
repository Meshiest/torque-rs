//! Typed preferences: a struct whose fields live in `$Pref::...` globals.
//!
//! A plugin declares them with `#[derive(Prefs)]` and registers them with
//! [`ScriptRegistry::prefs`](crate::ScriptRegistry::prefs): every script engine gets each pref's default before its
//! scripts run (a script, or the `prefs.cs` a game executes, then overrides it), and Rust reads the current values back
//! as the struct ([`VmPrefs::prefs`], or a [`Pref`] parameter), each clamped to its range. Prefs are ordinary globals,
//! so `export("$Pref::*", file)` saves them with the rest; [`Engine::export_prefs`] saves exactly the registered ones.
//!
//! ```
//! use torque_engine::{EngineBuilder, Plugin, PluginId, Prefs, ServerStage, VmPrefs};
//! use torque_ts_vm::vm::Vm;
//!
//! #[derive(Prefs)]
//! #[prefs(prefix = "$Pref::Pad")]
//! struct PadPrefs {
//!     /// How far the stick must move before it counts.
//!     #[pref(default = 0.15, range = 0.0..=0.9)]
//!     dead_zone: f32,
//!     #[pref(default = true)]
//!     invert_y: bool,
//! }
//!
//! #[derive(PluginId)]
//! struct PadPlugin;
//! impl Plugin for PadPlugin {
//!     fn build(&self, engine: &mut EngineBuilder) {
//!         engine.script().prefs::<PadPrefs>();
//!     }
//! }
//!
//! let engine = EngineBuilder::new().add_plugins(PadPlugin).build()?;
//! let mut vm = Vm::new();
//! for &stage in ServerStage::ALL {
//!     engine.replay(stage, &mut vm)?;
//! }
//! // The defaults are set before any script runs.
//! assert_eq!(vm.global("$Pref::Pad::DeadZone").as_deref(), Some(&b"0.15"[..]));
//! assert_eq!(vm.prefs::<PadPrefs>().dead_zone, 0.15);
//! // A script overrides one; Rust reads it back clamped to the range.
//! vm.eval(b"$Pref::Pad::DeadZone = 4; $Pref::Pad::InvertY = 0;")?;
//! let pad = vm.prefs::<PadPrefs>();
//! assert_eq!((pad.dead_zone, pad.invert_y), (0.9, false));
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

use torque_ts_vm::convert::FromScript;
use torque_ts_vm::text::{atof, atoi};
use torque_ts_vm::vm::{Vm, VmError};

use crate::Engine;
use crate::param::{Cx, Param, ParamError, ParamKind};
use crate::script::{EntryKind, ScriptRegistry};

/// How a pref's global is read: what a reference page shows beside the default.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PrefKind {
    /// A truth value (`dAtob`).
    Bool,
    /// A whole number (`dAtoi`).
    Int,
    /// A number (`dAtof`).
    Float,
    /// Text.
    Text,
}

/// A field type a pref can have: how the global's text reads as the type, and how a value is clamped to the pref's
/// range. The types are the ones scripts pass as arguments: `i32`, `u32`, `f32`, `bool` and `String`.
#[diagnostic::on_unimplemented(
    message = "`{Self}` cannot be a pref: use `i32`, `u32`, `f32`, `bool` or `String`",
    label = "not a pref type"
)]
pub trait PrefType: Sized {
    /// What the global holds.
    const KIND: PrefKind;
    /// The type a `default = ..` literal has: the type itself, or text for a `String`.
    type Literal;
    /// The value the global's text reads as, by the rules of a native's arguments.
    fn from_text(text: &[u8]) -> Self;
    /// The value clamped to `min` and `max` (a number's); a type without a range returns it as it is.
    fn clamp_to(self, min: Option<f64>, max: Option<f64>) -> Self {
        let _ = (min, max);
        self
    }
}

/// The whole number `value` clamped to the range `min` and `max` allow (an end left out leaves it open).
fn clamp_whole(value: i64, min: Option<f64>, max: Option<f64>) -> i64 {
    let low = min.map_or(i64::MIN, |m| m.ceil() as i64);
    let high = max.map_or(i64::MAX, |m| m.floor() as i64);
    if low > high {
        return value;
    }
    value.clamp(low, high)
}

impl PrefType for i32 {
    const KIND: PrefKind = PrefKind::Int;
    type Literal = i32;
    fn from_text(text: &[u8]) -> i32 {
        i32::from_script(text).unwrap_or_default()
    }
    fn clamp_to(self, min: Option<f64>, max: Option<f64>) -> i32 {
        clamp_whole(i64::from(self), min, max) as i32
    }
}

impl PrefType for u32 {
    const KIND: PrefKind = PrefKind::Int;
    type Literal = u32;
    fn from_text(text: &[u8]) -> u32 {
        u32::from_script(text).unwrap_or_default()
    }
    fn clamp_to(self, min: Option<f64>, max: Option<f64>) -> u32 {
        clamp_whole(i64::from(self), min, max) as u32
    }
}

impl PrefType for f32 {
    const KIND: PrefKind = PrefKind::Float;
    type Literal = f32;
    fn from_text(text: &[u8]) -> f32 {
        f32::from_script(text).unwrap_or_default()
    }
    fn clamp_to(self, min: Option<f64>, max: Option<f64>) -> f32 {
        let low = min.map_or(self, |m| self.max(m as f32));
        max.map_or(low, |m| low.min(m as f32))
    }
}

impl PrefType for bool {
    const KIND: PrefKind = PrefKind::Bool;
    type Literal = bool;
    fn from_text(text: &[u8]) -> bool {
        bool::from_script(text).unwrap_or_default()
    }
}

impl PrefType for String {
    const KIND: PrefKind = PrefKind::Text;
    type Literal = &'static str;
    fn from_text(text: &[u8]) -> String {
        String::from_script(text).unwrap_or_default()
    }
}

/// One pref, as `#[derive(Prefs)]` declares it: a constant.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct PrefDecl {
    /// The global's name, with its `$`: `$Pref::Pad::DeadZone`.
    pub name: &'static str,
    /// The Rust field's name.
    pub field: &'static str,
    /// How the global is read.
    pub kind: PrefKind,
    /// The doc comment, whole.
    pub doc: &'static str,
    /// The default, as the text the global is set to.
    pub default: &'static str,
    /// The lowest value a read gives, when the pref has a range.
    pub min: Option<f64>,
    /// The highest value a read gives, when the pref has a range.
    pub max: Option<f64>,
}

impl PrefDecl {
    /// The range as a reference shows it (`0 to 0.9`), when the pref has one.
    pub fn range_text(&self) -> Option<String> {
        let (min, max) = (self.min?, self.max?);
        Some(format!("{min} to {max}"))
    }

    /// Sets the global to the default, as a script's `$Pref::Pad::DeadZone = 0.15;` does (a number stays a number, so
    /// `export` writes it without quotes).
    pub(crate) fn install(&self, vm: &mut Vm) {
        let text = self.default.as_bytes();
        match self.kind {
            PrefKind::Text => vm.set_global(self.name, self.default),
            PrefKind::Float => vm.set_global_float(self.name.as_bytes(), atof(text)),
            PrefKind::Bool | PrefKind::Int => match u32::try_from(atoi(text)) {
                Ok(whole) => vm.set_global_uint(self.name.as_bytes(), whole),
                Err(_) => vm.set_global_float(self.name.as_bytes(), f64::from(atoi(text))),
            },
        }
    }
}

/// A struct of prefs: implemented by `#[derive(Prefs)]`, registered with
/// [`ScriptRegistry::prefs`](crate::ScriptRegistry::prefs) and read back with [`VmPrefs::prefs`].
pub trait Prefs: Sized + 'static {
    /// The globals' common start: `$Pref::Pad`.
    const PREFIX: &'static str;
    /// The prefs, in field order.
    const PREFS: &'static [PrefDecl];

    /// Builds the struct from what `read` gives for each pref: the global's text, or `None` for the default. Each value is
    /// clamped to its range.
    fn read_with(read: &mut dyn FnMut(&PrefDecl) -> Option<Vec<u8>>) -> Self;

    /// The struct of the defaults.
    fn defaults() -> Self {
        Self::read_with(&mut |_| None)
    }

    /// The struct of what `vm`'s globals hold now: a global that is not set reads as its default.
    fn read(vm: &Vm) -> Self {
        Self::read_with(&mut |decl| vm.global(decl.name))
    }
}

/// One pref's value for [`Prefs::read_with`]: `read`'s text (the default's when it gives none) as a `T`, clamped to the
/// pref's range. What `#[derive(Prefs)]` calls for each field.
pub fn read_pref<T: PrefType>(
    decl: &PrefDecl,
    read: &mut dyn FnMut(&PrefDecl) -> Option<Vec<u8>>,
) -> T {
    let text = read(decl);
    let text = text.as_deref().unwrap_or(decl.default.as_bytes());
    T::from_text(text).clamp_to(decl.min, decl.max)
}

/// Reads prefs off a script engine: `vm.prefs::<PadPrefs>()`.
pub trait VmPrefs {
    /// The struct `T` of the prefs' current values (see [`Prefs::read`]).
    fn prefs<T: Prefs>(&self) -> T;
}

impl VmPrefs for Vm {
    fn prefs<T: Prefs>(&self) -> T {
        T::read(self)
    }
}

/// The current values of the prefs `T`, read when the function is called: a parameter for a recipe or a native.
///
/// It reads the script engine, so it counts as a [`VmRef`](crate::VmRef): beside a [`VmMut`](crate::VmMut) it does not
/// compile (take the struct from the `VmMut` with [`VmPrefs::prefs`]).
///
/// ```
/// use torque_engine::{EngineBuilder, Pref, Prefs, ServerStage};
/// use torque_ts_vm::vm::Vm;
///
/// #[derive(Prefs)]
/// #[prefs(prefix = "$Pref::Hud")]
/// struct HudPrefs {
///     #[pref(default = 1.0, range = 0.5..=2.0)]
///     scale: f32,
/// }
///
/// /// the HUD's size, as the player set it
/// #[torque_engine::function(ns = "Hud")]
/// fn scale(prefs: Pref<HudPrefs>) -> f32 {
///     prefs.scale
/// }
///
/// let mut builder = EngineBuilder::new();
/// builder.script().prefs::<HudPrefs>().natives(torque_engine::natives![scale]);
/// let engine = builder.build()?;
/// let mut vm = Vm::new();
/// for &stage in ServerStage::ALL {
///     engine.replay(stage, &mut vm)?;
/// }
/// assert_eq!(vm.eval(b"return Hud::scale();")?, b"1");
/// vm.eval(b"$Pref::Hud::Scale = 9;")?;
/// assert_eq!(vm.eval(b"return Hud::scale();")?, b"2");
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub struct Pref<T>(pub T);

impl<T> std::ops::Deref for Pref<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T: Prefs> Param for Pref<T> {
    type Item<'a> = Pref<T>;
    type State = ();
    const KIND: ParamKind = ParamKind::VmRead;
    fn fetch<'a>(cx: &'a Cx<'a>, _: &'a ()) -> Result<Pref<T>, ParamError> {
        cx.share_vm()
            .map(|vm| Pref(T::read(vm)))
            .ok_or(ParamError::VmTaken)
    }
}

impl ScriptRegistry<'_> {
    /// Declares the prefs of `T`: each global is set to its default in every script engine of the side, before the
    /// scripts run. A global another declaration already took fails the build
    /// ([`BuildError::DuplicateScriptName`](crate::BuildError::DuplicateScriptName)). See [`Prefs`].
    pub fn prefs<T: Prefs>(&mut self) -> &mut Self {
        for decl in T::PREFS {
            self.pref(decl);
        }
        self
    }

    /// Declares one pref by hand (see [`prefs`](Self::prefs)).
    pub fn pref(&mut self, decl: &'static PrefDecl) -> &mut Self {
        let key = decl.name.to_ascii_lowercase();
        let effects = self.effects;
        self.declare(key, "pref", decl.name.to_string(), || {
            EntryKind::Pref(decl, effects)
        })
    }
}

impl Engine {
    /// Every pref a plugin declared, in the order they were declared: what a reference page lists.
    pub fn prefs(&self) -> impl Iterator<Item = &'static PrefDecl> + '_ {
        self.script_entries().filter_map(|entry| match entry {
            EntryKind::Pref(decl, _) => Some(*decl),
            _ => None,
        })
    }

    /// Writes the registered prefs of `vm`, as `export` writes globals (script, `name = value;` lines): to the console
    /// when `file` is `None`, else to the file (replacing it). One scan, sorted by name ignoring case: the lines, and
    /// their order, are those of `export("$Pref::Pad::*")` for the same names. The prefs a script engine does not have
    /// are left out.
    pub fn export_prefs(&self, vm: &mut Vm, file: Option<&[u8]>) -> Result<(), VmError> {
        let names: Vec<&[u8]> = self.prefs().map(|decl| decl.name.as_bytes()).collect();
        vm.export_named_variables(&names, file, false)
    }
}

#[cfg(test)]
#[path = "prefs_tests.rs"]
mod tests;
