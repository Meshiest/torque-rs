use torque_engine::NetMessage;

#[derive(Debug, Clone, PartialEq, Default, NetMessage)]
struct Open {
    data: String,
}

fn main() {}
