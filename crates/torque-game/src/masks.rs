//! The type masks a host's movers query and test with (`collide::TypeMasks`): a host whose classes use other bits than TGE's
//! names its own ([`set_type_masks`], a `Vm` extension); without one they are TGE's.

use std::rc::Rc;

use collide::TypeMasks;
use ts_vm::vm::Vm;

struct Masks(&'static TypeMasks);

/// The host's masks for every later [`type_masks`] on `vm`.
pub fn set_type_masks(vm: &mut Vm, masks: &'static TypeMasks) {
    vm.set_extension(Rc::new(Masks(masks)));
}

/// The masks `vm`'s game named, else TGE's.
pub fn type_masks(vm: &Vm) -> &'static TypeMasks {
    vm.extension::<Masks>().map_or(&TypeMasks::TGE, |m| m.0)
}
