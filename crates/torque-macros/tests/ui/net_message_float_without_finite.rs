use torque_engine::NetMessage;

#[derive(Debug, Clone, PartialEq, Default, NetMessage)]
struct Move {
    to: [f32; 3],
}

fn main() {}
