use torque_engine::Prefs;

#[derive(Prefs)]
struct NoPrefix {
    #[pref(default = 1)]
    a: i32,
}

#[derive(Prefs)]
#[prefs(prefix = "Pref::Pad")]
struct BadPrefix {
    #[pref(default = 1)]
    a: i32,
}

#[derive(Prefs)]
#[prefs(prefix = "$Pref::Pad")]
struct UnknownKey {
    #[pref(speed = 2)]
    a: i32,
}

#[derive(Prefs)]
#[prefs(prefix = "$Pref::Pad")]
struct NoDefault {
    #[pref(range = 0..=5)]
    a: i32,
}

#[derive(Prefs)]
#[prefs(prefix = "$Pref::Pad")]
struct NotALiteral {
    #[pref(default = 1 + 2)]
    a: i32,
}

#[derive(Prefs)]
#[prefs(prefix = "$Pref::Pad")]
struct OpenRange {
    #[pref(default = 1, range = 0..5)]
    a: i32,
}

#[derive(Prefs)]
#[prefs(prefix = "$Pref::Pad")]
struct SameGlobal {
    #[pref(default = 1)]
    dead_zone: i32,
    #[pref(default = 1, name = "DEADZONE")]
    other: i32,
}

fn main() {}
