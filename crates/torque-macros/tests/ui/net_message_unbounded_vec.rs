use torque_engine::NetMessage;

#[derive(Debug, Clone, PartialEq, Default, NetMessage)]
struct Open {
    items: Vec<u8>,
}

fn main() {}
