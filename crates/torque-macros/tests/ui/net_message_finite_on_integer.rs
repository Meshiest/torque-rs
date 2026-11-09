use torque_engine::NetMessage;

#[derive(Debug, Clone, PartialEq, Default, NetMessage)]
struct Count {
    #[net(finite)]
    n: u32,
}

fn main() {}
