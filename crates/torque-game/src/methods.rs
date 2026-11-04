//! Console methods of the game classes made real: [`register_method`] swaps the counted stub a class table installed
//! (`ts_vm::classes::install_specs`) for a native **in place**, with the stub's argument counts and usage, so that a call with
//! the wrong number of arguments prints what the table says whoever implements the method, and a call site bound to the stub
//! sees the new native.
//!
//! **Where the counts come from.** The table a host installed is its own: the host names where its method specs are read
//! ([`set_spec_source`], a `Vm` extension), and without one they are this crate's [`crate::specs`].

use std::rc::Rc;

use ts_vm::classes::{ClassSpec, MethodSpec};
use ts_vm::vm::{Native, NativeCall, Vm, VmError};

/// Finds the spec of `method` in `class` (names compared ignoring case): the class's name as the table spells it, and the
/// method.
pub type SpecSource = fn(class: &str, method: &str) -> Option<(&'static str, &'static MethodSpec)>;

/// The `Vm` extension that holds a host's [`SpecSource`].
struct Source(SpecSource);

/// Reads method specs from `source` for every later [`register_method`] on `vm`.
pub fn set_spec_source(vm: &mut Vm, source: SpecSource) {
    vm.set_extension(Rc::new(Source(source)));
}

/// The method of `class` in a table of class specs (names compared ignoring case).
pub fn find_in(
    table: &'static [ClassSpec],
    class: &str,
    method: &str,
) -> Option<(&'static str, &'static MethodSpec)> {
    let class = table.iter().find(|c| c.name.eq_ignore_ascii_case(class))?;
    let method = class
        .methods
        .iter()
        .find(|m| m.name.eq_ignore_ascii_case(method))?;
    Some((class.name, method))
}

/// The spec of `class::method` from the host's source, else from this crate's specs.
pub fn spec(vm: &Vm, class: &str, method: &str) -> Option<(&'static str, &'static MethodSpec)> {
    match vm.extension::<Source>() {
        Some(source) => (source.0)(class, method),
        None => find_in(crate::specs::CLASSES, class, method),
    }
}

/// Makes `f` the implementation of `class::method`, with the spec's argument counts and usage. False, and nothing registered,
/// when the spec has no such method (a typo is a failed test, not a second function that nothing calls). It is charged the
/// bytes of its arguments before `f` runs (they are scanned or copied): a run that cannot pay ends with the step limit and `f`
/// does not run.
pub fn register_method(
    vm: &mut Vm,
    class: &str,
    method: &str,
    f: impl Fn(&mut Vm, &NativeCall) -> Native + 'static,
) -> bool {
    let Some((class, spec)) = spec(vm, class, method) else {
        return false;
    };
    vm.register_in(
        class,
        spec.name,
        spec.min,
        spec.max,
        spec.usage,
        move |vm, c| {
            let bytes = c
                .argv
                .iter()
                .skip(2)
                .map(Vec::len)
                .fold(0usize, usize::saturating_add);
            if bytes > 0 && !vm.charge_work(u64::try_from(bytes).unwrap_or(u64::MAX)) {
                return Native::Fail(VmError::StepLimit);
            }
            f(vm, c)
        },
    );
    true
}
