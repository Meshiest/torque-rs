//! A `Vm` for tests that need the game classes. Behind the `testing` feature (always built for this crate's own tests); a
//! crate takes it as a dev-dependency only.

use ts_vm::vm::Vm;

/// A new `Vm` with the game classes' specs installed ([`crate::specs::install`]).
pub fn vm() -> Vm {
    let mut vm = Vm::new();
    crate::specs::install(&mut vm);
    vm
}
