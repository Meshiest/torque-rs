use torque_engine::function;

/// A free function has no object.
#[function]
fn give_ammo(&mut self, amount: i32) -> i32 {
    amount
}

fn main() {}
