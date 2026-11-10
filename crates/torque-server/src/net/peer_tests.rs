use std::rc::Rc;

use super::*;

fn engine_mode(vm: &mut Vm) {
    vm.set_extension(Rc::new(ProtocolMode(ProtocolKind::Engine)));
}

#[test]
fn a_peer_of_torques_protocol_is_the_engines_only_when_marked_or_the_server_says_so() {
    let mut vm = Vm::new();
    assert_eq!(PeerKind::of(&vm, false), PeerKind::Tge);
    assert_eq!(PeerKind::of(&vm, true), PeerKind::EnginePeer);
    vm.set_global(ENGINE_CLASSES_GLOBAL, "0");
    assert_eq!(PeerKind::of(&vm, false), PeerKind::Tge);
    vm.set_global(ENGINE_CLASSES_GLOBAL, "1");
    assert_eq!(PeerKind::of(&vm, false), PeerKind::EnginePeer);
    assert_eq!(PeerKind::of(&vm, true), PeerKind::EnginePeer);
}

#[test]
fn on_a_server_of_the_engines_protocol_every_peer_is_the_engines() {
    let mut vm = Vm::new();
    engine_mode(&mut vm);
    assert_eq!(ProtocolMode::of(&vm), ProtocolKind::Engine);
    for marked in [false, true] {
        assert_eq!(PeerKind::of(&vm, marked), PeerKind::Engine);
    }
}

#[test]
fn only_a_peer_of_torque_is_kept_from_the_engines_classes() {
    assert!(!PeerKind::Tge.sees_engine_classes());
    assert!(PeerKind::EnginePeer.sees_engine_classes());
    assert!(PeerKind::Engine.sees_engine_classes());
    assert_eq!(ProtocolMode::of(&Vm::new()), ProtocolKind::Tge);
}
