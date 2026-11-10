//! A native extension built on its own: two console functions, a global and two render passes, over the engine's C ABI.
//! The engine's tests load it from a `plugins/` folder and compare it with the same plugin compiled into the game.

use torque_plugin_sdk::{
    Call, Extension, PassFrame, Registrar, Space, Stage, Triangles, function, natives,
};

/// greets someone by name
#[function]
fn sample_greet(name: &str) -> String {
    format!("Hello, {name}!")
}

/// adds two or three whole numbers
#[function(ns = "Sample")]
fn add(a: i32, b: i32, c: Option<i32>) -> i32 {
    a.wrapping_add(b).wrapping_add(c.unwrap_or(0))
}

/// the greeting global, said twice
#[function(ns = "Sample", name = "greetingTwice")]
fn greeting_twice(call: Call) -> String {
    let greeting = String::from_utf8_lossy(&call.global("$Sample::Greeting")).into_owned();
    format!("{greeting} {greeting}")
}

/// calls the script function `add` back `n` times (the loader's bench: the cost of a call from an extension into the
/// engine)
#[cfg(feature = "bench")]
#[function(ns = "Sample", name = "callBack")]
fn call_back(call: Call, n: i32) -> i32 {
    let mut total = 0;
    for _ in 0..n {
        let sum = call.call_function("", "add", &[b"3", b"4"]).unwrap_or_default();
        total += i32::from(sum.first().is_some_and(|b| *b == b'7'));
    }
    total
}

/// writes and reads the dynamic field `count` of the object `n` times (the loader's bench: field calls)
#[cfg(feature = "bench")]
#[function(ns = "Sample", name = "fieldBack")]
fn field_back(call: Call, object: i32, n: i32) -> i32 {
    let object = object as u32;
    let mut total = 0;
    for i in 0..n {
        call.set_field(object, b"count", b"", i.to_string().as_bytes());
        let value = call.field(object, b"count", b"").unwrap_or_default();
        total += i32::from(!value.is_empty());
    }
    total
}

/// A square in the world, above the floor: drawn at the opaque stage, depth tested.
fn world_square(_frame: &PassFrame, out: &mut Triangles) {
    let corners = [
        [-2.0, 0.0, 1.0],
        [2.0, 0.0, 1.0],
        [2.0, 0.0, 5.0],
        [-2.0, 0.0, 5.0],
    ];
    out.quad(Space::World, corners, [1.0, 0.35, 0.08, 1.0]);
}

/// A badge in the picture's top left corner, over everything.
fn badge(_frame: &PassFrame, out: &mut Triangles) {
    let corners = [
        [-0.95, 0.6, 0.0],
        [-0.6, 0.6, 0.0],
        [-0.6, 0.95, 0.0],
        [-0.95, 0.95, 0.0],
    ];
    out.quad(Space::Clip, corners, [0.1, 0.6, 1.0, 0.8]);
}

#[derive(Default)]
struct Sample;

impl Extension for Sample {
    const ID: &'static str = "example.native-sample";
    const VERSION: &'static str = "0.1.0";

    fn build(&self, ext: &mut Registrar) {
        ext.script()
            .natives(natives![sample_greet, add, greeting_twice])
            .global("$Sample::Greeting", "Hello");
        #[cfg(feature = "bench")]
        ext.script().natives(natives![call_back, field_back]);
        ext.render()
            .pass(Stage::Opaque, || world_square)
            .pass(Stage::Overlay, || badge);
    }
}

torque_plugin_sdk::export!(Sample);
