use torque_engine::NetMessage;
use torque_ts_vm::sim::ObjId;

#[derive(Debug, Clone, PartialEq, Default, NetMessage)]
struct Pick {
    target: ObjId,
}

fn main() {}
