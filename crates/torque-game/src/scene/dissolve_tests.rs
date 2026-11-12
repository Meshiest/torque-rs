use super::*;

fn shape(vm: &mut Vm) -> ObjId {
    vm.eval(b"new StaticShape(Statue); new ParticleEmitterData(Sand);")
        .expect("the objects");
    vm.objects().find(b"Statue").expect("the statue")
}

fn eaten(vm: &mut Vm) -> f32 {
    vm.eval(b"$out = Statue.getDissolve();").expect("a call");
    String::from_utf8_lossy(&vm.global("$out").expect("the result"))
        .parse()
        .expect("a number")
}

#[test]
fn a_script_starts_a_dissolve_and_its_values_are_held_to_the_wire() {
    let mut vm = crate::testing::vm();
    register(&mut vm);
    let id = shape(&mut vm);
    assert_eq!(dissolve_of(&vm, id), None);
    assert_eq!(eaten(&mut vm), 0.0);
    vm.eval(b"Statue.dissolve(1500, 1, \"0.2 0.6 1 40\", 3, Sand, 1);")
        .expect("a call");
    let d = dissolve_of(&vm, id).expect("a dissolve");
    let sand = vm.objects().find(b"Sand");
    assert!(d.running && d.out && d.tint);
    assert_eq!((d.serial, d.duration_ms, d.emitter), (1, 1500, sand));
    assert_eq!(
        (d.edge_color, d.glow, d.edge),
        ([0.2, 0.6, 1.0], MAX_GLOW, 1.0)
    );
    // The look's defaults until a script sets them; then kept for later dissolves.
    assert_eq!((d.cell, d.sweep, d.density), (0.25, 0.0, 64.0));
    vm.eval(b"Statue.setDissolveLook(99, -3, 1e9);")
        .expect("a call");
    let d = dissolve_of(&vm, id).expect("a dissolve");
    assert_eq!((d.cell, d.sweep, d.density), (MAX_CELL, -1.0, MAX_DENSITY));
    // An emitter that is not a ParticleEmitterData names none.
    vm.eval(b"new SimObject(NotAnEmitter); Statue.dissolve(100, 1, \"\", \"\", NotAnEmitter);")
        .expect("a call");
    assert_eq!(dissolve_of(&vm, id).expect("a dissolve").emitter, None);
    vm.eval(b"Statue.dissolve(100, 1, \"\", \"\", Statue);")
        .expect("a call");
    assert_eq!(dissolve_of(&vm, id).expect("a dissolve").emitter, None);
    // A materialize with only a time: the last look, the default way out unless told, a new serial.
    vm.eval(b"Statue.dissolve(-5, 0);").expect("a call");
    let d = dissolve_of(&vm, id).expect("a dissolve");
    assert!(!d.out && !d.tint && d.emitter.is_none());
    assert_eq!((d.serial, d.duration_ms, d.cell), (4, 0, MAX_CELL));
    // No time: materialized at once.
    assert_eq!(eaten(&mut vm), 0.0);
    vm.eval(b"Statue.dissolve(0);").expect("a call");
    assert_eq!(eaten(&mut vm), 1.0);
    vm.eval(b"Statue.clearDissolve();").expect("a call");
    let d = dissolve_of(&vm, id).expect("kept");
    assert!(!d.running);
    assert_eq!(d.serial, 5);
    assert_eq!(eaten(&mut vm), 0.0);
}

#[test]
fn the_share_eaten_runs_with_the_sim_clock_both_ways() {
    let d = Dissolve {
        running: true,
        duration_ms: 2000,
        start_ms: 1000,
        ..Dissolve::default()
    };
    assert_eq!(d.eaten_at(500), 0.0);
    assert_eq!(d.eaten_at(1500), 0.25);
    assert_eq!(d.eaten_at(9000), 1.0);
    let back = Dissolve { out: false, ..d };
    assert_eq!(back.eaten_at(1500), 0.75);
    assert_eq!(back.eaten_at(9000), 0.0);
}
