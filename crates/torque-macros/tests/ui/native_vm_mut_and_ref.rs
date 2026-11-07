use torque_engine::{VmMut, VmRef, function};

/// One changes what the other reads.
#[function]
fn both(_writer: VmMut, _reader: VmRef) {}

fn main() {}
