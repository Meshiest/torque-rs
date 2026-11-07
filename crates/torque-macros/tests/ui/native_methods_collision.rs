use torque_engine::methods;

struct Tools;

#[methods(class = "Player")]
impl Tools {
    pub fn is_armed() -> bool {
        true
    }

    /// Names compare without case, as scripts call them.
    #[torque(name = "IsArmed")]
    pub fn armed_too() -> bool {
        false
    }
}

fn main() {}
