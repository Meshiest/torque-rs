//! The engine's own light and sky classes on a server with TGE's game classes ([`torque_server::game::install`]): their ghost
//! classes past TGE's ids, a TimeOfDay that ticks its day and calls its script back, and a SpotLight switched and mounted from
//! script.

use std::cell::RefCell;
use std::rc::Rc;

use torque_ghosts::light::ENGINE_OBJECT_CLASSES;
use torque_ghosts::tge::TgeClass;
use torque_net::classes::Kind;
use torque_net::ghost::net_flags;
use torque_server::net::codecs::net_codecs;
use torque_server::server::{Server, TICK_MS};
use torque_server::state::ServerState;
use torque_ts_vm::host::MemFiles;
use torque_ts_vm::vm::Vm;

fn game_server() -> Server {
    let mut vm = torque_game::testing::vm();
    let files = MemFiles::new();
    let b = [-1.0, -2.0, 0.0, 1.0, 2.0, 1.0];
    let car = torque_shapes::synth::hull_shape(b, &[("detail2", b)], &["mount0", "mount1"], &[]);
    files.insert("car.dts", &torque_shapes::write::write_dts(&car));
    vm.set_file_host(files);
    let state = Rc::new(RefCell::new(ServerState::default()));
    torque_server::game::install(&mut vm, &state);
    let mut s = Server::with_state(vm, state);
    s.run(TICK_MS, |_| false);
    s
}

fn global(s: &Server, name: &str) -> String {
    String::from_utf8_lossy(&s.vm.global(name).unwrap_or_default()).into_owned()
}

fn eval(s: &mut Server, code: &str) -> String {
    String::from_utf8_lossy(&s.vm.eval(code.as_bytes()).unwrap()).into_owned()
}

#[test]
fn the_engines_classes_take_the_ids_past_tges_and_are_scoped_always() {
    let vm = Vm::new();
    let codecs = net_codecs(&vm);
    let tge = codecs.object_classes().count(Kind::Object) as u32;
    for (k, name) in ENGINE_OBJECT_CLASSES.iter().enumerate() {
        let info = codecs.ghost_class(name).expect(name);
        assert_eq!(info.class_id, tge + k as u32, "{name}");
        assert!(info.class_id < 1 << codecs.ghost_class_bits());
        assert_eq!(
            info.flags,
            net_flags::GHOSTABLE | net_flags::SCOPE_ALWAYS,
            "{name}"
        );
        assert_eq!(
            TgeClass::of_code(info.code).map(TgeClass::name),
            Some(*name)
        );
    }
}

#[test]
fn a_time_of_day_ticks_its_day_and_calls_its_events() {
    let mut s = game_server();
    eval(
        &mut s,
        "function TimeOfDay::onTimeEvent(%this, %id, %time, %elevation) { $events = $events SPC %id; }
function TimeOfDay::onAnimateStart(%this) { $started = 1; }
function TimeOfDay::onAnimateDone(%this) { $done = 1; }
new TimeOfDay(Day) { dayLength = 10; startTime = 0.2; azimuthOverride = 1; };
Day.addTimeOfDayEvent(90, \"noon\");
Day.addTimeOfDayEvent(180, \"sunset\");",
    );
    assert_eq!(
        eval(&mut s, "return Day.time;"),
        "0.2",
        "starts at its start time"
    );
    // Two seconds of a ten-second day: past noon (0.25).
    for _ in 0..63 {
        s.run(TICK_MS, |_| false);
    }
    let time: f32 = eval(&mut s, "return Day.time;").parse().unwrap();
    assert!((0.39..0.41).contains(&time), "{time}");
    assert_eq!(global(&s, "$events").trim(), "noon");
    // setTimeOfDay passes sunset.
    eval(&mut s, "Day.setTimeOfDay(0.6);");
    assert_eq!(global(&s, "$events").trim(), "noon sunset");
    // Stopped, the time holds; animate runs it to its target and ends.
    eval(&mut s, "Day.setPlay(0); Day.animate(270, 360);");
    assert_eq!(global(&s, "$started"), "1");
    for _ in 0..10 {
        s.run(TICK_MS, |_| false);
    }
    assert_eq!(global(&s, "$done"), "1");
    assert_eq!(eval(&mut s, "return Day.time;"), "0.75");
    assert_eq!(eval(&mut s, "return Day.play;"), "0");
}

#[test]
fn a_spot_light_switches_and_rides_a_shape() {
    let mut s = game_server();
    eval(
        &mut s,
        "datablock StaticShapeData(Car) { shapeFile = \"car.dts\"; };
new StaticShape(Body) { dataBlock = Car; };
new SpotLight(Lamp) { range = 30; innerAngle = 30; outerAngle = 50; castShadows = 1; };",
    );
    assert_eq!(eval(&mut s, "return Lamp.isEnabled;"), "1");
    eval(&mut s, "Lamp.setLightEnabled(false);");
    assert_eq!(eval(&mut s, "return Lamp.isEnabled;"), "0");
    eval(&mut s, "Lamp.setLightEnabled(!Lamp.isEnabled);");
    assert_eq!(eval(&mut s, "return Lamp.isEnabled;"), "1");
    let (lamp, body) = (
        s.vm.objects().find(b"Lamp").unwrap(),
        s.vm.objects().find(b"Body").unwrap(),
    );
    assert_eq!(eval(&mut s, "return Body.mountObject(Lamp, 1);"), "1");
    assert_eq!(
        torque_game::scene::lights::spot_mount(&s.vm, lamp),
        Some((body, 1))
    );
    // The shape's own mounts are untouched.
    assert_eq!(torque_game::sim::mount::riders(&s.vm, body), &[] as &[u32]);
    assert_eq!(eval(&mut s, "return Body.unmountObject(Lamp);"), "1");
    assert_eq!(torque_game::scene::lights::spot_mount(&s.vm, lamp), None);
}
