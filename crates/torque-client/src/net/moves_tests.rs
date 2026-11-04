use super::*;

#[test]
fn the_clamp_quantises_as_tge_does() {
    assert_eq!(clamp_axis(0.0), 16);
    assert_eq!(clamp_axis(1.0), 32);
    assert_eq!(clamp_axis(-1.0), 0);
    assert_eq!(clamp_axis(5.0), 32);
    assert_eq!(clamp_axis(-5.0), 0);
    assert_eq!(clamp_axis(0.5), 24);
    assert_eq!(clamp_axis(-0.03), 15);
    assert_eq!(clamp_axis(f32::NAN), 0);
    assert_eq!(clamp_angle(0.0), 0);
    assert_eq!(clamp_angle(0.01), 104);
    assert_eq!(clamp_angle(-0.01), (-104i32 as u32) & 0xffff);
    assert_eq!(
        clamp_angle(10.0),
        38767,
        "a whole turn wraps: 104303 less 65536"
    );
    assert_eq!(clamp_angle(-10.0), (-104303i32 as u32) & 0xffff);
    assert_eq!(clamp_angle(f32::NAN), 0);
    assert_eq!(clamp_angle(f32::INFINITY), 0);
}

#[derive(Debug)]
struct Halves;

impl MoveRules for Halves {
    fn angle(&self, turn: f32, plus: f32, minus: f32) -> f32 {
        (turn + plus - minus) * 0.5
    }
}

#[test]
fn the_move_is_made_by_the_rules_the_vm_holds() {
    let mut vm = Vm::new();
    vm.set_global("$mvYaw", "0.02");
    vm.set_global("$mvForwardAction", "1");
    let mut last = TriggerMemory::default();
    let tge = next_move(&mut vm, &mut last, 0, 0).expect("a move");
    assert_eq!(
        vm.global_f32("$mvYaw"),
        Some(0.0),
        "the mouse's turn is taken"
    );
    assert_eq!((tge.pyaw, tge.py), (clamp_angle(0.02), 32));
    set_move_rules(&mut vm, Rc::new(Halves));
    vm.set_global("$mvYaw", "0.02");
    let half = next_move(&mut vm, &mut last, 0, 0).expect("a move");
    assert_eq!(half.pyaw, clamp_angle(0.01));
    assert!(next_move(&mut vm, &mut last, MAX_PENDING_MOVES + 1, 0).is_none());
}
