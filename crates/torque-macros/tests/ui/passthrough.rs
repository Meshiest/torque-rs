use torque_macros::passthrough;

#[passthrough]
fn set_dead_zone(zone: f32) -> f32 {
    zone.clamp(0.0, 0.9)
}

#[passthrough]
struct Pad {
    dead_zone: f32,
}

fn main() {
    let pad = Pad { dead_zone: set_dead_zone(2.0) };
    assert_eq!(pad.dead_zone, 0.9);
}
