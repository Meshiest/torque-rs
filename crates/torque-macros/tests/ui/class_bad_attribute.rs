use torque_engine::EngineClass;

#[derive(EngineClass)]
#[class(kind = "datablock")]
struct UnknownKey {
    a: i32,
}

#[derive(EngineClass)]
#[class(name = "9Lives")]
struct BadName {
    a: i32,
}

#[derive(EngineClass)]
struct UnknownFieldKey {
    #[field(colour = 1)]
    a: i32,
}

#[derive(EngineClass)]
struct NotALiteral {
    #[field(default = 1 + 1)]
    a: i32,
}

#[derive(EngineClass)]
struct SameField {
    max_length: f32,
    #[field(name = "MAXLENGTH")]
    other: f32,
}

#[derive(EngineClass)]
struct Tuple(i32);

fn main() {}
