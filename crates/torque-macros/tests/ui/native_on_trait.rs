use torque_engine::methods;

#[methods(class = "Tool")]
trait Tool {
    fn use_on(&mut self, target: i32) -> bool;
}

fn main() {}
