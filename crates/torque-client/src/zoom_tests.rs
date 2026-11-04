use super::*;

#[test]
fn the_zoom_moves_ninety_degrees_per_speed_and_stops_at_the_target() {
    let mut z = Zoom {
        speed: 200,
        last: 1000.0,
        ..Zoom::default()
    };
    z.target = 10.0;
    z.step(1100.0);
    assert_eq!(z.camera, 45.0, "100 ms of a 90 degree, 200 ms zoom");
    z.step(1200.0);
    assert_eq!(z.camera, 10.0, "stops at the target");
    z.target = 90.0;
    z.step(1200.0);
    assert_eq!(z.camera, 90.0, "no time passed: a snap");
    assert_eq!(clamp_fov(500.0), 179.0);
    assert_eq!(clamp_fov(0.5), 1.0);
    assert_eq!(clamp_fov(f32::NAN), 1.0);
}

#[test]
fn the_connection_sends_the_fov_when_its_whole_part_changes() {
    let mut c = ConnFov::default();
    c.set(90.4);
    assert_eq!(c.take(), None, "90.4 is 90 as a whole");
    c.set(45.5);
    assert_eq!(c.take(), Some(45));
    assert_eq!(c.take(), None);
    c.set(400.0);
    assert_eq!((c.camera, c.take()), (179.0, Some(179)));
}

#[derive(Debug)]
struct Floor;

impl ZoomRules for Floor {
    fn min_speed(&self) -> i32 {
        150
    }
}

#[test]
fn the_zoom_speed_is_held_to_its_range() {
    assert_eq!(
        zoom_speed(-5, zoom_rules(&Vm::new()).min_speed()),
        0,
        "TGE's floor: a snap"
    );
    assert_eq!(zoom_speed(5000, MIN_ZOOM_SPEED), MAX_ZOOM_SPEED);
    let mut vm = Vm::new();
    set_zoom_rules(&mut vm, Rc::new(Floor));
    assert_eq!(zoom_speed(5, zoom_rules(&vm).min_speed()), 150);
}
