use torque_engine::function;

/// An optional argument before a required one.
#[function]
fn backwards(first: Option<i32>, second: i32) -> i32 {
    first.unwrap_or(0) + second
}

fn main() {}
