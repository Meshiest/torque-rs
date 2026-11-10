use torque_engine::NetMessage;
use torque_ts_vm::sim::ObjId;

#[derive(Debug, Clone, PartialEq, Default, NetMessage)]
struct Pick {
    #[net(max_len = 4)]
    targets: Vec<Option<ObjId>>,
}

fn main() {}
