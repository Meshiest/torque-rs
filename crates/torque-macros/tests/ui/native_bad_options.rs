use torque_engine::{function, methods};

#[function(namespace = "Pad")]
fn unknown_key() {}

#[function(name = "set dead zone")]
fn bad_name() {}

struct Tools;

#[methods(class = "Player")]
impl Tools {
    /// Only `pub` fns of an inherent block become methods.
    #[torque(name = "hidden")]
    fn private_one() {}

    pub fn by_value(self) {}
}

fn main() {}
