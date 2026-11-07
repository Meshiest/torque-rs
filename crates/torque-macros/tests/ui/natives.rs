//! The attributes as a plugin writes them: functions, methods on an inherent block and on a trait impl, `list = Name`,
//! and `natives!` over them all.

use torque_engine::{EngineBuilder, Local, ServerStage, This, VmMut, function, methods, natives};
use torque_ts_vm::vm::Vm;

/// the sum of two numbers
#[function(ns = "Math")]
fn add(a: i32, b: i32) -> i32 {
    a + b
}

/// How often it was called.
#[function(description = "")]
pub fn call_count(mut calls: Local<u32>) -> u32 {
    *calls += 1;
    *calls
}

#[function(usage = "shout(text); - kept as it was")]
fn shout(#[arg(name = "text")] r#in: &str, mut vm: VmMut) -> String {
    vm.set_global("$Shouted", "1");
    r#in.to_uppercase()
}

struct Crate {
    rounds: i32,
}

#[methods(class = "SimObject")]
impl Crate {
    /// takes rounds out, returns what is left
    pub fn take_rounds(&mut self, amount: i32) -> i32 {
        self.rounds -= amount;
        self.rounds
    }

    /// the object's id
    pub fn id_of(this: This) -> u32 {
        this.0
    }

    fn not_a_method(&self) -> i32 {
        self.rounds
    }
}

trait Tool {
    fn power(&self) -> i32;
}

#[methods(class = "SimObject", list = CrateTool)]
impl Tool for Crate {
    fn power(&self) -> i32 {
        self.rounds * 2
    }
}

fn main() {
    let list = natives![add, call_count, shout, Crate, CrateTool];
    assert_eq!(list.len(), 6);
    let mut builder = EngineBuilder::new();
    builder.script().natives(list);
    let engine = builder.build().expect("the engine builds");
    let mut vm = Vm::new();
    for &stage in ServerStage::ALL {
        engine.replay(stage, &mut vm).expect("the replay runs");
    }
    let listing = vm.native_listing();
    assert!(listing.contains("Math::add 3..3 Math::add(a, b) - the sum of two numbers\n"), "{listing}");
    assert!(listing.contains("callCount 1..1 callCount()\n"), "{listing}");
    assert!(listing.contains("shout 2..2 shout(text); - kept as it was\n"), "{listing}");
    assert!(listing.contains("SimObject::takeRounds 3..3 simObject.takeRounds(amount) - takes rounds out, returns what is left\n"), "{listing}");
    assert!(listing.contains("SimObject::power 2..2 simObject.power()\n"), "{listing}");
    assert_eq!(vm.eval(b"return Math::add(2, 3);").unwrap(), b"5");
    assert_eq!(vm.eval(b"callCount(); return callCount();").unwrap(), b"2");
    assert_eq!(vm.eval(b"return shout(\"hi\");").unwrap(), b"HI");
    vm.eval(b"new SimObject(Box);").unwrap();
    let id = vm.objects().find(b"Box").expect("the object");
    vm.objects_mut().get_mut(id).expect("the object").ext = Some(Box::new(Crate { rounds: 10 }));
    assert_eq!(vm.eval(b"return Box.takeRounds(3);").unwrap(), b"7");
    assert_eq!(vm.eval(b"return Box.power();").unwrap(), b"14");
    assert_eq!(vm.eval(b"return Box.idOf();").unwrap(), id.to_string().as_bytes());
    let _ = Crate { rounds: 0 }.not_a_method();
}
