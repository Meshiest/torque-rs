use std::collections::HashMap;

use torque_engine::NetMessage;

#[derive(Debug, Clone, PartialEq, Default, NetMessage)]
struct Table {
    entries: HashMap<u32, u32>,
}

fn main() {}
